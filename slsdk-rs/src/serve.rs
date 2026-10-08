//! The serve() skeleton: the single entry point for component authors — listening, transport
//! selection, token validation, and the health endpoint all live inside the SDK; authors only
//! write business handlers (architecture.md "Plane B Contract · Component Author Experience").
//!
//! Lifecycle contract (architecture.md "Plane B Contract · Lifecycle"):
//! env injects the listen address and token → the shell polls `GET /health` (readiness probe
//! and health check share one endpoint) → SIGTERM graceful exit (Windows falls back to a
//! stdin `shutdown` command) → stdout/stderr are left for log aggregation.
//!
//! Security contract (architecture.md "Security"): the token is the platform-invariant first
//! wall, designed on the principle "assume the port is reachable and still be safe" — no
//! token means 401, always; UDS and Origin checks are merely optional attack surface
//! reduction, never a security dependency.

use crate::config::{config_router, ConfigIntegration, ConfigStore};
use crate::endpoint::{Endpoint, EndpointParseError, ENV_ENDPOINT, ENV_TOKEN};
use crate::manifest::ComponentManifest;
use axum::extract::{Request, State};
use axum::http::{header::AUTHORIZATION, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use axum::Router;
use serde_json::json;
use std::fmt;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::sync::watch;

/// Failure shapes of [`serve`] (no anyhow, keeping dependencies minimal).
#[derive(Debug)]
pub enum ServeError {
    /// `SL_ENDPOINT` is set but failed to parse: the injection contract is broken, fail
    /// directly rather than silently falling back.
    InvalidEndpoint {
        value: String,
        source: EndpointParseError,
    },
    /// The current platform does not support this transport (Windows receiving a `uds:`
    /// descriptor).
    UnsupportedEndpoint { value: String },
    /// Listen failure (bind / leftover socket cleanup failure).
    Listen {
        endpoint: Endpoint,
        source: std::io::Error,
    },
}

impl fmt::Display for ServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServeError::InvalidEndpoint { value, source } => {
                write!(f, "invalid {ENV_ENDPOINT} value: {value} ({source})")
            }
            ServeError::UnsupportedEndpoint { value } => {
                write!(
                    f,
                    "transport descriptor not supported on this platform: {value} (tokio/mio do not support Windows UDS)"
                )
            }
            ServeError::Listen { endpoint, source } => {
                write!(f, "failed to listen on {endpoint}: {source}")
            }
        }
    }
}

impl std::error::Error for ServeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ServeError::InvalidEndpoint { source, .. } => Some(source),
            ServeError::Listen { source, .. } => Some(source),
            ServeError::UnsupportedEndpoint { .. } => None,
        }
    }
}

/// Graceful exit handle: cloneable for dispatch to other tasks (e.g. proactively stopping
/// the server on a custom control command).
#[derive(Clone)]
pub struct ShutdownHandle {
    tx: watch::Sender<bool>,
}

impl ShutdownHandle {
    /// Triggers graceful exit: stops accepting new connections and waits for in-flight
    /// requests to finish (equivalent to SIGTERM).
    pub fn shutdown(&self) {
        let _ = self.tx.send(true);
    }
}

