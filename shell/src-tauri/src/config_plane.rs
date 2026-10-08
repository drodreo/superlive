//! Component configuration plane, shell-side implementation (architecture.md
//! "Configuration plane" section, finalized 2026-09-22).
//!
//! Responsibility split (boundary with the slsdk-rs `config` module):
//! - SDK side (inside the component process): the standard endpoint
//!   `GET/PUT /config` — validation, write-back, hot-reload callbacks.
//!   The shell side reuses [`slsdk_rs::ConfigStore`] so validation and
//!   write-back semantics match the component endpoint exactly (the channel
//!   the shell uses to write files directly while a component is not
//!   running); both ends stay zero-drift and the shell needs no cron
//!   dependency.
//! - This module (shell side): component state machine + launch policy
//!   persistence + upgrade required-field comparison + in-flight
//!   configuration forwarding (component standard endpoint).
//!
//! # State machine (architecture.md "Component state machine")
//!
//! ```text
//! installed → needs-config (required fields missing) / ready (zero config or required fields complete)
//!   → (post-config validation passes) → launch options prompt → starting → running → stopped
//! After an upgrade, any state can fall back to needs-config.
//! ```
//!
//! Persistence and transition loop:
//! - Install complete (install_component): determine state against the new
//!   manifest (rule 3 of the three lifecycle rules); launch_policy keeps its
//!   existing value (first install takes [`default_launch_policy`]).
//! - Start request (start_component): needs-config rejected → starting →
//!   (spawn succeeds) running / (failure) write back the determined state.
//! - Process exit (graceful stop / crash / old process exiting after an
//!   upgrade replacement, supervise cleanup): recompute the state from the
//!   current manifest+config and persist it — config complete → stopped;
//!   config missing → needs-config (an old process exiting after an upgrade
//!   is not mislabeled as stopped; the gate semantics hold).
//! - Config write succeeds (component not running): recompute the state
//!   (needs-config → ready).
//! - Shell startup (supervisor): recompute per component + start those with
//!   launch_policy=auto that are not needs-config.
//!
//! Meta is stored at `registry/{id}.meta` (JSON): deliberately not using a
//! .json suffix — the manifest scan of the registry directory filters by
//! extension ([`crate::registry::load_all_manifests`]) and meta must not be
//! mixed into the manifest listing.

use crate::config::DataLayout;
use crate::registry::Registry;
use serde::{Deserialize, Serialize};
use slsdk_rs::{ComponentManifest, ConfigField, Endpoint};
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

/// Component state (the five states of architecture.md "Component state
/// machine"). The serde form is the state string (kebab-case) that
/// get_component_state returns to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentState {
    /// Required config missing: the start gate blocks it (lifecycle rules
    /// 2/3 of three).
    NeedsConfig,
    /// Config complete, not running.
    Ready,
    /// Starting (transient: spawn → health probe window).
    Starting,
    /// Running.
    Running,
    /// Stopped (config complete, process has exited).
    Stopped,
}

impl ComponentState {
    /// State string (the return form of get_component_state, consistent with
    /// serde serialization).
    pub fn as_str(self) -> &'static str {
        match self {
            ComponentState::NeedsConfig => "needs-config",
            ComponentState::Ready => "ready",
            ComponentState::Starting => "starting",
            ComponentState::Running => "running",
            ComponentState::Stopped => "stopped",
        }
    }
}

/// Launch policy (first item of architecture.md "Launch options slot"):
/// component-level persistence lives in the shell-side registry (a user
/// runtime choice, shell management data — it does not go into the
/// component manifest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LaunchPolicy {
    /// Started automatically at shell startup (needs-config still hits the gate).
    Auto,
    /// Only started explicitly by the user.
    Manual,
}

