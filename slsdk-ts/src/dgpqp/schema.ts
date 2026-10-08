// QuerySchema: describes which fields and operators an entity supports
//
// Shared-facility edition (derived from the superlive ui/src/dgpqp/schema.ts componentization build-out):
// - placeholder/description removed (i18n protocol fields, hardcoded Chinese in v1, no consumers inside the facility)
// - `reference.entity` widened to string (architecture.md "ReferenceEntity injection table":
//   reference-tier data sources are no longer decided by a built-in facility mapping; consumers inject them via
//   `references?: Record<string, ReferenceSpec>`; the schema only declares reference keys,
//   and the facility stays unaware of concrete entities)
// - type tiers: string / number / datetime / boolean / enum / reference;
//   boolean has no dedicated input (falls to the default TextInput, original behavior)
// - Business conditions fixed for the entity lifetime are injected via QueryListToolbar.fixedExtra,
//   **never expressed in the schema** (keeps the schema decoupled from business rules)
//
// Field keys are wire protocol names (camelCase, matching the core-side ColumnResolver match strings),
// labels are display copy (hardcoded Chinese).

import type { QueryOperator } from './operator';

export interface QuerySchema {
  fields: QueryFieldSpec[];
  defaultField: string; // unique, used for quick search (the main search box)
}

export interface QueryFieldSpec {
  key: string;
  label: string;
  /** Field type: decides how values are parsed and which input control renders */
  type: 'string' | 'number' | 'datetime' | 'boolean' | 'enum' | 'reference';
  /** The operator subset this field supports, decided by the schema author (the drawer presets operators[0]) */
  operators: QueryOperator[];
  /** Enum candidate values when type='enum' */
  enumValues?: string[];
  /** Reference entity key when type='reference' (a key of the consumer's references injection table;
   *  when the consumer has not registered that key, the facility warns in dev mode and empties the dropdown) */
  reference?: { entity: string };
}
