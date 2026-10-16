//! node adapter: listens on node:http and bridges IncomingMessage/ServerResponse onto the
//! Web-standard handler pipeline with streaming bodies (no whole-body buffering anywhere).
//!
//! UDS notes (aligned with slsdk-rs serve.rs):
//! - Leftover socket file cleanup before bind (a previous non-graceful exit leaves the file
//!   behind and a direct bind would hit AddrInUse); a single force-removal also covers
//!   NotFound naturally, matching the rs delete-and-ignore-NotFound shape.
//! - The socket file is removed again on full exit (best effort), so `wait()` callers see
//!   a clean path afterwards.

import { rmSync } from "node:fs";
import http from "node:http";
import type { IncomingMessage, ServerResponse } from "node:http";
import { createInterface } from "node:readline";
import { Readable } from "node:stream";
import { composeHandler } from "./handler";
import type { ServeHandler } from "./handler";
import type { Endpoint } from "./endpoint";
import type { ServerManifest } from "./manifest";
import { ServeError } from "./serve-error";
import type { ServeInfo } from "./serve-types";

export function serveNode(
  endpoint: Endpoint,
  handler: ServeHandler,
  manifest: ServerManifest,
  token: string | null,
): Promise<ServeInfo> {
  const composed = composeHandler(handler, manifest, token);
  const server = http.createServer((req, res) => {
    void bridge(req, res, composed);
  });

  // Bind: leftover socket cleanup + listen, with the error/listening race handled by
  // racing both events on the same promise.
  const bound = new Promise<void>((resolve, reject) => {
    const onError = (error: Error): void => {
      cleanup();
      reject(ServeError.listen(endpoint, error));
    };
    const onListening = (): void => {
      cleanup();
      resolve();
    };
    const cleanup = (): void => {
      server.off("error", onError);
      server.off("listening", onListening);
    };
    server.on("error", onError);
    server.on("listening", onListening);
    if (endpoint.kind === "uds") {
      try {
        // force: ENOENT is swallowed here (the healthy case), other errors propagate
        rmSync(endpoint.path, { force: true });
      } catch (error) {
        cleanup();
        reject(ServeError.listen(endpoint, error));
        return;
      }
      server.listen(endpoint.path);
    } else {
      server.listen(endpoint.port, endpoint.host);
    }
  });

  const setup = (async (): Promise<ServeInfo> => {
    await bound;
    const done = new Promise<void>((resolve) => {
      server.once("close", () => {
        // Remove the socket file on full exit (best effort), like rs' ServeInfo::wait
        if (endpoint.kind === "uds") {
          try {
            rmSync(endpoint.path, { force: true });
          } catch {
            // ignore: the next serve startup re-runs the leftover cleanup
          }
        }
        resolve();
      });
    });

    // Graceful exit: stop accepting, drop idle keep-alive connections, let in-flight
    // requests finish. SIGTERM is the shell's termination signal; SIGINT is the debug
    // companion. Handlers are removed once the exit completes so a long-lived host
    // process keeps default signal behavior afterwards.
    const requestShutdown = (): void => {
      server.close();
      server.closeIdleConnections?.();
    };
    process.once("SIGTERM", requestShutdown);
    process.once("SIGINT", requestShutdown);
    // Windows has no real SIGTERM (the shell's kill is a hard kill): the shell sends a
    // `shutdown` line via stdin as the graceful-exit fallback (rs serve.rs parity). Same
    // exit path as the signals.
    const disposeStdinWatcher = spawnStdinShutdownWatcher(requestShutdown);
    void done.then(() => {
      disposeStdinWatcher();
      process.removeListener("SIGTERM", requestShutdown);
      process.removeListener("SIGINT", requestShutdown);
    });

    const wait = (): Promise<void> => done;
    const shutdown = async (): Promise<void> => {
      requestShutdown();
      await done;
    };

    const address = server.address();
    if (endpoint.kind === "uds") {
      const actual = typeof address === "string" ? address : endpoint.path;
      return { endpoint: { kind: "uds", path: actual }, shutdown, wait };
    }
    const info = address as { address: string; port: number };
    const actual: Endpoint = {
      kind: "tcp",
      host: info.address,
      port: info.port,
    };
    return { endpoint: actual, port: info.port, shutdown, wait };
  })();

  return setup;
}

/** stdin `shutdown` watcher: the Windows graceful-exit fallback (Windows has no real
 * SIGTERM; the shell's kill is a hard kill), mirroring slsdk-rs serve.rs
 * `spawn_signal_watcher`'s cfg(windows) branch. Command semantics are byte-for-byte the
 * rs ones: a line whose trimmed content equals `"shutdown"` (case-sensitive) triggers the
 * same graceful exit as SIGTERM; any other line is ignored; stdin close/error simply ends
 * the watcher (the explicit `shutdown()` and signal paths remain).
 *
 * `input` defaults to `process.stdin` and is injectable for tests. Returns a dispose
 * function (closes the readline interface) called when the serve fully exits.
 */
export function spawnStdinShutdownWatcher(
  requestShutdown: () => void,
  input: NodeJS.ReadableStream = process.stdin,
): () => void {
  const rl = createInterface({ input });
  rl.on("line", (line) => {
    if (line.trim() === "shutdown") {
      // rs parity: break out of the read loop after a match — one trigger per watcher
      rl.close();
      requestShutdown();
    }
  });
  return () => {
    rl.close();
  };
}

/** IncomingMessage → Web Request: absolute URL from the Host header, header list copied
 * (multi-value headers appended), body piped as a live stream (duplex half per the fetch
 * spec for streaming request bodies). */
function toRequest(req: IncomingMessage): Request {
  const host = req.headers.host ?? "localhost";
  const url = new URL(req.url ?? "/", `http://${host}`).toString();
  const headers = new Headers();
  for (const [key, value] of Object.entries(req.headers)) {
    if (Array.isArray(value)) {
      for (const item of value) {
        if (item !== undefined) headers.append(key, item);
      }
    } else if (value !== undefined) {
      headers.set(key, value);
    }
  }
  const method = req.method ?? "GET";
  const hasBody = method !== "GET" && method !== "HEAD";
  const body = hasBody
    ? (Readable.toWeb(req) as unknown as ReadableStream)
    : null;
  const init = {
    method,
    headers,
    body,
    duplex: "half",
  } as unknown as RequestInit;
  return new Request(url, init);
}

/** Response → ServerResponse: status/status text/headers copied, body streamed to the
 * socket (node suppresses the body itself for HEAD requests). */
async function bridge(
  req: IncomingMessage,
  res: ServerResponse,
  handler: (req: Request) => Promise<Response>,
): Promise<void> {
  let response: Response;
  try {
    response = await handler(toRequest(req));
  } catch {
    // Reached only when the IncomingMessage → Request bridge fails (malformed target /
    // Host header); business-handler errors never escape composeHandler (mapped to 500 there)
    response = new Response(null, { status: 400 });
  }
  res.statusCode = response.status;
  if (response.statusText) {
    res.statusMessage = response.statusText;
  }
  response.headers.forEach((value, key) => {
    res.setHeader(key, value);
  });
  if (response.body === null) {
    res.end();
    return;
  }
  const stream = Readable.fromWeb(
    response.body as unknown as import("node:stream/web").ReadableStream,
  );
  stream.on("error", () => {
    // Client went away mid-body or the handler stream errored: tear the connection down
    res.destroy();
  });
  stream.pipe(res);
}
