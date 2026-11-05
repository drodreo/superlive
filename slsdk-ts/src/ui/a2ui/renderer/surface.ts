/**
 * SurfaceModel — the engine side of the A2UI renderer (single-surface subset
 * of the official web_core SurfaceModel + MessageProcessor + NodeResolver):
 *
 * - Flat component table fed by `apply` (ServerToClientMessage envelopes).
 *   updateComponents goes through two passes: validate (batch-unique ids,
 *   vocabulary membership, prop shape) then mutate (upsert, keeping the old
 *   entry when nothing changed so React memo comparisons stay reference-
 *   stable).
 * - Data model: the reactive JSON Pointer tree (see data-model.ts).
 * - Tree resolution: the flat table resolves to a tree on demand; a missing
 *   child anywhere below "root" keeps the tree pending (progressive
 *   materialization — parents arriving before children render nothing but
 *   the loading placeholder, never a broken screen).
 * - Action pipeline: an {event} action evaluates its context and leaves the
 *   client as an ActionPayload to the host; a {functionCall} action executes
 *   against the local function table. Errors flow to the onError channel.
 */
import {
  ComponentBinding,
  inspectComponentSchema,
  isDataBinding,
  isFunctionCall,
  resolveDynamic,
  resolveScopePath,
  type ComponentSpec,
  type EvalFrame,
  type FunctionTable,
} from "./binding";
import { createCatalogValidator } from "../../catalog/validate";
import { DataModel, DataModelError } from "./data-model";
import type { Unsubscribe } from "./signals";
import type {
  ActionMessagePayload,
  A2uiComponentInstance,
  DynamicValue,
  ErrorMessagePayload,
  ServerToClientMessage,
} from "../types";

/** Shape of the authoritative catalog document (superlive.catalog.json). */
export interface CatalogDocument {
  catalogId: string;
  components: Record<string, unknown>;
  /** Local `$defs` of the catalog document — the `$ref` resolution source. */
  readonly $defs?: Record<string, unknown>;
  /**
   * Declared function vocabulary (the top-level `functions` segment): the
   * schema every `{functionCall}` action / `{call}` binding must match at
   * load time (official FunctionCall oneOf: catalog anyFunction).
   */
  readonly functions?: Record<string, unknown>;
}

/** A resolved tree node (flat table entry + resolved child links). */
export interface SurfaceNode {
  componentId: string;
  /**
   * Data path when the node originates from template iteration. Reserved for
   * the B2-2 ChildList-iteration batch; undefined for direct instances.
   */
  dataPath?: string;
  component: string;
  /** Raw props (id/component stripped); bindings evaluated per snapshot. */
  propsSource: Record<string, unknown>;
  child?: SurfaceNode;
  childrenMap?: Record<string, SurfaceNode[]>;
}

export interface SurfaceOptions {
  catalog: CatalogDocument;
  onAction?: (payload: ActionMessagePayload) => void;
  onError?: (payload: ErrorMessagePayload) => void;
  functions?: FunctionTable;
}

interface TableEntry {
  component: string;
  propsSource: Record<string, unknown>;
}

/**
 * Renderer-builtin function implementations — the pure evaluation vocabulary
 * (predicates) that needs no host: every surface gets them, and a host-provided
 * function of the same name overrides the builtin. HTTP-flavored functions are
 * host vocabulary (registered by the embedding SurfaceHost), not builtins.
 */
const BUILTIN_FUNCTIONS: FunctionTable = {
  not: (args) => !args.value,
  eq: (args) => args.left === args.right,
};

export class SurfaceModel {
  readonly dataModel = new DataModel();
  surfaceId: string | undefined;
  catalogId: string | undefined;

