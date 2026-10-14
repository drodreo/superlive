//! Registry: runtime registration (namespace → component runtime info) +
//! disk persistence (envelope records).
//!
//! The namespace is the gateway dispatch key (architecture.md: the first URL
//! segment is the namespace).
//! The disk layer stores a [`RegistryEntry`] envelope (registry/{id}.json):
//! `installed` components deploy from the install directory, `dev`
//! components point at a user project directory (registered by the shell's
//! built-in creation flow, reloaded on restart). Runtime info
//! (endpoint/token/stop handle) lives with the shell's lifecycle, and on
//! shell restart the supervisor scans the disk records and starts
//! components again.

use crate::config::DataLayout;
use serde::{Deserialize, Serialize};
use slsdk_rs::{ComponentManifest, Endpoint};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{watch, RwLock};

/// Where a registered component's body lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComponentSource {
    /// Installed from a package: files in `components/{id}/`, no project dir.
    Installed,
    /// Registered by the built-in creation flow: the component's body stays
    /// in the user's project directory (`project_dir` is always present);
    /// rebuild + stop/start picks up the new artifacts (reload-is-upgrade).
    Dev,
}

/// Disk envelope of a registry record (registry/{id}.json). The bare
/// manifest form predates the envelope; the read side accepts both (a
/// top-level record without a "manifest" key reads as `Installed`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub source: ComponentSource,
    /// Dev component's project directory (launch commands resolve relative
    /// to it; the static server mounts its `web/dist`). Always `None` for
    /// installed entries.
    #[serde(default)]
    pub project_dir: Option<PathBuf>,
    pub manifest: ComponentManifest,
    /// Registration timestamp (unix epoch milliseconds), captured once at
    /// create/install — a lifecycle identity fact, never refreshed. Absent
    /// in records written before the field existed (reads as None; those
    /// sort last).
    #[serde(default)]
    pub registered_at: Option<u64>,
}

impl RegistryEntry {
    /// The installed-form envelope (the install flow's persistence shape).
    /// This constructor is the install path's single assembly point, so the
    /// registration timestamp is captured right here: the envelope is built
    /// exactly once, at install time (refresh paths rebuild dev envelopes
    /// from the existing entry and never come through here).
    pub fn installed(manifest: ComponentManifest) -> Self {
        Self {
            source: ComponentSource::Installed,
            project_dir: None,
            manifest,
            registered_at: Some(now_ms()),
        }
    }
}

/// Current unix epoch time in milliseconds. Panics only if the system clock
/// is set before 1970 — not a real-world state for a desktop shell, so a
/// bare unwrap with this note is the honest form (no error plumbing for a
/// cannot-happen case).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Registration entry of a running component.
pub struct RunningComponent {
    pub manifest: Arc<ComponentManifest>,
    /// The listen descriptor the shell allocated for the component (the
    /// gateway forwarding target).
    pub endpoint: Endpoint,
    /// per-component token (injected into Authorization when the gateway
    /// forwards; the component side's first wall).
    pub token: Arc<String>,
    /// Component child process pid (needed for unix graceful-exit SIGTERM).
    pub pid: Option<u32>,
    /// Stop signal: send triggers supervise's graceful-exit flow.
    pub stop: watch::Sender<()>,
}

/// Snapshot from the gateway's table lookup (no process handles, cheap to
/// clone).
#[derive(Clone)]
pub struct RunningSnapshot {
    pub manifest: Arc<ComponentManifest>,
    pub endpoint: Endpoint,
    pub token: Arc<String>,
}

/// Entry reported by /_shell/registry.
#[derive(Debug, Serialize)]
pub struct RegistrySummary {
    pub namespace: String,
    pub id: String,
    pub version: String,
    pub endpoint: String,
}

/// Runtime table operation errors.
#[derive(Debug)]
pub enum RegistryError {
    /// The namespace is already taken (duplicate registration rejected).
    NamespaceTaken { namespace: String },
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::NamespaceTaken { namespace } => {
                write!(f, "the namespace is already in use: {namespace}")
            }
        }
    }
}

impl std::error::Error for RegistryError {}

