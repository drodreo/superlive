// SurfaceHost tests — the shell-side host for surface-tier components.
//
// First test suite of the shell (vitest + jsdom + @testing-library/react;
// no prior test tradition exists here). Coverage maps onto the SurfaceHost
// contract:
// - description document shapes: bare message array / {dataSpec, messages}
//   envelope / invalid JSON / shape mismatch / unreachable document / retry;
// - dataSpec.queries at mount: gateway-shaped /api/{ns}/{path} requests with
//   params (null entries dropped), results landing in the surface data model;
// - functionCall HTTP actions: the renderer evaluates the Action declaration
//   (literal / row-scoped bindings / predicate-computed bodies) and executes
//   it against the host's HTTP function family — gateway-shaped requests,
//   query serialization, missing path → error face;
// - refresh semantics: non-2xx / network failure / missing path never
//   refresh, a 2xx response re-runs every declared query, {event} actions
//   have no shell-side consumer.
//
// The network is mocked at the global fetch boundary (vi.stubGlobal — no
// msw); the tauri core module is mocked so shellApi's token injection runs
// for real while no tauri runtime is present.

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { A2uiSurface, type ServerToClientMessage } from "@shell/sdk/ui";
import { SurfaceHost } from "../SurfaceHost";
import type { ComponentInfo } from "../api";

const ORIGIN = "http://127.0.0.1:39876";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === "shell_token") return "test-token";
    throw new Error(`unexpected tauri command: ${cmd}`);
  }),
}));

// ===== helpers =====

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function makeInfo(overrides: Partial<ComponentInfo> = {}): ComponentInfo {
  return {
    id: "mycomp",
    version: "1.0.0",
    namespace: "testns",
    display_name: "My Comp",
    running: true,
    ui: { ui_type: "surface", entry: "surface.json" },
    menu: null,
    routes: [],
    update: null,
    source: "installed",
    project_dir: null,
    ...overrides,
  };
}

/**
 * A minimal surface description: one Button (the root) whose action context
 * carries /selectedId from the data model, labelled by a Text child.
 */
function buttonSurface(
  opts: { withModel?: boolean; actionName?: string } = {},
): ServerToClientMessage[] {
  const messages: ServerToClientMessage[] = [
    {
      version: "v0.9.1",
      createSurface: { surfaceId: "s1", catalogId: "superlive" },
    },
  ];
  if (opts.withModel !== false) {
    messages.push({
      version: "v0.9.1",
      updateDataModel: {
        surfaceId: "s1",
        path: "/selectedId",
        value: "t-42",
      },
    });
  }
  messages.push({
    version: "v0.9.1",
    updateComponents: {
      surfaceId: "s1",
      components: [
        { id: "lbl", component: "Text", text: "Delete item" },
        {
          id: "root",
          component: "Button",
          child: "lbl",
          action: {
            event: {
              name: opts.actionName ?? "action",
              context: { id: { path: "/selectedId" } },
            },
          },
        },
      ],
    },
  });
  return messages;
}

interface Route {
  url: string;
  respond: (url: string, init: RequestInit | undefined) => Response;
}

function stubFetch(routes: Route[]): ReturnType<typeof vi.fn> {
  const fetchMock = vi.fn(
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      const route = routes.find(
        (candidate) => url === candidate.url || url.startsWith(candidate.url),
      );
      if (!route) throw new Error(`unexpected fetch: ${url}`);
      return route.respond(url, init);
    },
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function callsTo(
  fetchMock: ReturnType<typeof vi.fn>,
  url: string,
): Array<Array<unknown>> {
  return fetchMock.mock.calls.filter(([called]) => String(called) === url);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  // restore console spies; the module-level tauri mock stays (its token is
  // cached after the first shellApi call anyway)
  vi.restoreAllMocks();
});

// ===== smoke: the test facility itself =====

describe("smoke", () => {
  it("renders a Button through @shell/sdk/ui in jsdom", async () => {
    render(<A2uiSurface description={buttonSurface()} />);
    await screen.findByText("Delete item");
    expect(screen.getByRole("button")).toBeTruthy();
  });
});

// ===== description document shapes =====