/// Return value of [`serve`]: the actual listen address + exit handle.
///
/// The service runs in an independent tokio task: after the author gets `ServeInfo` they can
/// continue their own initialization; not awaiting [`ServeInfo::wait`] is fine — SIGTERM
/// (stdin `shutdown` on Windows) exits as a fallback.
pub struct ServeInfo {
    /// Actual listen address (`tcp:127.0.0.1:0` expands to the real assigned port).
    pub endpoint: Endpoint,
    /// Handle for proactively triggering exit.
    pub shutdown: ShutdownHandle,
    join: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl ServeInfo {
    /// Triggers graceful exit (forwards to [`ShutdownHandle::shutdown`]),
    /// no conflict with the same-named field: `info.shutdown()` is the method, `info.shutdown`
    /// is the field.
    pub fn shutdown(&self) {
        self.shutdown.shutdown()
    }

    /// Waits for the service to fully exit; the return value is the final result of the
    /// accept/serve loop.
    /// The UDS socket file is cleaned up on exit (best effort: when wait is not called, the
    /// fallback deletion by the next serve startup takes over).
    pub async fn wait(self) -> std::io::Result<()> {
        let result = match self.join.await {
            Ok(result) => result,
            Err(join_err) => Err(std::io::Error::other(format!(
                "serve task terminated unexpectedly: {join_err}"
            ))),
        };
        #[cfg(unix)]
        if let Endpoint::Uds { path } = &self.endpoint {
            let _ = std::fs::remove_file(path);
        }
        result
    }
}

/// Component backend service entry: assembles business routes + the health endpoint, selects
/// the transport per platform and listens.
///
/// **Reserved path**: `/health` is mounted by the SDK (readiness probe and health check
/// share one endpoint); author routes must not register the same path — axum Router::merge
/// panics directly on overlapping routes.
/// Warns when the manifest declares a config section but the config endpoint is not
/// integrated (see [`serve_with_config`]).
///
/// Environment contract:
/// - With `SL_ENDPOINT` absent, enters standalone debugging mode (`tcp:127.0.0.1:0`, not
///   managed by the shell);
/// - With `SL_TOKEN` absent, also treated as standalone debugging mode, **no auth enabled**
///   (safety responsibility is on the developer's self-testing scenario); in the
///   shell-launched scenario the variable is always present, and the token middleware is the
///   first wall.
pub async fn serve(routes: Router, manifest: &ComponentManifest) -> Result<ServeInfo, ServeError> {
    serve_with_config(routes, manifest, None).await
}

/// Config integration entry of [`serve`]: when the manifest declares a `config` section AND
/// this function receives [`ConfigIntegration`] (file path + hot-reload callback), the
/// standard config endpoint `GET/PUT /config` is mounted automatically
/// (architecture.md "Configuration Plane · Config Read/Write" — zero cost for component
/// authors; value read/write semantics see the wire contract in the [`crate::config`] module
/// docs).
///
/// Inconsistency between declaration and integration params only warns, never fails: a
/// missing `config` section = the component declares no configurable items; a missing
/// `ConfigIntegration` = the component chooses to manage config itself (single source of
/// truth for declarations = the manifest, see the wire contract).
pub async fn serve_with_config(
    routes: Router,
    manifest: &ComponentManifest,
    config: Option<ConfigIntegration>,
) -> Result<ServeInfo, ServeError> {
    // 1. Listen descriptor: injected by the shell, absent means standalone debugging
    let endpoint: Endpoint = match std::env::var(ENV_ENDPOINT) {
        Ok(value) => value
            .parse()
            .map_err(|source| ServeError::InvalidEndpoint { value, source })?,
        Err(_) => {
            log::warn!("{ENV_ENDPOINT} not set: standalone debug mode (default tcp:127.0.0.1:0, not managed by the shell)");
            Endpoint::Tcp {
                addr: "127.0.0.1:0"
                    .parse()
                    .expect("the built-in default address constant is always valid"),
            }
        }
    };

    // 2. token: the first wall in the shell-launched scenario; an empty string counts as not
    //    injected (the shell never injects an empty string, guarding against packaging
    //    script misconfiguration)
    let token: Option<Arc<String>> = std::env::var(ENV_TOKEN)
        .ok()
        .filter(|t| !t.is_empty())
        .map(Arc::new);
    if token.is_none() {
        log::warn!(
            "{ENV_TOKEN} not set: standalone debug mode runs without authentication and any request is allowed (this state should never occur when launched by the shell)"
        );
    }

    // 3. Assembly: business routes + /config (mounted only when declaration and integration
    //    params are both present) + /health + token middleware
    //    the layer applies to all routes after merge: the shell holds the token, and the
    //    health probe carries it too
    let mut app = routes;
    match (manifest.config.as_ref(), config) {
        (Some(declaration), Some(integration)) => {
            let store = Arc::new(ConfigStore::new(
                declaration.fields.clone(),
                integration.path,
                integration.on_change,
            ));
            app = app.merge(config_router(store));
        }
        (Some(_), None) => log::warn!(
            "manifest declares a config section but no ConfigIntegration was provided: the /config endpoint is not mounted (components must integrate via serve_with_config)"
        ),
        (None, Some(_)) => log::warn!(
            "ConfigIntegration provided but the manifest declares no config section: the /config endpoint is not mounted (the manifest is the single source of truth for declarations)"
        ),
        (None, None) => {}
    }
    let health = Router::new()
        .route("/health", get(health_handler))
        .with_state(HealthInfo {
            id: Arc::new(manifest.id.clone()),
            version: Arc::new(manifest.version.clone()),
        });
    let mut app = app.merge(health);
    if let Some(expected) = token {
        app = app.layer(axum::middleware::from_fn_with_state(
            expected,
            auth_middleware,
        ));
    }

    // 4. Listen in platform cfg branches (transport platform adaptation, invisible to
    //    component authors).
    //    UnixListener and TcpListener are different types, so bind+spawn must each complete
    //    inside their own arm; only the JoinHandle and the actual listen address are exposed
    //    outward in unified form.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    spawn_signal_watcher(shutdown_tx.clone());

    #[cfg(unix)]
    let (join, serve_endpoint) = match &endpoint {
        Endpoint::Uds { path } => {
            // Leftover socket file cleanup: a previous non-graceful exit leaves the file
            // behind, and a direct bind would hit AddrInUse.
            // Delete directly and ignore NotFound — checking exists() then deleting has a
            // TOCTOU race window between the two steps; a single-step delete removes it
            // naturally (nonexistent = nothing to do, other errors propagate as usual)
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(ServeError::Listen {
                        endpoint: endpoint.clone(),
                        source,
                    })
                }
            }
            let listener =
                tokio::net::UnixListener::bind(path).map_err(|source| ServeError::Listen {
                    endpoint: endpoint.clone(),
                    source,
                })?;
            // Read back the actual socket path so ServeInfo's report matches the real
            // listener
            let actual = listener
                .local_addr()
                .ok()
                .and_then(|addr| addr.as_pathname().map(|p| p.to_string_lossy().into_owned()))
                .unwrap_or_else(|| path.clone());
            (
                spawn_serve(listener, app.clone(), shutdown_rx.clone()),
                Endpoint::Uds { path: actual },
            )
        }
        Endpoint::Tcp { addr } => {
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|source| ServeError::Listen {
                    endpoint: endpoint.clone(),
                    source,
                })?;
            let actual = listener.local_addr().map_err(|source| ServeError::Listen {
                endpoint: endpoint.clone(),
                source,
            })?;
            (
                spawn_serve(listener, app, shutdown_rx),
                Endpoint::Tcp { addr: actual },
            )
        }
    };
    #[cfg(windows)]
    let (join, serve_endpoint) = match &endpoint {
        Endpoint::Tcp { addr } => {
            let listener =
                tokio::net::TcpListener::bind(addr).map_err(|source| ServeError::Listen {
                    endpoint: endpoint.clone(),
                    source,
                })?;
            let actual = listener.local_addr().map_err(|source| ServeError::Listen {
                endpoint: endpoint.clone(),
                source,
            })?;
            (
                spawn_serve(listener, app, shutdown_rx),
                Endpoint::Tcp { addr: actual },
            )
        }
        Endpoint::Uds { path } => {
            return Err(ServeError::UnsupportedEndpoint {
                value: format!("uds:{path}"),
            });
        }
    };

    Ok(ServeInfo {
        endpoint: serve_endpoint,
        shutdown: ShutdownHandle { tx: shutdown_tx },
        join,
    })
}