/// Runtime registry. The lock only protects short critical-section table
/// operations and is never held across an await.
pub struct Registry {
    // key = namespace
    inner: RwLock<HashMap<String, RunningComponent>>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }

    /// Register: rejected on a namespace conflict.
    pub async fn register(
        &self,
        namespace: &str,
        component: RunningComponent,
    ) -> Result<(), RegistryError> {
        let mut guard = self.inner.write().await;
        if guard.contains_key(namespace) {
            return Err(RegistryError::NamespaceTaken {
                namespace: namespace.to_string(),
            });
        }
        guard.insert(namespace.to_string(), component);
        Ok(())
    }

    pub async fn lookup(&self, namespace: &str) -> Option<RunningSnapshot> {
        let guard = self.inner.read().await;
        guard.get(namespace).map(|c| RunningSnapshot {
            manifest: Arc::clone(&c.manifest),
            endpoint: c.endpoint.clone(),
            token: Arc::clone(&c.token),
        })
    }

    pub async fn remove(&self, namespace: &str) -> Option<RunningComponent> {
        self.inner.write().await.remove(namespace)
    }

    /// Request a component's graceful stop (the management command entry):
    /// sends the stop signal to the running entry, and on receipt supervise
    /// walks SIGTERM → grace → removal. Returns false when not registered.
    pub async fn request_stop(&self, namespace: &str) -> bool {
        let guard = self.inner.read().await;
        guard
            .get(namespace)
            .is_some_and(|c| c.stop.send(()).is_ok())
    }

    pub async fn list(&self) -> Vec<RegistrySummary> {
        let guard = self.inner.read().await;
        let mut out: Vec<_> = guard
            .iter()
            .map(|(ns, c)| RegistrySummary {
                namespace: ns.clone(),
                id: c.manifest.id.clone(),
                version: c.manifest.version.clone(),
                endpoint: c.endpoint.to_string(),
            })
            .collect();
        out.sort_by(|a, b| a.namespace.cmp(&b.namespace));
        out
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------- Disk layer ----------

/// Manifest persistence: writes an `Installed` envelope to
/// registry/{id}.json (creates the directory if missing). Synchronous IO —
/// called only from installer's spawn_blocking context; calling it directly
/// in an async context is forbidden.
pub fn persist_manifest_sync(
    layout: &DataLayout,
    manifest: &ComponentManifest,
) -> std::io::Result<()> {
    persist_entry_sync(layout, &RegistryEntry::installed(manifest.clone()))
}

/// Envelope persistence (the creation flow's dev registration writes
/// through here): creates the registry directory if missing. Synchronous IO
/// — call only from a blocking thread pool.
pub fn persist_entry_sync(layout: &DataLayout, entry: &RegistryEntry) -> std::io::Result<()> {
    std::fs::create_dir_all(layout.registry_dir())?;
    let json = serde_json::to_string_pretty(entry)
        .map_err(|e| std::io::Error::other(format!("failed to serialize the entry: {e}")))?;
    std::fs::write(layout.registry_file(&entry.manifest.id), json)
}

/// Parse one registry record. Back-compat: a record without a top-level
/// "manifest" key is the pre-envelope bare manifest form and reads as
/// `Installed`.
fn parse_entry(bytes: &[u8]) -> Result<RegistryEntry, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    if value.get("manifest").is_some() {
        serde_json::from_value(value)
    } else {
        Ok(RegistryEntry {
            source: ComponentSource::Installed,
            project_dir: None,
            manifest: serde_json::from_value(value)?,
            // The bare form predates the envelope, hence predates the
            // registration timestamp as well
            registered_at: None,
        })
    }
}

/// Scan all registry records (for the supervisor's startup recovery). A
/// corrupt single file does not drag down the whole: skip and log.
/// Re-validation happens here on every read: the config path safety assumes
/// "id is constrained by validation", and the on-disk registration is the
/// only entry that bypasses the install flow, so the loop must be closed by
/// validating on read. Dev entries additionally project the project's
/// `manifest.json` (the authoritative declaration) over the envelope
/// snapshot — see [`validate_and_project`].
pub async fn load_all_entries(layout: &DataLayout) -> std::io::Result<Vec<RegistryEntry>> {
    let mut entries_out = Vec::new();
    // The registry directory not existing = no installed components yet
    // (first-run semantics); return an empty list
    let mut entries = match tokio::fs::read_dir(layout.registry_dir()).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(entries_out),
        Err(e) => return Err(e),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let loaded = tokio::fs::read(&path)
            .await
            .map_err(|e| std::io::Error::other(format!("{}: {e}", path.display())))
            .and_then(|bytes| {
                parse_entry(&bytes)
                    .map_err(|e| std::io::Error::other(format!("failed to parse: {e}")))
            })
            .map_err(|e| std::io::Error::other(format!("{}: {e}", path.display())));
        match loaded {
            Ok(record) => match validate_and_project(record).await {
                Ok(projected) => entries_out.push(projected),
                Err(e) => {
                    log::warn!("skipping a registry record that failed validation ({path:?}): {e}")
                }
            },
            Err(e) => log::warn!("skipping a corrupted registry record: {e}"),
        }
    }
    Ok(entries_out)
}

