/**
 * Binding evaluation for the A2UI renderer.
 *
 * Two concerns, mirroring the official web_core split (generic-binder +
 * data-context) at subset scale:
 *
 * 1. Schema inspection — walk a component's catalog JSON Schema node and
 *    classify each declared prop: dynamic (DynamicString/Number/Boolean/Value
 *    $refs), action ($ref Action), child reference ($ref ComponentId), child
 *    list ($ref ChildList) or plain literal. The catalog JSON is the
 *    authoritative vocabulary source (batch 1 decision), so the classifier
 *    reads the same document the validator reads.
 *
 * 2. Evaluation — resolve each dynamic prop against the surface DataModel
 *    ({"path"} binding) or the local function table ({"call"}), collect the
 *    data model paths read along the way, and keep a snapshot subscription
 *    that only fires when a re-evaluation actually changes a value. The
 *    dependency set is derived from what evaluation reads, so a listener is
 *    woken exactly when a path it depends on changed.
 */
import type { DataModel } from "./data-model";
import type { Unsubscribe } from "./signals";

// ===== catalog schema inspection =====

export type DynamicType = "string" | "number" | "boolean" | "value";

export interface PropSpec {
  kind: "dynamic" | "action" | "childRef" | "childList" | "literal";
  /** For kind "dynamic": which Dynamic* family the $ref points at. */
  dynamicType?: DynamicType;
  /** For kind "literal": declared primitive type, when present. */
  literalType?: "string" | "number" | "boolean";
  /** For kind "literal": declared enum values, when present. */
  enumValues?: readonly unknown[];
}

export interface ComponentSpec {
  props: Record<string, PropSpec>;
  required: readonly string[];
}

type Schema = Record<string, unknown>;

function refTail(ref: unknown): string {
  return typeof ref === "string" ? ref.slice(ref.lastIndexOf("/") + 1) : "";
}

/**
 * Public props of the official ComponentCommon definition (common_types.json
 * v0.9 — external to every catalog document, mirrored here; protocol-fixed).
 * `id` is the protocol-level component key (stripped before prop validation;
 * its required status is enforced by the protocol layer) and `accessibility`
 * is a free-form literal object. Every catalog component composes this ref,
 * so its props must land in the whitelist or common props like
 * `accessibility` would be rejected as undeclared.
 */
const OFFICIAL_COMMON_PROPS: Record<
  string,
  Readonly<Record<string, PropSpec>>
> = {
  ComponentCommon: {
    id: { kind: "literal", literalType: "string" },
    accessibility: { kind: "literal" },
  },
};

/** Collect the props a $ref branch contributes, resolving local defs. */
function refProps(
  ref: string,
  defs: Readonly<Record<string, unknown>> | undefined,
  requiredSet: Set<string>,
): [string, PropSpec][] {
  const tail = refTail(ref);
  const official = OFFICIAL_COMMON_PROPS[tail];
  if (official) return Object.entries(official);
  const local = defs?.[tail];
  if (!local || typeof local !== "object" || Array.isArray(local)) return [];
  const s = local as Schema;
  if (Array.isArray(s.required)) {
    for (const r of s.required) {
      if (typeof r === "string") requiredSet.add(r);
    }
  }
  if (!s.properties || typeof s.properties !== "object") return [];
  return Object.entries(s.properties as Record<string, unknown>).map(
    ([name, propSchema]) => [name, classifyProp(propSchema)],
  );
}

/**
 * Classify one property schema node from the component's catalog entry.
 * Component entries are `allOf: [ComponentCommon, CatalogComponentCommon,
 * {properties, required}]`; the classifier accepts either the wrapped or the
 * bare `{properties, required}` form and merges all object branches. Shared
 * `$ref` branches contribute their public props: ComponentCommon resolves
 * from the official mirror above, everything else from the catalog document's
 * local `$defs` (CatalogComponentCommon → weight), passed as `defs`.
 */
export function inspectComponentSchema(
  schemaNode: unknown,
  defs?: Readonly<Record<string, unknown>>,
): ComponentSpec {
  const props: Record<string, PropSpec> = {};
  const requiredSet = new Set<string>();

  const visit = (node: unknown): void => {
    if (!node || typeof node !== "object" || Array.isArray(node)) return;
    const s = node as Schema;
    if (typeof s.$ref === "string") {
      // $ref-only wrapper: contribute the referenced common props.
      for (const [name, spec] of refProps(s.$ref, defs, requiredSet)) {
        props[name] = spec;
      }
      return;
    }
    if (Array.isArray(s.allOf)) {
      for (const branch of s.allOf) visit(branch);
    }
    if (s.properties && typeof s.properties === "object") {
      for (const [name, propSchema] of Object.entries(
        s.properties as Record<string, unknown>,
      )) {
        props[name] = classifyProp(propSchema);
      }
    }
    if (Array.isArray(s.required)) {
      for (const r of s.required) {
        if (typeof r === "string") requiredSet.add(r);
      }
    }
  };
  visit(schemaNode);
  return { props, required: [...requiredSet] };
}

