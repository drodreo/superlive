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
use crate::import::ImportOutcome;
use crate::registry::{ComponentSource, RegistryEntry};
use crate::{installer, registry, supervisor, AppState};
use serde::Serialize;
use slsdk_rs::{ComponentManifest, MenuDeclaration, RouteDeclaration, UiDeclaration};
use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Component entry for the frontend view: on-disk registered entry
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
    /// `None` = pure-backend shape (no UI mount surface; the frontend renders
    /// nothing for such components — menu/routes are empty per validate).
    pub ui: Option<UiDeclaration>,
    pub menu: Option<MenuDeclaration>,
    pub routes: Vec<RouteDeclaration>,
    /// Where the component's body lives (installed package vs dev project).
    pub source: ComponentSource,
    /// Dev component's project directory; `None` for installed entries.
    pub project_dir: Option<String>,
}

impl ComponentView {
    pub(crate) fn from_entry(entry: RegistryEntry, running: bool) -> Self {
        let manifest = entry.manifest;
        Self {
            id: manifest.id,
            version: manifest.version,
            namespace: manifest.namespace,
            display_name: manifest.display_name,
            running,
            ui: manifest.ui,
            menu: manifest.menu,
            routes: manifest.routes,
            source: entry.source,
            project_dir: entry.project_dir.map(|d| d.to_string_lossy().into_owned()),
        }
    }
}

/// Fetch the on-disk registration by id (install/uninstall/start/stop all
/// treat the on-disk registration as authoritative).
async fn find_entry(state: &AppState, id: &str) -> Result<RegistryEntry, String> {
    registry::load_all_entries(&state.layout)
        .await
        .map_err(|e| format!("failed to read the component registry: {e}"))?
        .into_iter()
        .find(|e| e.manifest.id == id)
        .ok_or_else(|| format!("component not installed: {id}"))
}

/// Installed component list: on-disk registration + running state merged
/// (data source for the management page list and the dynamic menu).
#[tauri::command]
pub async fn list_components(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ComponentView>, String> {
    let mut entries = registry::load_all_entries(&state.layout)
        .await
        .map_err(|e| format!("failed to read the component registry: {e}"))?;
    // Newest registration first; legacy records without a timestamp sink to
    // the end (the management table and the dynamic menu inherit this order)
    registry::sort_newest_first(&mut entries);
    // The running table's key is the namespace (the gateway dispatch key);
    // running state is judged by it
    let running: HashSet<String> = state
        .registry
        .list()
        .await
        .into_iter()
        .map(|s| s.namespace)
        .collect();
    Ok(entries
        .into_iter()
        .map(|e| {
            let running = running.contains(&e.manifest.namespace);
            ComponentView::from_entry(e, running)
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
    config_plane::reconcile_state(
        &state.layout,
        &manifest,
        state.layout.component_config_file(&manifest.id),
    )
    .await
    .map_err(|e| format!("failed to persist the config state after install: {e}"))?;
    Ok(ComponentView::from_entry(
        RegistryEntry::installed(manifest),
        false,
    ))
}

/// Uninstall a component. Ordering contract: stop the process first, wait
/// for the running-table removal to complete, then delete data. A stop
/// timeout aborts the whole operation (no data deleted), avoiding the
/// wild-process state of "process still up, data already deleted".
///
/// Data scope by source: an installed entry loses its install directory +
/// registration + meta; a dev entry loses only the registration + meta —
/// the user's project directory is never touched (unregister ≠ delete the
/// code).
#[tauri::command]
pub async fn uninstall_component(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    uninstall_core(&state, &id).await
}

async fn uninstall_core(state: &AppState, id: &str) -> Result<(), String> {
    let entry = find_entry(state, id).await?;
    let manifest = entry.manifest;
    let namespace = manifest.namespace;

    // 1. Running: send the stop signal and poll until supervise finishes
    //    "graceful exit → registration removal → settled-state persistence".
    //    Re-sending request_stop inside the loop is idempotent: if the entry
    //    is still there the signal is sent normally; if the receiver has
    //    dropped (supervise is finishing) it returns false with no side
    //    effects.
    if state.registry.lookup(&namespace).await.is_some() {
        config_plane::stop_and_wait_removed(&state.registry, &namespace, id).await?;
    }

    // 2. Dev entries keep their project directory untouched; installed
    //    entries lose the install directory (the /comps/{id} static space
    //    disappears with it). A missing directory counts as already clean
    //    (idempotent).
    if entry.source == ComponentSource::Installed {
        match tokio::fs::remove_dir_all(state.layout.component_dir(id)).await {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("failed to remove the component directory: {e}")),
        }
    }

    // 3. Delete the on-disk registration (registry/{id}.json) and the
    //    shell-side meta (registry/{id}.meta).
    match tokio::fs::remove_file(state.layout.registry_file(id)).await {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "failed to remove the registry entry from disk: {e}"
            ))
        }
    }
    match tokio::fs::remove_file(state.layout.meta_file(id)).await {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("failed to remove the component meta: {e}")),
    }

    log::info!(
        "component uninstalled: {id} (ns={namespace}, source={:?})",
        entry.source
    );
    Ok(())
}

