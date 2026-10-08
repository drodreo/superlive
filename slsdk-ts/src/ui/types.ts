/**
 * Small types for the shared UI facility (@shell/sdk/ui).
 */

/** Dropdown option (Select / FormField select tier) */
export interface SelectOption {
  value: string;
  label: string;
}

// ===== Common response shapes (protocol types between component service layers and createEntityHook) =====

/** Per-item result of a batch write (the wire contract for add/update/delete) */
export interface BatchResult<T> {
  ok: boolean;
  data?: T;
  error?: string;
}

/** List query response (the search route): total is always present */
export interface SearchResponse<T> {
  data: T[];
  skip?: number;
  limit?: number;
  total: number;
}

/** Standard delete input */
export interface IdInput {
  id: string;
}

// ===== reference-tier injection table (the "ReferenceEntity injection table" contract in architecture.md) =====

/** reference-tier dropdown option: value is the submitted value, label is the display copy */
export interface ReferenceOption {
  value: string;
  label: string;
}

/**
 * reference-tier data source (injected by the consumer; the facility stays unaware of concrete entities).
 * - `search(keyword)`: keyword search returning one page of options (debounce/race protection is built into the shared hook;
 *   paging and filtering semantics are owned by the spec implementation)
 * - `byId(id)` (optional): resolves a single id into an option (used to echo the current value in edit forms;
 *   when absent, the facility skips echo resolution and the dropdown only shows options matches)
 */
export interface ReferenceSpec {
  search(keyword: string): import('rxjs').Observable<ReferenceOption[]>;
  byId?(id: string): import('rxjs').Observable<ReferenceOption | undefined>;
}

// ===== file-tier probe injection (the file-tier backfill contract in architecture.md) =====

/** Probe result (the domain file_meta API shape: failed fields return null; all three null = unrecognized) */
export interface FileProbeResult {
  size: number | null;
  mime: string | null;
  fileName: string | null;
}

/**
 * file-tier probe function (injected by the consumer — probing requires a domain API, e.g. sl-writing materials'
 * getFileMeta). When fileProbe is not injected, CrudListPage's file tier degrades to a plain
 * input (no probe button rendered).
 */
export type FileProbeFn = (path: string) => Promise<FileProbeResult>;
