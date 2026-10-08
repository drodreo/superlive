import { adaptBody } from './body';
import { ApiError } from './errors';
import { createObserve, type ObserveOptions } from './observe';
import { SdkResponse } from './response';
import type { ClientOptions, RequestBody, RequestOptions } from './types';
import type { Observable } from 'rxjs';

/** The five verbs in the Plane A contract vocabulary (see docs/architecture.md) */
export type HttpMethod = 'GET' | 'POST' | 'PUT' | 'DELETE' | 'HEAD';

export interface SdkClient {
  request(
    method: HttpMethod,
    path: string,
    body?: RequestBody,
    opts?: RequestOptions,
  ): Promise<SdkResponse>;
  get(path: string, opts?: RequestOptions): Promise<SdkResponse>;
  post(path: string, body?: RequestBody, opts?: RequestOptions): Promise<SdkResponse>;
  put(path: string, body?: RequestBody, opts?: RequestOptions): Promise<SdkResponse>;
  delete(path: string, opts?: RequestOptions): Promise<SdkResponse>;
  head(path: string, opts?: RequestOptions): Promise<SdkResponse>;
  /** Subscribe to the shell's one-way pushes over WebSocket (auto-reconnect on disconnect; see observe.ts) */
  observe<T>(path: string, opts?: ObserveOptions): Observable<T>;
}

export interface CreateClientConfig {
  /** fetch implementation injection point: for test stubs and customized in-shell environments */
  fetchImpl?: typeof fetch;
}

/** Path validation lives in a non-async wrapper layer so bad concatenation throws synchronously at call time instead of surfacing as a Promise rejection */
function assertPath(path: string): void {
  if (!path.startsWith('/')) {
    throw new TypeError(`path must start with /, got: ${path}`);
  }
}

export function createClient(options: ClientOptions, config?: CreateClientConfig): SdkClient {
  const fetchImpl = config?.fetchImpl ?? globalThis.fetch.bind(globalThis);
  // Trailing slash normalization: a baseUrl with a trailing slash would produce a //path double slash on concatenation (undefined gateway behavior),
  // handled once at configuration time; request and observe share the normalized result
  const baseUrl = options.baseUrl.replace(/\/+$/, '');
  const observe = createObserve(baseUrl, options.token);

  async function doRequest(
    method: HttpMethod,
    path: string,
    body?: RequestBody,
    opts?: RequestOptions,
  ): Promise<SdkResponse> {
    const url = new URL(baseUrl + path);
    if (opts?.query) {
      for (const [key, value] of Object.entries(opts.query)) {
        url.searchParams.set(key, String(value));
      }
    }

    const adapted = adaptBody(body);
    // Merge order: Content-Type → Authorization → user headers; writing user headers last lets them override the former two
    const headers: Record<string, string> = {};
    if (adapted.contentType) {
      headers['Content-Type'] = adapted.contentType;
    }
    headers['Authorization'] = `Bearer ${options.token}`;
    if (opts?.headers) {
      Object.assign(headers, opts.headers);
    }

    const init: RequestInit = { method, headers };
    if (opts?.signal !== undefined) {
      init.signal = opts.signal;
    }
    if (adapted.body !== undefined) {
      if (adapted.body instanceof ReadableStream) {
        // The fetch spec requires streaming bodies to declare duplex explicitly; the TS RequestInit type does not cover this field yet (the only as-any exception in the repo)
        (init as any).duplex = 'half';
      }
      init.body = adapted.body;
    }

    // Network-level errors (fetch rejects) propagate as-is, kept separate from protocol-level errors (ApiError)
    const res = await fetchImpl(url.toString(), init);
    if (!res.ok) {
      throw await ApiError.from(res);
    }
    return new SdkResponse(res);
  }

  function request(
    method: HttpMethod,
    path: string,
    body?: RequestBody,
    opts?: RequestOptions,
  ): Promise<SdkResponse> {
    assertPath(path);
    return doRequest(method, path, body, opts);
  }

  return {
    request,
    get: (path, opts) => request('GET', path, undefined, opts),
    post: (path, body, opts) => request('POST', path, body, opts),
    put: (path, body, opts) => request('PUT', path, body, opts),
    delete: (path, opts) => request('DELETE', path, undefined, opts),
    head: (path, opts) => request('HEAD', path, undefined, opts),
    observe,
  };
}
