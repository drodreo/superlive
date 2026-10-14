//! Import flow: registering an **existing** project as a dev component
//! (architecture.md "Creation flow" — Importing an existing project).
//!
//! Two-phase contract:
//!
//! 1. **Inspect** (no disk mutation): manifest present and parsing, declaration
//!    validation, shape detection (server/ and web/ directories), the
//!    version-compatibility gate, and an upgrade scan. Findings split into
//!    *unfixable* (contract violations — the reason list is the rejection) and
//!    *upgrades* (the conservative gap-filling list: manifest-level corrections
//!    applied in memory + missing scaffold files). Registration-key collisions
//!    (id/namespace already registered) are unfixable findings.
//! 2. **Apply**: with `apply_upgrades` the in-memory corrections are written
//!    back (declaration file only, same serialization as the creation flow)
//!    and only-missing scaffold files are generated (an existing file is never
//!    touched); without it the upgrade list is returned for confirmation.
//!    Registration reuses the creation flow's tail verbatim (dev envelope +
//!    snapshot + settled config state) — no second registration path.
//!
//! Error channels follow the creation flow's [`CreateError`]: unfixable
//! findings surface as `invalid_argument` (the directory's content is the
//! problem), world-state collisions as `conflict`.

use crate::commands::{ComponentView, CreateError};
use crate::config_plane;
use crate::installer::{parse_semver, shell_version_satisfied};
use crate::registry::{self, ComponentSource, RegistryEntry};
use crate::AppState;
use serde::Serialize;
use slscaffold::{MissingFile, MissingFilesRequest};
use slsdk_rs::ComponentManifest;
use std::path::{Path, PathBuf};

/// The kind of one conservative gap-filling correction (the frontend maps
/// this to a human label; `detail` carries the precise what-and-to-what).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeKind {
    /// Menu/route `titles` are empty — derived from `display_name` as zh+en.
    DeriveTitles,
    /// `version` differs from the project's version source — refreshed to it.
    RefreshVersion,
    /// `min_shell_version` is empty — filled with the current shell version.
    FillMinShellVersion,
    /// `scripts/superlive.build` is missing — generated.
    AddBuildScript,
    /// `README.md` is missing — generated.
    AddReadme,
}

/// One upgrade-list entry: the correction kind plus a precise English
/// description (the confirmation dialog renders these verbatim).
#[derive(Debug, Clone, Serialize)]
pub struct UpgradeItem {
    pub kind: UpgradeKind,
    pub detail: String,
}

/// Import outcome, tagged for the frontend's `status` branch:
/// `registered` = the same payload shape as the creation flow's result;
/// `upgrade_required` = the confirmation list (a normal flow, not an error —
/// the frontend re-invokes with `apply_upgrades` after the user confirms).
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ImportOutcome {
    Registered {
        component: Box<ComponentView>,
        project_dir: String,
        next_steps: Vec<String>,
    },
    UpgradeRequired {
        items: Vec<UpgradeItem>,
    },
}

/// Phase-1 product: everything inspect learned without touching the disk.
struct Inspection {
    /// The manifest with in-memory corrections already applied (the caller
    /// writes it back only when [`Inspection::manifest_modified`] is set).
    manifest: ComponentManifest,
    shape: slscaffold::ComponentShape,
    /// Conservative gap-filling list (empty = fully compliant).
    upgrades: Vec<UpgradeItem>,
    /// Contract violations; non-empty ⇒ the import is rejected with these
    /// reasons joined.
    unfixable: Vec<String>,
    /// Whether the in-memory corrections changed the manifest (drives the
    /// declaration write-back — a compliant project's manifest.json is the
    /// author's file and stays untouched).
    manifest_modified: bool,
}

