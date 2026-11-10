//! Component process management: spawn (env injection) → health polling →
//! registration → supervise (exit guard).
//!
//! Lifecycle contract (architecture.md "Plane B contract · lifecycle"):
//! env injects SL_ENDPOINT/SL_TOKEN → poll GET /health (readiness probe and
//! health check share one endpoint) →
//! stop: SIGTERM (unix, nix safe wrapper) / stdin `shutdown` (windows
//! fallback) →
//! force kill after the grace timeout. stdout/stderr are captured into the
//! component's log file (`{logs_dir}/{id}.log`, append; the management page
//! reads it back via the read_component_log command).

use crate::config::{DataLayout, HEALTH_INTERVAL, STOP_GRACE};
use crate::registry::{Registry, RunningComponent};
use slsdk_rs::{ComponentManifest, ENV_ENDPOINT, ENV_TOKEN, Endpoint};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, RunEvent};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::watch;

#[derive(Debug)]
pub enum LifecycleError {
    /// Child process spawn failure (command missing / directory missing, etc.).
    Spawn {
        source: io::Error,
        /// The binary the shell tried to launch (and the arguments it would
        /// have carried) — a standalone log line must answer "what did the
        /// shell try to run".
        program: PathBuf,
        args: Vec<String>,
    },
    /// Health probe timeout: the component did not become ready within the
    /// deadline (or the id mismatched).
    HealthTimeout {
        id: String,
        /// The descriptor the shell kept polling — the dial target.
        endpoint: String,
        /// Outcome of the most recent probe (connection error or mismatched
        /// response): without it the timeout alone cannot distinguish "not
        /// listening yet" from "listening but wrong identity".
        last_probe: String,
    },
    /// Other IO failure.
    Io { source: io::Error },
    /// The namespace is already taken.
    NamespaceTaken { namespace: String },
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifecycleError::Spawn {
                source,
                program,
                args,
            } => {
                write!(
                    f,
                    "failed to spawn the component process (program={} args={args:?}): {source}",
                    program.display()
                )
            }
            LifecycleError::HealthTimeout {
                id,
                endpoint,
                last_probe,
            } => {
                write!(
                    f,
                    "component {id} health probe timed out; it did not become ready within the deadline (endpoint={endpoint}; last probe outcome: {last_probe})"
                )
            }
            LifecycleError::Io { source } => write!(f, "component management IO failure: {source}"),
            LifecycleError::NamespaceTaken { namespace } => {
                write!(f, "the namespace is already in use: {namespace}")
            }
        }
    }
}

impl std::error::Error for LifecycleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LifecycleError::Spawn { source, .. } | LifecycleError::Io { source } => Some(source),
            _ => None,
        }
    }
}

/// Random loopback TCP endpoint: bind :0 to get a system-assigned free port,
/// release it, then hand it to the component; a tiny race window exists when
/// the component binds (local loopback, accepted in v1) — failure surfaces as
/// a health timeout.
async fn allocate_tcp_endpoint() -> io::Result<Endpoint> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    drop(listener);
    Ok(Endpoint::Tcp { addr })
}

/// Allocate the component's listening descriptor. A manifest declaring
/// tcp-only transport always gets the loopback TCP descriptor on every
/// platform (a runtime with no Unix domain socket support cannot bind the
/// platform default); otherwise unix = UDS (the shell centrally manages the
/// sockets/ directory), windows = random 127.0.0.1 port — tokio/mio has no
/// Windows UDS (ecosystem-level limitation, falls back to TCP).
async fn allocate_endpoint(
    layout: &DataLayout,
    manifest: &ComponentManifest,
) -> io::Result<Endpoint> {
    if manifest.transport == Some(slsdk_rs::TransportCaps::TcpOnly) {
        return allocate_tcp_endpoint().await;
    }
    #[cfg(unix)]
    {
        tokio::fs::create_dir_all(layout.sockets_dir()).await?;
        let path = layout.socket_path(&manifest.id);
        // Leftover cleanup: a socket file left by a previous non-graceful
        // exit would make the component's bind fail with AddrInUse
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok(Endpoint::Uds {
            path: path.to_string_lossy().into_owned(),
        })
    }
    #[cfg(windows)]
    {
        let _ = layout;
        allocate_tcp_endpoint().await
    }
}

/// Absolute form of the launch command: a relative command resolves against
/// the component's workdir, not the shell process's current directory.
///
/// Windows rationale: std's `resolve_exe` and `CreateProcessW` both resolve a
/// relative program path against the *calling process's* current directory
/// (the per-child `lpCurrentDirectory` never participates), so the relative
/// `./target/release/<id>-server` form would be looked up under the shell's
/// cwd and fail there. Unix is equivalent either way (the forked child chdirs
/// to the workdir before exec), and an absolute command passes through
/// unchanged. Joining then re-collecting the path components drops the middle
/// `.` element `join` produces for `./x` commands — a pure string operation
/// (no canonicalization syscalls; `..` elements are preserved untouched).
fn absolute_launch_command(command: &str, workdir: &Path) -> PathBuf {
    let command = Path::new(command);
    if command.is_absolute() {
        return command.to_path_buf();
    }
    workdir.join(command).components().collect::<PathBuf>()
}

