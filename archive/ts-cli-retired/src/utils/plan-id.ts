/**
 * Plan ID / Destination Slug Converters
 *
 * Two identifier formats coexist:
 *   plan_id  = kebab-case ('tokyo-2026') — DB primary keys in all normalized tables
 *   dest_slug = snake_case ('tokyo_2026') — plan object keys, active_destination field
 */

/** Convert dest_slug (snake_case) → plan_id (kebab-case). */
export function toPlanId(slug: string): string {
  return slug.replace(/_/g, '-');
}

/** Convert plan_id (kebab-case) → dest_slug (snake_case). */
export function toDestSlug(planId: string): string {
  return planId.replace(/-/g, '_');
}
