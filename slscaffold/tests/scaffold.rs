//! Integration tests over the public scaffold API: shape-dependent file trees,
//! placeholder residue, error paths, and the single-file build-script entry.

use slscaffold::{
    BackendTemplate, ComponentShape, LocalizedTitle, MissingFile, ScaffoldError, ScaffoldRequest,
    SdkDependency,
};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Placeholders this crate substitutes (name list mirrored from the renderer).
const PLACEHOLDERS: [&str; 8] = [
    "ID",
    "NAME",
    "NS",
    "DEV_PORT",
    "SLSDK_RS_LINE",
    "SLSDK_TS_LINE",
    "DEP_MODE_DESC",
    "TITLE_ROWS",
];

fn request(target: PathBuf, shape: ComponentShape) -> ScaffoldRequest {
    request_with_template(target, shape, BackendTemplate::Cargo)
}

fn request_with_template(
    target: PathBuf,
    shape: ComponentShape,
    template: BackendTemplate,
) -> ScaffoldRequest {
    ScaffoldRequest {
        id: "demo-comp".to_string(),
        display_name: "Demo Comp".to_string(),
        display_titles: vec![
            LocalizedTitle {
                locale: "zh".to_string(),
                value: "演示组件".to_string(),
            },
            LocalizedTitle {
                locale: "en".to_string(),
                value: "Demo's Comp".to_string(),
            },
            LocalizedTitle {
                locale: "ja".to_string(),
                value: "デモのコンポーネント".to_string(),
            },
        ],
        namespace: "demo".to_string(),
        target_dir: target,
        shape,
        template,
        dev_port: 8700,
        sdk: SdkDependency::Registry,
    }
}

fn scaffold_into(dir: &TempDir, shape: ComponentShape) -> slscaffold::ScaffoldResult {
    scaffold_with_template(dir, shape, BackendTemplate::Cargo)
}

fn scaffold_with_template(
    dir: &TempDir,
    shape: ComponentShape,
    template: BackendTemplate,
) -> slscaffold::ScaffoldResult {
    let target = dir.path().join("proj");
    slscaffold::scaffold(&request_with_template(target, shape, template))
        .expect("scaffold must succeed")
}

fn read_all(root: &Path, created: &[PathBuf]) -> Vec<(String, String)> {
    created
        .iter()
        .map(|path| {
            let rel = path
                .strip_prefix(root)
                .expect("created paths live under root")
                .to_string_lossy()
                .to_string();
            let content = fs::read_to_string(path).expect("created file is readable");
            (rel, content)
        })
        .collect()
}

fn has_file(created: &[PathBuf], suffix: &str) -> bool {
    created
        .iter()
        .any(|path| path.to_string_lossy().ends_with(suffix))
}

// ---- shape-dependent file trees ----

#[test]
fn frontend_and_backend_generates_the_full_tree() {
    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::FrontendAndBackend);
    assert_eq!(result.created.len(), 12);
    for expected in [
        "README.md",
        "server/Cargo.toml",
        "server/src/main.rs",
        "web/package.json",
        "web/tsconfig.json",
        "web/vite.config.js",
        "web/index.html",
        "web/src/main.tsx",
        "web/src/entry.tsx",
        "web/src/App.tsx",
        "web/src/i18n.ts",
        "scripts/superlive.build",
    ] {
        assert!(has_file(&result.created, expected), "missing {expected}");
    }
    assert!(result.root.join("server/Cargo.toml").is_file());
    assert!(result.root.join("web/src/App.tsx").is_file());
}

#[test]
fn frontend_only_omits_the_server_plane() {
    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::FrontendOnly);
    assert_eq!(result.created.len(), 10);
    assert!(!has_file(&result.created, "server/"));
    assert!(!result.root.join("server").exists());
    assert!(result.root.join("web/src/App.tsx").is_file());
}