/// Start a component: the on-disk registered entry goes through the full
/// start flow (spawn → health → registration).
///
/// Config gate (lifecycle rule 2 of three): needs-config (required config
/// missing) rejects startup, and the error text lists the missing keys to
/// guide the user to the config page.
/// Dev gate: a backend declared but not yet built gets a directed
/// "build first" error instead of a vague spawn/health failure.
#[tauri::command]
pub async fn start_component(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    let entry = find_entry(&state, &id).await?;
    // Start/reload semantics for dev entries: re-read the project declaration,
    // refresh the version and the envelope snapshot, check source identity —
    // starting must run against the live project (failures are hard errors).
    let entry = registry::refresh_dev_entry(&state.layout, &entry)
        .await
        .map_err(|e| format!("failed to refresh the dev component declaration: {e}"))?;
    let config_path = registry::config_path_for(&state.layout, &entry);
    let missing = config_plane::required_missing_keys(&entry.manifest, &config_path).await?;
    if !missing.is_empty() {
        return Err(format!(
            "component {id} is missing required config ({}); complete it on the configuration page before starting",
            missing.join("、")
        ));
    }
    if state
        .registry
        .lookup(&entry.manifest.namespace)
        .await
        .is_some()
    {
        return Err(format!("component {id} is already running"));
    }
    registry::ensure_launch_artifact(&entry)?;
    let workdir = registry::workdir_for(&state.layout, &entry);
    supervisor::launch_with_transition(&state, Arc::new(entry.manifest), workdir)
        .await
        .map_err(|e| e.to_string())
}

/// Stop a component: request a graceful exit (the actual removal and state
/// persistence are completed by supervise cleanup).
#[tauri::command]
pub async fn stop_component(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    let manifest = find_entry(&state, &id).await?.manifest;
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
async fn restart_running_component(state: &AppState, entry: RegistryEntry) -> Result<(), String> {
    // Same start/reload semantics as start_component: dev entries re-read the
    // project declaration before the effective-config restart.
    let entry = registry::refresh_dev_entry(&state.layout, &entry)
        .await
        .map_err(|e| format!("failed to refresh the dev component declaration: {e}"))?;
    let namespace = entry.manifest.namespace.clone();
    let id = entry.manifest.id.clone();
    let workdir = registry::workdir_for(&state.layout, &entry);
    config_plane::stop_and_wait_removed(&state.registry, &namespace, &id).await?;
    supervisor::launch_with_transition(state, Arc::new(entry.manifest), workdir)
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
    let entry = find_entry(&state, &id).await?;
    let config_path = registry::config_path_for(&state.layout, &entry);
    let manifest = entry.manifest;
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
    let entry = find_entry(&state, &id).await?;
    let config_path = registry::config_path_for(&state.layout, &entry);

    // Running: forward to the component's standard endpoint (the component
    // is the sole authority for validation and write-back)
    if let Some(snapshot) = state.registry.lookup(&entry.manifest.namespace).await {
        // Old values before the PUT: baseline for restart_required change
        // detection (isomorphic to the component's PUT response)
        let old_values = config_plane::merged_config(&entry.manifest, &config_path)
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
        let fields = entry
            .manifest
            .config
            .as_ref()
            .map(|c| c.fields.as_slice())
            .unwrap_or(&empty);
        let changed = config_plane::restart_required_changed(fields, &old_values, &new_values);
        if changed {
            // Effective semantics: restart_required entries are applied by
            // the shell restarting the component (architecture.md
            // "Config read/write")
            restart_running_component(&state, entry).await?;
        }
        return Ok(serde_json::json!({ "restart_required_changed": changed }));
    }

    // Not running: shell-side validation + file write (same convention as the
    // component endpoint, reusing the SDK ConfigStore)
    match config_plane::write_config_unstarted(&entry.manifest, &config_path, values).await {
        Ok(()) => {}
        Err(slsdk_rs::ConfigError::Validation(errors)) => {
            return Err(serde_json::json!({ "errors": errors }).to_string());
        }
        Err(e) => return Err(e.to_string()),
    }
    // Settled-state recompute and persist: with required complete,
    // needs-config → ready (gate opens)
    config_plane::reconcile_state(&state.layout, &entry.manifest, config_path)
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
    let entry = find_entry(&state, &id).await?;
    let config_path = registry::config_path_for(&state.layout, &entry);
    let manifest = entry.manifest;
    let running = state.registry.lookup(&manifest.namespace).await.is_some();
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
    let entry = find_entry(&state, &id).await?;
    let config_path = registry::config_path_for(&state.layout, &entry);
    let manifest = entry.manifest;
    let existing = config_plane::load_meta(&state.layout, &id).await;
    let state_value = match existing {
        Some(meta) => meta.state,
        None => {
            // No meta (pre-existing component): backstop with the scene-derived settled state
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

// ---------- Component creation (the built-in creation flow's shell-side step; the UI dialog lands in phase 3b) ----------

/// Create-flow failures, split into channels the frontend can branch on:
/// caller mistakes vs world-state conflicts vs runtime faults (the command
/// surface keeps `Result<T, String>`; the error string carries this JSON).
#[derive(Debug)]
pub(crate) enum CreateError {
    /// Caller argument is malformed (unknown shape/sdk_mode, missing
    /// sdk_path). Fixing means re-submitting the form.
    InvalidArgument(String),
    /// World state moved (namespace/id already taken, target not empty, SDK
    /// checkout missing). Retrying verbatim fails again.
    Conflict(String),
    /// Runtime fault (IO/serialization) — not the caller's fault.
    Internal(String),
}

impl CreateError {
    pub(crate) fn message(&self) -> &str {
        match self {
            CreateError::InvalidArgument(m)
            | CreateError::Conflict(m)
            | CreateError::Internal(m) => m,
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            CreateError::InvalidArgument(_) => "invalid_argument",
            CreateError::Conflict(_) => "conflict",
            CreateError::Internal(_) => "internal",
        }
    }
}

impl serde::Serialize for CreateError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("CreateError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", self.message())?;
        s.end()
    }
}

impl std::fmt::Display for CreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind(), self.message())
    }
}

