//! Process-death test probe — a test vehicle, not a second production
//! guard implementation. It exercises the exact production crate
//! (`superlive-process-guard`) from a real, Tauri-free process so the
//! parent-death guarantees can be verified with real processes:
//!
//! - `child`: park for 60 seconds — nothing else. The parent owns and
//!   publishes the pid; the child only has to stay alive until a
//!   parent-death guard ends it (a natural end would make a broken-guard
//!   test pass, hence the long sleep).
//! - `parent-normal <pid-file> <gate-file>`: spawn this executable in
//!   child mode through the production guard (`configure_command` + the
//!   Windows job assign), atomically publish the child's pid (pending→
//!   rename, the file only ever exists pinned to a pid the parent holds a
//!   `Child` handle for), then hold that handle and wait for the gate
//!   file. Returning from main drops the Child inside the runtime
//!   teardown — the natural reclaim path under test.
//! - `parent-hard <pid-file> <gate-file>`: same, but exits via
//!   `std::process::exit(0)` **while still holding the Child** — no
//!   destructor ever runs, the handle never drops, and only the
//!   kernel-level guards (Linux PDEATHSIG / Windows job object) can
//!   reclaim the child. Not used on macOS: the contract explicitly does
//!   not claim crash/SIGKILL coverage there.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(windows)]
use superlive_process_guard::assign_child_to_kill_on_close_job;
use superlive_process_guard::configure_command;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args
        .get(1)
        .map(String::as_str)
        .expect(
            "usage: process-death-probe <child|parent-normal|parent-hard> <pid-file> <gate-file>",
        )
        .to_owned();

    match mode.as_str() {
        // Child mode parks synchronously (that is the point of the probe);
        // nothing else runs on this runtime.
        "child" => run_child(),
        "parent-normal" | "parent-hard" => {
            let pid_file = PathBuf::from(args.get(2).expect("missing pid-file argument"));
            let gate_file = PathBuf::from(args.get(3).expect("missing gate-file argument"));
            // Held by main from here on: parent-normal returns and the
            // drop inside the runtime teardown is the reclaim path under
            // test; parent-hard exits with the handle still held — the
            // drop never happens and only the kernel-level guards can
            // reclaim. The hold must span both paths, which is why the
            // Child is returned here instead of dropped inside the helper.
            let _child = spawn_child_and_wait(&pid_file, &gate_file).await;
            if mode == "parent-hard" {
                std::process::exit(0);
            }
        }
        other => panic!("unknown mode: {other}"),
    }
}

/// Child mode: park until killed. The parent owns and publishes the pid —
/// the child does nothing else.
fn run_child() {
    std::thread::sleep(Duration::from_secs(60));
}

/// Parent side of the topology: spawn a real child through the production
/// guard, publish the child's pid atomically (the parent holds the `Child`
/// handle that pins that pid), then block on the outer test's gate file.
/// Returns the Child so the caller can hold it across its exit path.
async fn spawn_child_and_wait(pid_file: &Path, gate_file: &Path) -> tokio::process::Child {
    let exe = std::env::current_exe().expect("probe parent: locate own executable");
    let mut command = tokio::process::Command::new(exe);
    command.args(["child"]);
    configure_command(&mut command);

    let child = command.spawn().expect("probe parent: spawn the child");

    #[cfg(windows)]
    let mut child = child;

    #[cfg(windows)]
    if let Err(e) = assign_child_to_kill_on_close_job(&mut child) {
        panic!("probe parent: job assignment failed: {e}");
    }

    // Atomic pid publish from the parent side: pending→rename, so the file
    // only ever exists pinned to the exact pid this Child handle owns —
    // no window where it names a pid the parent never held.
    let pid = child
        .id()
        .expect("probe parent: the fresh child carries a pid");
    let mut pending = pid_file.as_os_str().to_owned();
    pending.push(format!(".{pid}.pending"));
    let pending = PathBuf::from(pending);
    std::fs::write(&pending, pid.to_string()).expect("probe parent: write the pending pid file");
    std::fs::rename(&pending, pid_file).expect("probe parent: atomic pid publish");

    // Hold the Child and wait for the gate: the child must still be alive
    // when this parent dies, or the death assertion proves nothing.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !gate_file.exists() {
        assert!(
            Instant::now() < deadline,
            "probe parent: the gate never opened"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    child
}