/// Windows orphan guard: assign a freshly spawned component process to a
/// global job object created with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.
///
/// The shell process owns the only job handle and never closes it: any form
/// of shell death (normal exit, crash, external kill) closes the handle, and
/// the kernel then terminates every component in the job — the Windows
/// equivalent of the Linux PDEATHSIG guard. A stale component process
/// surviving its shell is not a hypothetical: it holds a sharing violation
/// on the install directory, and every later update of that component fails
/// until the process is killed by hand.
///
/// Failure here only degrades the guard (the component keeps running
/// normally), so callers log a warning instead of failing the spawn.
#[cfg(windows)]
fn assign_child_to_kill_on_close_job(child: &mut Child) -> io::Result<()> {
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

    let job = kill_on_close_job_handle() as windows_sys::Win32::Foundation::HANDLE;
    let child_handle = child
        .raw_handle()
        .ok_or_else(|| io::Error::other("the child handle is already gone"))?;
    let assigned = unsafe { AssignProcessToJobObject(job, child_handle) };
    if assigned == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The process-lifetime KILL_ON_JOB_CLOSE job every component is assigned
/// to. Lazily created once, owned by this static (never closed): the handle
/// dies with the shell process, and the kernel then kills the job members.
/// Stored as isize (the raw HANDLE is a pointer, not Send/Sync-safe for a
/// static).
#[cfg(windows)]
static KILL_ON_CLOSE_JOB: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

/// Get (creating on first use) the global KILL_ON_JOB_CLOSE job handle (as
/// isize; cast to HANDLE at the call sites). Exposed separately so tests
/// can assert membership of this *specific* job (an IsProcessInJob(NULL)
/// check would also answer true for an inherited sandbox job, which would
/// not prove our guard did anything).
#[cfg(windows)]
pub(crate) fn kill_on_close_job_handle() -> isize {
    *KILL_ON_CLOSE_JOB.get_or_init(|| unsafe {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let applied = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if applied == 0 {
            log::error!(
                "KILL_ON_JOB_CLOSE not applied to the component job object (orphan guard inert): {}",
                io::Error::last_os_error()
            );
        }
        job as isize
    })
}

/// Full component start flow: allocate endpoint → spawn (env injection) →
/// health polling →
/// register + start supervise. Any step failing reclaims the already-created
/// child process and socket file.
///
/// `workdir` is the child's working directory — launch.command's relative
/// resolution base. Callers derive it via [`crate::registry::workdir_for`]:
/// installed entries resolve against the install directory, dev entries
/// against the project directory.
pub async fn spawn_component(
    layout: &DataLayout,
    registry: &Arc<Registry>,
    manifest: Arc<ComponentManifest>,
    workdir: PathBuf,
    health_timeout: Duration,
    app: Option<AppHandle>,
) -> Result<(), LifecycleError> {
    // launch=None (pure-frontend shape): no backend process to spawn — succeed
    // as a no-op. Every start path funnels through this single entry, so the
    // gate here covers restore-start / manual start / config restart alike
    // (process-state presentation is phase 3; this phase only guarantees the
    // logic does not break)
    let Some(launch) = manifest.launch.as_ref() else {
        log::info!(
            "component {} has no backend process (launch=None), skipping spawn",
            manifest.id
        );
        return Ok(());
    };
    let endpoint = allocate_endpoint(layout, &manifest)
        .await
        .map_err(|source| LifecycleError::Io { source })?;
    let token = Arc::new(crate::auth::generate_token());

    // Relative launch.command must become absolute *before* Command::new:
    // on Windows both resolve_exe and CreateProcessW resolve it against the
    // shell's cwd, not the child workdir (see absolute_launch_command).
    let program = absolute_launch_command(&launch.command, &workdir);

    // stdin: unix passes null (the unix component path does not read stdin; a
    // dangling pipe serves no purpose);
    // windows keeps the pipe — slsdk serve's stdin `shutdown` fallback
    // channel depends on it.
    // stdout/stderr: captured into the component's log file (a piped stream
    // MUST be drained continuously — an unread pipe fills its fixed kernel
    // buffer and deadlocks the writing component — so each pipe gets a
    // dedicated reader task below).
    #[cfg(unix)]
    let mut command = {
        let mut c = Command::new(&program);
        c.stdin(Stdio::null());
        c
    };
    #[cfg(windows)]
    let mut command = {
        let mut c = Command::new(&program);
        c.stdin(Stdio::piped());
        // A GUI shell spawning a console-subsystem component must not flash
        // a console window (CreateProcessW allocates one by default):
        // CREATE_NO_WINDOW (0x08000000) suppresses it. The pipe redirections
        // below (stdin/stdout/stderr) are unaffected by this flag.
        c.creation_flags(0x08000000);
        c
    };
    command
        .args(&launch.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env(ENV_ENDPOINT, endpoint.to_string())
        .env(ENV_TOKEN, token.as_str())
        .current_dir(&workdir)
        // Startup-window leak guard: if this spawn future is cancelled while
        // the child is still starting (health probe pending, not yet
        // registered) or the async runtime tears the supervise task down,
        // the Child drops — tokio would detach the process by default, so
        // kill_on_drop turns that drop into a kill. A guard for that window
        // only, not a substitute for the graceful shell-exit sweep (which
        // goes through request_stop + supervise).
        .kill_on_drop(true);

    #[cfg(target_os = "linux")]
    {
        // Orphan-source fix: the kernel delivers SIGKILL to the component the
        // moment the shell dies (any form — crash, external kill, normal
        // exit), so a dead shell can no longer leak live component processes.
        // prctl/PDEATHSIG is Linux-only: macOS has no kernel-level
        // equivalent — there, a normal shell exit is handled by the run-event
        // graceful sweep plus the Child kill_on_drop teardown reclaim, and a
        // shell crash or SIGKILL is not covered by either mechanism. The
        // Windows counterpart is the KILL_ON_JOB_CLOSE job assignment right
        // after spawn (see assign_child_to_kill_on_close_job); it is also
        // per-thread (it watches the thread that forked): tokio worker
        // threads live as long as the process, which makes it safe here —
        // never move this spawn into `spawn_blocking` (disposable blocking
        // threads would kill healthy components).
        let shell_pid = nix::unistd::getpid().as_raw();
        unsafe {
            command.pre_exec(move || {
                nix::sys::prctl::set_pdeathsig(nix::sys::signal::Signal::SIGKILL)
                    .map_err(io::Error::other)?;
                // Race window: if the shell died between fork and prctl, the
                // signal never fires — a parent already different from the
                // spawning shell means we are an orphan right now; exit
                // immediately (async-signal-safe _exit, exec never happens).
                if nix::unistd::getppid().as_raw() != shell_pid {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
    }

    let mut child = command.spawn().map_err(|source| LifecycleError::Spawn {
        source,
        program: program.clone(),
        args: launch.args.clone(),
    })?;
    let pid = child.id();

    // Windows orphan guard: assign the fresh child to the global
    // KILL_ON_JOB_CLOSE job *immediately* after spawn (the spawn→assign
    // window is the Windows counterpart of the fork→prctl race above, same
    // millisecond scale, accepted). Failure only degrades the guard (the
    // component still runs normally) — logged, never fatal.
    #[cfg(windows)]
    if let Err(e) = assign_child_to_kill_on_close_job(&mut child) {
        log::warn!(
            "component {} job-object assignment failed (orphan guard degraded, the component still runs): {e}",
            manifest.id
        );
    }

    // Hand the output pipes to dedicated reader tasks right away: starting
    // them *before* the health probe means even a probe-failure reclaim
    // leaves the component's own output on disk (exactly the diagnostics
    // surface the capture exists for). Each task drains until EOF (process
    // exit), so no pipe buffer can fill and deadlock the component.
    let log_path = layout.component_log_file(&manifest.id);
    spawn_pipe_reader_tasks(&mut child, log_path);

    // Readiness probe failed: the component did not become ready in time —
    // cannot leave a wild process, reclaim then report the error
    if let Err(e) = wait_until_healthy(&endpoint, &token, &manifest.id, health_timeout).await {
        child.start_kill().ok();
        let _ = child.wait().await;
        cleanup_socket(&endpoint).await;
        return Err(e);
    }

    let (stop_tx, stop_rx) = watch::channel(());
    let shutdown_stdin = child.stdin.take();

    if let Err(e) = registry
        .register(
            &manifest.namespace,
            RunningComponent {
                manifest: Arc::clone(&manifest),
                endpoint: endpoint.clone(),
                token,
                pid,
                stop: stop_tx,
            },
        )
        .await
    {
        // Registration conflict: reclaim the just-started process (kill first
        // then report, preventing a wild process)
        child.start_kill().ok();
        let _ = child.wait().await;
        cleanup_socket(&endpoint).await;
        return Err(LifecycleError::NamespaceTaken {
            namespace: match e {
                crate::registry::RegistryError::NamespaceTaken { namespace } => namespace,
            },
        });
    }

    let socket_path = socket_cleanup_path(layout, &manifest.id, &endpoint);
    let shutdown = ShutdownChannel {
        pid,
        stdin: shutdown_stdin,
    };
    let ctx = SuperviseContext {
        manifest: Arc::clone(&manifest),
        workdir,
        socket_path,
        registry: Arc::clone(registry),
        layout: layout.clone(),
        app,
    };
    tokio::spawn(supervise(child, shutdown, stop_rx, ctx));
    log::info!(
        "component spawned: {} (ns={} v={} pid={pid:?} endpoint={endpoint} program={})",
        manifest.id,
        manifest.namespace,
        manifest.version,
        program.display()
    );
    Ok(())
}

/// Drain one output pipe (stdout or stderr) into the component's log file,
/// appending line by line until EOF (the process exited and both pipe ends
/// closed). The component's own output already carries timestamps; no second
/// prefix is added. Both streams append to the same file (v1 keeps a single
/// log per component); append-mode writes are per-write atomic, so the
/// worst-case interleaving is line-level — acceptable for a diagnostics
/// surface. Write failures (disk full, directory removed) stop the drain
/// quietly: the pipe is then closed and the component may fail on its next
/// write, but blocking the shell's task pool to keep reading would be worse.
async fn pipe_to_log_file<R>(pipe: R, log_path: PathBuf)
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let mut file = match tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            log::warn!("failed to open the component log file {log_path:?}: {e}");
            return;
        }
    };
    let mut lines = tokio::io::BufReader::new(pipe).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if file.write_all(line.as_bytes()).await.is_err()
                    || file.write_all(b"\n").await.is_err()
                {
                    log::warn!(
                        "failed to write the component log file {log_path:?}; capture stops"
                    );
                    return;
                }
            }
            Ok(None) => return, // EOF: the component process has exited
            Err(e) => {
                log::warn!("failed to read the component's output pipe ({log_path:?}): {e}");
                return;
            }
        }
    }
}

