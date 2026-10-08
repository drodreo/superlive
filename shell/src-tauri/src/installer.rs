//! Install flow: zip extraction → manifest read + validation → fingerprint
//! verification → landing (atomic replacement) → registration.
//!
//! S5 decision: the manifest is kept in the shell data directory
//! (registry/{id}.json) and is not part of the install directory's contents.
//! Therefore manifest.json is removed from the working directory right after
//! extraction, so install directory = pure files-list contents and the
//! slsdk_rs::verify_files set-difference check (anti-smuggling) holds; if
//! the files list contains manifest.json, the install is rejected outright
//! (packages conflicting with the S5 decision are stopped at the door).
//! Landing uses a same-disk rename (tmp/ and components/ both live under
//! <data>, avoiding a cross-filesystem EXDEV).

use crate::config::DataLayout;
use crate::registry;
use slsdk_rs::{ComponentManifest, VerifyError};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum InstallError {
    /// zip read/unpack failure.
    Zip { source: zip::result::ZipError },
    /// Filesystem failure.
    Io {
        path: String,
        source: std::io::Error,
    },
    /// Package root missing manifest.json.
    MissingManifest,
    /// manifest.json is not valid contract JSON.
    ManifestJson { source: serde_json::Error },
    /// Manifest semantic validation failed (slsdk_rs::validate:
    /// id/namespace/files rule set).
    InvalidManifest { source: slsdk_rs::ManifestError },
    /// The files list contains manifest.json — conflicts with the S5 decision
    /// (the manifest is kept outside the install directory).
    ManifestInFiles,
    /// Fingerprint verification failed (missing / mismatch / smuggling,
    /// slsdk_rs::verify_files).
    Verify { source: VerifyError },
    /// The namespace is already taken by another component.
    NamespaceTaken { namespace: String },
    /// The component's declared minimum shell version is higher than the
    /// current shell (slsdk manifest contract: install-time compatibility
    /// gate).
    IncompatibleShell { min: String, shell: String },
    /// The extraction/verification background task terminated abnormally (panic).
    Join,
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Zip { source } => write!(f, "failed to read the install package: {source}"),
            InstallError::Io { path, source } => write!(f, "install IO failure ({path}): {source}"),
            InstallError::MissingManifest => {
                write!(f, "manifest.json missing from the install package root")
            }
            InstallError::ManifestJson { source } => {
                write!(f, "manifest.json is not valid contract JSON: {source}")
            }
            InstallError::InvalidManifest { source } => write!(f, "manifest validation failed: {source}"),
            InstallError::ManifestInFiles => write!(
                f,
                "the files list contains manifest.json (the manifest lives outside the install directory and must not be listed in files)"
            ),
            InstallError::Verify { source } => write!(f, "file fingerprint verification failed: {source}"),
            InstallError::NamespaceTaken { namespace } => {
                write!(f, "the namespace is already taken by another component: {namespace}")
            }
            InstallError::IncompatibleShell { min, shell } => {
                write!(f, "the component requires shell version >= {min}, current shell version {shell}")
            }
            InstallError::Join => write!(f, "the install background task terminated unexpectedly"),
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InstallError::Zip { source } => Some(source),
            InstallError::Io { source, .. } => Some(source),
            InstallError::ManifestJson { source } => Some(source),
            InstallError::InvalidManifest { source } => Some(source),
            InstallError::Verify { source } => Some(source),
            _ => None,
        }
    }
}

/// Install entry point. On success the manifest has been landed and
/// registered (registry/{id}.json); the return value lets the caller (the
/// second-stage UI / management command) present the install result.
pub async fn install(
    zip_path: &Path,
    layout: &DataLayout,
) -> Result<ComponentManifest, InstallError> {
    // Same-disk temporary workspace: belongs to <data> together with
    // components/, so rename is atomic and stays on one filesystem
    let work = layout
        .tmp_dir()
        .join(format!("install-{}", &crate::auth::generate_token()[..12]));

    // Directory self-defense: install is a public API and does not require
    // the caller to ensure_dirs first (the namespace check reads registry,
    // extraction writes tmp, landing writes components — all after this);
    // async version: no synchronous file IO on the tokio worker thread
    if let Err(source) = layout.ensure_dirs_async().await {
        return Err(InstallError::Io {
            path: "<ensure_dirs>".to_string(),
            source,
        });
    }

    let result = install_inner(zip_path, layout, &work).await;
    if result.is_err() {
        // Failure cleanup: no workspace leftovers (on success work has
        // already been renamed away; removing a nonexistent path is a no-op)
        let _ = tokio::fs::remove_dir_all(&work).await;
    }
    result
}

