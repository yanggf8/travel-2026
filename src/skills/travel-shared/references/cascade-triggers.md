# Cascade Triggers (Shared)

Source of truth (data): `data/travel-plan.json` → `cascade_rules.triggers`.

## Common triggers

- `active_destination_change` — scoped to the active destination; typically resets downstream planning (often P5).
- `process_1_date_anchor_change` — scoped to all destinations; resets downstream processes affected by date changes.
- `process_2_destination_change` — scoped to current destination; resets downstream processes affected by destination changes.
- `process_3_4_packages_selected` — scoped to current destination; populates:
  - `process_3_transportation.flight`
  - `process_4_accommodation.hotel`

## Runner

Use `src/cli/cascade.ts` for dry-run/apply, and `src/cascade/runner.ts` for programmatic integration.

