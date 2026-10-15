//! SIGTERM graceful-exit coverage (the process-level signal path that
//! tests/serve.rs declines to inject into the shared test process gets its
//! own harness-free binary here).
//!
//! `harness = false`: `main` re-spawns this very binary with
//! `SIGTERM_PROBE_CHILD=1` as the probe child. The parent half waits for the
//! child's `READY <pid>` line, delivers SIGTERM to that pid, and asserts a
//! graceful exit (code 0 — never the default-disposition 143). This is
//! exactly the production shape of the shell's lifecycle (spawn → run →
//! SIGTERM): serve() having returned (READY) must imply the handler is
//! installed, so any SIGTERM after READY exits gracefully.

use slsdk_rs::{ComponentManifest, LaunchConfig, UiDeclaration, UiType};
use std::io::BufRead;
use std::process::{Command, Stdio};

fn main() {
    if std::env::var_os("SIGTERM_PROBE_CHILD").is_some() {
        child();
    } else {
        parent();
    }
}

/// The probe child: a minimal serve() that reports `READY <pid>` once serve()
/// returned and exits through the graceful path on SIGTERM.
fn child() {
    let manifest = ComponentManifest {
        id: "sigterm-probe".to_string(),
        version: "0.1.0".to_string(),
        min_shell_version: "0.0.0".to_string(),
        namespace: "sigterm-probe".to_string(),
        display_name: "SIGTERM Probe".to_string(),
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
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("probe runtime");
    rt.block_on(async {
        let app = axum::Router::new();
        let info = slsdk_rs::serve(app, &manifest)
            .await
            .expect("serve must start");
        eprintln!("READY {}", std::process::id());
        info.wait().await.expect("graceful serve exit");
    });
}

/// The parent half: spawn the child, deliver SIGTERM after READY, assert a
/// graceful exit.
fn parent() {
    let exe = std::env::current_exe().expect("test binary path");
    let mut child = Command::new(exe)
        .env("SIGTERM_PROBE_CHILD", "1")
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

    // serve() has returned (READY): the handler is installed — SIGTERM now
    // must produce a graceful exit, never the default-disposition kill (143).
    let delivered = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("kill(1) must be available");
    assert!(delivered.success(), "SIGTERM delivery must succeed");

    let status = child.wait().expect("child must be waitable");
    assert!(
        status.success(),
        "SIGTERM after serve() returned must exit gracefully (code 0), got {status}"
    );
}
