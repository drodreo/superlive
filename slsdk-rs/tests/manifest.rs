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
    assert_eq!(back.ui.ui_type, UiType::Module);
}

#[test]
fn full_fixture_parses_with_expected_fields() {
    let m: ComponentManifest = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(m.id, "xatodo");
    assert_eq!(m.namespace, "todo");
    assert_eq!(m.display_name, "XaTodo");
    assert_eq!(m.min_shell_version, "0.1.0");
    assert_eq!(m.ui.entry, "assets/main.js");
    assert_eq!(m.menu.as_ref().unwrap().title, "待办");
    assert_eq!(
        m.menu.as_ref().unwrap().icon_path.as_deref(),
        Some("assets/icon.svg")
    );
    assert_eq!(m.routes.len(), 1);
    assert_eq!(m.routes[0].path, "/todo");
    assert_eq!(m.launch.command, "xatodo-server");
    assert_eq!(m.launch.args, vec!["--config", "config.toml"]);
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
