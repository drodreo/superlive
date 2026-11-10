//! Parent-death process guards for spawned component processes, shared by
//! the shell (production) and the process-death probe/tests (verification).
//! One implementation, exercised everywhere — no test-side re-derivation:
//!
//! - every platform: `kill_on_drop` on the tokio Command — a cancelled
//!   spawn future or an async-runtime teardown drops the `Child`, and the
//!   drop kills the process (a last-resort reclaim, never a substitute for
//!   a graceful stop);
//! - Linux: `PR_SET_PDEATHSIG(SIGKILL)` armed inside `pre_exec`, with a
//!   `getppid` race backstop for the fork→prctl window;
//! - Windows: the child is assigned to a global `KILL_ON_JOB_CLOSE` job
//!   object — once the assignment succeeds, any form of parent death
//!   closes the (never-closed) job handle and the kernel kills the job
//!   members; a failed assignment is reported to the caller, which logs it
//!   and degrades the guard for that component.

#[cfg(any(windows, target_os = "linux"))]
use std::io;
use tokio::process::Command;

/// Configure a spawn command with the platform's parent-death guards. Call
/// this on every spawned child (production and probe alike) before
/// `.spawn()`. The Windows job assignment is a post-spawn step — see
/// [`assign_child_to_kill_on_close_job`].
pub fn configure_command(command: &mut Command) {
    // Startup-window leak guard: if the spawning future is cancelled while
    // the child is still starting (health probe pending, not yet
    // registered) or the async runtime tears the supervise task down, the
    // Child drops — tokio would detach the process by default, so
    // kill_on_drop turns that drop into a kill. A guard for that window
    // only, not a substitute for the graceful shell-exit sweep (which goes
    // through a stop request + supervision).
    command.kill_on_drop(true);

    #[cfg(target_os = "linux")]
    {
        // Orphan-source fix: the kernel delivers SIGKILL to the component
        // the moment the shell dies (any form — crash, external kill,
        // normal exit), so a dead shell can no longer leak live component
        // processes. prctl/PDEATHSIG is Linux-only: macOS has no
        // kernel-level equivalent — there, a normal shell exit is handled
        // by the run-event graceful sweep plus the Child kill_on_drop
        // teardown reclaim, and a shell crash or SIGKILL is not covered by
        // either mechanism. It is also per-thread (it watches the thread
        // that forked): tokio worker threads live as long as the process,
        // which makes it safe here — never call this from a
        // `spawn_blocking` context (disposable blocking threads would kill
        // healthy children).
        let parent_pid = nix::unistd::getpid().as_raw();
        unsafe {
            command.pre_exec(move || {
                nix::sys::prctl::set_pdeathsig(nix::sys::signal::Signal::SIGKILL)
                    .map_err(io::Error::other)?;
                // Race window: if the parent died between fork and prctl,
                // the signal never fires — a parent already different from
                // the spawning process means we are an orphan right now;
                // exit immediately (async-signal-safe _exit, exec never
                // happens).
                if nix::unistd::getppid().as_raw() != parent_pid {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
    }
}

/// Windows orphan guard: assign a freshly spawned child process to a
/// global job object created with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.
///
/// The parent process owns the only job handle and never closes it: any
/// form of parent death (normal exit, crash, external kill) closes the
/// handle, and the kernel then terminates every process in the job. A
/// stale child surviving its parent is not a hypothetical: it can hold a
/// sharing violation on an install directory and block every later update
/// until killed by hand.
///
/// Failure here only degrades the guard (the child keeps running
/// normally), so callers log a warning instead of failing the spawn.
#[cfg(windows)]
pub fn assign_child_to_kill_on_close_job(child: &mut tokio::process::Child) -> io::Result<()> {
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

/// The process-lifetime KILL_ON_JOB_CLOSE job every child is assigned to.
/// Lazily created once, owned by this static (never closed): the handle
/// dies with the parent process, and the kernel then kills the job
/// members. Stored as isize (the raw HANDLE is a pointer, not
/// Send/Sync-safe for a static).
#[cfg(windows)]
static KILL_ON_CLOSE_JOB: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

/// Get (creating on first use) the global KILL_ON_JOB_CLOSE job handle (as
/// isize; cast to HANDLE at the call sites). Crate-private: only this
/// crate's unit test asserts membership against this *specific* job (an
/// IsProcessInJob(NULL) check would also answer true for an inherited
/// sandbox job, which would not prove the guard did anything).
#[cfg(windows)]
fn kill_on_close_job_handle() -> isize {
    *KILL_ON_CLOSE_JOB.get_or_init(|| unsafe {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
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
                "KILL_ON_JOB_CLOSE not applied to the child job object (orphan guard inert): {}",
                io::Error::last_os_error()
            );
        }
        job as isize
    })
}

#[cfg(all(test, windows))]
mod windows_tests {
    /// The Windows orphan guard must actually cover spawned children: a
    /// child assigned to the KILL_ON_JOB_CLOSE job answers
    /// IsProcessInJob(hJob) = TRUE. Membership is asserted against *this*
    /// job (not NULL — an inherited sandbox job would also answer true) so
    /// the assignment itself is what is proven. This test runs for real on
    /// Windows CI (the shell crate's own test binary cannot run there).
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
