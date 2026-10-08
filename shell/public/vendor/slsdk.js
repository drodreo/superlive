import * as __WEBPACK_EXTERNAL_MODULE_rxjs__ from "rxjs";
function adaptBody(body) {
    if (void 0 === body) return {
        body: void 0
    };
    if (body instanceof FormData) return {
        body
    };
    if (body instanceof Blob) return {
        body,
        contentType: body.type || 'application/octet-stream'
    };
    if (body instanceof ReadableStream) return {
        body,
        contentType: 'application/octet-stream'
    };
    if (body instanceof ArrayBuffer) return {
        body,
        contentType: 'application/octet-stream'
    };
    if (body instanceof Uint8Array) return {
        body: body,
        contentType: 'application/octet-stream'
    };
    return {
        body: JSON.stringify(body),
        contentType: 'application/json'
    };
}
class ApiError extends Error {
    status;
    body;
    raw;
    constructor(message, status, body, raw){
        super(message);
        this.name = 'ApiError';
        this.status = status;
        this.body = body;
        this.raw = raw;
    }
    static async from(res) {
        let body;
        let summary = '';
        try {
            body = await res.clone().json();
            summary = JSON.stringify(body) ?? '';
        } catch  {
            try {
                const text = await res.clone().text();
                body = text;
                summary = text;
            } catch  {
                body = void 0;
            }
        }
        const message = summary ? `API ${res.status}: ${summary.slice(0, 200)}` : `API ${res.status}`;
        return new ApiError(message, res.status, body, res);
    }
}
function createObserve(baseUrl, token) {
    return function(path, opts) {
        if (!path.startsWith('/')) throw new TypeError(`path \u{5FC5}\u{987B}\u{4EE5} / \u{5F00}\u{5934}\u{FF0C}\u{6536}\u{5230}\u{FF1A}${path}`);
        const wsUrl = `${baseUrl.replace(/^http/, 'ws')}${path}?token=${encodeURIComponent(token)}`;
        const wsFactory = opts?.wsFactory ?? ((url)=>new WebSocket(url));
        const retry = {
            initialDelayMs: 500,
            factor: 2,
            maxDelayMs: 30000,
            ...opts?.retry
        };
        return new __WEBPACK_EXTERNAL_MODULE_rxjs__.Observable((subscriber)=>{
            let socket = null;
            let reconnectTimer = null;
            let attempt = 0;
            let closedByUser = false;
            const clearReconnectTimer = ()=>{
                if (null !== reconnectTimer) {
                    clearTimeout(reconnectTimer);
                    reconnectTimer = null;
                }
            };
            const connect = ()=>{
                if (closedByUser) return;
                socket = wsFactory(wsUrl);
                socket.onopen = ()=>{
                    attempt = 0;
                };
                socket.onmessage = (ev)=>{
                    let payload;
                    try {
                        payload = JSON.parse(String(ev.data));
                    } catch  {
                        payload = ev.data;
                    }
                    subscriber.next(payload);
                };
                socket.onclose = ()=>{
                    if (closedByUser) return;
                    const delay = Math.min(retry.initialDelayMs * retry.factor ** attempt, retry.maxDelayMs);
                    attempt += 1;
                    reconnectTimer = setTimeout(connect, delay);
                };
            };
            connect();
            return ()=>{
                closedByUser = true;
                clearReconnectTimer();
                socket?.close();
            };
        });
    };
}
class SdkResponse {
    raw;
    constructor(raw){
        this.raw = raw;
    }
    get status() {
        return this.raw.status;
    }
    get headers() {
        return this.raw.headers;
    }
    get stream() {
        if (!this.raw.body) throw new Error("\u54CD\u5E94\u4F53\u6CA1\u6709\u53EF\u8BFB\u6D41\uFF08\u5982 204/HEAD \u54CD\u5E94\uFF0C\u6216\u6D41\u5DF2\u88AB\u6D88\u8D39\uFF09");
        return this.raw.body;
    }
    json() {
        return this.raw.json();
    }
    text() {
        return this.raw.text();
    }
    bytes() {
        return this.raw.arrayBuffer().then((buf)=>new Uint8Array(buf));
    }
}
function assertPath(path) {
    if (!path.startsWith('/')) throw new TypeError(`path \u{5FC5}\u{987B}\u{4EE5} / \u{5F00}\u{5934}\u{FF0C}\u{6536}\u{5230}\u{FF1A}${path}`);
}
function createClient(options, config) {
    const fetchImpl = config?.fetchImpl ?? globalThis.fetch.bind(globalThis);
    const baseUrl = options.baseUrl.replace(/\/+$/, '');
    const observe = createObserve(baseUrl, options.token);
    async function doRequest(method, path, body, opts) {
        const url = new URL(baseUrl + path);
        if (opts?.query) for (const [key, value] of Object.entries(opts.query))url.searchParams.set(key, String(value));
        const adapted = adaptBody(body);
        const headers = {};
        if (adapted.contentType) headers['Content-Type'] = adapted.contentType;
        headers['Authorization'] = `Bearer ${options.token}`;
        if (opts?.headers) Object.assign(headers, opts.headers);
        const init = {
            method,
            headers
        };
        if (opts?.signal !== void 0) init.signal = opts.signal;
        if (void 0 !== adapted.body) {
            if (adapted.body instanceof ReadableStream) init.duplex = 'half';
            init.body = adapted.body;
        }
        const res = await fetchImpl(url.toString(), init);
        if (!res.ok) throw await ApiError.from(res);
        return new SdkResponse(res);
    }
    function request(method, path, body, opts) {
        assertPath(path);
        return doRequest(method, path, body, opts);
    }
    return {
        request,
        get: (path, opts)=>request('GET', path, void 0, opts),
        post: (path, body, opts)=>request('POST', path, body, opts),
        put: (path, body, opts)=>request('PUT', path, body, opts),
        delete: (path, opts)=>request('DELETE', path, void 0, opts),
        head: (path, opts)=>request('HEAD', path, void 0, opts),
        observe
    };
}
export { ApiError, SdkResponse, createClient };