/// Starts the accept/serve loop (independent task): generics unify the two Listener kinds;
/// the author side only gets a JoinHandle.
/// On receiving a signal the watch channel stops accept and waits for in-flight connections
/// to finish (graceful exit).
fn spawn_serve<L>(
    listener: L,
    app: Router,
    shutdown_rx: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<std::io::Result<()>>
where
    L: axum::serve::Listener + Send + 'static,
    // axum's WithGracefulShutdown IntoFuture impl requires Addr: Debug (the SocketAddr of
    // both Tcp/Unix satisfy it)
    <L as axum::serve::Listener>::Addr: std::fmt::Debug,
{
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let mut rx = shutdown_rx;
                let _ = rx.changed().await;
            })
            .await
    })
}

/// Installs exit signal listening (independent task): holds a clone of the watch sender for
/// the process lifetime, ensuring the serve side's receiver `changed()` does not return
/// early because all senders were dropped.
fn spawn_signal_watcher(shutdown_tx: watch::Sender<bool>) {
    #[cfg(unix)]
    {
        tokio::spawn(async move {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut sig) => {
                    if sig.recv().await.is_some() {
                        let _ = shutdown_tx.send(true);
                    }
                }
                // Install failure only degrades to "cannot auto-exit"; the service itself
                // keeps running
                Err(e) => log::error!("failed to install the SIGTERM handler: {e}"),
            }
        });
    }
    #[cfg(windows)]
    {
        // Windows has no SIGTERM: the shell sends a `shutdown` line via stdin as fallback
        // (architecture.md "Lifecycle")
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) if line.trim() == "shutdown" => {
                        let _ = shutdown_tx.send(true);
                        break;
                    }
                    Ok(Some(_)) => continue,
                    // stdin closed or read error: no more signal source; only the
                    // ShutdownHandle trigger path remains
                    _ => break,
                }
            }
        });
    }
}

#[derive(Clone)]
struct HealthInfo {
    id: Arc<String>,
    version: Arc<String>,
}

/// `GET /health`: readiness probe and health check share one endpoint (architecture.md
/// "Lifecycle").
/// Returns the component identity and version, so the shell can also verify it launched the
/// expected component.
async fn health_handler(State(info): State<HealthInfo>) -> Json<serde_json::Value> {
    Json(json!({
        "id": info.id.as_str(),
        "version": info.version.as_str(),
    }))
}

/// token middleware: `Authorization: Bearer <token>` compared constant-time against the
/// injected value (subtle, guarding against timing side channels); no token / mismatch means
/// 401, always (architecture.md "Security": assume the port is reachable and still be safe).
/// scheme is case-insensitive per RFC 7235 (manual curl/third-party tool tests are not
/// troubled by misleading 401s).
async fn auth_middleware(
    State(expected): State<Arc<String>>,
    req: Request,
    next: Next,
) -> Response {
    let authorized = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .and_then(|(scheme, token)| {
            (scheme.eq_ignore_ascii_case("bearer")).then(|| token.trim_start())
        })
        .is_some_and(|given| constant_time_eq(given.as_bytes(), expected.as_bytes()));
    if authorized {
        next.run(req).await
    } else {
        // RFC 7235: a 401 should carry WWW-Authenticate describing the challenge
        let mut res = StatusCode::UNAUTHORIZED.into_response();
        res.headers_mut()
            .insert("WWW-Authenticate", HeaderValue::from_static("Bearer"));
        res
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}
