/**
 * QueryListToolbar - the list-page toolbar for search/sort/drawer.
 *
 * Core abstraction (kept verbatim from the superlive v13 consolidation):
 * - one standalone component overall (main search + sort + drawer), not handled inside CrudListPage
 * - querySchema is handled by this component (CrudListPage passes it in via the queryToolbar slot)
 * - fixedExtra holds business conditions fixed for the entity lifetime: never rendered, never expressed in the schema,
 *   automatically merged to the head of the conditions array on onFilterChange
 * - references (the reference-tier injection table) passes through to QueryDrawer → QueryConditionRow
 * - pagination is not this component's concern (page is held by the data hook, passed through by CrudListPage)
 */
import { useState, useCallback, useEffect, useRef } from 'react';
import { SearchBar } from '../SearchBar';
import { Select } from '../Select';
import { QueryDrawer } from './QueryDrawer';
import type {
  QuerySchema,
  QueryCondition,
  QuerySorting,
} from '../../../dgpqp';
import type { ReferenceSpec, SelectOption } from '../../types';

export interface QueryListToolbarFilter {
  conditions?: QueryCondition[];
  sorting?: QuerySorting;
  includeDeleted?: boolean;
}

export interface QueryListToolbarProps {
  /** The query schema (required) */
  querySchema: QuerySchema;
  /** Callback on filter change (the assembled dgpqp filter from main search + drawer + fixedExtra + sorting) */
  onFilterChange: (filter: QueryListToolbarFilter) => void;
  /**
   * Business conditions fixed for the entity lifetime.
   * - never rendered to the user (never enters the drawer)
   * - never expressed in the schema (keeps the schema decoupled from business rules)
   * - automatically merged to the head of the conditions array when onFilterChange fires
   */
  fixedExtra?: QueryCondition[];
  /** Default includeDeleted (defaults to false) */
  defaultIncludeDeleted?: boolean;
  /** reference-tier injection table (key = schema.reference.entity), passed through to the query drawer */
  references?: Record<string, ReferenceSpec>;
}

const FILTER_DEBOUNCE_MS = 300;

export function QueryListToolbar({
  querySchema,
  onFilterChange,
  fixedExtra = [],
  defaultIncludeDeleted = false,
  references,
}: QueryListToolbarProps) {
  const [searchInput, setSearchInput] = useState('');
  const [sortField, setSortField] = useState<string>('');
  const [sortDir, setSortDir] = useState<'asc' | 'desc'>('asc');
  const [includeDeleted] = useState(defaultIncludeDeleted);
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [drawerConditions, setDrawerConditions] = useState<QueryCondition[]>([]);

  // Sort dropdown options (every schema field is sortable;
  // the defaultField may participate in sorting — original behavior)
  const sortFieldOptions: SelectOption[] = querySchema.fields.map((f) => ({
    value: f.key,
    label: f.label,
  }));

  // Assemble the dgpqp filter and notify the parent when main search / drawer / sorting change
  // (onFilterChange/fixedExtra are kept in refs as the latest values, avoiding re-run loops from inline reference changes)
  const filterDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const onFilterChangeRef = useRef(onFilterChange);
  useEffect(() => {
    onFilterChangeRef.current = onFilterChange;
  }, [onFilterChange]);

  const fixedExtraRef = useRef(fixedExtra);
  useEffect(() => {
    fixedExtraRef.current = fixedExtra;
  }, [fixedExtra]);

  useEffect(() => {
    const cb = onFilterChangeRef.current;
    if (!cb) return;
    if (filterDebounceRef.current) clearTimeout(filterDebounceRef.current);
    filterDebounceRef.current = setTimeout(() => {
      const conditions: QueryCondition[] = [];
      // 1. fixedExtra lifetime-fixed conditions (highest priority, merged first)
      for (const c of fixedExtraRef.current) conditions.push(c);
      // 2. Main search: build a contains condition on defaultField
      if (searchInput.trim() && querySchema.defaultField) {
        conditions.push({
          field: querySchema.defaultField,
          operator: 'contains',
          reversed: false,
          value: { type: 'single', data: searchInput.trim() },
        });
      }
      // 3. Drawer conditions
      for (const c of drawerConditions) conditions.push(c);
      // 4. Sorting
      const sorting: QuerySorting | undefined = sortField
        ? { field: sortField, direction: sortDir }
        : undefined;
      cb({ conditions, sorting, includeDeleted });
    }, FILTER_DEBOUNCE_MS);
    return () => {
      if (filterDebounceRef.current) clearTimeout(filterDebounceRef.current);
    };
  }, [searchInput, sortField, sortDir, drawerConditions, includeDeleted, querySchema]);

  const openDrawer = useCallback(() => setDrawerOpen(true), []);
  const closeDrawer = useCallback(() => setDrawerOpen(false), []);

  const handleSortFieldChange = useCallback((field: string | null) => {
    setSortField(field ?? '');
  }, []);

  const handleSortDirChange = useCallback((dir: string | null) => {
    if (dir === 'asc' || dir === 'desc') {
      setSortDir(dir);
    }
  }, []);

  const handleDrawerApply = useCallback((conditions: QueryCondition[]) => {
    setDrawerConditions(conditions);
    setDrawerOpen(false);
  }, []);

  // Main search placeholder: the defaultField's Chinese label (the original passed i18n params; here it is written inline)
  const defaultFieldLabel =
    querySchema.fields.find((f) => f.key === querySchema.defaultField)?.label ??
    querySchema.defaultField;

  return (
    <>
      <div className="sl-ui-crud-toolbar">
        <div className="sl-ui-crud-toolbar-row">
          <SearchBar
            value={searchInput}
            onChange={setSearchInput}
            placeholder={`按${defaultFieldLabel}搜索…`}
            showAdvanced={querySchema.fields.length > 1}
            onAdvanced={openDrawer}
          />
          {sortFieldOptions.length > 0 && (
            <div className="sl-ui-crud-sort">
              <label className="sl-ui-crud-sort-label">排序</label>
              <Select
                className="sl-ui-crud-sort-field"
                value={sortField || null}
                onChange={handleSortFieldChange}
                options={sortFieldOptions}
                placeholder="排序字段"
              />
              <Select
                className="sl-ui-crud-sort-dir"
                value={sortDir}
                onChange={handleSortDirChange}
                options={[
                  { value: 'asc', label: '升序' },
                  { value: 'desc', label: '降序' },
                ]}
                placeholder="方向"
              />
            </div>
          )}
        </div>
      </div>
      <QueryDrawer
        querySchema={querySchema}
        open={drawerOpen}
        onClose={closeDrawer}
        onApply={handleDrawerApply}
        references={references}
      />
    </>
  );
}

export default QueryListToolbar;