/// Phase 1 — inspect the project directory (synchronous IO; runs inside a
/// blocking hop). Every check happens before any mutation; the only writes
/// are in-memory manifest corrections tracked by `manifest_modified`.
fn inspect(root: &Path) -> Result<Inspection, CreateError> {
    let mut unfixable: Vec<String> = Vec::new();
    let mut upgrades: Vec<UpgradeItem> = Vec::new();

    // -- manifest present and parsing --
    let manifest_path = root.join("manifest.json");
    let bytes = match std::fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(CreateError::InvalidArgument(format!(
                "no manifest.json in {}",
                root.display()
            )));
        }
        Err(e) => {
            return Err(CreateError::InvalidArgument(format!(
                "failed to read {}: {e}",
                manifest_path.display()
            )));
        }
    };
    let mut manifest: ComponentManifest = serde_json::from_slice(&bytes).map_err(|e| {
        CreateError::InvalidArgument(format!("failed to parse {}: {e}", manifest_path.display()))
    })?;
    let mut manifest_modified = false;

    // -- shape detection (server/ and web/ directories) --
    let has_server = root.join("server").is_dir();
    let has_web = root.join("web").is_dir();
    if !has_server && !has_web {
        unfixable.push(
            "neither a server/ nor a web/ directory exists — no recognizable component shape"
                .to_string(),
        );
    }
    // The declaration and the detected shape must agree: a launch without a
    // server/ (or a ui without a web/) can never start or mount; a directory
    // contradicting the declaration is the project disagreeing with itself.
    if manifest.launch.is_some() && !has_server {
        unfixable.push(
            "the manifest declares a backend launch but no server/ directory exists".to_string(),
        );
    }
    if manifest.launch.is_none() && has_server {
        unfixable.push(
            "a server/ directory exists but the manifest declares no launch — the backend would never start"
                .to_string(),
        );
    }
    if manifest.ui.is_some() && !has_web {
        unfixable.push("the manifest declares a UI but no web/ directory exists".to_string());
    }
    if manifest.ui.is_none() && has_web {
        unfixable.push(
            "a web/ directory exists but the manifest declares no ui — nothing would mount"
                .to_string(),
        );
    }
    let shape = match (has_server, has_web) {
        (true, true) => slscaffold::ComponentShape::FrontendAndBackend,
        (false, true) => slscaffold::ComponentShape::FrontendOnly,
        (true, false) => slscaffold::ComponentShape::BackendOnly,
        (false, false) => {
            // Nothing else is decidable without a shape; report what we have
            return Ok(Inspection {
                manifest,
                shape: slscaffold::ComponentShape::BackendOnly,
                upgrades,
                unfixable,
                manifest_modified,
            });
        }
    };

    // -- upgrade scan (conservative, in-memory) --

    // min_shell_version empty → fill with the current shell version. A
    // deliberate pre-correction BEFORE declaration validation: an empty value
    // is an upgrade finding per the contract, not a contract violation.
    if manifest.min_shell_version.is_empty() {
        manifest.min_shell_version = env!("CARGO_PKG_VERSION").to_string();
        manifest_modified = true;
        upgrades.push(UpgradeItem {
            kind: UpgradeKind::FillMinShellVersion,
            detail: format!(
                "fill the empty min_shell_version with the current shell version {}",
                env!("CARGO_PKG_VERSION")
            ),
        });
    }

    // version vs the project's version source (same rule as the creation
    // flow's readback). The version source is the authority — if it cannot be
    // read the project's release chain is broken, which is unfixable here.
    let has_server_shape = matches!(
        shape,
        slscaffold::ComponentShape::FrontendAndBackend | slscaffold::ComponentShape::BackendOnly
    );
    match registry::project_version(root, has_server_shape) {
        Ok(source_version) => {
            if manifest.version.is_empty() || manifest.version != source_version {
                upgrades.push(UpgradeItem {
                    kind: UpgradeKind::RefreshVersion,
                    detail: format!(
                        "refresh the manifest version ({}) to the project version source value \"{}\"",
                        if manifest.version.is_empty() {
                            "empty"
                        } else {
                            manifest.version.as_str()
                        },
                        source_version
                    ),
                });
                manifest.version = source_version;
                manifest_modified = true;
            }
        }
        Err(reason) => {
            unfixable.push(format!(
                "the project version source is unreadable: {reason}"
            ));
        }
    }

    // titles derivation: the localized maps are empty (or absent) — derive
    // zh+en from display_name (architecture.md "Localized titles" assembly
    // chain). One list entry covers menu + routes (same source, same values).
    if manifest.ui.is_some() {
        let titles_missing = manifest.menu.as_ref().is_some_and(|m| m.titles.is_empty())
            || manifest.routes.iter().any(|r| r.titles.is_empty());
        if titles_missing {
            if manifest.display_name.trim().is_empty() {
                unfixable.push(
                    "display_name is empty — localized titles cannot be derived from it"
                        .to_string(),
                );
            } else {
                let derived: std::collections::BTreeMap<String, String> =
                    std::collections::BTreeMap::from([
                        ("zh".to_string(), manifest.display_name.clone()),
                        ("en".to_string(), manifest.display_name.clone()),
                    ]);
                if let Some(menu) = &mut manifest.menu {
                    if menu.titles.is_empty() {
                        menu.titles = derived.clone();
                    }
                }
                for route in &mut manifest.routes {
                    if route.titles.is_empty() {
                        route.titles = derived.clone();
                    }
                }
                manifest_modified = true;
                upgrades.push(UpgradeItem {
                    kind: UpgradeKind::DeriveTitles,
                    detail: format!(
                        "derive zh/en menu and route titles from the display name \"{}\"",
                        manifest.display_name
                    ),
                });
            }
        }
    }

    // Missing scaffold files (existence check only — generation re-checks
    // and never overwrites; the double check keeps the list and the action
    // consistent under a racing filesystem)
    if !root.join("scripts").join("superlive.build").exists() {
        upgrades.push(UpgradeItem {
            kind: UpgradeKind::AddBuildScript,
            detail: "generate the missing build entry scripts/superlive.build".to_string(),
        });
    }
    if !root.join("README.md").exists() {
        upgrades.push(UpgradeItem {
            kind: UpgradeKind::AddReadme,
            detail: "generate the missing README.md".to_string(),
        });
    }

    // -- declaration validation (over the corrected in-memory manifest) --
    if let Err(e) = manifest.validate_declaration() {
        unfixable.push(format!("declaration validation failed: {e}"));
    }

    // -- the version-compatibility gate --
    if !manifest.min_shell_version.is_empty()
        && !shell_version_satisfied(&manifest.min_shell_version)
    {
        unfixable.push(if parse_semver(&manifest.min_shell_version).is_none() {
            format!(
                "min_shell_version \"{}\" is not a valid major.minor.patch version",
                manifest.min_shell_version
            )
        } else {
            format!(
                "min_shell_version {} is newer than this shell ({})",
                manifest.min_shell_version,
                env!("CARGO_PKG_VERSION")
            )
        });
    }

    Ok(Inspection {
        manifest,
        shape,
        upgrades,
        unfixable,
        manifest_modified,
    })
}

