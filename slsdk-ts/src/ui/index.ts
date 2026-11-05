/**
 * @shell/sdk/ui entry: the shared UI facility (the "@shell/sdk dual entry" section in architecture.md).
 *
 * Contents:
 * - Component facility: 16 components fully exported via the components barrel (Button/Select/Checkbox +
 *   the list·form·dialog·state families) + CrudListPage + the query component family
 *   (the four QueryInputs / QueryConditionRow / QueryDrawer / QueryListToolbar)
 * - A2UI renderer public face: description-JSON-driven surface rendering on the preset
 *   vocabulary — hosts mount <A2uiSurface> and consume action payloads; component authors
 *   take their own preset copy via createPresetRegistry(), wire their implementations
 *   with the bind* primitives and contribute custom controls via registerControl
 *   (immutable, collision- and declaration-rule-checked), folding the contributions into
 *   a distributable vocabulary with mergeCatalog; the catalog's TS companion types ride
 *   along for programmatic description construction (the machinery — surface/data
 *   model/binding — stays internal)
 * - hooks (createEntityHook / useReferenceOptions / useShellState)
 * - utils (datetime)
 * - CSS contract facility (injectUiCss / removeUiCss / useUiCss — components own their styles,
 *   zero concern for consumers; the customization channel is CSS cascade overlay, domain-side overrides need no !important)
 * - types (the reference-tier injection table ReferenceSpec + the file-tier probe injection FileProbeFn)
 *
 * Shell adaptations component authors should never worry about (useShellState/token/CSS scoping) and the
 * official shapes of the domain framework layer all live here.
 */

// ===== Component facility =====
export * from "./components";
export type {
  TextInputProps,
  NumberInputProps,
  DateTimePickerProps,
  RangePickerProps,
} from "./components/query/QueryInputs";
export {
  TextInput,
  NumberInput,
  DateTimePicker,
  RangePicker,
} from "./components/query/QueryInputs";
export {
  QueryConditionRow,
  type QueryConditionRowProps,
} from "./components/query/QueryConditionRow";
export {
  QueryDrawer,
  type QueryDrawerProps,
} from "./components/query/QueryDrawer";
export {
  QueryListToolbar,
  type QueryListToolbarProps,
  type QueryListToolbarFilter,
} from "./components/query/QueryListToolbar";
// The query model types the toolbar family consumes — component authors
// construct `querySchema` props with them (they are internal to the query
// component family; the ui entry owns them now).
export type {
  QueryCondition,
  QueryFieldSpec,
  QueryOperator,
  QueryPagination,
  QuerySchema,
  QuerySorting,
  QueryValue,
} from "./query-model";
export { CrudListPage } from "./pages/CrudListPage";
export type { CrudListPageProps, FieldValue } from "./pages/CrudListPage";

// ===== A2UI renderer public face =====
export {
  // Host mounting face: description in → rendered tree and actions out.
  A2uiSurface,
  // Component-author face: a fresh preset-copy registry to contribute custom
  // controls into (registerControl — immutable, collision- and rule-checked),
  // the catalog merge that folds the contributions into a distributable
  // vocabulary document, and the two binding primitives the implementations
  // glue their props/events with.
  createPresetRegistry,
  bindAction,
  bindControlled,
  type A2uiSurfaceProps,
  A2uiRegistry,
  type A2uiComponentImpl,
  type ComponentRenderContext,
  // Protocol payload types (action/error consumer side + description envelope).
  type ActionMessagePayload,
  type ErrorMessagePayload,
  type ServerToClientMessage,
  // Host function implementations for {functionCall} actions (the HTTP
  // action family lives host-side; predicates are renderer builtins).
  type FunctionTable,
  // The live surface handle hosts receive through <A2uiSurface surfaceRef>:
  // incremental ServerToClientMessage delivery without description rebuilds.
  SurfaceModel,
} from "./a2ui/renderer";
// Push subscription factory (the ws observe channel): hosts subscribe to
// their component's /subscriptions/{ns}/surface feed and apply the arrived
// envelopes into the live SurfaceModel.
export {
  createObserve,
  type ObserveOptions,
  type RetryOptions,
} from "../observe";
export { mergeCatalog, type MergedCatalogDocument } from "./catalog/merge";
// Description companion types (the catalog JSON Schema's TS ease-of-use layer).
export type {
  CatalogComponentName,
  CatalogComponentDefinition,
  SuperliveCatalog,
} from "./catalog/types";

// ===== hooks =====
export {
  createEntityHook,
  DEFAULT_PAGE_SIZE,
  type EntityApi,
  type CreateEntityHookConfig,
  type UseEntityResult,
} from "./hooks/createEntityHook";
export {
  useReferenceOptions,
  useReferenceOptionById,
  type UseReferenceOptionsResult,
  type UseReferenceOptionByIdResult,
} from "./hooks/useReferenceOptions";
export {
  useShellState,
  type ShellEnv,
  type ShellState,
} from "./hooks/useShellState";

// ===== utils =====
export { formatDateTime } from "./utils/datetime";

// ===== CSS contract facility =====
export { injectUiCss, removeUiCss, useUiCss } from "./inject";

// ===== Component i18n registration =====
export { registerI18n, type ComponentI18nResources } from "./i18n";

// ===== Types =====
export type {
  SelectOption,
  BatchResult,
  SearchResponse,
  IdInput,
  ReferenceOption,
  ReferenceSpec,
  FileProbeResult,
  FileProbeFn,
} from "./types";
