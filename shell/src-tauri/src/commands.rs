//! Tauri command bridge: a thin wrapper between the shell frontend ↔ the
//! Rust core (installer/lifecycle/registry/config_plane).
//!
//! Responsibility boundary: this module only does the translation of
//! "command parameters ↔ core function parameters" and error textification,
//! implementing no new business logic; two exceptions — [`uninstall_component`]'s
//! "stop the process first, then delete data" sequencing, and
//! [`write_component_config`]'s running-state branch "write → restart_required
//! takes effect with restart" orchestration (both are combinations of existing
//! primitives, not intrusions into module-internal state machines).
//!
//! Error form: commands uniformly return `Result<T, String>` to the frontend
//! (Display error text); the frontend just displays it, with no structured
//! error classification needed. The only structured exception:
//! `write_component_config`'s **validation failure** returns the JSON text
//! `{"errors": {key: reason}}` (field-by-field localization on the config
//! page).

use crate::config_plane;
use crate::{installer, registry, supervisor, AppState};
use serde::Serialize;
use slsdk_rs::{ComponentManifest, MenuDeclaration, RouteDeclaration, UiDeclaration};
use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::sync::Arc;

/// Component entry for the frontend view: on-disk registered manifest
/// (authoritative) merged with running-table state.
/// Field names stay snake_case, consistent with the manifest contract fields
/// (slsdk_rs's serde form); the frontend TS interface is defined in the same
/// form, with no two-layer naming conversion.
#[derive(Debug, Serialize)]
pub struct ComponentView {
    pub id: String,
    pub version: String,
    pub namespace: String,
    pub display_name: String,
    pub running: bool,
    pub ui: UiDeclaration,
    pub menu: Option<MenuDeclaration>,
    pub routes: Vec<RouteDeclaration>,
}

impl ComponentView {
    fn from_manifest(manifest: ComponentManifest, running: bool) -> Self {
        Self {
            id: manifest.id,
            version: manifest.version,
            namespace: manifest.namespace,
            display_name: manifest.display_name,
            running,
            ui: manifest.ui,
            menu: manifest.menu,
            routes: manifest.routes,
        }
    }
}

/// Fetch the manifest by id from the on-disk registration (install/uninstall/
/// start/stop all treat the on-disk registration as authoritative).
async fn find_manifest(state: &AppState, id: &str) -> Result<ComponentManifest, String> {
    registry::load_all_manifests(&state.layout)
        .await
        .map_err(|e| format!("failed to read the component registry: {e}"))?
        .into_iter()
        .find(|m| m.id == id)
        .ok_or_else(|| format!("component not installed: {id}"))
}

/// Installed component list: on-disk registration + running state merged
/// (data source for the management page list and the dynamic menu).
#[tauri::command]
pub async fn list_components(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ComponentView>, String> {
    let manifests = registry::load_all_manifests(&state.layout)
        .await
        .map_err(|e| format!("failed to read the component registry: {e}"))?;
    // The running table's key is the namespace (the gateway dispatch key);
    // running state is judged by it
    let running: HashSet<String> = state
        .registry
        .list()
        .await
        .into_iter()
        .map(|s| s.namespace)
        .collect();
    Ok(manifests
        .into_iter()
        .map(|m| {
            let running = running.contains(&m.namespace);
            ComponentView::from_manifest(m, running)
        })
        .collect())
}

/// Install a component package (zip disk path). Installing only lands files
/// and registration, changing no running state —
/// on a same-id reinstall (upgrade) with the component running, the old
/// process keeps running; the user switches explicitly via stop/start.
///
/// Configuration plane (rule 3): after landing, the settled state is
/// re-compared against the new manifest and the existing config.toml
/// (missing required → needs-config blocks the start gate; complete →
/// ready), launch policy keeps its existing value.
#[tauri::command]
pub async fn install_component(
    state: tauri::State<'_, AppState>,
    zip_path: String,
) -> Result<ComponentView, String> {
    let manifest = installer::install(Path::new(&zip_path), &state.layout)
        .await
        .map_err(|e| e.to_string())?;
    config_plane::reconcile_state(&state.layout, &manifest)
        .await
        .map_err(|e| format!("failed to persist the config state after install: {e}"))?;
    Ok(ComponentView::from_manifest(manifest, false))
}

/// Uninstall a component. Ordering contract: stop the process first, wait
/// for the running-table removal to complete, then delete data (install
/// directory → on-disk registration → shell-side meta). A stop timeout
/// aborts the whole operation (no data deleted), avoiding the wild-process
/// state of "process still up, data already deleted".
#[tauri::command]
pub async fn uninstall_component(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let manifest = find_manifest(&state, &id).await?;
    let namespace = manifest.namespace;

    // 1. Running: send the stop signal and poll until supervise finishes
    //    "graceful exit → registration removal → settled-state persistence".
    //    Re-sending request_stop inside the loop is idempotent: if the entry
    //    is still there the signal is sent normally; if the receiver has
    //    dropped (supervise is finishing) it returns false with no side
    //    effects.
    if state.registry.lookup(&namespace).await.is_some() {
        config_plane::stop_and_wait_removed(&state.registry, &namespace, &id).await?;
    }

    // 2. Delete the install directory (the /comps/{id} static space
    //    disappears with it). A missing directory counts as already clean
    //    (idempotent).
    match tokio::fs::remove_dir_all(state.layout.component_dir(&id)).await {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("failed to remove the component directory: {e}")),
    }

    // 3. Delete the on-disk registration (registry/{id}.json) and the
    //    shell-side meta (registry/{id}.meta).
    match tokio::fs::remove_file(state.layout.registry_file(&id)).await {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "failed to remove the registry entry from disk: {e}"
            ))
        }
    }
    match tokio::fs::remove_file(state.layout.meta_file(&id)).await {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("failed to remove the component meta: {e}")),
    }

    log::info!("component uninstalled: {id} (ns={namespace})");
    Ok(())
}

