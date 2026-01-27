# State + Dirty Flags (Shared)

Source of truth (data): `data/travel-plan.json` → `cascade_state`.

## Destination process state

- `destinations.{slug}.{process}.status` indicates workflow state (`pending`, `researched`, `selected`, `skipped`, etc).
- `destinations.{slug}.{process}.updated_at` is set to a single ISO timestamp for the whole operation.

## Dirty flags

- Mark `cascade_state.destinations.{slug}.{process}.dirty = true` when a user action changes upstream inputs for that process.
- A cascade run clears the dirty flag(s) that *caused* triggers to fire (so re-running is idempotent).

## Package select convention

When a package offer is selected:

- Set `destinations.{slug}.process_3_4_packages.selected_offer_id`
- Set `destinations.{slug}.process_3_4_packages.results.chosen_offer`
- Mark `cascade_state.destinations.{slug}.process_3_4_packages.dirty = true`
- Run cascade to populate P3/P4.

