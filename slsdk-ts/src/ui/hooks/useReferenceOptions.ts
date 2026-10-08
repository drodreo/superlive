/**
 * useReferenceOptions - hook that loads dropdown options for reference fields (injection-table shape).
 *
 * Contract (architecture.md "ReferenceEntity injection table"): the reference-tier data source is
 * injected by the consumer via `ReferenceSpec`; the facility stays unaware of concrete entities — the domain registry
 * (the componentization-era per-domain REFERENCE_SPECS maps) is gone; the spec is the only data channel.
 *
 * Built into the shared hook:
 * - 300ms debounce on the search term (avoids hitting spec.search on every keystroke)
 * - race protection (old subscriptions are cancelled when keyword/spec changes; late stale responses never land in state)
 * - missing spec: console.warn in dev mode (schema.reference.entity not registered in the injection table),
 *   options are emptied rather than silently fabricating fake data
 *
 * Callers: CrudListPage (FormField reference tier) + QueryConditionRow (the query
 * drawer reference tier). Paging/filtering semantics are owned by the spec.search implementation (contract:
 * search(keyword) returns one page of ReferenceOption[]).
 */
import { useEffect, useRef, useState } from 'react';
import type { ReferenceOption, ReferenceSpec } from '../types';

const DEFAULT_SEARCH_DEBOUNCE_MS = 300;

export interface UseReferenceOptionsResult {
  options: ReferenceOption[];
  loading: boolean;
  error: string | null;
}

/**
 * Loads dropdown options for a reference field
 * @param spec - the reference data source (a hit in the consumer's injection table; undefined = not registered)
 * @param search - the search term (handed to spec.search after a 300ms debounce)
 */
export function useReferenceOptions(
  spec: ReferenceSpec | undefined,
  search: string,
): UseReferenceOptionsResult {
  const [options, setOptions] = useState<ReferenceOption[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const cancelledRef = useRef(false);

  // search debounce (avoids hitting spec.search on every keystroke)
  const debouncedSearch = useDebouncedValue(search, DEFAULT_SEARCH_DEBOUNCE_MS);

  useEffect(() => {
    if (!spec) {
      // spec not registered (schema misconfigured or the consumer forgot to inject) — warn in dev mode, clear instead of fabricating
      console.warn(
        '[sl-ui] no ReferenceSpec injected for the reference kind (miss in the references registry) — clearing the dropdown',
      );
      setOptions([]);
      setLoading(false);
      setError(null);
      return;
    }
    cancelledRef.current = false;
    setLoading(true);
    setError(null);

    const sub = spec.search(debouncedSearch.trim()).subscribe({
      next: (opts) => {
        if (!cancelledRef.current) {
          setOptions(opts);
          setLoading(false);
        }
      },
      error: (e: unknown) => {
        if (!cancelledRef.current) {
          const err = e as { detail?: string; message?: string } | null;
          setError(err?.detail ?? err?.message ?? '加载失败');
          setLoading(false);
        }
      },
    });

    return () => {
      cancelledRef.current = true;
      sub.unsubscribe();
    };
  }, [spec, debouncedSearch]);

  return { options, loading, error };
}

export interface UseReferenceOptionByIdResult {
  option: ReferenceOption | undefined;
  loading: boolean;
}

/**
 * Resolves a single reference option by id (used to echo the current value in edit forms).
 * Returns undefined directly when spec.byId is not provided (the facility skips echo resolution; the dropdown only shows
 * options matches) — byId is an optional capability by contract.
 */
export function useReferenceOptionById(
  spec: ReferenceSpec | undefined,
  id: string | null | undefined,
): UseReferenceOptionByIdResult {
  const [option, setOption] = useState<ReferenceOption | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const cancelledRef = useRef(false);

  useEffect(() => {
    if (!spec?.byId || !id) {
      setOption(undefined);
      setLoading(false);
      return;
    }
    cancelledRef.current = false;
    setLoading(true);

    const sub = spec.byId(id).subscribe({
      next: (opt) => {
        if (!cancelledRef.current) {
          setOption(opt);
          setLoading(false);
        }
      },
      error: () => {
        if (!cancelledRef.current) {
          setOption(undefined);
          setLoading(false);
        }
      },
    });

    return () => {
      cancelledRef.current = true;
      sub.unsubscribe();
    };
  }, [spec, id]);

  return { option, loading };
}

/** Minimal debounce hook (same-origin implementation as the CRUD list page search debounce) */
function useDebouncedValue<T>(value: T, delay: number = DEFAULT_SEARCH_DEBOUNCE_MS): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(timer);
  }, [value, delay]);
  return debounced;
}
