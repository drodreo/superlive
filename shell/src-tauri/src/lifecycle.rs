//! Component process management: spawn (env injection) → health polling →
//! registration → supervise (exit guard).
//!
//! Lifecycle contract (architecture.md "Plane B contract · lifecycle"):
//! env injects SL_ENDPOINT/SL_TOKEN → poll GET /health (readiness probe and
//! health check share one endpoint) →
//! stop: SIGTERM (unix, nix safe wrapper) / stdin `shutdown` (windows
//! fallback) →
//! force kill after the grace timeout. stdout/stderr are inherited from the
//! shell process (log aggregation deferred to module 3, second stage).

use crate::config::{DataLayout, HEALTH_INTERVAL, STOP_GRACE};
use crate::registry::{Registry, RunningComponent};
use slsdk_rs::{ComponentManifest, Endpoint, ENV_ENDPOINT, ENV_TOKEN};
use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::watch;

#[derive(Debug)]
pub enum LifecycleError {
    /// Child process spawn failure (command missing / directory missing, etc.).
    Spawn { source: io::Error },
    /// Health probe timeout: the component did not become ready within the
    /// deadline (or the id mismatched).
    HealthTimeout { id: String },
    /// Other IO failure.
    Io { source: io::Error },
    /// The namespace is already taken.
    NamespaceTaken { namespace: String },
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifecycleError::Spawn { source } => {
                write!(f, "failed to spawn the component process: {source}")
            }
            LifecycleError::HealthTimeout { id } => {
                write!(f, "component {id} health probe timed out; it did not become ready within the deadline")
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
            LifecycleError::Spawn { source } | LifecycleError::Io { source } => Some(source),
            _ => None,
        }
    }
}

/// Allocate the component's listening descriptor: unix = UDS (the shell
/// centrally manages the sockets/ directory);
/// windows = random 127.0.0.1 port — tokio/mio has no Windows UDS
/// (ecosystem-level limitation, falls back to TCP).
async fn allocate_endpoint(layout: &DataLayout, id: &str) -> io::Result<Endpoint> {
    #[cfg(unix)]
    {
        tokio::fs::create_dir_all(layout.sockets_dir()).await?;
        let path = layout.socket_path(id);
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
        // bind :0 to get a system-assigned free port, release it, then hand
        // it to the component; a tiny race window exists when the component
        // binds (local loopback, accepted in v1) — failure surfaces as a
        // health timeout
        let _ = layout;
        let _ = id;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        drop(listener);
        Ok(Endpoint::Tcp { addr })
    }
}

/// Full component start flow: allocate endpoint → spawn (env injection) →
/// health polling →
/// register + start supervise. Any step failing reclaims the already-created
/// child process and socket file.
pub async fn spawn_component(
    layout: &DataLayout,
    registry: &Arc<Registry>,
    manifest: Arc<ComponentManifest>,
    health_timeout: Duration,
) -> Result<(), LifecycleError> {
    let endpoint = allocate_endpoint(layout, &manifest.id)
        .await
        .map_err(|source| LifecycleError::Io { source })?;
    let token = Arc::new(crate::auth::generate_token());

    // stdin: unix passes null (the unix component path does not read stdin; a
    // dangling pipe serves no purpose);
    // windows keeps the pipe — slsdk serve's stdin `shutdown` fallback
    // channel depends on it
    #[cfg(unix)]
    let mut command = {
        let mut c = Command::new(&manifest.launch.command);
        c.stdin(Stdio::null());
        c
    };
    #[cfg(windows)]
    let mut command = {
        let mut c = Command::new(&manifest.launch.command);
        c.stdin(Stdio::piped());
        c
    };
    command
        .args(&manifest.launch.args)
        .env(ENV_ENDPOINT, endpoint.to_string())
        .env(ENV_TOKEN, token.as_str())
        .current_dir(layout.component_dir(&manifest.id));

    let mut child = command
        .spawn()
        .map_err(|source| LifecycleError::Spawn { source })?;
    let pid = child.id();

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
    tokio::spawn(supervise(
        child,
        shutdown,
        stop_rx,
        Arc::clone(&manifest),
        socket_path,
        Arc::clone(registry),
        layout.clone(),
    ));
    log::info!(
        "component spawned: {} (ns={} v={} pid={pid:?} endpoint={endpoint})",
        manifest.id,
        manifest.namespace,
        manifest.version
    );
    Ok(())
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
    loop {
        match probe_health(endpoint, token).await {
            Ok(Some(id)) if id == expect_id => return Ok(()),
            Ok(other) => log::debug!("health not ready yet (response: {other:?}), keep polling"),
            Err(e) => log::debug!(
                "health probe connection failed (the component may not be listening yet): {e}"
            ),
        }
        if Instant::now() >= deadline {
            return Err(LifecycleError::HealthTimeout {
                id: expect_id.to_string(),
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

/// Exit guard: waits for a natural exit or a stop signal; a stop signal
/// triggers graceful shutdown (SIGTERM/stdin shutdown) with force-kill after
/// the grace timeout; after exit it removes the registry entry, recomputes
/// and persists the configuration plane's settled state from manifest+config,
/// and cleans up the socket file.
async fn supervise(
    mut child: Child,
    mut shutdown: ShutdownChannel,
    mut stop_rx: watch::Receiver<()>,
    manifest: Arc<ComponentManifest>,
    socket_path: Option<PathBuf>,
    registry: Arc<Registry>,
    layout: DataLayout,
) {
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
                        "component {namespace} graceful shutdown timed out, killing it forcefully"
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
        &layout.component_config_file(&manifest.id),
    )
    .await
    {
        Ok(crate::config_plane::ComponentState::NeedsConfig) => {
            crate::config_plane::ComponentState::NeedsConfig
        }
        Ok(_) => crate::config_plane::ComponentState::Stopped,
        Err(e) => {
            log::warn!("failed to determine the post-exit state of component {namespace} ({e}); conservatively marking needs-config");
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
        if let Some(mut stdin) = shutdown_stdin {
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(b"shutdown\n").await;
                let _ = stdin.shutdown().await;
            });
        }
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
            Duration::from_millis(600),
        )
        .await;
        assert!(matches!(result, Err(LifecycleError::HealthTimeout { .. })));
        assert!(registry.lookup("sleepns").await.is_none());
        // The socket file was never created by the component; the allocation
        // path should leave nothing behind
        assert!(!layout.socket_path("sleeper").exists());
    }

    // ---------- supervise graceful stop ----------

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
            Arc::new(manifest("sleepapp", "sleepns", "/bin/sleep", &["30"])),
            None,
            Arc::clone(&registry),
            layout.clone(),
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
            Arc::new(m),
            None,
            Arc::clone(&registry),
            layout.clone(),
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
    /// request_stop →
    /// graceful exit → registry removal → socket cleanup.
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
        spawn_component(&layout, &registry, m, Duration::from_secs(3))
            .await
            .unwrap();

        // Registered
        assert!(registry.lookup("e2ens").await.is_some());

        // Request graceful stop → supervise SIGTERMs /bin/sleep → removal +
        // socket cleanup
        assert!(registry.request_stop("e2ens").await);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while registry.lookup("e2ens").await.is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "it should be removed from the registry after stop"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // The socket file has been cleaned up (the mock responder's
        // connection drops with it)
        assert!(
            !sock.exists(),
            "the socket file should be cleaned up on exit"
        );
    }
}
