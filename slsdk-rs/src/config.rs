//! Component standard config endpoint (`GET/PUT /config`) — the runtime value read/write
//! channel described in architecture.md "Configuration Plane · Config Read/Write". Once a
//! component declares a `config` section in its manifest and integrates via
//! [`crate::serve_with_config`], the endpoint is mounted automatically with zero extra code.
//!
//! # Wire contract (the alignment anchor between the shell config page and components)
//!
//! What the component receives are paths with the `/api/{ns}` prefix already stripped by the
//! shell; the endpoint lives at `/config` in the component's own path space (full shape on the
//! shell side: `/api/{ns}/config`), coexisting with `/health`.
//!
//! - **GET /config** → `200 {"fields": [ConfigField...], "values": {"<key>": <value>...}}`
//!   - `values` is a **flat key→value** map; keys are the dotted-path form of
//!     [`ConfigField::key`] (e.g. `"tushare.token"`), and the shell renders the form by
//!     looking up values per key.
//!   - Merging semantics: existing values in config.toml take priority; missing entries fall
//!     back to declared defaults; when both are absent the key does not appear in `values`
//!     (the UI handles placeholders itself). Unrecognized sections are never leaked (GET only
//!     returns the declared list).
//!   - sensitive fields **return the real value as-is**: the endpoint runs behind token auth;
//!     masking is the shell UI rendering layer's responsibility (architecture.md
//!     "Configuration Plane": values are returned in real form).
//! - **PUT /config** → body `{"<key>": <new value>, ...}` (partial update, multiple keys allowed)
//!   - Write-back happens only after all validations pass; any failure →
//!     `400 {"errors": {"<key>": "<reason>"}}`; a wholly invalid body uses the reserved key
//!     `"_"` to carry the global error. Per-key rules:
//!     1. The key must be in the declared list (undeclared keys are rejected with no write —
//!        "preserve unrecognized sections" means existing file content is kept as-is, not
//!        that the endpoint may add arbitrary keys);
//!     2. The value type must match `r#type` (select additionally requires ∈ options;
//!        datetime is validated as an ISO 8601 string, no deep calendar validation in v1);
//!     3. Non-empty strings with `format == "cron"` must parse under cron 0.12 (same crate
//!        and same semantics as sl-finance's existing validation; empty string = clear the
//!        schedule semantics, skip validation);
//!     4. required entries must be non-empty in the **merged state** (existing file values +
//!        this change; an empty string counts as missing for strings).
//!   - Write-back: dynamic toml write-back, **preserving unrecognized sections** — read into
//!     a toml::Value, then set along the key path; sections outside the declaration are kept
//!     as-is; missing sections auto-create tables. On success, fire on_change (full snapshot
//!     after write-back) and return `200` with a GET-shaped `{"fields", "values"}` (saves the
//!     UI a second GET).
//! - **405**: methods other than GET/PUT are rejected automatically by axum routing.
//!
//! # Single source of truth for declarations = manifest.config
//!
//! This module's endpoint only reads/writes **values**; field declarations are taken from
//! manifest.config on the serve side — the shell (reads the manifest to render the form) and
//! this endpoint (the same declaration set) are naturally same-sourced, with no second
//! parameter and no drift surface.

use crate::manifest::{ConfigField, ConfigFieldType};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use axum::Router;
use serde_json::json;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

/// Callback fired after a successful config write-back (hot-reload hook): receives the
/// **complete config snapshot after write-back** (a `toml::Value::Table`), which components
/// use to hot-reload runtime items (e.g. rebuilding cron tasks).
///
/// Called synchronously: the callback body runs on the PUT response path, so avoid long
/// blocking operations; for async hot reload, `tokio::spawn` inside the callback (see
/// sl-finance's cron handle shutdown+rebuild pattern).
pub type OnChangeCallback = Arc<dyn Fn(toml::Value) + Send + Sync>;

