/**
 * CrudListPage - generic CRUD list-page framework (the superlive v13 consolidated edition, facility-ized).
 *
 * Facility consolidation checklist (relative to superlive pages/_shared/CrudListPage.tsx and the
 * componentization-era private edition):
 * - i18n removed: all copy hardcoded in Chinese (v1 of the component is monolingual)
 * - host facilities removed: the tauri file dialog (plugin-dialog/invoke), admin getConfig,
 *   and the reference global registry — all unreachable in the component runtime
 * - renderFieldInput supports eight tiers: textarea / input / select (static options) /
 *   checkbox / reference (references injection table) / directory (hand-typed path) / datetime
 *   (native datetime-local control — react-datepicker is not on the dependency whitelist, degrading to the native
 *   control with values round-tripping as ISO strings, the same zero-new-dependency shape as QueryInputs.DateTimePicker) /
 *   file (hand-typed path + fileProbe probe backfill — the probe function is injected by the consumer (domain APIs cannot
 *   enter the facility), autoFill declares the backfill keys; without fileProbe it degrades to a plain input)
 * - added hideDefaultDetailButton / hideDefaultEditButton (semantics aligned with the original)
 * - kept: the client-side sort/search fallback mode (still usable when queryToolbar is omitted), form validation,
 *   delete confirmation, the detail dialog, and pagination pass-through
 * - reference-tier injection table (architecture.md "ReferenceEntity injection table"):
 *   the `references?: Record<string, ReferenceSpec>` prop passes down to the form's reference
 *   tier and the query drawer; a missing spec warns in dev mode (built into useReferenceOptions)
 */
import { useState, useCallback, useMemo, type ReactNode } from 'react';
import {
  DataTable,
  Pagination,
  SearchBar,
  Input,
  Textarea,
  Select,
  Checkbox,
  FormField,
  Modal,
  ConfirmDialog,
  FormDialog,
  LoadingSpinner,
  EmptyState,
  ErrorMessage,
  type DataTableColumn,
  type FormSchema,
} from '../components';
import type { IdInput, ReferenceSpec, FileProbeFn } from '../types';
import type { FileAutoFill } from '../components/FormField';
import { useReferenceOptions, useReferenceOptionById } from '../hooks/useReferenceOptions';
import { Button } from '../components/Button';
import { DEFAULT_PAGE_SIZE } from '../hooks/createEntityHook';

/** Form value carrier (string / string[] / boolean / undefined; the checkbox tier round-trips as 'true'/'false') */
export type FieldValue = string | string[] | boolean | undefined;

function buildFormDefaults(schema: FormSchema): Record<string, FieldValue> {
  const defaults: Record<string, FieldValue> = {};
  for (const f of schema.fields) {
    defaults[f.key] = f.type === 'checkbox' ? false : '';
  }
  return defaults;
}

/** ISO 8601 → datetime-local control value (local timezone, minute precision); empty/invalid returns the empty string */
function toLocalInputValue(iso: string | undefined): string {
  if (!iso) return '';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** datetime-local control value → ISO 8601; empty/invalid returns the empty string */
function fromLocalInputValue(local: string): string {
  if (!local) return '';
  const d = new Date(local);
  return Number.isNaN(d.getTime()) ? '' : d.toISOString();
}

/** Friendly formatting for detail values (boolean / DateTime / array / null) */
function formatDetailValue(v: unknown): string {
  if (v === null || v === undefined) return '-';
  if (typeof v === 'boolean') return v ? '✓ 是' : '✗ 否';
  if (typeof v === 'string') {
    if (v.length === 0) return '-';
    // ISO 8601 DateTime → localized time
    if (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/.test(v)) {
      const d = new Date(v);
      if (!Number.isNaN(d.getTime())) {
        return d.toLocaleString('zh-CN', { hour12: false });
      }
    }
    return v;
  }
  if (Array.isArray(v)) return v.length === 0 ? '-' : `${v.length} 项`;
  return String(v);
}

/** Inline SVG icons (lucide-react is not on the component whitelist) */
function PlusIcon() {
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round">
      <line x1="12" y1="5" x2="12" y2="19" />
      <line x1="5" y1="12" x2="19" y2="12" />
    </svg>
  );
}

function TrashIcon() {
  return (
    <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <polyline points="3 6 5 6 21 6" />
      <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
    </svg>
  );
}

