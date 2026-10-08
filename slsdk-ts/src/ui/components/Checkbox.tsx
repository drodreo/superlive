/**
 * Checkbox - checkbox component (controlled, verbatim from superlive).
 * Consumer: the CrudListPage checkbox tier (kept in the facility; some domain forms have no checkbox-tier fields yet,
 * so the tier and styles ride along with the facility as a spare).
 */
export interface CheckboxProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label?: string;
  disabled?: boolean;
  id?: string;
  name?: string;
}

export function Checkbox({ checked, onChange, label, disabled, id, name }: CheckboxProps) {
  return (
    <label className="sl-ui-checkbox-wrapper">
      <input
        id={id}
        name={name}
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        disabled={disabled}
        className="sl-ui-checkbox-input"
      />
      {label && <span className="sl-ui-checkbox-label">{label}</span>}
    </label>
  );
}

export default Checkbox;
