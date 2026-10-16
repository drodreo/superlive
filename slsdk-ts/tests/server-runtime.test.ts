import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { detectRuntime } from "../src/server/runtime";
import { serveOn } from "../src/server/serve";

const manifest = {
  id: "rt-comp",
  version: "0.1.0",
  min_shell_version: "0.0.1",
  namespace: "rt",
  display_name: "Rt",
};

const handler = (): Response => new Response("ok");

let savedEndpoint: string | undefined;
let savedToken: string | undefined;

beforeEach(() => {
  savedEndpoint = process.env.SL_ENDPOINT;
  savedToken = process.env.SL_TOKEN;
});

afterEach(() => {
  if (savedEndpoint === undefined) {
    delete process.env.SL_ENDPOINT;
  } else {
    process.env.SL_ENDPOINT = savedEndpoint;
  }
  if (savedToken === undefined) {
    delete process.env.SL_TOKEN;
  } else {
    process.env.SL_TOKEN = savedToken;
  }
  delete (globalThis as { Deno?: unknown }).Deno;
  delete (globalThis as { Bun?: unknown }).Bun;
  vi.restoreAllMocks();
});

describe("runtime dispatch matrix", () => {
  it("deno + uds → hard UnsupportedEndpoint error naming the deno limitation and the tcp-only guidance", async () => {
    process.env.SL_ENDPOINT = "uds:/tmp/nope.sock";
    const err = await serveOn("deno", handler, manifest).catch((e) => e);
    expect(err.name).toBe("ServeError");
    expect(err.kind).toBe("UnsupportedEndpoint");
    expect(err.value).toBe("uds:/tmp/nope.sock");
    expect(err.message).toContain(
      "Deno.serve does not support Unix domain sockets",
    );
    expect(err.message).toContain("tcp-only");
  });

  it("deno + tcp dispatches to Deno.serve with hostname/port and reports the expanded addr", async () => {
    process.env.SL_ENDPOINT = "tcp:127.0.0.1:0";
    process.env.SL_TOKEN = "t";
    const shutdown = vi.fn();
    const addSignalListener = vi.fn();
    const removeSignalListener = vi.fn();
    let capturedOptions: { hostname?: string; port?: number } | undefined;
    (globalThis as { Deno?: unknown }).Deno = {
      serve: vi.fn(
        (
          options: { hostname?: string; port?: number },
          h: (req: Request) => Promise<Response>,
        ) => {
          capturedOptions = options;
          expect(h).toBeTypeOf("function");
          return {
            addr: { hostname: "127.0.0.1", port: 45678 },
            finished: Promise.resolve(),
            shutdown,
          };
        },
      ),
      addSignalListener,
      removeSignalListener,
    };
    const info = await serveOn("deno", handler, manifest);
    expect(capturedOptions).toEqual({ hostname: "127.0.0.1", port: 0 });
    expect(info.endpoint).toEqual({
      kind: "tcp",
      host: "127.0.0.1",
      port: 45678,
    });
    expect(info.port).toBe(45678);
    // signal wall (node-branch parity): SIGTERM/SIGINT registered, same handler both
    expect(addSignalListener).toHaveBeenCalledWith(
      "SIGTERM",
      expect.any(Function),
    );
    expect(addSignalListener).toHaveBeenCalledWith(
      "SIGINT",
      expect.any(Function),
    );
    const term = addSignalListener.mock.calls.find(
      (c) => c[0] === "SIGTERM",
    )?.[1] as () => void;
    const int = addSignalListener.mock.calls.find(
      (c) => c[0] === "SIGINT",
    )?.[1] as () => void;
    expect(term).toBe(int);
    await info.shutdown();
    await vi.waitFor(() => {
      expect(removeSignalListener).toHaveBeenCalledWith("SIGTERM", term);
      expect(removeSignalListener).toHaveBeenCalledWith("SIGINT", int);
    });
    expect(shutdown).toHaveBeenCalledOnce();
  });

  it("bun + tcp dispatches to Bun.serve with hostname/port", async () => {
    process.env.SL_ENDPOINT = "tcp:127.0.0.1:0";
    process.env.SL_TOKEN = "t";
    const stop = vi.fn(() => Promise.resolve(null));
    const onSpy = vi.spyOn(process, "on");
    const offSpy = vi.spyOn(process, "removeListener");
    let capturedOptions: Record<string, unknown> | undefined;
    (globalThis as { Bun?: unknown }).Bun = {
      serve: vi.fn((options: Record<string, unknown>) => {
        capturedOptions = options;
        return { port: 45679, stop };
      }),
    };
    try {
      const info = await serveOn("bun", handler, manifest);
      expect(capturedOptions?.hostname).toBe("127.0.0.1");
      expect(capturedOptions?.port).toBe(0);
      expect(capturedOptions?.fetch).toBeTypeOf("function");
      expect(info.endpoint).toEqual({
        kind: "tcp",
        host: "127.0.0.1",
        port: 45679,
      });
      expect(info.port).toBe(45679);
      // signal wall via the node-compatible process surface (node-branch parity)
      expect(onSpy).toHaveBeenCalledWith("SIGTERM", expect.any(Function));
      expect(onSpy).toHaveBeenCalledWith("SIGINT", expect.any(Function));
      await info.shutdown();
      await vi.waitFor(() => {
        expect(offSpy).toHaveBeenCalledWith("SIGTERM", expect.any(Function));
        expect(offSpy).toHaveBeenCalledWith("SIGINT", expect.any(Function));
      });
      expect(stop).toHaveBeenCalledOnce();
    } finally {
      onSpy.mockRestore();
      offSpy.mockRestore();
    }
  });

  it("bun + uds dispatches to Bun.serve with the unix option; the descriptor round-trips", async () => {
    process.env.SL_ENDPOINT = "uds:/tmp/bun.sock";
    process.env.SL_TOKEN = "t";
    const stop = vi.fn(() => Promise.resolve(null));
    let capturedOptions: Record<string, unknown> | undefined;
    (globalThis as { Bun?: unknown }).Bun = {
      serve: vi.fn((options: Record<string, unknown>) => {
        capturedOptions = options;
        return { port: 0, stop };
      }),
    };
    const info = await serveOn("bun", handler, manifest);
    expect(capturedOptions?.unix).toBe("/tmp/bun.sock");
    expect(info.endpoint).toEqual({ kind: "uds", path: "/tmp/bun.sock" });
    expect(info.port).toBeUndefined();
    await info.shutdown();
    expect(stop).toHaveBeenCalledOnce();
  });

  it("node is the fallback dispatch when neither Deno nor Bun is present", async () => {
    // no mock: real node path (covered end-to-end in server-serve.test.ts);
    // here only assert the detection order that drives dispatch
    expect(detectRuntime()).toBe("node");
  });

  it("detection order: deno wins over bun, bun wins over node", () => {
    (globalThis as { Bun?: unknown }).Bun = {};
    expect(detectRuntime()).toBe("bun");
    (globalThis as { Deno?: unknown }).Deno = {};
    expect(detectRuntime()).toBe("deno");
  });
});