/// Parse a "major.minor.patch" three-segment version (missing segments
/// count as 0; returns None on parse failure).
fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Whether the shell version satisfies the component's declared minimum
/// (shell >= min to install).
/// Invalid version strings are always treated as unsatisfied (slsdk validate
/// blocks well-formed ones; this is a backstop against evasion).
fn shell_version_satisfied(min_shell_version: &str) -> bool {
    match (
        parse_semver(min_shell_version),
        parse_semver(env!("CARGO_PKG_VERSION")),
    ) {
        (Some(min), Some(shell)) => shell >= min,
        _ => false,
    }
}

/// Component config backup restore: renames the config.toml backup in tmp
/// back into the target directory.
/// On failure returns (backup path, error) for the caller to log as error —
/// the backup stays in tmp for manual recovery.
async fn restore_config_backup(
    backup: &Path,
    to_dir: &Path,
) -> Result<(), (PathBuf, std::io::Error)> {
    let dest = to_dir.join("config.toml");
    match tokio::fs::rename(backup, &dest).await {
        Ok(()) => Ok(()),
        Err(source) => Err((backup.to_path_buf(), source)),
    }
}

async fn install_inner(
    zip_path: &Path,
    layout: &DataLayout,
    work: &Path,
) -> Result<ComponentManifest, InstallError> {
    // Heavy synchronous work (extraction + fingerprint verification) runs on
    // the blocking thread pool: both the zip library and sha256 are sync APIs
    let manifest = {
        let zip_path = zip_path.to_path_buf();
        let work = work.to_path_buf();
        tokio::task::spawn_blocking(move || extract_and_verify(&zip_path, &work))
            .await
            .map_err(|_| InstallError::Join)?
    }?;

    // Compatibility gate (slsdk manifest field contract: the shell performs
    // the compatibility check at install time) — an old shell installing a
    // new component would otherwise fail with the vague form of a health
    // timeout
    if !shell_version_satisfied(&manifest.min_shell_version) {
        return Err(InstallError::IncompatibleShell {
            min: manifest.min_shell_version.clone(),
            shell: env!("CARGO_PKG_VERSION").to_string(),
        });
    }

    // Namespace conflicts are judged by on-disk registration (the shell may
    // not have started an installed component yet)
    let installed = registry::load_all_manifests(layout)
        .await
        .map_err(|source| InstallError::Io {
            path: layout.registry_dir().display().to_string(),
            source,
        })?;
    if installed
        .iter()
        .any(|m| m.namespace == manifest.namespace && m.id != manifest.id)
    {
        return Err(InstallError::NamespaceTaken {
            namespace: manifest.namespace.clone(),
        });
    }

    // Landing: same-id reinstall = replacement (roadmap "upgrade atomicity":
    // rename-replace after the temp directory passes verification).
    // POSIX rename cannot overwrite a non-empty directory, so the old
    // directory is moved aside first (tiny window; accepted in v1)
    let target = layout.component_dir(&manifest.id);
    // Component config protection (configuration-plane contract: config.toml
    // is the single source of truth): the component reads/writes config.toml
    // inside the install directory via a cwd-relative path, so a whole
    // directory replacement would take it with it — stash it in tmp (same-disk
    // rename) before replacement, then restore it over the package placeholder
    // after landing. Backup failure = upgrade aborted (continuing would lose
    // user config); restore failure does not roll back the install, but the
    // backup stays in tmp and an error is logged for manual recovery.
    let config_file = target.join("config.toml");
    let config_backup = layout
        .tmp_dir()
        .join(format!("config-keep-{}.toml", manifest.id));
    let mut config_kept = false;
    if tokio::fs::try_exists(&target)
        .await
        .map_err(|source| InstallError::Io {
            path: target.display().to_string(),
            source,
        })?
    {
        let replaced = layout.tmp_dir().join(format!("replaced-{}", manifest.id));
        // Backstop for leftovers of an interrupted previous replacement
        let _ = tokio::fs::remove_dir_all(&replaced).await;
        if tokio::fs::try_exists(&config_file)
            .await
            .map_err(|source| InstallError::Io {
                path: config_file.display().to_string(),
                source,
            })?
        {
            let _ = tokio::fs::remove_file(&config_backup).await;
            tokio::fs::rename(&config_file, &config_backup)
                .await
                .map_err(|source| InstallError::Io {
                    path: config_file.display().to_string(),
                    source,
                })?;
            config_kept = true;
        }
        // Calls restore_config_backup directly instead of via a closure: a
        // closure returning a future that captures parameter references
        // cannot express the HRTB (lifetime may not live long enough);
        // calling the async fn directly has no such issue with the argument
        // borrows
        if let Err(source) = tokio::fs::rename(&target, &replaced).await {
            // Moving the old directory away failed: restore the config backup
            // to its original place (no install happened; the original
            // directory's shape must be intact)
            if config_kept {
                if let Err((backup, e)) = restore_config_backup(&config_backup, &target).await {
                    log::error!(
                        "failed to restore the component config backup ({} left in tmp for manual recovery): {e}",
                        backup.display()
                    );
                }
            }
            return Err(InstallError::Io {
                path: target.display().to_string(),
                source,
            });
        }
        if let Err(source) = tokio::fs::rename(work, &target).await {
            // Mid-way failure, move back: restore the old version from
            // replaced to target, never leaving a "target missing" state
            // (if moving back fails it can only be logged — a missing target
            // is an environment-level failure requiring manual intervention)
            if let Err(e) = tokio::fs::rename(&replaced, &target).await {
                log::error!(
                    "failed to move the replacement back into place ({} → {}): {e}",
                    replaced.display(),
                    target.display()
                );
            }
            // If moving back succeeded the old directory's shape is intact;
            // restore the config backup to its original place
            if config_kept {
                if let Err((backup, e)) = restore_config_backup(&config_backup, &target).await {
                    log::error!(
                        "failed to restore the component config backup ({} left in tmp for manual recovery): {e}",
                        backup.display()
                    );
                }
            }
            return Err(InstallError::Io {
                path: target.display().to_string(),
                source,
            });
        }
        // Old-version cleanup failure does not affect the install result
        // (registry registration follows the new manifest); the leftover
        // directory waits for the backstop cleanup of the next same-id
        // replacement
        if let Err(e) = tokio::fs::remove_dir_all(&replaced).await {
            log::warn!(
                "failed to clean up the old directory (does not affect the install outcome, {}): {e}",
                replaced.display()
            );
        }
    } else {
        tokio::fs::rename(work, &target)
            .await
            .map_err(|source| InstallError::Io {
                path: target.display().to_string(),
                source,
            })?;
    }
    // Restore the component config (upgrade success path): overwrites the
    // same-named placeholder file in the new package
    if config_kept {
        match tokio::fs::rename(&config_backup, &config_file).await {
            Ok(()) => {}
            Err(source) => log::error!(
                "failed to restore the component config ({} is still in tmp and can be restored to {} manually): {source}",
                config_backup.display(),
                config_file.display()
            ),
        }
    }

    // Registration persistence: persist_manifest_sync is synchronous IO,
    // wrapped in the blocking thread pool
    {
        // The path in the error message is computed up front: layout/manifest
        // are about to be moved into the closure
        let registry_path = layout.registry_file(&manifest.id).display().to_string();
        let layout = layout.clone();
        let manifest = manifest.clone();
        tokio::task::spawn_blocking(move || registry::persist_manifest_sync(&layout, &manifest))
            .await
            .map_err(|_| InstallError::Join)?
            .map_err(|source| InstallError::Io {
                path: registry_path,
                source,
            })?;
    }

    Ok(manifest)
}