/// Reads the on-disk registry and returns a conflict reason when the
/// imported project's id or namespace collides (the same uniqueness rules
/// and wording as the creation flow's pre-flight).
async fn find_collision(
    state: &AppState,
    manifest: &ComponentManifest,
) -> Result<Option<CreateError>, CreateError> {
    let existing = registry::load_all_entries(&state.layout)
        .await
        .map_err(|e| {
            CreateError::Internal(format!("failed to read the component registry: {e}"))
        })?;
    if existing.iter().any(|e| e.manifest.id == manifest.id) {
        return Ok(Some(CreateError::Conflict(format!(
            "component id is already registered: {}",
            manifest.id
        ))));
    }
    if existing
        .iter()
        .any(|e| e.manifest.namespace == manifest.namespace)
    {
        return Ok(Some(CreateError::Conflict(format!(
            "the namespace is already in use: {}",
            manifest.namespace
        ))));
    }
    Ok(None)
}

/// The import command's core (the tauri wrapper stays a thin translation
/// shell, same split as `create_component` / `create_component_core`).
pub(crate) async fn import_core(
    state: &AppState,
    dir: &str,
    apply_upgrades: bool,
) -> Result<ImportOutcome, CreateError> {
    let root = PathBuf::from(dir);

    // Phase 1 — inspect (synchronous IO inside one blocking hop; no disk
    // mutation happens here)
    let root_for_inspect = root.clone();
    let inspection = tokio::task::spawn_blocking(move || inspect(&root_for_inspect))
        .await
        .map_err(|_| {
            CreateError::Internal("the import inspection task terminated unexpectedly".into())
        })??;

    // Unfixable findings reject with the joined reasons (the frontend
    // surfaces them verbatim; the user may re-pick a directory)
    if !inspection.unfixable.is_empty() {
        return Err(CreateError::InvalidArgument(
            inspection.unfixable.join("; "),
        ));
    }

    // Registration-key collisions are unfixable findings per the contract —
    // checked after inspection so a malformed manifest reports its own
    // reason first
    if let Some(conflict) = find_collision(state, &inspection.manifest).await? {
        return Err(conflict);
    }

    // Upgradeable findings without the apply flag → the confirmation list
    // (a normal flow, not an error)
    if !inspection.upgrades.is_empty() && !apply_upgrades {
        return Ok(ImportOutcome::UpgradeRequired {
            items: inspection.upgrades.clone(),
        });
    }

    // Phase 2 — apply: generate only-missing scaffold files, write the
    // corrected declaration back (only when corrected), then register.
    let has_upgrades = !inspection.upgrades.is_empty();
    let manifest_modified = inspection.manifest_modified;
    let project_dir = root.clone();
    let manifest = inspection.manifest.clone();
    let root_for_files = root.clone();
    let (missing_req, files) = gap_fill_request(&inspection);
    tokio::task::spawn_blocking(move || -> Result<(), CreateError> {
        if has_upgrades {
            slscaffold::fill_missing_files(&missing_req, &root_for_files, &files).map_err(|e| {
                CreateError::Internal(format!(
                    "failed to generate the missing scaffold files: {e}"
                ))
            })?;
        }
        if manifest_modified {
            registry::write_declaration_sync(&project_dir, &manifest).map_err(|e| {
                CreateError::Internal(format!("failed to write back the corrected manifest: {e}"))
            })?;
        }
        Ok(())
    })
    .await
    .map_err(|_| CreateError::Internal("the import apply task terminated unexpectedly".into()))??;

    // Registration tail — identical to the creation flow's (dev envelope +
    // snapshot + settled config state). The collision re-check right before
    // the write is the apply-phase backstop for a racing registration.
    if let Some(conflict) = find_collision(state, &inspection.manifest).await? {
        return Err(conflict);
    }
    let entry = RegistryEntry {
        source: ComponentSource::Dev,
        project_dir: Some(root.clone()),
        manifest: inspection.manifest,
        // Registration timestamp: captured once, at creation
        registered_at: Some(registry::now_ms()),
    };
    {
        let layout = state.layout.clone();
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || registry::persist_entry_sync(&layout, &entry))
            .await
            .map_err(|_| {
                CreateError::Internal("the persistence task terminated unexpectedly".into())
            })?
            .map_err(|e| {
                CreateError::Internal(format!("failed to persist the dev registration: {e}"))
            })?;
    }
    // Settled state lands the same way an install/creation does, so the
    // management page treats all sources uniformly
    let config_path = registry::config_path_for(&state.layout, &entry);
    config_plane::reconcile_state(&state.layout, &entry.manifest, config_path)
        .await
        .map_err(|e| CreateError::Internal(format!("failed to persist the config state: {e}")))?;

    log::info!(
        "dev component imported: {} (ns={} shape={} project={})",
        entry.manifest.id,
        entry.manifest.namespace,
        match inspection.shape {
            slscaffold::ComponentShape::FrontendAndBackend => "frontend-backend",
            slscaffold::ComponentShape::FrontendOnly => "frontend",
            slscaffold::ComponentShape::BackendOnly => "backend",
        },
        root.display()
    );

    let running = state
        .registry
        .lookup(&entry.manifest.namespace)
        .await
        .is_some();
    Ok(ImportOutcome::Registered {
        component: Box::new(ComponentView::from_entry(entry, running)),
        project_dir: root.to_string_lossy().into_owned(),
        next_steps: vec![format!(
            "build   : {}/scripts/superlive.build (pwsh -File on Windows) — builds every plane; release packaging is the shell's managed flow",
            root.display()
        )],
    })
}

