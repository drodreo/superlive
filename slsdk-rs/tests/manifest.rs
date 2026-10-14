//! Manifest contract tests: serde round-trips, full fixture parsing, semantic validation
//! legal/illegal cases, and the four fingerprint verifier cases
//! (pass/tampered/missing/extra).

use sha2::{Digest, Sha256};
use slsdk_rs::{
    verify_files, ComponentManifest, ConfigFieldType, FileEntry, ManifestError, UiType, VerifyError,
};
use std::path::Path;

/// Full manifest fixture (snake_case field names, isomorphic to the slsdk-rs type
/// definitions).
/// The sha256 values are placeholder hex (this file only tests parsing and validation;
/// fingerprint authenticity is constructed dynamically by the verify-series tests).
const FIXTURE: &str = r#"{
  "id": "xatodo",
  "version": "0.1.0",
  "min_shell_version": "0.1.0",
  "namespace": "todo",
  "display_name": "XaTodo",
  "ui": { "ui_type": "module", "entry": "assets/main.js" },
  "menu": { "title": "待办", "icon_path": "assets/icon.svg" },
  "routes": [ { "path": "/todo", "title": "待办列表" } ],
  "launch": { "command": "xatodo-server", "args": ["--config", "config.toml"] },
  "files": [
    { "path": "xatodo-server", "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" },
    { "path": "assets/main.js", "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" }
  ]
}"#;

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn serde_roundtrip_preserves_manifest() {
    let manifest: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    let re = serde_json::to_string(&manifest).unwrap();
    let back: ComponentManifest = serde_json::from_str(&re).unwrap();
    assert_eq!(manifest, back);
    assert_eq!(back.ui.as_ref().unwrap().ui_type, UiType::Module);
}

#[test]
fn full_fixture_parses_with_expected_fields() {
    let m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(m.id, "xatodo");
    assert_eq!(m.namespace, "todo");
    assert_eq!(m.display_name, "XaTodo");
    assert_eq!(m.min_shell_version, "0.1.0");
    assert_eq!(m.ui.as_ref().unwrap().entry, "assets/main.js");
    assert_eq!(m.menu.as_ref().unwrap().title, "待办");
    assert_eq!(
        m.menu.as_ref().unwrap().icon_path.as_deref(),
        Some("assets/icon.svg")
    );
    assert_eq!(m.routes.len(), 1);
    assert_eq!(m.routes[0].path, "/todo");
    assert_eq!(m.launch.as_ref().unwrap().command, "xatodo-server");
    assert_eq!(
        m.launch.as_ref().unwrap().args,
        vec!["--config", "config.toml"]
    );
    assert_eq!(m.files.len(), 2);
    assert!(m.validate().is_ok(), "fixture itself must be valid");
}

