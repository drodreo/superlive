//! Cross-process shutdown-contract coverage: the graceful-exit contract has
//! one carrier channel per platform, and this file exercises each carrier
//! against a real spawned child process (the shell's production shape).
//!
//! - unix: SIGTERM → graceful exit. Independently re-checked here with its
//!   own probe child; sigterm.rs carries the deeper coverage on the same
//!   channel (with and without a live /events subscriber).
//! - windows: stdin "shutdown" line → graceful exit. This is the only
//!   carrier there (no SIGTERM exists) and sigterm.rs is a unix-only no-op,
//!   so without this file the channel has zero coverage — exactly the gap
//!   that let the Windows stdin path ship untested.
//!
//! `harness = false`: `main` re-spawns this very binary with
//! `SHUTDOWN_PROBE_CHILD=1` as the probe child (the same self-re-spawn
//! pattern as sigterm.rs — no extra [[bin]] target, nothing new in the
//! published artifacts). The child serves the slsdk skeleton and reports
//! `READY <pid>` on stderr once serve() has returned; the parent half
//! delivers the platform carrier and asserts a graceful exit (code 0)
//! within a hard deadline.

use slsdk_rs::{ComponentManifest, LaunchConfig, UiDeclaration, UiType};
use std::io::BufRead;
#[cfg(windows)]
use std::io::Write;
// unconditional: spawn_probe (both platforms) spawns the probe child; only
// the unix parent uses Command for `kill`. A cfg(unix) gate here broke the
// windows compile (found by the CI windows job).
use std::process::Command;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Hard deadline for the graceful exit (the SDK's own backstop is 5s-grade;
/// 10s leaves the graceful path room to prove itself and CI runners room to
/// schedule). On timeout the child is killed and the scenario fails loudly —
/// never hangs the suite.
const EXIT_DEADLINE: Duration = Duration::from_secs(10);

fn main() {
    if std::env::var_os("SHUTDOWN_PROBE_CHILD").is_some() {
        child();
        return;
    }
    #[cfg(windows)]
    parent_stdin_shutdown();
    #[cfg(unix)]
    parent_sigterm();
}

fn probe_manifest() -> ComponentManifest {
    ComponentManifest {
        id: "shutdown-probe".to_string(),
        version: "0.1.0".to_string(),
        min_shell_version: "0.0.0".to_string(),
        namespace: "shutdown-probe".to_string(),
        display_name: "Shutdown Probe".to_string(),
        ui: Some(UiDeclaration {
            ui_type: UiType::Module,
            entry: String::new(),
        }),
        menu: None,
        routes: Vec::new(),
        launch: Some(LaunchConfig {
            command: String::new(),
            args: Vec::new(),
        }),
        files: Vec::new(),
        config: None,
        transport: None,
        update: None,
    }
}

/// The probe child: a minimal serve() that reports `READY <pid>` once
/// serve() returned and exits through the graceful path on the carrier
/// signal. On windows the serve() side installs the stdin watcher; on unix
/// the SIGTERM handler. Both are registered by the time serve() returns.
fn child() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("probe runtime");
    rt.block_on(async {
        let info = slsdk_rs::serve(axum::Router::new(), &probe_manifest())
            .await
            .expect("serve must start");
        eprintln!("READY {}", std::process::id());
        info.wait().await.expect("graceful serve exit");
    });
}

/// Spawn this binary as the probe child and resolve once it reports
/// `READY <pid>`. stdin is a pipe on every platform: the windows parent
/// writes the shutdown line into it; unix keeps it open and unused (the
/// unix SDK installs no stdin watcher — the carrier is SIGTERM only).
fn spawn_probe() -> (std::process::Child, u32) {
    let exe = std::env::current_exe().expect("test binary path");
    let mut child = Command::new(exe)
        .env("SHUTDOWN_PROBE_CHILD", "1")
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("probe child must spawn");
    let stderr = child.stderr.take().expect("piped stderr");
    let pid = std::io::BufReader::new(stderr)
        .lines()
        .find_map(|line| {
            let line = line.ok()?;
            line.strip_prefix("READY ")?.parse::<u32>().ok()
        })
        .expect("the probe child must report READY <pid>");
    (child, pid)
}

/// Wait for the child's exit within the deadline; on timeout kill it and
/// fail loudly. Returns the exit status.
fn wait_graceful_exit(child: &mut std::process::Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + EXIT_DEADLINE;
    loop {
        if let Some(status) = child.try_wait().expect("child must be waitable") {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the probe child did not exit within 10s of the shutdown signal");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The windows parent: write the `shutdown` line into the child's stdin
/// (the shell's exact carrier: lifecycle.rs sends the same line) and assert
/// a graceful exit (code 0 — never a crash or a hung process).
#[cfg(windows)]
fn parent_stdin_shutdown() {
    let (mut child, _pid) = spawn_probe();
    {
        let mut stdin = child.stdin.take().expect("piped stdin");
        stdin
            .write_all(b"shutdown\n")
            .expect("writing the shutdown line must succeed");
        // Dropping the handle also closes the pipe: an EOF is a legitimate
        // no-more-signals state for the watcher, but the shutdown line has
        // already landed.
    }
    let status = wait_graceful_exit(&mut child);
    assert!(
        status.success(),
        "stdin `shutdown` must exit gracefully (code 0), got {status}"
    );
}

/// The unix parent: deliver SIGTERM to the child pid (the shell's exact
/// carrier on unix) and assert a graceful exit (code 0 — never the default
/// disposition's 143).
#[cfg(unix)]
fn parent_sigterm() {
    let (mut child, pid) = spawn_probe();
    let delivered = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("kill(1) must be available");
    assert!(delivered.success(), "SIGTERM delivery must succeed");

    let status = wait_graceful_exit(&mut child);
    assert!(
        status.success(),
        "SIGTERM must exit gracefully (code 0), got {status}"
    );
}
