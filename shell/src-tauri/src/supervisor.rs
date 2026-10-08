//! Component recovery at shell startup: scan the on-disk registration,
//! recompute configuration-plane states and start components per their
//! launch policies.
//!
//! Handling policy (module 3 v1 spec + configuration-plane batch rework):
//! - Recompute the state per component (manifest + existing config.toml);
//!   needs-config is persisted to meta in sync and blocked at the start gate
//!   (architecture.md "three lifecycle rules" 2/3);
//! - Only start when launch_policy=auto and config is complete (policy
//!   default = auto, see config_plane);
//!   the manual policy registers without starting, waiting for the user to
//!   start explicitly;
//! - A single component's start failure is only logged and skipped with the
//!   settled state written back, without dragging down the rest;
//! - A component crashing and exiting is handled by supervise (log + registry
//!   removal + settled-state persistence); no automatic restart.

use crate::config::HEALTH_TIMEOUT;
use crate::config_plane::{self, ComponentMeta, ComponentState};
use crate::lifecycle;
use crate::AppState;
use slsdk_rs::ComponentManifest;
use std::sync::Arc;

/// Scan all manifests in registry/, recompute states and restore-start per
/// policy (serial: a low-frequency path, keeping log ordering clear).
pub async fn start_all(state: &AppState) {
    let manifests = match crate::registry::load_all_manifests(&state.layout).await {
        Ok(manifests) => manifests,
        Err(source) => {
            log::error!(
                "failed to scan the component registry directory ({}): {source}",
                state.layout.registry_dir().display()
            );
            return;
        }
    };
    if manifests.is_empty() {
        log::info!("no installed components");
        return;
    }
    for manifest in manifests {
        let config_path = state.layout.component_config_file(&manifest.id);
        // Recompute the state: out-of-shell config.toml changes / orphan meta
        // both defer to the scene
        let computed = match config_plane::compute_state(&manifest, &config_path).await {
            Ok(state) => state,
            Err(e) => {
                log::error!(
                    "failed to determine the config state for component {}, skipping: {e}",
                    manifest.id
                );
                continue;
            }
        };
        let launch_policy = config_plane::load_meta(&state.layout, &manifest.id)
            .await
            .map(|m| m.launch_policy)
            .unwrap_or_else(config_plane::default_launch_policy);
        if let Err(e) = config_plane::save_meta(
            &state.layout,
            &manifest.id,
            &ComponentMeta {
                state: computed,
                launch_policy,
            },
        )
        .await
        {
            log::warn!(
                "failed to persist the state for component {} (continuing startup): {e}",
                manifest.id
            );
        }
        if computed == ComponentState::NeedsConfig {
            log::info!(
                "component {} is in needs-config (missing required config), blocked at the startup gate",
                manifest.id
            );
            continue;
        }
        if launch_policy != config_plane::LaunchPolicy::Auto {
            log::info!(
                "component {} has a manual start policy, waiting for an explicit user start",
                manifest.id
            );
            continue;
        }
        log::info!(
            "restarting component {} (ns={} v={})",
            manifest.id,
            manifest.namespace,
            manifest.version
        );
        launch(state, Arc::new(manifest)).await;
    }
}

/// Single-component start and state-transition loop: starting → (success)
/// running / (failure) settled state written back.
/// Shared by three call sites: the supervisor's restore-start,
/// start_component, and effective-config restarts.
pub(crate) async fn launch_with_transition(
    state: &AppState,
    manifest: Arc<ComponentManifest>,
) -> Result<(), lifecycle::LifecycleError> {
    let id = manifest.id.clone();
    let policy = config_plane::load_meta(&state.layout, &id)
        .await
        .map(|m| m.launch_policy)
        .unwrap_or_else(config_plane::default_launch_policy);
    if let Err(e) = config_plane::save_meta(
        &state.layout,
        &id,
        &ComponentMeta {
            state: ComponentState::Starting,
            launch_policy: policy,
        },
    )
    .await
    {
        log::warn!("failed to persist the starting state for component {id} (continuing): {e}");
    }
    let result =
        lifecycle::spawn_component(&state.layout, &state.registry, manifest, HEALTH_TIMEOUT).await;
    match &result {
        Ok(()) => {
            if let Err(e) = config_plane::save_meta(
                &state.layout,
                &id,
                &ComponentMeta {
                    state: ComponentState::Running,
                    launch_policy: policy,
                },
            )
            .await
            {
                log::warn!("failed to persist the running state for component {id}: {e}");
            }
        }
        Err(e) => {
            // Failure writes back the settled state (ready: the caller has
            // necessarily passed the needs-config gate)
            log::error!("failed to spawn component {id}: {e}");
            if let Err(e) = config_plane::save_meta(
                &state.layout,
                &id,
                &ComponentMeta {
                    state: ComponentState::Ready,
                    launch_policy: policy,
                },
            )
            .await
            {
                log::warn!("failed to persist the failure state for component {id}: {e}");
            }
        }
    }
    result
}

