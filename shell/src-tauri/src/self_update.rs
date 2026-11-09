//! Target-side recovery for shell self-updates.
//!
//! Tauri's generic macOS restart spawns the replacement Mach-O directly from
//! the outgoing process. After a bundle replacement, that launch identity can
//! remain unable to use Local Network even after nehelper rebuilds its UUID
//! cache. A newly installed shell therefore hands itself to LaunchServices
//! once after a version change, before any component process starts. The
//! version marker makes this a single transition rather than a restart loop.

use crate::config::DataLayout;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn installed_app_bundle(executable: &Path) -> Result<PathBuf, String> {
    let macos = executable
        .parent()
        .filter(|path| path.file_name() == Some("MacOS".as_ref()))
        .ok_or_else(|| "the shell executable is not inside Contents/MacOS".to_string())?;
    let contents = macos
        .parent()
        .filter(|path| path.file_name() == Some("Contents".as_ref()))
        .ok_or_else(|| "the shell executable is not inside Contents/MacOS".to_string())?;
    contents
        .parent()
        .filter(|path| path.extension() == Some("app".as_ref()))
        .map(Path::to_path_buf)
        .ok_or_else(|| "the shell executable is not inside an installed app bundle".to_string())
}

fn write_version(layout: &DataLayout, version: &str) -> Result<(), String> {
    std::fs::create_dir_all(layout.root())
        .map_err(|e| format!("failed to create the shell data directory: {e}"))?;
    std::fs::write(layout.shell_version_file(), version)
        .map_err(|e| format!("failed to record the launched shell version: {e}"))
}

/// Returns true exactly once for a launch whose version differs from the
/// previously recorded version. The first-ever launch only records its
/// version; it is an install, not an update transition.
fn prepare_version_handoff(layout: &DataLayout, version: &str) -> Result<bool, String> {
    match std::fs::read_to_string(layout.shell_version_file()) {
        Ok(previous) if previous.trim() == version => Ok(false),
        Ok(_) => {
            write_version(layout, version)?;
            Ok(true)
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // Released versions predate the marker. An existing data root
            // without it is therefore an upgrade; only a genuinely new data
            // root is a first install and can proceed without a handoff.
            let upgraded_from_pre_marker_version = layout.root().exists();
            write_version(layout, version)?;
            Ok(upgraded_from_pre_marker_version)
        }
        Err(e) => Err(format!("failed to read the launched shell version: {e}")),
    }
}

fn spawn_launchservices_handoff(layout: &DataLayout) -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|e| format!("failed to locate the shell executable: {e}"))?;
    let bundle = installed_app_bundle(&executable)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(layout.shell_log_file())
        .map_err(|e| format!("failed to open the shell log for the updater handoff: {e}"))?;
    let log_stdout = log
        .try_clone()
        .map_err(|e| format!("failed to clone the updater handoff log handle: {e}"))?;
    let pid = std::process::id().to_string();
    Command::new("/bin/sh")
        .args([
            "-c",
            r#"while kill -0 "$1" 2>/dev/null; do sleep 0.1; done; /usr/bin/open -n "$2" || exec "$3""#,
            "superlive-updater-handoff",
            pid.as_str(),
        ])
        .arg(&bundle)
        .arg(&executable)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_stdout))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| format!("failed to start the updater relaunch handoff: {e}"))?;
    Ok(())
}

/// Run before state/server/component setup. `Ok(true)` means the outgoing
/// process is exiting and the caller must not start any services. Other
/// platforms and debug builds never need this installed-app recovery path.
pub fn handoff_updated_shell(app: &tauri::AppHandle, data_dir: &Path) -> Result<bool, String> {
    if !cfg!(target_os = "macos") || cfg!(debug_assertions) {
        return Ok(false);
    }

    let layout = DataLayout::new(data_dir);
    let version = app.package_info().version.to_string();
    if !prepare_version_handoff(&layout, &version)? {
        return Ok(false);
    }

    if let Err(e) = spawn_launchservices_handoff(&layout) {
        // The handoff was not armed. Remove the newly written marker so the
        // next launch can retry instead of permanently skipping recovery.
        let _ = std::fs::remove_file(layout.shell_version_file());
        return Err(e);
    }
    app.exit(0);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{installed_app_bundle, prepare_version_handoff};
    use crate::config::DataLayout;
    use std::path::Path;

    #[test]
    fn resolves_the_bundle_from_its_executable() {
        let executable = Path::new("/Applications/Superlive.app/Contents/MacOS/superlive-shell");
        assert_eq!(
            installed_app_bundle(executable).unwrap(),
            Path::new("/Applications/Superlive.app")
        );
    }

    #[test]
    fn rejects_a_non_bundle_executable() {
        assert!(installed_app_bundle(Path::new("/usr/local/bin/superlive-shell")).is_err());
    }

    #[test]
    fn first_launch_records_version_without_a_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path().join("fresh-data"));

        assert!(!prepare_version_handoff(&layout, "0.4.13").unwrap());
        assert_eq!(
            std::fs::read_to_string(layout.shell_version_file()).unwrap(),
            "0.4.13"
        );
    }

    #[test]
    fn pre_marker_upgrade_requests_exactly_one_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());

        assert!(prepare_version_handoff(&layout, "0.4.13").unwrap());
        assert!(!prepare_version_handoff(&layout, "0.4.13").unwrap());
    }

    #[test]
    fn corrupt_marker_returns_a_recoverable_error() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        std::fs::write(layout.shell_version_file(), [0xff]).unwrap();

        assert!(prepare_version_handoff(&layout, "0.4.13").is_err());
    }

    #[test]
    fn unchanged_version_does_not_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        prepare_version_handoff(&layout, "0.4.13").unwrap();

        assert!(!prepare_version_handoff(&layout, "0.4.13").unwrap());
    }

    #[test]
    fn version_change_requests_exactly_one_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        prepare_version_handoff(&layout, "0.4.12").unwrap();

        assert!(prepare_version_handoff(&layout, "0.4.13").unwrap());
        assert!(!prepare_version_handoff(&layout, "0.4.13").unwrap());
        assert_eq!(
            std::fs::read_to_string(layout.shell_version_file()).unwrap(),
            "0.4.13"
        );
    }
}