#[test]
fn backend_only_omits_the_web_plane() {
    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::BackendOnly);
    assert_eq!(result.created.len(), 4);
    assert!(!has_file(&result.created, "web/"));
    assert!(!result.root.join("web").exists());
    assert!(result.root.join("server/src/main.rs").is_file());
}

// ---- App sample differs by shape ----

// ---- backend template variants ----

#[test]
fn ts_templates_generate_the_ts_server_tree() {
    for template in [BackendTemplate::Deno, BackendTemplate::Bun] {
        let dir = TempDir::new().unwrap();
        let result = scaffold_with_template(&dir, ComponentShape::FrontendAndBackend, template);
        // The TS server plane is an entry file plus its package.json
        // dependency root (no Cargo.toml / main.rs).
        assert_eq!(result.created.len(), 12);
        assert!(has_file(&result.created, "server/server.ts"));
        assert!(has_file(&result.created, "server/package.json"));
        assert!(!has_file(&result.created, "server/Cargo.toml"));
        assert!(!has_file(&result.created, "server/src/main.rs"));
        // The web plane is untouched by the backend template.
        assert!(result.root.join("web/src/App.tsx").is_file());

        // The dependency root resolves slsdk-ts through the same two modes as
        // the web plane's manifest, and carries the pnpm build allowlist so a
        // fresh `pnpm install` does not die on esbuild's postinstall gate.
        let package = fs::read_to_string(result.root.join("server/package.json")).unwrap();
        assert!(package.contains(r#""name": "demo-comp-server""#));
        assert!(package.contains(r#""private": true"#));
        assert!(package.contains(r#""slsdk-ts": "^0.2.0""#));
        assert!(package.contains(r#""onlyBuiltDependencies": ["#));
        assert!(package.contains("esbuild"));
    }
}

#[test]
fn ts_server_entry_carries_identity_and_sdk_contract() {
    for template in [BackendTemplate::Deno, BackendTemplate::Bun] {
        let dir = TempDir::new().unwrap();
        let result = scaffold_with_template(&dir, ComponentShape::BackendOnly, template);
        let server = fs::read_to_string(result.root.join("server/server.ts")).unwrap();
        assert!(server.contains(
            r#"import { serve, ENV_ENDPOINT, type ServerManifest } from "slsdk-ts/server";"#
        ));
        assert!(server.contains(r#"const COMPONENT_ID = "demo-comp";"#));
        assert!(server.contains("const COMPONENT_VERSION"));
        assert!(server.contains(r#"namespace: "demo""#));
        // The ServerManifest literal carries every required field of the
        // slsdk-ts contract (regression: a missing field is a generated-
        // project type error the tree checks never saw).
        assert!(server.contains(r#"min_shell_version: "0.0.0""#));
        assert!(server.contains(r#"display_name: "Demo Comp""#));
        // Call order is the contract: handler first, manifest second (the
        // slsdk-ts signature mirrors slsdk-rs's serve(app, manifest)) — the
        // flipped order once passed the tree checks and broke only at runtime.
        assert!(
            server.contains("serve(routes, manifest);"),
            "the serve call must pass the handler first"
        );
        assert!(
            !server.contains("serve(manifest"),
            "flipped serve argument order"
        );
        assert!(server.contains("/api/v1/hello"));
        // Zero placeholder residue.
        for token in PLACEHOLDERS {
            assert!(
                !server.contains(&format!("__{token}__")),
                "{template:?} carries {token}"
            );
        }
    }
}

#[test]
fn readme_documents_the_runtime_per_template() {
    let dir = TempDir::new().unwrap();
    let result = scaffold_with_template(
        &dir,
        ComponentShape::FrontendAndBackend,
        BackendTemplate::Deno,
    );
    let readme = fs::read_to_string(result.root.join("README.md")).unwrap();
    assert!(readme.contains("Deno backend (`server/server.ts`"));
    // The deno template declares tcp-only transport in its manifest: the
    // README must tell the author why.
    assert!(readme.contains("tcp-only"));
    assert!(readme.contains("- Deno (https://deno.com)"));
    assert!(
        readme.contains("deno run --node-modules-dir=manual --allow-net --allow-read --allow-env server/server.ts"),
        "the deno run recipe must carry the manual node_modules flag"
    );
    assert!(
        readme.contains("bun install"),
        "the deno recipe must name the node_modules pre-install step"
    );
    assert!(!readme.contains("Rust toolchain"));

    let dir = TempDir::new().unwrap();
    let result = scaffold_with_template(
        &dir,
        ComponentShape::FrontendAndBackend,
        BackendTemplate::Bun,
    );
    let readme = fs::read_to_string(result.root.join("README.md")).unwrap();
    assert!(readme.contains("Bun backend (`server/server.ts`"));
    assert!(readme.contains("- Bun (https://bun.sh)"));
    assert!(readme.contains("bun run server/server.ts"));
    assert!(!readme.contains("tcp-only"));

    // The cargo template keeps the original README shape.
    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::FrontendAndBackend);
    let readme = fs::read_to_string(result.root.join("README.md")).unwrap();
    assert!(readme.contains("Rust toolchain"));
    assert!(readme.contains("cargo run --manifest-path server/Cargo.toml"));
    assert!(!readme.contains("tcp-only"));
}

#[test]
fn ts_server_package_json_follows_the_sdk_dependency_mode() {
    // dev-path mode: the dependency points at the local checkout, the same
    // rendering the web plane's manifest gets from the shared placeholder.
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("proj");
    let superlive_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("slscaffold lives inside the superlive checkout")
        .to_path_buf();
    let request = ScaffoldRequest {
        sdk: SdkDependency::Local {
            superlive_root: superlive_root.clone(),
        },
        ..request_with_template(target, ComponentShape::BackendOnly, BackendTemplate::Deno)
    };
    let result = slscaffold::scaffold(&request).expect("local-mode scaffold must succeed");
    let package = fs::read_to_string(result.root.join("server/package.json")).unwrap();
    // The `file:{path}` (single-slash) form is the existing rendering the web
    // plane's manifest uses — this plane mirrors it byte for byte.
    let expected = format!(
        r#""slsdk-ts": "file:{}/slsdk-ts""#,
        superlive_root.display()
    );
    assert!(
        package.contains(&expected),
        "local-mode line missing: {package}"
    );
}

#[test]
fn scaffold_creates_a_missing_parent_chain() {
    // Regression: a target whose parent does not exist used to surface as a
    // bare Io NotFound (the scaffold owns the tree it creates, parents
    // included).
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("a").join("b").join("proj");
    let result = slscaffold::scaffold(&request(target, ComponentShape::BackendOnly))
        .expect("missing parents are created, not an error");
    assert!(result.root.join("server/src/main.rs").is_file());
}

// ---- the generated cargo project must compile ----
//
// Regression: the template checks verify tree shape and placeholder residue
// but never compile the generated code, so a manifest-shape drift between
// slsdk-rs and the template (Option fields, new struct fields) surfaced only
// at the user's first build. This test is the compile gate.

#[test]
fn generated_cargo_project_compiles() {
    // Local SDK deps: the generated project compiles against this checkout
    // (registry versions depend on a crates.io release that lags the repo).
    // The independent CARGO_TARGET_DIR keeps this crate's target untouched.
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("proj");
    let superlive_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("slscaffold lives inside the superlive checkout")
        .to_path_buf();
    let request = ScaffoldRequest {
        sdk: SdkDependency::Local {
            superlive_root: superlive_root.clone(),
        },
        ..request(target, ComponentShape::BackendOnly)
    };
    let result = slscaffold::scaffold(&request).expect(
        "local-mode scaffold must succeed (build slsdk-ts first: cd slsdk-ts && pnpm install && pnpm build)",
    );

    let output = std::process::Command::new("cargo")
        .args(["check", "--manifest-path"])
        .arg(result.root.join("server").join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", dir.path().join("cargo-target"))
        .output()
        .expect("cargo must be on PATH for the compile check");
    assert!(
        output.status.success(),
        "the generated cargo project must compile:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn next_steps_follow_the_backend_template() {
    let cases = [
        (BackendTemplate::Cargo, "cargo run --manifest-path"),
        (
            BackendTemplate::Deno,
            "deno run --node-modules-dir=manual --allow-net --allow-read --allow-env",
        ),
        (BackendTemplate::Bun, "bun run"),
    ];
    for (template, needle) in cases {
        let dir = TempDir::new().unwrap();
        let result = scaffold_with_template(&dir, ComponentShape::BackendOnly, template);
        let backend_step = result
            .next_steps
            .iter()
            .find(|step| step.starts_with("backend :") && step.contains("SL_ENDPOINT"))
            .expect("backend-only next_steps carry the backend line");
        assert!(
            backend_step.contains(needle),
            "{template:?} next_steps: {backend_step}"
        );
        if template == BackendTemplate::Deno {
            // The pre-install line precedes the dev line: deno cannot resolve
            // file: deps without a pre-built node_modules (bun/npm).
            let dev_index = result
                .next_steps
                .iter()
                .position(|step| step.contains("SL_ENDPOINT"))
                .expect("the backend dev line exists");
            assert!(
                dev_index > 0 && result.next_steps[dev_index - 1].contains("bun install"),
                "the deno pre-install line must precede the dev line: {:?}",
                result.next_steps
            );
        }
    }
}

#[test]
fn app_sample_api_chain_follows_the_shape() {
    // Server shapes carry the API sample (client + hello route + ns prefix);
    // frontend-only drops it entirely — there is no backend to call, a sample
    // request would just render a guaranteed error.
    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::FrontendAndBackend);
    let app = fs::read_to_string(result.root.join("web/src/App.tsx")).unwrap();
    assert!(app.contains("createClient"));
    assert!(app.contains("/api/v1/hello"));
    assert!(app.contains("NAMESPACE_PREFIX"));

    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::FrontendOnly);
    let app = fs::read_to_string(result.root.join("web/src/App.tsx")).unwrap();
    assert!(!app.contains("createClient"));
    assert!(!app.contains("/api/v1/hello"));
    assert!(!app.contains("NAMESPACE_PREFIX"));
    assert!(app.contains("no backend process"));
    assert!(app.contains("useShellState"));
}

// ---- placeholder residue ----

#[test]
fn generated_files_have_zero_placeholder_residue_in_all_shapes() {
    for shape in [
        ComponentShape::FrontendAndBackend,
        ComponentShape::FrontendOnly,
        ComponentShape::BackendOnly,
    ] {
        let dir = TempDir::new().unwrap();
        let result = scaffold_into(&dir, shape);
        for (rel, content) in read_all(&result.root, &result.created) {
            for name in PLACEHOLDERS {
                let token = format!("__{name}__");
                assert!(
                    !content.contains(&token),
                    "{shape:?} {rel} still contains {token}"
                );
            }
            // window.__SL_SHELL__ is legitimate runtime code (the shell-injection
            // global), not a placeholder — it must survive substitution untouched.
            // main.tsx fakes the injection in every web shape; App.tsx references
            // the global directly only in the server shapes (the frontend-only
            // sample has no API call and thus no direct reference).
            if rel.ends_with("main.tsx") {
                assert!(
                    content.contains("__SL_SHELL__"),
                    "{rel} lost the __SL_SHELL__ runtime global"
                );
            }
            if rel.ends_with("App.tsx") && shape != ComponentShape::FrontendOnly {
                assert!(
                    content.contains("__SL_SHELL__"),
                    "{rel} lost the __SL_SHELL__ runtime global"
                );
            }
        }
    }
}

// ---- web i18n module ----

#[test]
fn web_i18n_registers_every_configured_locale_row_and_the_namespace_note() {
    // The generated i18n module carries one registration line per configured
    // locale (input order, values verbatim with single quotes escaped) and
    // states the shell-managed `menuTitle` constraint (visible to the author
    // and to any AI assistant working on the generated project)
    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::FrontendAndBackend);
    let i18n = fs::read_to_string(result.root.join("web/src/i18n.ts")).unwrap();
    assert!(
        i18n.contains("zh: { title: '演示组件' },"),
        "zh title row missing: {i18n}"
    );
    assert!(
        i18n.contains("en: { title: 'Demo\\'s Comp' },"),
        "en title row missing (or single quotes not escaped): {i18n}"
    );
    assert!(
        i18n.contains("ja: { title: 'デモのコンポーネント' },"),
        "ja title row missing: {i18n}"
    );
    // Rows keep the request's input order (zh → en → ja)
    let zh_row = i18n.find("zh: { title:").unwrap();
    let en_row = i18n.find("en: { title:").unwrap();
    let ja_row = i18n.find("ja: { title:").unwrap();
    assert!(
        zh_row < en_row && en_row < ja_row,
        "row order drifted: {i18n}"
    );
    assert!(i18n.contains("menuTitle"), "constraint note missing");

    // Backend-only has no web plane, so no i18n module at all (tree shape unchanged)
    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::BackendOnly);
    assert!(!has_file(&result.created, "web/src/i18n.ts"));
}

#[test]
fn web_i18n_emits_a_single_row_for_a_single_locale() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("proj");
    let mut req = request(target, ComponentShape::FrontendOnly);
    req.display_titles = vec![LocalizedTitle {
        locale: "zh".to_string(),
        value: "只有中文".to_string(),
    }];
    slscaffold::scaffold(&req).expect("scaffold must succeed");
    let i18n = fs::read_to_string(req.target_dir.join("web/src/i18n.ts")).unwrap();
    assert!(
        i18n.contains("zh: { title: '只有中文' },"),
        "zh title row missing: {i18n}"
    );
    assert!(!i18n.contains("en: { title:"), "stale en row: {i18n}");
    assert!(!i18n.contains("ja: { title:"), "stale ja row: {i18n}");
}

#[test]
fn scaffold_rejects_invalid_display_titles() {
    let build = |titles: Vec<LocalizedTitle>| {
        let dir = TempDir::new().unwrap();
        let mut req = request(dir.path().join("proj"), ComponentShape::FrontendOnly);
        req.display_titles = titles;
        slscaffold::scaffold(&req).err()
    };

    let err = build(Vec::new()).expect("empty set must be rejected");
    assert!(
        matches!(err, ScaffoldError::InvalidDisplayTitles(_)),
        "{err}"
    );

    let err = build(vec![LocalizedTitle {
        locale: "zh".to_string(),
        value: "  ".to_string(),
    }])
    .expect("empty value must be rejected");
    assert!(
        matches!(err, ScaffoldError::InvalidDisplayTitles(_)),
        "{err}"
    );

    let err = build(vec![
        LocalizedTitle {
            locale: "zh".to_string(),
            value: "一".to_string(),
        },
        LocalizedTitle {
            locale: "zh".to_string(),
            value: "二".to_string(),
        },
    ])
    .expect("duplicate locales must be rejected");
    assert!(
        matches!(err, ScaffoldError::InvalidDisplayTitles(_)),
        "{err}"
    );
}

// ---- validation error paths ----

#[test]
fn invalid_id_and_namespace_are_rejected() {
    let dir = TempDir::new().unwrap();
    let mut req = request(dir.path().join("a"), ComponentShape::BackendOnly);
    req.id = "bad_id".to_string();
    assert!(matches!(
        slscaffold::scaffold(&req),
        Err(ScaffoldError::InvalidId(_))
    ));

    req.id = "ok".to_string();
    req.namespace = String::new();
    assert!(matches!(
        slscaffold::scaffold(&req),
        Err(ScaffoldError::InvalidNamespace(_))
    ));
}

#[test]
fn zero_dev_port_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut req = request(dir.path().join("a"), ComponentShape::BackendOnly);
    req.dev_port = 0;
    assert!(matches!(
        slscaffold::scaffold(&req),
        Err(ScaffoldError::InvalidDevPort(0))
    ));
}

#[test]
fn nonempty_target_is_rejected_and_empty_target_is_accepted() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("proj");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("stale.txt"), "x").unwrap();
    assert!(matches!(
        slscaffold::scaffold(&request(target.clone(), ComponentShape::BackendOnly)),
        Err(ScaffoldError::TargetExists { .. })
    ));

    fs::remove_file(target.join("stale.txt")).unwrap();
    let result = slscaffold::scaffold(&request(target, ComponentShape::BackendOnly))
        .expect("empty existing target must be accepted");
    assert!(result.root.join("README.md").is_file());
}

// ---- build script (single file; the on-disk dialect follows the host platform) ----

#[test]
#[cfg(unix)]
fn build_script_is_the_bash_dialect_and_follows_the_shape() {
    let render_build = |shape| {
        let dir = TempDir::new().unwrap();
        let result = scaffold_into(&dir, shape);
        fs::read_to_string(result.root.join("scripts/superlive.build")).unwrap()
    };

    let full = render_build(ComponentShape::FrontendAndBackend);
    let frontend = render_build(ComponentShape::FrontendOnly);
    let backend = render_build(ComponentShape::BackendOnly);

    // POSIX hosts get the bash dialect in the single build file.
    for script in [&full, &frontend, &backend] {
        assert!(
            script.starts_with("#!/usr/bin/env bash"),
            "unix host must render the bash dialect"
        );
    }

    // FrontendOnly: no server plane anywhere, web build intact.
    assert!(!frontend.contains("cargo"));
    assert!(frontend.contains("\"$PM\" run build"));
    assert!(frontend.contains("web/dist/entry.js"));

    // BackendOnly: no web plane anywhere, server build intact.
    assert!(!backend.contains("\"$PM\" run build"));
    assert!(!backend.contains("pnpm"));
    assert!(backend.contains("cargo build --release"));
    assert!(backend.contains("target/release/demo-comp-server"));

    // FrontendAndBackend: both planes with runtime directory gates.
    assert!(full.contains("if [ -d \"$ROOT/server\" ]"));
    assert!(full.contains("if [ -d \"$ROOT/web\" ]"));
    assert!(full.contains("target/release/demo-comp-server"));
    assert!(full.contains("web/dist/entry.js"));

    // Packaging semantics moved into the shell — anti-residue guard over
    // code-level markers (the header prose may describe the shell's duties).
    for script in [&full, &frontend, &backend] {
        for banned in ["zip", "manifest", "staging", "sha256"] {
            assert!(!script.contains(banned), "banned packaging token: {banned}");
        }
    }
}

#[test]
fn readme_differs_by_shape() {
    let render_readme = |shape| {
        let dir = TempDir::new().unwrap();
        let result = scaffold_into(&dir, shape);
        fs::read_to_string(result.root.join("README.md")).unwrap()
    };

    let frontend = render_readme(ComponentShape::FrontendOnly);
    let backend = render_readme(ComponentShape::BackendOnly);
    let full = render_readme(ComponentShape::FrontendAndBackend);

    assert!(!frontend.contains("Rust toolchain"));
    assert!(frontend.contains("Node.js + pnpm"));
    assert!(backend.contains("Rust toolchain"));
    assert!(!backend.contains("Node.js + pnpm"));
    // Both single-plane shapes carry their own dev recipe; the combined shape
    // keeps the two-terminal recipe.
    assert!(full.contains("Terminal 1 — backend"));
    assert!(frontend.contains("This shape has no backend"));
    assert!(backend.contains("cargo run --manifest-path server/Cargo.toml"));
    // The packaging script is gone; the single build entry replaced it.
    assert!(full.contains("scripts/superlive.build"));
    assert!(!full.contains("package-component"));
}

// ---- SDK dependency modes ----

#[test]
fn registry_dependencies_use_version_specs() {
    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::FrontendAndBackend);
    let cargo = fs::read_to_string(result.root.join("server/Cargo.toml")).unwrap();
    let package = fs::read_to_string(result.root.join("web/package.json")).unwrap();
    assert!(cargo.contains("slsdk-rs = \"0.2\""));
    assert!(package.contains("\"slsdk-ts\": \"^0.2.0\""));
}

#[test]
fn local_sdk_uses_forward_slash_path_and_file_lines() {
    let dir = TempDir::new().unwrap();
    let checkout = dir.path().join("superlive");
    fs::create_dir_all(checkout.join("slsdk-rs")).unwrap();
    fs::create_dir_all(checkout.join("slsdk-ts/dist")).unwrap();
    fs::write(checkout.join("slsdk-ts/dist/index.js"), "export {};").unwrap();

    let target = dir.path().join("proj");
    let mut req = request(target, ComponentShape::FrontendAndBackend);
    req.sdk = SdkDependency::Local {
        superlive_root: checkout.clone(),
    };
    let result = slscaffold::scaffold(&req).expect("local sdk scaffold must succeed");

    let cargo = fs::read_to_string(result.root.join("server/Cargo.toml")).unwrap();
    let package = fs::read_to_string(result.root.join("web/package.json")).unwrap();
    let expected_rs = format!(
        "slsdk-rs = {{ path = \"{}\" }}",
        checkout.join("slsdk-rs").display()
    );
    let expected_ts = format!(
        "\"slsdk-ts\": \"file:{}\"",
        checkout.join("slsdk-ts").display()
    );
    assert!(cargo.contains(&expected_rs), "cargo line: {cargo}");
    assert!(package.contains(&expected_ts), "package line: {package}");
    // Forward slashes on every platform (no backslash separators anywhere).
    assert!(
        !cargo.contains('\\'),
        "cargo dependency line must use / only"
    );
    assert!(
        !package.contains('\\'),
        "package dependency line must use / only"
    );
}

#[test]
fn local_sdk_without_built_dist_is_rejected() {
    let dir = TempDir::new().unwrap();
    let checkout = dir.path().join("superlive");
    fs::create_dir_all(checkout.join("slsdk-rs")).unwrap();
    fs::create_dir_all(checkout.join("slsdk-ts")).unwrap();

    let mut req = request(dir.path().join("proj"), ComponentShape::BackendOnly);
    req.sdk = SdkDependency::Local {
        superlive_root: checkout,
    };
    match slscaffold::scaffold(&req) {
        Err(ScaffoldError::SdkMissing { reason, .. }) => {
            assert!(reason.contains("build the SDK first"));
        }
        other => panic!("expected SdkMissing, got {other:?}"),
    }
}

#[test]
fn local_sdk_missing_checkout_directory_is_rejected() {
    let dir = TempDir::new().unwrap();
    let mut req = request(dir.path().join("proj"), ComponentShape::BackendOnly);
    req.sdk = SdkDependency::Local {
        superlive_root: dir.path().join("not-a-checkout"),
    };
    assert!(matches!(
        slscaffold::scaffold(&req),
        Err(ScaffoldError::SdkMissing { .. })
    ));
}

// ---- next steps ----

#[test]
fn next_steps_follow_the_shape() {
    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::FrontendOnly);
    assert_eq!(result.next_steps.len(), 2);
    assert!(result.next_steps[0].starts_with("frontend:"));
    assert!(result.next_steps[1].starts_with("build   :"));

    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::BackendOnly);
    assert_eq!(result.next_steps.len(), 2);
    assert!(result.next_steps[0].starts_with("backend :"));

    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::FrontendAndBackend);
    assert_eq!(result.next_steps.len(), 3);
    assert!(result.next_steps[0].starts_with("backend :"));
    assert!(result.next_steps[1].starts_with("frontend:"));
    assert!(result.next_steps[2].starts_with("build   :"));
    // The dev port lands in the backend recipe.
    assert!(result.next_steps[0].contains("8700"));
    // The build hint points at the single build entry.
    assert!(result.next_steps[2].contains("scripts/superlive.build"));
}

// ---- exec bit ----

#[test]
#[cfg(unix)]
fn bash_build_script_is_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::BackendOnly);
    let mode = fs::metadata(result.root.join("scripts/superlive.build"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
}

// ---- imported-project gap filling (architecture.md "Creation flow":
// conservative upgrades — an existing file is never overwritten) ----

fn missing_request() -> slscaffold::MissingFilesRequest {
    slscaffold::MissingFilesRequest {
        id: "demo-comp".to_string(),
        display_name: "Demo Comp".to_string(),
        namespace: "demo".to_string(),
        shape: ComponentShape::FrontendAndBackend,
    }
}

#[test]
fn fill_missing_files_creates_only_the_missing_entries() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    // Pre-place a customized build script — the conservative red line: it
    // must survive untouched.
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(root.join("scripts/superlive.build"), "# my custom build\n").unwrap();

    let created = slscaffold::fill_missing_files(
        &missing_request(),
        root,
        &[MissingFile::BuildScript, MissingFile::Readme],
    )
    .unwrap();

    // Only the README was missing; the build script stayed untouched
    assert_eq!(created, vec!["README.md"]);
    assert_eq!(
        fs::read_to_string(root.join("scripts/superlive.build")).unwrap(),
        "# my custom build\n",
        "an existing file must never be overwritten"
    );

    let readme = fs::read_to_string(root.join("README.md")).unwrap();
    // The imported variant: no scaffold claim, no dev recipe, no SDK-mode section
    assert!(
        !readme.contains("scaffolded by"),
        "imported README must not claim scaffolding"
    );
    assert!(
        !readme.contains("## Develop"),
        "imported README carries no dev recipe"
    );
    assert!(!readme.contains("SDK dependency mode"));
    assert!(
        !readme.contains("__"),
        "no placeholder may survive rendering"
    );
    assert!(readme.contains("## Build"));
    assert!(readme.contains("superlive.build"));
}

#[test]
fn fill_missing_files_is_idempotent_when_everything_exists() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let created = slscaffold::fill_missing_files(
        &missing_request(),
        root,
        &[MissingFile::BuildScript, MissingFile::Readme],
    )
    .unwrap();
    assert_eq!(created.len(), 2);

    // Second run: nothing is missing, nothing is rewritten
    let before = fs::read_to_string(root.join("README.md")).unwrap();
    let created = slscaffold::fill_missing_files(
        &missing_request(),
        root,
        &[MissingFile::BuildScript, MissingFile::Readme],
    )
    .unwrap();
    assert!(created.is_empty());
    assert_eq!(fs::read_to_string(root.join("README.md")).unwrap(), before);
}

#[test]
#[cfg(unix)]
fn gap_filled_build_script_is_executable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    slscaffold::fill_missing_files(&missing_request(), root, &[MissingFile::BuildScript]).unwrap();
    let mode = fs::metadata(root.join("scripts/superlive.build"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
}

#[test]
fn fresh_creation_readme_keeps_the_scaffold_sections() {
    // Regression guard for the IMPORTED conditional: the fresh-creation
    // branch still renders the dev recipe and the SDK-mode section.
    let dir = TempDir::new().unwrap();
    let result = scaffold_into(&dir, ComponentShape::FrontendAndBackend);
    let readme = fs::read_to_string(result.root.join("README.md")).unwrap();
    assert!(readme.contains("scaffolded by `slscaffold`"));
    assert!(readme.contains("## Develop"));
    assert!(readme.contains("SDK dependency mode"));
    assert!(
        readme.contains("8700"),
        "the dev port lands in the fresh-creation recipe"
    );
    assert!(
        !readme.contains("__"),
        "no placeholder may survive rendering"
    );
}