async fn launch(state: &AppState, manifest: Arc<ComponentManifest>) {
    if let Err(e) = launch_with_transition(state, manifest).await {
        log::error!("component spawn failed, skipping: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry;
    use slsdk_rs::ComponentManifest;
    use std::time::Duration;

    /// Hand-build an on-disk registration (minimal registration form that
    /// bypasses the install flow).
    fn manifest_with_command(id: &str, ns: &str, command: &str) -> ComponentManifest {
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
                "launch": {{ "command": "{command}", "args": [] }},
                "files": [ {{ "path": "bin", "sha256": "aa" }} ]
            }}"#
        ))
        .unwrap()
    }

    /// Registration form with a required config field (for needs-config gate
    /// tests).
    fn manifest_with_required(id: &str, ns: &str, command: &str) -> ComponentManifest {
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
                "launch": {{ "command": "{command}", "args": [] }},
                "files": [ {{ "path": "bin", "sha256": "aa" }} ],
                "config": {{ "fields": [ {{ "key": "tok", "label": "tok", "type": "string", "required": true }} ] }}
            }}"#
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn no_installed_components_is_noop() {
        // No registry directory / no registration: returns normally, no panic
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), start_all(&state))
            .await
            .expect("an empty registry scan should return immediately");
    }

    #[tokio::test]
    async fn failed_spawn_is_skipped_and_falls_back_to_ready() {
        // Single-component spawn failure (command missing): skip and continue,
        // without dragging down the scan flow;
        // state-transition loop: starting → failure writes back ready
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path()).unwrap();
        state.layout.ensure_dirs().unwrap();
        std::fs::create_dir_all(state.layout.component_dir("badapp")).unwrap();
        let layout = state.layout.clone();
        let bad = manifest_with_command("badapp", "badns", "/nonexistent/binary");
        // persist_manifest_sync is synchronous IO and the doc forbids calling
        // it directly from async — the test side obeys the same rule
        tokio::task::spawn_blocking(move || registry::persist_manifest_sync(&layout, &bad))
            .await
            .unwrap()
            .unwrap();

        tokio::time::timeout(Duration::from_secs(20), start_all(&state))
            .await
            .expect("a failing component should be skipped and the scan should end normally");
        // Not registered (spawn failed outright; must not appear in the
        // running table)
        assert!(state.registry.lookup("badns").await.is_none());
        // State written back to ready (default policy auto)
        let meta = config_plane::load_meta(&state.layout, "badapp")
            .await
            .expect("meta should be written back after a failure");
        assert_eq!(meta.state, ComponentState::Ready);
        assert_eq!(meta.launch_policy, config_plane::LaunchPolicy::Auto);
    }

    #[tokio::test]
    async fn manual_policy_skips_launch() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path()).unwrap();
        state.layout.ensure_dirs().unwrap();
        std::fs::create_dir_all(state.layout.component_dir("manual")).unwrap();
        let layout = state.layout.clone();
        let m = manifest_with_command("manual", "manualns", "/bin/sleep");
        tokio::task::spawn_blocking(move || registry::persist_manifest_sync(&layout, &m))
            .await
            .unwrap()
            .unwrap();
        config_plane::save_meta(
            &state.layout,
            "manual",
            &ComponentMeta {
                state: ComponentState::Stopped,
                launch_policy: config_plane::LaunchPolicy::Manual,
            },
        )
        .await
        .unwrap();

        tokio::time::timeout(Duration::from_secs(10), start_all(&state))
            .await
            .expect("a manual policy should skip spawning and the scan should end immediately");
        // Not started (the gate is before spawn; must not appear in the
        // running table)
        assert!(state.registry.lookup("manualns").await.is_none());
        // Policy kept, state keeps the recomputed value (no config section →
        // ready)
        let meta = config_plane::load_meta(&state.layout, "manual")
            .await
            .unwrap();
        assert_eq!(meta.state, ComponentState::Ready);
        assert_eq!(meta.launch_policy, config_plane::LaunchPolicy::Manual);
    }

    #[tokio::test]
    async fn needs_config_blocks_auto_launch() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path()).unwrap();
        state.layout.ensure_dirs().unwrap();
        std::fs::create_dir_all(state.layout.component_dir("gated")).unwrap();
        let layout = state.layout.clone();
        // If /bin/sleep were started by mistake it would wait for the health
        // timeout (far beyond the test budget) — a test timeout is the signal
        // that the gate failed
        let m = manifest_with_required("gated", "gatedns", "/bin/sleep");
        tokio::task::spawn_blocking(move || registry::persist_manifest_sync(&layout, &m))
            .await
            .unwrap()
            .unwrap();

        tokio::time::timeout(Duration::from_secs(10), start_all(&state))
            .await
            .expect("needs-config should block before spawn and the scan should end immediately");
        assert!(state.registry.lookup("gatedns").await.is_none());
        let meta = config_plane::load_meta(&state.layout, "gated")
            .await
            .expect("needs-config should be persisted");
        assert_eq!(meta.state, ComponentState::NeedsConfig);
    }
}
