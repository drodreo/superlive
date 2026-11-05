// QueryCondition: a single filter condition

import type { QueryOperator } from "./operator";

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
 * Serialization protocol:
 * - `type` tag field ("single" / "range" / "set")
 * - `data` content field (adjacent tagging)
 */
export type QueryValue =
  | { type: "single"; data: QuerySingleValue }
  | { type: "range"; data: { min: QuerySingleValue; max: QuerySingleValue } }
  | { type: "set"; data: QuerySingleValue[] };

/** QuerySingleValue: a scalar filter value */
export type QuerySingleValue = string | number | boolean | null;