/// Single-record read (NotFound → Ok(None)); used by the static server's
/// data-source branch. A corrupt record surfaces as Err — callers decide the
/// fallback. Same projection semantics as [`load_all_entries`].
pub async fn load_entry(layout: &DataLayout, id: &str) -> std::io::Result<Option<RegistryEntry>> {
    match tokio::fs::read(layout.registry_file(id)).await {
        Ok(bytes) => {
            let record = parse_entry(&bytes)
                .map_err(|e| std::io::Error::other(format!("failed to parse: {e}")))?;
            let projected = validate_and_project(record).await.map_err(|e| {
                std::io::Error::other(format!("registry record failed validation: {e}"))
            })?;
            Ok(Some(projected))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Component-list ordering: newest registration first; legacy records
/// without a timestamp sink to the end (stable sort keeps their relative
/// order). The single list exit (`list_components`) applies this so the
/// management table and the dynamic menu inherit the same order.
pub fn sort_newest_first(entries: &mut [RegistryEntry]) {
    // Reverse<Option<u64>>: Some descends newest-first; None (legacy records
    // without a timestamp) sinks to the end, ties keep their relative order
    entries.sort_by_key(|e| std::cmp::Reverse(e.registered_at));
}

/// Per-source validation + dev projection:
/// - installed: the envelope manifest is the package manifest copy — package-level
///   validation ([`ComponentManifest::validate`]);
/// - dev: the snapshot is a declaration projection — declaration-level validation;
///   then the project's `manifest.json` (the authoritative declaration) is read over
///   it. Unreachable / unparsable / invalid / id-drifted project declarations fall
///   back to the envelope snapshot with a warning — the record survives
///   project-directory moves, entries are never dropped for projection failures.
async fn validate_and_project(record: RegistryEntry) -> Result<RegistryEntry, String> {
    match record.source {
        ComponentSource::Installed => record
            .manifest
            .validate()
            .map_err(|e| e.to_string())
            .map(|_| record),
        ComponentSource::Dev => {
            record
                .manifest
                .validate_declaration()
                .map_err(|e| format!("snapshot failed declaration validation: {e}"))?;
            let Some(project_dir) = record.project_dir.as_ref() else {
                // Envelope invariant (dev ⇒ project_dir) broken: keep the
                // snapshot entry rather than dropping it
                log::warn!(
                    "dev component {} has no project_dir; keeping the envelope snapshot",
                    record.manifest.id
                );
                return Ok(record);
            };
            match load_project_declaration(project_dir, &record.manifest.id).await {
                Ok(declaration) => Ok(RegistryEntry {
                    source: record.source,
                    project_dir: record.project_dir.clone(),
                    manifest: declaration,
                    // The projection replaces the declaration snapshot only;
                    // lifecycle identity facts pass through untouched
                    registered_at: record.registered_at,
                }),
                Err(reason) => {
                    log::warn!(
                        "dev component {}: falling back to the envelope snapshot ({reason})",
                        record.manifest.id
                    );
                    Ok(record)
                }
            }
        }
    }
}

/// Read the project's authoritative declaration (`project_dir/manifest.json`):
/// parse + declaration-level validation + registration-key consistency. The error
/// carries the concrete reason — the caller decides between falling back to the
/// snapshot (load paths) and failing the operation (start/reload paths).
async fn load_project_declaration(
    project_dir: &Path,
    registered_id: &str,
) -> Result<ComponentManifest, String> {
    let path = project_dir.join("manifest.json");
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let declaration: ComponentManifest = serde_json::from_slice(&bytes)
        .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
    declaration
        .validate_declaration()
        .map_err(|e| format!("{} failed declaration validation: {e}", path.display()))?;
    if declaration.id != registered_id {
        return Err(format!(
            "the declaration id ({}) drifted from the registered id ({registered_id})",
            declaration.id
        ));
    }
    Ok(declaration)
}

/// Write the project's declaration file (the reload path keeps its version fresh).
async fn write_project_declaration(
    project_dir: &Path,
    declaration: &ComponentManifest,
) -> Result<(), String> {
    let path = project_dir.join("manifest.json");
    tokio::fs::write(
        &path,
        declaration_json(declaration).map_err(|e| e.to_string())?,
    )
    .await
    .map_err(|e| format!("failed to write {}: {e}", path.display()))
}

/// Shared declaration serialization (pretty, POSIX trailing newline).
fn declaration_json(declaration: &ComponentManifest) -> std::io::Result<String> {
    let mut json = serde_json::to_string_pretty(declaration)
        .map_err(|e| std::io::Error::other(format!("failed to serialize the declaration: {e}")))?;
    json.push('\n');
    Ok(json)
}

/// Synchronous declaration write — the creation flow's blocking persistence
/// hop. Same bytes as the async reload path ([`write_project_declaration`]).
pub(crate) fn write_declaration_sync(
    project_dir: &Path,
    declaration: &ComponentManifest,
) -> std::io::Result<()> {
    std::fs::write(
        project_dir.join("manifest.json"),
        declaration_json(declaration)?,
    )
}

/// The component's config.toml path, same source as the spawn workdir:
/// installed → `components/{id}/config.toml`, dev → `project_dir/config.toml`.
pub fn config_path_for(layout: &DataLayout, entry: &RegistryEntry) -> PathBuf {
    workdir_for(layout, entry).join("config.toml")
}

/// Project version, same source as the packaging chain: `server/Cargo.toml`
/// [package].version for server shapes, `web/package.json` "version" for
/// frontend-only shapes.
pub(crate) fn project_version(root: &Path, has_server: bool) -> Result<String, String> {
    if has_server {
        let cargo_toml = root.join("server").join("Cargo.toml");
        let text = std::fs::read_to_string(&cargo_toml)
            .map_err(|e| format!("failed to read {}: {e}", cargo_toml.display()))?;
        let parsed: toml::Value = toml::from_str(&text)
            .map_err(|e| format!("failed to parse {}: {e}", cargo_toml.display()))?;
        parsed
            .get("package")
            .and_then(|p| p.get("version"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("{} has no [package].version", cargo_toml.display()))
    } else {
        let package_json = root.join("web").join("package.json");
        let text = std::fs::read_to_string(&package_json)
            .map_err(|e| format!("failed to read {}: {e}", package_json.display()))?;
        serde_json::from_str::<serde_json::Value>(&text)
            .map_err(|e| format!("failed to parse {}: {e}", package_json.display()))?
            .get("version")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("{} has no \"version\"", package_json.display()))
    }
}

/// Extract the COMPONENT_ID constant from a server main.rs source text
/// (no regex dependency: the constant is template-generated and the shape is
/// stable — `const COMPONENT_ID: &str = "...";`).
fn extract_component_id(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("const COMPONENT_ID") else {
            continue;
        };
        let Some(eq) = rest.find('=') else {
            continue;
        };
        let after = rest[eq + 1..].trim_start();
        if let Some(value) = after.strip_prefix('"').and_then(|s| s.split('"').next()) {
            return Some(value.to_string());
        }
    }
    None
}

/// Source-identity check (server shapes): the declaration id must match the
/// COMPONENT_ID constant in server/src/main.rs — a drifted constant makes the
/// component's /health self-report disagree with the declaration, which
/// misaligns the health probe and the gateway dispatch. A missing file or
/// constant only warns (a broken template surfaces as its own compile error).
fn check_server_component_id(project_dir: &Path, declared_id: &str) -> Result<(), String> {
    let main_rs = project_dir.join("server").join("src").join("main.rs");
    let text = match std::fs::read_to_string(&main_rs) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::warn!(
                "dev component {declared_id}: {} not found, skipping the component-id source check",
                main_rs.display()
            );
            return Ok(());
        }
        Err(e) => return Err(format!("failed to read {}: {e}", main_rs.display())),
    };
    let Some(constant_id) = extract_component_id(&text) else {
        log::warn!(
            "dev component {declared_id}: no COMPONENT_ID constant in {}, skipping the component-id source check",
            main_rs.display()
        );
        return Ok(());
    };
    if constant_id != declared_id {
        return Err(format!(
            "source identity drift: the declaration id is \"{declared_id}\" but {} declares COMPONENT_ID = \"{constant_id}\"",
            main_rs.display()
        ));
    }
    Ok(())
}

