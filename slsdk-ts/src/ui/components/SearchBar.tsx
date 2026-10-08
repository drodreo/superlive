/**
 * SearchBar - search box component (debounced).
 * lucide-react is not on the whitelist: the Search/SlidersHorizontal icons are replaced with inline SVG.
 */
import { useEffect, useRef, useState, type ChangeEvent } from 'react';

export interface SearchBarProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  debounceMs?: number;
  /** Whether to show the advanced-filter button (embedded on the right side of the search box) */
  showAdvanced?: boolean;
  onAdvanced?: () => void;
}

function SearchIcon() {
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="sl-ui-search-icon">
      <circle cx="11" cy="11" r="8" />
      <line x1="21" y1="21" x2="16.65" y2="16.65" />
    </svg>
  );
}

function SlidersIcon() {
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <line x1="4" y1="21" x2="4" y2="14" />
      <line x1="4" y1="10" x2="4" y2="3" />
      <line x1="12" y1="21" x2="12" y2="12" />
      <line x1="12" y1="8" x2="12" y2="3" />
      <line x1="20" y1="21" x2="20" y2="16" />
      <line x1="20" y1="12" x2="20" y2="3" />
      <line x1="1" y1="14" x2="7" y2="14" />
      <line x1="9" y1="8" x2="15" y2="8" />
      <line x1="17" y1="16" x2="23" y2="16" />
    </svg>
  );
}

export function SearchBar({
  value,
  onChange,
  placeholder,
  debounceMs = 300,
  showAdvanced = false,
  onAdvanced,
}: SearchBarProps) {
  const finalPlaceholder = placeholder ?? '搜索…';
  const [localValue, setLocalValue] = useState(value);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Sync when the external value changes (controlled)
  useEffect(() => {
    setLocalValue(value);
  }, [value]);

  function handleInput(e: ChangeEvent<HTMLInputElement>) {
    const v = e.target.value;
    setLocalValue(v);
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = setTimeout(() => onChange(v), debounceMs);
  }

  function handleClear() {
    setLocalValue('');
    if (timerRef.current) clearTimeout(timerRef.current);
    onChange('');
  }

  // Integrated mode: advanced filter embedded to the right of the search box (one visual unit, no divider)
  if (showAdvanced) {
    return (
      <div className="sl-ui-search-with-advanced">
        <div className="sl-ui-search-bar-inner">
          <SearchIcon />
          <input
            type="text"
            className="sl-ui-search-input"
            value={localValue}
            onChange={handleInput}
            placeholder={finalPlaceholder}
          />
          <button
            className="sl-ui-search-clear"
            onClick={handleClear}
            title="清空"
            aria-label="清空"
            aria-hidden={!localValue}
            tabIndex={localValue ? 0 : -1}
            style={!localValue ? { visibility: 'hidden', pointerEvents: 'none' } : undefined}
          >
            ✕
          </button>
        </div>
        <button
          className="sl-ui-search-advanced-btn"
          onClick={onAdvanced}
          title="高级筛选"
          aria-label="高级筛选"
          type="button"
        >
          <SlidersIcon />
        </button>
      </div>
    );
  }

  // Standalone mode: plain search box
  return (
    <div className="sl-ui-search-bar">
      <SearchIcon />
      <input
        type="text"
        className="sl-ui-search-input"
        value={localValue}
        onChange={handleInput}
        placeholder={finalPlaceholder}
      />
      <button
        className="sl-ui-search-clear"
        onClick={handleClear}
        title="清空"
        aria-label="清空"
        aria-hidden={!localValue}
        tabIndex={localValue ? 0 : -1}
        style={!localValue ? { visibility: 'hidden', pointerEvents: 'none' } : undefined}
      >
        ✕
      </button>
    </div>
  );
}

export default SearchBar;