describe("description document loading", () => {
  it("accepts the bare message-array form", async () => {
    stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () => jsonResponse(buttonSurface()),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    await screen.findByText("Delete item");
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("accepts the {dataSpec, messages} envelope form", async () => {
    stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () =>
          jsonResponse({ dataSpec: {}, messages: buttonSurface() }),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    await screen.findByText("Delete item");
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("shows the error face when the document is not valid JSON", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () => new Response("not json{{{", { status: 200 }),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    await screen.findByRole("button", { name: "Retry" });
  });

  it("shows the error face when the document is neither form", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () => jsonResponse({ hello: "world" }),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    await screen.findByRole("button", { name: "Retry" });
  });

  it("shows the error face when the document is unreachable (404)", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () => new Response("nope", { status: 404 }),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    await screen.findByRole("button", { name: "Retry" });
  });

  it("retries through the error face back to ready", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    let failing = true;
    const fetchMock = stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () =>
          failing
            ? new Response("nope", { status: 404 })
            : jsonResponse(buttonSurface()),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    await screen.findByRole("button", { name: "Retry" });
    failing = false;
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await screen.findByText("Delete item");
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });
});

// ===== dataSpec.queries at mount =====

describe("dataSpec queries at mount", () => {
  const queryDescription = (
    request: Record<string, unknown>,
  ): Record<string, unknown> => ({
    dataSpec: { queries: [{ pointer: "/greeting", request }] },
    messages: [
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            { id: "root", component: "Text", text: { path: "/greeting" } },
          ],
        },
      },
    ],
  });

  it("runs queries through the gateway (/api/{ns}/{path}) and lands results in the data model", async () => {
    const fetchMock = stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () => jsonResponse(queryDescription({ path: "hello" })),
      },
      {
        url: `${ORIGIN}/api/testns/hello`,
        respond: () => jsonResponse("hello-items"),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    await screen.findByText("hello-items");
    expect(fetchMock).toHaveBeenCalledWith(
      `${ORIGIN}/api/testns/hello`,
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("serializes query params and drops null entries", async () => {
    const fetchMock = stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () =>
          jsonResponse(
            queryDescription({
              path: "hello",
              params: { a: "1", b: null, c: 7 },
            }),
          ),
      },
      {
        url: `${ORIGIN}/api/testns/hello`,
        respond: () => jsonResponse("hello-items"),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    await screen.findByText("hello-items");
    expect(fetchMock).toHaveBeenCalledWith(
      `${ORIGIN}/api/testns/hello?a=1&c=7`,
      expect.anything(),
    );
  });

  it("shows the error face when a query fails", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () => jsonResponse(queryDescription({ path: "hello" })),
      },
      {
        url: `${ORIGIN}/api/testns/hello`,
        respond: () => new Response("boom", { status: 500 }),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    await screen.findByRole("button", { name: "Retry" });
  });
});

// ===== dataSpec load-time validation =====

