//! slsdk-rs: the superlive v2 **Plane B contract reference implementation**.
//!
//! Component backend authors and the shell both reference this crate: both sides share the same
//! type and protocol definitions, preventing contract drift.
//! Design basis: the "Plane B Contract", "Installation and Lifecycle", and "Security" sections
//! of docs/architecture.md in the repo.
//!
//! Entry point for component authors: [`serve::serve`] handles listening, transport platform
//! adaptation, token validation, and the health endpoint in one line, so authors only write
//! business handlers; components declaring config fields automatically get the standard config
//! endpoint (`GET/PUT /config`, see the wire contract in the [`config`] module docs) once
//! integrated via [`serve::serve_with_config`].
//! The manifest and fingerprint verification are shared by the shell installation flow and
//! component packaging scripts.

pub mod config;
pub mod endpoint;
pub mod manifest;
pub mod serve;

pub use config::{config_router, ConfigError, ConfigIntegration, ConfigStore, OnChangeCallback};
pub use endpoint::{Endpoint, EndpointParseError, ENV_ENDPOINT, ENV_TOKEN};
pub use manifest::{
    verify_files, ComponentConfig, ComponentManifest, ConfigField, ConfigFieldType, FileEntry,
    LaunchConfig, ManifestError, MenuDeclaration, RouteDeclaration, UiDeclaration, UiType,
    VerifyError,
};
pub use serve::{serve, serve_with_config, ServeError, ServeInfo, ShutdownHandle};