/// Default policy = Auto: keeps continuity with pre-configuration-plane shell
/// behavior (restore-and-start everything at program startup), so the
/// platform expectation "zero-config components work right after install" is
/// not silently changed; the user can explicitly switch to Manual in the
/// launch options prompt after configuration completes.
pub fn default_launch_policy() -> LaunchPolicy {
    LaunchPolicy::Auto
}

/// Shell-side component management metadata (persisted at `registry/{id}.meta`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentMeta {
    pub state: ComponentState,
    pub launch_policy: LaunchPolicy,
}

/// Read a component's meta. Returns None if the file does not exist
/// (pre-existing components / before first install); a corrupt file is
/// treated as no meta (fall back to deriving from the scene), logging a warn
/// without dragging down the main flow.
pub async fn load_meta(layout: &DataLayout, id: &str) -> Option<ComponentMeta> {
    let bytes = match tokio::fs::read(layout.meta_file(id)).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!(
                "failed to read the component meta ({}): {e}",
                layout.meta_file(id).display()
            );
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(meta) => Some(meta),
        Err(e) => {
            log::warn!("failed to parse the component meta (treating it as absent, falling back to on-the-fly derivation): {e}");
            None
        }
    }
}

/// Write a component's meta (the registry/ directory is already ensured to
/// exist at shell startup; create_dir_all here is defense in depth).
pub async fn save_meta(layout: &DataLayout, id: &str, meta: &ComponentMeta) -> io::Result<()> {
    let dir = layout.registry_dir();
    tokio::fs::create_dir_all(&dir).await?;
    let json = serde_json::to_string_pretty(meta)
        .map_err(|e| io::Error::other(format!("failed to serialize meta: {e}")))?;
    tokio::fs::write(layout.meta_file(id), json).await
}

/// Settle the determined state after install/config write: recompute the
/// state from manifest + the existing config.toml; launch_policy keeps its
/// existing value (falls back to the default when there is no meta).
///
/// On compute failure (config.toml corrupt/unreadable), conservatively land
/// on needs-config (the gate blocks startup and the user can resolve it by
/// reconfiguring); only save_meta IO failures are propagated.
pub async fn reconcile_state(
    layout: &DataLayout,
    manifest: &ComponentManifest,
) -> io::Result<ComponentMeta> {
    let config_path = layout.component_config_file(&manifest.id);
    let state = match compute_state(manifest, &config_path).await {
        Ok(state) => state,
        Err(e) => {
            log::warn!(
                "failed to determine the config state for component {} ({e}); conservatively marking needs-config",
                manifest.id
            );
            ComponentState::NeedsConfig
        }
    };
    let launch_policy = load_meta(layout, &manifest.id)
        .await
        .map(|m| m.launch_policy)
        .unwrap_or_else(default_launch_policy);
    let meta = ComponentMeta {
        state,
        launch_policy,
    };
    save_meta(layout, &manifest.id, &meta).await?;
    Ok(meta)
}

/// Read the component's config.toml into a toml table. NotFound is consumed
/// as first-install semantics (empty table, same convention as the SDK
/// read_table); a non-table top level is treated as a corrupt file.
async fn read_config_table(config_path: &Path) -> Result<toml::Value, String> {
    let text = match tokio::fs::read_to_string(config_path).await {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(toml::Value::Table(toml::map::Map::new()))
        }
        Err(e) => return Err(format!("failed to read {}: {e}", config_path.display())),
    };
    let parsed: toml::Value = toml::from_str(&text)
        .map_err(|e| format!("failed to parse {}: {e}", config_path.display()))?;
    if !parsed.is_table() {
        return Err(format!(
            "{} top level is not a table",
            config_path.display()
        ));
    }
    Ok(parsed)
}

/// Drill down a dotted path to fetch a value (None if any segment is missing
/// or is not a table).
///
/// Mirrored from the private `lookup_path` in slsdk-rs config.rs (this task's
/// boundary is limited to shell/, so the SDK export surface is not modified):
/// semantics are locked by the dotted-path contract of `ConfigField::key`,
/// so drift risk is minimal.
fn lookup_config_path<'a>(table: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut cur = table;
    for seg in key.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