/// One localized-title row of the create dialog (locale + title verbatim).
#[derive(Debug, serde::Deserialize)]
pub struct DisplayTitleInput {
    pub locale: String,
    pub value: String,
}

/// Create-component request (the UI dialog's fields verbatim).
#[derive(Debug, serde::Deserialize)]
pub struct CreateComponentArgs {
    pub id: String,
    /// Localized display titles (locale → title rows; architecture.md
    /// "Localized titles"): at least one row, every row carries a non-empty
    /// locale and value, locales unique. The single-language fallback
    /// `display_name` is derived at assembly time from these rows.
    pub display_titles: Vec<DisplayTitleInput>,
    pub namespace: String,
    /// "frontend-backend" | "frontend" | "backend"
    pub shape: String,
    /// Project location (the dialog's position picker); must not exist or be
    /// an empty directory.
    pub target_dir: String,
    /// Fixed dev port written into the vite dev proxy / README recipe.
    pub dev_port: u16,
    /// "registry" (crates.io/npm versions) | "local" (path deps against a
    /// superlive checkout).
    pub sdk_mode: String,
    /// superlive checkout path; required when sdk_mode = "local".
    pub sdk_path: Option<String>,
}

/// Create-component outcome: the registered view plus the scaffolding
/// hints (the UI pops them as the post-create guide).
#[derive(Debug, Serialize)]
pub struct CreateComponentResult {
    pub component: ComponentView,
    pub project_dir: String,
    pub next_steps: Vec<String>,
}

fn parse_shape(shape: &str) -> Result<slscaffold::ComponentShape, CreateError> {
    match shape {
        "frontend-backend" => Ok(slscaffold::ComponentShape::FrontendAndBackend),
        "frontend" => Ok(slscaffold::ComponentShape::FrontendOnly),
        "backend" => Ok(slscaffold::ComponentShape::BackendOnly),
        other => Err(CreateError::InvalidArgument(format!(
            "unknown component shape: {other} (expected frontend-backend | frontend | backend)"
        ))),
    }
}

fn parse_sdk(
    sdk_mode: &str,
    sdk_path: Option<String>,
) -> Result<slscaffold::SdkDependency, CreateError> {
    match sdk_mode {
        "registry" => Ok(slscaffold::SdkDependency::Registry),
        "local" => match sdk_path {
            Some(path) => Ok(slscaffold::SdkDependency::Local {
                superlive_root: PathBuf::from(path),
            }),
            None => Err(CreateError::InvalidArgument(
                "sdk_mode \"local\" requires sdk_path (a superlive checkout)".to_string(),
            )),
        },
        other => Err(CreateError::InvalidArgument(format!(
            "unknown sdk_mode: {other} (expected registry | local)"
        ))),
    }
}

/// Validates the localized-title rows at the input boundary: a non-empty set,
/// each row carrying a non-empty locale and value, locales unique (a duplicate
/// would be silently collapsed by the manifest's map — reject here instead).
fn validate_display_titles(titles: &[DisplayTitleInput]) -> Result<(), CreateError> {
    if titles.is_empty() {
        return Err(CreateError::InvalidArgument(
            "display_titles must carry at least one locale row".to_string(),
        ));
    }
    let mut seen = HashSet::new();
    for row in titles {
        if row.locale.trim().is_empty() {
            return Err(CreateError::InvalidArgument(
                "display_titles contains an empty locale".to_string(),
            ));
        }
        if row.value.trim().is_empty() {
            return Err(CreateError::InvalidArgument(
                "display_titles contains an empty title value".to_string(),
            ));
        }
        if !seen.insert(row.locale.as_str()) {
            return Err(CreateError::InvalidArgument(format!(
                "display_titles contains a duplicate locale: {}",
                row.locale
            )));
        }
    }
    Ok(())
}