/// Config integration parameters for [`crate::serve_with_config`]: config.toml path +
/// hot-reload callback.
/// Field declarations are not part of the parameters — the single source of truth is
/// manifest.config (see module docs).
pub struct ConfigIntegration {
    /// Path to the component's config.toml. In the shell-launched scenario cwd = the
    /// component's install directory, so the convention is a relative path `"config.toml"`;
    /// standalone debugging and tests may pass absolute or temporary paths.
    pub path: PathBuf,
    /// Hot-reload hook fired after a successful write-back (None = pure persistence, no
    /// runtime side effects).
    pub on_change: Option<OnChangeCallback>,
}

/// Shared state of the config endpoint: declared fields (from manifest.config) + file path +
/// hot-reload hook.
pub struct ConfigStore {
    fields: Vec<ConfigField>,
    path: PathBuf,
    on_change: Option<OnChangeCallback>,
}

impl ConfigStore {
    /// Builds from the declared list, file path, and callback (shared entry for the serve
    /// side and tests).
    pub fn new(
        fields: Vec<ConfigField>,
        path: PathBuf,
        on_change: Option<OnChangeCallback>,
    ) -> Self {
        Self {
            fields,
            path,
            on_change,
        }
    }

    /// GET semantics: read the file + merge against declarations, producing a response body
    /// isomorphic to the wire contract.
    pub async fn merged(&self) -> Result<serde_json::Value, ConfigError> {
        let table = self.read_table().await?;
        let values = self.merged_values(&table);
        Ok(json!({ "fields": &self.fields, "values": values }))
    }

    /// PUT semantics: validate → dynamic toml write-back (preserving unrecognized sections)
    /// → on_change → return the updated shape.
    pub async fn apply(
        &self,
        updates: serde_json::Value,
    ) -> Result<serde_json::Value, ConfigError> {
        let updates = updates.as_object().ok_or_else(|| {
            ConfigError::invalid_updates("body must be a JSON object (a map of key to new value)")
        })?;
        let mut table = self.read_table().await?;

        // 1. Merged-state validation: collect all keys and return once, so the UI can locate
        //    errors per field
        let errors = self.validate(updates, &table);
        if !errors.is_empty() {
            return Err(ConfigError::Validation(errors));
        }

        // 2. Write-back: set along the dotted key path (missing sections auto-create
        //    tables); unrecognized sections are naturally preserved
        for (key, value) in updates {
            let toml_value = json_to_toml(value)?;
            set_path(&mut table, key, toml_value)?;
        }
        let serialized =
            toml::to_string_pretty(&table).map_err(|source| ConfigError::Serialize { source })?;
        tokio::fs::write(&self.path, serialized)
            .await
            .map_err(|source| ConfigError::Io {
                path: self.path.clone(),
                source,
            })?;

        // 3. Hot-reload hook: hand over the complete snapshot after write-back (the
        //    component decides which items need rebuilding)
        if let Some(on_change) = &self.on_change {
            on_change(table.clone());
        }

        let values = self.merged_values(&table);
        Ok(json!({ "fields": &self.fields, "values": values }))
    }

