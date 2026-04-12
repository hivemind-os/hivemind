/**
 * Evaluate a field visibility condition against current form values.
 *
 * Condition format (from x-ui.condition in JSON Schema):
 *   { field: "other_field", eq: true }   — visible when other_field equals true
 *   { field: "other_field", neq: "" }    — visible when other_field is not empty string
 *
 * Returns true (visible) when there is no condition or when the condition is satisfied.
 */
export function evaluateFieldCondition(
  condition: { field?: string; eq?: any; neq?: any } | undefined,
  values: Record<string, any>,
): boolean {
  if (!condition || !condition.field) return true;

  const actual = values[condition.field];

  if ('eq' in condition) {
    return actual === condition.eq;
  }
  if ('neq' in condition) {
    return actual !== condition.neq;
  }

  // No comparator specified — treat as truthy check
  return !!actual;
}
