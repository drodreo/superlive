/**
 * Reactive data model for one A2UI surface: a plain object tree addressed by
 * JSON Pointer (RFC 6901) with path-level subscriptions — mirrors the official
 * web_core DataModel (a2ui-project/a2ui, state/data-model.ts) at subset scale.
 *
 * Defensive limits follow the official model: auto-vivification is capped at
 * MAX_ARRAY_INDEX and the prototype-pollution key set is rejected outright.
 *
 * Keystroke contract (specification layer): controlled components write user
 * input straight into this model via `set` — the value flows back down through
 * bindings. Nothing leaves the client on the way.
 */
import { pathAffects, SignalHub, type Unsubscribe } from "./signals";

/** Auto-vivification cap for numeric pointer segments (official parity). */
export const MAX_ARRAY_INDEX = 10000;

/** Keys that must never be created on the model tree (prototype pollution). */
const FORBIDDEN_KEYS = new Set(["__proto__", "constructor", "prototype"]);

/** Thrown for malformed pointers / index overflow / forbidden keys. */
export class DataModelError extends Error {
  constructor(
    message: string,
    readonly pointer: string,
  ) {
    super(message);
    this.name = "DataModelError";
  }
}

/** Parse a JSON Pointer into unescaped tokens; "" addresses the whole doc. */
function parseTokens(pointer: string): string[] {
  if (pointer === "") return [];
  if (!pointer.startsWith("/")) {
    throw new DataModelError(
      `JSON Pointer must be empty or start with "/": got "${pointer}"`,
      pointer,
    );
  }
  return pointer
    .slice(1)
    .split("/")
    .map((t) => t.replaceAll("~1", "/").replaceAll("~0", "~"));
}

function isForbiddenKey(token: string): boolean {
  return FORBIDDEN_KEYS.has(token);
}

/** Numeric segment within the auto-vivification cap (array index candidate). */
function arrayIndex(token: string): number | null {
  if (!/^(0|[1-9]\d*)$/.test(token)) return null;
  const n = Number(token);
  if (n > MAX_ARRAY_INDEX) {
    throw new DataModelError(
      `array index ${n} exceeds MAX_ARRAY_INDEX (${MAX_ARRAY_INDEX})`,
      `/${token}`,
    );
  }
  return n;
}

/** Assign `value` at the token path, assuming ancestors are containers. */
function assignAt(root: Container, tokens: string[], value: unknown): void {
  let node: unknown = root;
  for (let i = 0; i < tokens.length - 1; i++) {
    node = (node as Container)[tokens[i]];
  }
  (node as Container)[tokens[tokens.length - 1]] = value;
}

type Container = Record<string, unknown>;

export class DataModel {
  private root: Container = {};
  private hub = new SignalHub();
  private rev = 0;

  /** Monotonic change counter (React tree-level store snapshot). */
  get version(): number {
    return this.rev;
  }

  /** Read the value at `pointer`; missing paths yield undefined. */
  get(pointer: string): unknown {
    let node: unknown = this.root;
    for (const token of parseTokens(pointer)) {
      if (node === null || typeof node !== "object") return undefined;
      node = Array.isArray(node)
        ? (node as unknown[])[arrayIndex(token) ?? -1]
        : (node as Container)[token];
    }
    return node;
  }

