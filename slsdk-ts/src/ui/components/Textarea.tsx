/**
 * Textarea - multi-line text input component (controlled). Verbatim from superlive.
 * The error state uses a dedicated modifier class sl-ui-input-field--error (CSS contract: class+class compound selectors banned).
 */
export interface TextareaProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  rows?: number;
  disabled?: boolean;
  error?: string;
  id?: string;
  name?: string;
  className?: string;
}

export function Textarea({
  value,
  onChange,
  placeholder,
  rows = 4,
  disabled,
  error,
  id,
  name,
  className,
}: TextareaProps) {
  return (
    <div className="sl-ui-input-wrapper">
      <textarea
        id={id}
        name={name}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        rows={rows}
        disabled={disabled}
        className={`sl-ui-input-field sl-ui-textarea-field${error ? ' sl-ui-input-field--error' : ''}${className ? ' ' + className : ''}`}
      />
    </div>
  );
}

export default Textarea;
