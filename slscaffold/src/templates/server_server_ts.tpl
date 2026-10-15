//! __NAME__ component backend (TypeScript).
//!
//! The slsdk-ts server entry handles everything infrastructural: listening
//! (SL_ENDPOINT - `tcp:`/`uds:` injected by the shell), token auth (SL_TOKEN,
//! the first wall) and the /health endpoint. We only write business handlers.
//!
//! Without SL_ENDPOINT the process runs in standalone debug mode
//! (tcp:127.0.0.1:0, no auth) - see README.md for the fixed-port recipe.

import { serve, ENV_ENDPOINT, type ServerManifest } from "slsdk-ts/server";

/// Single source of identity: the shell reads COMPONENT_VERSION from this file
/// as the project's version source (a TypeScript backend has no Cargo.toml).
/// Keep it in step with the version in the project's manifest.json.
const COMPONENT_ID = "__ID__";
const COMPONENT_VERSION = "0.1.0";

/// The manifest this process reports: serve() consumes id / namespace /
/// version from it (id and namespace are single sources of identity - the
/// shell cross-checks them against manifest.json).
const manifest: ServerManifest = {
  id: COMPONENT_ID,
  version: COMPONENT_VERSION,
  // Serve-self-report placeholders, mirroring the cargo template's main.rs:
  // serve() only reads id/version; the install-time gate uses the shell's own
  // copy of manifest.json, not this in-process declaration.
  min_shell_version: "0.0.0",
  namespace: "__NS__",
  display_name: "__NAME__",
};

/// Sample business handler. /health is reserved (mounted by the SDK) - author
/// routes must not register it. Handlers receive the standard `Request` and
/// return any `Response` (the runtime's native primitives; no framework).
const routes = {
  "/api/v1/hello": () => Response.json({ message: "hello from __ID__" }),
};

// serve() keeps the process alive until the shell stops it (SIGTERM). The
// shell injects the listen descriptor through the SL_ENDPOINT environment
// variable (format `tcp:addr` / `uds:path`; exported above as ENV_ENDPOINT
// for standalone scripts that want the same name) and the per-component
// token under SL_TOKEN; both are consumed inside the SDK.
serve(manifest, routes);
