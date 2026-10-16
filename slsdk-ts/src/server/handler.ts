//! Handler composition: the request pipeline the serve surface runs on every runtime —
//! token wall first, then the SDK-owned `/health` endpoint, then the component handler.
//!
//! Wire contract (aligned one-to-one with slsdk-rs `serve.rs`):
//! - The token middleware applies to ALL routes including `/health` ("the shell holds the
//!   token, and the health probe carries it too") — no token / mismatch means 401, always.
//! - `Authorization: Bearer <token>`; scheme compared case-insensitively (RFC 7235, so
//!   manual curl tests are not troubled by misleading 401s); the token part is
//!   `trim_start`ed; comparison is constant-time (see below).
//! - The 401 response is empty-bodied and carries `WWW-Authenticate: Bearer` (RFC 7235).
//! - `GET /health` returns `{"id","version"}` (manifest identity, so the shell can also
//!   verify it launched the expected component); `/health` is a reserved SDK path —
//!   component handlers never see it (rs: author routes must not register the same path).

import type { ServerManifest } from "./manifest";

/** The component business handler: Web-standard Request → Response (the native shape of
 * deno.serve / Bun.serve; the node adapter bridges IncomingMessage/ServerResponse onto it
 * with streaming bodies, never buffering them whole). */
export type ServeHandler = (req: Request) => Response | Promise<Response>;

export function composeHandler(
  handler: ServeHandler,
  manifest: ServerManifest,
  token: string | null,
): (req: Request) => Promise<Response> {
  return async (req) => {
    // 1. Token wall: applies to every route (health probe carries the token too)
    if (token !== null && !(await isAuthorized(req, token))) {
      return new Response(null, {
        status: 401,
        headers: { "www-authenticate": "Bearer" },
      });
    }
    // 2. Reserved path: /health is SDK-owned (readiness probe and health check share one
    //    endpoint). Non-GET methods on the reserved path get 405, matching the axum
    //    MethodRouter behavior on the rs side.
    if (new URL(req.url).pathname === "/health") {
      if (req.method === "GET" || req.method === "HEAD") {
        return new Response(
          JSON.stringify({ id: manifest.id, version: manifest.version }),
          {
            status: 200,
            headers: { "content-type": "application/json" },
          },
        );
      }
      return new Response(null, {
        status: 405,
        headers: { allow: "GET, HEAD" },
      });
    }
    // 3. Component business handler; a throw maps to an empty 500 — caught here (not per
    //    adapter) so all three runtimes share the same failure shape (the rs side surfaces
    //    handler panics as a broken connection; a typed 500 keeps the connection reusable)
    try {
      return await handler(req);
    } catch {
      return new Response(null, { status: 500 });
    }
  };
}

/** `Authorization: Bearer <token>` check against the injected expected token. */
async function isAuthorized(req: Request, expected: string): Promise<boolean> {
  const header = req.headers.get("authorization");
  if (header === null) return false;
  const space = header.indexOf(" ");
  if (space === -1) return false;
  if (header.slice(0, space).toLowerCase() !== "bearer") return false;
  const given = header.slice(space + 1).trimStart();
  return timingSafeEq(given, expected);
}

/** Constant-time string equality: both sides are hashed to fixed 32 bytes (Web Crypto,
 * available natively on node/deno/bun — no runtime-specific dependency) and the digests
 * are XOR-folded, so no byte-position or length-dependent early exit exists. Hashing a
 * string reveals at most its length, the same information surface as slsdk-rs'
 * `subtle::ConstantTimeEq` (which also returns early-free `false` on length mismatch). */
async function timingSafeEq(a: string, b: string): Promise<boolean> {
  const encoder = new TextEncoder();
  const [da, db] = await Promise.all([
    crypto.subtle.digest("SHA-256", encoder.encode(a)),
    crypto.subtle.digest("SHA-256", encoder.encode(b)),
  ]);
  const va = new Uint8Array(da);
  const vb = new Uint8Array(db);
  let diff = 0;
  for (let i = 0; i < va.length; i++) {
    diff |= va[i]! ^ vb[i]!;
  }
  return diff === 0;
}
