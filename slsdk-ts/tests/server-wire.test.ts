import { describe, expect, it } from 'vitest';
import { composeHandler } from '../src/server/handler';
import type { ServerManifest } from '../src/server/manifest';

const manifest: ServerManifest = {
  id: 'wire-comp',
  version: '1.2.3',
  min_shell_version: '0.0.1',
  namespace: 'wire',
  display_name: 'Wire',
};

const noop = (): Response => new Response('ok');

describe('composeHandler wire contract (one-to-one with slsdk-rs serve.rs)', () => {
  it('GET /health returns exactly the {"id","version"} identity payload', async () => {
    const handle = composeHandler(noop, manifest, null);
    const res = await handle(new Request('http://127.0.0.1:0/health'));
    expect(res.status).toBe(200);
    expect(res.headers.get('content-type')).toBe('application/json');
    // byte-exact: serde_json::json!({"id": .., "version": ..}) parity
    expect(await res.text()).toBe('{"id":"wire-comp","version":"1.2.3"}');
  });

  it('/health matches on the path only (query string ignored, axum route parity)', async () => {
    const handle = composeHandler(noop, manifest, null);
    const res = await handle(new Request('http://127.0.0.1:0/health?probe=1'));
    expect(res.status).toBe(200);
  });

  it('HEAD /health → 200; POST /health → 405 with an allow header (reserved path)', async () => {
    const handle = composeHandler(noop, manifest, null);
    const head = await handle(new Request('http://x/health', { method: 'HEAD' }));
    expect(head.status).toBe(200);
    const post = await handle(new Request('http://x/health', { method: 'POST' }));
    expect(post.status).toBe(405);
    expect(post.headers.get('allow')).toBe('GET, HEAD');
  });

  it('token wall applies to every route including /health (the health probe carries the token too)', async () => {
    const handle = composeHandler(noop, manifest, 'tok-1');
    // no header / wrong scheme / wrong token / bare scheme — all 401 + WWW-Authenticate: Bearer
    for (const auth of [undefined, 'Basic tok-1', 'Bearer wrong', 'Bearer']) {
      const headers = auth === undefined ? undefined : { authorization: auth };
      const res = await handle(new Request('http://x/health', { headers }));
      expect(res.status, `auth=${auth}`).toBe(401);
      expect(res.headers.get('www-authenticate'), `auth=${auth}`).toBe('Bearer');
    }
    // wrong token on a business path is equally walled
    const biz = await handle(new Request('http://x/api/wire/x', { headers: { authorization: 'Bearer nope' } }));
    expect(biz.status).toBe(401);
  });

  it('bearer scheme is case-insensitive (RFC 7235) and the token part is trim_started', async () => {
    const handle = composeHandler(noop, manifest, 'tok-1');
    const lower = await handle(new Request('http://x/health', { headers: { authorization: 'bearer tok-1' } }));
    expect(lower.status).toBe(200);
    // inner whitespace after the scheme survives header parsing; trim_start mirrors rs
    const spaced = await handle(new Request('http://x/health', { headers: { authorization: 'Bearer   tok-1' } }));
    expect(spaced.status).toBe(200);
  });

  it('token wall precedes /health: valid token on /health → 200, valid token on POST /health → 405', async () => {
    const handle = composeHandler(noop, manifest, 'tok-1');
    const ok = await handle(new Request('http://x/health', { headers: { authorization: 'Bearer tok-1' } }));
    expect(ok.status).toBe(200);
    const post = await handle(
      new Request('http://x/health', { method: 'POST', headers: { authorization: 'Bearer tok-1' } }),
    );
    expect(post.status).toBe(405);
  });

  it('business handler receives the request verbatim and its Response is served', async () => {
    const handle = composeHandler(async (req) => {
      const body = await req.text();
      return new Response(`echo:${req.method}:${new URL(req.url).pathname}:${req.headers.get('x-trace')}:${body}`, {
        status: 201,
        headers: { 'x-echo': 'yes' },
      });
    }, manifest, null);
    const res = await handle(
      new Request('http://127.0.0.1:0/api/wire/items?x=1', {
        method: 'POST',
        headers: { 'x-trace': 't9', 'content-type': 'text/plain' },
        body: 'payload-bytes',
      }),
    );
    expect(res.status).toBe(201);
    expect(res.headers.get('x-echo')).toBe('yes');
    expect(await res.text()).toBe('echo:POST:/api/wire/items:t9:payload-bytes');
  });

  it('a handler rejection maps to an empty 500, keeping the contract typed', async () => {
    const handle = composeHandler(() => {
      throw new Error('boom');
    }, manifest, null);
    const res = await handle(new Request('http://x/x'));
    expect(res.status).toBe(500);
    expect(await res.text()).toBe('');
  });
});