function classifyProp(propSchema: unknown): PropSpec {
  if (
    !propSchema ||
    typeof propSchema !== "object" ||
    Array.isArray(propSchema)
  ) {
    return { kind: "literal" };
  }
  const s = propSchema as Schema;
  const tail = refTail(s.$ref);
  switch (tail) {
    case "DynamicString":
      return { kind: "dynamic", dynamicType: "string" };
    case "DynamicNumber":
      return { kind: "dynamic", dynamicType: "number" };
    case "DynamicBoolean":
      return { kind: "dynamic", dynamicType: "boolean" };
    case "DynamicValue":
      return { kind: "dynamic", dynamicType: "value" };
    case "Action":
      return { kind: "action" };
    case "ComponentId":
      return { kind: "childRef" };
    case "ChildList":
      return { kind: "childList" };
    default:
      break;
  }
  const spec: PropSpec = { kind: "literal" };
  if (s.type === "string" || s.type === "number" || s.type === "boolean") {
    spec.literalType = s.type;
  }
  if (Array.isArray(s.enum)) spec.enumValues = s.enum;
  return spec;
}

// ===== dynamic value evaluation =====

/** Local function table for {"call"} bindings (catalog functions domain). */
export type FunctionTable = Record<
  string,
  (args: Record<string, unknown>) => unknown
>;

export interface EvalFrame {
  /** Whether a `{"path"}` binding was read (dependency to subscribe). */
  dep?: string;
  /** Error text when the declaration could not be resolved. */
  error?: string;
}

export function isDataBinding(decl: unknown): decl is { path: string } {
  return (
    typeof decl === "object" &&
    decl !== null &&
    !Array.isArray(decl) &&
    typeof (decl as { path?: unknown }).path === "string" &&
    (decl as { path: string }).path.length > 0 &&
    !("call" in decl)
  );
}

export function isFunctionCall(decl: unknown): decl is {
  call: string;
  args?: Record<string, unknown>;
} {
  return (
    typeof decl === "object" &&
    decl !== null &&
    !Array.isArray(decl) &&
    typeof (decl as { call?: unknown }).call === "string" &&
    !("path" in decl)
  );
}

/**
 * Resolve a declared path against an evaluation scope (protocol.md "Path
 * resolution & scope"). Paths starting with "/" are absolute — always from the
 * data model root. Relative paths (no leading "/") only exist inside a
 * template-iteration Child Scope: they resolve against the iterated array
 * element (`name` under scope "/users/0" → "/users/0/name"). A relative path
 * in the Root Scope is an error (null).
 */
export function resolveScopePath(
  path: string,
  scopePath: string | undefined,
): string | null {
  if (path.startsWith("/")) return path;
  if (!scopePath) return null;
  return `${scopePath}/${path}`;
}

/**
 * Resolve one dynamic declaration to its current value.
 *
 * Literals pass through (with a light type check for string/number/boolean
 * families); {"path"} reads the data model and records the dependency;
 * {"call"} resolves arguments (each arg may itself be a DynamicValue) and
 * invokes the local function table. Failures are reported on `frame.error`
 * and evaluate to undefined — rendering degrades, it never throws.
 */
export function resolveDynamic(
  decl: unknown,
  expected: DynamicType,
  dataModel: DataModel,
  functions: FunctionTable,
  frame: EvalFrame,
  scopePath?: string,
): unknown {
  if (decl === undefined) return undefined;
  if (isDataBinding(decl)) {
    const pointer = resolveScopePath(decl.path, scopePath);
    if (pointer === null) {
      frame.error = `relative path "${decl.path}" used outside a template iteration scope`;
      return undefined;
    }
    frame.dep = pointer;
    return dataModel.get(pointer);
  }
  if (isFunctionCall(decl)) {
    const fn = functions[decl.call];
    if (!fn) {
      frame.error = `function "${decl.call}" is not registered`;
      return undefined;
    }
    const args: Record<string, unknown> = {};
    for (const [key, raw] of Object.entries(decl.args ?? {})) {
      const argFrame: EvalFrame = {};
      // DynamicValue args: {"path"} reads the model (undefined stays
      // undefined), literals pass through — no fallback to the raw decl.
      args[key] = resolveDynamic(
        raw,
        "value",
        dataModel,
        functions,
        argFrame,
        scopePath,
      );
      if (argFrame.dep) frame.dep = argFrame.dep;
      if (argFrame.error) frame.error = argFrame.error;
    }
    return fn(args);
  }
  // Literal branch: light type conformance per family.
  switch (expected) {
    case "string":
      if (typeof decl === "string") return decl;
      frame.error = `expected a string literal, binding or call; got ${typeof decl}`;
      return undefined;
    case "number":
      if (typeof decl === "number") return decl;
      frame.error = `expected a number literal, binding or call; got ${typeof decl}`;
      return undefined;
    case "boolean":
      if (typeof decl === "boolean") return decl;
      frame.error = `expected a boolean literal, binding or call; got ${typeof decl}`;
      return undefined;
    case "value":
      return decl;
  }
}

