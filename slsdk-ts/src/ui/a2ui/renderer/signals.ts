/**
 * Minimal signal hub for the A2UI renderer (~30 lines, same idea as the
 * @preact/signals-core reactivity the official A2UI web_core relies on).
 *
 * Listeners are (matcher, callback) pairs; `emit` fires every callback whose
 * matcher hits at least one of the changed paths. No scheduling, no batching —
 * callers (DataModel / SurfaceModel) guarantee one emit per mutation batch.
 */

export type Unsubscribe = () => void;

export interface SignalListener {
  match: (changedPath: string) => boolean;
  cb: () => void;
}

export class SignalHub {
  private listeners = new Set<SignalListener>();

  subscribe(
    match: (changedPath: string) => boolean,
    cb: () => void,
  ): Unsubscribe {
    const listener: SignalListener = { match, cb };
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  emit(changedPaths: readonly string[]): void {
    if (changedPaths.length === 0) return;
    // Iterate over a copy: callbacks may subscribe/unsubscribe reentrantly.
    for (const listener of [...this.listeners]) {
      if (changedPaths.some((p) => listener.match(p))) listener.cb();
    }
  }

  clear(): void {
    this.listeners.clear();
  }
}

/**
 * Whether a change at `changed` should wake a listener subscribed to
 * `subscribed`: either path being a prefix of the other counts (a listener on
 * `/items` sees `/items/0` created; a listener on `/items/0/name` sees the
 * whole `/items` subtree replaced). JSON Pointer segments are "/"-separated,
 * so prefix matching must respect the segment boundary. A whole-document
 * change affects every subscription: "" (RFC 6901 whole-document pointer) and
 * "/" (the spec's equivalent form — updateDataModel treats an omitted path
 * and "/" alike as the entire model) both replace the entire tree.
 */
export function pathAffects(subscribed: string, changed: string): boolean {
  if (changed === "" || changed === "/") return true;
  return (
    changed === subscribed ||
    changed.startsWith(subscribed + "/") ||
    subscribed.startsWith(changed + "/")
  );
}