  private readonly catalog: CatalogDocument;
  private readonly onAction?: SurfaceOptions["onAction"];
  private readonly onError?: SurfaceOptions["onError"];
  private readonly functions: FunctionTable;
  /** The catalog's declared function vocabulary (`functions` segment). */
  private readonly functionVocabulary: Record<string, unknown>;
  private functionsValidator: ReturnType<typeof createCatalogValidator> | null =
    null;
  private readonly specCache = new Map<string, ComponentSpec>();
  private readonly table = new Map<string, TableEntry>();
  private readonly treeListeners = new Set<() => void>();
  /**
   * Data model paths the current tree iterates over (ChildList templates).
   * Re-subscribed after every rebuild: only array *length* changes rebuild the
   * tree (element content flows through per-node binding subscriptions).
   */
  private readonly iterationDeps = new Set<string>();
  private readonly iterationLengths = new Map<string, number>();
  private iterationUnsubs: Unsubscribe[] = [];
  private tree: SurfaceNode | null = null;
  private rev = 0;
  private disposed = false;

  constructor(options: SurfaceOptions) {
    this.catalog = options.catalog;
    this.onAction = options.onAction;
    this.onError = options.onError;
    this.functions = { ...BUILTIN_FUNCTIONS, ...(options.functions ?? {}) };
    this.functionVocabulary = options.catalog.functions ?? {};
  }

  // ===== message application =====

  /** Apply a description (JSON string or parsed message list), in order. */
  apply(input: string | ServerToClientMessage[]): void {
    if (this.disposed) return;
    let messages: ServerToClientMessage[];
    if (typeof input === "string") {
      try {
        messages = JSON.parse(input) as ServerToClientMessage[];
      } catch (error) {
        this.report(
          "PARSE_FAILED",
          `description is not valid JSON: ${String(error)}`,
        );
        return;
      }
    } else {
      messages = input;
    }
    for (const message of messages) {
      this.applyMessage(message);
    }
  }

  private applyMessage(message: ServerToClientMessage): void {
    if ("createSurface" in message) {
      const op = message.createSurface;
      if (op.catalogId !== this.catalog.catalogId) {
        this.report(
          "CATALOG_MISMATCH",
          `surface "${op.surfaceId}" declares catalog "${op.catalogId}", renderer hosts "${this.catalog.catalogId}"`,
        );
        return;
      }
      this.surfaceId = op.surfaceId;
      this.catalogId = op.catalogId;
      this.bump();
    } else if ("updateComponents" in message) {
      this.applyComponents(message.updateComponents.components);
    } else if ("updateDataModel" in message) {
      const op = message.updateDataModel;
      // The spec defines "/" and omission alike as the entire data model
      // (server_to_client.json: "If omitted, or set to '/', refers to the
      // entire data model") — distinct from RFC 6901, where "/" addresses
      // the single key "". Normalize to "" (the whole-document pointer).
      const pointer = !op.path || op.path === "/" ? "" : op.path;
      try {
        if ("value" in op) {
          this.dataModel.set(pointer, op.value);
        } else {
          this.dataModel.delete(pointer);
        }
      } catch (error) {
        this.report(
          "INVALID_DATA_MODEL_OPERATION",
          error instanceof DataModelError
            ? `${error.message} (at "${error.pointer}")`
            : String(error),
        );
      }
    } else if ("deleteSurface" in message) {
      this.table.clear();
      this.tree = null;
      // The whole-document pointer is "" — "/" would address the single
      // empty-segment key (a no-op) and leak stale data across surface
      // lifecycles. Mirrors the updateDataModel normalization above.
      this.dataModel.delete("");
      this.bump();
    }
  }

  // ===== updateComponents: validate pass + mutate pass =====

