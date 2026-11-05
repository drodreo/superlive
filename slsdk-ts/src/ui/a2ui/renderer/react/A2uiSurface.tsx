/**
 * <A2uiSurface> — the React projection of one A2UI surface.
 *
 * Pure function face (batch 2 scope): description JSON in → rendered tree +
 * actions out. The component owns a SurfaceModel per description instance;
 * React subscribes through useSyncExternalStore at two levels:
 *
 * - surface level: structural version (tree shape changes → re-render);
 * - node level: each rendered node owns a ComponentBinding whose evaluated
 *   props snapshot is the external store (binding changes re-render exactly
 *   the affected node; the memo comparator keeps unchanged subtrees out).
 *
 * StrictMode double-mount safety follows the official A2uiSurface pattern:
 * the surface instance is created lazily in a ref box (idempotent under
 * double render) and only the subscription is attached/detached in effects —
 * a detached surface keeps its state and can re-attach losslessly, so the
 * simulated unmount/remount leaves no duplicates and no torn subscriptions.
 * Subscriptions are fully internal to the instance, so an unmounted surface
 * becomes garbage-collectable as a whole.
 */
import {
  memo,
  useEffect,
  useMemo,
  useRef,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import catalogJson from "../../../catalog/superlive.catalog.json";
import type {
  ActionMessagePayload,
  ErrorMessagePayload,
  ServerToClientMessage,
} from "../../types";
import {
  SurfaceModel,
  type CatalogDocument,
  type SurfaceNode,
} from "../surface";
import type { FunctionTable } from "../binding";
import { defaultRegistry } from "../registry";
import type { A2uiRegistry, ComponentRenderContext } from "./wiring";

/** The catalog JSON document is data, not code — typed at the boundary. */
const SUPERLIVE_CATALOG = catalogJson as unknown as CatalogDocument;

export interface A2uiSurfaceProps {
  /**
   * Description payload: raw JSON text or pre-parsed envelope messages. Like
   * the catalog, it is captured per surface instance by identity: a fresh
   * array every render rebuilds the surface and replays the description —
   * pass a stable reference (or memoize).
   */
  description: string | ServerToClientMessage[];
  /** Host-side action consumer (event actions surface here). */
  onAction?: (payload: ActionMessagePayload) => void;
  /** Renderer error channel (validation, unknown vocabulary, cycles …). */
  onError?: (payload: ErrorMessagePayload) => void;
  /** Vocabulary implementations; defaults to the preset superlive registry. */
  registry?: A2uiRegistry;
  /**
   * Vocabulary document; defaults to the preset superlive catalog. Pass the
   * mergeCatalog product when contributed controls are in play. The catalog
   * is captured per surface instance: swapping it rebuilds the surface (and
   * replays the description).
   */
  catalog?: CatalogDocument;
  /**
   * Host function implementations for `{functionCall}` actions (e.g. the
   * HTTP action family). Merged over the renderer builtins (predicates) and
   * captured per surface instance like the catalog.
   */
  functions?: FunctionTable;
  /**
   * Receives the live SurfaceModel once the description identity creates it
   * (and null when a rebuild replaces it). This is the host-side incremental
   * channel: calling apply() on the handed-out model feeds push updates into
   * the live surface without rebuilding it — the description prop stays
   * identity-stable and the node bindings keep their state. Reads on the
   * model (surfaceId, dataModel) are likewise available here.
   */
  surfaceRef?: (surface: SurfaceModel | null) => void;
}

export function A2uiSurface({
  description,
  onAction,
  onError,
  registry = defaultRegistry,
  catalog = SUPERLIVE_CATALOG,
  functions,
  surfaceRef,
}: A2uiSurfaceProps) {
  const box = useRef<{
    description: string | ServerToClientMessage[];
    catalog: CatalogDocument;
    functions: FunctionTable | undefined;
    surface: SurfaceModel;
  } | null>(null);

  // Lazy init inside render: idempotent per description identity, so the
  // StrictMode double render reuses the same instance instead of duplicating.
  // A superseded instance is never disposed here — disposing would rip
  // subscriptions React is still bridged to mid-render; all of a surface's
  // subscriptions are internal to the instance, so dropping the last
  // reference lets the whole graph be garbage-collected.
  if (
    box.current === null ||
    box.current.description !== description ||
    box.current.catalog !== catalog ||
    box.current.functions !== functions
  ) {
    const surface = new SurfaceModel({
      catalog,
      onAction,
      onError,
      functions,
    });
    surface.apply(description);
    box.current = { description, catalog, functions, surface };
  }
  const surface = box.current.surface;

  // Hand the live instance to the host (and null it on replacement/unmount).
  // Effect, not render: the callback is host state mutation and must stay
  // out of the render path; the [surface] dependency re-fires exactly when a
  // rebuild swapped the instance.
  useEffect(() => {
    surfaceRef?.(surface);
    return () => surfaceRef?.(null);
  }, [surface, surfaceRef]);

  // Structural store: re-render when the tree shape changed.
  useSyncExternalStore(
    surface.subscribe,
    surface.getVersion,
    surface.getVersion,
  );

  const tree = surface.getTree();
  if (!tree) {
    return (
      <div className="sl-ui-a2ui-placeholder" role="status">
        Loading…
      </div>
    );
  }
  return <A2uiNode surface={surface} node={tree} registry={registry} />;
}

interface NodeProps {
  surface: SurfaceModel;
  node: SurfaceNode;
  registry: A2uiRegistry;
}

/**
 * Memo comparator: a node re-renders when its identity (component id +
 * iteration data path) changed, its props declaration changed (reference —
 * the surface upsert keeps declarations reference-stable when equal), or the
 * child id sequence changed. Value-level dynamics are NOT part of this
 * decision: they flow through the node's own external store below.
 */
function nodePropsEqual(a: NodeProps, b: NodeProps): boolean {
  if (a.surface !== b.surface || a.registry !== b.registry) return false;
  const an = a.node;
  const bn = b.node;
  if (an === bn) return true;
  if (
    an.componentId !== bn.componentId ||
    an.dataPath !== bn.dataPath ||
    an.propsSource !== bn.propsSource
  ) {
    return false;
  }
  if ((an.child?.componentId ?? null) !== (bn.child?.componentId ?? null)) {
    return false;
  }
  const aSlots = Object.keys(an.childrenMap ?? {});
  const bSlots = Object.keys(bn.childrenMap ?? {});
  if (aSlots.length !== bSlots.length) return false;
  return aSlots.every((slot) => {
    const aIds = an.childrenMap?.[slot].map((c) => c.componentId) ?? [];
    const bIds = bn.childrenMap?.[slot].map((c) => c.componentId) ?? [];
    return aIds.length === bIds.length && aIds.every((id, i) => id === bIds[i]);
  });
}

const A2uiNode = memo(function A2uiNode({
  surface,
  node,
  registry,
}: NodeProps): ReactNode {
  const impl = registry.get(node.component);
  if (!impl) {
    // Vocabulary entry not wired (yet): report and degrade to nothing — the
    // host keeps working, and the slot can appear once the control registers.
    surface.report(
      "UNKNOWN_COMPONENT_IMPLEMENTATION",
      `component "${node.component}" (id "${node.componentId}") has no registered implementation`,
    );
    return null;
  }

  // One binding store per node identity; keyed on the comparator's stable
  // subset (id / data path / declaration reference) so a tree rebuild that
  // re-creates equal nodes reuses the binding instead of re-subscribing.
  const binding = useMemo(
    () => surface.createNodeBinding(node),
    [surface, node.componentId, node.dataPath, node.propsSource],
  );
  useEffect(() => () => binding.dispose(), [binding]);
  const evaluated = useSyncExternalStore(
    binding.subscribe,
    binding.getSnapshot,
    binding.getSnapshot,
  );

  const ctx: ComponentRenderContext = useMemo(() => {
    return {
      surfaceId: surface.surfaceId ?? "",
      componentId: node.componentId,
      props: evaluated,
      get<T = unknown>(name: string): T | undefined {
        return evaluated[name] as T | undefined;
      },
      writeBack(name: string, value: unknown): void {
        surface.writeBack(node, name, value);
      },
      dispatchAction(
        name: string,
        extraContext?: Record<string, unknown>,
      ): void {
        surface.dispatchAction(node, name, extraContext);
      },
      resolveRowValue(
        rowsPropName: string,
        decl: unknown,
        rowIndex: number,
      ): unknown {
        return surface.resolveRowValue(node, rowsPropName, decl, rowIndex);
      },
      dispatchRowAction(
        rowsPropName: string,
        actionDecl: unknown,
        rowIndex: number,
        extraContext?: Record<string, unknown>,
      ): void {
        surface.dispatchRowAction(
          node,
          rowsPropName,
          actionDecl,
          rowIndex,
          extraContext,
        );
      },
      renderChild(): ReactNode {
        return node.child ? (
          <A2uiNode surface={surface} node={node.child} registry={registry} />
        ) : null;
      },
      renderChildren(slot: string): ReactNode {
        const children = node.childrenMap?.[slot] ?? [];
        return (
          <>
            {children.map((child) => (
              <A2uiNode
                key={child.componentId + (child.dataPath ?? "")}
                surface={surface}
                node={child}
                registry={registry}
              />
            ))}
          </>
        );
      },
    };
  }, [surface, node, registry, evaluated]);

  return impl.render(ctx);
}, nodePropsEqual);
