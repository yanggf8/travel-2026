# travel-cli mutation-command bugs — RESOLVED

Status as of 2026-06-11: **all three bug classes are fixed.** This file is kept as a
record of what was wrong and where the regression tests live. No action remaining.

Regression tests: `rust/crates/travel-cli/tests/set_mutation_bugs.rs` (real-Turso:
seed → run binary → SELECT → assert → teardown; skips cleanly without creds).

## Bug 1 — mutation UPDATEs silently succeeded when 0 rows matched — FIXED

`set-*` commands wrote via `UPDATE … WHERE plan_id=? AND destination=?`, discarded
`rows_affected`, and unconditionally printed "✅ updated" + wrote the audit triad
(`plan_events`, `plans.version` bump, `operation_runs` status `completed`). On a
hand-scaffolded plan with no offer-cascade skeleton row the UPDATE matched 0 rows, so
the command reported success while writing nothing.

Resolution — the invariant **"a `completed`/✅ audit implies a row actually changed"**
is now enforced two ways:
- **Booking-first commands upsert** (so 0-rows can't happen): `set-flight`,
  `set-hotel`, `set-airport-transfer` use `INSERT … ON CONFLICT(<pk>) DO UPDATE`.
- **Itinerary commands fail loud** when their parent row is missing: `set-day-theme`,
  `set-tod-*`, `set-route-segment`, `set-activity-*`, `add-activity`, `delete-activity`,
  `swap-days` guard with an existence check (`day_exists` / `require_session` /
  `find_activity` / `require_day`) and return a non-zero error writing NO `completed`
  audit row.

Covered by: `set_flight_persists_on_fresh_plan`, `set_hotel_persists_on_fresh_plan`,
`set_airport_transfer_persists_on_fresh_plan`, `set_day_theme_fails_loud_when_day_missing`,
`set_tod_focus_fails_loud_when_session_missing`,
`set_route_segment_fails_loud_when_day_missing`,
`set_activity_time_fails_loud_when_activity_missing`.

## Bug 2 — `--dest` advertised but rejected by the arg parser — FIXED

The usage string told users to pass `--dest <slug>`, `read_destination()` consumed it,
but the parser's catch-all `other if other.starts_with("--") => Err("unknown argument…")`
fired first. All `set-*` parsers now accept-and-skip `--dest` before the catch-all.

Covered by: `set_flight_accepts_dest_flag` (+ every persist test passes `--dest`).

## Bug 3 — `--plan-id` ignored / rejected by mutation commands — FIXED (2026-06-11)

Every mutation dispatch arm in `main.rs` resolved the plan as
`env::var("TRAVEL_PLAN_ID").unwrap_or_else(|_| "test-set-dates-2026".to_string())` —
so `--plan-id` did nothing (and 7 commands' strict parsers actively rejected it with
`unknown argument: --plan-id`), and with `$TRAVEL_PLAN_ID` unset **every mutation
silently defaulted to the real `test-set-dates-2026` plan**. This is what silently
misdirected hand edits to the wrong plan.

Resolution:
- All ~28 mutation dispatch arms now resolve the plan via
  `plan_resolver::resolve_plan_id(rest)` — the documented ladder
  (`--plan-id > $TRAVEL_PLAN_ID > --travel-date > --travel-start/--travel-end > active
  > upcoming > most-recent`). The hardcoded `test-set-dates-2026` default is gone; an
  unresolvable plan now fails loud instead of silently hitting the test plan.
- The 7 strict parsers (`set_flight`, `set_hotel`, `set_airport_transfer`,
  `set_day_theme`, `set_activity`, `set_tod`, `swap_days`) accept-and-skip `--plan-id`
  (it's consumed by the resolver), matching the `--dest` treatment.

Covered by: `set_flight_honors_plan_id_flag`, `set_route_segment_honors_plan_id_flag`,
`set_flight_fails_loud_for_unknown_plan_id`.

## Note — unrelated WIP in the tree

`set_activity.rs` carries an unwired `reorder-activities` feature (`run_reorder` /
`parse_reorder`, no `main.rs` dispatch arm). It compiles but is dead code; out of scope
for these bugs.
