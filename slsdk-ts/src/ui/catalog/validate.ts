/**
 * Minimal JSON Schema validator for the keyword subset the superlive catalog
 * uses (no JSON-Schema library on the dependency whitelist): type / const /
 * enum / properties / required / additionalProperties / unevaluatedProperties
 * / allOf / oneOf / items / $ref (+ ignored discriminator annotation).
 *
 * Two entry points share the same resolution core:
 * - {@link createCatalogValidator}: instance validation against a whole
 *   document context (the catalog authority checks in tests/catalog.test.ts
 *   ride on it);
 * - {@link validateComponentEntry}: the structural declaration rules every
 *   component entry must satisfy — the gate every contributed control passes
 *   through at registration time, the same rules the preset catalog is held
 *   to.
 */

/** The official A2UI v0.9 common types base every catalog may reference. */
export const COMMON_TYPES_BASE =
  "https://a2ui.org/specification/v0_9/common_types.json";

type Schema = Record<string, unknown>;

/** Collect every $ref string in a schema subtree. */
export function collectRefs(schema: unknown, out: string[] = []): string[] {
  if (Array.isArray(schema)) {
    for (const item of schema) collectRefs(item, out);
    return out;
  }
  if (schema && typeof schema === "object") {
    const rec = schema as Record<string, unknown>;
    if (typeof rec.$ref === "string") out.push(rec.$ref);
    for (const value of Object.values(rec)) collectRefs(value, out);
  }
  return out;
}

/** Resolve a $ref fragment pointer against a document. Returns null when unresolvable. */
function resolvePointer(
  doc: Record<string, unknown>,
  fragment: string,
): Schema | null {
  let node: unknown = doc;
  for (const seg of fragment.split("/").filter(Boolean)) {
    if (!node || typeof node !== "object") return null;
    node = (node as Record<string, unknown>)[seg];
  }
  return node === undefined ? null : (node as Schema);
}

export interface CatalogValidatorContext {
  /** The document self-relative (`#…`) $refs resolve against. */
  document: Record<string, unknown>;
  /** Snapshot of the official common types (the external $ref base). */
  commonTypes?: Record<string, unknown>;
}

/**
 * A validator bound to a document context: `#…` refs resolve against
 * `context.document`, official-base refs against `context.commonTypes`
 * (unresolvable there when the snapshot is absent).
 */
