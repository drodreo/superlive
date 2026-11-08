//! A minimal long-running component binary for the update-flow e2e test
//! ([`crate::commands`] `perform_update_with_a_live_component_process`):
//! serves the slsdk skeleton (empty business router + the standard /health)
//! until a graceful stop arrives, so the shell's real spawn → supervise →
//! stop → reinstall → restart chain can run against a live process.
//!
//! Identity comes from argv (`test-component <id> <version>`): the shell
//! reads launch.args from the package manifest, so the same binary plays
//! both generations of the update flow (v1 ships args `["livedemo",
//! "0.1.0"]`, v2 ships `["livedemo", "0.2.0"]`) and each generation's
//! /health reports its own version — the process-level proof that the
//! restart actually picked up the new package.
//!
//! Not production code: test infrastructure that happens to live in
//! src/bin/ so plain `cargo test` builds it (the unit test locates it as
//! `target/<profile>/test-component`, a sibling of the test binary's deps
//! directory). The tauri bundler only packages the main binary, so this
//! never ships with the shell.

use slsdk_rs::{ComponentManifest, LaunchConfig, UiDeclaration, UiType};

fn main() {
    let mut args = std::env::args().skip(1);
    let id = args
        .next()
        .unwrap_or_else(|| panic!("usage: test-component <id> <version>"));
    let version = args
        .next()
        .unwrap_or_else(|| panic!("usage: test-component <id> <version>"));

    // Pure-backend shape: ui=None with empty menu/routes satisfies the
    // manifest validation's mount-consistency rule.
    let manifest = ComponentManifest {
        id: id.clone(),
        version,
        min_shell_version: "0.0.0".to_string(),
        namespace: id.clone(),
        display_name: id.clone(),
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
        transport: None,
        update: None,
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test-component runtime");
    rt.block_on(async {
        let info = slsdk_rs::serve(axum::Router::new(), &manifest)
            .await
            .expect("serve must start");
        info.wait().await.expect("graceful serve exit");
    });
}
