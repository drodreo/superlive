//! Real parent-death tests. Topology: the test (outer) spawns a real
//! probe-parent process (`process-death-probe`); the parent spawns a real
//! probe-child through the production guard (`configure_command` + the
//! Windows job assign) and atomically publishes the child's pid (pending→
//! rename, pinned to the `Child` handle the parent holds). The parent then
//! blocks on a gate file; the outer test only opens the gate after proving
//! the child is alive — so the child is provably alive at the moment the
//! parent dies. The parent then exits either normally (the Child drop rides
//! the runtime teardown) or hard (`std::process::exit(0)` with the Child
//! still held — destructors skipped). The child must be gone within the
//! timeout, asserted through OS liveness APIs only (never the pid file,
//! never an internal counter).
//!
//! Failure cleanup: the outer test arms a guard on the parent pid as soon
//! as the parent is spawned, and a second guard on the child pid as soon
//! as the pid file is read — any gate/read/assert/timeout failure kills
//! whichever process its guard covers. The parent guard is disarmed right
//! after `wait()` returns (before assertions), so a recycled pid can never
//! be killed by mistake.
//!
//! Coverage matrix:
//! - every platform: `normal_parent_exit_reclaims_child` (kill_on_drop
//!   reclaim through the natural Child/runtime drop);
//! - Linux: `hard_parent_exit_kills_child_via_pdeathsig` (PR_SET_PDEATHSIG);
//! - Windows: `hard_parent_exit_kills_child_via_job_object`
//!   (KILL_ON_JOB_CLOSE) — these run for real on Windows CI;
//! - macOS: no hard-exit test — the contract explicitly does not claim
//!   crash/SIGKILL coverage there.

use std::time::{Duration, Instant};

use tokio::process::Command;

/// True when the pid is provably gone.
///
/// Unix: `kill(pid, 0)` failing with ESRCH — the pid has left the process
/// table, the strongest death signal. (A zombie would still answer kill 0,
/// but the probe parent already exited, so the child was reparented and is
/// reaped by init; the assertion never needs to tolerate one.)
///
/// Windows: `OpenProcess` + `GetExitCodeProcess` — a non-STILL_ACTIVE exit
/// code is gone, a missing process (`ERROR_INVALID_PARAMETER`) is gone;
/// anything else (access denied, query failure) keeps polling rather than
/// being read as success.
async fn process_gone(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: kill(2) with signal 0 is a pure liveness probe.
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if rc == 0 {
            return false;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, HANDLE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // Windows API STILL_ACTIVE is defined as 259.
        const STILL_ACTIVE: u32 = 259;
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) as HANDLE;
            if handle.is_null() {
                return std::io::Error::last_os_error().raw_os_error()
                    == Some(ERROR_INVALID_PARAMETER as i32);
            }
            let mut exit_code = 0u32;
            let queried = GetExitCodeProcess(handle, &mut exit_code);
            CloseHandle(handle);
            if queried == 0 {
                // Cannot tell — keep polling, never read failure as success.
                return false;
            }
            exit_code != STILL_ACTIVE
        }
    }
}

/// Kill the process for real (the failure-path guard). Unix SIGKILL;
/// Windows TerminateProcess — opened with PROCESS_TERMINATE (the right the
/// call actually needs; a query-limited handle cannot terminate). Best
/// effort — used only on the way out of a failed test.
fn kill_process(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid) as HANDLE;
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

/// Test-only RAII guard: armed on a pid (the parent or the child) as soon
/// as that pid is known, so any panic, assert failure, or timeout drops it
/// and kills the process — no failure path may leak a live process.
/// Disarmed only after the process is confirmed gone — or, for the parent,
/// right after `wait()` reaped it (before its pid could be recycled).
struct PidKillGuard(u32, bool);

impl PidKillGuard {
    fn arm(pid: u32) -> Self {
        Self(pid, true)
    }

    fn disarm(&mut self) {
        self.1 = false;
    }
}

impl Drop for PidKillGuard {
    fn drop(&mut self) {
        if self.1 {
            kill_process(self.0);
        }
    }
}

/// Shared body: spawn the probe parent in `mode` (guard armed on the
/// parent pid immediately), wait for the atomically published child pid,
/// prove the child is alive *before* the parent may die, open the gate,
/// require a clean parent exit, and require the child to be gone within
/// 5s — with the two RAII guards covering every failure path.
async fn run_parent_death_test(mode: &'static str) {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("child.pid");
    let gate_file = dir.path().join("gate");

    let mut parent = Command::new(env!("CARGO_BIN_EXE_process-death-probe"))
        .args([
            mode,
            &pid_file.to_string_lossy(),
            &gate_file.to_string_lossy(),
        ])
        .spawn()
        .expect("spawn the probe parent");
    // Armed from this instant: any failure below (pid-file wait, gate
    // write, wait timeout) drops this guard and kills the parent.
    let mut parent_guard = PidKillGuard::arm(parent.id().expect("the fresh parent carries a pid"));

    // The parent publishes the child pid atomically (pending→rename), so
    // the file only ever exists pinned to a pid the parent's Child handle
    // actually owns.
    let deadline = Instant::now() + Duration::from_secs(10);
    let content = loop {
        match std::fs::read_to_string(&pid_file) {
            Ok(content) => break content,
            Err(_) => {
                assert!(
                    Instant::now() < deadline,
                    "the probe parent never published the child pid file"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    };
    let pid: u32 = content
        .trim()
        .parse()
        .expect("the pid file carries the child pid");

    // Prove the child is alive *before* the parent is allowed to die —
    // without this, the later death assertion would prove nothing.
    assert!(
        !process_gone(pid).await,
        "the child must be alive before the parent exits ({mode})"
    );
    println!("child {pid} confirmed alive before parent exit ({mode})");
    // Child guard armed only now that the pid is known and proven live:
    // any later failure drops it and kills the child.
    let mut child_guard = PidKillGuard::arm(pid);

    // Open the gate: the parent may now exit (normally, or hard-exit with
    // the Child still held).
    std::fs::write(&gate_file, b"").expect("open the exit gate");

    // The parent must exit cleanly — code 0 for both the normal return and
    // the hard std::process::exit(0).
    let status = tokio::time::timeout(Duration::from_secs(15), parent.wait())
        .await
        .expect("the probe parent must exit within 15s")
        .expect("wait the probe parent");
    // Disarm before asserting: wait() has reaped the parent, so its pid is
    // free for reuse — the guard must never fire on a recycled pid.
    parent_guard.disarm();
    assert!(
        status.success(),
        "the probe parent ({mode}) must exit with code 0: {status}"
    );

    // The child must be gone within 5s of the parent's death.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if process_gone(pid).await {
            child_guard.disarm();
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the child must be dead within 5s after parent {mode}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Every platform: a normal parent exit must reclaim the child through the
/// natural Child drop (kill_on_drop).
#[tokio::test]
async fn normal_parent_exit_reclaims_child() {
    run_parent_death_test("parent-normal").await;
}

/// Linux: PR_SET_PDEATHSIG(SIGKILL) — the kernel kills the child the
/// moment the parent dies, destructor or not.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn hard_parent_exit_kills_child_via_pdeathsig() {
    run_parent_death_test("parent-hard").await;
}

/// Windows: KILL_ON_JOB_CLOSE — the parent's death closes the job handle
/// and the kernel terminates the members, destructor or not.
#[cfg(windows)]
#[tokio::test]
async fn hard_parent_exit_kills_child_via_job_object() {
    run_parent_death_test("parent-hard").await;
}
