//! superlive component project scaffolding.
//!
//! Renders the standard component skeleton (Rust backend / React web / build
//! script / README) from templates embedded at compile time, in three shapes:
//!
//! - [`ComponentShape::FrontendAndBackend`] — server + web + build script + README
//! - [`ComponentShape::FrontendOnly`] — web + build script + README (no server/)
//! - [`ComponentShape::BackendOnly`] — server + build script + README (no web/)
//!
//! The generated project directory is a plain filesystem tree: the dev manifest
//! assembly and the shell-side registration are the caller's responsibility (the
//! scaffold only initializes the project directory).
//!
//! Shared-library consumption (`slsdk-rs` / `slsdk-ts`) follows
//! [`SdkDependency`]: registry versions (crates.io / npm) or path/file
//! dependencies against a local superlive checkout for developing against
//! unreleased SDK changes.
//!
//! Templates are conditionalized with line markers (`#@if EXPR` / `#@endif`,
//! recognized only as whole trimmed lines; `#` is a comment in both bash and
//! PowerShell, so retained markers stay syntactically inert). The stripping
//! logic lives in [`render`]; it is the single adaptation point for all three
//! shapes — no duplicated per-shape template variants exist.

mod render;

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Embedded template assets (single source of truth for generated content).
mod templates {
    pub const README: &str = include_str!("templates/README.md.tpl");
    pub const SERVER_CARGO_TOML: &str = include_str!("templates/server_Cargo.toml.tpl");
    pub const SERVER_MAIN_RS: &str = include_str!("templates/server_main.rs.tpl");
    /// TypeScript backend entry shared by the [`BackendTemplate::Deno`] and
    /// [`BackendTemplate::Bun`] templates (the runtime difference lives in the
    /// build script and README, not in the source file).
    pub const SERVER_SERVER_TS: &str = include_str!("templates/server_server_ts.tpl");
    /// Dependency root for the TS server plane: `deno`/`bun` resolve
    /// `slsdk-ts` through the nearest `package.json` ancestor chain of
    /// `server/server.ts`, so the plane carries its own root (same
    /// `__SLSDK_TS_LINE__` rendering as the web plane's manifest).
    pub const SERVER_PACKAGE_JSON: &str = include_str!("templates/server_package_json.tpl");
    pub const WEB_PACKAGE_JSON: &str = include_str!("templates/web_package.json.tpl");
    pub const WEB_TSCONFIG_JSON: &str = include_str!("templates/web_tsconfig.json.tpl");
    pub const WEB_VITE_CONFIG_JS: &str = include_str!("templates/web_vite.config.js.tpl");
    pub const WEB_INDEX_HTML: &str = include_str!("templates/web_index.html.tpl");
    pub const WEB_MAIN_TSX: &str = include_str!("templates/web_main.tsx.tpl");
    pub const WEB_ENTRY_TSX: &str = include_str!("templates/web_entry.tsx.tpl");
    pub const WEB_APP_TSX: &str = include_str!("templates/web_App.tsx.tpl");
    pub const WEB_I18N_TS: &str = include_str!("templates/web_i18n.ts.tpl");
    /// Build-script dialect for POSIX hosts. `file_plan` selects the host
    /// platform's dialect into the single `scripts/superlive.build` output, so
    /// the dialect of the *other* platform stays unreferenced on any given
    /// target (its content is verified by `build_script_tests` on every target).
    #[allow(dead_code)]
    pub const BUILD_SCRIPT_SH: &str = include_str!("templates/superlive_build.sh.tpl");
    /// Build-script dialect for Windows hosts (see [`BUILD_SCRIPT_SH`]).
    #[allow(dead_code)]
    pub const BUILD_SCRIPT_PS1: &str = include_str!("templates/superlive_build.ps1.tpl");
}

/// Component build shape: which planes the generated project contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentShape {
    /// Rust backend + React frontend.
    FrontendAndBackend,
    /// React frontend only (manifest `launch = None`).
    FrontendOnly,
    /// Rust backend only (manifest `ui = None`; no UI mount surface).
    BackendOnly,
}

/// Backend runtime template for server shapes: which runtime the generated
/// backend skeleton targets (architecture.md "Plane B contract": any language).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendTemplate {
    /// Rust backend: `server/Cargo.toml` + `server/src/main.rs` on slsdk-rs
    /// (the original template shape).
    #[default]
    Cargo,
    /// TypeScript backend on the Deno runtime: `server/server.ts` on
    /// slsdk-ts's server entry.
    Deno,
    /// TypeScript backend on the Bun runtime: `server/server.ts` on
    /// slsdk-ts's server entry.
    Bun,
}

