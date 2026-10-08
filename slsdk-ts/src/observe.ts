import { Observable } from 'rxjs';

export interface RetryOptions {
  /** Initial reconnect delay, defaults to 500ms */
  initialDelayMs?: number;
  /** Backoff multiplier, defaults to 2 */
  factor?: number;
  /** Reconnect delay cap, defaults to 30000ms */
  maxDelayMs?: number;
}

export interface ObserveOptions {
  /** WebSocket constructor injection point: for test stubs and special runtimes */
  wsFactory?: (url: string) => WebSocket;
  retry?: RetryOptions;
}

/**
 * Creates the observe instance method attached to the client (the closure holds baseUrl/token).
 * Receive-only: server pushes arrive via WebSocket message → JSON parse (falls back to the raw string on failure) → next;
 * the send direction does not go through the ws (use post instead).
 */
export function createObserve(baseUrl: string, token: string) {
  return function observe<T>(path: string, opts?: ObserveOptions): Observable<T> {
    if (!path.startsWith('/')) {
      throw new TypeError(`path must start with /, got: ${path}`);
    }
    // ws cannot carry custom headers, so the token travels via the ?token= query — a trade-off already settled in the architecture doc
    const wsUrl = `${baseUrl.replace(/^http/, 'ws')}${path}?token=${encodeURIComponent(token)}`;
    const wsFactory = opts?.wsFactory ?? ((url: string) => new WebSocket(url));
    const retry = {
      initialDelayMs: 500,
      factor: 2,
      maxDelayMs: 30000,
      ...opts?.retry,
    };

    return new Observable<T>((subscriber) => {
      let socket: WebSocket | null = null;
      let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
      let attempt = 0;
      let closedByUser = false;

      const clearReconnectTimer = () => {
        if (reconnectTimer !== null) {
          clearTimeout(reconnectTimer);
          reconnectTimer = null;
        }
      };

      const connect = () => {
        if (closedByUser) {
          return;
        }
        socket = wsFactory(wsUrl);
        socket.onopen = () => {
          // Reset the backoff counter after a successful connection so the next disconnect restarts from the first delay tier
          attempt = 0;
        };
        socket.onmessage = (ev: MessageEvent) => {
          let payload: unknown;
          try {
            payload = JSON.parse(String(ev.data));
          } catch {
            payload = ev.data;
          }
          subscriber.next(payload as T);
        };
        // In the standard WebSocket sequence onclose always follows onerror; reconnection is handled uniformly in onclose and the stream is not torn down on error
        socket.onclose = () => {
          if (closedByUser) {
            return;
          }
          const delay = Math.min(retry.initialDelayMs * retry.factor ** attempt, retry.maxDelayMs);
          attempt += 1;
          reconnectTimer = setTimeout(connect, delay);
        };
      };

      connect();

      return () => {
        closedByUser = true;
        clearReconnectTimer();
        socket?.close();
      };
    });
  };
}