  private applyComponents(components: A2uiComponentInstance[]): void {
    // ---- validate pass (any failure rejects the whole batch) ----
    // Official MUST: every batch declares the "root" component — without it
    // nothing is renderable, so the batch is rejected outright.
    if (!components.some((instance) => instance.id === "root")) {
      this.report(
        "VALIDATION_FAILED",
        'updateComponents batch is missing the required "root" component',
      );
      return;
    }
    const seen = new Set<string>();
    for (const instance of components) {
      if (seen.has(instance.id)) {
        this.report(
          "VALIDATION_FAILED",
          `duplicate component id "${instance.id}" in updateComponents batch`,
        );
        return;
      }
      seen.add(instance.id);

      if (!this.catalog.components[instance.component]) {
        this.report(
          "VALIDATION_FAILED",
          `component "${instance.component}" (id "${instance.id}") is not in catalog "${this.catalog.catalogId}"`,
        );
        return;
      }

      const spec = this.specOf(instance.component);
      const { id: _id, component: _component, ...propsSource } = instance;

      for (const name of spec.required) {
        // "component" is required by every catalog schema and consumed by the
        // strip above; "id" is the protocol-level key, not a prop.
        if (name === "component" || name === "id") continue;
        if (!(name in propsSource)) {
          this.report(
            "VALIDATION_FAILED",
            `component "${instance.component}" (id "${instance.id}") misses required prop "${name}"`,
          );
          return;
        }
      }
      for (const name of Object.keys(propsSource)) {
        const propSpec = spec.props[name];
        if (!propSpec) {
          this.report(
            "VALIDATION_FAILED",
            `component "${instance.component}" (id "${instance.id}") has undeclared prop "${name}"`,
          );
          return;
        }
        const problem = checkPropShape(name, propSpec, propsSource[name]);
        if (problem) {
          this.report(
            "VALIDATION_FAILED",
            `component "${instance.component}" (id "${instance.id}") prop "${name}": ${problem}`,
          );
          return;
        }
      }
      // FunctionCall vocabulary gate (official FunctionCall oneOf:
      // catalog.json#/$defs/anyFunction): every declared call must be in the
      // catalog functions segment and match its declaration schema. Load-time
      // and batch-rejecting, like the rest of the validate pass.
      for (const problem of this.checkFunctionDeclarations(propsSource)) {
        this.report(
          "VALIDATION_FAILED",
          `component "${instance.component}" (id "${instance.id}"): ${problem}`,
        );
        return;
      }
    }

    // ---- mutate pass (reference-stable upsert) ----
    for (const instance of components) {
      const { id: _id, component, ...propsSource } = instance;
      const existing = this.table.get(instance.id);
      if (
        existing &&
        existing.component === component &&
        deepEqual(existing.propsSource, propsSource)
      ) {
        continue; // keep the old entry: stable references for React memo
      }
      this.table.set(instance.id, { component, propsSource });
    }
    this.rebuildTree();
    this.bump();
  }

  // ===== tree resolution (progressive materialization) =====

  private rebuildTree(): void {
    this.iterationDeps.clear();
    this.tree = this.buildNode("root", new Set());
    this.resubscribeIterations();
  }

  /**
   * Track the iterated arrays behind the current tree: a length change means
   * instances must appear/disappear, so the tree rebuilds and React is bumped.
   * Content-only changes inside elements are deliberately ignored here — those
   * flow through the per-node binding subscriptions.
   */
  private resubscribeIterations(): void {
    for (const unsub of this.iterationUnsubs) unsub();
    this.iterationUnsubs = [];
    this.iterationLengths.clear();
    for (const pointer of this.iterationDeps) {
      const list = this.dataModel.get(pointer);
      this.iterationLengths.set(
        pointer,
        Array.isArray(list) ? list.length : -1,
      );
      this.iterationUnsubs.push(
        this.dataModel.subscribe(pointer, () => {
          if (this.disposed) return;
          const current = this.dataModel.get(pointer);
          const length = Array.isArray(current) ? current.length : -1;
          if (length === this.iterationLengths.get(pointer)) return;
          this.iterationLengths.set(pointer, length);
          this.rebuildTree();
          this.bump();
        }),
      );
    }
  }

