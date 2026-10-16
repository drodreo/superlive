//! The serve() entry: the single starting point for component authors on the Node-family
//! runtimes — env parsing, runtime dispatch, token wall, the /health endpoint, and
//! graceful shutdown all live inside the SDK; authors only write business handlers
//! (ts sibling of slsdk-rs `serve()`).
//!
//! Lifecycle contract (architecture.md "Plane B Contract · Lifecycle"): env injects the
//! listen descriptor and token → the shell polls `GET /health` → SIGTERM graceful exit
//! (SIGINT works the same way for debugging).
//!
//! Dispatch matrix (runtime × transport):
//!
//! | runtime | uds                    | tcp                     |
//! |---------|------------------------|-------------------------|
//! | node    | http.listen(path)      | http.listen(port, host) |
//! | bun     | Bun.serve({ unix })    | Bun.serve({ port })     |
//! | deno    | ✗ rejected (see below) | Deno.serve({ port })    |
//!
//! deno has no Unix domain socket support: a `uds:` descriptor on deno is a hard error
//! telling the author to declare the component tcp-only — never a silent fallback.

import { ENV_ENDPOINT, ENV_TOKEN, parseEndpoint } from "./endpoint";
import type { Endpoint } from "./endpoint";
import { composeHandler } from "./handler";
import type { ServeHandler } from "./handler";
import { detectRuntime } from "./runtime";
import type { RuntimeKind } from "./runtime";
import type { ServerManifest } from "./manifest";
import { serveNode } from "./node";
import { ServeError, formatEndpoint } from "./serve-error";
import type { ServeInfo } from "./serve-types";

/** Component backend service entry: reads `SL_ENDPOINT`, picks the runtime implementation,
 * wraps the handler with the token wall + the SDK-owned `/health` endpoint, and listens.
 *
 * Environment contract:
 * - `SL_ENDPOINT` absent → standalone debug mode (`tcp:127.0.0.1:0`, not managed by the
 *   shell), with a warn pointing at the default descriptor (the ts parity of the rs
 *   standalone path — `ServeInfo.endpoint`/`port` report the expanded actual address);
 * - `SL_TOKEN` absent or empty switches to the unauthenticated standalone-debug mode (the
 *   shell-launched scenario always has it); when present, every request must carry
 *   `Authorization: Bearer <token>` or get 401 — including the health probe.
 *
 * **Reserved path**: `/health` is mounted by the SDK; the component handler never sees it
 * (on the rs side, author routes must not register the same path).
 */
export async function serve(
  handler: ServeHandler,
  manifest: ServerManifest,
): Promise<ServeInfo> {
  return serveOn(detectRuntime(), handler, manifest);
}

/** Runtime-injection seam (internal): `serve()` calls this with the detected runtime;
 * tests call it directly to exercise the deno/bun dispatch branches on node. */
export async function serveOn(
  runtime: RuntimeKind,
  handler: ServeHandler,
  manifest: ServerManifest,
): Promise<ServeInfo> {
  // 1. Listen descriptor: injected by the shell, absent means standalone debugging
  //    (rs serve.rs env-match parity: warn + default tcp:127.0.0.1:0)
  const raw = process.env[ENV_ENDPOINT];
  let endpoint: Endpoint;
  if (raw === undefined) {
    console.warn(
      `${ENV_ENDPOINT} not set: standalone debug mode (default tcp:127.0.0.1:0, not managed by the shell)`,
    );
    endpoint = { kind: "tcp", host: "127.0.0.1", port: 0 };
  } else {
    try {
      endpoint = parseEndpoint(raw);
    } catch (error) {
      throw ServeError.invalidEndpoint(raw, error);
    }
  }

  // 2. Token: the first wall in the shell-launched scenario; an empty string counts as not
  //    injected (the shell never injects an empty string, guarding against packaging
  //    script misconfiguration)
  const rawToken = process.env[ENV_TOKEN];
  const token = rawToken !== undefined && rawToken.length > 0 ? rawToken : null;
  if (token === null) {
    console.warn(
      `${ENV_TOKEN} not set: running without authentication, every request is allowed (this state should never occur when launched by the shell)`,
    );
  }

  // 3. Runtime dispatch
  if (runtime === "deno") {
    if (endpoint.kind === "uds") {
      throw ServeError.unsupportedEndpoint(
        formatEndpoint(endpoint),
        "Deno.serve does not support Unix domain sockets; declare the component as tcp-only in the manifest",
      );
    }
    return serveDeno(endpoint, handler, manifest, token);
  }
  if (runtime === "bun") {
    return serveBun(endpoint, handler, manifest, token);
  }
  return serveNode(endpoint, handler, manifest, token);
}

/** deno branch: Deno.serve consumes the composed pipeline natively (Request → Response).
 *
 * Known limitation: the stdin `shutdown` graceful-exit fallback (the Windows channel, rs
 * serve.rs parity) is implemented on the node branch only — a deno stdin line adapter is
 * deferred under the zero-runtime-dependency constraint (node is the current primary
 * runtime for Windows components); Windows deno components exit via the explicit
 * `shutdown()` or a kill only.
 */
function serveDeno(
  endpoint: Endpoint & { kind: "tcp" },
  handler: ServeHandler,
  manifest: ServerManifest,
  token: string | null,
): ServeInfo {
  const composed = composeHandler(handler, manifest, token);
  const deno = globalThis.Deno;
  if (deno === undefined) {
    throw new TypeError("deno runtime requested but globalThis.Deno is absent");
  }
  const server = deno.serve(
    { hostname: endpoint.host, port: endpoint.port },
    composed,
  );
  const actual: Endpoint = {
    kind: "tcp",
    host: server.addr.hostname,
    port: server.addr.port,
  };
  return {
    endpoint: actual,
    port: server.addr.port,
    shutdown: async () => {
      server.shutdown();
      await server.finished;
    },
    wait: () => server.finished,
  };
}

/** bun branch: Bun.serve consumes the composed pipeline natively; `unix` carries the UDS path.
 *
 * Known limitation: same as the deno branch — the stdin `shutdown` graceful-exit fallback
 * is implemented on the node branch only (node is the current primary runtime for Windows
 * components).
 */
function serveBun(
  endpoint: Endpoint,
  handler: ServeHandler,
  manifest: ServerManifest,
  token: string | null,
): ServeInfo {
  const composed = composeHandler(handler, manifest, token);
  const bun = globalThis.Bun;
  if (bun === undefined) {
    throw new TypeError("bun runtime requested but globalThis.Bun is absent");
  }
  const server =
    endpoint.kind === "tcp"
      ? bun.serve({
          hostname: endpoint.host,
          port: endpoint.port,
          fetch: composed,
        })
      : bun.serve({ unix: endpoint.path, fetch: composed });
  const actual: Endpoint =
    endpoint.kind === "tcp"
      ? { kind: "tcp", host: endpoint.host, port: server.port }
      : endpoint;
  return {
    endpoint: actual,
    port: endpoint.kind === "tcp" ? server.port : undefined,
    // stop() without arguments drains in-flight requests before closing (graceful)
    shutdown: async () => {
      server.stop();
      await server.stopped;
    },
    wait: () => server.stopped,
  };
}