/// Start a component: the on-disk registered manifest goes through the full
/// start flow (spawn → health → registration).
///
/// Config gate (lifecycle rule 2 of three): needs-config (required config
/// missing) rejects startup, and the error text lists the missing keys to
/// guide the user to the config page.
#[tauri::command]
pub async fn start_component(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    let manifest = find_manifest(&state, &id).await?;
    let config_path = state.layout.component_config_file(&id);
    let missing = config_plane::required_missing_keys(&manifest, &config_path).await?;
    if !missing.is_empty() {
        return Err(format!(
            "component {id} is missing required config ({}); complete it on the configuration page before starting",
            missing.join("、")
        ));
    }
    if state.registry.lookup(&manifest.namespace).await.is_some() {
        return Err(format!("component {id} is already running"));
    }
    supervisor::launch_with_transition(&state, Arc::new(manifest))
        .await
        .map_err(|e| e.to_string())
}

/// Stop a component: request a graceful exit (the actual removal and state
/// persistence are completed by supervise cleanup).
#[tauri::command]
pub async fn stop_component(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    let manifest = find_manifest(&state, &id).await?;
    if !state.registry.request_stop(&manifest.namespace).await {
        return Err(format!("component {id} is not running"));
    }
    Ok(())
}

// ---------- Configuration plane (architecture.md "Configuration plane"; the command contract is the UI-line coding anchor) ----------

/// Effective-config restart of a running component: request graceful stop →
/// wait for running-table removal (supervise cleanup persists the settled
/// state in the same breath) → start again (starting → running transition
/// loop).
async fn restart_running_component(
    state: &AppState,
    manifest: Arc<ComponentManifest>,
) -> Result<(), String> {
    let namespace = manifest.namespace.clone();
    let id = manifest.id.clone();
    config_plane::stop_and_wait_removed(&state.registry, &namespace, &id).await?;
    supervisor::launch_with_transition(state, manifest)
        .await
        .map_err(|e| e.to_string())
}