/// Extract → read manifest → validate → remove manifest → fingerprint
/// verification.
/// Fully synchronous, executed inside spawn_blocking.
fn extract_and_verify(zip_path: &Path, work: &Path) -> Result<ComponentManifest, InstallError> {
    std::fs::create_dir_all(work).map_err(|source| InstallError::Io {
        path: work.display().to_string(),
        source,
    })?;

    let file = std::fs::File::open(zip_path).map_err(|source| InstallError::Io {
        path: zip_path.display().to_string(),
        source,
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|source| InstallError::Zip { source })?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|source| InstallError::Zip { source })?;
        // enclosed_name rejects zip slip (`..` escapes and absolute paths)
        let Some(rel) = entry.enclosed_name() else {
            return Err(InstallError::Io {
                path: entry.name().to_string(),
                source: std::io::Error::other("illegal path inside the install package (zip slip)"),
            });
        };
        let out: PathBuf = work.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|source| InstallError::Io {
                path: out.display().to_string(),
                source,
            })?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|source| InstallError::Io {
                    path: parent.display().to_string(),
                    source,
                })?;
            }
            let mut writer = std::fs::File::create(&out).map_err(|source| InstallError::Io {
                path: out.display().to_string(),
                source,
            })?;
            std::io::copy(&mut entry, &mut writer).map_err(|source| InstallError::Io {
                path: out.display().to_string(),
                source,
            })?;
            // unix executable-bit restoration: the component binary cannot
            // start without it
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode)).map_err(
                    |source| InstallError::Io {
                        path: out.display().to_string(),
                        source,
                    },
                )?;
            }
        }
    }

    let manifest_path = work.join("manifest.json");
    let bytes = std::fs::read(&manifest_path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            InstallError::MissingManifest
        } else {
            InstallError::Io {
                path: manifest_path.display().to_string(),
                source,
            }
        }
    })?;
    let manifest: ComponentManifest =
        serde_json::from_slice(&bytes).map_err(|source| InstallError::ManifestJson { source })?;
    manifest
        .validate()
        .map_err(|source| InstallError::InvalidManifest { source })?;

    if manifest.files.iter().any(|e| e.path == "manifest.json") {
        return Err(InstallError::ManifestInFiles);
    }
    // S5: move the manifest out of the working directory — install directory
    // = pure files contents, so the verify set-difference check holds
    std::fs::remove_file(&manifest_path).map_err(|source| InstallError::Io {
        path: manifest_path.display().to_string(),
        source,
    })?;
    slsdk_rs::verify_files(work, &manifest).map_err(|source| InstallError::Verify { source })?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn sha256_hex(data: &[u8]) -> String {
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Build an install package: manifest.json + files (each with optional
    /// unix permissions).
    fn make_zip(path: &Path, manifest_json: &str, files: &[(&str, &[u8], u32)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let plain =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("manifest.json", plain).unwrap();
        writer.write_all(manifest_json.as_bytes()).unwrap();
        for (name, content, mode) in files {
            let options = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(*mode);
            writer.start_file(*name, options).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap();
    }

    /// Assemble the manifest.json text: files as (path, content), fingerprints
    /// computed on the spot to guarantee consistency.
    fn manifest_json(id: &str, ns: &str, version: &str, files: &[(&str, &[u8])]) -> String {
        let entries: Vec<String> = files
            .iter()
            .map(|(p, c)| format!(r#"{{ "path": "{p}", "sha256": "{}" }}"#, sha256_hex(c)))
            .collect();
        format!(
            r#"{{
                "id": "{id}",
                "version": "{version}",
                "min_shell_version": "0.1.0",
                "namespace": "{ns}",
                "display_name": "{id}",
                "ui": {{ "ui_type": "module", "entry": "assets/main.js" }},
                "menu": null,
                "routes": [],
                "launch": {{ "command": "./bin", "args": [] }},
                "files": [{}]
            }}"#,
            entries.join(",")
        )
    }

    #[tokio::test]
    async fn incompatible_min_shell_version_rejected() {
        // Compatibility gate (slsdk manifest contract): the component's
        // declared minimum shell version higher than the current shell →
        // reject install
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let content = b"x";
        let mj = manifest_json("futureapp", "future-ns", "1.0.0", &[("bin", content)]).replace(
            "\"min_shell_version\": \"0.1.0\"",
            "\"min_shell_version\": \"999.0.0\"",
        );
        let zip_path = dir.path().join("pkg.zip");
        make_zip(&zip_path, &mj, &[("bin", content, 0o755)]);
        let result = install(&zip_path, &layout).await;
        assert!(
            matches!(
                result,
                Err(InstallError::IncompatibleShell { ref min, .. }) if min == "999.0.0"
            ),
            "the install should be rejected due to an insufficient shell version: {result:?}"
        );
        // Rejected install leaves no directory
        assert!(!layout.component_dir("futureapp").exists());
    }

    #[tokio::test]
    async fn installs_verifies_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let zip_path = dir.path().join("pkg.zip");
        let bin = b"#!/bin/sh\necho fake-server\n";
        let data = b"hello";
        let mj = manifest_json(
            "demo",
            "demo-ns",
            "1.0.0",
            &[("bin", bin), ("assets/data.txt", data)],
        );
        make_zip(
            &zip_path,
            &mj,
            &[("bin", bin, 0o755), ("assets/data.txt", data, 0o644)],
        );

        let manifest = install(&zip_path, &layout).await.unwrap();
        assert_eq!(manifest.id, "demo");
        assert_eq!(manifest.namespace, "demo-ns");

        // Landing check one: install directory contents match the fingerprints
        let installed_bin = layout.component_dir("demo").join("bin");
        assert_eq!(std::fs::read(&installed_bin).unwrap(), bin);
        assert_eq!(
            std::fs::read(layout.component_dir("demo").join("assets/data.txt")).unwrap(),
            data
        );
        // Executable-bit restoration (unix)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&installed_bin)
                .unwrap()
                .permissions()
                .mode();
            assert_ne!(mode & 0o111, 0, "the binary should keep its executable bit");
        }

        // Landing check two: manifest registered in the shell data directory
        // (not inside the install directory — S5)
        let registry_file = layout.registry_file("demo");
        let stored: ComponentManifest =
            serde_json::from_slice(&std::fs::read(&registry_file).unwrap()).unwrap();
        assert_eq!(stored.version, "1.0.0");
        assert!(!layout.component_dir("demo").join("manifest.json").exists());

        // Landing check three: no workspace leftovers
        assert_eq!(std::fs::read_dir(layout.tmp_dir()).unwrap().count(), 0);
    }

    /// Upgrade preserves config (configuration-plane contract): config.toml
    /// is the single source of truth and must not be taken away by a
    /// directory replacement — values the user wrote before the upgrade
    /// survive verbatim after it (overwriting the new package's placeholder).
    #[tokio::test]
    async fn upgrade_preserves_component_config() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let bin = b"#!/bin/sh\n";
        let placeholder = b"value = \"placeholder\"\n";

        // First install v1 (config.toml placeholder landed)
        let v1 = manifest_json(
            "cfgapp",
            "cfg-ns",
            "1.0.0",
            &[("bin", bin), ("config.toml", placeholder)],
        );
        let zip1 = dir.path().join("v1.zip");
        make_zip(
            &zip1,
            &v1,
            &[("bin", bin, 0o755), ("config.toml", placeholder, 0o644)],
        );
        install(&zip1, &layout).await.unwrap();

        // User config written (simulating component/shell-side write-back)
        let config_path = layout.component_dir("cfgapp").join("config.toml");
        std::fs::write(&config_path, b"value = \"user-set\"\n").unwrap();

        // Upgrade to v2 (new package's placeholder content differs)
        let placeholder_v2 = b"value = \"placeholder-v2\"\n";
        let v2 = manifest_json(
            "cfgapp",
            "cfg-ns",
            "2.0.0",
            &[("bin", bin), ("config.toml", placeholder_v2)],
        );
        let zip2 = dir.path().join("v2.zip");
        make_zip(
            &zip2,
            &v2,
            &[("bin", bin, 0o755), ("config.toml", placeholder_v2, 0o644)],
        );
        let upgraded = install(&zip2, &layout).await.unwrap();
        assert_eq!(upgraded.version, "2.0.0");

        // Config survives: the user value overwrites the new package's placeholder
        let after = std::fs::read(&config_path).unwrap();
        assert_eq!(after, b"value = \"user-set\"\n".to_vec());
        // No backup leftovers (tmp clean after a successful restore)
        assert!(!layout.tmp_dir().join("config-keep-cfgapp.toml").exists());
    }

    #[tokio::test]
    async fn tampered_fingerprint_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let zip_path = dir.path().join("pkg.zip");
        // The fingerprint declared by the manifest mismatches the actual
        // content (content was modified)
        let mj = manifest_json("demo", "demo-ns", "1.0.0", &[("bin", b"original")]);
        make_zip(&zip_path, &mj, &[("bin", b"tampered", 0o644)]);

        let result = install(&zip_path, &layout).await;
        assert!(
            matches!(result, Err(InstallError::Verify { .. })),
            "a tampered fingerprint should be rejected: {result:?}"
        );
        // No landing after rejection
        assert!(!layout.component_dir("demo").exists());
    }

    #[tokio::test]
    async fn missing_manifest_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let zip_path = dir.path().join("pkg.zip");
        // Hand-build a package without manifest.json (make_zip always writes
        // the manifest; this bypasses it)
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("only-file.txt", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"no manifest here").unwrap();
        writer.finish().unwrap();

        let result = install(&zip_path, &layout).await;
        assert!(matches!(result, Err(InstallError::MissingManifest)));
    }

    #[tokio::test]
    async fn malformed_manifest_json_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let zip_path = dir.path().join("pkg.zip");
        // manifest.json exists but is not valid contract JSON (missing
        // required fields)
        make_zip(&zip_path, "{}", &[]);

        let result = install(&zip_path, &layout).await;
        assert!(matches!(result, Err(InstallError::ManifestJson { .. })));
    }

    #[tokio::test]
    async fn manifest_listed_in_files_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let zip_path = dir.path().join("pkg.zip");
        let content = b"x";
        let mut mj = manifest_json("demo", "demo-ns", "1.0.0", &[("bin", content)]);
        // Stuff manifest.json into files: conflicts with the S5 decision
        mj = mj.replace(
            "\"files\": [",
            &format!(
                "\"files\": [{{ \"path\": \"manifest.json\", \"sha256\": \"{}\" }},",
                sha256_hex(mj.as_bytes())
            ),
        );
        make_zip(&zip_path, &mj, &[("bin", content, 0o644)]);

        let result = install(&zip_path, &layout).await;
        assert!(
            matches!(result, Err(InstallError::ManifestInFiles)),
            "files containing manifest.json should be rejected: {result:?}"
        );
    }

    #[tokio::test]
    async fn invalid_manifest_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let zip_path = dir.path().join("pkg.zip");
        // Namespace contains illegal characters (slsdk validate rule: letters,
        // digits, and hyphens only)
        let content = b"x";
        let mj = manifest_json("demo", "bad/ns", "1.0.0", &[("bin", content)]);
        make_zip(&zip_path, &mj, &[("bin", content, 0o644)]);

        let result = install(&zip_path, &layout).await;
        assert!(matches!(result, Err(InstallError::InvalidManifest { .. })));
    }

    #[tokio::test]
    async fn namespace_conflict_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let content = b"x";
        let first = dir.path().join("a.zip");
        make_zip(
            &first,
            &manifest_json("app-a", "shared-ns", "1.0.0", &[("bin", content)]),
            &[("bin", content, 0o644)],
        );
        let second = dir.path().join("b.zip");
        make_zip(
            &second,
            &manifest_json("app-b", "shared-ns", "1.0.0", &[("bin", content)]),
            &[("bin", content, 0o644)],
        );

        install(&first, &layout).await.unwrap();
        let result = install(&second, &layout).await;
        assert!(matches!(
            result,
            Err(InstallError::NamespaceTaken { namespace }) if namespace == "shared-ns"
        ));
    }

    #[tokio::test]
    async fn same_id_reinstall_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let content_v1 = b"version-one";
        let content_v2 = b"version-two";
        let v1 = dir.path().join("v1.zip");
        make_zip(
            &v1,
            &manifest_json("demo", "demo-ns", "1.0.0", &[("bin", content_v1)]),
            &[("bin", content_v1, 0o644)],
        );
        let v2 = dir.path().join("v2.zip");
        make_zip(
            &v2,
            &manifest_json("demo", "demo-ns", "2.0.0", &[("bin", content_v2)]),
            &[("bin", content_v2, 0o644)],
        );

        install(&v1, &layout).await.unwrap();
        install(&v2, &layout).await.unwrap();

        // Install directory has been replaced by v2
        assert_eq!(
            std::fs::read(layout.component_dir("demo").join("bin")).unwrap(),
            content_v2
        );
        // Registration updated in sync
        let stored: ComponentManifest =
            serde_json::from_slice(&std::fs::read(layout.registry_file("demo")).unwrap()).unwrap();
        assert_eq!(stored.version, "2.0.0");
        // No workspace leftovers
        assert_eq!(std::fs::read_dir(layout.tmp_dir()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn zip_slip_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let zip_path = dir.path().join("evil.zip");
        // Hand-build a package with a ../ escape entry (enclosed_name should
        // reject it)
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        writer.start_file("../escaped.txt", options).unwrap();
        writer.write_all(b"evil").unwrap();
        writer.finish().unwrap();

        let result = install(&zip_path, &layout).await;
        assert!(matches!(result, Err(InstallError::Io { .. })));
    }

    #[tokio::test]
    async fn extra_undeclared_file_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let zip_path = dir.path().join("pkg.zip");
        let content = b"x";
        // Package carries a file outside the files list: the set-difference
        // check should reject it (anti-smuggling)
        let mj = manifest_json("demo", "demo-ns", "1.0.0", &[("bin", content)]);
        make_zip(
            &zip_path,
            &mj,
            &[("bin", content, 0o644), ("smuggled.txt", b"sneaky", 0o644)],
        );

        let result = install(&zip_path, &layout).await;
        assert!(matches!(result, Err(InstallError::Verify { .. })));
    }
}
