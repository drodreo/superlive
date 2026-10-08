/**
 * QueryDrawer - the advanced-filter drawer sliding out from the right.
 *
 * Rules (the superlive v13 abstraction kept verbatim):
 * - iterate querySchema.fields directly (excluding defaultField) to render all filter conditions
 * - users cannot add/remove conditions (avoids the logic headaches of arbitrary combinations)
 * - the operator is preset to fieldSpec.operators[0] (no dropdown exposed)
 * - apply: all non-empty values become a QueryCondition[] callback; clear: reset everything and callback with an empty array
 * - references (the reference-tier injection table) passes through verbatim to QueryConditionRow
 *
 * Parent: QueryListToolbar (open/onClose/onApply controlled).
 */
import { useState, useCallback, useMemo } from 'react';
import { Button } from '../Button';
import type { QuerySchema, QueryCondition, QueryFieldSpec, QueryValue } from '../../../dgpqp';
import type { ReferenceSpec } from '../../types';
import { QueryConditionRow } from './QueryConditionRow';

export interface QueryDrawerProps {
  querySchema: QuerySchema;
  open: boolean;
  onClose: () => void;
  onApply: (conditions: QueryCondition[]) => void;
  /** reference-tier injection table (key = schema.reference.entity), passed through verbatim to condition rows */
  references?: Record<string, ReferenceSpec>;
}

// Build the default QueryValue for an operator
function defaultValue(operator: string): QueryValue {
  if (operator === 'between') {
    return { type: 'range', data: { min: null, max: null } };
  }
  if (operator === 'in') {
    return { type: 'set', data: [] };
  }
  return { type: 'single', data: null };
}

// Whether a QueryValue is empty (left unfilled)
function isEmptyValue(value: QueryValue): boolean {
  if (value.type === 'single') return value.data === null || value.data === '';
  if (value.type === 'range') {
    return value.data.min === null && value.data.max === null;
  }
  if (value.type === 'set') return value.data.length === 0;
  return true;
}

export function QueryDrawer({
  querySchema,
  open,
  onClose,
  onApply,
  references,
}: QueryDrawerProps) {
  // Filterable fields (excluding the defaultField reserved for the main search)
  const filterableFields = useMemo<QueryFieldSpec[]>(
    () => querySchema.fields.filter((f) => f.key !== querySchema.defaultField),
    [querySchema],
  );

  // Internal state: each field's current value (indexed by fieldKey, initialized per the operators[0] type)
  const [values, setValues] = useState<Record<string, QueryValue>>(() => {
    const init: Record<string, QueryValue> = {};
    for (const f of filterableFields) {
      const op = f.operators[0] ?? 'eq';
      init[f.key] = defaultValue(op);
    }
    return init;
  });

  // Update a single field's value
  const updateValue = useCallback((fieldKey: string, value: QueryValue) => {
    setValues((prev) => ({ ...prev, [fieldKey]: value }));
  }, []);

  // Clear everything (values only, schema untouched) and notify the parent to re-query
  const clearAll = useCallback(() => {
    const next: Record<string, QueryValue> = {};
    for (const f of filterableFields) {
      const op = f.operators[0] ?? 'eq';
      next[f.key] = defaultValue(op);
    }
    setValues(next);
    onApply([]);
  }, [filterableFields, onApply]);

  // Apply: convert all non-empty values into QueryCondition[] for the parent
  const handleApply = useCallback(() => {
    const conditions: QueryCondition[] = [];
    for (const f of filterableFields) {
      const value = values[f.key];
      if (!value) continue;
      if (isEmptyValue(value)) continue;
      conditions.push({
        field: f.key,
        operator: f.operators[0] ?? 'eq',
        reversed: false,
        value,
      });
    }
    onApply(conditions);
  }, [filterableFields, values, onApply]);

  return (
    <>
      {/* Backdrop */}
      <div
        className={`sl-ui-query-drawer-backdrop${open ? ' sl-ui-query-drawer-backdrop--open' : ''}`}
        onClick={onClose}
        aria-hidden="true"
      />

      {/* Drawer body */}
      <div
        className={`sl-ui-query-drawer${open ? ' sl-ui-query-drawer--open' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-label="高级筛选"
      >
        {/* Header */}
        <div className="sl-ui-query-drawer-header">
          <span className="sl-ui-query-drawer-title">高级筛选</span>
          <Button
            variant="ghost"
            size="sm"
            onClick={onClose}
            aria-label="关闭"
            className="sl-ui-query-drawer-close"
          >
            ✕
          </Button>
        </div>

        {/* Body: iterate schema.fields (excluding defaultField) to render all conditions */}
        <div className="sl-ui-query-drawer-body">
          {filterableFields.map((f) => {
            const op = f.operators[0] ?? 'eq';
            const value = values[f.key] ?? defaultValue(op);
            const condition: QueryCondition = {
              field: f.key,
              operator: op,
              reversed: false,
              value,
            };
            return (
              <QueryConditionRow
                key={f.key}
                condition={condition}
                fieldSpec={f}
                references={references}
                onChange={(c) => updateValue(f.key, c.value)}
              />
            );
          })}
        </div>

        {/* Footer */}
        <div className="sl-ui-query-drawer-footer">
          <Button variant="ghost" size="md" onClick={clearAll}>
            清空
          </Button>
          <Button variant="primary" size="md" onClick={handleApply}>
            应用
          </Button>
        </div>
      </div>
    </>
  );
}

export default QueryDrawer;