describe("dataSpec load-time validation", () => {
  // A malformed spec is rejected before the query loop runs: the error face
  // must appear (findByRole would time out against a stuck loading phase)
  // naming the violation, and no API call besides the description itself.
  // The violation detail is asserted on the console.error payload (the raw
  // message): the error face interpolates it through i18next, which HTML-
  // escapes quotes, so DOM text assertions would bind to escaping details.
  function envelopeWithSpec(dataSpec: unknown): Record<string, unknown> {
    return { dataSpec, messages: buttonSurface() };
  }

  it("shows the error face when queries is not an array (no hang, no API call)", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const fetchMock = stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () => jsonResponse(envelopeWithSpec({ queries: {} })),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    await screen.findByRole("button", { name: "Retry" });
    expect(errorSpy).toHaveBeenCalledWith(
      expect.stringContaining("queries must be an array"),
    );
    // only the description document was fetched — the query loop never ran
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});

// ===== functionCall HTTP actions =====

describe("functionCall HTTP actions", () => {
  const QUERY_URL = `${ORIGIN}/api/testns/todos`;
  const ACTION_URL = `${ORIGIN}/api/testns/todos/t-42`;

  /**
   * Envelope with a /todos mount query plus a Button carrying the given
   * Action declaration (the functionCall pipeline exercises it against the
   * host's HTTP function table).
   */
  function envelopeWithAction(
    action: Record<string, unknown>,
    opts: { model?: Record<string, unknown> } = {},
  ): Record<string, unknown> {
    const messages: unknown[] = [
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
    ];
    if (opts.model !== undefined) {
      messages.push({
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/", value: opts.model },
      });
    }
    messages.push({
      version: "v0.9.1",
      updateComponents: {
        surfaceId: "s1",
        components: [
          { id: "lbl", component: "Text", text: "Delete item" },
          { id: "root", component: "Button", child: "lbl", action },
        ],
      },
    });
    return {
      dataSpec: {
        queries: [{ pointer: "/todos", request: { path: "todos" } }],
      },
      messages,
    };
  }

  // Longest-prefix first: matching is `url === candidate.url ||
  // url.startsWith(candidate.url)` in registration order, so the action URL
  // (…/todos/t-42) must sit before the query URL (…/todos) — otherwise the
  // action request gets answered by the query route (a 200) and the non-2xx
  // path under test never happens.
  function stubActionRoutes(
    body: Record<string, unknown>,
    actionResponse: () => Response,
    extraRoutes: Route[] = [],
  ): ReturnType<typeof vi.fn> {
    return stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () => jsonResponse(body),
      },
      ...extraRoutes,
      { url: ACTION_URL, respond: actionResponse },
      { url: QUERY_URL, respond: () => jsonResponse([]) },
    ]);
  }

  it("executes an httpDelete action with a literal path through the gateway", async () => {
    const fetchMock = stubActionRoutes(
      envelopeWithAction({
        functionCall: { call: "httpDelete", args: { path: "todos/t-42" } },
      }),
      () => jsonResponse({ ok: true }),
    );
    render(<SurfaceHost info={makeInfo()} />);
    fireEvent.click(await screen.findByRole("button"));
    await waitFor(() => {
      expect(callsTo(fetchMock, ACTION_URL)).toHaveLength(1);
    });
    const [, init] = callsTo(fetchMock, ACTION_URL)[0] as [string, RequestInit];
    expect(init.method).toBe("DELETE");
    expect(init.body).toBeUndefined();
  });

  it("sends the evaluated body entries as the JSON request body", async () => {
    const fetchMock = stubActionRoutes(
      envelopeWithAction(
        {
          functionCall: {
            call: "httpPatch",
            args: {
              path: "todos/t-42",
              body: {
                completed: {
                  call: "not",
                  args: { value: { path: "/selectedDone" } },
                },
              },
            },
          },
        },
        { model: { selectedDone: false } },
      ),
      () => jsonResponse({ ok: true }),
    );
    render(<SurfaceHost info={makeInfo()} />);
    fireEvent.click(await screen.findByRole("button"));
    await waitFor(() => {
      expect(callsTo(fetchMock, ACTION_URL)).toHaveLength(1);
    });
    const [, init] = callsTo(fetchMock, ACTION_URL)[0] as [string, RequestInit];
    expect(new Headers(init.headers).get("content-type")).toBe(
      "application/json",
    );
    // The body entry was evaluated (the predicate ran): not(false) → true.
    expect(JSON.parse(init.body as string)).toEqual({ completed: true });
  });

  it("serializes query argument entries into the URL", async () => {
    const fetchMock = stubActionRoutes(
      envelopeWithAction(
        {
          functionCall: {
            call: "httpGet",
            args: { path: "todos", query: { id: { path: "/selectedId" } } },
          },
        },
        { model: { selectedId: "t-42" } },
      ),
      () => jsonResponse({ ok: true }),
      // The GET goes to the query space with params — a distinct URL.
      [
        {
          url: `${ORIGIN}/api/testns/todos?id=t-42`,
          respond: () => jsonResponse({ ok: true }),
        },
      ],
    );
    render(<SurfaceHost info={makeInfo()} />);
    fireEvent.click(await screen.findByRole("button"));
    await waitFor(() => {
      expect(
        callsTo(fetchMock, `${ORIGIN}/api/testns/todos?id=t-42`),
      ).toHaveLength(1);
    });
  });

  it("shows the error face when the path argument evaluates to nothing", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const fetchMock = stubActionRoutes(
      envelopeWithAction({
        functionCall: {
          call: "httpDelete",
          args: { path: { path: "/missing" } },
        },
      }),
      () => jsonResponse({ ok: true }),
    );
    render(<SurfaceHost info={makeInfo()} />);
    fireEvent.click(await screen.findByRole("button"));
    await screen.findByRole("button", { name: "Retry" });
    // The request never fired and the refresh never ran.
    expect(callsTo(fetchMock, ACTION_URL)).toHaveLength(0);
    expect(callsTo(fetchMock, QUERY_URL)).toHaveLength(1);
  });

  it("shows the error face and never refreshes on non-2xx", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const fetchMock = stubActionRoutes(
      envelopeWithAction({
        functionCall: { call: "httpDelete", args: { path: "todos/t-42" } },
      }),
      () => new Response("boom", { status: 500 }),
    );
    render(<SurfaceHost info={makeInfo()} />);
    fireEvent.click(await screen.findByRole("button"));
    await screen.findByRole("button", { name: "Retry" });
    // only the mount query ran; the refresh never fired
    expect(callsTo(fetchMock, QUERY_URL)).toHaveLength(1);
  });

  it("shows the error face and never refreshes on a network failure", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const fetchMock = stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () =>
          jsonResponse(
            envelopeWithAction({
              functionCall: {
                call: "httpDelete",
                args: { path: "todos/t-42" },
              },
            }),
          ),
      },
      {
        // The action request rejects at the network layer. Registered before
        // the shorter query prefix, or the query route answers it first.
        url: ACTION_URL,
        respond: () => {
          throw new TypeError("network down");
        },
      },
      {
        url: QUERY_URL,
        respond: () => jsonResponse([]),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    fireEvent.click(await screen.findByRole("button"));
    await screen.findByRole("button", { name: "Retry" });
    expect(callsTo(fetchMock, QUERY_URL)).toHaveLength(1);
  });

  it("re-runs every declared query after a 2xx response", async () => {
    const fetchMock = stubActionRoutes(
      envelopeWithAction({
        functionCall: { call: "httpDelete", args: { path: "todos/t-42" } },
      }),
      () => jsonResponse({ ok: true }),
    );
    render(<SurfaceHost info={makeInfo()} />);
    fireEvent.click(await screen.findByRole("button"));
    await waitFor(() => {
      expect(callsTo(fetchMock, QUERY_URL)).toHaveLength(2);
    });
  });

  it("leaves event actions without a consumer (no request fired)", async () => {
    const fetchMock = stubActionRoutes(
      envelopeWithAction({
        event: { name: "todo.delete", context: { id: "t-42" } },
      }),
      () => jsonResponse({ ok: true }),
    );
    render(<SurfaceHost info={makeInfo()} />);
    fireEvent.click(await screen.findByRole("button"));
    // Give the (non-existent) handler a tick: nothing but the mount query.
    await waitFor(() => {
      expect(callsTo(fetchMock, QUERY_URL)).toHaveLength(1);
    });
    expect(callsTo(fetchMock, ACTION_URL)).toHaveLength(0);
  });
});

