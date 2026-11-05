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

use slsdk_rs::{ComponentManifest, Endpoint, LaunchConfig, UiDeclaration, UiType};
use std::io::BufRead;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn main() {
    if std::env::var_os("SIGTERM_PROBE_CHILD").is_some() {
        child();
    } else if std::env::var_os("SIGTERM_PROBE_CHILD_EVENTS").is_some() {
        child_events();
    } else {
        parent_no_subscription();
        parent_with_subscription();
    }
}

fn probe_manifest() -> ComponentManifest {
    ComponentManifest {
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
    }
}

/// The probe child: a minimal serve() that reports `READY <pid>` once serve()
/// returned and exits through the graceful path on SIGTERM.
fn child() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("probe runtime");
    rt.block_on(async {
        let app = axum::Router::new();
        let info = slsdk_rs::serve(app, &probe_manifest())
            .await
            .expect("serve must start");
        eprintln!("READY {}", std::process::id());
        info.wait().await.expect("graceful serve exit");
    });
}

/// The events probe child: like [`child`], but it holds a live subscription
/// to its own `/events` feed before reporting READY. This is the shape that
/// used to hang graceful shutdown forever (the notifier never drops, so the
/// stream never ends); after the fix the SIGTERM path tears the stream down
/// and the process still exits gracefully.
fn child_events() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("probe runtime");
    rt.block_on(async {
        let info = slsdk_rs::serve(axum::Router::new(), &probe_manifest())
            .await
            .expect("serve must start");
        let addr = match &info.endpoint {
            Endpoint::Tcp { addr } => *addr,
            other => panic!("standalone serve must be tcp, got {other:?}"),
        };
        // Subscribe before READY: the subscription must be live when the
        // parent delivers SIGTERM.
        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("the probe must reach its own /events endpoint");
        stream
            .write_all(b"GET /events HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .expect("writing the events request must succeed");
        let mut head = Vec::new();
        let mut chunk = [0u8; 256];
        loop {
            let n = stream
                .read(&mut chunk)
                .await
                .expect("reading the events head must succeed");
            assert!(n > 0, "connection closed before the response head");
            head.extend_from_slice(&chunk[..n]);
            if head.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let head_text = String::from_utf8_lossy(&head);
        assert!(
            head_text.starts_with("HTTP/1.1 200"),
            "the events endpoint should answer 200, head: {head_text}"
        );
        // Keep the subscription alive: drain the stream in the background
        // until the server ends it (EOF) during shutdown.
        tokio::spawn(async move {
            let mut drain = [0u8; 256];
            loop {
                match stream.read(&mut drain).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
        eprintln!("READY {}", std::process::id());
        info.wait().await.expect("graceful serve exit");
    });
}

/// Spawns this binary as a probe child (the env key selects the child role)
/// and resolves once the child reports `READY <pid>`.
fn spawn_probe(env_key: &str) -> (std::process::Child, u32) {
    let exe = std::env::current_exe().expect("test binary path");
    let mut child = Command::new(exe)
        .env(env_key, "1")
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

/// Delivers SIGTERM and waits for a graceful exit, with a hard deadline: on
/// timeout the child is SIGKILLed and the scenario fails loudly (never hangs
/// the test suite).
fn assert_graceful_exit_after_sigterm(child: &mut std::process::Child, pid: u32) {
    let delivered = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("kill(1) must be available");
    assert!(delivered.success(), "SIGTERM delivery must succeed");

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("child must be waitable") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
            panic!("the probe child did not exit within 10s of SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        status.success(),
        "SIGTERM after serve() returned must exit gracefully (code 0), got {status}"
    );
}

/// The parent half, scenario 1 (the original coverage): spawn the child,
/// deliver SIGTERM after READY, assert a graceful exit — no subscriber.
fn parent_no_subscription() {
    let (mut child, pid) = spawn_probe("SIGTERM_PROBE_CHILD");
    assert_graceful_exit_after_sigterm(&mut child, pid);
}

/// The parent half, scenario 2: same, but the child holds a live `/events`
/// subscription when SIGTERM lands. The shell's own fallback is a 5s SIGKILL;
/// the deadline here (10s) leaves the graceful path room to prove itself.
fn parent_with_subscription() {
    let (mut child, pid) = spawn_probe("SIGTERM_PROBE_CHILD_EVENTS");
    assert_graceful_exit_after_sigterm(&mut child, pid);
}