export interface CrudListPageProps<T extends { id?: string | null }> {
  /** Display name (e.g. "Market") */
  displayName: string;
  /** List page column definitions */
  columns: DataTableColumn[];
  /** Form schema */
  formSchema: FormSchema;
  /** Business object → form values */
  toFormValue: (row: T) => Record<string, FieldValue>;
  /** Form values → business object */
  fromFormValue: (value: Record<string, FieldValue>, defaults?: Partial<T>) => Partial<T>;
  /** Data loading state */
  loading: boolean;
  /** Error message */
  error: string | null;
  /** Data list */
  items: T[];
  /** The total count returned by the backend (falls back to items.length when omitted) */
  total?: number;
  /** Refresh the data */
  refresh: () => void;
  /** Add */
  add: (items: T[]) => Promise<unknown[]>;
  /** Update */
  update: (items: T[]) => Promise<unknown[]>;
  /** Delete */
  remove: (items: IdInput[]) => Promise<unknown[]>;
  /** Row display name (used in delete confirmation and similar) */
  getDisplayName: (row: T) => string;
  /** Row ID */
  getId: (row: T) => string | null | undefined;
  /** Read-only mode (hides the add/edit/delete buttons) */
  readOnly?: boolean;
  /** Hide the built-in detail button (the caller takes over the detail entry via extraRowActions) */
  hideDefaultDetailButton?: boolean;
  /** Hide the built-in edit button (the caller's detail drawer takes over the edit entry) */
  hideDefaultEditButton?: boolean;
  /** The list-page toolbar slot for search/sort/drawer/pagination (the v13 abstraction) */
  queryToolbar?: ReactNode;
  /** Current page (1-indexed); in queryToolbar mode the caller passes it from hook.page */
  page?: number;
  /** Page-change callback */
  onPageChange?: (page: number) => void;
  /** Extra row actions (the returned ReactNode is appended after the row actions) */
  extraRowActions?: (row: T) => ReactNode;
  /** reference-tier injection table (key = schema.reference.entity / the querySchema reference keys) */
  references?: Record<string, ReferenceSpec>;
  /** file-tier probe function (an injected domain API, e.g. sl-writing materials getFileMeta;
   *  without it the file tier degrades to a plain input, no probe button rendered) */
  fileProbe?: FileProbeFn;
}