/// Upgrade required-field comparison (rule 3): the keys of manifest-declared
/// required entries that are missing from the existing config.toml (or whose
/// string value is empty). Empty list = config complete.
///
/// The convention matches the required check in the SDK endpoint validation:
/// only **existing file values** are consulted; a declared default does not
/// count (with a required+default combination, the component's PUT validation
/// likewise requires the value to be persisted in the file — the shell gate
/// must use the same convention, otherwise you get the split of "UI shows a
/// default value while startup/write reports required fields missing").
pub fn missing_required(manifest: &ComponentManifest, table: &toml::Value) -> Vec<String> {
    let Some(config) = &manifest.config else {
        return Vec::new();
    };
    config
        .fields
        .iter()
        .filter(|f| f.required)
        .filter(|f| match lookup_config_path(table, &f.key) {
            None => true,
            // Same convention as SDK validation: an empty string counts as missing
            Some(v) => v.as_str() == Some(""),
        })
        .map(|f| f.key.clone())
        .collect()
}

/// State determination: all required present → ready, otherwise needs-config.
pub async fn compute_state(
    manifest: &ComponentManifest,
    config_path: &Path,
) -> Result<ComponentState, String> {
    let table = read_config_table(config_path).await?;
    if missing_required(manifest, &table).is_empty() {
        Ok(ComponentState::Ready)
    } else {
        Ok(ComponentState::NeedsConfig)
    }
}

/// List of missing required keys (start_component's rejection message needs
/// the concrete keys).
pub async fn required_missing_keys(
    manifest: &ComponentManifest,
    config_path: &Path,
) -> Result<Vec<String>, String> {
    let table = read_config_table(config_path).await?;
    Ok(missing_required(manifest, &table))
}

/// UI state composition: the live running table takes precedence (running
/// overrides everything); computed=needs-config is an on-the-ground config
/// fact (config.toml can be changed from outside the shell) and overrides
/// every non-running state — keeping the UI display consistent with the
/// start gate so stale meta cannot mask config drift; meta's starting is an
/// orphan state left by a shell interruption (shell exited while starting)
/// and falls back to scene-derived state; otherwise meta wins, and with no
/// meta the scene-derived state is used.
pub fn effective_state(
    meta: Option<ComponentMeta>,
    running: bool,
    computed: ComponentState,
) -> ComponentState {
    if running {
        return ComponentState::Running;
    }
    if computed == ComponentState::NeedsConfig {
        return ComponentState::NeedsConfig;
    }
    match meta.map(|m| m.state) {
        Some(ComponentState::Starting) => computed,
        Some(state) => state,
        None => computed,
    }
}

/// GET semantics (manifest+file merge, isomorphic to the SDK endpoint GET):
/// returns a flat key→value map (existing file value first → declared
/// default → omitted when both are missing). Reuses the SDK
/// [`slsdk_rs::ConfigStore::merged`] for zero convention drift.
pub async fn merged_config(
    manifest: &ComponentManifest,
    config_path: &Path,
) -> Result<serde_json::Map<String, serde_json::Value>, slsdk_rs::ConfigError> {
    let store = config_store_for(manifest, config_path);
    let merged = store.merged().await?;
    let values = merged
        .get("values")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(values
        .as_object()
        .cloned()
        .unwrap_or_else(serde_json::Map::new))
}

/// Write while not running: shell-side validation + dynamic toml write-back
/// (preserving unrecognized sections), exactly the same convention as the
/// component's standard endpoint PUT — reusing the SDK
/// [`slsdk_rs::ConfigStore::apply`] directly (validation rules / cron
/// parsing / section-preservation logic kept single-source). When there is
/// no config section the declaration list is empty and every key is rejected
/// as undeclared.
pub async fn write_config_unstarted(
    manifest: &ComponentManifest,
    config_path: &Path,
    updates: serde_json::Value,
) -> Result<(), slsdk_rs::ConfigError> {
    let store = config_store_for(manifest, config_path);
    store.apply(updates).await.map(|_| ())
}

