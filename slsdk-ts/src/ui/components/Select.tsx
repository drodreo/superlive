/**
 * Select - lightweight dropdown (a wrapper over the native select).
 *
 * superlive's 552-line general Select (portal dropdown + search + paged loading) is not part of the facility
 * minimal set: this component's only consumers are the QueryListToolbar sort dropdown and the enum/reference
 * condition rows (static options, single select), which the native select fully covers. The interface stays a subset of the original
 * (value: string | null; clearing goes through the placeholder empty-value option).
 */
import type { SelectOption } from '../types';

export interface SelectProps {
  value: string | null;
  onChange: (value: string | null) => void;
  options: SelectOption[];
  placeholder?: string;
  disabled?: boolean;
  className?: string;
}

export function Select({ value, onChange, options, placeholder, disabled, className }: SelectProps) {
  return (
    <select
      className={`sl-ui-select${className ? ' ' + className : ''}`}
      value={value ?? ''}
      onChange={(e) => onChange(e.target.value === '' ? null : e.target.value)}
      disabled={disabled}
    >
      <option value="">{placeholder ?? '请选择'}</option>
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

export default Select;