/// How the generated project consumes the contract SDKs (`slsdk-rs` / `slsdk-ts`).
#[derive(Debug, Clone)]
pub enum SdkDependency {
    /// Registry versions: `slsdk-rs = "0.2"` (crates.io) and
    /// `"slsdk-ts": "^0.2.0"` (npm) — the standard consumption path.
    Registry,
    /// Path/file dependencies against a local superlive checkout, for developing
    /// against unreleased SDK changes. The checkout must contain `slsdk-rs/` and
    /// a built `slsdk-ts/dist/index.js`.
    Local { superlive_root: PathBuf },
}

/// One localized-title row: the locale code and its title value, kept in the
/// caller's input order (the generated i18n module emits one registration
/// line per row, in this order).
#[derive(Debug)]
pub struct LocalizedTitle {
    /// Locale code of the row (e.g. `zh` / `en` / `ja`).
    pub locale: String,
    /// Title value for the locale.
    pub value: String,
}

/// Scaffold request: identity, target location, shape, and SDK consumption mode.
#[derive(Debug)]
pub struct ScaffoldRequest {
    /// Component unique id (letters / digits / hyphens). Used as the crate/bin
    /// name suffix, the install directory name, and the `/comps/{id}` URL key.
    pub id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Localized titles for the generated project's i18n module — one row per
    /// locale configured at creation, injected verbatim (input order
    /// preserved) so every locale is pre-wired in the generated registration
    /// (architecture.md *Localized titles*). Validated by [`scaffold`]: the
    /// set is non-empty, every row carries a non-empty locale and value, and
    /// locales are unique.
    pub display_titles: Vec<LocalizedTitle>,
    /// Namespace registered at the shell gateway (`/api/{ns}/{rest}`; same
    /// charset as [`ScaffoldRequest::id`]). Defaulting logic lives with the caller.
    pub namespace: String,
    /// Project root to create. Must not exist, or must be an existing empty
    /// directory.
    pub target_dir: PathBuf,
    /// Which planes to generate.
    pub shape: ComponentShape,
    /// Which backend runtime the server plane targets (frontend-only projects
    /// ignore it; the default matches the original Rust-only template).
    pub template: BackendTemplate,
    /// Fixed dev port written into the vite dev proxy and the README recipe.
    pub dev_port: u16,
    /// SDK consumption mode.
    pub sdk: SdkDependency,
}

/// Scaffold outcome: the project root plus every file created under it.
#[derive(Debug)]
pub struct ScaffoldResult {
    /// Absolute path of the created project root.
    pub root: PathBuf,
    /// Absolute paths of all created files (in generation order).
    pub created: Vec<PathBuf>,
    /// Post-scaffold hints (dev commands, build command), shaped by
    /// [`ComponentShape`].
    pub next_steps: Vec<String>,
}

/// Scaffold failures.
#[derive(Debug)]
pub enum ScaffoldError {
    /// id contains characters outside the legal set (letters / digits / hyphens)
    /// or is empty.
    InvalidId(String),
    /// namespace fails the same charset rule as id.
    InvalidNamespace(String),
    /// dev port 0 (no meaningful vite proxy target).
    InvalidDevPort(u16),
    /// display_titles violates the input contract (an empty set, an empty
    /// locale or value, or a duplicate locale); the payload names the
    /// violated rule.
    InvalidDisplayTitles(&'static str),
    /// target_dir exists as a file or as a non-empty directory.
    TargetExists { path: PathBuf },
    /// Local SDK dependency validation failed (`reason` says what to do).
    SdkMissing { path: PathBuf, reason: &'static str },
    /// Filesystem error while creating the project tree.
    Io { path: PathBuf, source: io::Error },
    /// An embedded template asset is malformed (broken condition markers) — a
    /// build-time asset bug, not a caller error.
    InvalidTemplate {
        template: &'static str,
        reason: &'static str,
    },
}

impl fmt::Display for ScaffoldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScaffoldError::InvalidId(id) => write!(
                f,
                "invalid id (only letters, digits, and hyphens are allowed): {id}"
            ),
            ScaffoldError::InvalidNamespace(ns) => write!(
                f,
                "invalid namespace (only letters, digits, and hyphens are allowed): {ns}"
            ),
            ScaffoldError::InvalidDevPort(port) => {
                write!(f, "invalid dev port: {port} (must be at least 1)")
            }
            ScaffoldError::InvalidDisplayTitles(reason) => {
                write!(f, "invalid display titles: {reason}")
            }
            ScaffoldError::TargetExists { path } => write!(
                f,
                "target directory exists and is not empty (or is not a directory): {}",
                path.display()
            ),
            ScaffoldError::SdkMissing { path, reason } => {
                write!(f, "{}: {} ({reason})", path.display(), reason)
            }
            ScaffoldError::Io { path, source } => {
                write!(f, "io error on {}: {source}", path.display())
            }
            ScaffoldError::InvalidTemplate { template, reason } => {
                write!(f, "malformed template asset {template}: {reason}")
            }
        }
    }
}