#[test]
fn iframe_ui_type_is_lowercase_in_json() {
    let json = r#"{ "ui_type": "iframe", "entry": "index.html" }"#;
    let ui: slsdk_rs::UiDeclaration = serde_json::from_str(json).unwrap();
    assert_eq!(ui.ui_type, UiType::Iframe);
    assert_eq!(serde_json::to_string(&ui.ui_type).unwrap(), r#""iframe""#);
}

// ---------- validate: legal and illegal cases ----------

fn manifest_with_namespace(ns: &str) -> ComponentManifest {
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.namespace = ns.to_string();
    m
}

#[test]
fn validate_rejects_namespace_with_slash() {
    let m = manifest_with_namespace("todo/items");
    assert!(matches!(
        m.validate(),
        Err(ManifestError::InvalidNamespace(_))
    ));
}

#[test]
fn validate_rejects_empty_version() {
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.version = String::new();
    assert!(matches!(
        m.validate(),
        Err(ManifestError::EmptyField { field: "version" })
    ));
}

#[test]
fn validate_rejects_empty_min_shell_version() {
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.min_shell_version = String::new();
    assert!(matches!(
        m.validate(),
        Err(ManifestError::EmptyField {
            field: "min_shell_version"
        })
    ));
}

#[test]
fn validate_rejects_absolute_file_path() {
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.files[0].path = "/etc/passwd".to_string();
    assert!(matches!(
        m.validate(),
        Err(ManifestError::InvalidFilePath {
            reason: "absolute path",
            ..
        })
    ));
}

#[test]
fn validate_rejects_parent_escape_in_file_path() {
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.files[0].path = "../../escape".to_string();
    assert!(matches!(
        m.validate(),
        Err(ManifestError::InvalidFilePath {
            reason: "contains a .. escape",
            ..
        })
    ));
}

#[test]
fn validate_rejects_empty_files() {
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.files.clear();
    assert!(matches!(m.validate(), Err(ManifestError::EmptyFiles)));
}

#[test]
fn validate_rejects_empty_id() {
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.id = String::new();
    assert!(matches!(
        m.validate(),
        Err(ManifestError::EmptyField { field: "id" })
    ));
}

#[test]
fn validate_rejects_invalid_id() {
    // id is the /comps/{id} URL key and the install directory name: containing / or spaces
    // is illegal (same charset as namespace)
    for bad in ["todo/app", "todo app"] {
        let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
        m.id = bad.to_string();
        assert!(
            matches!(m.validate(), Err(ManifestError::InvalidId(_))),
            "id={bad}"
        );
    }
}

#[test]
fn validate_rejects_empty_namespace() {
    let m = manifest_with_namespace("");
    assert!(matches!(
        m.validate(),
        Err(ManifestError::EmptyField { field: "namespace" })
    ));
}

// ---------- shape variants: pure-frontend / pure-backend / legacy manifests ----------

#[test]
fn declaration_level_accepts_empty_files_package_level_rejects() {
    // A dev manifest.json is a pure declaration (files empty): it passes the
    // declaration-level gate and is rejected only at the package level
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.files.clear();
    assert!(
        m.validate_declaration().is_ok(),
        "a pure declaration (files empty) must pass the declaration gate"
    );
    assert!(matches!(m.validate(), Err(ManifestError::EmptyFiles)));
}

#[test]
fn declaration_level_covers_the_shared_rule_set() {
    // The declaration gate keeps every rule except the files rules: a broken
    // namespace / id / version is caught at declaration level too
    let mut bad_ns: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    bad_ns.files.clear();
    bad_ns.namespace = "bad/ns".to_string();
    assert!(matches!(
        bad_ns.validate_declaration(),
        Err(ManifestError::InvalidNamespace(_))
    ));

    let mut bad_version: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    bad_version.files.clear();
    bad_version.version = String::new();
    assert!(matches!(
        bad_version.validate_declaration(),
        Err(ManifestError::EmptyField { field: "version" })
    ));
}

#[test]
fn pure_frontend_shape_validates() {
    // launch=None + ui present: the pure-frontend shape carries no extra
    // constraint
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.launch = None;
    assert!(m.ui.is_some());
    assert!(m.validate().is_ok());
}

#[test]
fn pure_backend_shape_validates() {
    // ui=None + empty mounts: the pure-backend shape validates as long as no
    // mount points are declared
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.ui = None;
    m.routes.clear();
    m.menu = None;
    assert!(m.launch.is_some());
    assert!(m.validate().is_ok());
}

#[test]
fn pure_backend_with_routes_rejected() {
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.ui = None;
    m.menu = None;
    // routes stay non-empty: no UI, yet a mount point is declared
    assert!(!m.routes.is_empty());
    assert!(matches!(m.validate(), Err(ManifestError::MountWithoutUi)));
}

#[test]
fn pure_backend_with_menu_rejected() {
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.ui = None;
    m.routes.clear();
    // menu stays Some: no UI, yet a mount point is declared
    assert!(m.menu.is_some());
    assert!(matches!(m.validate(), Err(ManifestError::MountWithoutUi)));
}

#[test]
fn manifest_without_ui_and_launch_keys_deserializes_to_none() {
    // Old contract (no ui/launch keys): serde(default) deserializes both to
    // None — existing manifests keep parsing
    let json = r#"{
      "id": "legacy",
      "version": "0.1.0",
      "min_shell_version": "0.1.0",
      "namespace": "legacy",
      "display_name": "Legacy",
      "menu": null,
      "routes": [],
      "files": [ { "path": "bin", "sha256": "aa" } ]
    }"#;
    let m: ComponentManifest = serde_json::from_str(json).unwrap();
    assert!(m.ui.is_none());
    assert!(m.launch.is_none());
    assert!(m.validate().is_ok());
}