/// The gap-filling request for the apply phase: identity from the corrected
/// manifest, shape from the detection, and the file set matching the scan
/// findings (generation itself re-checks existence and never overwrites).
fn gap_fill_request(inspection: &Inspection) -> (MissingFilesRequest, Vec<MissingFile>) {
    let req = MissingFilesRequest {
        id: inspection.manifest.id.clone(),
        display_name: inspection.manifest.display_name.clone(),
        namespace: inspection.manifest.namespace.clone(),
        shape: inspection.shape,
    };
    let mut files = Vec::new();
    if inspection
        .upgrades
        .iter()
        .any(|u| u.kind == UpgradeKind::AddBuildScript)
    {
        files.push(MissingFile::BuildScript);
    }
    if inspection
        .upgrades
        .iter()
        .any(|u| u.kind == UpgradeKind::AddReadme)
    {
        files.push(MissingFile::Readme);
    }
    (req, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fully compliant project manifest: localized titles, version aligned
    /// with the project's version source, a min_shell_version older than any
    /// real shell.
    fn compliant_manifest_json(id: &str, ns: &str, display: &str, version: &str) -> String {
        format!(
            r#"{{
                "id": "{id}",
                "version": "{version}",
                "min_shell_version": "0.0.1",
                "namespace": "{ns}",
                "display_name": "{display}",
                "ui": {{ "ui_type": "module", "entry": "entry.js" }},
                "menu": {{
                    "title": "{display}",
                    "titles": {{ "zh": "{display}", "en": "{display}" }}
                }},
                "routes": [
                    {{ "path": "/", "title": "{display}", "titles": {{ "zh": "{display}", "en": "{display}" }} }}
                ],
                "launch": {{ "command": "./target/release/{id}-server", "args": [] }},
                "files": []
            }}"#
        )
    }

    /// A manifest with every upgrade finding: empty titles, a drifted
    /// version, an empty min_shell_version.
    fn upgradable_manifest_json(id: &str, ns: &str, display: &str) -> String {
        format!(
            r#"{{
                "id": "{id}",
                "version": "9.9.9",
                "min_shell_version": "",
                "namespace": "{ns}",
                "display_name": "{display}",
                "ui": {{ "ui_type": "module", "entry": "entry.js" }},
                "menu": {{ "title": "{display}" }},
                "routes": [ {{ "path": "/", "title": "{display}" }} ],
                "launch": {{ "command": "./target/release/{id}-server", "args": [] }},
                "files": []
            }}"#
        )
    }

    /// Assembles a project directory: the manifest plus the shape directories
    /// and version sources the shape implies.
    fn make_project(
        root: &Path,
        manifest_json: &str,
        with_server: bool,
        with_web: bool,
    ) -> PathBuf {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("manifest.json"), manifest_json).unwrap();
        if with_server {
            std::fs::create_dir_all(root.join("server/src")).unwrap();
            std::fs::write(
                root.join("server/Cargo.toml"),
                "[package]\nname = \"x\"\nversion = \"0.2.0\"\nedition = \"2021\"\n",
            )
            .unwrap();
        }
        if with_web {
            std::fs::create_dir_all(root.join("web/src")).unwrap();
            std::fs::write(
                root.join("web/package.json"),
                r#"{ "name": "x", "version": "0.2.0" }"#,
            )
            .unwrap();
        }
        root.to_path_buf()
    }

    fn error_kind_of(err: &CreateError) -> &'static str {
        err.kind()
    }

    fn error_message_of(err: &CreateError) -> String {
        err.message().to_string()
    }

    #[tokio::test]
    async fn compliant_project_registers_without_touching_the_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let project = make_project(
            &dir.path().join("proj"),
            &compliant_manifest_json("fullapp", "fullns", "Full App", "0.2.0"),
            true,
            true,
        );
        // A compliant project carries its scaffold files — no gap-filling
        // findings of any kind.
        std::fs::create_dir_all(project.join("scripts")).unwrap();
        std::fs::write(project.join("scripts/superlive.build"), "#!/bin/sh\n").unwrap();
        std::fs::write(project.join("README.md"), "# Full App\n").unwrap();
        let before = std::fs::read_to_string(project.join("manifest.json")).unwrap();

        let outcome = import_core(&state, &project.to_string_lossy(), false)
            .await
            .unwrap();
        let ImportOutcome::Registered {
            component,
            project_dir,
            next_steps,
        } = outcome
        else {
            panic!("expected Registered, got {outcome:?}");
        };
        assert_eq!(component.id, "fullapp");
        assert_eq!(component.source, ComponentSource::Dev);
        assert_eq!(project_dir, project.to_string_lossy());
        assert!(next_steps.iter().all(|s| s.contains("superlive.build")));

        // The registration landed on disk (dev envelope)
        let entry = registry::load_entry(&state.layout, "fullapp")
            .await
            .unwrap()
            .expect("the dev envelope must be persisted");
        assert_eq!(entry.source, ComponentSource::Dev);
        assert_eq!(
            entry.project_dir.as_deref(),
            Some(project.as_path()),
            "the envelope points at the imported project"
        );
        assert!(entry.registered_at.is_some());

        // A compliant project's manifest.json is the author's file — byte-identical
        assert_eq!(
            std::fs::read_to_string(project.join("manifest.json")).unwrap(),
            before,
            "a compliant import must not rewrite the manifest"
        );
    }

    #[tokio::test]
    async fn upgradable_project_lists_findings_without_applying() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let project = make_project(
            &dir.path().join("proj"),
            &upgradable_manifest_json("upapp", "upns", "Up App"),
            true,
            true,
        );
        let before = std::fs::read_to_string(project.join("manifest.json")).unwrap();

        let outcome = import_core(&state, &project.to_string_lossy(), false)
            .await
            .unwrap();
        let ImportOutcome::UpgradeRequired { items } = outcome else {
            panic!("expected UpgradeRequired, got {outcome:?}");
        };
        let kinds: Vec<UpgradeKind> = items.iter().map(|i| i.kind).collect();
        for expected in [
            UpgradeKind::FillMinShellVersion,
            UpgradeKind::RefreshVersion,
            UpgradeKind::DeriveTitles,
            UpgradeKind::AddBuildScript,
            UpgradeKind::AddReadme,
        ] {
            assert!(
                kinds.contains(&expected),
                "the list must contain {expected:?}: {kinds:?}"
            );
        }
        // The version item names the authoritative value
        let version_item = items
            .iter()
            .find(|i| i.kind == UpgradeKind::RefreshVersion)
            .unwrap();
        assert!(version_item.detail.contains("0.2.0"));

        // Nothing landed: the manifest is byte-identical and no files appeared
        assert_eq!(
            std::fs::read_to_string(project.join("manifest.json")).unwrap(),
            before,
            "the inspect phase must not write anything"
        );
        assert!(!project.join("scripts/superlive.build").exists());
        assert!(!project.join("README.md").exists());
    }

    #[tokio::test]
    async fn applying_upgrades_corrects_the_manifest_and_generates_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let project = make_project(
            &dir.path().join("proj"),
            &upgradable_manifest_json("fixapp", "fixns", "Fix App"),
            true,
            true,
        );

        let outcome = import_core(&state, &project.to_string_lossy(), true)
            .await
            .unwrap();
        assert!(matches!(outcome, ImportOutcome::Registered { .. }));

        // The written-back declaration carries every correction
        let corrected: ComponentManifest =
            serde_json::from_str(&std::fs::read_to_string(project.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(
            corrected.version, "0.2.0",
            "version refreshed from the source"
        );
        assert_eq!(
            corrected.min_shell_version,
            env!("CARGO_PKG_VERSION"),
            "min_shell_version filled with the shell version"
        );
        let menu = corrected.menu.as_ref().unwrap();
        assert_eq!(menu.titles.get("zh").map(String::as_str), Some("Fix App"));
        assert_eq!(menu.titles.get("en").map(String::as_str), Some("Fix App"));
        assert!(
            corrected.routes.iter().all(|r| r.titles.len() == 2),
            "route titles derived as well"
        );
        corrected
            .validate_declaration()
            .expect("the corrected declaration must validate");

        // The missing scaffold files were generated
        assert!(project.join("scripts/superlive.build").is_file());
        assert!(project.join("README.md").is_file());
        let build_script =
            std::fs::read_to_string(project.join("scripts/superlive.build")).unwrap();
        assert!(
            build_script.contains("fixapp-server"),
            "the build script knows the id"
        );
        let readme = std::fs::read_to_string(project.join("README.md")).unwrap();
        assert!(
            !readme.contains("scaffolded by"),
            "the imported README variant"
        );

        // And the registration exists with the corrected manifest
        let entry = registry::load_entry(&state.layout, "fixapp")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.manifest.version, "0.2.0");
    }

    #[tokio::test]
    async fn existing_gap_files_survive_the_apply() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let project = make_project(
            &dir.path().join("proj"),
            &upgradable_manifest_json("keepapp", "keepns", "Keep App"),
            true,
            true,
        );
        // Pre-place customized files — the conservative red line
        std::fs::create_dir_all(project.join("scripts")).unwrap();
        std::fs::write(
            project.join("scripts/superlive.build"),
            "# my custom build\n",
        )
        .unwrap();
        std::fs::write(project.join("README.md"), "# my own readme\n").unwrap();

        import_core(&state, &project.to_string_lossy(), true)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(project.join("scripts/superlive.build")).unwrap(),
            "# my custom build\n",
            "an existing build script must never be overwritten"
        );
        assert_eq!(
            std::fs::read_to_string(project.join("README.md")).unwrap(),
            "# my own readme\n",
            "an existing README must never be overwritten"
        );
    }

    #[tokio::test]
    async fn unparsable_manifest_is_rejected_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join("web")).unwrap();
        std::fs::write(project.join("manifest.json"), "{ not json").unwrap();

        let err = import_core(&state, &project.to_string_lossy(), false)
            .await
            .unwrap_err();
        assert_eq!(error_kind_of(&err), "invalid_argument");
        assert!(error_message_of(&err).contains("parse"));
    }

    #[tokio::test]
    async fn missing_manifest_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let project = dir.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();

        let err = import_core(&state, &project.to_string_lossy(), false)
            .await
            .unwrap_err();
        assert_eq!(error_kind_of(&err), "invalid_argument");
        assert!(error_message_of(&err).contains("no manifest.json"));
    }

    #[tokio::test]
    async fn newer_min_shell_version_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let json = compliant_manifest_json("newapp", "newns", "New App", "0.2.0").replace(
            "\"min_shell_version\": \"0.0.1\"",
            "\"min_shell_version\": \"999.0.0\"",
        );
        let project = make_project(&dir.path().join("proj"), &json, true, true);

        let err = import_core(&state, &project.to_string_lossy(), false)
            .await
            .unwrap_err();
        assert_eq!(error_kind_of(&err), "invalid_argument");
        assert!(error_message_of(&err).contains("newer than this shell"));
    }

    #[tokio::test]
    async fn shape_contradiction_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        // Declares a launch but the server/ directory is absent
        let project = make_project(
            &dir.path().join("proj"),
            &compliant_manifest_json("ghostapp", "ghostns", "Ghost App", "0.2.0"),
            false,
            true,
        );

        let err = import_core(&state, &project.to_string_lossy(), false)
            .await
            .unwrap_err();
        assert_eq!(error_kind_of(&err), "invalid_argument");
        assert!(error_message_of(&err).contains("no server/ directory"));
    }

    #[tokio::test]
    async fn unreadable_version_source_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        // server/ exists but carries no Cargo.toml — the version authority is gone
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join("server")).unwrap();
        std::fs::write(
            project.join("manifest.json"),
            compliant_manifest_json("srcapp", "srcns", "Src App", "0.2.0"),
        )
        .unwrap();

        let err = import_core(&state, &project.to_string_lossy(), false)
            .await
            .unwrap_err();
        assert_eq!(error_kind_of(&err), "invalid_argument");
        assert!(error_message_of(&err).contains("version source"));
    }

    /// Pre-registers a minimal dev envelope for the collision tests (the same
    /// helper shape as the creation tests' `write_envelope`).
    fn write_envelope(state: &AppState, id: &str, ns: &str) {
        std::fs::create_dir_all(state.layout.registry_dir()).unwrap();
        let entry = RegistryEntry {
            source: ComponentSource::Dev,
            project_dir: Some(std::path::PathBuf::from("/elsewhere")),
            manifest: serde_json::from_str::<ComponentManifest>(&compliant_manifest_json(
                id, ns, "Other", "0.1.0",
            ))
            .unwrap(),
            registered_at: None,
        };
        std::fs::write(
            state.layout.registry_file(id),
            serde_json::to_string_pretty(&entry).unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn registered_id_is_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let project = make_project(
            &dir.path().join("proj"),
            &compliant_manifest_json("dupapp", "dupns", "Dup App", "0.2.0"),
            true,
            true,
        );
        write_envelope(&state, "dupapp", "otherns");

        let err = import_core(&state, &project.to_string_lossy(), false)
            .await
            .unwrap_err();
        assert_eq!(error_kind_of(&err), "conflict");
        assert!(error_message_of(&err).contains("already registered"));
    }

    #[tokio::test]
    async fn registered_namespace_is_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path().join("data")).unwrap();
        let project = make_project(
            &dir.path().join("proj"),
            &compliant_manifest_json("nsapp", "busy-ns", "Ns App", "0.2.0"),
            true,
            true,
        );
        write_envelope(&state, "otherapp", "busy-ns");

        let err = import_core(&state, &project.to_string_lossy(), false)
            .await
            .unwrap_err();
        assert_eq!(error_kind_of(&err), "conflict");
        assert!(error_message_of(&err).contains("namespace is already in use"));
    }
}