  /**
   * Set `value` at `pointer`, creating intermediate containers as needed
   * (array when the next token is numeric, object otherwise; a scalar or
   * array sitting in the way is replaced). Fires notifications for every
   * created container and the target path, in escaped pointer form.
   */
  set(pointer: string, value: unknown): void {
    const tokens = parseTokens(pointer);
    // Raw (escaped) segments of the pointer, used to build notification paths
    // that byte-match what subscribers passed to subscribe().
    const rawTokens = pointer === "" ? [] : pointer.slice(1).split("/");
    sanitizeValue(value, pointer);

    if (tokens.length === 0) {
      this.root =
        value === null || typeof value !== "object" ? {} : (value as Container);
      this.commit(["/"]);
      return;
    }

    let node: unknown = this.root;
    const changed: string[] = [];
    for (let i = 0; i < tokens.length - 1; i++) {
      const token = tokens[i];
      if (isForbiddenKey(token)) {
        throw new DataModelError(`forbidden key "${token}"`, pointer);
      }
      const walked = `/${rawTokens.slice(0, i + 1).join("/")}`;
      const idx = arrayIndex(token);

      if (node === null || typeof node !== "object") {
        // Scalar in the way: replace with a fresh container.
        const fresh = arrayIndex(tokens[i + 1]) !== null ? [] : {};
        assignAt(this.root, tokens.slice(0, i + 1), fresh);
        changed.push(walked);
        node = fresh;
        continue;
      }

      const current = Array.isArray(node)
        ? idx !== null
          ? (node as unknown[])[idx]
          : undefined
        : (node as Container)[token];

      if (
        current === undefined ||
        current === null ||
        typeof current !== "object"
      ) {
        const fresh = arrayIndex(tokens[i + 1]) !== null ? [] : {};
        if (Array.isArray(node)) {
          if (idx === null) {
            throw new DataModelError(
              `cannot use non-numeric key "${token}" inside an array`,
              pointer,
            );
          }
          (node as unknown[])[idx] = fresh;
        } else {
          (node as Container)[token] = fresh;
        }
        changed.push(walked);
        node = fresh;
      } else {
        node = current;
      }
    }

    const last = tokens[tokens.length - 1];
    if (isForbiddenKey(last)) {
      throw new DataModelError(`forbidden key "${last}"`, pointer);
    }
    const parent = node as Container;
    const idx = arrayIndex(last);
    if (Array.isArray(parent)) {
      if (idx === null) {
        throw new DataModelError(
          `cannot use non-numeric key "${last}" inside an array`,
          pointer,
        );
      }
      (parent as unknown[])[idx] = value;
    } else {
      parent[last] = value;
    }
    changed.push(pointer);
    this.commit(changed);
  }

  /**
   * Remove the value at `pointer` (protocol semantics: updateDataModel with
   * the value omitted). Array elements are spliced per RFC 6901 `remove`.
   */
  delete(pointer: string): void {
    const tokens = parseTokens(pointer);
    if (tokens.length === 0) {
      this.root = {};
      this.commit(["/"]);
      return;
    }
    let node: unknown = this.root;
    for (const token of tokens.slice(0, -1)) {
      if (node === null || typeof node !== "object") return; // nothing to remove
      node = Array.isArray(node)
        ? (node as unknown[])[arrayIndex(token) ?? -1]
        : (node as Container)[token];
    }
    if (node === null || typeof node !== "object") return;
    const parent = node as Container;
    const last = tokens[tokens.length - 1];
    const idx = arrayIndex(last);
    if (Array.isArray(parent)) {
      if (idx !== null && idx < parent.length)
        (parent as unknown[]).splice(idx, 1);
    } else {
      delete parent[last];
    }
    this.commit([pointer]);
  }

  /** Subscribe to changes affecting `pointer` (either-direction prefix). */
  subscribe(pointer: string, cb: () => void): Unsubscribe {
    return this.hub.subscribe((changed) => pathAffects(pointer, changed), cb);
  }

  /** Subscribe to every change (React tree-level notification). */
  subscribeAll(cb: () => void): Unsubscribe {
    return this.hub.subscribe(() => true, cb);
  }

  private commit(changedPaths: string[]): void {
    this.rev++;
    this.hub.emit(changedPaths);
  }

  /** Drop all subscriptions (surface dispose). */
  clearSubscriptions(): void {
    this.hub.clear();
  }
}

/**
 * Reject wire-authored payloads carrying prototype-pollution keys anywhere in
 * the tree (updateDataModel values come from the agent side).
 */
function sanitizeValue(value: unknown, pointer: string): void {
  if (value === null || typeof value !== "object") return;
  for (const [key, child] of Object.entries(value)) {
    if (isForbiddenKey(key)) {
      throw new DataModelError(`forbidden key "${key}" in payload`, pointer);
    }
    sanitizeValue(child, pointer);
  }
}
