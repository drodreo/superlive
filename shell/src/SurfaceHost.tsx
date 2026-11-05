/**
 * SurfaceHost — the shell-side host for surface-tier components (manifest
 * ui_type "surface"): description-driven components that ship no JS of their
 * own. The shell fetches the description document from the component's
 * static space, executes its declarative dataSpec (queries at mount) through
 * the shell API channel, registers the HTTP action family for the renderer's
 * functionCall pipeline, and mounts the A2UI renderer on the parsed messages.
 *
 * Description document shapes (two accepted forms):
 * - a bare `ServerToClientMessage[]` (pure static surface, no dataSpec);
 * - `{ dataSpec?, messages }` — the envelope form carrying the declarative
 *   data flow next to the message list.
 *
 * dataSpec contract (deliberately minimal):
 * - queries: executed once at mount (in order); each result lands in the
 *   surface data model at the query's `pointer` via an appended
 *   updateDataModel message.
 * - actions are official functionCall vocabulary: the description declares
 *   `{functionCall: {call: "httpDelete", args: {path: …}}}` actions, the
 *   renderer evaluates the arguments (row-scoped bindings resolve against
 *   the row) and executes them against the function table registered here.
 *   The HTTP family fetches through the shell gateway with the component's
 *   namespace prefix (`/api/{ns}/{path}` — the gateway strips the prefix, so
 *   the path addresses the component's own path space); a 2xx response
 *   re-runs every query.
 *
 * Push channel (the ws subscription plane): once ready, the host subscribes
 * to `/subscriptions/{ns}/surface` and applies every arrived
 * ServerToClientMessage into the live SurfaceModel incrementally — the
 * description prop stays identity-stable for the whole ready phase, so node
 * bindings (and their local input state) survive every push. The feed is
 * live-only (no replay): every successful (re)connect re-runs every declared
 * query through the same incremental channel, which is also how a 2xx action
 * response refreshes the data.
 *
 * Errors are explicit, never silent: description fetch/parse/query failures
 * and HTTP failures (network, non-2xx, missing path) all surface the shell
 * error face (with retry) plus console.error, and a failed action never
 * triggers a refresh. The dataSpec is validated at load (a static file must
 * fail at load): a malformed spec surfaces the same error face before any
 * query runs. `{event}` actions stay on the official semantics but have no
 * shell-side consumer — the host mounts no onAction handler.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  A2uiSurface,
  createObserve,
  type FunctionTable,
  type ServerToClientMessage,
  type SurfaceModel,
} from "@shell/sdk/ui";
import {
  componentAssetUrl,
  SHELL_ORIGIN,
  shellApi,
  shellToken,
  type ComponentInfo,
} from "./api";

// ===== description document shapes =====

/** Literal query parameter values; null/undefined entries are not sent. */
type QueryParamValue = string | number | boolean | null;

interface DataSpecQuery {
  /** Data model pointer the result lands at (e.g. "/todos"). */
  pointer: string;
  request: {
    method?: string;
    /** Component-relative path (the gateway adds /api/{ns}/). */
    path: string;
    params?: Record<string, QueryParamValue>;
  };
}

interface DataSpec {
  queries?: DataSpecQuery[];
}

type DescriptionDocument =
  | ServerToClientMessage[]
  | { dataSpec?: DataSpec; messages: ServerToClientMessage[] };

/** One executed query, materialized as an updateDataModel envelope message. */
type DataMessage = Extract<ServerToClientMessage, { updateDataModel: unknown }>;

// ===== helpers =====

/** Serialize query params, dropping null/undefined entries. */
function buildQueryString(
  params: Record<string, QueryParamValue> | undefined,
): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params ?? {})) {
    if (value === null || value === undefined) continue;
    search.set(key, String(value));
  }
  const encoded = search.toString();
  return encoded ? `?${encoded}` : "";
}

function isEnvelope(
  value: unknown,
): value is { dataSpec?: DataSpec; messages: ServerToClientMessage[] } {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    Array.isArray((value as { messages?: unknown }).messages)
  );
}

