//! Registry: runtime registration (namespace → component runtime info) +
//! disk persistence (manifest records).
//!
//! The namespace is the gateway dispatch key (architecture.md: the first URL
//! segment is the namespace).
//! The disk layer stores only the manifest (registry/{id}.json); runtime
//! info (endpoint/token/stop handle) lives with the shell's lifecycle, and
//! on shell restart the supervisor scans the disk records and starts
//! components again.

use crate::config::DataLayout;
use serde::Serialize;
use slsdk_rs::{ComponentManifest, Endpoint};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{watch, RwLock};

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

/// Manifest persistence: writes registry/{id}.json (creates the directory if
/// missing).
/// Synchronous IO — called only from installer's spawn_blocking context;
/// calling it directly in an async context is forbidden.
pub fn persist_manifest_sync(
    layout: &DataLayout,
    manifest: &ComponentManifest,
) -> std::io::Result<()> {
    std::fs::create_dir_all(layout.registry_dir())?;
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|e| std::io::Error::other(format!("failed to serialize the manifest: {e}")))?;
    std::fs::write(layout.registry_file(&manifest.id), json)
}

/// Scan all manifests in registry/ (for the supervisor's startup recovery).
/// A corrupt single file does not drag down the whole: skip and log.
pub async fn load_all_manifests(layout: &DataLayout) -> std::io::Result<Vec<ComponentManifest>> {
    let mut manifests = Vec::new();
    // The registry directory not existing = no installed components yet
    // (first-run semantics); return an empty list
    let mut entries = match tokio::fs::read_dir(layout.registry_dir()).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(manifests),
        Err(e) => return Err(e),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let loaded = tokio::fs::read(&path)
            .await
            .and_then(|bytes| {
                serde_json::from_slice::<ComponentManifest>(&bytes)
                    .map_err(|e| std::io::Error::other(format!("failed to parse: {e}")))
            })
            .map_err(|e| std::io::Error::other(format!("{}: {e}", path.display())));
        match loaded {
            Ok(manifest) => match manifest.validate() {
                // re-validate: the config path safety assumes "id is
                // constrained by validate", and the on-disk registration is
                // the only entry that bypasses the install flow, so the loop
                // must be closed by validating on read
                Ok(()) => manifests.push(manifest),
                Err(e) => {
                    log::warn!("skipping a registry record that failed validation ({path:?}): {e}")
                }
            },
            Err(e) => log::warn!("skipping a corrupted registry record: {e}"),
        }
    }
    Ok(manifests)
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

        let loaded = load_all_manifests(&layout).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "xatodo");
        assert_eq!(loaded[0].namespace, "todo");
        assert_eq!(loaded[0].ui.ui_type, UiType::Module);
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

        let loaded = load_all_manifests(&layout).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "good");
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

        let loaded = load_all_manifests(&layout).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "good");
    }

    #[tokio::test]
    async fn load_missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let loaded = load_all_manifests(&layout).await.unwrap();
        assert!(loaded.is_empty());
    }
}