export function CrudListPage<T extends { id?: string | null }>({
  displayName,
  columns,
  formSchema,
  toFormValue,
  fromFormValue,
  loading,
  error,
  items,
  total: totalProp,
  refresh,
  add,
  update,
  remove,
  getDisplayName,
  getId,
  readOnly = false,
  hideDefaultDetailButton = false,
  hideDefaultEditButton = false,
  queryToolbar,
  extraRowActions,
  page: pageProp,
  onPageChange,
  references,
  fileProbe,
}: CrudListPageProps<T>) {
  // ===== Client-side UI state (fallback mode when queryToolbar is omitted) =====
  const [fallbackPage, setFallbackPage] = useState(1);
  const page = pageProp ?? fallbackPage;
  const [search, setSearch] = useState('');
  const [sortKey, setSortKey] = useState('');
  const [sortDir, setSortDir] = useState<'asc' | 'desc'>('asc');

  // ===== Form / delete / detail state =====
  const [formOpen, setFormOpen] = useState(false);
  const [formLoading, setFormLoading] = useState(false);
  const [editingRow, setEditingRow] = useState<T | null>(null);
  const [formValues, setFormValues] = useState<Record<string, FieldValue>>(buildFormDefaults(formSchema));
  const [formErrors, setFormErrors] = useState<Record<string, string>>({});

  const [deleteTarget, setDeleteTarget] = useState<{ id: string; name: string } | null>(null);
  const [deleteLoading, setDeleteLoading] = useState(false);
  const [detailRow, setDetailRow] = useState<T | null>(null);

  const total = totalProp ?? items.length;

  // ===== Client-side sorting / search (fallback mode) =====
  const handleSort = useCallback((key: string) => {
    if (sortKey === key) {
      setSortDir((d) => (d === 'asc' ? 'desc' : 'asc'));
    } else {
      setSortKey(key);
      setSortDir('asc');
    }
  }, [sortKey]);

  const handleSearch = useCallback((value: string) => {
    setSearch(value);
    setFallbackPage(1);
  }, []);

  // ===== Form / delete / detail callbacks =====
  const openAddForm = useCallback(() => {
    setEditingRow(null);
    setFormValues(buildFormDefaults(formSchema));
    setFormErrors({});
    setFormOpen(true);
  }, [formSchema]);

  const openEditForm = useCallback((row: T) => {
    setEditingRow(row);
    setFormValues(toFormValue(row));
    setFormErrors({});
    setFormOpen(true);
  }, [toFormValue]);

  const closeForm = useCallback(() => {
    setFormOpen(false);
    setEditingRow(null);
    setFormValues({});
    setFormErrors({});
  }, []);

  const validateForm = useCallback((): boolean => {
    const errors: Record<string, string> = {};
    for (const f of formSchema.fields) {
      if (f.required && !formValues[f.key]) {
        errors[f.key] = '该项为必填项';
      }
    }
    setFormErrors(errors);
    return Object.keys(errors).length === 0;
  }, [formSchema.fields, formValues]);

  const handleFormSubmit = useCallback(async () => {
    if (!validateForm()) return;
    setFormLoading(true);
    try {
      const data = fromFormValue(formValues, editingRow ?? undefined);
      if (editingRow?.id) {
        // Merge the id into the PUT body; the business object's validate() requires the id to be Some
        await update([{ ...(data as T), id: editingRow.id } as T]);
      } else {
        await add([data as T]);
      }
      closeForm();
      refresh();
    } catch (e: unknown) {
      const err = e as { detail?: string; message?: string };
      setFormErrors({ _form: err?.detail ?? err?.message ?? '提交失败' });
    } finally {
      setFormLoading(false);
    }
  }, [validateForm, formValues, editingRow, fromFormValue, update, add, closeForm, refresh]);

  const confirmDelete = useCallback((row: T) => {
    const id = getId(row) ?? '';
    const name = getDisplayName(row) || id;
    setDeleteTarget({ id: String(id), name });
  }, [getId, getDisplayName]);

  const handleDelete = useCallback(async () => {
    if (!deleteTarget) return;
    setDeleteLoading(true);
    try {
      await remove([{ id: deleteTarget.id }]);
      setDeleteTarget(null);
      refresh();
    } finally {
      setDeleteLoading(false);
    }
  }, [deleteTarget, remove, refresh]);

  const renderFieldInput = useCallback((f: FormSchema['fields'][number]) => {
    const value = formValues[f.key] ?? '';
    const setField = (v: string) => setFormValues((s) => ({ ...s, [f.key]: v }));

    if (f.type === 'textarea') {
      return (
        <Textarea
          value={String(value)}
          onChange={setField}
          placeholder={f.placeholder}
          disabled={f.disabled}
        />
      );
    }
    if (f.type === 'select') {
      return (
        <Select
          value={value ? String(value) : null}
          onChange={(v) => setField(v ?? '')}
          options={f.options ?? []}
          placeholder={f.placeholder ?? '请选择'}
          disabled={f.disabled}
        />
      );
    }
    if (f.type === 'checkbox') {
      // Form value carrier: the hooks converters round-trip as string 'true'/'false' (original shape),
      // the render check compares explicitly to avoid Boolean('false') evaluating true
      const checked = value === true || value === 'true';
      return (
        <Checkbox
          checked={checked}
          onChange={(v) => setFormValues((s) => ({ ...s, [f.key]: v }))}
          disabled={f.disabled}
        />
      );
    }
    if (f.type === 'reference') {
      const spec = f.reference ? references?.[f.reference.entity] : undefined;
      return (
        <ReferenceFieldInput
          spec={spec}
          value={String(value)}
          onChange={setField}
          placeholder={f.placeholder}
          disabled={f.disabled}
        />
      );
    }
    if (f.type === 'directory') {
      // Directory path input: the tauri directory dialog is unreachable in the component environment (module ⑧ ruling 10 precedent), hand-typed form
      return (
        <Input
          value={String(value)}
          onChange={setField}
          placeholder={f.placeholder ?? '如 outputs/invoices/templates'}
          disabled={f.disabled}
        />
      );
    }
    if (f.type === 'datetime') {
      // Date-time: native datetime-local (the original react-datepicker is not on the component
      // dependency whitelist); the form value carries an ISO string, control values round-trip through the local shape
      return (
        <input
          type="datetime-local"
          className="sl-ui-input-field"
          value={toLocalInputValue(value ? String(value) : undefined)}
          onChange={(e) => setField(fromLocalInputValue(e.target.value))}
          disabled={f.disabled}
        />
      );
    }
    if (f.type === 'file') {
      // File path: hand-typed + fileProbe probe backfill (autoFill declares the keys); without the
      // probe function FileFieldInput degrades to a plain input
      return (
        <FileFieldInput
          path={String(value)}
          onPathChange={setField}
          autoFill={f.autoFill}
          probe={fileProbe}
          onAutoFill={(patch) => setFormValues((s) => ({ ...s, ...patch }))}
          formValues={formValues}
        />
      );
    }
    return (
      <Input
        value={String(value)}
        onChange={setField}
        placeholder={f.placeholder}
        disabled={f.disabled}
      />
    );
  }, [formValues, references, fileProbe]);

  const allColumns: DataTableColumn[] = useMemo(
    () => [
      ...columns,
      {
        key: '_actions',
        label: '操作',
        render: (_: unknown, row: Record<string, unknown>) => (
          <div className="sl-ui-row-actions">
            {!hideDefaultDetailButton && (
              <Button variant="ghost" size="sm" onClick={() => setDetailRow(row as T)}>详情</Button>
            )}
            {!readOnly && !hideDefaultEditButton && (
              <Button variant="ghost" size="sm" onClick={() => openEditForm(row as T)}>编辑</Button>
            )}
            {extraRowActions?.(row as T)}
            {!readOnly && (
              <Button
                variant="ghost"
                size="sm"
                icon={<TrashIcon />}
                className="sl-ui-btn--text-danger"
                onClick={() => confirmDelete(row as T)}
                title="删除"
                aria-label="删除"
              />
            )}
          </div>
        ),
      },
    ],
    [columns, readOnly, hideDefaultDetailButton, hideDefaultEditButton, extraRowActions, openEditForm, confirmDelete],
  );

  // In client-side fallback mode, filter the current page by search (in queryToolbar mode search stays empty, no filtering)
  const visibleItems = useMemo(() => {
    if (!search.trim()) return items;
    const q = search.trim().toLowerCase();
    return items.filter((row) =>
      columns.some((col) => String((row as Record<string, unknown>)[col.key] ?? '').toLowerCase().includes(q)),
    );
  }, [items, search, columns]);

  return (
    <div className="sl-ui-crud-list-page">
      <header className="sl-ui-crud-header">
        {!readOnly && (
          <Button variant="primary" size="md" icon={<PlusIcon />} onClick={openAddForm}>
            新建
          </Button>
        )}
      </header>

      {/* The queryToolbar slot is injected by the caller (QueryListToolbar); without it, fall back to a plain search box */}
      {queryToolbar ?? (
        <div className="sl-ui-crud-toolbar">
          <div className="sl-ui-crud-toolbar-row">
            <SearchBar value={search} onChange={handleSearch} placeholder={`搜索${displayName}…`} />
          </div>
        </div>
      )}

      <div className="sl-ui-crud-content">
        {loading && <LoadingSpinner message={`正在加载${displayName}…`} />}

        {!loading && error && <ErrorMessage error={error} onRetry={refresh} />}

        {!loading && !error && visibleItems.length === 0 && (
          <EmptyState
            icon="data"
            title={`暂无${displayName}`}
            description="点击上方按钮创建第一条记录"
            action={
              !readOnly ? (
                <Button variant="primary" size="md" onClick={openAddForm}>
                  新建{displayName}
                </Button>
              ) : undefined
            }
          />
        )}

        {!loading && !error && visibleItems.length > 0 && (
          <>
            <DataTable
              columns={allColumns}
              rows={visibleItems as Record<string, unknown>[]}
              sortKey={sortKey}
              sortDir={sortDir}
              onSort={handleSort}
              emptyMessage={`暂无${displayName}`}
            />
            <div className="sl-ui-crud-pagination">
              <Pagination page={page} total={total} pageSize={DEFAULT_PAGE_SIZE} onChange={onPageChange ?? setFallbackPage} />
            </div>
          </>
        )}
      </div>

      <FormDialog
        open={formOpen}
        onClose={closeForm}
        onSubmit={handleFormSubmit}
        title={editingRow ? `编辑${displayName}` : `新建${displayName}`}
        loading={formLoading}
        submitDisabled={formLoading}
      >
        {formErrors._form && <div className="sl-ui-form-error-banner">{formErrors._form}</div>}
        {formSchema.fields.map((f) => (
          <FormField key={f.key} label={f.label} required={f.required} error={formErrors[f.key]} hint={f.hint}>
            {renderFieldInput(f)}
          </FormField>
        ))}
      </FormDialog>

      <ConfirmDialog
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        onConfirm={handleDelete}
        title={`删除${displayName}`}
        message={`确定要删除「${deleteTarget?.name ?? ''}」吗？`}
        confirmText="删除"
        cancelText="取消"
        danger
        loading={deleteLoading}
        warningText="此操作不可恢复。"
      />

      <Modal open={!!detailRow} onClose={() => setDetailRow(null)} title={`${displayName}详情`}>
        {detailRow && (
          <div className="sl-ui-info-grid">
            {columns.map((col) => (
              <div key={col.key} className="sl-ui-info-row">
                <span className="sl-ui-info-label">{col.label}</span>
                <span className="sl-ui-info-value">{formatDetailValue((detailRow as Record<string, unknown>)[col.key])}</span>
              </div>
            ))}
          </div>
        )}
      </Modal>
    </div>
  );
}