/**
 * Load-time dataSpec validation. The types above describe the accepted
 * shape, but a description document arrives as untyped JSON (the cast in
 * the loader lies if the author shipped a malformed spec), so the runtime
 * truth is checked here — field-by-field — and the first violation is
 * returned as a readable message (null = valid).
 */
function validateDataSpec(dataSpec: DataSpec): string | null {
  if (dataSpec.queries !== undefined) {
    if (!Array.isArray(dataSpec.queries)) {
      return "queries must be an array";
    }
    for (const [index, query] of dataSpec.queries.entries()) {
      const at = `queries[${index}]`;
      if (typeof query !== "object" || query === null) {
        return `${at} must be an object`;
      }
      if (typeof query.pointer !== "string") {
        return `${at}.pointer must be a string`;
      }
      if (typeof query.request !== "object" || query.request === null) {
        return `${at}.request must be an object`;
      }
      if (
        query.request.method !== undefined &&
        typeof query.request.method !== "string"
      ) {
        return `${at}.request.method must be a string`;
      }
      if (typeof query.request.path !== "string") {
        return `${at}.request.path must be a string`;
      }
    }
  }
  return null;
}

// ===== component =====

type HostPhase =
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | {
      kind: "ready";
      /**
       * The description's own messages plus the initial query results, in
       * order. Stable identity: built once per ready phase — pushes and
       * query refreshes apply into the live SurfaceModel incrementally and
       * never touch this array.
       */
      messages: ServerToClientMessage[];
      dataSpec: DataSpec | undefined;
      /** Surface id from the description's createSurface (data message routing). */
      surfaceId: string;
    };