// ---------- verify_files: pass/tampered/missing/extra ----------

/// Writes actual files and builds the manifest with dynamically computed fingerprints
/// (avoiding hardcoded sha256 constants).
fn build_verified_dir() -> (tempfile::TempDir, ComponentManifest) {
    let dir = tempfile::tempdir().unwrap();
    let files = [
        ("bin/server", b"server binary bytes\n" as &[u8]),
        ("assets/main.js", b"console.log('hello');\n"),
    ];
    let mut entries = Vec::new();
    for (path, content) in files {
        let full = dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, content).unwrap();
        entries.push(FileEntry {
            path: path.to_string(),
            sha256: sha256_hex(content),
        });
    }
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.files = entries;
    (dir, m)
}

#[test]
fn verify_accepts_matching_tree() {
    let (dir, m) = build_verified_dir();
    assert!(verify_files(dir.path(), &m).is_ok());
}

/// S6: the difference-set scan must reject when the install directory contains a symlink
/// (closure — follow semantics break "install directory contents = package contents";
/// ancestor recursion and external pointing are both threat shapes).
#[test]
#[cfg(unix)]
fn verify_rejects_symlink() {
    let (dir, m) = build_verified_dir();
    let target = dir.path().join("bin/server");
    let link = dir.path().join("bin/link-to-server");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    match verify_files(dir.path(), &m) {
        Err(VerifyError::Io { source, .. }) => {
            assert!(
                source.to_string().contains("symlink"),
                "error message should state the rejection reason: {source}"
            );
        }
        other => panic!("expected Io (symlink rejection), got {other:?}"),
    }
}

#[test]
fn verify_rejects_tampered_content() {
    let (dir, m) = build_verified_dir();
    // Content modified, so the fingerprint naturally mismatches
    std::fs::write(dir.path().join("bin/server"), b"tampered!").unwrap();
    match verify_files(dir.path(), &m) {
        Err(VerifyError::HashMismatch { path, .. }) => assert_eq!(path, "bin/server"),
        other => panic!("expected HashMismatch, got {other:?}"),
    }
}

#[test]
fn verify_rejects_missing_file() {
    let (dir, m) = build_verified_dir();
    std::fs::remove_file(dir.path().join("assets/main.js")).unwrap();
    match verify_files(dir.path(), &m) {
        Err(VerifyError::MissingFile(path)) => assert_eq!(path, "assets/main.js"),
        other => panic!("expected MissingFile, got {other:?}"),
    }
}

#[test]
fn verify_rejects_undeclared_extra_file() {
    let (dir, m) = build_verified_dir();
    // Smuggled files outside the manifest must be caught by the difference-set detection
    std::fs::write(dir.path().join("smuggle.sh"), b"rm -rf /\n").unwrap();
    match verify_files(dir.path(), &m) {
        Err(VerifyError::ExtraFile(path)) => assert!(path.contains("smuggle.sh")),
        other => panic!("expected ExtraFile, got {other:?}"),
    }
}

#[test]
fn verify_fingerprint_is_case_insensitive() {
    let (dir, mut m) = build_verified_dir();
    // Declared side uppercased, actual lowercase: hex case convention differences should
    // not cause failure
    for entry in &mut m.files {
        entry.sha256 = entry.sha256.to_uppercase();
    }
    assert!(verify_files(dir.path(), &m).is_ok());
}