impl std::error::Error for ScaffoldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ScaffoldError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// One entry of the generation plan: which template lands where, and with which
/// post-render treatment.
struct FilePlan {
    template: &'static str,
    rel_path: &'static str,
    /// Strip `#@if` / `#@endif` conditional blocks (only the README and the
    /// build script carry shape conditionals).
    conditional: bool,
    /// Mark the generated file executable (the bash build-script dialect on
    /// unix; PowerShell scripts carry no exec-bit semantics).
    exec_bit: bool,
}

fn file_plan(shape: ComponentShape, template: BackendTemplate) -> Vec<FilePlan> {
    let mut plan = Vec::new();
    if matches!(
        shape,
        ComponentShape::FrontendAndBackend | ComponentShape::BackendOnly
    ) {
        match template {
            BackendTemplate::Cargo => {
                plan.push(FilePlan {
                    template: templates::SERVER_CARGO_TOML,
                    rel_path: "server/Cargo.toml",
                    conditional: false,
                    exec_bit: false,
                });
                plan.push(FilePlan {
                    template: templates::SERVER_MAIN_RS,
                    rel_path: "server/src/main.rs",
                    conditional: false,
                    exec_bit: false,
                });
            }
            // One shared TS entry for both TS runtimes — the runtime difference
            // lives in the build script and README (superlive.build compiles
            // with deno or bun; the README documents the runtime requirement).
            // server/package.json is the plane's dependency root: deno/bun
            // resolve slsdk-ts through the entry's nearest package.json
            // ancestor chain.
            BackendTemplate::Deno | BackendTemplate::Bun => {
                plan.push(FilePlan {
                    template: templates::SERVER_SERVER_TS,
                    rel_path: "server/server.ts",
                    conditional: false,
                    exec_bit: false,
                });
                plan.push(FilePlan {
                    template: templates::SERVER_PACKAGE_JSON,
                    rel_path: "server/package.json",
                    conditional: false,
                    exec_bit: false,
                });
            }
        }
    }
    if matches!(
        shape,
        ComponentShape::FrontendAndBackend | ComponentShape::FrontendOnly
    ) {
        plan.push(FilePlan {
            template: templates::WEB_PACKAGE_JSON,
            rel_path: "web/package.json",
            conditional: false,
            exec_bit: false,
        });
        plan.push(FilePlan {
            template: templates::WEB_TSCONFIG_JSON,
            rel_path: "web/tsconfig.json",
            conditional: false,
            exec_bit: false,
        });
        plan.push(FilePlan {
            template: templates::WEB_VITE_CONFIG_JS,
            rel_path: "web/vite.config.js",
            conditional: false,
            exec_bit: false,
        });
        plan.push(FilePlan {
            template: templates::WEB_INDEX_HTML,
            rel_path: "web/index.html",
            conditional: false,
            exec_bit: false,
        });
        plan.push(FilePlan {
            template: templates::WEB_MAIN_TSX,
            rel_path: "web/src/main.tsx",
            conditional: false,
            exec_bit: false,
        });
        plan.push(FilePlan {
            template: templates::WEB_ENTRY_TSX,
            rel_path: "web/src/entry.tsx",
            conditional: false,
            exec_bit: false,
        });
        plan.push(FilePlan {
            template: templates::WEB_APP_TSX,
            rel_path: "web/src/App.tsx",
            conditional: true,
            exec_bit: false,
        });
        plan.push(FilePlan {
            template: templates::WEB_I18N_TS,
            rel_path: "web/src/i18n.ts",
            conditional: false,
            exec_bit: false,
        });
    }
    // Single-file build entry `scripts/superlive.build`: the dialect follows the
    // host platform — bash on unix (executable), PowerShell on windows (run via
    // pwsh, no exec-bit semantics). The cfg blocks are inline so the platform
    // selection is visible at the exact construction site.
    #[cfg(unix)]
    plan.push(FilePlan {
        template: templates::BUILD_SCRIPT_SH,
        rel_path: "scripts/superlive.build",
        conditional: true,
        exec_bit: true,
    });
    #[cfg(windows)]
    plan.push(FilePlan {
        template: templates::BUILD_SCRIPT_PS1,
        rel_path: "scripts/superlive.build",
        conditional: true,
        exec_bit: false,
    });
    plan.push(FilePlan {
        template: templates::README,
        rel_path: "README.md",
        conditional: true,
        exec_bit: false,
    });
    plan
}