export function SurfaceHost({ info }: { info: ComponentInfo }) {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<HostPhase>({ kind: "loading" });
  const [reloadTick, setReloadTick] = useState(0);

  // The live surface handle (handed over through <A2uiSurface surfaceRef>):
  // push updates and query refreshes apply into it incrementally. Unmounts
  // and phase changes clear it through the ref callback's cleanup.
  const surfaceBox = useRef<SurfaceModel | null>(null);
  const takeSurface = useCallback((surface: SurfaceModel | null) => {
    surfaceBox.current = surface;
  }, []);
  const applyToSurface = useCallback((message: ServerToClientMessage) => {
    surfaceBox.current?.apply([message]);
  }, []);

  const retry = useCallback(() => {
    setPhase({ kind: "loading" });
    setReloadTick((tick) => tick + 1);
  }, []);

  // Mount: fetch the description document, parse, run the queries. Re-runs
  // on retry only (reloadTick) — a description is static for a component
  // version, and data refreshes flow through refreshAction instead.
  useEffect(() => {
    let cancelled = false;
    const run = async (): Promise<void> => {
      const fail = (message: string): void => {
        if (!cancelled) setPhase({ kind: "error", message });
      };
      let response: Response;
      try {
        response = await fetch(
          componentAssetUrl(info.id, info.ui?.entry ?? ""),
        );
      } catch (error) {
        fail(`fetching the description failed: ${String(error)}`);
        return;
      }
      if (!response.ok) {
        fail(
          `the description document is not reachable (${response.status} ${info.id}/${info.ui?.entry ?? ""})`,
        );
        return;
      }
      let parsed: DescriptionDocument;
      try {
        parsed = JSON.parse(await response.text()) as DescriptionDocument;
      } catch (error) {
        fail(`the description document is not valid JSON: ${String(error)}`);
        return;
      }
      const messages = Array.isArray(parsed)
        ? parsed
        : isEnvelope(parsed)
          ? parsed.messages
          : null;
      if (messages === null) {
        fail(
          "the description document is neither a message array nor a {dataSpec, messages} envelope",
        );
        return;
      }
      const surfaceId =
        messages.find(
          (
            message,
          ): message is Extract<
            ServerToClientMessage,
            { createSurface: unknown }
          > => "createSurface" in message,
        )?.createSurface.surfaceId ?? "";
      const dataSpec = Array.isArray(parsed) ? undefined : parsed.dataSpec;

      // The description is a static file: a malformed dataSpec fails the
      // load here, before any query or action runs.
      if (dataSpec !== undefined) {
        const violation = validateDataSpec(dataSpec);
        if (violation !== null) {
          console.error(`[SurfaceHost] invalid dataSpec: ${violation}`);
          fail(`invalid dataSpec: ${violation}`);
          return;
        }
      }

      // Mount-time queries run in declaration order; each result becomes an
      // updateDataModel message appended after the description's own — the
      // complete first frame, so the surface renders with data and the
      // array's identity never changes afterwards.
      const initial: DataMessage[] = [];
      for (const query of dataSpec?.queries ?? []) {
        const value = await runQuery(info.namespace, query.request);
        if (cancelled) return;
        if (value === undefined) {
          fail(`query for "${query.pointer}" failed (see the console)`);
          return;
        }
        initial.push({
          version: "v0.9.1",
          updateDataModel: { surfaceId, path: query.pointer, value },
        });
      }
      if (cancelled) return;
      setPhase({
        kind: "ready",
        messages: [...messages, ...initial],
        dataSpec,
        surfaceId,
      });
    };
    void run();
    return () => {
      cancelled = true;
    };
    // The component identity (id + entry) drives the document; retry
    // re-enters through reloadTick.
  }, [info.id, info.ui, reloadTick]);

  /** Re-run one query (params possibly overridden) and land the result. */
  const refreshQuery = useCallback(
    async (
      surfaceId: string,
      query: DataSpecQuery,
      paramOverrides: Record<string, QueryParamValue>,
    ): Promise<void> => {
      const value = await runQuery(info.namespace, {
        ...query.request,
        params: { ...query.request.params, ...paramOverrides },
      });
      if (value === undefined) {
        setPhase({
          kind: "error",
          message: `refreshing "${query.pointer}" failed (see the console)`,
        });
        return;
      }
      // Incremental landing: the result applies into the live surface
      // (pointer-scoped data model write), never through the description.
      applyToSurface({
        version: "v0.9.1",
        updateDataModel: { surfaceId, path: query.pointer, value },
      });
    },
    [info.namespace, applyToSurface],
  );

  /**
   * The preset HTTP action family (catalog functions httpGet/httpPost/
   * httpPut/httpPatch/httpDelete), executed through the renderer's
   * functionCall pipeline: fetch the component's own API space via the shell
   * gateway, and on a 2xx response re-run every dataSpec query through the
   * existing refresh channel. Failures (network, non-2xx, missing path)
   * surface the error face and never refresh.
   */
  const httpFunctions = useCallback(
    (state: Extract<HostPhase, { kind: "ready" }>): FunctionTable => {
      const runHttp =
        (method: string) =>
        async (args: Record<string, unknown>): Promise<void> => {
          const path = args.path;
          if (typeof path !== "string" || path.length === 0) {
            const message = `${method} action: the "path" argument must evaluate to a non-empty string`;
            console.error(`[SurfaceHost] ${message}`);
            setPhase({ kind: "error", message });
            return;
          }
          const search = new URLSearchParams();
          for (const [key, value] of Object.entries(
            (args.query ?? {}) as Record<string, unknown>,
          )) {
            if (value === undefined || value === null) continue;
            search.set(key, String(value));
          }
          const queryString = search.toString();
          const init: RequestInit = { method };
          if (args.body !== undefined) {
            init.headers = { "Content-Type": "application/json" };
            init.body = JSON.stringify(args.body);
          }
          let response: Response;
          try {
            response = await shellApi(
              `${info.namespace}/${path.replace(/^\/+/, "")}${queryString ? `?${queryString}` : ""}`,
              init,
            );
          } catch (error) {
            console.error(
              `[SurfaceHost] ${method} ${path} request failed:`,
              error,
            );
            setPhase({
              kind: "error",
              message: `${method} ${path} request failed: ${String(error)}`,
            });
            return;
          }
          if (!response.ok) {
            console.error(
              `[SurfaceHost] ${method} ${path} answered ${response.status}`,
            );
            setPhase({
              kind: "error",
              message: `${method} ${path} failed: HTTP ${response.status}`,
            });
            return;
          }
          // 2xx: re-run every declared query (the refresh channel).
          for (const query of state.dataSpec?.queries ?? []) {
            await refreshQuery(state.surfaceId, query, {});
          }
        };
      return {
        httpGet: runHttp("GET"),
        httpPost: runHttp("POST"),
        httpPut: runHttp("PUT"),
        httpPatch: runHttp("PATCH"),
        httpDelete: runHttp("DELETE"),
      };
    },
    [info.namespace, refreshQuery],
  );

  // Stable per-ready-phase function table: the memo keeps the reference
  // steady across data refreshes, so only genuine description reloads
  // rebuild the surface through the functions identity.
  const functions = useMemo(
    () => (phase.kind === "ready" ? httpFunctions(phase) : undefined),
    [phase, httpFunctions],
  );

  // Push subscription (the ws plane): every ServerToClientMessage the
  // component pushes arrives through /subscriptions/{ns}/surface and applies
  // into the live surface incrementally. The ws layer reconnects with
  // backoff on its own; every successful (re)connect re-runs every declared
  // query through the same incremental channel — the feed is live-only (no
  // replay), so re-materializing the snapshot after a reconnect is the
  // host's responsibility (the push-channel contract).
  useEffect(() => {
    if (phase.kind !== "ready") return;
    let disposed = false;
    let subscription: { unsubscribe(): void } | null = null;
    const surfaceId = phase.surfaceId;
    const queries = phase.dataSpec?.queries ?? [];
    const repull = async (): Promise<void> => {
      for (const query of queries) {
        const value = await runQuery(info.namespace, query.request);
        if (disposed) return;
        if (value === undefined) continue; // already logged by runQuery
        applyToSurface({
          version: "v0.9.1",
          updateDataModel: { surfaceId, path: query.pointer, value },
        });
      }
    };
    void (async () => {
      try {
        const token = await shellToken();
        if (disposed) return;
        const observe = createObserve(SHELL_ORIGIN, token);
        subscription = observe<ServerToClientMessage>(
          `/subscriptions/${info.namespace}/surface`,
          {
            wsFactory: (url) => {
              const ws = new WebSocket(url);
              // Every successful connect (first or reconnect) re-pulls the
              // declared queries: live-feed pushes carry no history.
              ws.addEventListener("open", () => {
                void repull();
              });
              return ws;
            },
          },
        ).subscribe({
          next: (message) => {
            if (
              typeof message === "object" &&
              message !== null &&
              "version" in message
            ) {
              applyToSurface(message as ServerToClientMessage);
            }
          },
        });
      } catch (error) {
        console.error("[SurfaceHost] subscription setup failed:", error);
      }
    })();
    return () => {
      disposed = true;
      subscription?.unsubscribe();
    };
  }, [phase, info.namespace, applyToSurface]);

  if (phase.kind === "loading") {
    return <div className="comp-loading">{t("host.loading")}</div>;
  }
  if (phase.kind === "error") {
    return (
      <div className="comp-error">
        {t("host.surfaceLoadFailed", { error: phase.message })}
        <button type="button" onClick={retry}>
          {t("host.retry")}
        </button>
      </div>
    );
  }
  return (
    <A2uiSurface
      description={phase.messages}
      functions={functions}
      surfaceRef={takeSurface}
      onError={(payload) =>
        console.error("[SurfaceHost] renderer error:", payload)
      }
    />
  );
}

/** One spec query through the shell gateway (/api/{ns}/{path}). */
async function runQuery(
  namespace: string,
  request: DataSpecQuery["request"],
): Promise<unknown> {
  const method = request.method ?? "GET";
  const path = `${namespace}/${request.path.replace(/^\/+/, "")}${buildQueryString(request.params)}`;
  try {
    const response = await shellApi(path, { method });
    if (!response.ok) {
      console.error(
        `[SurfaceHost] query ${method} ${path} answered ${response.status}`,
      );
      return undefined;
    }
    return await response.json();
  } catch (error) {
    console.error(`[SurfaceHost] query ${method} ${path} failed:`, error);
    return undefined;
  }
}