  /**
   * Resolve one node; a missing component id anywhere below yields null so
   * the whole branch (up to root) stays pending until the definition arrives.
   * `scopePath` is the iterated element path when resolving inside a
   * template-iteration subtree (undefined = root scope).
   */
  private buildNode(
    componentId: string,
    visiting: Set<string>,
    scopePath?: string,
  ): SurfaceNode | null {
    const entry = this.table.get(componentId);
    if (!entry) return null; // not an error: definition pending
    if (visiting.has(componentId)) {
      this.report(
        "CYCLIC_REFERENCE",
        `component reference cycle at "${componentId}"`,
      );
      return null;
    }
    visiting.add(componentId);
    try {
      const spec = this.specOf(entry.component);
      const node: SurfaceNode = {
        componentId,
        component: entry.component,
        propsSource: entry.propsSource,
      };
      if (scopePath !== undefined) node.dataPath = scopePath;

      const childSpec = Object.entries(spec.props).find(
        ([, s]) => s.kind === "childRef",
      );
      if (childSpec) {
        const [childProp] = childSpec;
        const childId = entry.propsSource[childProp];
        const optionalMissing =
          childId === undefined && !spec.required.includes(childProp);
        if (!optionalMissing) {
          if (typeof childId !== "string") return null; // pending
          const child = this.buildNode(childId, visiting, scopePath);
          if (!child) return null;
          node.child = child;
        }
        // An optional child slot left undeclared simply stays empty — the
        // same semantics childList slots get for a missing declaration.
      }

      const listSlots = Object.entries(spec.props).filter(
        ([, s]) => s.kind === "childList",
      );
      if (listSlots.length > 0) {
        const childrenMap: Record<string, SurfaceNode[]> = {};
        for (const [slot] of listSlots) {
          const decl: unknown = entry.propsSource[slot];
          if (decl === undefined) continue; // optional slot left undeclared
          if (Array.isArray(decl)) {
            const children: SurfaceNode[] = [];
            for (const childId of decl) {
              if (typeof childId !== "string") return null;
              const child = this.buildNode(childId, visiting, scopePath);
              if (!child) return null;
              children.push(child);
            }
            childrenMap[slot] = children;
          } else if (
            decl &&
            typeof decl === "object" &&
            "componentId" in (decl as object)
          ) {
            const children = this.materializeIteration(
              componentId,
              slot,
              decl as { componentId: unknown; path: unknown },
              visiting,
              scopePath,
            );
            if (!children) return null;
            childrenMap[slot] = children;
          } else {
            return null; // pending / malformed
          }
        }
        node.childrenMap = childrenMap;
      }

      return node;
    } finally {
      visiting.delete(componentId);
    }
  }

  /**
   * ChildList template iteration (protocol.md "Collection scopes"): resolve
   * the bound array against the current scope and instantiate the template
   * component once per element — every instance gets the element path burned
   * in as its evaluation scope (`dataPath`), so relative bindings inside the
   * template resolve against that element. Returns null while the list data
   * is pending; a non-array value is a validation error.
   */
  private materializeIteration(
    ownerId: string,
    slot: string,
    decl: { componentId: unknown; path: unknown },
    visiting: Set<string>,
    scopePath: string | undefined,
  ): SurfaceNode[] | null {
    const { componentId: templateId, path } = decl;
    if (typeof templateId !== "string" || typeof path !== "string") {
      return null; // malformed shape is rejected in the validate pass
    }
    const pointer = resolveScopePath(path, scopePath);
    if (pointer === null) {
      this.report(
        "VALIDATION_FAILED",
        `component "${ownerId}" slot "${slot}": relative iteration path "${path}" used outside a template iteration scope`,
      );
      return null;
    }
    // Register before the data check: the declaration itself is a dependency —
    // when the array streams in later (updateDataModel after updateComponents),
    // the subscription is what revives the pending tree.
    this.iterationDeps.add(pointer);
    const list = this.dataModel.get(pointer);
    if (list === undefined) return null; // progressive: data not streamed yet
    if (!Array.isArray(list)) {
      this.report(
        "VALIDATION_FAILED",
        `component "${ownerId}" slot "${slot}": iteration path "${pointer}" holds ${typeof list}, expected an array`,
      );
      return null;
    }
    const children: SurfaceNode[] = [];
    for (let i = 0; i < list.length; i++) {
      const child = this.buildNode(
        templateId,
        visiting,
        joinPointer(pointer, i),
      );
      if (!child) return null;
      children.push(child);
    }
    return children;
  }

  // ===== React-facing snapshot store =====

  /** Resolved tree; null while the root subtree is pending. */
  getTree(): SurfaceNode | null {
    return this.tree;
  }

  get version(): number {
    return this.rev;
  }

  getVersion = (): number => this.rev;

  /** Structural change notification (tree/table/surface lifecycle). */
  subscribe = (cb: () => void): Unsubscribe => {
    this.treeListeners.add(cb);
    return () => {
      this.treeListeners.delete(cb);
    };
  };

