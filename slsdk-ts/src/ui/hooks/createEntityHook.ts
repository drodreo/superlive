/**
 * createEntityHook - factory for component list-data hooks (the superlive v13 consolidated edition, facility-ized).
 *
 * Consolidation notes (relative to superlive hooks/createEntityHook.ts and the componentization-era private edition):
 * - The generic factory is kept; Api instances are assembled directly by each hook caller (the facility does not depend on an
 *   api/registry hub — component environments have no global registry)
 * - hasGet / hasListAll / setListFilter / updateTriggersReload removed (no consumers inside components:
 *   list pages do not use get, filtering goes uniformly through the QueryListToolbar conditions channel,
 *   and includeDeleted is owned by QueryListToolbar)
 * - The dynamic itemsKey field is consolidated into a fixed `items` (pages destructure-rename, avoiding TS7061
 *   cross-type gymnastics)
 * - "Deleted" filtering: when the filter produced by QueryListToolbar's onFilterChange lacks
 *   includeDeleted, this factory defaults it to false (semantics aligned with the original setListFilter)
 */
import { useState, useEffect, useCallback } from 'react';
import type { Observable } from 'rxjs';
import type { BatchResult, SearchResponse } from '../types';
import type { QueryPagination } from '../../dgpqp';

/** Page size for list pages */
export const DEFAULT_PAGE_SIZE = 20;

/** The concrete Api object passed in by the caller (search/add/update/delete required) */
export interface EntityApi<T, F> {
  search: (filter: F) => Observable<SearchResponse<T>>;
  add: (items: T[]) => Observable<BatchResult<T>[]>;
  update: (items: T[]) => Observable<BatchResult<T>[]>;
  delete: (items: { id: string }[]) => Observable<BatchResult<T>[]>;
}

export interface CreateEntityHookConfig<T, F> {
  /** Lowercase plural, used in error messages (e.g. 'articles', 'materials') */
  entityName: string;
  api: EntityApi<T, F>;
}

export interface UseEntityResult<T, F> {
  items: T[];
  total: number;
  /** Current page (1-indexed) */
  page: number;
  loading: boolean;
  error: string | null;
  refresh: () => void;
  /** Raw setFilter (automatically resets page to 1) */
  setFilter: (filter: F | undefined) => void;
  /** Page turn (triggered by clicking Pagination; does not touch the filter) */
  setPage: (page: number) => void;
  add: (items: T[]) => Promise<BatchResult<T>[]>;
  update: (items: T[]) => Promise<BatchResult<T>[]>;
  remove: (items: { id: string }[]) => Promise<BatchResult<T>[]>;
}

/** Promisifies a single Observable call (shared by add/update/remove) */
function toPromise<T>(source$: Observable<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    source$.subscribe({ next: resolve, error: reject });
  });
}

export function createEntityHook<T, F extends object>(
  config: CreateEntityHookConfig<T, F>,
): (initialFilter?: F) => UseEntityResult<T, F> {
  const { entityName, api } = config;

  return function useEntityHook(initialFilter?: F): UseEntityResult<T, F> {
    const [items, setItems] = useState<T[]>([]);
    const [total, setTotal] = useState(0);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [filter, setFilterState] = useState<F | undefined>(initialFilter);
    const [page, setPageState] = useState(1);
    const [reloadKey, setReloadKey] = useState(0);

    // Subscribe to search: any change to filter + page + reloadKey triggers a reload
    useEffect(() => {
      setLoading(true);
      setError(null);
      const pagination: QueryPagination = {
        skip: (page - 1) * DEFAULT_PAGE_SIZE,
        limit: DEFAULT_PAGE_SIZE,
      };
      const searchFilter = {
        ...(filter ?? ({} as F)),
        includeDeleted: false,
        pagination,
      } as F;
      const sub = api.search(searchFilter).subscribe({
        next: (res) => {
          setItems(res.data);
          setTotal(res.total);
          setLoading(false);
        },
        error: (e: unknown) => {
          setError(
            (e as { detail?: string; message?: string })?.detail ??
              (e as Error)?.message ??
              `Failed to load ${entityName}`,
          );
          setLoading(false);
        },
      });
      return () => sub.unsubscribe();
    }, [filter, page, reloadKey, api, entityName]);

    const refresh = useCallback(() => setReloadKey((k) => k + 1), []);

    const add = useCallback(
      (newItems: T[]): Promise<BatchResult<T>[]> =>
        toPromise(api.add(newItems)).then((r) => {
          setReloadKey((k) => k + 1);
          return r;
        }),
      [api],
    );

    const update = useCallback(
      (newItems: T[]): Promise<BatchResult<T>[]> =>
        toPromise(api.update(newItems)).then((r) => {
          setReloadKey((k) => k + 1);
          return r;
        }),
      [api],
    );

    const remove = useCallback(
      (ids: { id: string }[]): Promise<BatchResult<T>[]> =>
        toPromise(api.delete(ids)).then((r) => {
          setReloadKey((k) => k + 1);
          return r;
        }),
      [api],
    );

    // setFilter automatically resets page to 1 (page is not part of the query; changing the filter should return to the first page)
    const setFilter = useCallback((f: F | undefined) => {
      setFilterState(f);
      setPageState(1);
    }, []);

    const setPage = useCallback((p: number) => {
      setPageState(p);
    }, []);

    return { items, total, page, loading, error, refresh, setFilter, setPage, add, update, remove };
  };
}