    /// Reads config.toml as a toml::Value. NotFound is digested into first-install semantics
    /// (empty table = all-defaults shape, architecture.md "Lifecycle Three Laws" #2); a
    /// non-table top level is treated as file corruption.
    async fn read_table(&self) -> Result<toml::Value, ConfigError> {
        let text = match tokio::fs::read_to_string(&self.path).await {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(toml::Value::Table(toml::map::Map::new()));
            }
            Err(source) => {
                return Err(ConfigError::Io {
                    path: self.path.clone(),
                    source,
                })
            }
        };
        let parsed: toml::Value = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: self.path.clone(),
            reason: source.to_string(),
        })?;
        if !parsed.is_table() {
            return Err(ConfigError::Parse {
                path: self.path.clone(),
                reason: "config.toml top level is not a table".to_string(),
            });
        }
        Ok(parsed)
    }

    /// Merging semantics: existing file values take priority → declared defaults → if both
    /// are absent the key does not appear (see module docs GET).
    fn merged_values(&self, table: &toml::Value) -> serde_json::Map<String, serde_json::Value> {
        let mut values = serde_json::Map::new();
        for field in &self.fields {
            if let Some(v) = lookup_path(table, &field.key) {
                values.insert(field.key.clone(), toml_to_json(v));
            } else if let Some(default) = &field.default {
                values.insert(field.key.clone(), default.clone());
            }
        }
        values
    }

    /// Per-key validation + required merged-state validation; errors collected by key
    /// (returned in one full pass).
    fn validate(
        &self,
        updates: &serde_json::Map<String, serde_json::Value>,
        table: &toml::Value,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut errors = serde_json::Map::new();
        let by_key: HashMap<&str, &ConfigField> =
            self.fields.iter().map(|f| (f.key.as_str(), f)).collect();

        for (key, value) in updates {
            let Some(field) = by_key.get(key.as_str()) else {
                errors.insert(key.clone(), json!("undeclared config key, rejecting write"));
                continue;
            };
            if let Some(msg) = check_value(field, value) {
                errors.insert(key.clone(), json!(msg));
            }
        }

        // required merged-state (existing file values + this change) non-empty check; an
        // empty string counts as missing
        for field in self.fields.iter().filter(|f| f.required) {
            let effective = updates
                .get(&field.key)
                .cloned()
                .or_else(|| lookup_path(table, &field.key).map(toml_to_json));
            let missing = match effective {
                None => true,
                Some(v) => v.as_str() == Some(""),
            };
            if missing {
                errors.insert(
                    field.key.clone(),
                    json!("required field is still empty after merge"),
                );
            }
        }
        errors
    }
}

/// Config endpoint router (`/config`, GET+PUT): this is exactly what
/// [`crate::serve_with_config`] merges internally; the public shape allows reuse by tests and
/// advanced "component manages its own config routing" scenarios, keeping the wiring
/// consistent.
pub fn config_router(store: Arc<ConfigStore>) -> Router {
    Router::new()
        .route("/config", get(config_get).put(config_put))
        .with_state(store)
}

async fn config_get(State(store): State<Arc<ConfigStore>>) -> Response {
    match store.merged().await {
        Ok(body) => Json(body).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn config_put(State(store): State<Arc<ConfigStore>>, body: Bytes) -> Response {
    // Parse the body manually: uniform 400 shape ({"errors":{"_":...}}), avoiding the
    // 415/422 multi-shape error surface of the Json extractor
    let updates = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(v) => v,
        Err(e) => {
            return ConfigError::invalid_updates(format!("body is not valid JSON: {e}"))
                .into_response()
        }
    };
    match store.apply(updates).await {
        Ok(body) => Json(body).into_response(),
        Err(err) => err.into_response(),
    }
}

/// Config endpoint errors (IntoResponse maps status codes per the module docs wire contract).
#[derive(Debug)]
pub enum ConfigError {
    /// Reading/writing config.toml failed (permissions etc.; NotFound is already digested
    /// into first-install semantics on the read side) → 500.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// config.toml parse failure (corrupted file / non-table top level) → 500.
    Parse { path: PathBuf, reason: String },
    /// Write-back serialization failure → 500.
    Serialize { source: toml::ser::Error },
    /// Declared key path conflicts with an existing scalar in the file (e.g. file has
    /// `a = 1` while the declaration is `a.b`) → 500.
    PathConflict { key: String },
    /// PUT body wholly invalid (non-object / value unmappable to TOML) → 400, carried by the
    /// reserved key "_".
    InvalidUpdates(String),
    /// Per-key validation failure → 400.
    Validation(serde_json::Map<String, serde_json::Value>),
}

impl ConfigError {
    fn invalid_updates(msg: impl Into<String>) -> Self {
        ConfigError::InvalidUpdates(msg.into())
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io { path, source } => {
                write!(f, "failed to read/write {}: {source}", path.display())
            }
            ConfigError::Parse { path, reason } => {
                write!(f, "failed to parse {}: {reason}", path.display())
            }
            ConfigError::Serialize { source } => {
                write!(f, "failed to serialize config.toml: {source}")
            }
            ConfigError::PathConflict { key } => {
                write!(
                    f,
                    "config path {key} conflicts with an existing scalar value in the file"
                )
            }
            ConfigError::InvalidUpdates(msg) => write!(f, "invalid PUT body: {msg}"),
            ConfigError::Validation(_) => write!(
                f,
                "config validation failed (see the response body for per-key errors)"
            ),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } => Some(source),
            ConfigError::Serialize { source } => Some(source),
            _ => None,
        }
    }
}

