import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { Readable } from 'node:stream';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { spawnStdinShutdownWatcher } from '../src/server/node';
import { serve } from '../src/server/serve';
import type { ServeInfo } from '../src/server/serve-types';

const manifest = {
  id: 'life-comp',
  version: '9.9.9',
  min_shell_version: '0.0.1',
  namespace: 'life',
  display_name: 'Life',
};

const handler = (): Response => new Response('ok');

let info: ServeInfo | undefined;
let savedEndpoint: string | undefined;
let savedToken: string | undefined;

beforeEach(() => {
  savedEndpoint = process.env.SL_ENDPOINT;
  savedToken = process.env.SL_TOKEN;
});

afterEach(async () => {
  await info?.shutdown();
  info = undefined;
  restore('SL_ENDPOINT', savedEndpoint);
  restore('SL_TOKEN', savedToken);
});

function restore(key: string, value: string | undefined): void {
  if (value === undefined) {
    delete process.env[key];
  } else {
    process.env[key] = value;
  }
}

function udsGet(socketPath: string, path: string, token: string): Promise<{ status?: number; body: string }> {
  return new Promise((resolve, reject) => {
    const req = http.request(
      { socketPath, path, method: 'GET', headers: { authorization: `Bearer ${token}` } },
      (res) => {
        let body = '';
        res.on('data', (chunk) => {
          body += chunk;
        });
        res.on('end', () => resolve({ status: res.statusCode, body }));
      },
    );
    req.on('error', reject);
    req.end();
  });
}