/// The single-language fallback title, derived at assembly time per the
/// contract chain: `titles['zh'] ?? titles['en'] ?? first entry`. The BTreeMap
/// keeps sorted keys, so "first entry" means the input Vec's first row, not
/// the map's iteration head.
fn fallback_display_name(titles: &[DisplayTitleInput]) -> String {
    let find = |locale: &str| {
        titles
            .iter()
            .find(|t| t.locale == locale)
            .map(|t| t.value.trim().to_string())
    };
    find("zh")
        .or_else(|| find("en"))
        .unwrap_or_else(|| titles[0].value.trim().to_string())
}

/// Create a dev component: scaffold the project, assemble the dev manifest
/// (same conventions as the packaging chain), register the dev envelope.
/// The project body stays at `target_dir`; the shell only writes the
/// registration.
#[tauri::command]
pub async fn create_component(
    state: tauri::State<'_, AppState>,
    args: CreateComponentArgs,
) -> Result<CreateComponentResult, String> {
    create_component_core(&state, args).await.map_err(|e| {
        serde_json::to_string(&e).unwrap_or_else(|_| {
            format!("{{\"kind\":\"internal\",\"message\":\"{}\"}}", e.message())
        })
    })
}

pub(crate) async fn create_component_core(
    state: &AppState,
    args: CreateComponentArgs,
) -> Result<CreateComponentResult, CreateError> {
    let shape = parse_shape(&args.shape)?;
    let sdk = parse_sdk(&args.sdk_mode, args.sdk_path.clone())?;
    validate_display_titles(&args.display_titles)?;
    let target = PathBuf::from(&args.target_dir);

    // Localized titles: the manifest's titles map (sorted keys → a stable
    // declaration file) plus the assembly-time fallback display name
    // (`titles['zh'] ?? titles['en'] ?? first row`, architecture.md
    // "Localized titles"). Every row additionally feeds the generated
    // project's i18n module (one registration line per locale, input order).
    let titles: BTreeMap<String, String> = args
        .display_titles
        .iter()
        .map(|t| (t.locale.trim().to_string(), t.value.trim().to_string()))
        .collect();
    let display_name = fallback_display_name(&args.display_titles);

    // Pre-flight on world state, before any filesystem mutation: id and
    // namespace are unique keys across the registry (dev or installed
    // alike), and the namespace must also be free in the running table.
    let existing = registry::load_all_entries(&state.layout)
        .await
        .map_err(|e| {
            CreateError::Internal(format!("failed to read the component registry: {e}"))
        })?;
    if existing.iter().any(|e| e.manifest.id == args.id) {
        return Err(CreateError::Conflict(format!(
            "component id is already registered: {}",
            args.id
        )));
    }
    if existing
        .iter()
        .any(|e| e.manifest.namespace == args.namespace)
    {
        return Err(CreateError::Conflict(format!(
            "the namespace is already in use: {}",
            args.namespace
        )));
    }
    if state.registry.lookup(&args.namespace).await.is_some() {
        return Err(CreateError::Conflict(format!(
            "the namespace is already in use by a running component: {}",
            args.namespace
        )));
    }

    // Scaffold (sync IO) + manifest version fact (readback from the project
    // version source): one blocking-task hop, the async side only assembles
    // and persists.
    let request = slscaffold::ScaffoldRequest {
        id: args.id.clone(),
        display_name: display_name.clone(),
        display_titles: args
            .display_titles
            .iter()
            .map(|t| slscaffold::LocalizedTitle {
                locale: t.locale.trim().to_string(),
                value: t.value.trim().to_string(),
            })
            .collect(),
        namespace: args.namespace.clone(),
        target_dir: target.clone(),
        shape,
        dev_port: args.dev_port,
        sdk,
    };
    let (scaffolded, version) =
        tokio::task::spawn_blocking(move || scaffold_and_read_version(&request, shape))
            .await
            .map_err(|_| CreateError::Internal("the creation task terminated unexpectedly".into()))?
            .map_err(|e| match e {
                ScaffoldStepError::Scaffold(source) => match source {
                    slscaffold::ScaffoldError::InvalidId(_)
                    | slscaffold::ScaffoldError::InvalidNamespace(_)
                    | slscaffold::ScaffoldError::InvalidDevPort(_)
                    | slscaffold::ScaffoldError::InvalidDisplayTitles(_) => {
                        CreateError::InvalidArgument(source.to_string())
                    }
                    slscaffold::ScaffoldError::TargetExists { .. }
                    | slscaffold::ScaffoldError::SdkMissing { .. } => {
                        CreateError::Conflict(source.to_string())
                    }
                    _ => CreateError::Internal(source.to_string()),
                },
                ScaffoldStepError::Inspect(m) => CreateError::Internal(m),
            })?;

    // Dev manifest assembly — a **pure declaration** (architecture.md
    // "Creation flow": the author may edit it afterwards; reload picks the
    // changes up; the version is a display value refreshed from the project's
    // version source). Shape mapping identical to the packaging chain's
    // manifest generation (ui/menu/routes for web shapes, launch for server
    // shapes; entry is relative to the dev static root web/dist). No files
    // list — fingerprints are a publish-time package concept.
    let has_web = matches!(
        shape,
        slscaffold::ComponentShape::FrontendAndBackend | slscaffold::ComponentShape::FrontendOnly
    );
    let has_server = matches!(
        shape,
        slscaffold::ComponentShape::FrontendAndBackend | slscaffold::ComponentShape::BackendOnly
    );
    let manifest = ComponentManifest {
        id: args.id.clone(),
        version,
        // The creation flow targets the running shell itself; the packaging
        // chain's install-time gate stays the authority for packages
        min_shell_version: env!("CARGO_PKG_VERSION").to_string(),
        namespace: args.namespace.clone(),
        ui: has_web.then(|| UiDeclaration {
            ui_type: slsdk_rs::UiType::Module,
            entry: "entry.js".to_string(),
        }),
        menu: has_web.then(|| MenuDeclaration {
            title: display_name.clone(),
            icon_path: None,
            titles: titles.clone(),
        }),
        routes: if has_web {
            vec![RouteDeclaration {
                path: "/".to_string(),
                title: display_name.clone(),
                titles: titles.clone(),
            }]
        } else {
            Vec::new()
        },
        launch: has_server.then(|| slsdk_rs::LaunchConfig {
            command: format!("./target/release/{}-server", args.id),
            args: Vec::new(),
        }),
        files: Vec::new(),
        config: None,
        // The single-language fallback title moves in last: menu/routes above
        // already cloned it (struct literal fields evaluate in written order)
        display_name,
    };
    manifest.validate_declaration().map_err(|e| {
        CreateError::Internal(format!("the assembled dev declaration is invalid: {e}"))
    })?;

    let entry = RegistryEntry {
        source: ComponentSource::Dev,
        project_dir: Some(scaffolded.root.clone()),
        manifest,
        // Registration timestamp: captured once, at creation
        registered_at: Some(registry::now_ms()),
    };
    {
        // Persist both the author-editable declaration file and the envelope
        // snapshot in one blocking hop.
        let layout = state.layout.clone();
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || {
            let project_dir = entry
                .project_dir
                .as_deref()
                .expect("a dev entry always carries a project_dir");
            registry::write_declaration_sync(project_dir, &entry.manifest)?;
            registry::persist_entry_sync(&layout, &entry)
        })
        .await
        .map_err(|_| CreateError::Internal("the persistence task terminated unexpectedly".into()))?
        .map_err(|e| {
            CreateError::Internal(format!("failed to persist the dev registration: {e}"))
        })?;
    }
    // Settled state (ready/auto) lands the same way an install does, so the
    // management page treats both sources uniformly. The dev config.toml lives
    // in the project directory (same source as the spawn workdir).
    let config_path = registry::config_path_for(&state.layout, &entry);
    config_plane::reconcile_state(&state.layout, &entry.manifest, config_path)
        .await
        .map_err(|e| CreateError::Internal(format!("failed to persist the config state: {e}")))?;

    log::info!(
        "dev component created: {} (ns={} shape={} project={})",
        args.id,
        args.namespace,
        args.shape,
        scaffolded.root.display()
    );

    let running = state.registry.lookup(&args.namespace).await.is_some();
    Ok(CreateComponentResult {
        component: ComponentView::from_entry(entry, running),
        project_dir: scaffolded.root.to_string_lossy().into_owned(),
        next_steps: scaffolded.next_steps,
    })
}