// ===== push subscriptions (the ws plane) =====

/**
 * Controlled WebSocket double: observe.ts drives it through the onopen/
 * onmessage/onclose assignments, the host's repull hook adds an "open"
 * listener — the mock supports both registration styles and lets tests
 * stage the connection lifecycle by hand.
 */
class MockWebSocket {
  static instances: MockWebSocket[] = [];
  static reset(): void {
    MockWebSocket.instances = [];
  }
  url: string;
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  private listeners = new Map<string, Array<() => void>>();

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
  }
  addEventListener(type: string, handler: () => void): void {
    const list = this.listeners.get(type) ?? [];
    list.push(handler);
    this.listeners.set(type, list);
  }
  private dispatch(type: string): void {
    for (const handler of this.listeners.get(type) ?? []) handler();
    if (type === "open") this.onopen?.();
    if (type === "close") this.onclose?.();
    if (type === "error") this.onerror?.();
  }
  /** Test-side: the connection succeeded (first connect or reconnect). */
  open(): void {
    this.dispatch("open");
  }
  /** Test-side: the shell bridge forwarded one NDJSON line (payload only). */
  receive(line: string): void {
    this.onmessage?.({ data: line });
  }
  /** Test-side: the connection dropped (observe reconnects with backoff). */
  close(): void {
    this.dispatch("close");
  }
}

/** A surface whose root Text label is bound to the data model. */
function boundLabelSurface(): Record<string, unknown> {
  return {
    dataSpec: {},
    messages: [
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/label", value: "hello" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            { id: "root", component: "Text", text: { path: "/label" } },
          ],
        },
      },
    ],
  };
}

/** A surface whose root Input is bound to /draft (local state checks). */
function boundInputSurface(): Record<string, unknown> {
  return {
    dataSpec: {},
    messages: [
      {
        version: "v0.9.1",
        createSurface: { surfaceId: "s1", catalogId: "superlive" },
      },
      {
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/draft", value: "start" },
      },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: "s1",
          components: [
            { id: "root", component: "Input", value: { path: "/draft" } },
          ],
        },
      },
    ],
  };
}