export function createCatalogValidator(context: CatalogValidatorContext) {
  function resolveInDoc(
    ref: string,
    doc: Record<string, unknown>,
  ): Schema | null {
    const hashIndex = ref.indexOf("#");
    const base = hashIndex === -1 ? ref : ref.slice(0, hashIndex);
    const fragment = hashIndex === -1 ? "" : ref.slice(hashIndex + 1);
    const documentId =
      typeof context.document.$id === "string" ? context.document.$id : null;
    const target =
      base === ""
        ? doc
        : base === COMMON_TYPES_BASE
          ? (context.commonTypes ?? null)
          : base === documentId || base === "catalog.json"
            ? context.document
            : null;
    if (!target) return null;
    return resolvePointer(target, fragment);
  }

  /**
   * The document a resolved ref belongs to (for further relative refs inside it).
   * A literal `catalog.json` base (how the official common-types snapshot
   * references the consuming catalog) always means this catalog document.
   */
  function docOfRef(
    ref: string,
    doc: Record<string, unknown>,
  ): Record<string, unknown> {
    const hashIndex = ref.indexOf("#");
    const base = hashIndex === -1 ? ref : ref.slice(0, hashIndex);
    if (base === COMMON_TYPES_BASE) return context.commonTypes ?? doc;
    if (base === "catalog.json") return context.document;
    return doc;
  }

  function validate(
    schema: unknown,
    value: unknown,
    at: string,
    doc: Record<string, unknown> = context.document,
  ): string[] {
    const errors: string[] = [];
    if (schema === undefined || schema === null) return errors;
    if (typeof schema !== "object" || Array.isArray(schema)) return errors;
    const s = schema as Schema;

    if (typeof s.$ref === "string") {
      const target = resolveInDoc(s.$ref, doc);
      if (!target) {
        errors.push(`${at}: unresolvable $ref ${s.$ref}`);
        return errors;
      }
      errors.push(...validate(target, value, at, docOfRef(s.$ref, doc)));
    }

    if (s.type !== undefined) {
      const types = Array.isArray(s.type) ? s.type : [s.type];
      if (!types.some((t) => checkType(t as string, value))) {
        errors.push(`${at}: expected type ${types.join("|")}`);
      }
    }
    if (s.const !== undefined && !deepEqual(s.const, value)) {
      errors.push(`${at}: expected const ${JSON.stringify(s.const)}`);
    }
    if (Array.isArray(s.enum) && !s.enum.some((e) => deepEqual(e, value))) {
      errors.push(`${at}: value not in enum`);
    }

    if (Array.isArray(s.allOf)) {
      for (const branch of s.allOf) {
        errors.push(...validate(branch, value, at, doc));
      }
      if (s.unevaluatedProperties === false && checkType("object", value)) {
        const declared = new Set<string>();
        const collect = (node: unknown, nodeDoc: Record<string, unknown>) => {
          if (!node || typeof node !== "object" || Array.isArray(node)) return;
          const rec = node as Schema;
          if (typeof rec.$ref === "string") {
            const resolved = resolveInDoc(rec.$ref, nodeDoc);
            if (resolved) collect(resolved, docOfRef(rec.$ref, nodeDoc));
          }
          if (Array.isArray(rec.allOf)) {
            for (const b of rec.allOf) collect(b, nodeDoc);
          }
          if (rec.properties) {
            for (const k of Object.keys(rec.properties as object)) {
              declared.add(k);
            }
          }
        };
        collect(s, doc);
        for (const k of Object.keys(value as object)) {
          if (!declared.has(k)) {
            errors.push(`${at}.${k}: unevaluated property not allowed`);
          }
        }
      }
    }

    if (Array.isArray(s.oneOf)) {
      const passed = s.oneOf.filter(
        (branch) => validate(branch, value, at, doc).length === 0,
      );
      if (passed.length !== 1) {
        errors.push(
          `${at}: expected exactly one oneOf branch to match, got ${passed.length}`,
        );
      }
    }

    if (checkType("object", value) && s.properties) {
      const props = s.properties as Record<string, unknown>;
      for (const [key, propSchema] of Object.entries(props)) {
        if ((value as Record<string, unknown>)[key] !== undefined) {
          errors.push(
            ...validate(
              propSchema,
              (value as Record<string, unknown>)[key],
              `${at}.${key}`,
              doc,
            ),
          );
        }
      }
      if (s.required && Array.isArray(s.required)) {
        for (const key of s.required as string[]) {
          if ((value as Record<string, unknown>)[key] === undefined) {
            errors.push(`${at}: missing required property "${key}"`);
          }
        }
      }
      if (s.additionalProperties === false) {
        for (const key of Object.keys(value as object)) {
          if (!(key in props)) {
            errors.push(`${at}.${key}: additional property not allowed`);
          }
        }
      }
      // Standalone closure (no allOf — e.g. function declaration args):
      // same contract as additionalProperties:false, expressed with the
      // official unevaluatedProperties keyword.
      if (
        s.unevaluatedProperties === false &&
        !Array.isArray(s.allOf) &&
        s.properties
      ) {
        for (const key of Object.keys(value as object)) {
          if (!(key in props)) {
            errors.push(`${at}.${key}: unevaluated property not allowed`);
          }
        }
      }
    }

    if (Array.isArray(value) && s.items !== undefined) {
      for (const [i, item] of value.entries()) {
        errors.push(...validate(s.items, item, `${at}[${i}]`, doc));
      }
    }

    return errors;
  }

  return { validate };
}

/**
 * The structural declaration rules every component entry must satisfy
 * (mirrors the direct assertions the preset catalog is tested against):
 *
 * - closed object entry (`unevaluatedProperties: false`, `type: "object"`);
 * - composition via `allOf` with the official ComponentCommon ref first and
 *   the catalog-local CatalogComponentCommon ref second;
 * - an own properties branch carrying the const discriminator named after
 *   the component;
 * - structural slots (child/children/footer) as ComponentId / ChildList
 *   refs, never raw types;
 * - $refs anchored on the official common types or the hosting document
 *   itself (`#…`).
 *
 * Returns every violation found; an empty list means the entry is a legal
 * vocabulary member declaration.
 */
