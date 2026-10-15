//! Shared serve-surface types (kept separate from serve.ts so the node adapter can import
//! them without a circular dependency).

import type { Endpoint } from "./endpoint";

/** Return value of `serve()`: the actual listen descriptor + exit handles. The service
 * keeps running after `serve()` resolves — SIGTERM/SIGINT (or an explicit `shutdown()`)
 * exit as the fallback, mirroring the slsdk-rs ServeInfo experience. */
export interface ServeInfo {
  /** Actual listen descriptor (`tcp:127.0.0.1:0` expands to the real assigned port; the
   * uds path is read back from the listener where the runtime exposes it). */
  endpoint: Endpoint;
  /** Actual TCP port (present only for tcp descriptors; useful when port 0 was requested). */
  port?: number;
  /** Triggers graceful exit and resolves once the service has fully exited
   * (listening stopped, in-flight requests drained). Idempotent. */
  shutdown(): Promise<void>;
  /** Resolves once the service has fully exited. Idempotent, callable any number of times. */
  wait(): Promise<void>;
}