/// Start/reload semantics for a dev entry (architecture.md "Creation flow"):
/// re-read the project's authoritative declaration, refresh the version from
/// the project's version source, check source identity, write the refreshed
/// declaration back (keeps the file's version fresh), and refresh the
/// envelope snapshot. Installed entries pass through unchanged.
///
/// Every failure is a hard error on the start path (the caller reports it;
/// the supervisor's restore path logs and skips the entry) — starting must
/// run against the live project, not a stale snapshot.
pub async fn refresh_dev_entry(
    layout: &DataLayout,
    entry: &RegistryEntry,
) -> Result<RegistryEntry, String> {
    if entry.source != ComponentSource::Dev {
        return Ok(entry.clone());
    }
    let Some(project_dir) = entry.project_dir.as_ref() else {
        return Err(format!(
            "dev component {} has no project_dir on its envelope record",
            entry.manifest.id
        ));
    };
    // 1. The project declaration is the authority; the registered id is the
    //    envelope's key — a drifted id fails here (load paths merely fall
    //    back, start refuses: the declaration must match what is registered)
    let mut declaration = load_project_declaration(project_dir, &entry.manifest.id).await?;
    // 2. Version refresh from the project's version source (display value)
    let has_server = declaration.launch.is_some();
    let version = project_version(project_dir, has_server)?;
    declaration.version = version;
    // 3. Source identity (server shapes): declaration id vs COMPONENT_ID
    check_server_component_id(project_dir, &declaration.id)?;
    // 4. Write the refreshed declaration back (keeps the version fresh)
    write_project_declaration(project_dir, &declaration).await?;
    // 5. Refresh the envelope snapshot
    let refreshed = RegistryEntry {
        source: entry.source,
        project_dir: entry.project_dir.clone(),
        manifest: declaration,
        // A refresh rewrites the declaration snapshot; the registration
        // timestamp is a create-time fact and must survive the rebuild
        // (this record is persisted back right below)
        registered_at: entry.registered_at,
    };
    {
        let layout = layout.clone();
        let persisted = refreshed.clone();
        tokio::task::spawn_blocking(move || persist_entry_sync(&layout, &persisted))
            .await
            .map_err(|_| "the persistence task terminated unexpectedly".to_string())?
            .map_err(|e| format!("failed to refresh the envelope snapshot: {e}"))?;
    }
    Ok(refreshed)
}

