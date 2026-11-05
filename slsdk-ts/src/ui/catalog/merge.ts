/**
 * Catalog merge — the composition side of the contribution channel.
 *
 * mergeCatalog(base, contributions) produces a fresh catalog document that
 * carries the base entries plus every contributed control declaration, ready
 * to be distributed as a whole (the vocabulary surface agents see). The base
 * document is never mutated; contributions ride in with the $defs their
 * entries reference, and anyComponent stays exhaustive over the merged
 * vocabulary.
 */
import { collectRefs } from "./validate";

/** The document shape a merge consumes and produces (a CatalogDocument). */
export interface MergedCatalogDocument {
  catalogId: string;
  components: Record<string, unknown>;
  $defs?: Record<string, unknown>;
  [key: string]: unknown;
}

/** The slice of a component implementation the merge reads. */
export interface CatalogContribution {
  /** The catalog entry declaration (present on contributed controls). */
  schema?: unknown;
  /** Local $defs the entry references; migrated into the merged document. */
  defs?: Record<string, unknown>;
}

/**
 * Resolve a document-relative pointer ("#/components/Badge") against the
 * merged document. Returns undefined when the pointer does not resolve.
 */
function resolveLocalRef(
  document: Record<string, unknown>,
  ref: string,
): unknown {
  let node: unknown = document;
  for (const seg of ref.slice(1).split("/").filter(Boolean)) {
    if (!node || typeof node !== "object") return undefined;
    node = (node as Record<string, unknown>)[seg];
  }
  return node;
}

/** The slice of the base document the merge reads; the product keeps every other base key. */
export interface CatalogMergeBase {
  catalogId: string;
  components: Record<string, unknown>;
  $defs?: Record<string, unknown>;
}

/**
 * Merge contributed control declarations into a base catalog document.
 * Contributions are (name, implementation) pairs — pairs whose implementation
 * carries no schema (preset wiring) are skipped. Name and $defs collisions
 * are rejected, and every document-relative $ref of a contributed entry must
 * resolve against the merged document before it is returned.
 */
export function mergeCatalog(
  base: CatalogMergeBase,
  contributions: Iterable<readonly [string, CatalogContribution]>,
): MergedCatalogDocument {
  const entries: Array<[string, CatalogContribution]> = [];
  for (const [name, impl] of contributions) {
    if (!impl.schema) continue;
    if (Object.hasOwn(base.components, name)) {
      throw new Error(
        `mergeCatalog: component "${name}" collides with an existing base catalog entry`,
      );
    }
    entries.push([name, impl]);
  }
  if (entries.length === 0) {
    return { ...base };
  }

  const baseDefs = base.$defs ?? {};
  const contributedDefs: Record<string, unknown> = {};
  for (const [, { defs }] of entries) {
    if (!defs) continue;
    for (const [key, value] of Object.entries(defs)) {
      if (Object.hasOwn(baseDefs, key) || Object.hasOwn(contributedDefs, key)) {
        throw new Error(
          `mergeCatalog: $defs key "${key}" collides with an existing definition`,
        );
      }
      contributedDefs[key] = value;
    }
  }

  const components = { ...base.components };
  for (const [name, { schema }] of entries) {
    components[name] = schema;
  }

  const merged: MergedCatalogDocument = {
    ...base,
    components,
    $defs: { ...baseDefs, ...contributedDefs },
  };

  // anyComponent stays exhaustive over the merged vocabulary.
  const anyComponent = merged.$defs?.anyComponent as
    { oneOf?: unknown[] } | undefined;
  if (Array.isArray(anyComponent?.oneOf)) {
    merged.$defs = {
      ...merged.$defs,
      anyComponent: {
        ...anyComponent,
        oneOf: [
          ...(anyComponent.oneOf as Array<Record<string, unknown>>),
          ...entries.map(([name]) => ({ $ref: `#/components/${name}` })),
        ],
      },
    };
  }

  // Reference integrity: every document-relative $ref of a contributed
  // entry must resolve inside the merged document.
  for (const [name, { schema }] of entries) {
    for (const ref of collectRefs(schema)) {
      if (!ref.startsWith("#")) continue;
      if (resolveLocalRef(merged, ref) === undefined) {
        throw new Error(
          `mergeCatalog: the entry for "${name}" references "${ref}", which does not resolve in the merged document`,
        );
      }
    }
  }

  return merged;
}
