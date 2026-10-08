//! Endpoint descriptor: the transport injection contract between shell and component.
//!
//! When the shell launches a component subprocess it injects `SL_ENDPOINT` (the listen
//! descriptor), with a format consistent with architecture.md "Plane B Contract":
//! `uds:{path}` (Linux/macOS) or `tcp:{addr}` (Windows random port). Inside `serve()` the
//! transport is chosen by platform cfg branches, invisible to component authors.

use std::fmt;
use std::str::FromStr;

/// Name of the env var where the shell injects the listen descriptor (lifecycle contract, see
/// architecture.md "Plane B Contract · Lifecycle").
pub const ENV_ENDPOINT: &str = "SL_ENDPOINT";

/// Name of the env var where the shell injects the per-component token.
/// The token is the platform-invariant first wall (architecture.md "Security"): every request
/// must carry it.
pub const ENV_TOKEN: &str = "SL_TOKEN";

/// Listen descriptor: the transport shape the shell injects to the component backend.
///
/// - [`Endpoint::Uds`]: HTTP over Unix domain socket (Linux/macOS).
/// - [`Endpoint::Tcp`]: HTTP over a random 127.0.0.1 port (Windows) — tokio/mio do not
///   support Windows UDS; the platform capability judgment lives at the ecosystem layer
///   rather than the OS layer, so the Windows branch falls back to TCP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Uds { path: String },
    Tcp { addr: std::net::SocketAddr },
}

/// Display output is exactly the descriptor format injected by the shell (`uds:{path}` /
/// `tcp:{addr}`), so `format!("{}", endpoint)` can be written directly back to env or logs.
impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Endpoint::Uds { path } => write!(f, "uds:{path}"),
            Endpoint::Tcp { addr } => write!(f, "tcp:{addr}"),
        }
    }
}

/// Descriptor parse error ([`Endpoint::from_str`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointParseError {
    /// Missing the `uds:` / `tcp:` prefix.
    MissingScheme(String),
    /// What follows `tcp:` is not a valid SocketAddr.
    InvalidTcpAddr(String),
}

impl fmt::Display for EndpointParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EndpointParseError::MissingScheme(value) => {
                write!(f, "endpoint is missing the uds:/tcp: prefix: {value}")
            }
            EndpointParseError::InvalidTcpAddr(value) => {
                write!(f, "invalid tcp address in endpoint: {value}")
            }
        }
    }
}

impl std::error::Error for EndpointParseError {}

impl FromStr for Endpoint {
    type Err = EndpointParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(path) = s.strip_prefix("uds:") {
            return Ok(Endpoint::Uds {
                path: path.to_string(),
            });
        }
        if let Some(addr) = s.strip_prefix("tcp:") {
            // Parse-then-validate: invalid addresses fail at the injection entry point
            // instead of surfacing later at bind
            let addr = addr
                .parse::<std::net::SocketAddr>()
                .map_err(|_| EndpointParseError::InvalidTcpAddr(s.to_string()))?;
            return Ok(Endpoint::Tcp { addr });
        }
        Err(EndpointParseError::MissingScheme(s.to_string()))
    }
}
