/**
 * A2UI renderer public face: "description JSON in → rendered tree and actions
 * out". Hosts mount <A2uiSurface>; component authors contribute
 * implementations through the open registry contract (registerControl on
 * their own registry instance) — the core never changes for new controls.
 */
export { A2uiSurface, type A2uiSurfaceProps } from "./react/A2uiSurface";
export {
  A2uiRegistry,
  bindAction,
  bindControlled,
  type A2uiComponentImpl,
  type ComponentRenderContext,
} from "./react/wiring";
export { createPresetRegistry, defaultRegistry } from "./registry";
export { DataModel, DataModelError, MAX_ARRAY_INDEX } from "./data-model";
export type {
  ActionMessagePayload,
  ErrorMessagePayload,
  ServerToClientMessage,
} from "../types";
export {
  ComponentBinding,
  inspectComponentSchema,
  resolveDynamic,
  resolveScopePath,
  type ComponentSpec,
  type FunctionTable,
  type PropSpec,
} from "./binding";
export {
  SurfaceModel,
  deepEqual,
  type CatalogDocument,
  type SurfaceNode,
  type SurfaceOptions,
} from "./surface";