export function validateComponentEntry(name: string, entry: unknown): string[] {
  const at = `components.${name}`;
  if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
    return [`${at}: entry must be an object`];
  }
  const def = entry as Schema;
  const errors: string[] = [];

  if (def.type !== "object") {
    errors.push(`${at}: must declare type "object"`);
  }
  if (def.unevaluatedProperties !== false) {
    errors.push(`${at}: must close with unevaluatedProperties:false`);
  }
  if (!Array.isArray(def.allOf)) {
    errors.push(`${at}: must use allOf`);
    return errors;
  }
  const branches = def.allOf as Schema[];
  if (
    (branches[0] as Schema | undefined)?.$ref !==
    `${COMMON_TYPES_BASE}#/$defs/ComponentCommon`
  ) {
    errors.push(`${at}: first allOf branch must be ComponentCommon`);
  }
  if (
    (branches[1] as Schema | undefined)?.$ref !==
    "#/$defs/CatalogComponentCommon"
  ) {
    errors.push(`${at}: second allOf branch must be CatalogComponentCommon`);
  }

  const own = branches.filter((b) => !("$ref" in b)).find((b) => b.properties);
  if (!own) {
    errors.push(`${at}: must have own properties branch`);
    return errors;
  }
  const componentProp = (own.properties as Record<string, Schema>).component;
  if (componentProp?.const !== name) {
    errors.push(
      `${at}: component const discriminator mismatch (expected "${name}")`,
    );
  }

  const structuralProps = new Set(["child", "children", "footer"]);
  const checkStructuralSlots = (props: Record<string, Schema>): void => {
    for (const [propName, propSchema] of Object.entries(props)) {
      if (!structuralProps.has(propName)) continue;
      const ref = propSchema.$ref;
      if (typeof ref !== "string") {
        errors.push(`${at}.${propName}: must be a $ref, not a raw type`);
        continue;
      }
      if (
        !["ComponentId", "ChildList"].some((tail) =>
          ref.endsWith(`#/$defs/${tail}`),
        )
      ) {
        errors.push(
          `${at}.${propName}: $ref must target ComponentId or ChildList, got ${ref}`,
        );
      }
      if (propSchema.type !== undefined) {
        errors.push(`${at}.${propName}: must not declare a raw string type`);
      }
    }
  };
  for (const branch of branches) {
    const props = branch.properties as Record<string, Schema> | undefined;
    if (props) checkStructuralSlots(props);
  }
  // Structural slots on a top-level properties block (outside allOf) are
  // held to the same rules as the branch-level ones.
  if (def.properties) {
    checkStructuralSlots(def.properties as Record<string, Schema>);
  }

  for (const ref of collectRefs(def)) {
    const base = ref.split("#")[0];
    if (base !== COMMON_TYPES_BASE && base !== "") {
      errors.push(`${at}: unexpected $ref base in "${ref}"`);
    }
  }

  return errors;
}

function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((item, i) => deepEqual(item, b[i]));
  }
  if (a && b && typeof a === "object" && typeof b === "object") {
    const ka = Object.keys(a as object);
    const kb = Object.keys(b as object);
    return (
      ka.length === kb.length &&
      ka.every(
        (k) =>
          deepEqual(
            (a as Record<string, unknown>)[k],
            (b as Record<string, unknown>)[k],
          ) && k in (b as object),
      )
    );
  }
  return false;
}

function checkType(expected: string, value: unknown): boolean {
  switch (expected) {
    case "object":
      return (
        typeof value === "object" && value !== null && !Array.isArray(value)
      );
    case "array":
      return Array.isArray(value);
    case "string":
      return typeof value === "string";
    case "number":
      return typeof value === "number";
    case "integer":
      return typeof value === "number" && Number.isInteger(value);
    case "boolean":
      return typeof value === "boolean";
    case "null":
      return value === null;
    default:
      return false;
  }
}
