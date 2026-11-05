/**
 * React wiring contract for A2UI component implementations.
 *
 * A registry entry turns an evaluated-props snapshot plus the surface
 * interaction pipeline into an existing superlive controlled component —
 * the presentational components stay protocol-unaware (they keep their
 * value/onChange controlled shape); every piece of A2UI semantics lives in
 * the render context handed to `render`.
 */
import type { ReactNode } from "react";
import type { CatalogComponentDefinition } from "../../../catalog/types";
import { validateComponentEntry } from "../../../catalog/validate";

/**
 * Everything a wired component may touch. Prop values come from the
 * evaluated snapshot (literals verbatim, dynamic slots already resolved
 * against the data model); writes and actions go through the surface.
 */
export interface ComponentRenderContext {
  readonly surfaceId: string;
  readonly componentId: string;
  /** Evaluated props snapshot (fresh on every binding change). */
  readonly props: Readonly<Record<string, unknown>>;
  /** Typed accessor over {@link props}. */
  get<T = unknown>(name: string): T | undefined;
  /**
   * Two-way binding write-back: writes the bound data model path locally
   * (keystrokes stay client-side per the A2UI specification). Unbound slots
   * silently ignore the write.
   */
  writeBack(name: string, value: unknown): void;
  /**
   * Dispatch the Action declared at `name` (event → host, call → local fn).
   * `extraContext` merges interaction products (selected page, assembled
   * filter, form values) into the event context over the declared one.
   */
  dispatchAction(name: string, extraContext?: Record<string, unknown>): void;
  /**
   * Evaluate a dynamic declaration against a row scope (DataTable rowActions
   * labels): relative {"path"} pointers resolve against the bound row
   * element at `rowIndex`, literals pass through. Requires the named rows
   * prop to be a {"path"} binding (otherwise the error channel reports and
   * the value is undefined).
   */
  resolveRowValue(
    rowsPropName: string,
    decl: unknown,
    rowIndex: number,
  ): unknown;
  /**
   * Dispatch an explicit Action declaration rooted at a row scope (DataTable
   * rowActions): the event context's pointers resolve against the bound row
   * element at `rowIndex` — the same relative-path rule template iteration
   * applies to ChildList subtrees.
   */
  dispatchRowAction(
    rowsPropName: string,
    actionDecl: unknown,
    rowIndex: number,
    extraContext?: Record<string, unknown>,
  ): void;
  /** Render the declared single child (Button.child), or null while pending. */
  renderChild(): ReactNode;
  /** Render a declared ChildList slot (Modal.children etc.). */
  renderChildren(slot: string): ReactNode;
}

/** A registry-ready component implementation. */
export interface A2uiComponentImpl {
  render(ctx: ComponentRenderContext): ReactNode;
  /**
   * Contributed controls carry their catalog entry declaration (the schema
   * side of the vocabulary member); preset implementations leave it unset —
   * their entries live in the authoritative catalog JSON.
   */
  schema?: CatalogComponentDefinition;
  /**
   * Local $defs the contributed entry references; migrated into the merged
   * catalog document by mergeCatalog (undefined for preset implementations).
   */
  defs?: Record<string, unknown>;
}

/**
 * Vocabulary name → implementation. Open by design: contributions only add
 * entries; the renderer core never changes for new controls.
 *
 * The contribution channel is {@link A2uiRegistry.registerControl} — an
 * immutable extension in the official Catalog.withEntries pattern: it
 * returns a fresh registry and never mutates the receiver, so hosts and
 * component authors can fork their own vocabulary off a shared preset
 * without leaking the customization into other surfaces.
 */
export class A2uiRegistry extends Map<string, A2uiComponentImpl> {
  /**
   * Contribute a custom control: implementation + catalog entry declaration
   * (+ the local $defs the entry references). The declaration must satisfy
   * the same structural rules the preset catalog is held to (closed allOf
   * composition, const discriminator, ComponentId/ChildList structural
   * slots); violations and name collisions are rejected with an error —
   * an existing vocabulary member is never silently overridden.
   */
  registerControl(
    name: string,
    impl: A2uiComponentImpl,
    schema: CatalogComponentDefinition,
    defs?: Record<string, unknown>,
  ): A2uiRegistry {
    if (this.has(name)) {
      throw new Error(
        `registerControl: component "${name}" is already registered — overriding an existing vocabulary member is rejected`,
      );
    }
    const errors = validateComponentEntry(name, schema);
    if (errors.length > 0) {
      throw new Error(
        `registerControl: the catalog entry for "${name}" violates the declaration rules:\n- ${errors.join("\n- ")}`,
      );
    }
    const next = new A2uiRegistry(this);
    next.set(name, defs ? { ...impl, schema, defs } : { ...impl, schema });
    return next;
  }
}

/**
 * Wire a component implementation whose user-input props write back into the
 * local data model (Input.value, Checkbox.checked, SearchBar.value …). Pure
 * marker constructor today — the flag documents the two-way-binding contract
 * the wrapped render function relies on.
 */
export function bindControlled(
  render: (ctx: ComponentRenderContext) => ReactNode,
): A2uiComponentImpl {
  return { render };
}

/**
 * Wire a component implementation whose activation props dispatch declared
 * actions (Button.action, SearchBar.advancedAction …). Marker constructor
 * paired with {@link bindControlled} for registry readability.
 */
export function bindAction(
  render: (ctx: ComponentRenderContext) => ReactNode,
): A2uiComponentImpl {
  return { render };
}