  /** Create the evaluated-props binding store for a rendered node. */
  createNodeBinding(node: SurfaceNode): ComponentBinding {
    return new ComponentBinding(
      {
        componentSpec: this.specOf(node.component),
        propsSource: node.propsSource,
        surfaceId: this.surfaceId ?? "",
        componentId: node.componentId,
        dataPath: node.dataPath,
      },
      this.dataModel,
      this.functions,
      (message) => this.report("BINDING_EVALUATION_FAILED", message),
    );
  }

  // ===== interaction pipeline =====

  /**
   * Two-way binding write-back for a controlled prop: only {"path"}-bound
   * slots accept local writes (keystrokes stay client-side); returns whether
   * a write happened.
   */
  writeBack(node: SurfaceNode, propName: string, value: unknown): boolean {
    const decl = node.propsSource[propName];
    if (!isDataBinding(decl)) return false;
    const pointer = resolveScopePath(decl.path, node.dataPath);
    if (pointer === null) {
      this.report(
        "BINDING_EVALUATION_FAILED",
        `relative path "${decl.path}" written back outside a template iteration scope on "${node.componentId}"`,
      );
      return false;
    }
    try {
      this.dataModel.set(pointer, value);
      return true;
    } catch (error) {
      this.report(
        "INVALID_DATA_MODEL_OPERATION",
        error instanceof DataModelError
          ? `${error.message} (at "${error.pointer}")`
          : String(error),
      );
      return false;
    }
  }

  /**
   * Dispatch the Action declared at `actionPropName` on this node.
   * `extraContext` carries interaction products the description cannot
   * declare statically (the selected page, the assembled filter, form
   * values — catalog descriptions promise these "returned in the event
   * context"); merged over the evaluated declaration context.
   */
  dispatchAction(
    node: SurfaceNode,
    actionPropName: string,
    extraContext?: Record<string, unknown>,
  ): void {
    const decl: unknown = node.propsSource[actionPropName];
    if (!decl || typeof decl !== "object" || Array.isArray(decl)) return;
    this.emitAction(decl, node.dataPath, node.componentId, extraContext);
  }

  /**
   * Row-scoped action dispatch for data-driven row actions (DataTable
   * rowActions): the Action declaration lives inside a prop value (an array
   * element), not a dedicated prop slot, so dispatch takes the declaration
   * itself plus the row index. The evaluation scope is the bound row element
   * — the same relative-path rule template iteration burns into
   * node.dataPath (materializeIteration), applied to the rows array element.
   */
  dispatchRowAction(
    node: SurfaceNode,
    rowsPropName: string,
    actionDecl: unknown,
    rowIndex: number,
    extraContext?: Record<string, unknown>,
  ): void {
    const scope = this.rowScopeOf(node, rowsPropName, rowIndex);
    if (scope === null) return;
    this.emitAction(actionDecl, scope, node.componentId, extraContext);
  }

  /**
   * Row-scoped dynamic evaluation (row-action labels): relative {"path"}
   * pointers resolve against the bound row element, literals pass through.
   * Errors surface through the error channel with an undefined value.
   */
  resolveRowValue(
    node: SurfaceNode,
    rowsPropName: string,
    decl: unknown,
    rowIndex: number,
  ): unknown {
    const scope = this.rowScopeOf(node, rowsPropName, rowIndex);
    if (scope === null) return undefined;
    const frame: EvalFrame = {};
    const value = resolveDynamic(
      decl,
      "string",
      this.dataModel,
      this.functions,
      frame,
      scope,
    );
    if (frame.error) {
      this.report(
        "BINDING_EVALUATION_FAILED",
        `row value on "${node.componentId}": ${frame.error}`,
      );
    }
    return value;
  }

