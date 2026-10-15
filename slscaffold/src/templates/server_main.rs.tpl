//! __NAME__ component backend.
//!
//! The slsdk-rs serve() skeleton handles everything infrastructural: listening
//! (SL_ENDPOINT — tcp:/uds: injected by the shell), token auth (SL_TOKEN, the first
//! wall), the /health endpoint (readiness probe + health check share it) and SIGTERM
//! graceful exit. We only write business handlers.
//!
//! Without SL_ENDPOINT the process runs in standalone debug mode (tcp:127.0.0.1:0,
//! no auth) — see README.md for the fixed-port recipe.

use axum::response::IntoResponse;
use axum::routing::get;
use axum::Json;
use slsdk_rs::{ComponentManifest, LaunchConfig, UiDeclaration, UiType};

/// Single source of identity: the packaging script greps this constant to keep the
/// manifest and the server self-report in sync (the shell matches /health responses
/// against manifest.id).
const COMPONENT_ID: &str = "__ID__";

/// The manifest this process reports for /health: serve() only consumes id/version
/// from it (the installed manifest is kept by the shell); the rest are placeholders.
/// files is empty — that would not pass manifest validation, but serve() never
/// validates (runtime safety, see slsdk-rs serve.rs).
fn component_manifest() -> ComponentManifest {
    ComponentManifest {
        id: COMPONENT_ID.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        min_shell_version: "0.0.0".to_string(),
        namespace: "__NS__".to_string(),
        display_name: "__NAME__".to_string(),
        ui: Some(UiDeclaration {
            ui_type: UiType::Module,
            entry: String::new(),
        }),
        menu: None,
        routes: Vec::new(),
        launch: Some(LaunchConfig {
            command: String::new(),
            args: Vec::new(),
        }),
        files: Vec::new(),
        config: None,
        // serve() never consumes the transport declaration (identity only):
        // absent = every transport the host platform offers.
        transport: None,
    }
}

/// Sample business handler. /health is reserved (mounted by the SDK) — author routes
/// must not register it (axum panics on overlapping routes).
async fn hello() -> impl IntoResponse {
    Json(serde_json::json!({ "message": "hello from __ID__" }))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let app = axum::Router::new().route("/api/v1/hello", get(hello));
    let manifest = component_manifest();

    let info = slsdk_rs::serve(app, &manifest).await?;
    log::info!("__NAME__ component server listening on {}", info.endpoint);

    // Wait for the accept/serve loop to end (returns after graceful exit).
    info.wait().await?;
    Ok(())
}
