import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createObserve } from '../src/observe';

/** Simplified EventTarget mock: implements only the interface surface observe.ts depends on; the serverXxx methods simulate server-side events */
class MockWebSocket {
  static instances: MockWebSocket[] = [];

  url: string;
  closed = false;
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
  }

  close(): void {
    this.closed = true;
  }

  serverOpen(): void {
    this.onopen?.();
  }

  serverMessage(data: string): void {
    this.onmessage?.({ data });
  }

  serverClose(): void {
    this.onclose?.();
  }
}

function factory(): (url: string) => WebSocket {
  return (url) => new MockWebSocket(url) as unknown as WebSocket;
}

function lastInstance(): MockWebSocket | undefined {
  return MockWebSocket.instances[MockWebSocket.instances.length - 1];
}

describe('createObserve', () => {
  beforeEach(() => {
    MockWebSocket.instances = [];
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('URL: http→ws protocol replacement + token via query (cold Observable, connects on subscribe)', () => {
    const observe = createObserve('http://127.0.0.1:39876/api', 'sec-ret');
    const sub = observe('/todo/feed', { wsFactory: factory() }).subscribe(() => {});
    expect(MockWebSocket.instances).toHaveLength(1);
    expect(MockWebSocket.instances[0]?.url).toBe(
      'ws://127.0.0.1:39876/api/todo/feed?token=sec-ret',
    );
    sub.unsubscribe();
  });

  it('https → wss', () => {
    const observe = createObserve('https://shell.example.com/api', 't');
    const sub = observe('/feed', { wsFactory: factory() }).subscribe(() => {});
    expect(MockWebSocket.instances[0]?.url.startsWith('wss://')).toBe(true);
    sub.unsubscribe();
  });

  it('token special characters go through encodeURIComponent', () => {
    const observe = createObserve('http://x/api', 'a/b?c=d');
    const sub = observe('/feed', { wsFactory: factory() }).subscribe(() => {});
    expect(lastInstance()?.url).toBe('ws://x/api/feed?token=a%2Fb%3Fc%3Dd');
    sub.unsubscribe();
  });

  it('path not starting with / → throws a TypeError synchronously before subscribing', () => {
    const observe = createObserve('http://x/api', 't');
    expect(() => observe('feed', { wsFactory: factory() })).toThrow(TypeError);
  });

  it('message: next after JSON parse; non-JSON arrives as the raw string', () => {
    const observe = createObserve('http://x/api', 't');
    const values: unknown[] = [];
    const sub = observe('/feed', { wsFactory: factory() }).subscribe((v) => values.push(v));
    const ws = MockWebSocket.instances[0];
    ws?.serverOpen();
    ws?.serverMessage('{"a":1}');
    ws?.serverMessage('plain-text');
    expect(values).toEqual([{ a: 1 }, 'plain-text']);
    sub.unsubscribe();
  });

  it('reconnects with exponential backoff after close (default first delay 500ms)', () => {
    const observe = createObserve('http://x/api', 't');
    observe('/feed', { wsFactory: factory() }).subscribe(() => {});
    expect(MockWebSocket.instances).toHaveLength(1);
    MockWebSocket.instances[0]?.serverClose();
    vi.advanceTimersByTime(499);
    expect(MockWebSocket.instances).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(MockWebSocket.instances).toHaveLength(2);
  });

  it('backoff counter resets after a successful open: a later drop starts from 500ms again', () => {
    const observe = createObserve('http://x/api', 't');
    observe('/feed', { wsFactory: factory() }).subscribe(() => {});
    MockWebSocket.instances[0]?.serverClose();
    vi.advanceTimersByTime(500);
    const second = MockWebSocket.instances[1];
    second?.serverOpen(); // resets the counter
    second?.serverClose();
    vi.advanceTimersByTime(499);
    expect(MockWebSocket.instances).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(MockWebSocket.instances).toHaveLength(3);
  });

  it('consecutive failures cap the backoff at maxDelayMs (custom retry: 100×3³=2700 → capped at 900)', () => {
    const observe = createObserve('http://x/api', 't');
    observe('/feed', {
      wsFactory: factory(),
      retry: { initialDelayMs: 100, factor: 3, maxDelayMs: 900 },
    }).subscribe(() => {});
    // Close one by one without opening: the delay sequence should be 100 → 300 → 900 → 900
    MockWebSocket.instances[0]?.serverClose();
    vi.advanceTimersByTime(99);
    expect(MockWebSocket.instances).toHaveLength(1);
    vi.advanceTimersByTime(1); // +100ms → the 2nd
    MockWebSocket.instances[1]?.serverClose();
    vi.advanceTimersByTime(299);
    expect(MockWebSocket.instances).toHaveLength(2);
    vi.advanceTimersByTime(1); // +300ms → the 3rd
    MockWebSocket.instances[2]?.serverClose();
    vi.advanceTimersByTime(899);
    expect(MockWebSocket.instances).toHaveLength(3);
    vi.advanceTimersByTime(1); // +900ms → the 4th
    MockWebSocket.instances[3]?.serverClose();
    vi.advanceTimersByTime(899);
    expect(MockWebSocket.instances).toHaveLength(4);
    vi.advanceTimersByTime(1); // capped at 900ms → the 5th
    expect(MockWebSocket.instances).toHaveLength(5);
  });

  it('onerror does not terminate the stream: reconnection is left to onclose, messages keep flowing after reconnect', () => {
    const observe = createObserve('http://x/api', 't');
    const values: unknown[] = [];
    const sub = observe('/feed', { wsFactory: factory() }).subscribe((v) => values.push(v));
    const first = MockWebSocket.instances[0];
    first?.serverOpen();
    first?.serverMessage('{"n":1}');
    first?.onerror?.(); // should neither emit an error nor tear down the stream
    first?.serverClose(); // standard sequence: close follows error
    vi.advanceTimersByTime(500);
    expect(MockWebSocket.instances).toHaveLength(2);
    const second = MockWebSocket.instances[1];
    second?.serverOpen();
    second?.serverMessage('{"n":2}');
    expect(values).toEqual([{ n: 1 }, { n: 2 }]);
    sub.unsubscribe();
  });

  it('unsubscribe: clears the reconnect timer and closes the socket, no further reconnects', () => {
    const observe = createObserve('http://x/api', 't');
    const sub = observe('/feed', { wsFactory: factory() }).subscribe(() => {});
    const ws = MockWebSocket.instances[0];
    ws?.serverClose(); // scheduled a 500ms reconnect
    sub.unsubscribe();
    expect(ws?.closed).toBe(true);
    vi.advanceTimersByTime(60000);
    expect(MockWebSocket.instances).toHaveLength(1);
  });
});
