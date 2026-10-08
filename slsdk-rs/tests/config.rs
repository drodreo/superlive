//! Config endpoint tests: ConfigStore core semantics (GET merging / PUT validation and
//! write-back / on_change hook) + config_router end to end.
//!
//! Deliberately not going through serve(): `SL_ENDPOINT`/`SL_TOKEN` are process-global env,
//! with a mutual-stomp window against tests/serve.rs running in parallel. Core semantics are
//! tested directly at the ConfigStore layer (no env dependency); end to end uses
//! config_router to build a Router + a local TCP listener — serve_with_config internally
//! merges exactly this same config_router function, so wiring consistency is guaranteed by
//! the shared function; the token middleware coverage semantics are already verified by
//! tests/serve.rs (unified layer after merge).

use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use serde_json::json;
use slsdk_rs::{ConfigField, ConfigFieldType, ConfigStore, OnChangeCallback};
use std::path::PathBuf;
use std::sync::Arc;

/// Digs down a dotted path into a toml::Value (explicit get chain, not relying on Value's
/// Index behavior for missing keys).
fn dig<'a>(v: &'a toml::Value, path: &str) -> Option<&'a toml::Value> {
    path.split('.').try_fold(v, |cur, seg| cur.get(seg))
}

// ---------- fixture helpers ----------

fn field(key: &str, field_type: ConfigFieldType) -> ConfigField {
    ConfigField {
        key: key.to_string(),
        label: format!("{key}.label"),
        r#type: field_type,
        default: None,
        required: false,
        options: None,
        format: None,
        restart_required: false,
        sensitive: false,
        group: None,
    }
}

fn with_default(mut f: ConfigField, default: serde_json::Value) -> ConfigField {
    f.default = Some(default);
    f
}

fn with_required(mut f: ConfigField) -> ConfigField {
    f.required = true;
    f
}

fn with_format(mut f: ConfigField, format: &str) -> ConfigField {
    f.format = Some(format.to_string());
    f
}

fn with_options(mut f: ConfigField, options: &[&str]) -> ConfigField {
    f.options = Some(options.iter().map(|s| s.to_string()).collect());
    f
}

fn store(
    dir: &tempfile::TempDir,
    fields: Vec<ConfigField>,
    on_change: Option<OnChangeCallback>,
) -> ConfigStore {
    ConfigStore::new(fields, config_path(dir), on_change)
}

fn config_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("config.toml")
}

/// Three-field declaration: required token / cron schedule / select style — isomorphic to
/// the sl-finance scenario.
fn finance_like_fields() -> Vec<ConfigField> {
    vec![
        with_required(field("tushare.token", ConfigFieldType::String)),
        with_format(
            with_default(
                field("tushare.schedule", ConfigFieldType::String),
                "".into(),
            ),
            "cron",
        ),
        with_options(
            with_default(
                field("kline.style", ConfigFieldType::Select),
                "candlestick".into(),
            ),
            &["candlestick", "line"],
        ),
    ]
}

// ---------- GET: merging semantics ----------

#[tokio::test]
async fn get_missing_file_returns_all_defaults() {
    // First-install semantics: file absent = all-defaults shape (Lifecycle Three Laws #2)
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir, finance_like_fields(), None);
    let body = store.merged().await.unwrap();
    assert!(body["values"].get("tushare.token").is_none()); // required without a default: absent when both are missing, awaiting user configuration
    assert_eq!(body["values"]["tushare.schedule"], "");
    assert_eq!(body["values"]["kline.style"], "candlestick");
    assert!(body["fields"].as_array().unwrap().len() == 3);
}