describe('serve() on node', () => {
  it('missing SL_ENDPOINT → standalone debug mode (rs serve.rs:170-181 parity): tcp loopback, expanded non-zero port, warn logged', async () => {
    delete process.env.SL_ENDPOINT;
    process.env.SL_TOKEN = 't';
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    try {
      info = await serve(handler, manifest);
      expect(info.endpoint).toMatchObject({ kind: 'tcp', host: '127.0.0.1' });
      expect(info.port).toBeGreaterThan(0);
      expect(warnSpy).toHaveBeenCalledWith(
        expect.stringContaining('standalone debug mode'),
      );
      expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining('tcp:127.0.0.1:0'));
      // the standalone server actually serves on the expanded port
      const res = await fetch(`http://127.0.0.1:${info.port}/health`, {
        headers: { authorization: 'Bearer t' },
      });
      expect(res.status).toBe(200);
    } finally {
      warnSpy.mockRestore();
    }
  });

  it('invalid SL_ENDPOINT rejects with ServeError InvalidEndpoint carrying the offending value', async () => {
    process.env.SL_ENDPOINT = 'nope';
    const err = await serve(handler, manifest).catch((e) => e);
    expect(err.name).toBe('ServeError');
    expect(err.kind).toBe('InvalidEndpoint');
    expect(err.value).toBe('nope');
  });

  it('empty-string SL_ENDPOINT is an InvalidEndpoint, not Missing (rs parity: the value fails to parse)', async () => {
    process.env.SL_ENDPOINT = '';
    await expect(serve(handler, manifest)).rejects.toMatchObject({ kind: 'InvalidEndpoint' });
  });

  it('tcp:127.0.0.1:0 expands to the real assigned port; shutdown drains and closes; wait() resolves', async () => {
    process.env.SL_ENDPOINT = 'tcp:127.0.0.1:0';
    process.env.SL_TOKEN = 't';
    info = await serve(handler, manifest);
    expect(info.endpoint).toEqual({ kind: 'tcp', host: '127.0.0.1', port: info.port });
    expect(typeof info.port).toBe('number');

    const base = `http://127.0.0.1:${info.port}`;
    const health = await fetch(`${base}/health`, { headers: { authorization: 'Bearer t' } });
    expect(await health.text()).toBe('{"id":"life-comp","version":"9.9.9"}');

    const waitP = info.wait(); // wait() may be called before shutdown
    await info.shutdown();
    await waitP;
    await expect(fetch(`${base}/health`)).rejects.toThrow(); // connection refused after full exit
  });

  it('uds serves HTTP over the socket; the socket file is removed on full exit (rs wait() parity)', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'slsdk-server-'));
    const sockPath = join(dir, 'test.sock');
    process.env.SL_ENDPOINT = `uds:${sockPath}`;
    process.env.SL_TOKEN = 't';
    try {
      info = await serve(handler, manifest);
      expect(info.endpoint.kind).toBe('uds');
      expect(existsSync(sockPath)).toBe(true);
      const res = await udsGet(sockPath, '/health', 't');
      expect(res.status).toBe(200);
      expect(res.body).toBe('{"id":"life-comp","version":"9.9.9"}');
      await info.shutdown();
      expect(existsSync(sockPath)).toBe(false);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it('a leftover socket file from a previous ungraceful exit does not block startup (bind-time cleanup)', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'slsdk-server-'));
    const sockPath = join(dir, 'test.sock');
    writeFileSync(sockPath, 'stale-leftover');
    process.env.SL_ENDPOINT = `uds:${sockPath}`;
    process.env.SL_TOKEN = 't';
    try {
      info = await serve(handler, manifest);
      expect(info.endpoint.kind).toBe('uds');
      const res = await udsGet(sockPath, '/health', 't');
      expect(res.status).toBe(200);
    } finally {
      await info?.shutdown();
      info = undefined;
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it('a stdin "shutdown" line triggers the same graceful exit as SIGTERM (rs Windows fallback parity)', async () => {
    const mockStdin = new Readable({ read() {} });
    const stdinSpy = vi
      .spyOn(process, 'stdin', 'get')
      .mockReturnValue(mockStdin as unknown as typeof process.stdin);
    process.env.SL_ENDPOINT = 'tcp:127.0.0.1:0';
    process.env.SL_TOKEN = 't';
    try {
      info = await serve(handler, manifest);
      const base = `http://127.0.0.1:${info.port}`;
      mockStdin.push('shutdown\n'); // the shell's graceful-exit command over stdin
      await info.wait(); // resolves only once the graceful exit has fully completed
      await expect(fetch(`${base}/health`)).rejects.toThrow(); // listener is gone
    } finally {
      stdinSpy.mockRestore();
    }
  });

  it('stdin watcher command semantics (rs parity): trimmed exact match, case-sensitive, other lines ignored', async () => {
    const input = new Readable({ read() {} });
    const requestShutdown = vi.fn();
    spawnStdinShutdownWatcher(requestShutdown, input);
    input.push('SHUTDOWN\n'); // case mismatch → ignored
    input.push('shutdown -x\n'); // not exactly "shutdown" after trim → ignored
    input.push('hello\n'); // unrelated line → ignored (rs loop-continue)
    await new Promise((resolve) => setTimeout(resolve, 20)); // let the line events fire
    expect(requestShutdown).not.toHaveBeenCalled();
    input.push('  shutdown \n'); // whitespace-trimmed match (rs line.trim() == "shutdown")
    await vi.waitFor(() => expect(requestShutdown).toHaveBeenCalledOnce());
    input.push(null); // stdin close ends the watcher, no throw (rs break-on-closed)
    await new Promise((resolve) => setTimeout(resolve, 20));
  });

  it('SIGTERM/SIGINT handlers are registered at startup and removed after full exit', async () => {
    const onceSpy = vi.spyOn(process, 'once');
    const offSpy = vi.spyOn(process, 'removeListener');
    process.env.SL_ENDPOINT = 'tcp:127.0.0.1:0';
    process.env.SL_TOKEN = 't';
    try {
      info = await serve(handler, manifest);
      expect(onceSpy).toHaveBeenCalledWith('SIGTERM', expect.any(Function));
      expect(onceSpy).toHaveBeenCalledWith('SIGINT', expect.any(Function));
      await info.shutdown();
      await vi.waitFor(() => {
        expect(offSpy).toHaveBeenCalledWith('SIGTERM', expect.any(Function));
        expect(offSpy).toHaveBeenCalledWith('SIGINT', expect.any(Function));
      });
    } finally {
      onceSpy.mockRestore();
      offSpy.mockRestore();
    }
  });

  it('shutdown() is idempotent; wait() resolves idempotently', async () => {
    process.env.SL_ENDPOINT = 'tcp:127.0.0.1:0';
    process.env.SL_TOKEN = 't';
    info = await serve(handler, manifest);
    await Promise.all([info.shutdown(), info.shutdown(), info.wait(), info.wait()]);
  });

  it('binary request bodies round-trip byte-exact through the bridge (streamed, not stringified)', async () => {
    process.env.SL_ENDPOINT = 'tcp:127.0.0.1:0';
    process.env.SL_TOKEN = 't';
    info = await serve(async (req) => new Response(await req.arrayBuffer()), manifest);
    const bytes = Uint8Array.from([0, 1, 2, 250, 251, 252, 253, 254, 255]);
    const res = await fetch(`http://127.0.0.1:${info.port}/bin`, {
      method: 'POST',
      headers: { authorization: 'Bearer t', 'content-type': 'application/octet-stream' },
      body: bytes,
    });
    expect(new Uint8Array(await res.arrayBuffer())).toEqual(bytes);
  });

  it('a throwing business handler surfaces as an empty 500 over the wire', async () => {
    process.env.SL_ENDPOINT = 'tcp:127.0.0.1:0';
    process.env.SL_TOKEN = 't';
    info = await serve(() => {
      throw new Error('boom');
    }, manifest);
    const res = await fetch(`http://127.0.0.1:${info.port}/x`, { headers: { authorization: 'Bearer t' } });
    expect(res.status).toBe(500);
  });
});
