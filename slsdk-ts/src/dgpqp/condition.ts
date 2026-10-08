// QueryCondition: a single filter condition
// Aligned with core/src/dgpqp/condition.rs as a cross-language protocol (copied verbatim from superlive ui).

import type { QueryOperator } from './operator';

export interface QueryCondition {
  field: string;
  operator: QueryOperator;
  /** true = negated (not) */
  reversed: boolean;
  value: QueryValue;
}

/** The three QueryValue shapes:
 * - single:   single-value comparison (eq / lt / gt / contains / in)
 * - range:    range comparison (between)
 * - set:      set membership (an alternative spelling of in)
 *
 * Serialization protocol (aligned with core/src/dgpqp/condition.rs):
 * - `type` tag field ("single" / "range" / "set")
 * - `data` content field (adjacent tagging)
 */
export type QueryValue =
  | { type: 'single'; data: QuerySingleValue }
  | { type: 'range'; data: { min: QuerySingleValue; max: QuerySingleValue } }
  | { type: 'set'; data: QuerySingleValue[] };

/** QuerySingleValue: aligned with core/src/dgpqp/condition.rs::QuerySingleValue */
export type QuerySingleValue = string | number | boolean | null;