/// Take both output pipes off the freshly spawned child and hand each to its
/// own reader task. Must be called promptly after spawn and before any
/// blocking wait: an undrained pipe fills its fixed kernel buffer and
/// deadlocks the writing component.
fn spawn_pipe_reader_tasks(child: &mut Child, log_path: PathBuf) {
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(pipe_to_log_file(stdout, log_path.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(pipe_to_log_file(stderr, log_path));
    }
}

/// Socket cleanup path after the component crashes and exits (unix UDS;
/// TCP/Windows needs no cleanup).
fn socket_cleanup_path(layout: &DataLayout, id: &str, endpoint: &Endpoint) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        match endpoint {
            Endpoint::Uds { .. } => Some(layout.socket_path(id)),
            Endpoint::Tcp { .. } => None,
        }
    }
    #[cfg(windows)]
    {
        let _ = (layout, id, endpoint);
        None
    }
}

async fn cleanup_socket(endpoint: &Endpoint) {
    #[cfg(unix)]
    if let Endpoint::Uds { path } = endpoint {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("socket cleanup failed ({path}): {e}"),
        }
    }
    #[cfg(windows)]
    {
        let _ = endpoint;
    }
}

/// Poll the component's /health until ready or timed out. 200 with a
/// matching id counts as ready — defends against probing a leftover old
/// instance or the wrong component (slsdk serve's /health returns
/// {id, version}).
pub(crate) async fn wait_until_healthy(
    endpoint: &Endpoint,
    token: &str,
    expect_id: &str,
    timeout: Duration,
) -> Result<(), LifecycleError> {
    let deadline = Instant::now() + timeout;
    // Diagnostics carried into the timeout error: the outcome of the most
    // recent probe. The loop body always assigns it before the deadline
    // check, so no sentinel initialization is needed.
    let mut last_probe;
    loop {
        match probe_health(endpoint, token).await {
            Ok(Some(id)) if id == expect_id => return Ok(()),
            Ok(other) => {
                last_probe = format!("health responded {other:?} (expected id {expect_id:?})");
                log::debug!("health not ready yet (response: {other:?}), keep polling");
            }
            Err(e) => {
                last_probe = format!("connection failed: {e}");
                log::debug!(
                    "health probe connection failed (the component may not be listening yet): {e}"
                );
            }
        }
        if Instant::now() >= deadline {
            return Err(LifecycleError::HealthTimeout {
                id: expect_id.to_string(),
                endpoint: endpoint.to_string(),
                last_probe,
            });
        }
        tokio::time::sleep(HEALTH_INTERVAL).await;
    }
}

/// A single health probe: connection failure = Err; non-2xx = Ok(None);
/// 2xx = Ok(Some(id)).
async fn probe_health(endpoint: &Endpoint, token: &str) -> io::Result<Option<String>> {
    let stream = crate::gateway::connect_component(endpoint).await?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(stream)
        .await
        .map_err(|e| io::Error::other(format!("health connection handshake failed: {e}")))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = hyper::Request::builder()
        .uri(format!(
            "http://{}/health",
            crate::gateway::UPSTREAM_AUTHORITY
        ))
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .body(axum::body::Body::empty())
        .map_err(|e| io::Error::other(format!("failed to build the health request: {e}")))?;
    let res = sender
        .send_request(req)
        .await
        .map_err(|e| io::Error::other(format!("failed to send the health request: {e}")))?;
    if !res.status().is_success() {
        return Ok(None);
    }
    let bytes = http_body_util::BodyExt::collect(res.into_body())
        .await
        .map_err(|e| io::Error::other(format!("failed to read the health response: {e}")))?
        .to_bytes()
        .to_vec();
    let id = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v.get("id").and_then(|id| id.as_str().map(str::to_string)));
    Ok(id)
}

/// Stop-request channel: pid (unix SIGTERM target) and stdin (windows
/// shutdown command channel) — two carriers of the same semantic, "request
/// the component to exit gracefully"; aggregating them keeps supervise's
/// parameter list from bloating.
struct ShutdownChannel {
    pid: Option<u32>,
    stdin: Option<ChildStdin>,
}

/// The spawn context supervise needs after (and during) the wait: everything
/// except the child process and its stop channel. One struct keeps the spawn
/// signature within clippy's argument budget.
struct SuperviseContext {
    manifest: Arc<ComponentManifest>,
    /// The child's working directory — also the config.toml base (same
    /// source as the config-plane read/write paths).
    workdir: PathBuf,
    socket_path: Option<PathBuf>,
    registry: Arc<Registry>,
    layout: DataLayout,
    /// Shell→UI push channel: the exit broadcast the management list listens
    /// for (None in tests).
    app: Option<AppHandle>,
}

