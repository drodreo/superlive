/**
 * FormField - form field wrapper component (label + control + error/hint).
 *
 * Shared-facility edition: FormFieldSchema.type supports eight tiers — input / textarea / select (static
 * options) / reference (injection table) / checkbox / directory (hand-typed path) / datetime
 * (native datetime-local control — react-datepicker is not on the dependency whitelist; a zero-new-dependency degraded
 * form, values round-trip as ISO strings) / file (hand-typed path + fileProbe probe backfill; autoFill
 * declares the backfill keys — the probe function is injected by the consumer via CrudListPage's fileProbe prop;
 * without it the file tier degrades to a plain input); i18n protocol fields are removed, and label/hint/placeholder
 * are written directly in Chinese.
 *
 * reference tier (injection-table contract): the schema only declares the reference key `reference.entity: string`;
 * the data source is injected by the consumer via `references?: Record<string, ReferenceSpec>` (see
 * CrudListPage / QueryListToolbar).
 */
import type { ReactNode } from 'react';
import type { SelectOption } from '../types';

/** file-tier probe backfill key declarations (the componentization-era port of the materials file_meta conventions) */
export interface FileAutoFill {
  /** The probed MIME is backfilled into this field */
  mimeTypeKey?: string;
  /** The probed byte size is backfilled into this field */
  sizeBytesKey?: string;
  /** A fixed value is backfilled into this field (the materials convention is 'local') */
  sourceTypeKey?: string;
  /** The fixed backfill value for the sourceTypeKey field */
  sourceTypeValue?: string;
  /** Title field (fileName is backfilled only when the field is currently empty, to avoid clobbering user input) */
  titleKey?: string;
}

export interface FormFieldSchema {
  key: string;
  label: string;
  type: 'input' | 'textarea' | 'select' | 'reference' | 'checkbox' | 'directory' | 'datetime' | 'file';
  required?: boolean;
  placeholder?: string;
  /** Disabled field (passed through to the underlying input control; not editable in the UI but shows the current value) */
  disabled?: boolean;
  /** Helper hint (explains what the field is for or how to change it) */
  hint?: string;
  /** select tier: static options */
  options?: SelectOption[];
  /** reference tier: reference entity key (a key of the consumer's references injection table) */
  reference?: { entity: string };
  /** file tier: probe backfill key declarations (pairs with CrudListPage's fileProbe) */
  autoFill?: FileAutoFill;
}

/** Form schema */
export interface FormSchema {
  fields: FormFieldSchema[];
}

export interface FormFieldProps {
  label?: string;
  required?: boolean;
  error?: string;
  hint?: string;
  children: ReactNode;
  className?: string;
}

export function FormField({ label, required, error, hint, children, className }: FormFieldProps) {
  return (
    <div className={`sl-ui-form-field${className ? ' ' + className : ''}`}>
      {label && (
        <label className={`sl-ui-form-label${required ? ' sl-ui-form-label--required' : ''}`}>{label}</label>
      )}
      {children}
      {error && <span className="sl-ui-form-error">{error}</span>}
      {hint && !error && <span className="sl-ui-form-hint">{hint}</span>}
    </div>
  );
}

export default FormField;