#[test]
fn hash_matches_known_vector() {
    // sha256("abc") standard test vector, locking in the correctness of the local
    // implementation
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn relative_path_helper_rejects_prefix_only_paths() {
    // Regression: `..` as a standalone segment (prefix form) must also be rejected
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.files[0].path = "..".to_string();
    assert!(Path::new("..").is_relative());
    assert!(matches!(
        m.validate(),
        Err(ManifestError::InvalidFilePath {
            reason: "contains a .. escape",
            ..
        })
    ));
}

// ---------- config section (architecture.md "Configuration Plane"): serde compatibility
// and validation ----------

/// Manifest fixture with a config section (full field coverage: default serializing back as
/// null, bool tier defaulting false, select tier options, i18n mapping).
const CONFIG_FIXTURE: &str = r#"{
  "id": "sl-finance",
  "version": "0.1.0",
  "min_shell_version": "0.1.0",
  "namespace": "finance",
  "display_name": "Finance",
  "ui": { "ui_type": "module", "entry": "assets/main.js" },
  "menu": null,
  "routes": [],
  "launch": { "command": "sl-finance-server", "args": [] },
  "files": [ { "path": "sl-finance-server", "sha256": "cc" } ],
  "config": {
    "fields": [
      {
        "key": "tushare.token",
        "label": "config.token.label",
        "type": "string",
        "default": null,
        "required": true,
        "sensitive": true,
        "restart_required": false,
        "group": "tushare"
      },
      {
        "key": "tushare.schedule",
        "label": "config.schedule.label",
        "type": "string",
        "format": "cron",
        "default": ""
      },
      {
        "key": "kline.style",
        "label": "config.style.label",
        "type": "select",
        "default": "candlestick",
        "options": ["candlestick", "line"]
      }
    ],
    "i18n": { "zh": "config/zh.json", "en": "config/en.json" }
  }
}"#;

