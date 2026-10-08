//! Shell data directory layout and runtime constants: the single source for
//! all paths.
//!
//! Layout (architecture.md "Install and lifecycle"):
//! ```text
//! <data>/
//! ├── components/{id}/   install directory (Plane B files; the static server mounts only its ui/ subdirectory)
//! ├── sockets/{id}.sock  component UDS listen path (unix)
//! ├── registry/{id}.json manifest persistence (S5 decision: the manifest is kept outside the install directory)
//! └── tmp/               install temp workspace (same disk as components, guaranteeing atomic rename)
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Listen port of the shell's embedded HTTP service (fixed constant in v1;
/// the log prints the actual address; configurability deferred).
pub const SHELL_PORT: u16 = 39876;

/// Total component health-probe timeout: exceeding it counts as a failed
/// start (process reclaimed).
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);

/// Health probe polling interval.
pub const HEALTH_INTERVAL: Duration = Duration::from_millis(150);

/// Graceful exit grace period: after SIGTERM/stdin shutdown is sent, if
/// still alive past the deadline a SIGKILL force-kill follows.
pub const STOP_GRACE: Duration = Duration::from_secs(5);

/// Shell data directory layout. All sub-paths derive from this; hand-joining
/// paths elsewhere is forbidden.
#[derive(Debug, Clone)]
pub struct DataLayout {
    root: PathBuf,
}

impl DataLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn components_dir(&self) -> PathBuf {
        self.root.join("components")
    }

    pub fn sockets_dir(&self) -> PathBuf {
        self.root.join("sockets")
    }

    pub fn registry_dir(&self) -> PathBuf {
        self.root.join("registry")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// Component install directory. The static server mounts only its `ui/`
    /// subdirectory as the `/comps/{id}` space (architecture.md "Install and
    /// lifecycle"); binaries/resources do not enter the URL space.
    pub fn component_dir(&self, id: &str) -> PathBuf {
        self.components_dir().join(id)
    }

    /// Component UDS listen path. id is constrained by manifest.validate to
    /// letters/digits/hyphens, so it cannot introduce path separators or
    /// traversal forms.
    pub fn socket_path(&self, id: &str) -> PathBuf {
        self.sockets_dir().join(format!("{id}.sock"))
    }

    /// Manifest persistence file (registry/{id}.json).
    pub fn registry_file(&self, id: &str) -> PathBuf {
        self.registry_dir().join(format!("{id}.json"))
    }

    /// Component config file (config.toml inside the install directory): the
    /// persistent form of the configuration-plane contract and its single
    /// source of truth. The component reads/writes the same file via a
    /// cwd-relative path (slsdk ConfigIntegration convention), and the
    /// shell-side unstarted-state writes / state comparisons also go through
    /// this path — single source; hand-joining is forbidden.
    pub fn component_config_file(&self, id: &str) -> PathBuf {
        self.component_dir(id).join("config.toml")
    }

    /// Shell-side component management metadata (configuration plane: state
    /// machine + launch policy persistence).
    /// Deliberately a .meta suffix instead of .json: the manifest scan of the
    /// registry/ directory filters by extension, and meta must not be mixed
    /// into the manifest listing.
    pub fn meta_file(&self, id: &str) -> PathBuf {
        self.registry_dir().join(format!("{id}.meta"))
    }

    /// Ensure the four subdirectories exist. Call only in a synchronous
    /// context (shell startup); async paths use tokio::fs.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for dir in [
            self.components_dir(),
            self.sockets_dir(),
            self.registry_dir(),
            self.tmp_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Async version of ensure_dirs (no synchronous file IO on the tokio
    /// worker thread).
    pub async fn ensure_dirs_async(&self) -> std::io::Result<()> {
        for dir in [
            self.components_dir(),
            self.sockets_dir(),
            self.registry_dir(),
            self.tmp_dir(),
        ] {
            tokio::fs::create_dir_all(dir).await?;
        }
        Ok(())
    }
}
