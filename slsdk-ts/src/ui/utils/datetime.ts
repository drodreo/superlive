/** Date-time utility: display formatting for DataTable / the detail dialog */

/** ISO 8601 → localized time (zh-CN, 24-hour); empty values render '-', invalid values are returned as-is */
export function formatDateTime(iso: string | null | undefined): string {
  if (!iso) return '-';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString('zh-CN', { hour12: false });
}
