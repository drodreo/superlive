//! Cross-runtime detection: the serve surface supports node / deno / bun. Detection is a
//! minimal structural probe on `globalThis` — no runtime-specific npm dependency, and the
//! declarations below keep `tsc --noEmit` green in this pure-node (DOM lib) toolchain.

export type RuntimeKind = "node" | "deno" | "bun";

declare global {
  // Minimal Deno.serve surface the SDK touches (Deno 2 HttpServer shape).
  var Deno:
    | {
        serve: (
          options: { hostname?: string; port?: number },
          handler: (req: Request) => Response | Promise<Response>,
        ) => {
          addr: { hostname: string; port: number };
          finished: Promise<void>;
          shutdown(): void;
        };
      }
    | undefined;

  // Minimal Bun.serve surface the SDK touches (`unix` for UDS, `stopped` for graceful exit).
  var Bun:
    | {
        serve: (options: {
          port?: number;
          hostname?: string;
          unix?: string;
          fetch: (req: Request) => Response | Promise<Response>;
        }) => {
          port: number;
          stopped: Promise<void>;
          stop(closeActiveConnections?: boolean): void;
        };
      }
    | undefined;
}

/** Detects the hosting runtime: Deno first, then Bun, then node (the fallback the SDK is
 * developed against). The order matches each runtime's own self-identification convention. */
export function detectRuntime(): RuntimeKind {
  if (typeof globalThis.Deno !== "undefined") return "deno";
  if (typeof globalThis.Bun !== "undefined") return "bun";
  return "node";
}