#[tokio::test]
async fn get_file_value_wins_and_missing_key_falls_back_to_default() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(
        config_path(&dir),
        "[tushare]\ntoken = \"real-token\"\n\n[server]\nhost = \"private\"\n",
    )
    .await
    .unwrap();
    let fields = vec![
        with_default(
            field("tushare.token", ConfigFieldType::String),
            "default-token".into(),
        ),
        with_default(
            field("tushare.schedule", ConfigFieldType::String),
            "0 0 3 * * *".into(),
        ),
    ];
    let store = store(&dir, fields, None);
    let body = store.merged().await.unwrap();
    // File values win; missing entries fall back to defaults
    assert_eq!(body["values"]["tushare.token"], "real-token");
    assert_eq!(body["values"]["tushare.schedule"], "0 0 3 * * *");
    // Unrecognized sections are not leaked: GET only returns the declared list
    // (unrecognized sections are component-private config)
    assert!(body["values"].get("server.host").is_none());
}

#[tokio::test]
async fn get_omits_key_without_value_and_default() {
    // No default declared and no file value → does not appear in values (the UI handles
    // placeholders itself)
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(config_path(&dir), "").await.unwrap();
    let store = store(
        &dir,
        vec![field("tushare.token", ConfigFieldType::String)],
        None,
    );
    let body = store.merged().await.unwrap();
    assert!(body["values"].get("tushare.token").is_none());
}

// ---------- PUT: per-key validation ----------

#[tokio::test]
async fn put_rejects_undeclared_key() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir, finance_like_fields(), None);
    let err = store
        .apply(json!({ "server.host": "x" }))
        .await
        .unwrap_err();
    match err {
        slsdk_rs::ConfigError::Validation(errors) => {
            assert!(errors["server.host"]
                .as_str()
                .unwrap()
                .contains("undeclared"),);
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[tokio::test]
async fn put_rejects_wrong_type() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir, vec![field("retries", ConfigFieldType::Number)], None);
    let err = store
        .apply(json!({ "retries": "three" }))
        .await
        .unwrap_err();
    match err {
        slsdk_rs::ConfigError::Validation(errors) => {
            assert!(errors["retries"]
                .as_str()
                .unwrap()
                .contains("type mismatch"));
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[tokio::test]
async fn put_rejects_datetime_non_string() {
    // datetime validated as an ISO 8601 string in v1 (behavior locked in: non-strings
    // rejected)
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir, vec![field("since", ConfigFieldType::Datetime)], None);
    let err = store.apply(json!({ "since": 42 })).await.unwrap_err();
    assert!(matches!(err, slsdk_rs::ConfigError::Validation(_)));
    store
        .apply(json!({ "since": "2026-01-01T00:00:00+08:00" }))
        .await
        .unwrap();
}

#[tokio::test]
async fn put_select_must_be_in_options() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(
        &dir,
        vec![with_options(
            field("style", ConfigFieldType::Select),
            &["a", "b"],
        )],
        None,
    );
    let err = store.apply(json!({ "style": "c" })).await.unwrap_err();
    match err {
        slsdk_rs::ConfigError::Validation(errors) => {
            assert!(errors["style"].as_str().unwrap().contains("options"));
        }
        other => panic!("expected Validation, got {other:?}"),
    }
    store.apply(json!({ "style": "a" })).await.unwrap();
}

#[tokio::test]
async fn put_cron_format_validation() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(
        &dir,
        vec![with_format(
            field("schedule", ConfigFieldType::String),
            "cron",
        )],
        None,
    );
    // Invalid cron → 400
    let err = store
        .apply(json!({ "schedule": "not-a-cron" }))
        .await
        .unwrap_err();
    match err {
        slsdk_rs::ConfigError::Validation(errors) => {
            assert!(errors["schedule"].as_str().unwrap().contains("cron"));
        }
        other => panic!("expected Validation, got {other:?}"),
    }
    // A valid cron (same example as the sl-finance test) and an empty string (clear
    // schedule semantics) both pass
    store
        .apply(json!({ "schedule": "0 0 3 * * *" }))
        .await
        .unwrap();
    store.apply(json!({ "schedule": "" })).await.unwrap();
}

#[tokio::test]
async fn put_body_must_be_object() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir, finance_like_fields(), None);
    let err = store.apply(json!(42)).await.unwrap_err();
    assert!(matches!(err, slsdk_rs::ConfigError::InvalidUpdates(_)));
}

