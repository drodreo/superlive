//! Manifest types and validation: the serde contract for the manifest file placed at the
//! package root.
//!
//! Contract source: docs/architecture.md "Installation and Lifecycle" — the archive root
//! carries a fixed manifest (id / version / compatible shell version / namespace / menu and
//! route registration declarations / launch command / per-file fingerprints / structured
//! config declaration); after extraction the shell verifies fingerprints file by file before
//! landing the files. The UI declaration and the launch command are both optional:
//! `ui = None` is the pure-backend shape, `launch = None` the pure-frontend shape.
//!
//! Field names keep their Rust snake_case form with no rename: the shell and this crate are
//! both Rust and share this single type definition, so structural isomorphism on both ends
//! means no drift.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::Path;

/// The manifest file at the root of a component package (package format contract, see
/// architecture.md "Installation and Lifecycle").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentManifest {
    /// Component unique id: the key of the install directory name and the `/comps/{id}`
    /// static URL space.
    pub id: String,
    /// Component version.
    pub version: String,
    /// Minimum compatible shell version: the shell gates on it at install time.
    pub min_shell_version: String,
    /// Namespace registered at the shell gateway: the `ns` of `/api/{ns}/{rest}`, the key for
    /// registry dispatch.
    pub namespace: String,
    /// Display name (used by menu / settings pages).
    pub display_name: String,
    /// UI declaration (module / iframe tiers); `None` = pure-backend shape
    /// (no UI mount surface: menu / routes must both be empty per
    /// [`ComponentManifest::validate`], and the shell exposes no
    /// `/comps/{id}` entry).
    #[serde(default)]
    pub ui: Option<UiDeclaration>,
    /// Menu registration declaration; omit if no menu is registered.
    pub menu: Option<MenuDeclaration>,
    /// Route registration declarations: where the component UI mounts in the shell's main
    /// route tree.
    pub routes: Vec<RouteDeclaration>,
    /// Component backend launch: the command the shell spawns as a subprocess;
    /// `None` = pure-frontend shape (the shell spawns no backend process, so
    /// the namespace has no gateway forwarding target).
    #[serde(default)]
    pub launch: Option<LaunchConfig>,
    /// Package file list (relative paths + sha256), verified file by file by
    /// [`verify_files`] after extraction.
    pub files: Vec<FileEntry>,
    /// Component config declaration (architecture.md "Configuration Plane"): statically
    /// declares the config field list, the single source of truth for rendering the shell
    /// config page — while the config page works the component process has not started yet,
    /// so runtime reporting is excluded, and the runtime endpoint ([`crate::config`]) only
    /// reads/writes values. `serde(default)` keeps deserialization of existing manifests
    /// (without a config section) unaffected.
    #[serde(default)]
    pub config: Option<ComponentConfig>,
    /// Declared transport capabilities of the backend process (the shell reads
    /// it when choosing the injected `SL_ENDPOINT` descriptor). `None` = the
    /// default: every transport the host platform offers (Unix domain socket
    /// on Linux/macOS, loopback TCP everywhere). `serde(default)` keeps legacy
    /// manifests (without the key) parsing unchanged, and `skip_serializing_if`
    /// omits the default so existing manifest bytes stay identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportCaps>,
}

/// Transport capability tiers a component backend can declare
/// (architecture.md "Plane B contract · transport"). The set is the minimal
/// v1 vocabulary; runtimes that cannot offer the platform default declare
/// their restriction here instead of failing at bind time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportCaps {
    /// The backend can only listen on TCP (e.g. a runtime with no Unix domain
    /// socket support): the shell injects a random loopback `tcp:` descriptor
    /// on every platform instead of its platform default.
    TcpOnly,
}

/// UI mounting shape (architecture.md "Plane A Contract": two UI container tiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiType {
    /// React component: dynamically imported into the host route tree at runtime; the
    /// experience matches a monolith.
    Module,
    /// Components with heterogeneous stacks or needing isolation: the shell mounts them in
    /// an iframe container.
    Iframe,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiDeclaration {
    pub ui_type: UiType,
    /// Relative path of the UI entry: an ESM bundle for the module tier (e.g.
    /// `assets/main.js`), HTML for the iframe tier.
    pub entry: String,
}

