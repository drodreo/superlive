/**
 * QueryConditionRow - a single condition row inside the query drawer: [field label] [value input].
 *
 * The privatized consolidation of superlive:
 * - the operator is preset to fieldSpec.operators[0] (no dropdown exposed)
 * - no reversed toggle or delete button rendered (the drawer iterates schema.fields; no add/remove)
 * - value inputs support the tiers the facility query schema actually uses: string / number / datetime /
 *   enum / reference (reference goes through the references injection table — the ReferenceSpec is passed through by the consumer
 *   along the CrudListPage/QueryListToolbar/QueryDrawer chain; the facility stays unaware of concrete entities;
 *   an unregistered spec warns in dev mode and empties the dropdown); the string hand-typed fallback tier is kept — domains without an
 *   entries data source (ruling 1 shape) can degrade the reference tier to hand-typed string;
 *   the boolean tier has no dedicated branch — the original QueryConditionRow likewise falls to the default TextInput
 *   (wire value as string), behavior aligned
 */
import type { QueryCondition, QueryFieldSpec, QuerySingleValue } from '../../../dgpqp';
import type { ReferenceSpec } from '../../types';
import { TextInput, NumberInput, RangePicker } from './QueryInputs';
import { Select } from '../Select';
import { useReferenceOptions } from '../../hooks/useReferenceOptions';

export interface QueryConditionRowProps {
  condition: QueryCondition;
  fieldSpec: QueryFieldSpec;
  onChange: (condition: QueryCondition) => void;
  /** reference-tier injection table (key = schema.reference.entity), passed through by the consumer */
  references?: Record<string, ReferenceSpec>;
}

/** Shared by the enum / reference tiers: single-select dropdown (a QueryConditionRow-private subcomponent) */
function EnumOrReferenceInput({
  fieldSpec,
  references,
  value,
  onChange,
}: {
  fieldSpec: QueryFieldSpec;
  references?: Record<string, ReferenceSpec>;
  value: string;
  onChange: (v: string) => void;
}) {
  if (fieldSpec.type === 'enum') {
    return (
      <Select
        value={value || null}
        onChange={(v) => onChange(v ?? '')}
        options={(fieldSpec.enumValues ?? []).map((opt) => ({ value: opt, label: opt }))}
        placeholder="请选择..."
      />
    );
  }
  const spec = fieldSpec.reference ? references?.[fieldSpec.reference.entity] : undefined;
  return <ReferenceConditionInput spec={spec} value={value} onChange={onChange} />;
}

/**
 * reference-tier input (injection-table shape): useReferenceOptions statically loads the first page of options,
 * with no in-dropdown search — a lightweight field tier (the same strategy as the module ⑦⑧ reference tiers).
 * A missing spec (references does not register that entity key) is warned in dev mode by the hook.
 */
function ReferenceConditionInput({
  spec,
  value,
  onChange,
}: {
  spec: ReferenceSpec | undefined;
  value: string;
  onChange: (v: string) => void;
}) {
  const { options, loading, error } = useReferenceOptions(spec, '');
  return (
    <div className="sl-ui-reference-field">
      <Select
        value={value || null}
        onChange={(v) => onChange(v ?? '')}
        options={options}
        placeholder="请选择..."
        disabled={loading}
      />
      {loading && <span className="sl-ui-reference-field-status">加载中…</span>}
      {!loading && error && <span className="sl-ui-reference-field-status sl-ui-reference-field-status--error">{error}</span>}
    </div>
  );
}

export function QueryConditionRow({
  condition,
  fieldSpec,
  onChange,
  references,
}: QueryConditionRowProps) {
  // Render the value input
  const renderValueInput = () => {
    const { operator, value } = condition;

    // single value
    if (value.type === 'single') {
      const singleValue = value.data ?? null;

      // string + contains / eq → TextInput
      if (fieldSpec.type === 'string' && (operator === 'contains' || operator === 'eq')) {
        return (
          <TextInput
            value={(singleValue as string) ?? ''}
            onChange={(v) => onChange({ ...condition, value: { type: 'single', data: v } })}
            placeholder="筛选值…"
          />
        );
      }

      // number + eq / lt / gt / lte / gte → NumberInput
      if (fieldSpec.type === 'number' && ['eq', 'lt', 'gt', 'lte', 'gte'].includes(operator)) {
        return (
          <NumberInput
            value={singleValue as number | null}
            onChange={(v) => onChange({ ...condition, value: { type: 'single', data: v } })}
          />
        );
      }

      // datetime + eq → native datetime-local
      if (fieldSpec.type === 'datetime' && operator === 'eq') {
        return (
          <input
            type="datetime-local"
            className="sl-ui-input-field"
            value={singleValue ? new Date(singleValue as string).toISOString().slice(0, 16) : ''}
            onChange={(e) => {
              const v = e.target.value;
              onChange({
                ...condition,
                value: { type: 'single', data: v ? new Date(v).toISOString() : null },
              });
            }}
          />
        );
      }

      // reference + eq → injection-table dropdown; enum + eq → enumValues dropdown
      if (fieldSpec.type === 'reference' && operator === 'eq' && fieldSpec.reference) {
        return (
          <EnumOrReferenceInput
            fieldSpec={fieldSpec}
            references={references}
            value={(singleValue as string) ?? ''}
            onChange={(v) => onChange({ ...condition, value: { type: 'single', data: v || null } })}
          />
        );
      }
      if (fieldSpec.type === 'enum' && operator === 'eq') {
        return (
          <EnumOrReferenceInput
            fieldSpec={fieldSpec}
            references={references}
            value={(singleValue as string) ?? ''}
            onChange={(v) => onChange({ ...condition, value: { type: 'single', data: v || null } })}
          />
        );
      }

      // default text (the boolean tier also lands here — original behavior)
      return (
        <TextInput
          value={(singleValue as string) ?? ''}
          onChange={(v) => onChange({ ...condition, value: { type: 'single', data: v } })}
        />
      );
    }

    // range value (between)
    if (value.type === 'range') {
      return (
        <RangePicker
          min={value.data?.min as string | number | undefined}
          max={value.data?.max as string | number | undefined}
          onChange={(min, max) =>
            onChange({
              ...condition,
              value: {
                type: 'range',
                data: { min: min as QuerySingleValue, max: max as QuerySingleValue },
              },
            })
          }
          type={fieldSpec.type === 'datetime' ? 'datetime' : 'number'}
        />
      );
    }

    // set value (in): the facility query schema has no fields with the in operator, so there is no legal render
    return null;
  };

  return (
    <div className="sl-ui-query-condition-row">
      {/* Field label (read-only) */}
      <span className="sl-ui-query-condition-field-label">{fieldSpec.label}</span>

      {/* Value input (operator preset to fieldSpec.operators[0]) */}
      <div className="sl-ui-query-condition-value">{renderValueInput()}</div>
    </div>
  );
}

export default QueryConditionRow;
