/**
 * Pagination - pagination component (verbatim from superlive).
 */
import { Button } from './Button';

export interface PaginationProps {
  page: number;
  total: number;
  pageSize?: number;
  onChange: (page: number) => void;
}

function getPageNumbers(current: number, total: number, maxVisible = 7): number[] {
  if (total <= maxVisible) return Array.from({ length: total }, (_, i) => i + 1);
  const half = Math.floor(maxVisible / 2);
  let start = Math.max(1, current - half);
  const end = Math.min(total, start + maxVisible - 1);
  if (end - start + 1 < maxVisible) start = Math.max(1, end - maxVisible + 1);
  const pages: number[] = [];
  if (start > 1) pages.push(1);
  if (start > 2) pages.push(-1); // ellipsis
  for (let i = start; i <= end; i++) pages.push(i);
  if (end < total - 1) pages.push(-1);
  if (end < total) pages.push(total);
  return pages;
}

export function Pagination({ page, total, pageSize = 20, onChange }: PaginationProps) {
  const totalPages = Math.max(1, Math.ceil(total / pageSize));
  const pages = getPageNumbers(page, totalPages);

  return (
    <div className="sl-ui-pagination">
      <Button
        variant="secondary"
        size="lg"
        disabled={page <= 1}
        onClick={() => onChange(page - 1)}
        title="上一页"
        aria-label="上一页"
      >
        ‹
      </Button>

      {pages.map((p, idx) =>
        p === -1 ? (
          <span key={`ellipsis-${idx}`} style={{ color: 'var(--sl-ui-color-text-muted)', padding: '0 4px' }}>…</span>
        ) : (
          <Button
            key={p}
            variant={p === page ? 'primary' : 'secondary'}
            size="lg"
            aria-current={p === page ? 'page' : undefined}
            onClick={() => onChange(p)}
          >
            {p}
          </Button>
        )
      )}

      <Button
        variant="secondary"
        size="lg"
        disabled={page >= totalPages}
        onClick={() => onChange(page + 1)}
        title="下一页"
        aria-label="下一页"
      >
        ›
      </Button>

      <span className="sl-ui-pagination-info">
        共 {total} 条 / {totalPages} 页
      </span>
    </div>
  );
}

export default Pagination;