/// Menu registration declaration. The field set is the minimal v1 set, to be refined when
/// the shell module integrates (e.g. sorting/grouping/badges).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MenuDeclaration {
    pub title: String,
    pub icon_path: Option<String>,
    /// Localized titles (locale → title; architecture.md "Localized titles").
    /// Absent or empty = a single-language entry (the [`MenuDeclaration::title`]
    /// fallback alone): `serde(default)` keeps legacy manifests without the key
    /// parsing unchanged, and `skip_serializing_if` omits an empty map so legacy
    /// package bytes stay identical. Consumed by the shell's i18n injection into
    /// the component's own namespace under the fixed `menuTitle` key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub titles: BTreeMap<String, String>,
}

/// Route registration declaration. The field set is the minimal v1 set, to be refined when
/// the shell module integrates (e.g. nested routes/permissions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteDeclaration {
    pub path: String,
    pub title: String,
    /// Localized titles (locale → title), same contract as
    /// [`MenuDeclaration::titles`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub titles: BTreeMap<String, String>,
}

/// Component backend launch config: the shell spawns an independent subprocess with
/// `command + args`
/// (architecture.md "Plane B Contract": process model, crash isolation, any language,
/// independent upgrades).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaunchConfig {
    pub command: String,
    pub args: Vec<String>,
}

/// Fingerprint entry for a single file inside the package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Path relative to the package root.
    pub path: String,
    /// sha256 (hex) of the file content.
    pub sha256: String,
}

/// The manifest config section (architecture.md "Configuration Plane · Config Declaration"):
/// config field list + label i18n mapping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentConfig {
    /// Declared config field list (the rendering basis of the shell config page).
    pub fields: Vec<ConfigField>,
    /// Label i18n: lang → static JSON file path relative to the component's `ui/` directory
    /// (e.g. `{"zh": "config/zh.json", "en": "config/en.json"}`); files ship with the
    /// package and are reachable via the `/comps/{id}/` static space — the config page does
    /// not depend on the component ESM module loading successfully.
    #[serde(default)]
    pub i18n: Option<HashMap<String, String>>,
}

/// Declaration of a single config field (full contract field set, see architecture.md
/// "Configuration Plane · Config Declaration").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigField {
    /// TOML path-form key: dotted paths within segments are supported (e.g.
    /// `"tushare.token"` = `[tushare] token`). Path segment validation see
    /// [`ComponentManifest::validate`] (non-empty segments, no whitespace).
    pub key: String,
    /// i18n key of the display label (the shell resolves it to concrete text via
    /// `ComponentConfig::i18n`).
    pub label: String,
    /// Value type tier (determines endpoint validation and config page control shape).
    pub r#type: ConfigFieldType,
    /// Default value: the merge fallback when config.toml lacks the entry (GET semantics).
    /// serde_json::Value chosen — the manifest is a JSON document with zero conversion, and
    /// toml boundary conversion is concentrated in the read/write two spots of
    /// [`crate::config`].
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    /// true = must have a non-empty value in the merged state, otherwise the config gate
    /// blocks it (Lifecycle Three Laws 2/3).
    #[serde(default)]
    pub required: bool,
    /// Enum value list for the select tier (should be None for other types).
    #[serde(default)]
    pub options: Option<Vec<String>>,
    /// Format constraint: v1 recognizes `"cron"` (cron 0.12 Schedule parsing, same
    /// semantics as sl-finance); other values pass through unvalidated. An empty string for
    /// cron means "clear schedule" semantics, skipping validation.
    #[serde(default)]
    pub format: Option<String>,
    /// true = takes effect only after the component restarts; false = runtime item
    /// (hot-reloaded by the component via the on_change callback).
    #[serde(default)]
    pub restart_required: bool,
    /// Sensitive value marker: the endpoint still returns the real value (behind token
    /// auth); masking is the shell UI rendering layer's responsibility.
    #[serde(default)]
    pub sensitive: bool,
    /// Grouping hint (used by the shell config page for grouped rendering).
    #[serde(default)]
    pub group: Option<String>,
}

/// Config value types (architecture.md "Configuration Plane", five tiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigFieldType {
    /// Arbitrary string.
    String,
    /// JSON number (integer vs float not distinguished; handled by the component's semantic
    /// layer).
    Number,
    /// Boolean.
    Boolean,
    /// Enum string; the value must be ∈ [`ConfigField::options`].
    Select,
    /// ISO 8601 string (validated as a string in v1, no deep calendar validation).
    Datetime,
}

