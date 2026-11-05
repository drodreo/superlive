/**
 * DataTable - table display component (the superlive privatized edition).
 *
 * i18n removed (labels render directly); DATETIME_KEYS is consolidated to the datetime columns the facility actually has
 * (createdAt/updatedAt); hits go through formatDateTime for localized display.
 * th/td carry explicit class names (sl-ui-data-table-th/td) — the CSS contract requires eliminating
 * "table-container-class + bare element" descendant selectors; every semantic node gets a single class.
 */
import type { ReactNode } from "react";
import { formatDateTime } from "../utils/datetime";

const DATETIME_KEYS = new Set(["createdAt", "updatedAt"]);

export interface DataTableColumn<
  T extends Record<string, unknown> = Record<string, unknown>,
> {
  key: string;
  label: string;
  sortable?: boolean;
  render?: (value: unknown, row: T) => ReactNode;
}

/**
 * One row action button definition for the trailing operations column. The
 * per-row label and handler come from the caller, already resolved against
 * the row's own scope — protocol semantics (action declarations, context
 * pointers) stay outside this presentational component.
 */
export interface DataTableRowAction {
  label: (rowIndex: number) => string;
  onAction: (rowIndex: number) => void;
}

export interface DataTableProps<
  T extends Record<string, unknown> = Record<string, unknown>,
> {
  columns: DataTableColumn<T>[];
  rows: T[];
  sortKey?: string;
  sortDir?: "asc" | "desc";
  onSort?: (key: string) => void;
  emptyMessage?: string;
  rowActions?: DataTableRowAction[];
}

function SortIcon({ active, dir }: { active: boolean; dir?: "asc" | "desc" }) {
  if (!active) return <span className="sl-ui-sort-icon">⇅</span>;
  return (
    <span className="sl-ui-sort-icon sl-ui-sort-icon--active">
      {dir === "asc" ? "↑" : "↓"}
    </span>
  );
}

export function DataTable<T extends Record<string, unknown>>({
  columns,
  rows,
  sortKey,
  sortDir,
  onSort,
  emptyMessage,
  rowActions,
}: DataTableProps<T>) {
  const finalEmptyMessage = emptyMessage ?? "暂无数据";
  const columnCount = columns.length + (rowActions ? 1 : 0);
  return (
    <div className="sl-ui-data-table-wrapper">
      <table className="sl-ui-data-table">
        <thead>
          <tr>
            {columns.map((col) => (
              <th
                key={col.key}
                className={`sl-ui-data-table-th${col.sortable ? " sl-ui-data-table-th--sortable" : ""}`}
                onClick={
                  col.sortable && onSort ? () => onSort(col.key) : undefined
                }
              >
                {col.label}
                {col.sortable && (
                  <SortIcon
                    active={sortKey === col.key}
                    dir={sortKey === col.key ? sortDir : undefined}
                  />
                )}
              </th>
            ))}
            {rowActions && (
              <th
                className="sl-ui-data-table-th sl-ui-data-table-th--actions"
                aria-label="actions"
              />
            )}
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 ? (
            <tr>
              <td
                className="sl-ui-data-table-td"
                colSpan={columnCount}
                style={{
                  textAlign: "center",
                  color: "var(--sl-ui-color-text-muted)",
                  padding: "24px",
                }}
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
                        ? formatDateTime(
                            row[col.key] as string | null | undefined,
                          )
                        : String(row[col.key] ?? "")}
                  </td>
                ))}
                {rowActions && (
                  <td className="sl-ui-data-table-td sl-ui-data-table-td--actions">
                    {rowActions.map((action, actionIndex) => (
                      <button
                        key={actionIndex}
                        type="button"
                        className="sl-ui-btn sl-ui-btn--sm sl-ui-data-table-action"
                        onClick={() => action.onAction(idx)}
                      >
                        {action.label(idx)}
                      </button>
                    ))}
                  </td>
                )}
              </tr>
            ))
          )}
        </tbody>
      </table>
    </div>
  );
}

export default DataTable;