function pushLine(payload: Record<string, unknown>): string {
  return JSON.stringify(payload);
}

describe("push subscriptions (ws plane)", () => {
  it("subscribes to the component's feed URL after ready", async () => {
    MockWebSocket.reset();
    vi.stubGlobal("WebSocket", MockWebSocket);
    stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () => jsonResponse(boundLabelSurface()),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    await screen.findByText("hello");
    await waitFor(() => {
      expect(MockWebSocket.instances).toHaveLength(1);
    });
    expect(MockWebSocket.instances[0].url).toBe(
      `ws://127.0.0.1:39876/subscriptions/testns/surface?token=test-token`,
    );
  });

  it("applies pushed messages into the live surface incrementally", async () => {
    MockWebSocket.reset();
    vi.stubGlobal("WebSocket", MockWebSocket);
    stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () => jsonResponse(boundLabelSurface()),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    await screen.findByText("hello");
    await waitFor(() => {
      expect(MockWebSocket.instances).toHaveLength(1);
    });
    const ws = MockWebSocket.instances[0];
    ws.open();
    // A non-envelope line is ignored; the envelope line lands.
    ws.receive("garbage not json");
    ws.receive(
      pushLine({
        version: "v0.9.1",
        updateDataModel: { surfaceId: "s1", path: "/label", value: "pushed" },
      }),
    );
    await screen.findByText("pushed");
  });

  it("keeps local input state across an unrelated push", async () => {
    MockWebSocket.reset();
    vi.stubGlobal("WebSocket", MockWebSocket);
    stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () => jsonResponse(boundInputSurface()),
      },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    const input = (await screen.findByRole("textbox")) as HTMLInputElement;
    expect(input.value).toBe("start");
    // The user types half a draft (local write-back into /draft).
    fireEvent.change(input, { target: { value: "start-typed" } });
    expect(input.value).toBe("start-typed");

    await waitFor(() => {
      expect(MockWebSocket.instances).toHaveLength(1);
    });
    const ws = MockWebSocket.instances[0];
    ws.open();
    // An unrelated push arrives (it touches /label, which this surface
    // never reads): it must not rebuild the surface — the input keeps its
    // half-typed value (a rebuild would re-render from /draft = "start").
    ws.receive(
      pushLine({
        version: "v0.9.1",
        updateDataModel: {
          surfaceId: "s1",
          path: "/label",
          value: "unrelated",
        },
      }),
    );
    await waitFor(() => {
      expect((screen.getByRole("textbox") as HTMLInputElement).value).toBe(
        "start-typed",
      );
    });
    // The surface is still live (a rebuild would have reset the value to
    // /draft = "start"): the input keeps accepting edits after the push.
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "start-typed-more" },
    });
    expect((screen.getByRole("textbox") as HTMLInputElement).value).toBe(
      "start-typed-more",
    );
  });

  it("re-pulls every declared query after a reconnect", async () => {
    MockWebSocket.reset();
    vi.stubGlobal("WebSocket", MockWebSocket);
    const queryUrl = `${ORIGIN}/api/testns/items`;
    const fetchMock = stubFetch([
      {
        url: `${ORIGIN}/comps/mycomp/surface.json`,
        respond: () =>
          jsonResponse({
            dataSpec: {
              queries: [{ pointer: "/items", request: { path: "items" } }],
            },
            messages: buttonSurface(),
          }),
      },
      { url: queryUrl, respond: () => jsonResponse([]) },
    ]);
    render(<SurfaceHost info={makeInfo()} />);
    await screen.findByText("Delete item");

    // 1: mount-time queries. 2: the first connect's re-pull (the live feed
    // carries no history, so the host re-materializes the snapshot).
    await waitFor(() => {
      expect(MockWebSocket.instances).toHaveLength(1);
    });
    MockWebSocket.instances[0].open();
    await waitFor(() => {
      expect(callsTo(fetchMock, queryUrl)).toHaveLength(2);
    });

    // The connection drops: observe reconnects (500ms first tier), and the
    // successful reconnect re-pulls again.
    MockWebSocket.instances[0].close();
    await waitFor(
      () => {
        expect(MockWebSocket.instances).toHaveLength(2);
      },
      { timeout: 3000 },
    );
    MockWebSocket.instances[1].open();
    await waitFor(() => {
      expect(callsTo(fetchMock, queryUrl)).toHaveLength(3);
    });
  });
});