/// Build the SDK config store from the manifest declaration (no hot-reload
/// callback on the shell side: the process is not running).
fn config_store_for(manifest: &ComponentManifest, config_path: &Path) -> slsdk_rs::ConfigStore {
    let fields = manifest
        .config
        .as_ref()
        .map(|c| c.fields.clone())
        .unwrap_or_default();
    slsdk_rs::ConfigStore::new(fields, config_path.to_path_buf(), None)
}

/// restart_required change detection: any restart_required field whose old
/// and new values differ (including one-sided presence, e.g. a newly written
/// key that did not exist before) counts as a change.
pub fn restart_required_changed(
    fields: &[ConfigField],
    old_values: &serde_json::Map<String, serde_json::Value>,
    new_values: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    fields
        .iter()
        .filter(|f| f.restart_required)
        .any(|f| old_values.get(&f.key) != new_values.get(&f.key))
}

/// In-flight configuration forwarding: forwards the PUT payload the shell
/// received verbatim to the component's standard endpoint (`PUT /config`,
/// with the per-component token — component validation + write-back +
/// hot-reload of runtime entries).
///
/// Connects to the component directly (reusing gateway's connect_component)
/// rather than looping back through the shell's HTTP: the same connection
/// primitive as the gateway forwarding layer, so from the component's
/// perspective the request it receives matches the form of UI traffic going
/// through the shell gateway (`/config` after the `/api/{ns}` prefix is
/// stripped), saving an in-shell loopback hop.
pub(crate) async fn forward_config_put(
    endpoint: &Endpoint,
    token: &str,
    updates: serde_json::Value,
) -> Result<(axum::http::StatusCode, Vec<u8>), String> {
    let stream = crate::gateway::connect_component(endpoint)
        .await
        .map_err(|e| format!("failed to connect to the component: {e}"))?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(stream)
        .await
        .map_err(|e| format!("component connection handshake failed: {e}"))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = hyper::Request::builder()
        .method(axum::http::Method::PUT)
        .uri(format!(
            "http://{}/config",
            crate::gateway::UPSTREAM_AUTHORITY
        ))
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(updates.to_string()))
        .map_err(|e| format!("failed to build the config request: {e}"))?;
    let res = sender
        .send_request(req)
        .await
        .map_err(|e| format!("failed to send the config request: {e}"))?;
    let status = res.status();
    let bytes = http_body_util::BodyExt::collect(res.into_body())
        .await
        .map_err(|e| format!("failed to read the config response: {e}"))?
        .to_bytes()
        .to_vec();
    Ok((status, bytes))
}

/// Graceful-stop wait budget (same convention as uninstall): supervise
/// force-kills after the STOP_GRACE grace period, plus extra buffer to cover
/// the "deregister + socket cleanup + meta persistence" tail.
pub(crate) fn stop_wait_budget() -> Duration {
    crate::config::STOP_GRACE + Duration::from_secs(2)
}

