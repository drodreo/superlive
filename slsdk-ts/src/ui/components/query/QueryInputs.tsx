/**
 * QueryInputs - value input controls for query condition rows (consolidated into one file).
 *
 * The privatized consolidation of superlive's QueryInputs directory: only the four controls the facility's query schema
 * actually uses are kept (TextInput / NumberInput / DateTimePicker / RangePicker);
 * EnumSelect / MultiSelect depend on the enum/reference field types and are not migrated.
 * DateTimePicker is a native <input type="datetime-local">, no react-datepicker dependency.
 */

// ── TextInput: single-line text ───────────────────────────────────

export interface TextInputProps {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
}

export function TextInput({ value, onChange, placeholder = '', disabled = false, className = '' }: TextInputProps) {
  return (
    <input
      type="text"
      className={`sl-ui-input-field ${className}`.trim()}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      disabled={disabled}
    />
  );
}

// ── NumberInput: number ─────────────────────────────────────────────

export interface NumberInputProps {
  value: number | null;
  onChange: (v: number | null) => void;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
}

export function NumberInput({ value, onChange, placeholder = '', disabled = false, className = '' }: NumberInputProps) {
  return (
    <input
      type="number"
      className={`sl-ui-input-field ${className}`.trim()}
      value={value ?? ''}
      onChange={(e) => {
        const v = e.target.value;
        onChange(v === '' ? null : Number(v));
      }}
      placeholder={placeholder}
      disabled={disabled}
    />
  );
}

// ── DateTimePicker: date-time (native control) ──────────────

export interface DateTimePickerProps {
  value: string | null; // ISO 8601 string
  onChange: (v: string | null) => void;
  disabled?: boolean;
  className?: string;
}

/** ISO → datetime-local control value (local timezone, minute precision) */
function toLocalString(iso: string | null): string {
  if (!iso) return '';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** datetime-local control value → ISO */
function toIso(local: string): string | null {
  if (!local) return null;
  const d = new Date(local);
  return Number.isNaN(d.getTime()) ? null : d.toISOString();
}

export function DateTimePicker({ value, onChange, disabled = false, className = '' }: DateTimePickerProps) {
  return (
    <input
      type="datetime-local"
      className={`sl-ui-input-field ${className}`.trim()}
      value={toLocalString(value)}
      onChange={(e) => onChange(toIso(e.target.value))}
      disabled={disabled}
    />
  );
}

// ── RangePicker: two-value range (between) ─────────────────────

export interface RangePickerProps {
  min: unknown;
  max: unknown;
  onChange: (min: unknown, max: unknown) => void;
  type: 'datetime' | 'number';
  disabled?: boolean;
  className?: string;
}

export function RangePicker({ min, max, onChange, type, disabled = false, className = '' }: RangePickerProps) {
  if (type === 'datetime') {
    return (
      <div className={`sl-ui-range-picker ${className}`.trim()}>
        <DateTimePicker
          value={(min as string | null) ?? null}
          onChange={(v) => onChange(v, max)}
          disabled={disabled}
        />
        <span className="sl-ui-range-picker-separator">~</span>
        <DateTimePicker
          value={(max as string | null) ?? null}
          onChange={(v) => onChange(min, v)}
          disabled={disabled}
        />
      </div>
    );
  }

  return (
    <div className={`sl-ui-range-picker ${className}`.trim()}>
      <NumberInput
        value={(min as number | null) ?? null}
        onChange={(v) => onChange(v, max)}
        disabled={disabled}
      />
      <span className="sl-ui-range-picker-separator">~</span>
      <NumberInput
        value={(max as number | null) ?? null}
        onChange={(v) => onChange(min, v)}
        disabled={disabled}
      />
    </div>
  );
}
