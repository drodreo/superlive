//! Endpoint descriptor: the transport injection contract between shell and component
//! (ts mirror of slsdk-rs `src/endpoint.rs`; parse semantics aligned one-to-one).
//!
//! The shell injects `SL_ENDPOINT` (`uds:{path}` | `tcp:{addr}`) when launching a component
//! subprocess; `SL_TOKEN` carries the per-component auth token (architecture.md
//! "Plane B Contract", "Security").

/** Name of the env var where the shell injects the listen descriptor. */
export const ENV_ENDPOINT = "SL_ENDPOINT";

/** Name of the env var where the shell injects the per-component token. */
export const ENV_TOKEN = "SL_TOKEN";

/** Listen descriptor: the transport shape the shell injects to the component backend.
 *
 * - `{ kind: "uds" }`: HTTP over Unix domain socket (Linux/macOS).
 * - `{ kind: "tcp" }`: HTTP over a 127.0.0.1 port (Windows, or explicit tcp descriptors) —
 *   the platform capability judgment lives at the ecosystem layer rather than the OS layer.
 */
export type Endpoint = { kind: "uds"; path: string } | { kind: "tcp"; host: string; port: number };

/** Descriptor parse error ({@link parseEndpoint}); variants mirror slsdk-rs `EndpointParseError`. */
export class EndpointParseError extends Error {
  /** `MissingScheme`: the `uds:` / `tcp:` prefix is absent. `InvalidTcpAddr`: what follows `tcp:` is not a valid address. */
  readonly kind: "MissingScheme" | "InvalidTcpAddr";
  /** The full descriptor that failed to parse (same value slsdk-rs reports). */
  readonly value: string;

  constructor(kind: "MissingScheme" | "InvalidTcpAddr", value: string) {
    const message =
      kind === "MissingScheme"
        ? `endpoint is missing the uds:/tcp: prefix: ${value}`
        : `invalid tcp address in endpoint: ${value}`;
    super(message);
    this.name = "EndpointParseError";
    this.kind = kind;
    this.value = value;
  }
}

/** Parses a listen descriptor in the shell injection format.
 *
 * Semantics aligned with slsdk-rs `endpoint.rs FromStr`:
 * - `uds:` prefix → Uds (path taken verbatim; `uds:` alone yields an empty path like rs).
 * - `tcp:` prefix → the rest must parse as `host:port` (bracket form `[::1]:port` for
 *   IPv6, port 0 allowed for a random port); port must be a 16-bit number, host must be
 *   non-empty — bare multi-colon IPv6 without brackets is rejected, matching the Rust
 *   `SocketAddr` grammar. Failures throw {@link EndpointParseError} with `InvalidTcpAddr`.
 * - Anything else → {@link EndpointParseError} with `MissingScheme`.
 */
export function parseEndpoint(desc: string): Endpoint {
  if (desc.startsWith("uds:")) {
    return { kind: "uds", path: desc.slice(4) };
  }
  if (desc.startsWith("tcp:")) {
    const parsed = parseTcpAddr(desc.slice(4));
    if (parsed === null) {
      throw new EndpointParseError("InvalidTcpAddr", desc);
    }
    return { kind: "tcp", ...parsed };
  }
  throw new EndpointParseError("MissingScheme", desc);
}

/** `host:port` split in the `SocketAddr` grammar: bracketed IPv6 or plain host (no bare
 * multi-colon IPv6), non-empty host, 16-bit numeric port. Returns null on any violation. */
function parseTcpAddr(rest: string): { host: string; port: number } | null {
  let host: string;
  let portStr: string;
  if (rest.startsWith("[")) {
    const close = rest.indexOf("]");
    if (close === -1) return null;
    host = rest.slice(1, close);
    const after = rest.slice(close + 1);
    if (!after.startsWith(":")) return null;
    portStr = after.slice(1);
  } else {
    const colon = rest.lastIndexOf(":");
    if (colon === -1) return null;
    host = rest.slice(0, colon);
    portStr = rest.slice(colon + 1);
    // Bare IPv6 like `::1:8080` is ambiguous and rejected by the Rust SocketAddr grammar too
    if (host.includes(":")) return null;
  }
  if (host.length === 0) return null;
  if (!/^\d+$/.test(portStr)) return null;
  const port = Number(portStr);
  if (port > 65535) return null;
  return { host, port };
}
