/**
 * DataTable - table display component (the superlive privatized edition).
 *
 * i18n removed (labels render directly); DATETIME_KEYS is consolidated to the datetime columns the facility actually has
 * (createdAt/updatedAt); hits go through formatDateTime for localized display.
 * th/td carry explicit class names (sl-ui-data-table-th/td) — the CSS contract requires eliminating
 * "table-container-class + bare element" descendant selectors; every semantic node gets a single class.
 */
import type { ReactNode } from 'react';
import { formatDateTime } from '../utils/datetime';

const DATETIME_KEYS = new Set(['createdAt', 'updatedAt']);

export interface DataTableColumn<T extends Record<string, unknown> = Record<string, unknown>> {
  key: string;
  label: string;
  sortable?: boolean;
  render?: (value: unknown, row: T) => ReactNode;
}

export interface DataTableProps<T extends Record<string, unknown> = Record<string, unknown>> {
  columns: DataTableColumn<T>[];
  rows: T[];
  sortKey?: string;
  sortDir?: 'asc' | 'desc';
  onSort?: (key: string) => void;
  emptyMessage?: string;
}

function SortIcon({ active, dir }: { active: boolean; dir?: 'asc' | 'desc' }) {
  if (!active) return <span className="sl-ui-sort-icon">⇅</span>;
  return <span className="sl-ui-sort-icon sl-ui-sort-icon--active">{dir === 'asc' ? '↑' : '↓'}</span>;
}

export function DataTable<T extends Record<string, unknown>>({
  columns,
  rows,
  sortKey,
  sortDir,
  onSort,
  emptyMessage,
}: DataTableProps<T>) {
  const finalEmptyMessage = emptyMessage ?? '暂无数据';
  return (
    <div className="sl-ui-data-table-wrapper">
      <table className="sl-ui-data-table">
        <thead>
          <tr>
            {columns.map((col) => (
              <th
                key={col.key}
                className={`sl-ui-data-table-th${col.sortable ? ' sl-ui-data-table-th--sortable' : ''}`}
                onClick={col.sortable && onSort ? () => onSort(col.key) : undefined}
              >
                {col.label}
                {col.sortable && (
                  <SortIcon active={sortKey === col.key} dir={sortKey === col.key ? sortDir : undefined} />
                )}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 ? (
            <tr>
              <td
                className="sl-ui-data-table-td"
                colSpan={columns.length}
                style={{ textAlign: 'center', color: 'var(--sl-ui-color-text-muted)', padding: '24px' }}
              >
                {finalEmptyMessage}
              </td>
            </tr>
          ) : (
            rows.map((row, idx) => (
              <tr key={idx}>
                {columns.map((col) => (
                  <td key={col.key} className="sl-ui-data-table-td">
                    {col.render
                      ? col.render(row[col.key], row)
                      : DATETIME_KEYS.has(col.key)
                        ? formatDateTime(row[col.key] as string | null | undefined)
                        : String(row[col.key] ?? '')}
                  </td>
                ))}
              </tr>
            ))
          )}
        </tbody>
      </table>
    </div>
  );
}

export default DataTable;