/// Exit guard: waits for a natural exit or a stop signal; a stop signal
/// triggers graceful shutdown (SIGTERM/stdin shutdown) with force-kill after
/// the grace timeout; after exit it removes the registry entry, recomputes
/// and persists the configuration plane's settled state from manifest+config,
/// and cleans up the socket file.
async fn supervise(
    mut child: Child,
    mut shutdown: ShutdownChannel,
    mut stop_rx: watch::Receiver<()>,
    ctx: SuperviseContext,
) {
    let SuperviseContext {
        manifest,
        workdir,
        socket_path,
        registry,
        layout,
        app,
    } = ctx;
    let namespace = manifest.namespace.clone();
    // Phase one: wait for natural exit or a stop signal. Do not touch child
    // inside the select arm —
    // once select expands, the wait future holds &mut child until the match
    // ends, and borrowing child again in the arm body would conflict at
    // compile time; all subsequent operations on child live outside the loop
    enum Phase {
        Exited(io::Result<std::process::ExitStatus>),
        StopRequested,
    }
    let phase = tokio::select! {
        status = child.wait() => Phase::Exited(status),
        _ = stop_rx.changed() => {
            // take instead of move: the other select-arm futures hold the
            // borrow of child until select finishes evaluating, and moving
            // the captured variable wholesale in the arm body triggers a
            // borrow conflict; the stop signal fires only once — receiving
            // it enters the grace phase (force-kill backstop lives outside
            // the select)
            let stdin = shutdown.stdin.take();
            request_shutdown(shutdown.pid, stdin, &namespace);
            Phase::StopRequested
        }
    };

    // Phase two: grace wait and force-kill after the stop request (select has
    // expanded, borrows released)
    let exit_status = match phase {
        Phase::Exited(status) => status,
        Phase::StopRequested => {
            log::info!(
                "component {namespace} shutting down gracefully (grace period {STOP_GRACE:?})"
            );
            match tokio::time::timeout(STOP_GRACE, child.wait()).await {
                Ok(status) => status,
                Err(_) => {
                    log::warn!(
                        "component {namespace} did not exit within the {STOP_GRACE:?} grace period after the stop request — sending SIGKILL (graceful exit is a convention the shell cannot assume the component honors)"
                    );
                    child.start_kill().ok();
                    child.wait().await
                }
            }
        }
    };

    match &exit_status {
        Ok(status) => log::info!("component {namespace} exited: {status}"),
        Err(e) => log::warn!("failed to wait for component {namespace} to exit: {e}"),
    }
    registry.remove(&namespace).await;

    // Configuration plane settled state (architecture.md "Component state
    // machine"): recomputed from the current manifest+config after process
    // exit. compute is config-dimensional (ready/needs-config); overlaying
    // the "process has exited" fact: config complete → stopped; config
    // missing → needs-config.
    // The old process exiting after an upgrade replacement also takes this
    // path, so a needs-config produced by the upgrade comparison is not
    // overwritten by stopped (gate semantics preserved); launch_policy keeps
    // its existing value.
    let state = match crate::config_plane::compute_state(
        &manifest,
        // Same source as the config-plane read/write paths: the config.toml
        // lives in the spawn workdir (install dir for installed entries,
        // project dir for dev entries)
        &workdir.join("config.toml"),
    )
    .await
    {
        Ok(crate::config_plane::ComponentState::NeedsConfig) => {
            crate::config_plane::ComponentState::NeedsConfig
        }
        Ok(_) => crate::config_plane::ComponentState::Stopped,
        Err(e) => {
            log::warn!(
                "failed to determine the post-exit state of component {namespace} ({e}); conservatively marking needs-config"
            );
            crate::config_plane::ComponentState::NeedsConfig
        }
    };
    let launch_policy = crate::config_plane::load_meta(&layout, &manifest.id)
        .await
        .map(|m| m.launch_policy)
        .unwrap_or_else(crate::config_plane::default_launch_policy);
    if let Err(e) = crate::config_plane::save_meta(
        &layout,
        &manifest.id,
        &crate::config_plane::ComponentMeta {
            state,
            launch_policy,
        },
    )
    .await
    {
        log::warn!("failed to persist the final state of component {namespace}: {e}");
    }

    if let Some(path) = socket_path {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("socket cleanup failed for component {namespace} ({path:?}): {e}"),
        }
    }

    // Exit broadcast (the UI's only passive-refresh channel): an external
    // kill / crash is invisible to the management page's own action flow, so
    // the shell pushes "this component just exited" and the list re-fetches.
    // Emitted last: by the time the UI reacts, the running table and the
    // settled meta are already consistent with the event.
    if let Some(app) = &app {
        if let Err(e) = app.emit(
            "component-exited",
            serde_json::json!({ "id": manifest.id, "namespace": namespace }),
        ) {
            log::warn!("failed to broadcast the exit of component {namespace}: {e}");
        }
    }
}

/// Request the component to exit gracefully: unix sends SIGTERM (nix safe
/// wrapper; tokio's own start_kill is SIGKILL-grade); windows writes the
/// stdin `shutdown` line (the fallback channel of the Plane B contract).
fn request_shutdown(pid: Option<u32>, shutdown_stdin: Option<ChildStdin>, namespace: &str) {
    #[cfg(unix)]
    {
        let _ = shutdown_stdin; // unix has no stdin command channel (the component's unix path does not read stdin)
        if let Some(pid) = pid {
            let raw = nix::unistd::Pid::from_raw(pid as i32);
            if let Err(e) = nix::sys::signal::kill(raw, nix::sys::signal::Signal::SIGTERM) {
                log::warn!("failed to send SIGTERM to component {namespace} (pid={pid}): {e}");
            }
        }
    }
    #[cfg(windows)]
    {
        let _ = pid;
        let _ = namespace;
        if let Some(mut stdin) = shutdown_stdin {
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(b"shutdown\n").await;
                let _ = stdin.shutdown().await;
            });
        }
    }
}