// ---------- PUT: required merged-state validation ----------

#[tokio::test]
async fn put_required_checked_over_merged_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(
        &dir,
        vec![with_required(field(
            "tushare.token",
            ConfigFieldType::String,
        ))],
        None,
    );
    // No file value + not included in this update → missing in the merged state
    let err = store
        .apply(json!({ "unrelated.key": 1 }))
        .await
        .unwrap_err();
    match err {
        slsdk_rs::ConfigError::Validation(errors) => {
            assert!(errors["tushare.token"]
                .as_str()
                .unwrap()
                .contains("required"),);
        }
        other => panic!("expected Validation, got {other:?}"),
    }
    // Explicitly submitting an empty string = also missing (an empty string counts as
    // missing for strings)
    let err = store
        .apply(json!({ "tushare.token": "" }))
        .await
        .unwrap_err();
    assert!(matches!(err, slsdk_rs::ConfigError::Validation(_)));
}

#[tokio::test]
async fn put_required_satisfied_by_existing_file_value() {
    // File already has a value + this update touches other declared fields → merged state
    // satisfied
    // (partial updates do not require resubmitting all required fields; updates cannot use
    // undeclared keys — per-key validation blocks them first)
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(config_path(&dir), "[tushare]\ntoken = \"seeded\"\n")
        .await
        .unwrap();
    let store = store(&dir, finance_like_fields(), None);
    let body = store
        .apply(json!({ "kline.style": "line" }))
        .await
        .expect("required is satisfied by the existing file value");
    assert_eq!(body["values"]["tushare.token"], "seeded");
    assert_eq!(body["values"]["kline.style"], "line");
}

// ---------- PUT: write-back (preserving unrecognized sections / section creation / return
// shape) ----------

#[tokio::test]
async fn put_writes_back_preserving_unknown_sections() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(
        config_path(&dir),
        "[tushare]\ntoken = \"old\"\n\n[server]\nhost = \"127.0.0.1\"\nport = 8650\n",
    )
    .await
    .unwrap();
    let store = store(
        &dir,
        vec![field("tushare.token", ConfigFieldType::String)],
        None,
    );
    store
        .apply(json!({ "tushare.token": "new-token" }))
        .await
        .unwrap();

    // Re-read the file on disk: the new value took effect and the unrecognized [server]
    // section is preserved as-is
    let text = tokio::fs::read_to_string(config_path(&dir)).await.unwrap();
    let table: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(
        dig(&table, "tushare.token").and_then(|v| v.as_str()),
        Some("new-token")
    );
    assert_eq!(
        dig(&table, "server.host").and_then(|v| v.as_str()),
        Some("127.0.0.1")
    );
    assert_eq!(
        dig(&table, "server.port").and_then(|v| v.as_integer()),
        Some(8650)
    );
}

#[tokio::test]
async fn put_creates_missing_sections() {
    // No [tushare] section in the file: tables are auto-created along the key path (direct
    // coverage of the toml::Value index-assignment pitfall)
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(config_path(&dir), "log_level = \"info\"\n")
        .await
        .unwrap();
    let store = store(
        &dir,
        vec![field("tushare.token", ConfigFieldType::String)],
        None,
    );
    store
        .apply(json!({ "tushare.token": "fresh" }))
        .await
        .unwrap();
    let text = tokio::fs::read_to_string(config_path(&dir)).await.unwrap();
    let table: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(
        dig(&table, "tushare.token").and_then(|v| v.as_str()),
        Some("fresh")
    );
    assert_eq!(
        dig(&table, "log_level").and_then(|v| v.as_str()),
        Some("info")
    );
}

#[tokio::test]
async fn put_returns_updated_merged_body() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir, finance_like_fields(), None);
    let body = store
        .apply(json!({ "tushare.token": "t", "tushare.schedule": "0 0 3 * * *" }))
        .await
        .unwrap();
    assert_eq!(body["values"]["tushare.token"], "t");
    assert_eq!(body["values"]["tushare.schedule"], "0 0 3 * * *");
    assert_eq!(body["values"]["kline.style"], "candlestick"); // defaults returned along with the updated shape
    assert!(body["fields"].as_array().unwrap().len() == 3);
}