  /**
   * The absolute data model pointer of one bound row element: the rows prop
   * must be a {"path"} binding; a relative rows path resolves against the
   * node's own template-iteration scope (mirroring writeBack).
   */
  private rowScopeOf(
    node: SurfaceNode,
    rowsPropName: string,
    rowIndex: number,
  ): string | null {
    const decl: unknown = node.propsSource[rowsPropName];
    if (!isDataBinding(decl)) {
      this.report(
        "VALIDATION_FAILED",
        `component "${node.componentId}": row actions require the "${rowsPropName}" prop bound as a {"path"} data binding`,
      );
      return null;
    }
    const pointer = resolveScopePath(decl.path, node.dataPath);
    if (pointer === null) {
      this.report(
        "BINDING_EVALUATION_FAILED",
        `relative path "${decl.path}" on "${node.componentId}" used outside a template iteration scope`,
      );
      return null;
    }
    return joinPointer(pointer, rowIndex);
  }

  /**
   * Shared action pipeline behind dispatchAction / dispatchRowAction:
   * validate the {event}/{functionCall} declaration, evaluate the event
   * context against `scopePath`, merge `extraContext` over it and hand the
   * payload to the host (or execute the local function call). Errors flow to
   * the error channel.
   */
  private emitAction(
    actionDecl: unknown,
    scopePath: string | undefined,
    sourceComponentId: string,
    extraContext?: Record<string, unknown>,
  ): void {
    if (
      !actionDecl ||
      typeof actionDecl !== "object" ||
      Array.isArray(actionDecl)
    ) {
      this.report(
        "VALIDATION_FAILED",
        `action declaration on "${sourceComponentId}" is not an object`,
      );
      return;
    }
    if ("event" in actionDecl) {
      const event = (
        actionDecl as { event?: { name?: unknown; context?: unknown } }
      ).event;
      if (
        !event ||
        typeof event !== "object" ||
        typeof event.name !== "string"
      ) {
        this.report(
          "VALIDATION_FAILED",
          `action declaration has a malformed event`,
        );
        return;
      }
      const context: Record<string, unknown> = {};
      for (const [key, raw] of Object.entries(
        (event.context as Record<string, unknown> | undefined) ?? {},
      )) {
        const frame: EvalFrame = {};
        context[key] = resolveDynamic(
          raw,
          "value",
          this.dataModel,
          this.functions,
          frame,
          scopePath,
        );
        if (frame.error) {
          this.report(
            "BINDING_EVALUATION_FAILED",
            `action context "${key}" on "${sourceComponentId}": ${frame.error}`,
          );
        }
      }
      Object.assign(context, extraContext ?? {});
      this.onAction?.({
        name: event.name,
        surfaceId: this.surfaceId ?? "",
        sourceComponentId,
        timestamp: new Date().toISOString(),
        // Evaluated values are wire-legal literals of DynamicValue.
        context: context as Record<string, DynamicValue>,
      });
      return;
    }

    if ("functionCall" in actionDecl) {
      void this.executeFunctionCall(
        (actionDecl as { functionCall: unknown }).functionCall,
        scopePath,
        sourceComponentId,
      );
    }
  }