/// Shell-exit sweep: request a graceful stop of every running component,
/// then wait until the running table drains (each supervise task removes
/// its entry right after its process exits). The wait budget covers every
/// component's full grace path plus overhead; on timeout the remaining
/// namespaces are logged and the sweep returns anyway — the exit must not
/// hang on a stuck component; runtime teardown then reclaims any surviving
/// `Child` through `kill_on_drop`.
async fn stop_all_components(registry: &Registry) {
    let namespaces: Vec<String> = registry
        .list()
        .await
        .into_iter()
        .map(|s| s.namespace)
        .collect();
    if namespaces.is_empty() {
        return;
    }
    for namespace in &namespaces {
        registry.request_stop(namespace).await;
    }
    let deadline = Instant::now() + STOP_GRACE + Duration::from_secs(1);
    loop {
        if registry.list().await.is_empty() {
            log::info!("shell exit sweep: all components stopped");
            return;
        }
        if Instant::now() >= deadline {
            let remaining: Vec<String> = registry
                .list()
                .await
                .into_iter()
                .map(|s| s.namespace)
                .collect();
            log::warn!("shell exit sweep timed out; components still running: {remaining:?}");
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Exit-cleanup state machine (one AtomicU8): idle = no exit in flight,
/// cleaning = the async sweep is running, ready = nothing left to wait for
/// (the sweep finished, or there was nothing to clean). Transitions go
/// through compare_exchange so a second exit request can never start a
/// second sweep.
const EXIT_IDLE: u8 = 0;
const EXIT_CLEANING: u8 = 1;
const EXIT_READY: u8 = 2;

static EXIT_CLEANUP_STATE: AtomicU8 = AtomicU8::new(EXIT_IDLE);

/// The Tauri run-event entry (wired from `App::run` in lib.rs). Only
/// ExitRequested carries shell-exit semantics: hold the exit, sweep the
/// running components, then re-request the exit with the original code.
pub(crate) fn handle_run_event(app: &AppHandle, event: RunEvent) {
    let RunEvent::ExitRequested { code, api, .. } = event else {
        return;
    };
    // The updater handoff calls app.exit(0) before setup manages AppState
    // (self_update::handoff_updated_shell early-returns the setup): no
    // component was ever started and `state()` would panic here. Nothing to
    // clean — mark ready and let that exit proceed untouched.
    let Some(state) = app.try_state::<crate::AppState>() else {
        EXIT_CLEANUP_STATE.store(EXIT_READY, Ordering::SeqCst);
        return;
    };
    match EXIT_CLEANUP_STATE.compare_exchange(
        EXIT_IDLE,
        EXIT_CLEANING,
        Ordering::SeqCst,
        Ordering::SeqCst,
    ) {
        // First exit request: hold it, sweep asynchronously, then re-request
        // the exit (the re-entry lands on the ready arm and exits).
        Ok(_) => {
            api.prevent_exit();
            let registry = Arc::clone(&state.registry);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                stop_all_components(&registry).await;
                EXIT_CLEANUP_STATE.store(EXIT_READY, Ordering::SeqCst);
                app.exit(code.unwrap_or(0));
            });
        }
        // A request while the sweep is in flight (a second window close
        // racing the finishing app.exit) must also be held: the in-flight
        // sweep owns the exit.
        Err(EXIT_CLEANING) => api.prevent_exit(),
        // Sweep already finished (or nothing to clean): the exit proceeds.
        Err(_) => {}
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::registry::Registry;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;
    use std::time::Duration;

    fn manifest(id: &str, ns: &str, command: &str, args: &[&str]) -> ComponentManifest {
        let args_json = serde_json::to_string(args).unwrap();
        serde_json::from_str(&format!(
            r#"{{
                "id": "{id}",
                "version": "0.1.0",
                "min_shell_version": "0.1.0",
                "namespace": "{ns}",
                "display_name": "{id}",
                "ui": {{ "ui_type": "module", "entry": "assets/main.js" }},
                "menu": null,
                "routes": [],
                "launch": {{ "command": "{command}", "args": {args_json} }},
                "files": [ {{ "path": "bin", "sha256": "aa" }} ]
            }}"#
        ))
        .unwrap()
    }

    /// In-process mock component: /health returns {"id": <id>} (no token
    /// check; the probe works with any token).
    async fn serve_health(id: &'static str) -> Endpoint {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/health",
                    get(move || async move { Json(json!({ "id": id })) }),
                ),
            )
            .await
            .unwrap();
        });
        Endpoint::Tcp { addr }
    }

    // ---------- health polling ----------

    #[tokio::test]
    async fn healthy_component_is_detected() {
        let endpoint = serve_health("mockapp").await;
        wait_until_healthy(&endpoint, "any-token", "mockapp", Duration::from_secs(2))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn id_mismatch_times_out() {
        let endpoint = serve_health("mockapp").await;
        let result = wait_until_healthy(
            &endpoint,
            "any-token",
            "someone-else",
            Duration::from_millis(400),
        )
        .await;
        assert!(matches!(result, Err(LifecycleError::HealthTimeout { .. })));
    }

    #[tokio::test]
    async fn unreachable_endpoint_times_out() {
        let endpoint = Endpoint::Tcp {
            addr: "127.0.0.1:1".parse().unwrap(),
        };
        let result =
            wait_until_healthy(&endpoint, "tok", "mockapp", Duration::from_millis(400)).await;
        assert!(matches!(result, Err(LifecycleError::HealthTimeout { .. })));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn healthy_component_detected_over_uds() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("health.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/health", get(|| async { Json(json!({ "id": "udsapp" })) })),
            )
            .await
            .unwrap();
        });
        let endpoint = Endpoint::Uds {
            path: sock.to_string_lossy().into_owned(),
        };
        wait_until_healthy(&endpoint, "tok", "udsapp", Duration::from_secs(2))
            .await
            .unwrap();
    }

    // ---------- spawn full flow (real child process) ----------

    #[tokio::test]
    async fn spawn_reclaims_process_when_health_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        // current_dir requires the directory to exist: simulate an installed
        // component's install directory
        std::fs::create_dir_all(layout.component_dir("sleeper")).unwrap();
        let registry = Arc::new(Registry::new());
        let m = Arc::new(manifest("sleeper", "sleepns", "/bin/sleep", &["30"]));

        // /bin/sleep listens on no endpoint → health must time out → the
        // process should be reclaimed
        let result = spawn_component(
            &layout,
            &registry,
            Arc::clone(&m),
            layout.component_dir("sleeper"),
            Duration::from_millis(600),
            None,
        )
        .await;
        assert!(matches!(result, Err(LifecycleError::HealthTimeout { .. })));
        assert!(registry.lookup("sleepns").await.is_none());
        // The socket file was never created by the component; the allocation
        // path should leave nothing behind
        assert!(!layout.socket_path("sleeper").exists());
    }

    /// Test-only RAII guard, deliberately local to this test module and
    /// never a production structure: armed right after the pid file parses,
    /// so any panic or assert failure drops it and SIGKILLs the still-live
    /// pid. Disarmed only after the process is confirmed gone.
    struct PidKillGuard {
        pid: u32,
        armed: bool,
    }

    impl PidKillGuard {
        fn arm(pid: u32) -> Self {
            Self { pid, armed: true }
        }

        fn disarm(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for PidKillGuard {
        fn drop(&mut self) {
            if self.armed {
                // Drop is synchronous: the blocking std Command is the only
                // form available here.
                let _ = std::process::Command::new("kill")
                    .args(["-9", &self.pid.to_string()])
                    .status();
            }
        }
    }

    /// Gone = kill(1) -0 fails, or `ps` reports a zombie state. A zombie
    /// still answers kill -0 (it sits in the process table) but will never
    /// run again — counting it alive would spin the assertion to the
    /// timeout. `ps -o stat=` starts the state column with 'Z' for zombies
    /// on both macOS and Linux.
    async fn process_gone(pid: u32) -> bool {
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .await
            .expect("kill(1) must be available")
            .success();
        if !alive {
            return true;
        }
        let out = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .await
            .expect("ps(1) must be available");
        String::from_utf8_lossy(&out.stdout).trim().starts_with('Z')
    }

    /// A spawn_component future cancelled while the child is still starting
    /// (health probe pending) must not leak the process: kill_on_drop kills
    /// the child when the cancelled future drops its Child. Anchor: the
    /// orphan components observed with PPID=1 after a shell exit. The pid
    /// file pins the exact pid the shell spawned (`exec` replaces the shell
    /// image, the pid is unchanged) and the assertion reads live process
    /// state (kill -0 plus the zombie distinction) — never the file. The
    /// RAII guard backstops every failure path.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_spawn_kills_the_starting_child() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        std::fs::create_dir_all(layout.component_dir("canceller")).unwrap();
        let registry = Arc::new(Registry::new());

        let pid_file = dir.path().join("child.pid");
        let pending_pid_file = dir.path().join("child.pid.pending");
        let pid_arg = pid_file.to_string_lossy().into_owned();
        let pending_pid_arg = pending_pid_file.to_string_lossy().into_owned();
        let m = Arc::new(manifest(
            "canceller",
            "cancellerns",
            "/bin/sh",
            &[
                "-c",
                &format!(
                    "echo $$ > \"{pending_pid_arg}\" && mv \"{pending_pid_arg}\" \"{pid_arg}\" && exec sleep 30"
                ),
            ],
        ));

        // No health endpoint exists: the spawn parks in the readiness probe
        // loop until cancelled — exactly the startup window under test.
        // The clones move into the spawned block so its future is 'static
        // (spawn_component borrows them only inside the block).
        let task_layout = layout.clone();
        let task_registry = Arc::clone(&registry);
        let handle = tokio::spawn(async move {
            spawn_component(
                &task_layout,
                &task_registry,
                m,
                task_layout.component_dir("canceller"),
                Duration::from_secs(120),
                None,
            )
            .await
        });

        // Wait until the child has written its pid (it is execing sleep).
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !pid_file.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the starting child never wrote its pid file"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // Armed from this instant: every path below (success, assert
        // failure, panic) ends in a guard drop, which SIGKILLs the pid
        // unless it was confirmed gone and disarmed.
        let mut guard = PidKillGuard::arm(pid);

        // Cancel the startup: the future unwinds, the Child drops, and
        // kill_on_drop must take the process down.
        handle.abort();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), handle)
                .await
                .is_ok(),
            "the cancelled spawn future must return promptly"
        );

        // The pid must be gone (or a never-runs-again zombie) within the
        // timeout; past it the assertion fails and the armed guard's Drop
        // delivers the SIGKILL as the test unwinds.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if process_gone(pid).await {
                guard.disarm();
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the starting child must be killed within 5s of cancellation"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // The startup never completed, so nothing may be registered.
        assert!(registry.list().await.is_empty());
    }

    // ---------- supervise graceful stop ----------

    /// A component that ignores SIGTERM must still be stopped: supervise's
    /// grace timeout falls back to SIGKILL (graceful exit is a convention the
    /// shell never assumes — this is the backstop half of the stop contract).
    #[tokio::test]
    async fn supervise_force_kills_a_sigterm_ignoring_child() {
        let (stop_tx, stop_rx) = watch::channel(());
        // sh's `trap '' TERM` ignores SIGTERM: the graceful request lands, is
        // ignored, and the child keeps running until the grace backstop fires.
        let child = Command::new("/bin/sh")
            .args(["-c", "trap '' TERM; sleep 30"])
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id().expect("a freshly spawned child carries a pid");
        let registry = Arc::new(Registry::new());
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());

        let handle = tokio::spawn(supervise(
            child,
            ShutdownChannel {
                pid: Some(pid),
                stdin: None,
            },
            stop_rx,
            SuperviseContext {
                manifest: Arc::new(manifest("stubbornapp", "stubbornns", "/bin/sh", &["-c"])),
                workdir: dir.path().join("work"),
                socket_path: None,
                registry: Arc::clone(&registry),
                layout: layout.clone(),
                app: None,
            },
        ));
        // Give supervise time to enter the wait state
        tokio::time::sleep(Duration::from_millis(100)).await;
        stop_tx.send(()).unwrap();

        // The child ignores SIGTERM: supervise must come back right after the
        // grace period via the SIGKILL backstop (long before the natural 30s
        // end of the mocked component).
        let joined = tokio::time::timeout(STOP_GRACE + Duration::from_secs(2), handle).await;
        assert!(
            joined.is_ok(),
            "supervise must return via the force-kill backstop after the grace period"
        );
        assert!(registry.lookup("stubbornns").await.is_none());
        // The force kill actually landed: the pid is gone (`trap '' TERM`
        // would have kept a live process for the full 30 seconds otherwise).
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .await
            .expect("kill(1) must be available");
        assert!(
            !alive.success(),
            "the SIGTERM-ignoring child must be dead after the force kill"
        );
    }

    #[tokio::test]
    async fn supervise_stops_child_on_signal() {
        let (stop_tx, stop_rx) = watch::channel(());
        let child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let registry = Arc::new(Registry::new());
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());

        let handle = tokio::spawn(supervise(
            child,
            ShutdownChannel { pid, stdin: None },
            stop_rx,
            SuperviseContext {
                manifest: Arc::new(manifest("sleepapp", "sleepns", "/bin/sleep", &["30"])),
                workdir: dir.path().join("work"),
                socket_path: None,
                registry: Arc::clone(&registry),
                layout: layout.clone(),
                app: None,
            },
        ));
        // Give supervise time to enter the wait state
        tokio::time::sleep(Duration::from_millis(100)).await;
        stop_tx.send(()).unwrap();

        // sleep terminates immediately on SIGTERM: supervise should return
        // well before the 30-second natural end
        let joined = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(
            joined.is_ok(),
            "supervise should return promptly after a graceful stop"
        );
        assert!(registry.lookup("sleepns").await.is_none());
        // Configuration plane cleanup: no config section (config complete) →
        // stopped persisted
        let meta = crate::config_plane::load_meta(&layout, "sleepapp")
            .await
            .expect("meta should be persisted after exit");
        assert_eq!(meta.state, crate::config_plane::ComponentState::Stopped);
        assert_eq!(meta.launch_policy, crate::config_plane::LaunchPolicy::Auto);
    }

    /// External SIGKILL (any death form) must trigger the full exit cleanup:
    /// supervise's child.wait() arm treats a killed child exactly like a
    /// natural exit — running-table removal + settled-state persistence — so
    /// the management surface cannot stay on "running" after the process is
    /// gone. This is the shell-side half of the bug-③ contract; the UI half
    /// is the component-exited broadcast.
    #[cfg(unix)]
    #[tokio::test]
    async fn supervise_cleans_up_when_child_is_sigkilled() {
        let (stop_tx, stop_rx) = watch::channel(());
        let child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id().expect("a freshly spawned child carries a pid");
        let registry = Arc::new(Registry::new());
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        // Runtime registration in place (the state the management page reads
        // "running" from): the kill must sweep it away.
        registry
            .register(
                "killedns",
                RunningComponent {
                    manifest: Arc::new(manifest("killedapp", "killedns", "/bin/sleep", &["30"])),
                    endpoint: Endpoint::Tcp {
                        addr: "127.0.0.1:1".parse().unwrap(),
                    },
                    token: Arc::new("tok".to_string()),
                    pid: Some(pid),
                    stop: stop_tx,
                },
            )
            .await
            .unwrap();

        let handle = tokio::spawn(supervise(
            child,
            ShutdownChannel {
                pid: Some(pid),
                stdin: None,
            },
            stop_rx,
            SuperviseContext {
                manifest: Arc::new(manifest("killedapp", "killedns", "/bin/sleep", &["30"])),
                workdir: dir.path().join("work"),
                socket_path: None,
                registry: Arc::clone(&registry),
                layout: layout.clone(),
                app: None,
            },
        ));
        // Let supervise settle into the child.wait() arm
        tokio::time::sleep(Duration::from_millis(100)).await;
        // External kill: nothing in the shell asked for a stop — supervise
        // must still observe the death and clean up.
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        )
        .unwrap();

        let joined = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(
            joined.is_ok(),
            "supervise should return promptly after the child is SIGKILLed"
        );
        assert!(
            registry.lookup("killedns").await.is_none(),
            "the killed component must leave the running table"
        );
        let meta = crate::config_plane::load_meta(&layout, "killedapp")
            .await
            .expect("meta should be persisted after exit");
        assert_eq!(meta.state, crate::config_plane::ComponentState::Stopped);
    }

    /// Gate semantics of exit cleanup: a component exiting with missing
    /// config (needs-config) is not overwritten by stopped — the key
    /// scenario of the old process exiting after the upgrade comparison
    /// produced needs-config.
    #[tokio::test]
    async fn supervise_marks_needs_config_when_required_missing() {
        let (stop_tx, stop_rx) = watch::channel(());
        let child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let registry = Arc::new(Registry::new());
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        // Manifest with a required field, config.toml has no value →
        // compute = needs-config
        let m: ComponentManifest = serde_json::from_str(
            r#"{
                "id": "gated",
                "version": "0.1.0",
                "min_shell_version": "0.1.0",
                "namespace": "gatedns",
                "display_name": "gated",
                "ui": { "ui_type": "module", "entry": "assets/main.js" },
                "menu": null,
                "routes": [],
                "launch": { "command": "/bin/sleep", "args": ["30"] },
                "files": [ { "path": "bin", "sha256": "aa" } ],
                "config": { "fields": [ { "key": "tok", "label": "tok", "type": "string", "required": true } ] }
            }"#,
        )
        .unwrap();

        let handle = tokio::spawn(supervise(
            child,
            ShutdownChannel { pid, stdin: None },
            stop_rx,
            SuperviseContext {
                manifest: Arc::new(m),
                workdir: dir.path().join("work"),
                socket_path: None,
                registry: Arc::clone(&registry),
                layout: layout.clone(),
                app: None,
            },
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        stop_tx.send(()).unwrap();
        let joined = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(joined.is_ok(), "supervise should return promptly");
        let meta = crate::config_plane::load_meta(&layout, "gated")
            .await
            .expect("meta should be persisted after exit");
        assert_eq!(meta.state, crate::config_plane::ComponentState::NeedsConfig);
    }

    /// End-to-end success chain: spawn → health ready → register →
    /// stop_all_components (the shell-exit sweep) → graceful exit → running
    /// table drained → socket cleanup.
    /// The real child process uses /bin/sleep to stand in for process
    /// semantics; health is answered by a mock responder pre-occupying the
    /// component socket (full coverage of the shell-side judgment chain).
    #[cfg(unix)]
    #[tokio::test]
    async fn spawn_registers_and_stop_removes_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        layout.ensure_dirs().unwrap();
        std::fs::create_dir_all(layout.component_dir("e2eapp")).unwrap();
        let registry = Arc::new(Registry::new());

        // Mock health responder: allocate_endpoint first cleans up leftover
        // socket files, so a pre-bind would be deleted and invalidated —
        // place a dummy file first to lock the ordering; the mock waits for
        // the dummy to be deleted by allocate before binding (in place within
        // the health polling interval = probe succeeds)
        let sock = layout.socket_path("e2eapp");
        std::fs::write(&sock, b"").unwrap();
        let wait_sock = sock.clone();
        tokio::spawn(async move {
            while wait_sock.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let listener = tokio::net::UnixListener::bind(&wait_sock).unwrap();
            axum::serve(
                listener,
                Router::new().route("/health", get(|| async { Json(json!({ "id": "e2eapp" })) })),
            )
            .await
            .unwrap();
        });

        let m = Arc::new(manifest("e2eapp", "e2ens", "/bin/sleep", &["30"]));
        spawn_component(
            &layout,
            &registry,
            m,
            layout.component_dir("e2eapp"),
            Duration::from_secs(3),
            None,
        )
        .await
        .unwrap();

        // Registered
        assert!(registry.lookup("e2ens").await.is_some());

        // Shell-exit path: the sweep requests the graceful stop of every
        // running entry (supervise SIGTERMs /bin/sleep → removal + socket
        // cleanup) and waits for the running table to drain before
        // returning.
        stop_all_components(&registry).await;
        assert!(
            registry.list().await.is_empty(),
            "the running table must be empty after the exit sweep"
        );
        assert!(registry.lookup("e2ens").await.is_none());
        // The socket file has been cleaned up (the mock responder's
        // connection drops with it)
        assert!(
            !sock.exists(),
            "the socket file should be cleaned up on exit"
        );
    }

    // ---------- endpoint allocation (transport declaration) ----------

    fn manifest_from_json(json: &str) -> ComponentManifest {
        serde_json::from_str(json).expect("test manifest JSON is valid")
    }

    // ---------- launch command absolutization ----------

    #[test]
    fn relative_launch_command_resolves_against_the_workdir() {
        let program =
            absolute_launch_command("./target/release/foo-server", Path::new("/tmp/proj"));
        assert_eq!(program, Path::new("/tmp/proj/target/release/foo-server"));
    }

    #[test]
    fn relative_launch_command_without_the_dot_prefix_joins_equally() {
        let program = absolute_launch_command("bin/foo-server", Path::new("/tmp/proj"));
        assert_eq!(program, Path::new("/tmp/proj/bin/foo-server"));
    }

    #[test]
    fn absolute_launch_command_passes_through_unchanged() {
        let program = absolute_launch_command("/usr/local/bin/foo", Path::new("/tmp/proj"));
        assert_eq!(program, Path::new("/usr/local/bin/foo"));
    }

    #[test]
    fn joined_middle_dot_component_is_normalized_away() {
        // join("./x") produces "<workdir>/./x": the middle CurDir element is
        // dropped by the component round-trip (zero "/." residue on every
        // platform; a pure string-level rewrite, no syscalls)
        let program = absolute_launch_command("./bin/foo", Path::new("/tmp/proj"));
        assert!(!program.to_string_lossy().contains("/."));
        assert_eq!(program, Path::new("/tmp/proj/bin/foo"));
    }

    #[tokio::test]
    async fn tcp_only_manifest_forces_a_loopback_tcp_descriptor() {
        let manifest = manifest_from_json(
            r#"{"id":"mockapp","version":"0.1.0","min_shell_version":"0.0.0","namespace":"mockapp","display_name":"Mock","menu":null,"routes":[],"files":[],"transport":"tcponly"}"#,
        );
        let dir = tempfile::TempDir::new().unwrap();
        let layout = DataLayout::new(dir.path());
        let endpoint = allocate_endpoint(&layout, &manifest).await.unwrap();
        assert!(
            matches!(endpoint, Endpoint::Tcp { .. }),
            "tcp-only transport must skip the platform UDS default"
        );
    }

    #[tokio::test]
    async fn default_manifest_keeps_the_platform_uds_descriptor() {
        let manifest = manifest_from_json(
            r#"{"id":"mockapp","version":"0.1.0","min_shell_version":"0.0.0","namespace":"mockapp","display_name":"Mock","menu":null,"routes":[],"files":[]}"#,
        );
        let dir = tempfile::TempDir::new().unwrap();
        let layout = DataLayout::new(dir.path());
        let endpoint = allocate_endpoint(&layout, &manifest).await.unwrap();
        assert!(
            matches!(endpoint, Endpoint::Uds { .. }),
            "a manifest without a transport declaration keeps the platform default"
        );
    }

    // ---------- stdout capture → component log file → read back ----------

    /// The full capture chain with a real child process: spawn's piped
    /// stdout lands in `{data_root}/logs/{id}.log`, and the management
    /// page's read_component_log returns it (tail semantics included). The
    /// component never opens a health endpoint, so the spawn ends in the
    /// probe-timeout reclaim — the reader tasks start before the probe (by
    /// contract), so the marker must be on disk once the pipes close.
    /// `exec sleep` replaces the shell, so start_kill reaps the actual
    /// sleep: no orphan lingers holding the pipe open past the assertion.
    #[tokio::test]
    async fn spawn_captures_output_and_read_component_log_returns_it() {
        use crate::AppState;
        use crate::commands::read_component_log_core;
        use crate::registry::{RegistryEntry, persist_entry_sync};

        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let layout = DataLayout::clone(&state.layout);
        std::fs::create_dir_all(layout.component_dir("logger")).unwrap();
        let registry = Arc::new(Registry::new());
        let m = Arc::new(manifest(
            "logger",
            "loggerns",
            "/bin/sh",
            &["-c", "echo marker-log-line; exec sleep 30"],
        ));

        // /bin/sh has no /health: the spawn ends in the reclaim, but the
        // piped stdout reader must have flushed the marker.
        let result = spawn_component(
            &layout,
            &registry,
            Arc::clone(&m),
            layout.component_dir("logger"),
            Duration::from_millis(600),
            None,
        )
        .await;
        assert!(matches!(result, Err(LifecycleError::HealthTimeout { .. })));

        // The drain task flushes asynchronously: poll until the marker lands.
        let log_path = layout.component_log_file("logger");
        let mut content = String::new();
        for _ in 0..40 {
            content = tokio::fs::read_to_string(&log_path)
                .await
                .unwrap_or_default();
            if content.contains("marker-log-line") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            content.contains("marker-log-line"),
            "the captured log must contain the child's stdout marker (got {content:?})"
        );

        // Management-page read path: the on-disk registration gate, then the
        // captured content, then the tail truncation.
        let entry = RegistryEntry::installed((*m).clone());
        persist_entry_sync(&layout, &entry).unwrap();
        let logged = read_component_log_core(&state, "logger", None)
            .await
            .expect("a registered component's log must be readable");
        // The file carries per-line trailing newlines (append form); the
        // read path returns the line content without the trailing blank.
        assert_eq!(
            logged,
            content.trim_end(),
            "read_component_log returns the captured content verbatim"
        );
        let tail_one = read_component_log_core(&state, "logger", Some(1))
            .await
            .unwrap();
        assert_eq!(
            tail_one,
            content.trim_end().lines().last().unwrap(),
            "tail=1 keeps only the final line"
        );
    }
}

