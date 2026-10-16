//! Failure shapes of the serve surface (ts mirror of slsdk-rs `ServeError`): a set-until-
//! parse-broken `SL_ENDPOINT` fails as InvalidEndpoint, an unsupported transport on the
//! current runtime as UnsupportedEndpoint, a bind failure as Listen.

import { ENV_ENDPOINT } from "./endpoint";
import type { Endpoint } from "./endpoint";

export type ServeErrorKind =
  "InvalidEndpoint" | "UnsupportedEndpoint" | "Listen";

export class ServeError extends Error {
  readonly kind: ServeErrorKind;
  /** The offending descriptor value (InvalidEndpoint / UnsupportedEndpoint). */
  readonly value?: string;
  /** The listen target (Listen). */
  readonly endpoint?: Endpoint;

  constructor(
    kind: ServeErrorKind,
    message: string,
    fields: { value?: string; endpoint?: Endpoint; cause?: unknown } = {},
  ) {
    super(
      message,
      fields.cause !== undefined ? { cause: fields.cause } : undefined,
    );
    this.name = "ServeError";
    this.kind = kind;
    this.value = fields.value;
    this.endpoint = fields.endpoint;
  }

  /** `SL_ENDPOINT` set but failed to parse: fail directly rather than silently falling back. */
  static invalidEndpoint(value: string, cause: unknown): ServeError {
    return new ServeError(
      "InvalidEndpoint",
      `invalid ${ENV_ENDPOINT} value: ${value} (${String(cause)})`,
      {
        value,
        cause,
      },
    );
  }

  /** The current runtime does not support this transport (deno receiving a `uds:` descriptor). */
  static unsupportedEndpoint(value: string, reason: string): ServeError {
    return new ServeError(
      "UnsupportedEndpoint",
      `transport descriptor not supported on this runtime: ${value} (${reason})`,
      {
        value,
      },
    );
  }

  /** Listen failure (bind / leftover socket cleanup failure). */
  static listen(endpoint: Endpoint, cause: unknown): ServeError {
    return new ServeError(
      "Listen",
      `failed to listen on ${formatEndpoint(endpoint)}: ${String(cause)}`,
      {
        endpoint,
        cause,
      },
    );
  }
}

/** Renders an endpoint back into its descriptor form (`uds:{path}` / `tcp:{host}:{port}`),
 * matching the shell injection format so it can be written back to logs directly. */
export function formatEndpoint(endpoint: Endpoint): string {
  return endpoint.kind === "uds"
    ? `uds:${endpoint.path}`
    : `tcp:${endpoint.host}:${endpoint.port}`;
}
