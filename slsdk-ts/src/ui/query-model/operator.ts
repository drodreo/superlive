// QueryOperator: the supported operator enum
// Serialization: JSON string values use snake_case (e.g. 'eq' / 'contains' / 'is_null').
//
// All negations go uniformly through the QueryCondition.reversed = true flag (no separate operators defined),
// keeping the operator set minimal (9 members).

export type QueryOperator =
  | "eq" // equal (reversed = ne)
  | "lt" // less than (reversed = gte)
  | "lte" // less than or equal (reversed = gt)
  | "gt" // greater than (reversed = lte)
  | "gte" // greater than or equal (reversed = lt)
  | "between" // range (inclusive of both ends, reversed = not_between)
  | "contains" // string contains (reversed = not_contains)
  | "in" // set membership (reversed = not_in)
  | "is_null"; // field IS NULL (reversed = is_not_null)