// ===== per-component binding snapshot =====

export interface BindingSource {
  componentSpec: ComponentSpec;
  /** Raw props of the component instance (id/component stripped). */
  propsSource: Record<string, unknown>;
  surfaceId: string;
  componentId: string;
  /**
   * Data path of the iterated array element when the instance originates from
   * template iteration (ChildList scope); relative {"path"} bindings resolve
   * against it, absolute paths stay root-relative.
   */
  dataPath?: string;
}

/**
 * Evaluated-props snapshot store for one rendered component node: evaluates
 * every dynamic prop against the data model / function table, subscribes to
 * exactly the paths evaluation read, and notifies only when the recomputed
 * snapshot differs. This is the object React's useSyncExternalStore bridges.
 */
export class ComponentBinding {
  private snapshot: Record<string, unknown> = {};
  private deps: string[] = [];
  private unsubs: Unsubscribe[] = [];
  private listeners = new Set<() => void>();
  private disposed = false;

  constructor(
    private readonly source: BindingSource,
    private readonly dataModel: DataModel,
    private readonly functions: FunctionTable,
    private readonly onError: (message: string) => void,
  ) {
    this.recompute();
    this.resubscribe();
  }

  /** Current evaluated props (literals + resolved dynamics + child decls). */
  getSnapshot = (): Readonly<Record<string, unknown>> => this.snapshot;

  /** Data-model paths the latest evaluation read. */
  getDependencies(): readonly string[] {
    return this.deps;
  }

  subscribe = (cb: () => void): Unsubscribe => {
    this.listeners.add(cb);
    return () => {
      this.listeners.delete(cb);
    };
  };

  dispose(): void {
    this.disposed = true;
    for (const unsub of this.unsubs) unsub();
    this.unsubs = [];
    this.listeners.clear();
  }

  /** Re-evaluate; notify React listeners only when a value actually changed. */
  refresh(): void {
    if (this.disposed) return;
    const changed = this.recompute();
    this.resubscribe();
    if (changed) {
      for (const cb of [...this.listeners]) cb();
    }
  }

  private recompute(): boolean {
    const next: Record<string, unknown> = {};
    const deps = new Set<string>();
    for (const [name, spec] of Object.entries(
      this.source.componentSpec.props,
    )) {
      const raw = this.source.propsSource[name];
      if (raw === undefined) continue;
      if (spec.kind === "dynamic") {
        const frame: EvalFrame = {};
        next[name] = resolveDynamic(
          raw,
          spec.dynamicType ?? "value",
          this.dataModel,
          this.functions,
          frame,
          this.source.dataPath,
        );
        if (frame.dep) deps.add(frame.dep);
        if (frame.error) {
          this.onError(
            `[a2ui] ${this.source.surfaceId}/${this.source.componentId} prop "${name}": ${frame.error}`,
          );
        }
      } else {
        // Literal / action / childRef / childList slots pass through verbatim;
        // structural handling happens in the surface / registry layers.
        next[name] = raw;
      }
    }
    const changed =
      propsDiffer(this.snapshot, next) || depsChanged(this.deps, deps);
    this.snapshot = next;
    this.deps = [...deps];
    return changed;
  }

  private resubscribe(): void {
    for (const unsub of this.unsubs) unsub();
    this.unsubs = this.deps.map((path) =>
      this.dataModel.subscribe(path, () => this.refresh()),
    );
  }
}

function propsDiffer(
  a: Readonly<Record<string, unknown>>,
  b: Readonly<Record<string, unknown>>,
): boolean {
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  if (ka.length !== kb.length) return true;
  for (const k of ka) {
    if (!Object.is(a[k], b[k])) return true;
  }
  return false;
}

function depsChanged(a: readonly string[], b: ReadonlySet<string>): boolean {
  if (a.length !== b.size) return true;
  return a.some((p) => !b.has(p));
}