// =====================================================================
// reference-tier dedicated input subcomponents
// =====================================================================

/**
 * ReferenceFieldInput - reference-tier dropdown input (injection-table shape).
 *
 * useReferenceOptions statically loads the first page of options (no in-dropdown search — a lightweight field tier
 * for small-volume scenarios); edit echo: when the current value is not in options and spec.byId is available,
 * useReferenceOptionById resolves the echo option and merges it into the dropdown (the contract's byId consumer).
 */
function ReferenceFieldInput({
  spec,
  value,
  onChange,
  placeholder,
  disabled,
}: {
  spec: ReferenceSpec | undefined;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  disabled?: boolean;
}) {
  const { options: searched, loading, error } = useReferenceOptions(spec, '');
  const { option: byIdOption } = useReferenceOptionById(spec, value || null);

  // Echo merge: append when searched does not contain the current value and the byId resolution succeeded
  const options = useMemo(() => {
    const merged = [...searched];
    if (byIdOption && !merged.some((o) => o.value === byIdOption.value)) {
      merged.unshift(byIdOption);
    }
    return merged;
  }, [searched, byIdOption]);

  return (
    <div className="sl-ui-reference-field">
      <Select
        value={value || null}
        onChange={(v) => onChange(v ?? '')}
        options={options}
        placeholder={placeholder ?? '请选择'}
        disabled={disabled || loading}
      />
      {loading && <span className="sl-ui-reference-field-status">加载中…</span>}
      {!loading && error && <span className="sl-ui-reference-field-status sl-ui-reference-field-status--error">{error}</span>}
    </div>
  );
}

