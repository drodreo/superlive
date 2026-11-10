//! superlive shell: the componentized platform shell (module 3, Rust core).
//!
//! Design basis: docs/architecture.md — shell = registry + unified API
//! gateway + component lifecycle management.
//! Module structure:
//! - [`config`]: data directory layout and constants (single path source)
//! - [`auth`]: shell-level token generation and /api positive-rule auth
//! - [`registry`]: component runtime registration + manifest disk persistence
//! - [`gateway`]: /api/{ns} prefix-stripping forwarding (streaming + ws pump)
//! - [`lifecycle`]: component process management (spawn/env injection/health
//!   polling/graceful exit)
//! - [`config_plane`]: shell-side component configuration plane (state
//!   machine/launch policy/required comparison/config read-write forwarding)
//! - [`installer`]: install flow (zip extraction + fingerprint verification +
//!   atomic landing)
//! - [`supervisor`]: component recovery at shell startup (started per policy)
//! - [`server`]: embedded HTTP service assembly
//! - [`commands`]: tauri command bridge (UI ↔ core)
//! - [`self_update`]: updater-specific process restart handoff
//!
//! UI containers (module/iframe tiers, webview injection) belong to module 3,
//! second stage.

#[cfg(test)]
mod asset_scope_tests;
pub mod auth;
pub mod commands;
pub mod config;
pub mod config_plane;
pub mod gateway;
pub mod import;
pub mod installer;
pub mod lifecycle;
pub mod registry;
mod self_update;
pub mod server;
pub mod subscriptions;
pub mod supervisor;

use config::{DataLayout, SHELL_LOG_BASENAME};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;

/// Shell global state: shared by tauri manage and axum state (all fields Arc,
/// cheap to clone).
#[derive(Clone)]
pub struct AppState {
    pub(crate) layout: Arc<DataLayout>,
    pub(crate) registry: Arc<registry::Registry>,
    pub(crate) subscriptions: Arc<subscriptions::SubscriptionHub>,
    pub(crate) shell_token: Arc<String>,
    /// Tauri app handle: the shell→UI push channel (component exit broadcast).
    /// `None` in tests (no tauri app exists there); assigned in setup.
    pub(crate) app: Option<tauri::AppHandle>,
}

impl AppState {
    /// Initialize: settle the directory layout (creating it if missing) and
    /// generate the shell-level token.
    /// Call only in a synchronous context (tauri setup / test preparation).
    pub fn new(data_dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let layout = Arc::new(DataLayout::new(data_dir));
        layout.ensure_dirs()?;
        Ok(Self {
            layout,
            registry: Arc::new(registry::Registry::new()),
            subscriptions: Arc::new(subscriptions::SubscriptionHub::new()),
            shell_token: Arc::new(auth::generate_token()),
            app: None,
        })
    }
}

/// Command for the webview to obtain the shell-level token.
/// The UI injection mechanism (module 3, second stage) takes this command as
/// its prerequisite: the frontend calls it once at startup, and every
/// subsequent /api request carries `Authorization: Bearer <token>`.
#[tauri::command]
fn shell_token(state: tauri::State<'_, AppState>) -> String {
    state.shell_token.as_str().to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // The data directory is resolved by tauri per platform convention
            // (Linux: ~/.local/share/<identifier>)
            let data_dir: PathBuf = app.path().app_data_dir()?;
            let layout = DataLayout::new(data_dir.clone());
            layout.ensure_dirs()?;
            // Attach the log sink before the updater handoff so preparation
            // failures and the post-exit launch fallback are diagnosable in
            // {data_root}/logs/shell.log even without a GUI console.
            app.handle().plugin(
                tauri_plugin_log::Builder::new()
                    .targets([
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Folder {
                            path: layout.logs_dir(),
                            file_name: Some(SHELL_LOG_BASENAME.into()),
                        }),
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    ])
                    .level(log::LevelFilter::Info)
                    .timezone_strategy(tauri_plugin_log::TimezoneStrategy::UseLocal)
                    .build(),
            )?;
            match self_update::handoff_updated_shell(app.handle(), &data_dir) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(e) => {
                    log::warn!("updater handoff failed; continuing normal startup: {e}");
                }
            }
            let mut state = AppState::new(data_dir)?;
            // Shell→UI push channel (component exit broadcast); tests keep None
            state.app = Some(app.handle().clone());
            log::info!("shell data directory: {}", state.layout.root().display());

            // Embedded HTTP service: a bind failure does not block shell
            // startup (logged inside server::run)
            let server_state = state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = server::run(server_state).await {
                    log::error!("the shell-embedded service exited: {e}");
                }
            });

            // Restore-and-start installed components
            let supervisor_state = state.clone();
            tauri::async_runtime::spawn(async move {
                supervisor::start_all(&supervisor_state).await;
            });

            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            shell_token,
            commands::list_components,
            commands::install_component,
            commands::install_component_from_url,
            commands::uninstall_component,
            commands::create_component,
            commands::import_component,
            commands::start_component,
            commands::stop_component,
            commands::get_component_config,
            commands::write_component_config,
            commands::get_component_state,
            commands::set_launch_policy,
            commands::check_update,
            commands::perform_update,
            commands::read_component_log,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application");

    // ExitRequested is intercepted in lifecycle: running components are
    // swept (graceful stop + drain wait) before the process actually exits.
    app.run(lifecycle::handle_run_event);
}