impl IntoResponse for ConfigError {
    fn into_response(self) -> Response {
        match self {
            ConfigError::Validation(errors) => {
                (StatusCode::BAD_REQUEST, Json(json!({ "errors": errors }))).into_response()
            }
            ConfigError::InvalidUpdates(msg) => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "errors": { "_": msg } })),
            )
                .into_response(),
            other => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": other.to_string() })),
            )
                .into_response(),
        }
    }
}

/// Single key-value validation (type / select enum / cron format). Returns Some(reason) =
/// validation failed.
fn check_value(field: &ConfigField, value: &serde_json::Value) -> Option<String> {
    let type_ok = match field.r#type {
        // datetime treated as an ISO 8601 string in v1 (module docs)
        ConfigFieldType::String | ConfigFieldType::Datetime => value.is_string(),
        ConfigFieldType::Number => value.is_number(),
        ConfigFieldType::Boolean => value.is_boolean(),
        ConfigFieldType::Select => value.is_string(),
    };
    if !type_ok {
        return Some(format!("type mismatch: expected {}", field.r#type.as_str()));
    }
    if field.r#type == ConfigFieldType::Select {
        let given = value.as_str().expect("checked to be a string above");
        let allowed = field.options.as_deref().unwrap_or(&[]);
        if !allowed.iter().any(|o| o == given) {
            return Some(format!("value is not one of the declared options: {given}"));
        }
    }
    if field.format.as_deref() == Some("cron") {
        // Same semantics as sl-finance (cron 0.12 Schedule parsing); empty string = clear
        // schedule semantics, skip
        if let Some(expr) = value.as_str() {
            if !expr.is_empty() {
                if let Err(e) = expr.parse::<cron::Schedule>() {
                    return Some(format!("invalid cron expression: {e}"));
                }
            }
        }
    }
    None
}

/// Drills down a dotted path to fetch a value (any missing segment or non-table yields None).
fn lookup_path<'a>(table: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut cur = table;
    for seg in key.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

/// toml → json: TOML datetime's serde serialization is a private map shape, so explicitly
/// convert to an RFC 3339 string; other variants (scalars/arrays/tables) go through standard
/// serde.
fn toml_to_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
        other => serde_json::to_value(other).unwrap_or(serde_json::Value::Null),
    }
}

/// json → toml: validation has already guaranteed leaf types (string/number/bool); TOML has
/// no null, rejected here.
fn json_to_toml(value: &serde_json::Value) -> Result<toml::Value, ConfigError> {
    serde_json::from_value(value.clone())
        .map_err(|e| ConfigError::invalid_updates(format!("value cannot be mapped to TOML: {e}")))
}

/// Writes along a dotted path: missing intermediate sections auto-create tables; an existing
/// scalar in the way = path conflict (file and declaration misaligned).
/// Key shape (non-empty segments, no whitespace) is already guarded by manifest validate, not
/// repeated here.
fn set_path(table: &mut toml::Value, key: &str, value: toml::Value) -> Result<(), ConfigError> {
    // Empty/whitespace segments are already guarded by manifest validate; defense in depth
    // here against the empty string itself (split_at would underflow)
    let segments: Vec<&str> = key.split('.').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err(ConfigError::PathConflict {
            key: key.to_string(),
        });
    }
    let (head, last) = segments.split_at(segments.len() - 1);
    let mut cur = table;
    for seg in head {
        if !cur.is_table() {
            return Err(ConfigError::PathConflict {
                key: key.to_string(),
            });
        }
        let t = cur
            .as_table_mut()
            .expect("table-ness was checked by is_table");
        t.entry((*seg).to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        cur = t.get_mut(*seg).expect("entry was just inserted");
    }
    if !cur.is_table() {
        return Err(ConfigError::PathConflict {
            key: key.to_string(),
        });
    }
    cur.as_table_mut()
        .expect("table-ness was checked by is_table")
        .insert(last[0].to_string(), value);
    Ok(())
}
