/**
 * Input - text input component (controlled). Verbatim from superlive (i18n-related comments removed).
 * The error state uses a dedicated modifier class sl-ui-input-field--error (CSS contract: class+class compound selectors banned).
 */
export interface InputProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  type?: 'text' | 'email' | 'password' | 'number' | 'tel' | 'url';
  disabled?: boolean;
  error?: string;
  id?: string;
  name?: string;
  autoComplete?: string;
  className?: string;
}

export function Input({
  value,
  onChange,
  placeholder,
  type = 'text',
  disabled,
  error,
  id,
  name,
  autoComplete,
  className,
}: InputProps) {
  return (
    <div className="sl-ui-input-wrapper">
      <input
        id={id}
        name={name}
        type={type}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        disabled={disabled}
        autoComplete={autoComplete}
        className={`sl-ui-input-field${error ? ' sl-ui-input-field--error' : ''}${className ? ' ' + className : ''}`}
      />
    </div>
  );
}

export default Input;