  /**
   * Execute a `{functionCall}` action declaration: evaluate every argument
   * against the scope, look the implementation up in the function table and
   * run it to completion — async implementations (returning a Promise) are
   * awaited. Unknown functions, evaluation failures and execution failures
   * flow to the error channel; the pipeline never throws into the caller.
   */
  private async executeFunctionCall(
    callDecl: unknown,
    scopePath: string | undefined,
    sourceComponentId: string,
  ): Promise<void> {
    if (
      !callDecl ||
      typeof callDecl !== "object" ||
      Array.isArray(callDecl) ||
      typeof (callDecl as { call?: unknown }).call !== "string"
    ) {
      this.report(
        "VALIDATION_FAILED",
        `action declaration on "${sourceComponentId}" has a malformed functionCall`,
      );
      return;
    }
    const call = (callDecl as { call: string }).call;
    const fn = this.functions[call];
    if (!fn) {
      this.report(
        "FUNCTION_NOT_FOUND",
        `function "${call}" is declared in the catalog but has no registered implementation`,
      );
      return;
    }
    const args: Record<string, unknown> = {};
    for (const [key, raw] of Object.entries(
      (callDecl as { args?: Record<string, unknown> }).args ?? {},
    )) {
      const frame: EvalFrame = {};
      args[key] = this.resolveActionValue(raw, scopePath, frame);
      if (frame.error) {
        this.report(
          "BINDING_EVALUATION_FAILED",
          `action functionCall "${call}" argument "${key}" on "${sourceComponentId}": ${frame.error}`,
        );
        return;
      }
    }
    try {
      await fn(args);
    } catch (error) {
      this.report(
        "FUNCTION_EXECUTION_FAILED",
        `function "${call}" failed: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }

  /**
   * One action-argument value: DynamicValue semantics through resolveDynamic,
   * plus entry-wise evaluation of literal object arguments. Official
   * FunctionCall args allow `DynamicValue | literal object`; in the action
   * pipeline the entries of a literal object are themselves DynamicValues, so
   * a `body` entry declared as `{"call": "not", …}` ships the computed
   * boolean, not the declaration.
   */
  private resolveActionValue(
    raw: unknown,
    scopePath: string | undefined,
    frame: EvalFrame,
  ): unknown {
    if (
      raw !== null &&
      typeof raw === "object" &&
      !Array.isArray(raw) &&
      !isDataBinding(raw) &&
      !isFunctionCall(raw)
    ) {
      const out: Record<string, unknown> = {};
      for (const [key, value] of Object.entries(raw)) {
        out[key] = this.resolveActionValue(value, scopePath, frame);
        if (frame.error) return undefined;
      }
      return out;
    }
    const inner: EvalFrame = {};
    const value = resolveDynamic(
      raw,
      "value",
      this.dataModel,
      this.functions,
      inner,
      scopePath,
    );
    if (inner.error) frame.error = inner.error;
    return value;
  }

  /**
   * Load-time FunctionCall declaration checks for one component's props:
   * walk the value tree collecting `{functionCall: …}` wrappers and nested
   * `{call: …}` declarations (DynamicValue functionCall-strings inside args),
   * and check each against the catalog functions vocabulary.
   */
  private checkFunctionDeclarations(value: unknown): string[] {
    const problems: string[] = [];
    const walk = (node: unknown): void => {
      if (Array.isArray(node)) {
        for (const item of node) walk(item);
        return;
      }
      if (!node || typeof node !== "object") return;
      if ("functionCall" in node) {
        const call = (node as { functionCall?: unknown }).functionCall;
        problems.push(...this.checkOneFunctionDeclaration(call));
        // Nested DynamicValue declarations live in the args (e.g. a body
        // entry computed by a predicate); the call itself is checked above.
        if (call && typeof call === "object" && !Array.isArray(call)) {
          walk((call as { args?: unknown }).args);
        }
        return;
      }
      if (isFunctionCall(node)) {
        problems.push(...this.checkOneFunctionDeclaration(node));
        walk((node as { args?: unknown }).args);
        return;
      }
      for (const entry of Object.values(node as Record<string, unknown>)) {
        walk(entry);
      }
    };
    walk(value);
    return problems;
  }

  /** Check one `{call, args?, returnType?}` declaration against the catalog. */
  private checkOneFunctionDeclaration(decl: unknown): string[] {
    if (!decl || typeof decl !== "object" || Array.isArray(decl)) {
      return ["malformed functionCall declaration"];
    }
    const call = (decl as { call?: unknown }).call;
    if (typeof call !== "string") {
      return ['functionCall declaration is missing the "call" name'];
    }
    const schema = this.functionVocabulary[call];
    if (!schema) {
      return [
        `function "${call}" is not declared in the catalog functions segment`,
      ];
    }
    return this.functionsValidatorInstance()
      .validate(schema, decl, `functions.${call}`)
      .map(
        (error) =>
          `function call "${call}" does not match its declaration: ${error}`,
      );
  }

  private functionsValidatorInstance(): ReturnType<
    typeof createCatalogValidator
  > {
    if (!this.functionsValidator) {
      this.functionsValidator = createCatalogValidator({
        document: this.catalog as unknown as Record<string, unknown>,
      });
    }
    return this.functionsValidator;
  }

  /** Surface the data model / error channel to host-side diagnostics. */
  report(code: string, message: string): void {
    this.onError?.({ code, surfaceId: this.surfaceId, message });
  }

  /** Drop all subscriptions; the instance must not be used afterwards. */
  dispose(): void {
    this.disposed = true;
    this.treeListeners.clear();
    for (const unsub of this.iterationUnsubs) unsub();
    this.iterationUnsubs = [];
    this.dataModel.clearSubscriptions();
  }

  private specOf(component: string): ComponentSpec {
    let spec = this.specCache.get(component);
    if (!spec) {
      spec = inspectComponentSchema(
        this.catalog.components[component],
        this.catalog.$defs,
      );
      this.specCache.set(component, spec);
    }
    return spec;
  }

  private bump(): void {
    this.rev++;
    for (const cb of [...this.treeListeners]) cb();
  }
}

// ===== prop shape checks (validate pass helpers) =====

/** Append an array index to a pointer (numeric segments never need escaping). */
function joinPointer(pointer: string, index: number): string {
  return `${pointer}/${index}`;
}

function checkPropShape(
  name: string,
  spec: {
    kind: string;
    dynamicType?: string;
    literalType?: string;
    enumValues?: readonly unknown[];
  },
  value: unknown,
): string | null {
  switch (spec.kind) {
    case "dynamic": {
      // A {path, call} object satisfies neither binding shape and would fall
      // through to the literal branch unreported — reject it at validation
      // with the offending object in the message.
      if (
        value &&
        typeof value === "object" &&
        !Array.isArray(value) &&
        "path" in value &&
        "call" in value
      ) {
        return `binding declares both "path" and "call": ${JSON.stringify(value)}`;
      }
      if (isDataBinding(value) || isFunctionCallShaped(value)) return null;
      const expected = spec.dynamicType;
      if (expected === "string")
        return typeof value === "string"
          ? null
          : "expected string literal or {path}/{call} binding";
      if (expected === "number")
        return typeof value === "number"
          ? null
          : "expected number literal or {path}/{call} binding";
      if (expected === "boolean")
        return typeof value === "boolean"
          ? null
          : "expected boolean literal or {path}/{call} binding";
      return null; // DynamicValue accepts any JSON value literally
    }
    case "action":
      return isActionShaped(value)
        ? null
        : `expected {event:{name}} or {functionCall:{call}}`;
    case "childRef":
      return typeof value === "string"
        ? null
        : "expected a component id string";
    case "childList": {
      if (Array.isArray(value)) {
        return value.every((v) => typeof v === "string")
          ? null
          : "expected component id strings";
      }
      if (
        value &&
        typeof value === "object" &&
        typeof (value as { componentId?: unknown }).componentId === "string" &&
        typeof (value as { path?: unknown }).path === "string"
      ) {
        return null;
      }
      return "expected id[] or {componentId, path}";
    }
    case "literal": {
      if (spec.literalType && typeof value !== spec.literalType) {
        return `expected ${spec.literalType}`;
      }
      if (spec.enumValues && !spec.enumValues.some((v) => v === value)) {
        return `value not in enum (${spec.enumValues.map(String).join("|")}) for "${name}"`;
      }
      return null;
    }
    default:
      return null;
  }
}

function isFunctionCallShaped(value: unknown): boolean {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    typeof (value as { call?: unknown }).call === "string" &&
    !("path" in value)
  );
}

function isActionShaped(value: unknown): boolean {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  if ("functionCall" in value) {
    return isFunctionCallShaped(
      (value as { functionCall?: unknown }).functionCall,
    );
  }
  if ("event" in value) {
    const event = (value as { event?: unknown }).event;
    return (
      typeof event === "object" &&
      event !== null &&
      typeof (event as { name?: unknown }).name === "string"
    );
  }
  return false;
}

/** Structural equality for JSON-shaped prop declarations. */
export function deepEqual(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((item, i) => deepEqual(item, b[i]));
  }
  if (typeof a === "object" && typeof b === "object" && a && b) {
    const ka = Object.keys(a as object);
    const kb = Object.keys(b as object);
    return (
      ka.length === kb.length &&
      ka.every(
        (k) =>
          k in (b as object) &&
          deepEqual(
            (a as Record<string, unknown>)[k],
            (b as Record<string, unknown>)[k],
          ),
      )
    );
  }
  return false;
}