/// Validates the id/namespace charset: non-empty, ASCII letters / digits /
/// hyphens only.
fn validate_ident(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Validates the localized-title rows: a non-empty set, every row carrying a
/// non-empty locale and value, locales unique (each row becomes one i18n
/// registration line — a duplicate locale would emit conflicting resources).
fn validate_display_titles(titles: &[LocalizedTitle]) -> Result<(), ScaffoldError> {
    if titles.is_empty() {
        return Err(ScaffoldError::InvalidDisplayTitles(
            "at least one locale row is required",
        ));
    }
    let mut seen = HashSet::new();
    for row in titles {
        if row.locale.trim().is_empty() {
            return Err(ScaffoldError::InvalidDisplayTitles(
                "a row carries an empty locale",
            ));
        }
        if row.value.trim().is_empty() {
            return Err(ScaffoldError::InvalidDisplayTitles(
                "a row carries an empty title value",
            ));
        }
        if !seen.insert(row.locale.trim()) {
            return Err(ScaffoldError::InvalidDisplayTitles(
                "locales must be unique",
            ));
        }
    }
    Ok(())
}

/// Resolves `path` to an absolute path without requiring the final segment to
/// exist: an existing path is canonicalized; a missing path is anchored on its
/// canonicalized parent.
fn absolutize(path: &Path) -> Result<PathBuf, ScaffoldError> {
    if let Ok(canon) = path.canonicalize() {
        return Ok(canon);
    }
    let file_name = path.file_name().ok_or_else(|| ScaffoldError::Io {
        path: path.to_path_buf(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"),
    })?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = parent.canonicalize().map_err(|source| ScaffoldError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    Ok(parent.join(file_name))
}

/// Forward-slash form of a path for generated dependency lines (safe on every
/// platform: cargo and pnpm both accept `/` separators).
fn forward_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Renders all planned files into memory (pure: a render failure leaves the
/// filesystem untouched).
fn render_plan(
    req: &ScaffoldRequest,
    root: &Path,
) -> Result<Vec<(PathBuf, String, bool)>, ScaffoldError> {
    let has_server = matches!(
        req.shape,
        ComponentShape::FrontendAndBackend | ComponentShape::BackendOnly
    );
    let has_web = matches!(
        req.shape,
        ComponentShape::FrontendAndBackend | ComponentShape::FrontendOnly
    );
    let conds: [(&str, bool); 6] = [
        ("HAS_SERVER", has_server),
        ("HAS_WEB", has_web),
        // Fresh creation always renders the non-imported README branch
        ("IMPORTED", false),
        ("BACKEND_CARGO", req.template == BackendTemplate::Cargo),
        ("BACKEND_DENO", req.template == BackendTemplate::Deno),
        ("BACKEND_BUN", req.template == BackendTemplate::Bun),
    ];

    let (rs_line, ts_line, dep_desc) = match &req.sdk {
        SdkDependency::Registry => (
            r#"slsdk-rs = "0.2""#.to_string(),
            r#""slsdk-ts": "^0.2.0""#.to_string(),
            "This project was generated with registry dependencies (slsdk-rs from crates.io, \
             slsdk-ts from npm) — the standard consumption path."
                .to_string(),
        ),
        SdkDependency::Local { superlive_root } => {
            let rs_dir = superlive_root.join("slsdk-rs");
            if !rs_dir.is_dir() {
                return Err(ScaffoldError::SdkMissing {
                    path: rs_dir,
                    reason: "does not exist (is this a superlive checkout?)",
                });
            }
            let ts_dist = superlive_root
                .join("slsdk-ts")
                .join("dist")
                .join("index.js");
            if !ts_dist.is_file() {
                return Err(ScaffoldError::SdkMissing {
                    path: ts_dist,
                    reason: "is missing — build the SDK first (cd slsdk-ts && pnpm install && pnpm build)",
                });
            }
            let rs_dir = forward_slashes(&rs_dir);
            let ts_dir = forward_slashes(&superlive_root.join("slsdk-ts"));
            (
                format!(r#"slsdk-rs = {{ path = "{rs_dir}" }}"#),
                format!(r#""slsdk-ts": "file:{ts_dir}""#),
                format!(
                    "This project was generated with path dependencies pointing at a local \
                     superlive checkout (`{}`) — development mode. Switch to registry versions \
                     (slsdk-rs / slsdk-ts on crates.io / npm) before publishing.",
                    forward_slashes(superlive_root)
                ),
            )
        }
    };

    let vars: [(&str, String); 8] = [
        ("ID", req.id.clone()),
        ("NAME", req.display_name.clone()),
        ("NS", req.namespace.clone()),
        ("DEV_PORT", req.dev_port.to_string()),
        ("SLSDK_RS_LINE", rs_line),
        ("SLSDK_TS_LINE", ts_line),
        ("DEP_MODE_DESC", dep_desc),
        ("TITLE_ROWS", render::title_rows(&req.display_titles)),
    ];

    let mut rendered = Vec::new();
    for entry in file_plan(req.shape, req.template) {
        let content = if entry.conditional {
            render::render(entry.template, &vars, &conds)?
        } else {
            render::substitute(entry.template, &vars)
        };
        rendered.push((root.join(entry.rel_path), content, entry.exec_bit));
    }
    Ok(rendered)
}

/// Post-scaffold hints, one line per command, shaped by [`ComponentShape`].
fn next_steps(req: &ScaffoldRequest, root: &Path) -> Vec<String> {
    let root = root.display();
    let port = req.dev_port;
    let mut steps = Vec::new();
    let backend = match req.template {
        BackendTemplate::Cargo => {
            format!("backend : SL_ENDPOINT=tcp:127.0.0.1:{port} cargo run --manifest-path {root}/server/Cargo.toml")
        }
        BackendTemplate::Deno => {
            format!(
                "backend : SL_ENDPOINT=tcp:127.0.0.1:{port} deno run --allow-net --allow-read --allow-env {root}/server/server.ts"
            )
        }
        BackendTemplate::Bun => {
            format!("backend : SL_ENDPOINT=tcp:127.0.0.1:{port} bun run {root}/server/server.ts")
        }
    };
    let frontend =
        format!("frontend: cd {root}/web && pnpm install && pnpm dev   → http://localhost:5173");
    match req.shape {
        ComponentShape::FrontendAndBackend => {
            steps.push(backend);
            steps.push(frontend);
        }
        ComponentShape::FrontendOnly => steps.push(frontend),
        ComponentShape::BackendOnly => steps.push(backend),
    }
    steps.push(format!(
        "build   : {root}/scripts/superlive.build (pwsh -File on Windows) — builds every plane; release packaging is the shell's managed flow"
    ));
    steps
}

/// Scaffolds a component project directory per `req`.
///
/// All validation and rendering happen before any filesystem mutation; a
/// validation or render failure leaves the target location untouched.
pub fn scaffold(req: &ScaffoldRequest) -> Result<ScaffoldResult, ScaffoldError> {
    if !validate_ident(&req.id) {
        return Err(ScaffoldError::InvalidId(req.id.clone()));
    }
    if !validate_ident(&req.namespace) {
        return Err(ScaffoldError::InvalidNamespace(req.namespace.clone()));
    }
    if req.dev_port == 0 {
        return Err(ScaffoldError::InvalidDevPort(req.dev_port));
    }
    validate_display_titles(&req.display_titles)?;

    if req.target_dir.exists() {
        let empty = req.target_dir.is_dir()
            && fs::read_dir(&req.target_dir)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false);
        if !empty {
            return Err(ScaffoldError::TargetExists {
                path: req.target_dir.clone(),
            });
        }
    }
    // The scaffold owns every directory it creates, parents included: a
    // missing parent chain is created up front (an absent parent used to
    // surface as a bare Io NotFound from absolutize's canonicalize — a
    // filesystem shape, not a caller error).
    if let Some(parent) = req.target_dir.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|source| ScaffoldError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
    }
    let root = absolutize(&req.target_dir)?;

    let files = render_plan(req, &root)?;

    let mut created = Vec::with_capacity(files.len());
    for (path, content, exec_bit) in files {
        write_generated_file(&path, &content, exec_bit)?;
        created.push(path);
    }

    let steps = next_steps(req, &root);
    Ok(ScaffoldResult {
        root,
        created,
        next_steps: steps,
    })
}

/// Writes one rendered file: creates the parent directories, writes the
/// content, and stamps the executable bit on unix when requested (the
/// bash build-script dialect). The single landing path shared by
/// [`scaffold`] and [`fill_missing_files`].
fn write_generated_file(path: &Path, content: &str, exec_bit: bool) -> Result<(), ScaffoldError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ScaffoldError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(path, content).map_err(|source| ScaffoldError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    #[cfg(unix)]
    if exec_bit {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)
            .map_err(|source| ScaffoldError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).map_err(|source| ScaffoldError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    #[cfg(not(unix))]
    let _ = exec_bit;
    Ok(())
}

/// Gap-filling request for an **imported** project (architecture.md
/// "Creation flow": upgrades are conservative and gap-filling only — an
/// existing file is never overwritten). The identity fields come from the
/// project's validated manifest; the caller guarantees they passed
/// declaration validation (no re-validation here).
pub struct MissingFilesRequest {
    /// Component id (the `__ID__` placeholder in the build script).
    pub id: String,
    /// Display name (the `__NAME__` placeholder).
    pub display_name: String,
    /// Namespace (README structure sections only; the imported README
    /// carries no dev-recipe section).
    pub namespace: String,
    /// Which planes the project carries (drives the HAS_SERVER / HAS_WEB
    /// template conditionals).
    pub shape: ComponentShape,
}

/// One gap-filling file of an imported project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingFile {
    /// `scripts/superlive.build` — the host platform's dialect, executable
    /// on unix (the same selection [`file_plan`] makes for fresh creation).
    BuildScript,
    /// `README.md` — the imported variant (no dev-recipe / SDK-dependency
    /// sections; those describe create-time choices the project already
    /// carries its own answers for).
    Readme,
}

impl MissingFile {
    /// The file's project-relative path.
    pub fn rel_path(self) -> &'static str {
        match self {
            MissingFile::BuildScript => "scripts/superlive.build",
            MissingFile::Readme => "README.md",
        }
    }
}

/// Renders one gap-filling file for an imported project (pure: no
/// filesystem access).
pub fn render_missing_file(
    req: &MissingFilesRequest,
    file: MissingFile,
) -> Result<String, ScaffoldError> {
    let has_server = matches!(
        req.shape,
        ComponentShape::FrontendAndBackend | ComponentShape::BackendOnly
    );
    let has_web = matches!(
        req.shape,
        ComponentShape::FrontendAndBackend | ComponentShape::FrontendOnly
    );
    let conds: [(&str, bool); 6] = [
        ("HAS_SERVER", has_server),
        ("HAS_WEB", has_web),
        ("IMPORTED", true),
        // Gap-filling targets imported projects, whose backend runtime is
        // whatever the project already carries — the fill-in files (build
        // script / README) describe the cargo shape, matching the pre-template
        // behavior for every import until a TS import path needs its own.
        ("BACKEND_CARGO", true),
        ("BACKEND_DENO", false),
        ("BACKEND_BUN", false),
    ];
    let vars: [(&str, String); 8] = [
        ("ID", req.id.clone()),
        ("NAME", req.display_name.clone()),
        ("NS", req.namespace.clone()),
        // The imported README carries no dev recipe — the port and SDK-mode
        // descriptions never reach a placeholder (substitute replaces tokens
        // it finds, so empty values here are inert)
        ("DEV_PORT", String::new()),
        ("SLSDK_RS_LINE", String::new()),
        ("SLSDK_TS_LINE", String::new()),
        ("DEP_MODE_DESC", String::new()),
        ("TITLE_ROWS", String::new()),
    ];
    let template: &'static str = match file {
        MissingFile::Readme => templates::README,
        MissingFile::BuildScript => {
            // Host-platform dialect, the same selection file_plan makes for
            // fresh creation
            #[cfg(unix)]
            let dialect = templates::BUILD_SCRIPT_SH;
            #[cfg(windows)]
            let dialect = templates::BUILD_SCRIPT_PS1;
            dialect
        }
    };
    let mut out = render::render(template, &vars, &conds)?;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// Renders the requested gap-filling files and lands **only the missing
/// ones** into the project root — a file that already exists is never
/// touched (the conservative-upgrade red line: user customizations are
/// untouchable). Returns the project-relative paths actually created, in
/// the caller's request order.
pub fn fill_missing_files(
    req: &MissingFilesRequest,
    root: &Path,
    files: &[MissingFile],
) -> Result<Vec<&'static str>, ScaffoldError> {
    let mut created = Vec::new();
    for file in files {
        let rel = file.rel_path();
        let target = root.join(rel);
        if target.exists() {
            continue;
        }
        let content = render_missing_file(req, *file)?;
        write_generated_file(&target, &content, *file == MissingFile::BuildScript)?;
        created.push(rel);
    }
    Ok(created)
}

/// Tests over both build-script dialects. The integration tests only see the
/// host platform's dialect on disk (unix here); the other dialect is verified
/// through the render path, so both dialects are checked on every target.
#[cfg(test)]
mod build_script_tests {
    use super::render::render;
    use super::templates::{BUILD_SCRIPT_PS1, BUILD_SCRIPT_SH};
    use super::{BackendTemplate, ComponentShape};

    fn conds(shape: ComponentShape, template: BackendTemplate) -> Vec<(&'static str, bool)> {
        let has_server = matches!(
            shape,
            ComponentShape::FrontendAndBackend | ComponentShape::BackendOnly
        );
        let has_web = matches!(
            shape,
            ComponentShape::FrontendAndBackend | ComponentShape::FrontendOnly
        );
        vec![
            ("HAS_SERVER", has_server),
            ("HAS_WEB", has_web),
            ("BACKEND_CARGO", template == BackendTemplate::Cargo),
            ("BACKEND_DENO", template == BackendTemplate::Deno),
            ("BACKEND_BUN", template == BackendTemplate::Bun),
        ]
    }

    fn render_dialect(
        dialect: &'static str,
        shape: ComponentShape,
        template: BackendTemplate,
    ) -> String {
        let vars = [
            ("ID", "demo-comp".to_string()),
            ("NAME", "Demo Comp".to_string()),
            ("NS", "demo".to_string()),
            ("DEV_PORT", "8700".to_string()),
            ("SLSDK_RS_LINE", String::new()),
            ("SLSDK_TS_LINE", String::new()),
            ("DEP_MODE_DESC", String::new()),
        ];
        render(dialect, &vars, &conds(shape, template))
            .expect("build-script templates are well-formed")
    }

    fn shapes() -> [ComponentShape; 3] {
        [
            ComponentShape::FrontendAndBackend,
            ComponentShape::FrontendOnly,
            ComponentShape::BackendOnly,
        ]
    }

    fn templates() -> [BackendTemplate; 3] {
        [
            BackendTemplate::Cargo,
            BackendTemplate::Deno,
            BackendTemplate::Bun,
        ]
    }

    #[test]
    fn build_scripts_carry_zero_packaging_semantics() {
        // Packaging (assembly / fingerprinting / manifest / zip) moved into the
        // shell: the build scripts must not carry any of its code, in either
        // dialect. The banned tokens are code-level markers of the old
        // packaging chain — the header's prose mention of "fingerprinting"
        // (describing what the shell owns) is documentation, not residue.
        let sh_banned = [
            "zip",
            "manifest",
            "staging",
            "sha256",
            "shasum",
            "ZIP_ENTRIES",
            "json_escape",
            "COMPONENT_ID",
            "NAMESPACE",
        ];
        let ps1_banned = [
            "ZipArchive",
            "Compress-Archive",
            "manifest",
            "Staging",
            "staging",
            "SHA256",
            "Get-FileHash",
            "ConvertTo-Json",
        ];
        for shape in shapes() {
            for template in templates() {
                let sh = render_dialect(BUILD_SCRIPT_SH, shape, template);
                for token in sh_banned {
                    assert!(
                        !sh.contains(token),
                        "sh {shape:?}/{template:?} carries {token:?}"
                    );
                }
                let ps1 = render_dialect(BUILD_SCRIPT_PS1, shape, template);
                for token in ps1_banned {
                    assert!(
                        !ps1.contains(token),
                        "ps1 {shape:?}/{template:?} carries {token:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn build_scripts_follow_the_shape_in_both_dialects() {
        let dialects: [(&str, &'static str); 2] =
            [("sh", BUILD_SCRIPT_SH), ("ps1", BUILD_SCRIPT_PS1)];
        for (name, tpl) in dialects {
            let full = render_dialect(
                tpl,
                ComponentShape::FrontendAndBackend,
                BackendTemplate::Cargo,
            );
            let frontend =
                render_dialect(tpl, ComponentShape::FrontendOnly, BackendTemplate::Cargo);
            let backend = render_dialect(tpl, ComponentShape::BackendOnly, BackendTemplate::Cargo);

            // FrontendOnly: no server plane anywhere, web build intact.
            assert!(!frontend.contains("cargo"), "{name} frontend carries cargo");
            assert!(
                frontend.contains("run build"),
                "{name} frontend lost the web build"
            );
            assert!(
                frontend.contains("web/dist/entry.js"),
                "{name} frontend lost the entry assertion"
            );

            // BackendOnly: no web plane anywhere, server build intact.
            assert!(!backend.contains("pnpm"), "{name} backend carries pnpm");
            assert!(
                !backend.contains("run build"),
                "{name} backend carries the web build"
            );
            assert!(
                backend.contains("cargo build --release"),
                "{name} backend lost the server build"
            );
            assert!(
                backend.contains("target/release/demo-comp-server"),
                "{name} backend lost the binary assertion"
            );

            // FrontendAndBackend: both planes, outputs asserted for each.
            assert!(full.contains("cargo build --release"));
            assert!(full.contains("run build"));
            assert!(full.contains("target/release/demo-comp-server"));
            assert!(full.contains("web/dist/entry.js"));
        }
    }

    #[test]
    fn build_scripts_follow_the_backend_template() {
        let dialects: [(&str, &'static str); 2] =
            [("sh", BUILD_SCRIPT_SH), ("ps1", BUILD_SCRIPT_PS1)];
        for (name, tpl) in dialects {
            let deno = render_dialect(tpl, ComponentShape::BackendOnly, BackendTemplate::Deno);
            assert!(
                deno.contains("deno compile --output"),
                "{name} deno backend lost the deno compile command"
            );
            assert!(
                deno.contains("server/server.ts"),
                "{name} deno backend lost the entry path"
            );
            assert!(
                deno.contains("bin/demo-comp-server"),
                "{name} deno backend lost the binary assertion"
            );
            assert!(
                !deno.contains("cargo build --release"),
                "{name} deno backend carries the cargo build"
            );
            assert!(
                deno.contains("https://deno.com"),
                "{name} deno backend lost the install guidance"
            );

            let bun = render_dialect(tpl, ComponentShape::BackendOnly, BackendTemplate::Bun);
            assert!(
                bun.contains("bun build --compile --outfile"),
                "{name} bun backend lost the bun build command"
            );
            assert!(
                bun.contains("server/server.ts"),
                "{name} bun backend lost the entry path"
            );
            assert!(
                bun.contains("bin/demo-comp-server"),
                "{name} bun backend lost the binary assertion"
            );
            assert!(
                !bun.contains("cargo build --release"),
                "{name} bun backend carries the cargo build"
            );
            assert!(
                bun.contains("https://bun.sh"),
                "{name} bun backend lost the install guidance"
            );

            // Exactly one server build section survives per template: the
            // cargo section only appears on the cargo template.
            let cargo = render_dialect(tpl, ComponentShape::BackendOnly, BackendTemplate::Cargo);
            assert!(
                !cargo.contains("deno compile") && !cargo.contains("bun build --compile"),
                "{name} cargo backend carries a TS runtime build"
            );
        }
    }

    #[test]
    fn ps1_cargo_artifact_assertion_carries_the_exe_suffix() {
        // Windows: cargo writes `<name>.exe` — the assertion must match the
        // artifact cargo actually produces (the pre-fix assertion without the
        // suffix could never pass on Windows).
        let ps1 = render_dialect(
            BUILD_SCRIPT_PS1,
            ComponentShape::BackendOnly,
            BackendTemplate::Cargo,
        );
        assert!(
            ps1.contains("server/target/release/demo-comp-server.exe"),
            "ps1 cargo assertion lost the .exe suffix"
        );
        assert!(
            !ps1.contains("'server/target/release/demo-comp-server'"),
            "ps1 still carries the suffix-less (Windows-broken) assertion"
        );
    }

    #[test]
    fn full_shape_build_scripts_gate_on_directories_at_runtime() {
        let sh = render_dialect(
            BUILD_SCRIPT_SH,
            ComponentShape::FrontendAndBackend,
            BackendTemplate::Cargo,
        );
        assert!(sh.contains("if [ -d \"$ROOT/server\" ]"));
        assert!(sh.contains("if [ -d \"$ROOT/web\" ]"));

        let ps1 = render_dialect(
            BUILD_SCRIPT_PS1,
            ComponentShape::FrontendAndBackend,
            BackendTemplate::Cargo,
        );
        assert!(ps1.contains("Test-Path (Join-Path $Root 'server')"));
        assert!(ps1.contains("Test-Path (Join-Path $Root 'web')"));

        // Single-plane shapes keep only their own gate (the opposite plane is
        // stripped at template level, not just skipped at runtime).
        let sh_fe = render_dialect(
            BUILD_SCRIPT_SH,
            ComponentShape::FrontendOnly,
            BackendTemplate::Cargo,
        );
        assert!(!sh_fe.contains("$ROOT/server"));
        let ps1_be = render_dialect(
            BUILD_SCRIPT_PS1,
            ComponentShape::BackendOnly,
            BackendTemplate::Cargo,
        );
        assert!(!ps1_be.contains("'web'"));
    }
}
