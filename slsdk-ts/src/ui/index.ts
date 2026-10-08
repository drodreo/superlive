/**
 * @shell/sdk/ui entry: the shared UI facility (the "@shell/sdk dual entry" section in architecture.md).
 *
 * Contents:
 * - Component facility: 15 components fully exported via the components barrel (Button/Select/Checkbox +
 *   the list·form·dialog·state families) + CrudListPage + the query component family
 *   (the four QueryInputs / QueryConditionRow / QueryDrawer / QueryListToolbar)
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
export * from './components';
export type {
  TextInputProps,
  NumberInputProps,
  DateTimePickerProps,
  RangePickerProps,
} from './components/query/QueryInputs';
export {
  TextInput,
  NumberInput,
  DateTimePicker,
  RangePicker,
} from './components/query/QueryInputs';
export {
  QueryConditionRow,
  type QueryConditionRowProps,
} from './components/query/QueryConditionRow';
export { QueryDrawer, type QueryDrawerProps } from './components/query/QueryDrawer';
export {
  QueryListToolbar,
  type QueryListToolbarProps,
  type QueryListToolbarFilter,
} from './components/query/QueryListToolbar';
export { CrudListPage } from './pages/CrudListPage';
export type { CrudListPageProps, FieldValue } from './pages/CrudListPage';

// ===== hooks =====
export {
  createEntityHook,
  DEFAULT_PAGE_SIZE,
  type EntityApi,
  type CreateEntityHookConfig,
  type UseEntityResult,
} from './hooks/createEntityHook';
export {
  useReferenceOptions,
  useReferenceOptionById,
  type UseReferenceOptionsResult,
  type UseReferenceOptionByIdResult,
} from './hooks/useReferenceOptions';
export { useShellState, type ShellEnv, type ShellState } from './hooks/useShellState';

// ===== utils =====
export { formatDateTime } from './utils/datetime';

// ===== CSS contract facility =====
export { injectUiCss, removeUiCss, useUiCss } from './inject';

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
} from './types';