/// The component process's working directory (launch.command's relative
/// resolution base): dev resolves against the project directory, installed
/// against the install directory.
pub fn workdir_for(layout: &DataLayout, entry: &RegistryEntry) -> PathBuf {
    match entry.source {
        ComponentSource::Dev => entry
            .project_dir
            .clone()
            .unwrap_or_else(|| layout.component_dir(&entry.manifest.id)),
        ComponentSource::Installed => layout.component_dir(&entry.manifest.id),
    }
}

/// Dev entries' launch artifact precheck: a dev backend binary lives in the
/// project's target/ and only exists after a build — a missing artifact gets
/// a directed error (with the build hint) instead of a vague spawn/health
/// failure. Installed entries and launch=None shapes pass through.
pub fn ensure_launch_artifact(entry: &RegistryEntry) -> Result<(), String> {
    if entry.source != ComponentSource::Dev {
        return Ok(());
    }
    let Some(launch) = entry.manifest.launch.as_ref() else {
        return Ok(());
    };
    let Some(project_dir) = entry.project_dir.as_ref() else {
        // Envelope invariant (dev ⇒ project_dir); tolerate and pass: spawn
        // would fall back to the install dir and fail there with its own
        // signal.
        return Ok(());
    };
    let artifact = project_dir.join(&launch.command);
    if artifact.exists() {
        return Ok(());
    }
    Err(format!(
        "the backend artifact of dev component {} is missing ({}) — build the project first (e.g. cargo build --release in {}), then start it again",
        entry.manifest.id,
        launch.command,
        project_dir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use slsdk_rs::UiType;

    fn manifest(id: &str, ns: &str) -> ComponentManifest {
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
                "launch": {{ "command": "bin", "args": [] }},
                "files": [ {{ "path": "bin", "sha256": "aa" }} ]
            }}"#
        ))
        .unwrap()
    }

    fn running(m: ComponentManifest) -> RunningComponent {
        RunningComponent {
            manifest: Arc::new(m),
            endpoint: slsdk_rs::Endpoint::Tcp {
                addr: "127.0.0.1:9000".parse().unwrap(),
            },
            token: Arc::new("tok".to_string()),
            pid: None,
            stop: watch::channel(()).0,
        }
    }

    #[tokio::test]
    async fn register_lookup_remove_roundtrip() {
        let registry = Registry::new();
        registry
            .register("todo", running(manifest("xatodo", "todo")))
            .await
            .unwrap();

        let snap = registry.lookup("todo").await.unwrap();
        assert_eq!(snap.manifest.id, "xatodo");
        assert_eq!(snap.token.as_str(), "tok");

        let list = registry.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].namespace, "todo");
        assert_eq!(list[0].id, "xatodo");
        assert_eq!(list[0].endpoint, "tcp:127.0.0.1:9000");

        let removed = registry.remove("todo").await;
        assert!(removed.is_some());
        assert!(registry.lookup("todo").await.is_none());
        assert!(registry.remove("todo").await.is_none());
    }

    #[tokio::test]
    async fn duplicate_namespace_rejected() {
        let registry = Registry::new();
        registry
            .register("todo", running(manifest("a", "todo")))
            .await
            .unwrap();
        let err = registry
            .register("todo", running(manifest("b", "todo")))
            .await
            .unwrap_err();
        assert!(matches!(err, RegistryError::NamespaceTaken { namespace } if namespace == "todo"));
    }

    #[tokio::test]
    async fn persist_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let m = manifest("xatodo", "todo");
        persist_manifest_sync(&layout, &m).unwrap();

        let loaded = load_all_entries(&layout).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].source, ComponentSource::Installed);
        assert_eq!(loaded[0].project_dir, None);
        // The install path captures the registration timestamp at envelope
        // build time (installed() is its single assembly point)
        assert!(loaded[0].registered_at.is_some());
        assert_eq!(loaded[0].manifest.id, "xatodo");
        assert_eq!(loaded[0].manifest.namespace, "todo");
        assert_eq!(
            loaded[0].manifest.ui.as_ref().unwrap().ui_type,
            UiType::Module
        );
    }

    #[tokio::test]
    async fn dev_entry_roundtrip_keeps_project_dir() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let project = dir.path().join("proj");
        let entry = RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(project.clone()),
            manifest: manifest("myapp", "myapp"),
            registered_at: None,
        };
        persist_entry_sync(&layout, &entry).unwrap();

        let loaded = load_all_entries(&layout).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].source, ComponentSource::Dev);
        assert_eq!(loaded[0].project_dir.as_deref(), Some(project.as_path()));

        let single = load_entry(&layout, "myapp").await.unwrap().unwrap();
        assert_eq!(single.source, ComponentSource::Dev);
        assert!(load_entry(&layout, "missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn bare_manifest_record_reads_as_installed() {
        // Pre-envelope form: registry/{id}.json stored a bare manifest (no
        // "manifest" key) — reads back as an Installed entry with no
        // project_dir.
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        std::fs::create_dir_all(layout.registry_dir()).unwrap();
        std::fs::write(
            layout.registry_file("legacy"),
            serde_json::to_string(&manifest("legacy", "legacy")).unwrap(),
        )
        .unwrap();

        let loaded = load_all_entries(&layout).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].source, ComponentSource::Installed);
        assert_eq!(loaded[0].project_dir, None);
        // The bare form has no timestamp: legacy records sort last
        assert_eq!(loaded[0].registered_at, None);
        assert_eq!(loaded[0].manifest.id, "legacy");
    }

    #[tokio::test]
    async fn envelope_without_timestamp_reads_as_none() {
        // Envelope records written before registered_at existed carry no
        // such key in JSON: serde default reads it as None (a different
        // code path from the bare-manifest form above, which hand-builds
        // the entry)
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        std::fs::create_dir_all(layout.registry_dir()).unwrap();
        let m = manifest("oldapp", "oldapp");
        let json = format!(
            r#"{{ "source": "installed", "project_dir": null, "manifest": {} }}"#,
            serde_json::to_string(&m).unwrap()
        );
        assert!(!json.contains("registered_at"));
        std::fs::write(layout.registry_file("oldapp"), json).unwrap();

        let loaded = load_all_entries(&layout).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].registered_at, None);
    }

    #[test]
    fn sort_newest_first_puts_newest_first_and_legacy_last() {
        let entry = |id: &str, ts: Option<u64>| RegistryEntry {
            source: ComponentSource::Installed,
            project_dir: None,
            manifest: manifest(id, id),
            registered_at: ts,
        };
        let mut entries = vec![
            entry("old", Some(100)),
            entry("legacy-a", None),
            entry("newest", Some(300)),
            entry("legacy-b", None),
            entry("mid", Some(200)),
        ];
        sort_newest_first(&mut entries);
        let ids: Vec<&str> = entries.iter().map(|e| e.manifest.id.as_str()).collect();
        // Timestamped records descend; legacy records sink to the end and
        // keep their relative order (stable sort)
        assert_eq!(ids, ["newest", "mid", "old", "legacy-a", "legacy-b"]);
    }

    #[tokio::test]
    async fn workdir_resolves_by_source() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let project = dir.path().join("proj");
        let dev = RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(project.clone()),
            manifest: manifest("devapp", "devapp"),
            registered_at: None,
        };
        let installed = RegistryEntry::installed(manifest("app", "app"));
        assert_eq!(workdir_for(&layout, &dev), project);
        assert_eq!(
            workdir_for(&layout, &installed),
            layout.component_dir("app")
        );
    }

    #[tokio::test]
    async fn config_path_resolves_by_source() {
        // config.toml shares the spawn-workdir source: install dir for
        // installed, project dir for dev
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let project = dir.path().join("proj");
        let dev = RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(project.clone()),
            manifest: manifest("devapp", "devapp"),
            registered_at: None,
        };
        let installed = RegistryEntry::installed(manifest("app", "app"));
        assert_eq!(config_path_for(&layout, &dev), project.join("config.toml"));
        assert_eq!(
            config_path_for(&layout, &installed),
            layout.component_dir("app").join("config.toml")
        );
    }

    // ---------- Envelope projection (dev entries read the project declaration as the authority, falling back to the snapshot) ----------

    /// Minimal pure-declaration form of the project manifest.json (empty
    /// files, editable fields included).
    fn declaration_text(id: &str, ns: &str, menu_title: &str) -> String {
        format!(
            r#"{{
                "id": "{id}",
                "version": "0.1.0",
                "min_shell_version": "0.1.0",
                "namespace": "{ns}",
                "display_name": "{id}",
                "ui": {{ "ui_type": "module", "entry": "entry.js" }},
                "menu": {{ "title": "{menu_title}" }},
                "routes": [],
                "launch": null,
                "files": []
            }}"#
        )
    }

    fn write_declaration(project: &Path, id: &str, ns: &str, menu_title: &str) {
        std::fs::write(
            project.join("manifest.json"),
            declaration_text(id, ns, menu_title),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn dev_entry_projects_the_project_declaration() {
        // The author edited the project declaration menu.title: the load
        // projection takes the declaration value; the snapshot is the fallback
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        write_declaration(&project, "devapp", "devns", "Edited Title");
        let entry = RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(project),
            manifest: manifest("devapp", "devns"),
            registered_at: Some(42),
        };
        persist_entry_sync(&layout, &entry).unwrap();

        let loaded = load_all_entries(&layout).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].manifest.menu.as_ref().unwrap().title,
            "Edited Title"
        );
        // Pure-declaration projection: files empty
        assert!(loaded[0].manifest.files.is_empty());
        // The projection never rewrites the registration timestamp
        assert_eq!(loaded[0].registered_at, Some(42));
    }

    #[tokio::test]
    async fn dev_entry_falls_back_to_snapshot_when_project_unreachable() {
        // The project directory moved: fall back to the envelope snapshot;
        // the entry survives (snapshot values win)
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let entry = RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(dir.path().join("gone")),
            manifest: manifest("devapp", "devns"),
            registered_at: None,
        };
        persist_entry_sync(&layout, &entry).unwrap();

        let loaded = load_all_entries(&layout).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].manifest.id, "devapp");
        // Snapshot values (the helper manifest menu=null), not declaration values
        assert!(loaded[0].manifest.menu.is_none());
        assert_eq!(
            loaded[0].project_dir.as_deref(),
            Some(dir.path().join("gone").as_path())
        );
    }

    #[tokio::test]
    async fn dev_entry_falls_back_when_declaration_id_drifts() {
        // Declaration id drift (differs from the registered key): key
        // consistency wins, fall back to the snapshot
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        write_declaration(&project, "other-id", "devns", "t");
        let entry = RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(project),
            manifest: manifest("devapp", "devns"),
            registered_at: None,
        };
        persist_entry_sync(&layout, &entry).unwrap();

        let loaded = load_all_entries(&layout).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].manifest.id, "devapp", "snapshot id kept");
    }

    #[tokio::test]
    async fn dev_entry_falls_back_when_declaration_invalid() {
        // Unparsable declaration (bad JSON): fall back to the snapshot; the
        // entry survives
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("manifest.json"), "{ not json").unwrap();
        let entry = RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(project),
            manifest: manifest("devapp", "devns"),
            registered_at: None,
        };
        persist_entry_sync(&layout, &entry).unwrap();

        let loaded = load_all_entries(&layout).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].manifest.id, "devapp");
    }

    // ---------- start/reload semantics (refresh_dev_entry) ----------

    /// Version source + COMPONENT_ID constant of a server-shape project.
    fn write_server_sources(project: &Path, version: &str, component_id: &str) {
        std::fs::create_dir_all(project.join("server/src")).unwrap();
        std::fs::write(
            project.join("server/Cargo.toml"),
            format!(
                "[package]\nname = \"x-server\"\nversion = \"{version}\"\nedition = \"2021\"\n"
            ),
        )
        .unwrap();
        std::fs::write(
            project.join("server/src/main.rs"),
            format!("const COMPONENT_ID: &str = \"{component_id}\";\nfn main() {{}}\n"),
        )
        .unwrap();
    }

    fn server_declaration_text(id: &str, ns: &str, version: &str) -> String {
        format!(
            r#"{{
                "id": "{id}",
                "version": "{version}",
                "min_shell_version": "0.1.0",
                "namespace": "{ns}",
                "display_name": "{id}",
                "ui": null,
                "menu": null,
                "routes": [],
                "launch": {{ "command": "./target/release/{id}-server", "args": [] }},
                "files": []
            }}"#
        )
    }

    #[tokio::test]
    async fn refresh_dev_entry_refreshes_version_snapshot_and_file() {
        // Reload semantics: version source 0.2.0 → both the declaration file
        // and the envelope snapshot get the refreshed version
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let project = dir.path().join("proj");
        write_server_sources(&project, "0.2.0", "devapp");
        std::fs::write(
            project.join("manifest.json"),
            server_declaration_text("devapp", "devns", "0.1.0"),
        )
        .unwrap();
        let entry = RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(project),
            manifest: manifest("devapp", "devns"),
            registered_at: None,
        };
        persist_entry_sync(&layout, &entry).unwrap();

        let refreshed = refresh_dev_entry(&layout, &entry).await.unwrap();
        assert_eq!(refreshed.manifest.version, "0.2.0");
        // Declaration file written back (the author-editable file stays fresh)
        let rewritten: ComponentManifest = serde_json::from_str(
            &std::fs::read_to_string(
                refreshed
                    .project_dir
                    .as_ref()
                    .unwrap()
                    .join("manifest.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(rewritten.version, "0.2.0");
        // Envelope snapshot refreshed: read the raw on-disk record (load
        // projects, which cannot prove the snapshot itself)
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(layout.registry_file("devapp")).unwrap())
                .unwrap();
        assert_eq!(raw["manifest"]["version"], "0.2.0");
    }

    #[tokio::test]
    async fn refresh_dev_entry_preserves_registered_at() {
        // The registration timestamp is captured once at creation; a refresh
        // rewrites the envelope snapshot wholesale, so the rebuilt record
        // must carry the original value — in memory and on disk
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let project = dir.path().join("proj");
        write_server_sources(&project, "0.2.0", "devapp");
        std::fs::write(
            project.join("manifest.json"),
            server_declaration_text("devapp", "devns", "0.1.0"),
        )
        .unwrap();
        let entry = RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(project),
            manifest: manifest("devapp", "devns"),
            registered_at: Some(1234567890),
        };
        persist_entry_sync(&layout, &entry).unwrap();

        let refreshed = refresh_dev_entry(&layout, &entry).await.unwrap();
        assert_eq!(refreshed.registered_at, Some(1234567890));
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(layout.registry_file("devapp")).unwrap())
                .unwrap();
        assert_eq!(raw["registered_at"], 1234567890);
    }

    #[tokio::test]
    async fn refresh_dev_entry_rejects_source_identity_drift() {
        // main.rs COMPONENT_ID differs from the declaration id: start refuses
        // clearly
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let project = dir.path().join("proj");
        write_server_sources(&project, "0.1.0", "other-id");
        std::fs::write(
            project.join("manifest.json"),
            server_declaration_text("devapp", "devns", "0.1.0"),
        )
        .unwrap();
        let entry = RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(project),
            manifest: manifest("devapp", "devns"),
            registered_at: None,
        };
        persist_entry_sync(&layout, &entry).unwrap();

        let err = refresh_dev_entry(&layout, &entry).await.unwrap_err();
        assert!(err.contains("source identity drift"), "{err}");
    }

    #[tokio::test]
    async fn refresh_dev_entry_fails_when_project_unreachable() {
        // Start must run against the live project: unreachable → hard error
        // (never a silent fallback)
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let entry = RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(dir.path().join("gone")),
            manifest: manifest("devapp", "devns"),
            registered_at: None,
        };
        persist_entry_sync(&layout, &entry).unwrap();

        assert!(refresh_dev_entry(&layout, &entry).await.is_err());
    }

    #[tokio::test]
    async fn refresh_dev_entry_installed_passes_through() {
        // Installed entries take no part in declaration refresh: pass through
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let entry = RegistryEntry::installed(manifest("app", "app"));
        let refreshed = refresh_dev_entry(&layout, &entry).await.unwrap();
        assert_eq!(refreshed.source, entry.source);
        assert_eq!(refreshed.project_dir, entry.project_dir);
        assert_eq!(refreshed.manifest, entry.manifest);
    }

    #[tokio::test]
    async fn launch_artifact_precheck() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join("target/release")).unwrap();

        let mut dev = RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(project.clone()),
            manifest: manifest("devapp", "devapp"),
            registered_at: None,
        };
        // launch.command is relative to project_dir: a missing artifact → an instructive error
        let err = ensure_launch_artifact(&dev).unwrap_err();
        assert!(err.contains("build the project first"), "{err}");
        // Artifact in place → passes (the helper manifest's launch.command = "bin")
        std::fs::write(project.join("bin"), b"bin").unwrap();
        assert!(ensure_launch_artifact(&dev).is_ok());
        // launch=None (frontend-only) → nothing to precheck, passes directly
        dev.manifest.launch = None;
        assert!(ensure_launch_artifact(&dev).is_ok());
        // Installed entries skip the artifact precheck (the packaging chain guarantees it) — the failure
        // signal for a pre-existing install directory missing artifacts comes from spawn itself
        let installed = RegistryEntry::installed(manifest("app", "app"));
        assert!(ensure_launch_artifact(&installed).is_ok());
        let _ = layout; // layout only exists for construction (keeps this case isomorphic with the others)
    }

    #[tokio::test]
    async fn load_skips_corrupted_files() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        std::fs::create_dir_all(layout.registry_dir()).unwrap();
        std::fs::write(
            layout.registry_file("good"),
            serde_json::to_string(&manifest("good", "good")).unwrap(),
        )
        .unwrap();
        std::fs::write(layout.registry_file("bad"), "{ not json").unwrap();
        std::fs::write(layout.registry_dir().join("notes.txt"), "ignore non-json").unwrap();

        let loaded = load_all_entries(&layout).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].manifest.id, "good");
    }

    #[tokio::test]
    async fn load_skips_records_failing_validation() {
        // Valid JSON but semantically invalid (namespace contains a path
        // separator) → intercepted by re-validate,
        // without dragging down the good records in the same directory (the
        // on-disk registration is the only entry bypassing the install flow)
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        std::fs::create_dir_all(layout.registry_dir()).unwrap();
        std::fs::write(
            layout.registry_file("good"),
            serde_json::to_string(&manifest("good", "good")).unwrap(),
        )
        .unwrap();
        let mut invalid = manifest("rogue", "rogue");
        invalid.namespace = "bad/ns".to_string();
        std::fs::write(
            layout.registry_file("rogue"),
            serde_json::to_string(&invalid).unwrap(),
        )
        .unwrap();

        let loaded = load_all_entries(&layout).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].manifest.id, "good");
    }

    #[tokio::test]
    async fn load_missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let loaded = load_all_entries(&layout).await.unwrap();
        assert!(loaded.is_empty());
    }
}