impl ConfigFieldType {
    /// Wire name (shown in validation error messages, consistent with the serde
    /// serialization form).
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ConfigFieldType::String => "string",
            ConfigFieldType::Number => "number",
            ConfigFieldType::Boolean => "boolean",
            ConfigFieldType::Select => "select",
            ConfigFieldType::Datetime => "datetime",
        }
    }
}

/// Manifest semantic validation errors ([`ComponentManifest::validate`]).
#[derive(Debug)]
pub enum ManifestError {
    /// A required field is empty.
    EmptyField { field: &'static str },
    /// namespace contains characters outside the legal URL segment charset
    /// (letters/digits/hyphens).
    /// The charset check naturally excludes `/`, so "no leading /" needs no separate branch.
    InvalidNamespace(String),
    /// id contains illegal characters (/comps/{id} URL key and install directory name; same
    /// rule as namespace).
    InvalidId(String),
    /// File path illegal (absolute path / contains `..` escape / empty path).
    InvalidFilePath { path: String, reason: &'static str },
    /// The files list is empty: a component without any file has no reason to be installed.
    EmptyFiles,
    /// The config section declaration is illegal (empty key / empty or whitespace segments /
    /// select tier missing options).
    InvalidConfigField { index: usize, reason: &'static str },
    /// A localized-titles entry is illegal (an empty locale key or an empty
    /// value): such an entry cannot resolve through the locale fallback chain.
    /// `at` names the declaration site ("menu" / "routes[i]").
    InvalidTitles { at: String, reason: &'static str },
    /// `ui = None` (pure-backend shape) but UI mount points are declared
    /// (menu / routes): a component without UI has nothing to mount.
    MountWithoutUi,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::EmptyField { field } => write!(f, "required field is empty: {field}"),
            ManifestError::InvalidNamespace(ns) => {
                write!(
                    f,
                    "invalid namespace (only letters, digits, and hyphens are allowed): {ns}"
                )
            }
            ManifestError::InvalidId(id) => {
                write!(
                    f,
                    "invalid id (only letters, digits, and hyphens are allowed): {id}"
                )
            }
            ManifestError::InvalidFilePath { path, reason } => {
                write!(f, "invalid files path ({reason}): {path}")
            }
            ManifestError::EmptyFiles => write!(f, "files list is empty"),
            ManifestError::InvalidConfigField { index, reason } => {
                write!(f, "invalid declaration at config.fields[{index}]: {reason}")
            }
            ManifestError::InvalidTitles { at, reason } => {
                write!(f, "invalid titles entry at {at}: {reason}")
            }
            ManifestError::MountWithoutUi => write!(
                f,
                "ui is absent (pure-backend component) but menu/routes are declared: a component without UI has no mount points"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

impl ComponentManifest {
    /// Declaration-level semantic validation: the v1 rule set for namespace / version /
    /// min_shell_version / localized titles / mount consistency / config — everything a
    /// **pure declaration** (a dev `manifest.json`, `files` empty) must satisfy. The dev
    /// load path and the creation flow validate at this level; the files list is a
    /// package concept and is not their concern.
    pub fn validate_declaration(&self) -> Result<(), ManifestError> {
        // id is the /comps/{id} static URL key and the install directory name, same rule as
        // namespace: non-empty + URL segment charset
        if self.id.is_empty() {
            return Err(ManifestError::EmptyField { field: "id" });
        }
        if !self
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(ManifestError::InvalidId(self.id.clone()));
        }
        if self.namespace.is_empty() {
            return Err(ManifestError::EmptyField { field: "namespace" });
        }
        // Legal URL segment = alphanumeric + hyphen; the charset constraint naturally
        // excludes `/` and path traversal shapes
        if !self
            .namespace
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(ManifestError::InvalidNamespace(self.namespace.clone()));
        }
        if self.version.is_empty() {
            return Err(ManifestError::EmptyField { field: "version" });
        }
        if self.min_shell_version.is_empty() {
            return Err(ManifestError::EmptyField {
                field: "min_shell_version",
            });
        }
        // Localized titles (architecture.md "Localized titles"): every locale key and
        // value must be non-empty — an empty entry would silently break the
        // locale fallback chain instead of localizing it. An absent or empty map
        // is the legal single-language form and needs no check.
        if let Some(menu) = &self.menu {
            validate_titles(&menu.titles, "menu")?;
        }
        for (index, route) in self.routes.iter().enumerate() {
            validate_titles(&route.titles, &format!("routes[{index}]"))?;
        }
        // Mount-point consistency: ui=None (pure-backend shape) has no UI mount
        // surface — menu and routes must both be empty (nothing to mount);
        // launch=None (pure-frontend shape) carries no extra constraint
        if self.ui.is_none() && (!self.routes.is_empty() || self.menu.is_some()) {
            return Err(ManifestError::MountWithoutUi);
        }
        if let Some(config) = &self.config {
            for (index, field) in config.fields.iter().enumerate() {
                // key is a dotted path into the toml Table: empty segments ("a..b") and
                // whitespace characters would both distort the write-back semantics
                if field.key.is_empty()
                    || field
                        .key
                        .split('.')
                        .any(|seg| seg.is_empty() || seg.contains(char::is_whitespace))
                {
                    return Err(ManifestError::InvalidConfigField {
                        index,
                        reason: "key is empty or contains empty segments/whitespace",
                    });
                }
                // A select tier without enum values cannot render on the config page nor be
                // validated by the endpoint
                if field.r#type == ConfigFieldType::Select
                    && field.options.as_ref().is_none_or(|o| o.is_empty())
                {
                    return Err(ManifestError::InvalidConfigField {
                        index,
                        reason: "select field is missing options",
                    });
                }
            }
        }
        Ok(())
    }

    /// Package-level semantic validation = [`Self::validate_declaration`] + the files
    /// rules (non-empty list, per-entry relative-path safety). The gate for the install
    /// path — a package without files has nothing to install, and a file escaping the
    /// install directory is outside the contract.
    pub fn validate(&self) -> Result<(), ManifestError> {
        self.validate_declaration()?;
        if self.files.is_empty() {
            return Err(ManifestError::EmptyFiles);
        }
        for entry in &self.files {
            validate_relative_path(&entry.path)?;
        }
        Ok(())
    }
}

/// Localized-titles validation shared by the menu and route declarations: every
/// locale key and every value must be a non-empty string. `at` names the
/// declaration site in error messages ("menu" / "routes[i]").
fn validate_titles(titles: &BTreeMap<String, String>, at: &str) -> Result<(), ManifestError> {
    for (locale, value) in titles {
        if locale.is_empty() {
            return Err(ManifestError::InvalidTitles {
                at: at.to_string(),
                reason: "empty locale key",
            });
        }
        if value.is_empty() {
            return Err(ManifestError::InvalidTitles {
                at: at.to_string(),
                reason: "empty value",
            });
        }
    }
    Ok(())
}

/// Relative path safety validation: rejects absolute paths and `..` escapes — files outside
/// the install directory are not covered by the contract.
fn validate_relative_path(path: &str) -> Result<(), ManifestError> {
    if path.is_empty() {
        return Err(ManifestError::InvalidFilePath {
            path: path.to_string(),
            reason: "empty path",
        });
    }
    let p = Path::new(path);
    if p.is_absolute() {
        return Err(ManifestError::InvalidFilePath {
            path: path.to_string(),
            reason: "absolute path",
        });
    }
    for comp in p.components() {
        match comp {
            std::path::Component::ParentDir => {
                return Err(ManifestError::InvalidFilePath {
                    path: path.to_string(),
                    reason: "contains a .. escape",
                });
            }
            // Prefix is the Windows drive-letter form, RootDir is the unix root; both mean
            // leaving the install directory
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(ManifestError::InvalidFilePath {
                    path: path.to_string(),
                    reason: "absolute path",
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// Install directory verification errors ([`verify_files`]).
#[derive(Debug)]
pub enum VerifyError {
    /// A file declared in the manifest does not exist in the install directory.
    MissingFile(String),
    /// The actual sha256 does not match the declared one (file tampered or list stale).
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },
    /// A file exists in the install directory that the manifest does not declare
    /// (difference-set detection, anti-smuggling).
    ExtraFile(String),
    /// Reading the filesystem failed (permissions/IO faults etc., distinct from "file not
    /// found").
    Io {
        path: String,
        source: std::io::Error,
    },
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerifyError::MissingFile(path) => write!(
                f,
                "declared file missing from the install directory: {path}"
            ),
            VerifyError::HashMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "fingerprint mismatch: {path} (declared {expected}, actual {actual})"
            ),
            VerifyError::ExtraFile(path) => {
                write!(f, "undeclared file found in the install directory: {path}")
            }
            VerifyError::Io { path, source } => write!(f, "failed to read {path}: {source}"),
        }
    }
}

impl std::error::Error for VerifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            VerifyError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Shell-side verifier: checks sha256 fingerprints file by file before the extracted files
/// land, and detects undeclared files.
///
/// Three failure kinds are reported separately: missing ([`VerifyError::MissingFile`]),
/// fingerprint mismatch ([`VerifyError::HashMismatch`]), extra ([`VerifyError::ExtraFile`]).
/// Fingerprint comparison is case-insensitive (hex case convention differences are not a
/// failure reason).
pub fn verify_files(root: &Path, manifest: &ComponentManifest) -> Result<(), VerifyError> {
    let declared: HashSet<&str> = manifest.files.iter().map(|e| e.path.as_str()).collect();

    // Per-file verification: missing and fingerprint mismatch
    for entry in &manifest.files {
        let full = root.join(&entry.path);
        let bytes = match std::fs::read(&full) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(VerifyError::MissingFile(entry.path.clone()));
            }
            Err(source) => {
                return Err(VerifyError::Io {
                    path: entry.path.clone(),
                    source,
                });
            }
        };
        let actual = sha256_hex(&bytes);
        if !actual.eq_ignore_ascii_case(&entry.sha256) {
            return Err(VerifyError::HashMismatch {
                path: entry.path.clone(),
                expected: entry.sha256.clone(),
                actual,
            });
        }
    }

    // Extra file detection: the difference of actual file set − declared set
    let mut actual_files = HashSet::new();
    collect_relative_files(root, root, &mut actual_files).map_err(|source| VerifyError::Io {
        path: root.to_string_lossy().into_owned(),
        source,
    })?;
    if let Some(extra) = actual_files.iter().find(|p| !declared.contains(p.as_str())) {
        return Err(VerifyError::ExtraFile(extra.clone()));
    }
    Ok(())
}

/// Recursively collects relative paths of all regular files under root (no walkdir, keeping
/// dependencies minimal).
/// Uses file_type (lstat semantics, does not follow symlinks) rather than
/// path::is_dir/is_file:
/// under follow semantics a symlink pointing to an ancestor directory recurses infinitely,
/// and a symlink pointing outside breaks the closure of "install directory contents =
/// package contents" — symlinks are rejected as errors.
fn collect_relative_files(
    root: &Path,
    dir: &Path,
    out: &mut HashSet<String>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_symlink() {
            return Err(std::io::Error::other(format!(
                "install directory contains a symlink (breaks containment, rejected): {}",
                path.display()
            )));
        }
        if file_type.is_dir() {
            collect_relative_files(root, &path, out)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("walk starts at root, so strip_prefix cannot fail");
            out.insert(rel.to_string_lossy().into_owned());
        }
    }
    Ok(())
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod transport_tests {
    use super::*;

    /// Minimal valid manifest body (legacy shape: no transport key).
    const LEGACY_MANIFEST: &str = r#"{
        "id": "demo",
        "version": "0.1.0",
        "min_shell_version": "0.2.0",
        "namespace": "demo",
        "display_name": "Demo",
        "menu": null,
        "routes": [],
        "files": []
    }"#;

    #[test]
    fn legacy_manifest_without_transport_key_parses_as_none() {
        let manifest: ComponentManifest = serde_json::from_str(LEGACY_MANIFEST).unwrap();
        assert_eq!(manifest.transport, None);
    }

    #[test]
    fn declared_tcp_only_parses_and_survives_a_round_trip() {
        let manifest: ComponentManifest = serde_json::from_str(
            &LEGACY_MANIFEST.replace(r#""files": []"#, r#""files": [], "transport": "tcponly""#),
        )
        .unwrap();
        assert_eq!(manifest.transport, Some(TransportCaps::TcpOnly));

        let bytes = serde_json::to_string(&manifest).unwrap();
        let back: ComponentManifest = serde_json::from_str(&bytes).unwrap();
        assert_eq!(back.transport, Some(TransportCaps::TcpOnly));
    }

    #[test]
    fn none_transport_serializes_without_the_key() {
        // Byte compatibility: a manifest without a transport declaration
        // serializes exactly like the pre-field schema (the key is absent,
        // not null).
        let manifest: ComponentManifest = serde_json::from_str(LEGACY_MANIFEST).unwrap();
        let bytes = serde_json::to_string(&manifest).unwrap();
        assert!(!bytes.contains("transport"), "{bytes}");

        let round: ComponentManifest = serde_json::from_str(&bytes).unwrap();
        assert_eq!(round.transport, None);
    }
}