/// Read a component's config (GET semantics, merged shell-side from
/// manifest+config.toml).
///
/// Returns: `{ "fields": [ConfigField...], "values": {"<key>": <value>}, "i18n": {"<lang>": "<path under ui/>"} | null }`
/// - `fields`: the manifest declaration list
///   (key/label/type/default/required/options/format/restart_required/sensitive/group)
/// - `values`: flat key→value (existing file value first → declared default
///   → omitted when both are missing)
/// - `i18n`: manifest config.i18n verbatim (lang → JSON path relative to the
///   component's `ui/` directory, fetchable via the `/comps/{id}/` static
///   space); with no config section: fields=[] values={} i18n=null
#[tauri::command]
pub async fn get_component_config(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<serde_json::Value, String> {
    let manifest = find_manifest(&state, &id).await?;
    let config_path = state.layout.component_config_file(&id);
    let values = config_plane::merged_config(&manifest, &config_path)
        .await
        .map_err(|e| e.to_string())?;
    let empty: Vec<slsdk_rs::ConfigField> = Vec::new();
    let fields = manifest
        .config
        .as_ref()
        .map(|c| c.fields.as_slice())
        .unwrap_or(&empty);
    let i18n = manifest.config.as_ref().and_then(|c| c.i18n.as_ref());
    Ok(serde_json::json!({
        "fields": fields,
        "values": values,
        "i18n": i18n,
    }))
}

/// Write a component's config (PUT semantics).
///
/// - Not running: shell-side validation + write config.toml (reusing the
///   slsdk ConfigStore, exactly the same convention as the component's
///   standard endpoint); on success the state is recomputed
///   (needs-config → ready).
/// - Running: forwards to the component's standard endpoint `PUT /config`
///   (component validation + write-back + hot-reload of runtime entries);
///   when a restart_required entry changes, the shell restarts the component
///   automatically to make the config effective.
///
/// Success returns: `{ "restart_required_changed": bool }` (true = component
/// restart triggered).
/// Validation failure returns Err whose content is the JSON
/// `{"errors": {"<key>": "<reason>", "_": "<global error>"}}`
/// (the UI parses it for field-by-field display); other failures return Err
/// as plain text.
#[tauri::command]
pub async fn write_component_config(
    state: tauri::State<'_, AppState>,
    id: String,
    values: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let manifest = find_manifest(&state, &id).await?;
    let config_path = state.layout.component_config_file(&id);

    // Running: forward to the component's standard endpoint (the component
    // is the sole authority for validation and write-back)
    if let Some(snapshot) = state.registry.lookup(&manifest.namespace).await {
        // Old values before the PUT: baseline for restart_required change
        // detection (isomorphic to the component's PUT response)
        let old_values = config_plane::merged_config(&manifest, &config_path)
            .await
            .map_err(|e| e.to_string())?;
        let (status, body) =
            config_plane::forward_config_put(&snapshot.endpoint, &snapshot.token, values).await?;
        if status == axum::http::StatusCode::BAD_REQUEST {
            // Component validation failed: relay {"errors": {...}} verbatim
            // (the UI and the unstarted state parse it uniformly)
            return Err(String::from_utf8_lossy(&body).into_owned());
        }
        if !status.is_success() {
            return Err(format!(
                "component config endpoint returned {status}: {}",
                String::from_utf8_lossy(&body)
            ));
        }
        let updated: serde_json::Value = serde_json::from_slice(&body)
            .map_err(|e| format!("failed to parse the component config response: {e}"))?;
        let new_values = updated
            .get("values")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        let empty: Vec<slsdk_rs::ConfigField> = Vec::new();
        let fields = manifest
            .config
            .as_ref()
            .map(|c| c.fields.as_slice())
            .unwrap_or(&empty);
        let changed = config_plane::restart_required_changed(fields, &old_values, &new_values);
        if changed {
            // Effective semantics: restart_required entries are applied by
            // the shell restarting the component (architecture.md
            // "Config read/write")
            restart_running_component(&state, Arc::new(manifest)).await?;
        }
        return Ok(serde_json::json!({ "restart_required_changed": changed }));
    }

    // Not running: shell-side validation + file write (same convention as the
    // component endpoint, reusing the SDK ConfigStore)
    match config_plane::write_config_unstarted(&manifest, &config_path, values).await {
        Ok(()) => {}
        Err(slsdk_rs::ConfigError::Validation(errors)) => {
            return Err(serde_json::json!({ "errors": errors }).to_string());
        }
        Err(e) => return Err(e.to_string()),
    }
    // Settled-state recompute and persist: with required complete,
    // needs-config → ready (gate opens)
    config_plane::reconcile_state(&state.layout, &manifest)
        .await
        .map_err(|e| format!("failed to persist the config state: {e}"))?;
    Ok(serde_json::json!({ "restart_required_changed": false }))
}

/// Query a component's state.
///
/// Returns the state string: `"needs-config"` | `"ready"` | `"starting"` |
/// `"running"` | `"stopped"`.
/// `starting` never leaks out: during a normal starting period
/// effective_state falls back to the computed settled state, so the UI
/// actually consumes the four states `"needs-config"` | `"ready"` |
/// `"running"` | `"stopped"`.
/// Composition rule: live running-table state first (running); orphan
/// starting (leftover from a shell interruption) derives from the scene;
/// otherwise the shell-side meta wins (scene derivation with no meta).
#[tauri::command]
pub async fn get_component_state(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<String, String> {
    let manifest = find_manifest(&state, &id).await?;
    let running = state.registry.lookup(&manifest.namespace).await.is_some();
    let config_path = state.layout.component_config_file(&id);
    let computed = config_plane::compute_state(&manifest, &config_path).await?;
    let meta = config_plane::load_meta(&state.layout, &id).await;
    Ok(config_plane::effective_state(meta, running, computed)
        .as_str()
        .to_string())
}

/// Set a component's launch policy (first item of the launch options slot).
///
/// `policy`: `"auto"` (started automatically at shell startup, needs-config
/// still hits the gate) | `"manual"` (only started explicitly by the user).
#[tauri::command]
pub async fn set_launch_policy(
    state: tauri::State<'_, AppState>,
    id: String,
    policy: config_plane::LaunchPolicy,
) -> Result<(), String> {
    let manifest = find_manifest(&state, &id).await?;
    let existing = config_plane::load_meta(&state.layout, &id).await;
    let state_value = match existing {
        Some(meta) => meta.state,
        None => {
            // No meta (pre-existing component): backstop with the scene-derived settled state
            let config_path = state.layout.component_config_file(&id);
            config_plane::compute_state(&manifest, &config_path).await?
        }
    };
    config_plane::save_meta(
        &state.layout,
        &id,
        &config_plane::ComponentMeta {
            state: state_value,
            launch_policy: policy,
        },
    )
    .await
    .map_err(|e| format!("failed to persist the start policy: {e}"))
}
