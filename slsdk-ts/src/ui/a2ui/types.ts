// A2UI v0.9.1 protocol types — the subset consumed by superlive.
//
// Alignment baseline: A2UI specification v0.9.1 snapshot (protocol.md envelope
// structure + common_types.json $defs). Each exported type carries the spec
// definition name it mirrors. Definitions not consumed by superlive
// (DynamicStringList / Checkable / CheckRule / AccessibilityAttributes) are
// intentionally absent — the component surface itself is owned by the catalog
// JSON Schema (../catalog/superlive.catalog.json).

// ===== common types (common_types.json $defs) =====

/** common_types.json $defs/ComponentId — component reference within one surface */
export type ComponentId = string;

/** common_types.json $defs/DataBinding — JSON Pointer (RFC 6901) into the surface data model */
export interface DataBinding {
  path: string;
}

/** common_types.json $defs/FunctionCall — invokes a named function on the client */
export interface FunctionCall {
  call: string;
  args?: Record<string, unknown>;
  returnType?:
    "string" | "number" | "boolean" | "array" | "object" | "any" | "void";
}

/** common_types.json $defs/DynamicString — literal | data binding | string-returning function call */
export type DynamicString =
  string | DataBinding | (FunctionCall & { returnType: "string" });

/** common_types.json $defs/DynamicNumber — literal | data binding | number-returning function call */
export type DynamicNumber =
  number | DataBinding | (FunctionCall & { returnType: "number" });

/** common_types.json $defs/DynamicBoolean — literal | data binding | boolean-returning function call */
export type DynamicBoolean =
  boolean | DataBinding | (FunctionCall & { returnType: "boolean" });

/** common_types.json $defs/DynamicValue — generic bindable value (Action.context entries) */
export type DynamicValue =
  string | number | boolean | unknown[] | DataBinding | FunctionCall;

/** common_types.json $defs/ChildList — static id list | template iteration over a data model list */
export type ChildList =
  ComponentId[] | { componentId: ComponentId; path: string };

/** common_types.json $defs/Action — server-side event | local client-side function call */
export type Action =
  | { event: { name: string; context?: Record<string, DynamicValue> } }
  | { functionCall: FunctionCall };

// ===== server → client messages (protocol.md "Envelope message structure") =====

/** protocol.md createSurface — surfaceId/catalogId are immutable after creation */
export interface CreateSurfaceMessage {
  surfaceId: string;
  catalogId: string;
  theme?: Record<string, unknown>;
  sendDataModel?: boolean;
}

/** protocol.md updateComponents — flat adjacency list, parent-child via id references */
export interface UpdateComponentsMessage {
  surfaceId: string;
  components: A2uiComponentInstance[];
}

/**
 * A component instance as produced by the agent. The concrete per-component
 * shape is owned by the catalog JSON Schema (src/ui/catalog/superlive.catalog.json);
 * this wide type only carries the protocol-level invariants (id + component name).
 * One component of the tree must use id "root".
 */
export interface A2uiComponentInstance {
  id: ComponentId;
  component: string;
  [key: string]: unknown;
}

/** protocol.md updateDataModel — replaces the value at path; path omitted = whole model */
export interface UpdateDataModelMessage {
  surfaceId: string;
  /** JSON Pointer into the surface data model; defaults to "/" */
  path?: string;
  /** Omitted = the key at path is removed */
  value?: unknown;
}

/** protocol.md deleteSurface */
export interface DeleteSurfaceMessage {
  surfaceId: string;
}

/** Server → client envelope: version + exactly one message key */
export type ServerToClientMessage =
  | { version: "v0.9.1"; createSurface: CreateSurfaceMessage }
  | { version: "v0.9.1"; updateComponents: UpdateComponentsMessage }
  | { version: "v0.9.1"; updateDataModel: UpdateDataModelMessage }
  | { version: "v0.9.1"; deleteSurface: DeleteSurfaceMessage };

// ===== client → server messages (actions.md interaction flow / error reporting) =====

/** actions.md "Example: The Action Payload" — user interaction dispatched to the agent */
export interface ActionMessagePayload {
  name: string;
  surfaceId: string;
  sourceComponentId?: ComponentId;
  timestamp?: string;
  context?: Record<string, DynamicValue>;
}

/** actions.md "Renderer-to-Agent Error Reporting" — e.g. code "VALIDATION_FAILED" */
export interface ErrorMessagePayload {
  code: string;
  surfaceId?: string;
  path?: string;
  message: string;
}

/** Client → server envelope: version + exactly one message key */
export type ClientToServerMessage =
  | { version: "v0.9.1"; action: ActionMessagePayload }
  | { version: "v0.9.1"; error: ErrorMessagePayload };