/**
 * FileFieldInput - file-tier path input (hand-typed path + a "detect file" button).
 *
 * Probe semantics (the componentization-era port of superlive autoFillFromPath):
 * - on a successful mime / size probe, backfill the corresponding fields (probe results are authoritative)
 * - sourceType gets the fixed value (autoFill.sourceTypeValue; the materials convention is 'local')
 * - the title field gets fileName backfilled only when currently empty (avoiding clobbering user input)
 * - when all three probe fields are null (not a file path), report "unrecognized" only and leave the form untouched
 * - without probe (the caller passed no fileProbe), degrade to a plain input, no probe button rendered
 */
function FileFieldInput({
  path,
  onPathChange,
  autoFill,
  probe,
  onAutoFill,
  formValues,
}: {
  path: string;
  onPathChange: (v: string) => void;
  autoFill?: FileAutoFill;
  probe?: FileProbeFn;
  onAutoFill: (patch: Record<string, string | undefined>) => void;
  formValues: Record<string, FieldValue>;
}) {
  const [probing, setProbing] = useState(false);
  const [probeError, setProbeError] = useState<string | null>(null);

  const handleProbe = useCallback(async () => {
    if (!probe) return;
    if (!path.trim()) {
      setProbeError('请先填写文件路径');
      return;
    }
    setProbing(true);
    setProbeError(null);
    try {
      const meta = await probe(path.trim());
      if (meta.size == null && meta.mime == null && meta.fileName == null) {
        setProbeError('无法识别该路径（不存在或不可读）');
        return;
      }
      const patch: Record<string, string | undefined> = {};
      if (autoFill?.mimeTypeKey && meta.mime != null) patch[autoFill.mimeTypeKey] = meta.mime;
      if (autoFill?.sizeBytesKey && meta.size != null) patch[autoFill.sizeBytesKey] = String(meta.size);
      if (autoFill?.sourceTypeKey && autoFill.sourceTypeValue) patch[autoFill.sourceTypeKey] = autoFill.sourceTypeValue;
      if (autoFill?.titleKey && meta.fileName != null) {
        const current = formValues[autoFill.titleKey];
        if (!current || String(current).length === 0) patch[autoFill.titleKey] = meta.fileName;
      }
      onAutoFill(patch);
    } catch (e: unknown) {
      setProbeError((e as Error)?.message ?? '探测失败');
    } finally {
      setProbing(false);
    }
  }, [probe, path, autoFill, formValues, onAutoFill]);

  return (
    <div className="sl-ui-file-field">
      <Input
        value={path}
        onChange={onPathChange}
        placeholder="如 /home/user/materials/report.pdf"
      />
      {probe && (
        <Button variant="secondary" size="md" onClick={handleProbe} disabled={probing}>
          {probing ? '识别中…' : '识别文件'}
        </Button>
      )}
      {probeError && <span className="sl-ui-file-field-status sl-ui-file-field-status--error">{probeError}</span>}
    </div>
  );
}