/// Failure of the blocking scaffold step (scaffold proper vs the inspect
/// pass that feeds the manifest assembly).
enum ScaffoldStepError {
    Scaffold(slscaffold::ScaffoldError),
    Inspect(String),
}

/// Import an existing project as a dev component (architecture.md
/// "Creation flow" — Importing an existing project). Phase 1 inspects the
/// directory (no disk mutation); `apply_upgrades = false` returns the
/// conservative gap-filling list for confirmation, `true` applies it and
/// registers. Error channels match the creation flow's JSON
/// `{"kind", "message"}` envelope.
#[tauri::command]
pub async fn import_component(
    state: tauri::State<'_, AppState>,
    dir: String,
    apply_upgrades: bool,
) -> Result<ImportOutcome, String> {
    crate::import::import_core(&state, &dir, apply_upgrades)
        .await
        .map_err(|e| {
            serde_json::to_string(&e).unwrap_or_else(|_| {
                format!("{{\"kind\":\"internal\",\"message\":\"{}\"}}", e.message())
            })
        })
}

/// The blocking half of creation: scaffold the project, then read back the
/// project version from the project's version source (same source as the
/// packaging chain: server/Cargo.toml for server shapes, web/package.json for
/// frontend-only). The declaration file itself is written by the async side
/// (it assembles the pure declaration from the user input + this version).
fn scaffold_and_read_version(
    request: &slscaffold::ScaffoldRequest,
    shape: slscaffold::ComponentShape,
) -> Result<(slscaffold::ScaffoldResult, String), ScaffoldStepError> {
    let scaffolded = slscaffold::scaffold(request).map_err(ScaffoldStepError::Scaffold)?;
    let has_server = matches!(
        shape,
        slscaffold::ComponentShape::FrontendAndBackend | slscaffold::ComponentShape::BackendOnly
    );
    let version = registry::project_version(&scaffolded.root, has_server)
        .map_err(ScaffoldStepError::Inspect)?;
    Ok((scaffolded, version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_plane::ComponentMeta;

    fn base_args(id: &str, ns: &str, shape: &str, target: &Path) -> CreateComponentArgs {
        CreateComponentArgs {
            id: id.to_string(),
            display_titles: vec![
                DisplayTitleInput {
                    locale: "zh".to_string(),
                    value: id.to_string(),
                },
                DisplayTitleInput {
                    locale: "en".to_string(),
                    value: id.to_string(),
                },
            ],
            namespace: ns.to_string(),
            shape: shape.to_string(),
            target_dir: target.to_string_lossy().into_owned(),
            dev_port: 5180,
            sdk_mode: "registry".to_string(),
            sdk_path: None,
        }
    }

    /// Pre-places one on-disk registration envelope (the minimal shape for conflict scenarios / uninstall tests).
    fn write_envelope(state: &AppState, entry: &RegistryEntry) {
        std::fs::create_dir_all(state.layout.registry_dir()).unwrap();
        std::fs::write(
            state.layout.registry_file(&entry.manifest.id),
            serde_json::to_string_pretty(entry).unwrap(),
        )
        .unwrap();
    }

    fn dev_entry(id: &str, ns: &str, project: &Path) -> RegistryEntry {
        RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(project.to_path_buf()),
            manifest: serde_json::from_str(&format!(
                r#"{{
                "id": "{id}",
                "version": "0.1.0",
                "min_shell_version": "0.1.0",
                "namespace": "{ns}",
                "display_name": "{id}",
                "ui": {{ "ui_type": "module", "entry": "entry.js" }},
                "menu": null,
                "routes": [],
                "launch": null,
                "files": [ {{ "path": "README.md", "sha256": "aa" }} ]
            }}"#
            ))
            .unwrap(),
            registered_at: None,
        }
    }

    fn installed_entry(id: &str, ns: &str) -> RegistryEntry {
        RegistryEntry::installed(
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
            .unwrap(),
        )
    }

    fn error_kind(err: &CreateError) -> &'static str {
        err.kind()
    }

    // ---------- create_component ----------

    #[tokio::test]
    async fn create_combined_registers_dev_entry() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let target = dir.path().join("myapp");

        let result = create_component_core(
            &state,
            base_args("myapp", "myns", "frontend-backend", &target),
        )
        .await
        .unwrap();
        // Project landed on disk + the registration view
        assert!(target.join("server/Cargo.toml").is_file());
        assert!(target.join("web/src/main.tsx").is_file());
        assert_eq!(result.project_dir, target.to_string_lossy());
        assert!(!result.next_steps.is_empty());
        let view = &result.component;
        assert_eq!(view.source, ComponentSource::Dev);
        assert_eq!(
            view.project_dir.as_deref(),
            Some(target.to_string_lossy().as_ref())
        );
        // Shape mapping (identical to the packaging chain): ui=module/entry.js, menu/routes, launch
        let ui = view.ui.as_ref().unwrap();
        assert_eq!(ui.ui_type, slsdk_rs::UiType::Module);
        assert_eq!(ui.entry, "entry.js");
        assert_eq!(view.menu.as_ref().unwrap().title, "myapp");
        assert_eq!(view.routes.len(), 1);
        assert_eq!(view.routes[0].path, "/");
        // Manifest assembly assertions go through the on-disk envelope (the persisted form is authoritative)
        let entry = registry::load_entry(&state.layout, "myapp")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.source, ComponentSource::Dev);
        assert_eq!(entry.project_dir.as_deref(), Some(target.as_path()));
        // The creation flow captures the registration timestamp at creation
        assert!(entry.registered_at.is_some());
        let m = &entry.manifest;
        let launch = m.launch.as_ref().unwrap();
        assert_eq!(launch.command, "./target/release/myapp-server");
        assert!(launch.args.is_empty());
        // Same version source as the packaging chain (server/Cargo.toml); min_shell_version = the shell version
        assert_eq!(m.version, "0.1.0");
        assert_eq!(m.min_shell_version, env!("CARGO_PKG_VERSION"));
        // Pure declaration: files is empty (fingerprints are a publish-time package concept); the declaration file
        // lands in the project directory and matches the envelope snapshot (envelope = snapshot of the declaration)
        assert!(m.files.is_empty());
        let declaration: ComponentManifest =
            serde_json::from_str(&std::fs::read_to_string(target.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(declaration, *m, "envelope snapshot = project declaration");
        // The settled state is persisted (same semantics as install)
        let meta = config_plane::load_meta(&state.layout, "myapp")
            .await
            .unwrap();
        assert_eq!(meta.state, crate::config_plane::ComponentState::Ready);
    }

    #[tokio::test]
    async fn create_frontend_only_has_no_launch() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let target = dir.path().join("webapp");
        create_component_core(&state, base_args("webapp", "webns", "frontend", &target))
            .await
            .unwrap();
        let m = &registry::load_entry(&state.layout, "webapp")
            .await
            .unwrap()
            .unwrap()
            .manifest;
        assert!(m.launch.is_none(), "frontend-only must not carry a launch");
        assert!(m.ui.is_some());
        // Same version source as the packaging chain's frontend-only shape (web/package.json)
        assert_eq!(m.version, "0.1.0");
    }

    #[tokio::test]
    async fn create_backend_only_has_no_ui_surface() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let target = dir.path().join("svcapp");
        create_component_core(&state, base_args("svcapp", "svcns", "backend", &target))
            .await
            .unwrap();
        let m = &registry::load_entry(&state.layout, "svcapp")
            .await
            .unwrap()
            .unwrap()
            .manifest;
        assert!(m.ui.is_none(), "backend-only must have no UI surface");
        assert!(m.menu.is_none());
        assert!(m.routes.is_empty());
        assert!(m.launch.is_some());
    }

    #[tokio::test]
    async fn create_assembles_localized_titles() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let target = dir.path().join("locapp");
        let mut args = base_args("locapp", "locns", "frontend", &target);
        args.display_titles = vec![
            DisplayTitleInput {
                locale: "en".to_string(),
                value: "Local Component".to_string(),
            },
            DisplayTitleInput {
                locale: "zh".to_string(),
                value: "本地组件".to_string(),
            },
            DisplayTitleInput {
                locale: "ja".to_string(),
                value: "ローカルコンポーネント".to_string(),
            },
        ];
        let result = create_component_core(&state, args).await.unwrap();

        // display_name derives zh-first regardless of row order
        let view = &result.component;
        assert_eq!(view.display_name, "本地组件");
        let menu = view.menu.as_ref().unwrap();
        assert_eq!(menu.title, "本地组件");
        assert_eq!(menu.titles.get("zh").map(String::as_str), Some("本地组件"));
        assert_eq!(
            menu.titles.get("en").map(String::as_str),
            Some("Local Component")
        );
        assert_eq!(
            menu.titles.get("ja").map(String::as_str),
            Some("ローカルコンポーネント")
        );
        assert_eq!(view.routes[0].path, "/");
        assert_eq!(
            view.routes[0].titles.get("en").map(String::as_str),
            Some("Local Component")
        );

        // The generated i18n module received every configured locale row
        // (one registration line per locale, verbatim)
        let i18n = std::fs::read_to_string(target.join("web/src/i18n.ts")).unwrap();
        assert!(
            i18n.contains("en: { title: 'Local Component' },"),
            "en title row missing: {i18n}"
        );
        assert!(
            i18n.contains("zh: { title: '本地组件' },"),
            "zh title row missing: {i18n}"
        );
        assert!(
            i18n.contains("ja: { title: 'ローカルコンポーネント' },"),
            "ja title row missing: {i18n}"
        );

        // The persisted declaration carries the same titles map (envelope =
        // snapshot of the declaration)
        let m = &registry::load_entry(&state.layout, "locapp")
            .await
            .unwrap()
            .unwrap()
            .manifest;
        assert_eq!(m.display_name, "本地组件");
        assert_eq!(m.menu.as_ref().unwrap().titles.len(), 3);
    }

    #[tokio::test]
    async fn create_rejects_invalid_title_rows() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let target = dir.path().join("app");

        // Empty set
        let mut args = base_args("app", "appns", "frontend", &target);
        args.display_titles = Vec::new();
        let err = create_component_core(&state, args).await.err().unwrap();
        assert_eq!(error_kind(&err), "invalid_argument", "{}", err);

        // Empty value row
        let mut args = base_args("app", "appns", "frontend", &target);
        args.display_titles = vec![DisplayTitleInput {
            locale: "zh".to_string(),
            value: "  ".to_string(),
        }];
        let err = create_component_core(&state, args).await.err().unwrap();
        assert_eq!(error_kind(&err), "invalid_argument", "{}", err);

        // Duplicate locales
        let mut args = base_args("app", "appns", "frontend", &target);
        args.display_titles = vec![
            DisplayTitleInput {
                locale: "zh".to_string(),
                value: "一".to_string(),
            },
            DisplayTitleInput {
                locale: "zh".to_string(),
                value: "二".to_string(),
            },
        ];
        let err = create_component_core(&state, args).await.err().unwrap();
        assert_eq!(error_kind(&err), "invalid_argument", "{}", err);

        // Nothing landed on disk (input validation precedes any mutation)
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn create_rejects_taken_namespace_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        // An installed entry already on disk holds the namespace
        write_envelope(&state, &installed_entry("existing", "busy-ns"));
        let err = create_component_core(
            &state,
            base_args(
                "newapp",
                "busy-ns",
                "frontend-backend",
                &dir.path().join("newapp"),
            ),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error_kind(&err), "conflict", "{}", err);
        // The project directory was not created (prechecks run before any filesystem change)
        assert!(!dir.path().join("newapp").exists());
    }

    #[tokio::test]
    async fn create_rejects_taken_id() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        write_envelope(&state, &dev_entry("same-id", "ns-a", &project));
        let err = create_component_core(
            &state,
            base_args("same-id", "ns-b", "frontend", &dir.path().join("fresh")),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error_kind(&err), "conflict", "{}", err);
    }

    #[tokio::test]
    async fn create_rejects_running_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        // The runtime table holds the namespace (a running record registered by hand)
        state
            .registry
            .register(
                "live-ns",
                crate::registry::RunningComponent {
                    manifest: Arc::new(
                        serde_json::from_str(
                            r#"{
                        "id": "live",
                        "version": "0.1.0",
                        "min_shell_version": "0.1.0",
                        "namespace": "live-ns",
                        "display_name": "live",
                        "ui": null,
                        "menu": null,
                        "routes": [],
                        "launch": { "command": "bin", "args": [] },
                        "files": [ { "path": "bin", "sha256": "aa" } ]
                    }"#,
                        )
                        .unwrap(),
                    ),
                    endpoint: slsdk_rs::Endpoint::Tcp {
                        addr: "127.0.0.1:9000".parse().unwrap(),
                    },
                    token: Arc::new("tok".to_string()),
                    pid: None,
                    stop: tokio::sync::watch::channel(()).0,
                },
            )
            .await
            .unwrap();
        let err = create_component_core(
            &state,
            base_args("newapp", "live-ns", "backend", &dir.path().join("newapp")),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error_kind(&err), "conflict", "{}", err);
    }

    #[tokio::test]
    async fn create_rejects_nonempty_target() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let target = dir.path().join("occupied");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("some.txt"), b"x").unwrap();
        let err = create_component_core(&state, base_args("newapp", "newns", "frontend", &target))
            .await
            .err()
            .unwrap();
        assert_eq!(error_kind(&err), "conflict", "{}", err);
    }

    #[tokio::test]
    async fn create_rejects_unknown_shape_as_invalid_argument() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let err = create_component_core(
            &state,
            base_args("app", "appns", "quantum", &dir.path().join("app")),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error_kind(&err), "invalid_argument", "{}", err);
    }

    // ---------- uninstall dev/installed branches ----------

    #[tokio::test]
    async fn uninstall_dev_keeps_project_dir() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join("web/dist")).unwrap();
        std::fs::write(project.join("web/dist/entry.js"), b"x").unwrap();
        write_envelope(&state, &dev_entry("devapp", "devns", &project));
        config_plane::save_meta(
            &state.layout,
            "devapp",
            &ComponentMeta {
                state: crate::config_plane::ComponentState::Ready,
                launch_policy: crate::config_plane::LaunchPolicy::Auto,
            },
        )
        .await
        .unwrap();

        uninstall_core(&state, "devapp").await.unwrap();

        // The project directory stays untouched (unregister ≠ delete the code)
        assert!(project.join("web/dist/entry.js").is_file());
        // Registration and meta removed
        assert!(registry::load_entry(&state.layout, "devapp")
            .await
            .unwrap()
            .is_none());
        assert!(config_plane::load_meta(&state.layout, "devapp")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn uninstall_installed_removes_install_dir() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        state.layout.ensure_dirs().unwrap();
        let install_dir = state.layout.component_dir("app");
        std::fs::create_dir_all(install_dir.join("ui")).unwrap();
        std::fs::write(install_dir.join("ui/main.js"), b"x").unwrap();
        write_envelope(&state, &installed_entry("app", "appns"));

        uninstall_core(&state, "app").await.unwrap();

        // Install directory removed + registration cleared (no regression of the existing behavior)
        assert!(!install_dir.exists());
        assert!(registry::load_entry(&state.layout, "app")
            .await
            .unwrap()
            .is_none());
    }
}