/// Request a stop and wait for the running-table removal to complete (the
/// first half of the restart orchestration; the meta state is recomputed and
/// persisted by supervise cleanup from manifest+config — not written here).
pub(crate) async fn stop_and_wait_removed(
    registry: &Registry,
    namespace: &str,
    id: &str,
) -> Result<(), String> {
    registry.request_stop(namespace).await;
    let deadline = Instant::now() + stop_wait_budget();
    while registry.lookup(namespace).await.is_some() {
        if Instant::now() >= deadline {
            return Err(format!(
                "component {id} stop timed out; the operation was aborted (no data touched, safe to retry)"
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build a manifest with a config section (JSON literal construction goes
    /// through the full serde contract).
    fn manifest_with_config(id: &str, ns: &str, config_json: &str) -> ComponentManifest {
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
                "launch": {{ "command": "/bin/true", "args": [] }},
                "files": [ {{ "path": "bin", "sha256": "aa" }} ],
                "config": {config_json}
            }}"#
        ))
        .unwrap()
    }

    /// Manifest without a config section (zero-config component).
    fn manifest_plain(id: &str, ns: &str) -> ComponentManifest {
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
                "launch": {{ "command": "/bin/true", "args": [] }},
                "files": [ {{ "path": "bin", "sha256": "aa" }} ]
            }}"#
        ))
        .unwrap()
    }

    /// Declarations: token (required + restart_required) / cron (format) /
    /// mode (select).
    const CONFIG_JSON: &str = r#"{
        "fields": [
            { "key": "tushare.token", "label": "token", "type": "string", "required": true, "restart_required": true },
            { "key": "job.cron", "label": "cron", "type": "string", "format": "cron" },
            { "key": "mode", "label": "mode", "type": "select", "options": ["fast", "slow"] }
        ],
        "i18n": { "zh": "config/zh.json", "en": "config/en.json" }
    }"#;

    // ---------- Upgrade required-field comparison / state determination ----------

    #[tokio::test]
    async fn no_config_section_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let m = manifest_plain("plain", "plainns");
        assert_eq!(
            compute_state(&m, &path).await.unwrap(),
            ComponentState::Ready
        );
    }

    #[tokio::test]
    async fn missing_required_file_is_needs_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let m = manifest_with_config("app", "appns", CONFIG_JSON);
        // File does not exist (first-install semantics = empty table)
        assert_eq!(
            compute_state(&m, &path).await.unwrap(),
            ComponentState::NeedsConfig
        );
        // File exists but required entries missing
        std::fs::write(&path, "[job]\ncron = \"* * * * *\"\n").unwrap();
        assert_eq!(
            compute_state(&m, &path).await.unwrap(),
            ComponentState::NeedsConfig
        );
        let missing = required_missing_keys(&m, &path).await.unwrap();
        assert_eq!(missing, vec!["tushare.token"]);
    }

    #[tokio::test]
    async fn required_with_default_still_needs_file_value() {
        // required+default combination: the default does not stand in for the
        // file value (same convention as SDK endpoint validation, preventing
        // the split of "UI shows the default while write/start reports
        // required fields missing")
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = r#"{
            "fields": [
                { "key": "a.b", "label": "ab", "type": "string", "required": true, "default": "x" }
            ]
        }"#;
        let m = manifest_with_config("dft", "dftns", config);
        assert_eq!(
            compute_state(&m, &path).await.unwrap(),
            ComponentState::NeedsConfig
        );
    }

    #[tokio::test]
    async fn empty_string_required_value_counts_as_missing() {
        // Same convention as SDK validation: an empty string counts as missing
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[tushare]\ntoken = \"\"\n").unwrap();
        let m = manifest_with_config("app", "appns", CONFIG_JSON);
        assert_eq!(
            compute_state(&m, &path).await.unwrap(),
            ComponentState::NeedsConfig
        );
    }

    #[tokio::test]
    async fn satisfied_required_is_ready() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[tushare]\ntoken = \"abc\"\n[job]\ncron = \"\"\n").unwrap();
        let m = manifest_with_config("app", "appns", CONFIG_JSON);
        assert_eq!(
            compute_state(&m, &path).await.unwrap(),
            ComponentState::Ready
        );
    }

    #[tokio::test]
    async fn corrupted_config_is_err_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not = [valid").unwrap();
        assert!(compute_state(&manifest_plain("x", "xns"), &path)
            .await
            .is_err());
    }

    // ---------- Unstarted-state write (empirical check of the reused SDK ConfigStore conventions) ----------

    #[tokio::test]
    async fn write_rejects_undeclared_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let m = manifest_with_config("app", "appns", CONFIG_JSON);
        let err = write_config_unstarted(&m, &path, json!({ "unknown": 1 }))
            .await
            .unwrap_err();
        match err {
            slsdk_rs::ConfigError::Validation(errors) => {
                assert!(errors.contains_key("unknown"));
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_rejects_bad_cron_same_as_sdk() {
        // cron convention reuses the SDK (cron 0.12): invalid expressions are
        // rejected (the shell pulls in no cron dependency)
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let m = manifest_with_config("app", "appns", CONFIG_JSON);
        let err = write_config_unstarted(
            &m,
            &path,
            json!({ "job.cron": "not-a-cron", "tushare.token": "t" }),
        )
        .await
        .unwrap_err();
        match err {
            slsdk_rs::ConfigError::Validation(errors) => {
                let msg = errors.get("job.cron").unwrap().as_str().unwrap();
                assert!(
                    msg.contains("cron"),
                    "error should point at the cron format: {msg}"
                );
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_persists_values_and_keeps_unknown_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[other]\nkeep_me = 1\n").unwrap();
        let m = manifest_with_config("app", "appns", CONFIG_JSON);
        write_config_unstarted(&m, &path, json!({ "tushare.token": "tok", "mode": "fast" }))
            .await
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("keep_me = 1"),
            "unrecognized sections should be preserved: {text}"
        );
        assert!(text.contains("tok"));
        assert!(text.contains("fast"));
    }

    #[tokio::test]
    async fn write_then_state_transitions_to_ready() {
        // needs-config → (post-config validation passes) → ready (main path, rule 2)
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let m = manifest_with_config("app", "appns", CONFIG_JSON);
        // Simulate an installed component: the SDK apply writing config.toml
        // does not create parent directories (the component-side cwd is
        // guaranteed by the shell)
        std::fs::create_dir_all(layout.component_dir("app")).unwrap();
        let path = layout.component_config_file("app");
        assert_eq!(
            compute_state(&m, &path).await.unwrap(),
            ComponentState::NeedsConfig
        );
        write_config_unstarted(&m, &path, json!({ "tushare.token": "tok" }))
            .await
            .unwrap();
        assert_eq!(
            compute_state(&m, &path).await.unwrap(),
            ComponentState::Ready
        );
        // Settled-state persistence loop
        let meta = reconcile_state(&layout, &m).await.unwrap();
        assert_eq!(meta.state, ComponentState::Ready);
        assert_eq!(meta.launch_policy, LaunchPolicy::Auto);
    }

    #[tokio::test]
    async fn reconcile_keeps_existing_policy_and_marks_needs_config() {
        // Upgrade scenario: meta already exists (Ready/Manual), the new
        // manifest version introduces a new required field →
        // state falls back to needs-config, policy is kept
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        let m = manifest_with_config("app", "appns", CONFIG_JSON);
        save_meta(
            &layout,
            "app",
            &ComponentMeta {
                state: ComponentState::Running,
                launch_policy: LaunchPolicy::Manual,
            },
        )
        .await
        .unwrap();
        let meta = reconcile_state(&layout, &m).await.unwrap();
        assert_eq!(meta.state, ComponentState::NeedsConfig);
        assert_eq!(meta.launch_policy, LaunchPolicy::Manual);
    }

    // ---------- meta persistence / state composition ----------

    #[tokio::test]
    async fn meta_round_trip_and_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let layout = DataLayout::new(dir.path());
        assert!(load_meta(&layout, "app").await.is_none());
        save_meta(
            &layout,
            "app",
            &ComponentMeta {
                state: ComponentState::Stopped,
                launch_policy: LaunchPolicy::Manual,
            },
        )
        .await
        .unwrap();
        let meta = load_meta(&layout, "app").await.unwrap();
        assert_eq!(meta.state, ComponentState::Stopped);
        assert_eq!(meta.launch_policy, LaunchPolicy::Manual);
    }

    #[test]
    fn effective_state_running_wins() {
        let meta = ComponentMeta {
            state: ComponentState::Stopped,
            launch_policy: LaunchPolicy::Manual,
        };
        assert_eq!(
            effective_state(Some(meta), true, ComponentState::Ready),
            ComponentState::Running
        );
    }

    #[test]
    fn effective_state_orphan_starting_falls_back_to_computed() {
        // Orphan starting (shell exited while starting) derives from the
        // scene and never gets stuck permanently
        let meta = ComponentMeta {
            state: ComponentState::Starting,
            launch_policy: LaunchPolicy::Auto,
        };
        assert_eq!(
            effective_state(Some(meta.clone()), false, ComponentState::Ready),
            ComponentState::Ready
        );
        assert_eq!(
            effective_state(Some(meta), false, ComponentState::NeedsConfig),
            ComponentState::NeedsConfig
        );
    }

    #[test]
    fn effective_state_none_uses_computed() {
        assert_eq!(
            effective_state(None, false, ComponentState::NeedsConfig),
            ComponentState::NeedsConfig
        );
    }

    #[test]
    fn effective_state_needs_config_beats_stale_meta() {
        // config.toml changed from outside the shell causing missing required
        // fields (a scene fact): stale meta (stopped/ready) must not mask
        // needs-config — UI display must stay consistent with the start gate
        for stale in [ComponentState::Stopped, ComponentState::Ready] {
            let meta = ComponentMeta {
                state: stale,
                launch_policy: LaunchPolicy::Manual,
            };
            assert_eq!(
                effective_state(Some(meta), false, ComponentState::NeedsConfig),
                ComponentState::NeedsConfig,
                "stale meta {stale:?} should be overridden with needs-config"
            );
        }
    }

    // ---------- restart_required change detection ----------

    #[test]
    fn restart_required_detects_change() {
        let config = r#"{ "fields": [
            { "key": "t", "label": "t", "type": "string", "restart_required": true },
            { "key": "h", "label": "h", "type": "string" }
        ] }"#;
        let m = manifest_with_config("r", "rns", config);
        let fields = &m.config.as_ref().unwrap().fields;

        let old = serde_json::Map::new();
        let mut new = serde_json::Map::new();
        new.insert("t".to_string(), json!("v"));
        // Newly added restart_required key = change
        assert!(restart_required_changed(fields, &old, &new));

        let mut old2 = new.clone();
        // Values unchanged = no change
        assert!(!restart_required_changed(fields, &old2, &new));
        // Changes to non-restart_required fields do not count
        old2.insert("h".to_string(), json!("x"));
        let mut new2 = old2.clone();
        new2.insert("h".to_string(), json!("y"));
        assert!(!restart_required_changed(fields, &old2, &new2));
        // Value changed = change
        let mut new3 = new.clone();
        new3.insert("t".to_string(), json!("v2"));
        assert!(restart_required_changed(fields, &new, &new3));
    }

    // ---------- In-flight forwarding ----------

    /// In-process mock component: /config PUT replies 400 (validation-failure
    /// form) or 200.
    async fn serve_mock_config(
        put_status: axum::http::StatusCode,
        body: serde_json::Value,
    ) -> Endpoint {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().route(
                    "/config",
                    axum::routing::put(move || {
                        let body = body.clone();
                        async move { (put_status, axum::Json(body)) }
                    }),
                ),
            )
            .await
            .unwrap();
        });
        Endpoint::Tcp { addr }
    }

    #[tokio::test]
    async fn forward_put_round_trips_status_and_body() {
        let endpoint = serve_mock_config(
            axum::http::StatusCode::BAD_REQUEST,
            json!({ "errors": { "a": "bad" } }),
        )
        .await;
        let (status, bytes) = forward_config_put(&endpoint, "tok", json!({ "a": 1 }))
            .await
            .unwrap();
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["errors"]["a"], "bad");
    }
}
