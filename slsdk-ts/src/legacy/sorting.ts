// QuerySorting: sorting descriptor (aligned with core/src/legacy/sorting.rs, copied verbatim)

export interface QuerySorting {
  field: string;
  direction: "asc" | "desc";
}