// ---------- PUT: on_change hook ----------

#[tokio::test]
async fn put_triggers_on_change_with_written_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    // The callback is a sync context (no await allowed): deliver snapshots via mpsc
    // try_send, received with await on the test side
    let (tx, mut rx) = tokio::sync::mpsc::channel::<toml::Value>(8);
    let store = store(
        &dir,
        vec![field("tushare.token", ConfigFieldType::String)],
        Some(Arc::new(move |snapshot| {
            let _ = tx.try_send(snapshot);
        })),
    );
    store
        .apply(json!({ "tushare.token": "hot" }))
        .await
        .unwrap();
    let snapshot = rx
        .recv()
        .await
        .expect("on_change should deliver the post-write snapshot");
    assert_eq!(
        dig(&snapshot, "tushare.token").and_then(|v| v.as_str()),
        Some("hot")
    );
    // Triggered exactly once: no extra snapshots left in the channel
    assert!(
        rx.try_recv().is_err(),
        "a single PUT should trigger on_change exactly once"
    );
}

#[tokio::test]
async fn on_change_not_triggered_when_validation_fails() {
    // Validation failure means no write-back and no hook fire (zero side effects on the
    // failure path)
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<toml::Value>(8);
    let store = store(
        &dir,
        vec![with_required(field("token", ConfigFieldType::String))],
        Some(Arc::new(move |snapshot| {
            let _ = tx.try_send(snapshot);
        })),
    );
    assert!(store.apply(json!({})).await.is_err());
    assert!(
        rx.try_recv().is_err(),
        "a failed validation should not trigger on_change"
    );
}

// ---------- End to end: config_router (the same function serve_with_config merges
// internally) ----------

async fn spawn_config_server(
    store: Arc<ConfigStore>,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app: Router = Router::new().merge(slsdk_rs::config_router(store));
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, handle)
}

async fn send(
    client: &Client<HttpConnector, axum::body::Body>,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let builder = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => builder
            .header("content-type", "application/json")
            .body(axum::body::Body::from(v.to_string()))
            .unwrap(),
        None => builder.body(axum::body::Body::empty()).unwrap(),
    };
    let res = client.request(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("response body should be JSON")
    };
    (status, json)
}

#[tokio::test]
async fn config_router_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(config_path(&dir), "[tushare]\ntoken = \"seed\"\n")
        .await
        .unwrap();
    let store = Arc::new(ConfigStore::new(
        finance_like_fields(),
        config_path(&dir),
        None,
    ));
    let (addr, server) = spawn_config_server(store).await;
    let client: Client<HttpConnector, axum::body::Body> =
        Client::builder(TokioExecutor::new()).build(HttpConnector::new());

    // GET → 200, fields + flat values (file values win + default fallback)
    let (status, body) = send(&client, "GET", &format!("http://{addr}/config"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["fields"].is_array());
    assert_eq!(body["values"]["tushare.token"], "seed");
    assert_eq!(body["values"]["kline.style"], "candlestick");

    // PUT partial update → 200 with the updated shape (isomorphic to GET)
    let (status, body) = send(
        &client,
        "PUT",
        &format!("http://{addr}/config"),
        Some(json!({ "tushare.token": "updated" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["values"]["tushare.token"], "updated");

    // PUT validation failure → 400 {"errors":{key:reason}}
    let (status, body) = send(
        &client,
        "PUT",
        &format!("http://{addr}/config"),
        Some(json!({ "tushare.schedule": "not-a-cron" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["errors"]["tushare.schedule"].is_string());

    // Methods other than GET/PUT → 405 (rejected automatically by axum routing, coexisting
    // with /health)
    let (status, _) = send(
        &client,
        "POST",
        &format!("http://{addr}/config"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

    server.abort();
}
