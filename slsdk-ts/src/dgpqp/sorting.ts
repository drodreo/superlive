// QuerySorting: sorting descriptor (aligned with core/src/dgpqp/sorting.rs, copied verbatim)

export interface QuerySorting {
  field: string;
  direction: 'asc' | 'desc';
}