/// Windows-side tests, compiled under `cfg(all(test, windows))` so
/// `cargo check --all-targets` covers them in CI: the unix test module
/// above is cfg'd out on Windows, which previously left the job-object
/// regression uncompiled there. The shell test binary still cannot *run*
/// on Windows (tauri entry-point issue, see ci.yml) — compilation is the
/// covered surface, not a runtime verification.
#[cfg(all(test, windows))]
mod windows_tests {
    /// The Windows orphan guard must actually cover spawned children: a
    /// child assigned to the KILL_ON_JOB_CLOSE job answers
    /// IsProcessInJob(hJob) = TRUE. Membership is asserted against *this*
    /// job (not NULL — an inherited sandbox job would also answer true) so
    /// the assignment itself is what is proven. Anchor of the 2026-11-09
    /// stale-process incident: without the guard, a shell death leaks live
    /// component processes that then permanently lock the install directory.
    #[tokio::test]
    async fn kill_on_close_job_covers_spawned_children() {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        // tokio Child: the production assign path goes through
        // Child::raw_handle(), only available on the tokio type
        let mut child = tokio::process::Command::new(
            std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into()),
        )
        .args(["/d", "/c", "ping", "-n", "30", "127.0.0.1"])
        .spawn()
        .unwrap();
        super::assign_child_to_kill_on_close_job(&mut child).unwrap();

        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, child.id().unwrap());
            assert!(
                process as isize != 0,
                "OpenProcess failed on the fresh child"
            );
            let mut in_job = 0i32;
            let job = super::kill_on_close_job_handle() as HANDLE;
            let ok = IsProcessInJob(process, job, &mut in_job);
            CloseHandle(process);
            assert!(ok != 0, "IsProcessInJob failed");
            assert!(
                in_job != 0,
                "the spawned child must live in the KILL_ON_JOB_CLOSE job"
            );
        }
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}