#[test]
fn config_section_serde_roundtrip() {
    let m: ComponentManifest = serde_json::from_str(CONFIG_FIXTURE).unwrap();
    let config = m.config.as_ref().expect("fixture carries a config section");
    assert_eq!(config.fields.len(), 3);
    assert_eq!(config.fields[0].key, "tushare.token");
    assert_eq!(config.fields[0].r#type, ConfigFieldType::String);
    assert!(config.fields[0].required);
    assert!(config.fields[0].sensitive);
    assert_eq!(config.fields[0].group.as_deref(), Some("tushare"));
    assert_eq!(config.fields[1].format.as_deref(), Some("cron"));
    assert_eq!(config.fields[1].default.as_ref().unwrap(), "");
    assert_eq!(config.fields[2].r#type, ConfigFieldType::Select);
    assert_eq!(
        config.fields[2].options.as_deref(),
        Some(&["candlestick".to_string(), "line".to_string()][..])
    );
    let i18n = config.i18n.as_ref().unwrap();
    assert_eq!(i18n.get("zh").map(String::as_str), Some("config/zh.json"));

    // Serialization round-trip (including the r#type → "type" wire name and the
    // ConfigFieldType lowercase form)
    let re = serde_json::to_string(&m).unwrap();
    let back: ComponentManifest = serde_json::from_str(&re).unwrap();
    assert_eq!(m, back);
    let wire = serde_json::to_value(&config.fields[2]).unwrap();
    assert_eq!(wire["type"], "select");
    assert!(m.validate().is_ok(), "config fixture itself must be valid");
}

#[test]
fn legacy_manifest_without_config_section_is_compatible() {
    // Existing manifests (without a config section) deserialize to None via serde(default)
    // — the six components are unaffected
    let m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    assert!(m.config.is_none());
    assert!(m.validate().is_ok());
}

// ---------- localized titles (architecture.md "Localized titles") ----------

#[test]
fn legacy_manifest_without_titles_deserializes_to_empty_maps() {
    // Old manifests carry no titles key on menu or routes: serde(default)
    // yields empty maps — every legacy manifest stays valid unchanged
    let m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    assert!(m.menu.as_ref().unwrap().titles.is_empty());
    assert!(m.routes[0].titles.is_empty());
    assert!(m.validate().is_ok());
}

#[test]
fn titles_roundtrip_and_wire_form() {
    // The wire form is a flat locale → string map on both menu and routes;
    // a round-trip preserves the entries
    let json = FIXTURE
        .replace(
            r#""menu": { "title": "待办", "icon_path": "assets/icon.svg" }"#,
            r#""menu": { "title": "待办", "icon_path": "assets/icon.svg", "titles": { "zh": "待办", "en": "Todo" } }"#,
        )
        .replace(
            r#""routes": [ { "path": "/todo", "title": "待办列表" } ]"#,
            r#""routes": [ { "path": "/todo", "title": "待办列表", "titles": { "zh": "待办列表", "en": "Todo list" } } ]"#,
        );
    let m: ComponentManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(
        m.menu
            .as_ref()
            .unwrap()
            .titles
            .get("en")
            .map(String::as_str),
        Some("Todo")
    );
    assert_eq!(
        m.routes[0].titles.get("zh").map(String::as_str),
        Some("待办列表")
    );
    assert!(m.validate().is_ok());

    let re = serde_json::to_string(&m).unwrap();
    let back: ComponentManifest = serde_json::from_str(&re).unwrap();
    assert_eq!(m, back);
}

#[test]
fn empty_titles_map_is_omitted_populated_map_is_serialized() {
    // skip_serializing_if: a single-language manifest serializes without the
    // titles key (legacy package bytes stay identical); a populated map lands
    // on the wire under the plain "titles" name
    let m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    let single = serde_json::to_string(&m).unwrap();
    assert!(
        !single.contains("titles"),
        "an empty titles map must be omitted at serialization"
    );

    let mut localized = m.clone();
    localized
        .menu
        .as_mut()
        .unwrap()
        .titles
        .insert("zh".to_string(), "待办".to_string());
    let wire = serde_json::to_value(localized.menu.as_ref().unwrap()).unwrap();
    assert_eq!(wire["titles"]["zh"], "待办");
}

#[test]
fn validate_rejects_empty_locale_key_in_menu_titles() {
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.menu
        .as_mut()
        .unwrap()
        .titles
        .insert(String::new(), "Todo".to_string());
    assert!(matches!(
        m.validate(),
        Err(ManifestError::InvalidTitles { at, reason })
            if at == "menu" && reason == "empty locale key"
    ));
}

#[test]
fn validate_rejects_empty_value_in_menu_titles() {
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.menu
        .as_mut()
        .unwrap()
        .titles
        .insert("zh".to_string(), String::new());
    assert!(matches!(
        m.validate(),
        Err(ManifestError::InvalidTitles { at, reason }) if at == "menu" && reason == "empty value"
    ));
}

#[test]
fn validate_rejects_empty_value_in_route_titles() {
    let mut m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    m.routes[0].titles.insert("en".to_string(), String::new());
    assert!(matches!(
        m.validate(),
        Err(ManifestError::InvalidTitles { at, reason })
            if at == "routes[0]" && reason == "empty value"
    ));
}

#[test]
fn validate_rejects_config_key_with_empty_segment_or_whitespace() {
    for bad in ["a..b", "a b", ".a", "a."] {
        let mut m: ComponentManifest = serde_json::from_str(CONFIG_FIXTURE).unwrap();
        m.config.as_mut().unwrap().fields[0].key = bad.to_string();
        assert!(
            matches!(
                m.validate(),
                Err(ManifestError::InvalidConfigField {
                    index: 0,
                    reason: "key is empty or contains empty segments/whitespace"
                })
            ),
            "key={bad}"
        );
    }
}

#[test]
fn validate_rejects_select_without_options() {
    let mut m: ComponentManifest = serde_json::from_str(CONFIG_FIXTURE).unwrap();
    m.config.as_mut().unwrap().fields[2].options = None;
    assert!(matches!(
        m.validate(),
        Err(ManifestError::InvalidConfigField {
            index: 2,
            reason: "select field is missing options"
        })
    ));
}
