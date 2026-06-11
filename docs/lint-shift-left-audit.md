# Lint shift-left + fail-proof audit — travel-cli

**Date:** 2026-06-12. Produced by a 17-agent audit workflow (map lints → write commands → adversarial verification → synthesis).

## Goal / principle
Writes currently store agent input **verbatim**; lints catch bad data **post-hoc**. Shift the cheap, single-row checks **into the write commands** (reject/normalize at write time) using the *same pure predicates the lints already use* — so lints become a **safety net** (still needed for the `db exec` raw-SQL backdoor + pre-existing rows), not the primary defense. This is the agent-first / fail-proof direction: a write should PREVENT bad data, not hope a later lint flags it.

## Enabling refactor (prerequisite for almost every item)
The reusable predicates are currently **module-private** in `validate_itinerary.rs` (`extract_map_urls`, `clean_stop`/`stop_link_problem`, `place_country`, `mentions_rail_or_bus`, `meal_has_pin`), and `validate_iso_date` is **duplicated** (`validate.rs:1063` chrono-based vs `set_activity.rs:1749` hand-rolled leap-year). **Extract these into one shared `pub(crate)` module (e.g. `src/checks.rs`)** — then each write-time guard is 1–3 lines.

## (1) Ranked shift-left + fail-proof changes (CONFIRMED only)

| # | Change | Command(s) | Lint | Keep lint too? | Effort | Impact |
|---|--------|-----------|------|----------------|--------|--------|
| **1** | **`validate_hhmm(&str)` + enforce `start <= end`** on every `--start`/`--end` at parse time. Raw `9am`/`25:00`/`""` are persisted verbatim today. | set-activity-time, add-activity, set-tod-time-range, set-flight (`--dep`/`--arr`) | day_conflicts, logical_order, business_hours, check_hours | **Yes** (overlap/order are cross-row) | M | **High** — a malformed time **silently drops the row out of every time lint** (`parse_time`→None), so bad data becomes invisible to validation. Defeats 4 lints at once. **DO THIS FIRST.** |
| **2** | **Reject embedded Maps URLs containing `&`** in activity titles (path-form has none; `&` = the `/maps/dir/` query form the dashboard truncates). | set-activity-title, add-activity, populate-itinerary | validate_map_links #1 (Warn) | Yes (db exec backdoor) | S | High — cheap single-string scan; the linkifier visibly breaks at first `&`. |
| **3** | **Validate route-segment stops** (`clean_stop`+`stop_link_problem`) before INSERT — reject empty-after-clean, retained `（）()＋`/clock-time, or mode-only (`步行`). | set-route-segment, set-route-segments-bulk | validate_map_links #2a (**Error**) | Yes | S | High — the only **Error**-severity map lint; made unreachable from this writer by construction. |
| **4** | **Cross-country guard** — reject when `place_country(from) != place_country(to)` (both known). | set-route-segment(s) | validate_map_links #2b (**Error**) | Yes | S | Med-High — blocks ocean-spanning ground routes (Okinawa↔Taiwan). Bundle with #3. |
| **5** | **Walking-vs-rail cross-check** — if `mode=='walking'` and `mentions_rail_or_bus(from+to)`, reject/warn. | set-route-segment(s) | validate_map_links #3 (Warn) | Yes | S | Med — `mode` is already enum-validated at this spot; natural extension. Bundle with #3/#4. |
| **6** | **Validate `--date`/`--booked-date` (ISO) + `--dep`/`--arr` (HH:MM) in set-flight** (validators already ship; set-flight just doesn't call them). | set-flight | (fail-proof gap) | n/a | S (after #1) | Med — `'2026-6-1'`/`'9:0'`/`'tomorrow'` persist silently today. |
| **7** | **De-dup `validate_iso_date` + add to `shaping-adopt`** (the 2nd, unguarded writer of `date_anchors`, `shaping.rs:839`). | shaping-adopt (new guard); set-dates/set-activity-booking (de-dup) | date_range/iso_date | Yes (+ cheap single-row lint) | M | Med — closes leap-year/error-text drift between the two copies and a real unguarded writer. |
| **8** | **WARN (never reject) when a sit-down meal lacks a pin** — suggest `<label>｜map:<place>`. Apply #2's `&`-URL rejection to meal URLs too. | set-meals | validate_map_links #5 (Info) | Yes (must stay) | S | Low-Med — **warn-only**: breakfast/conbini/in-flight meals are legitimately pinless; a hard gate would break DELETE+reINSERT re-runs. |
| **9** | **Validate `<date>` positional** (ISO) in offer commands before UPSERT/select. | update-offer, select-offer | (fail-proof gap) | n/a | S (after #7) | Low — malformed date writes a pricing row under a junk key instead of failing loud. |
| **10** | **Ergonomic: bound `<day>` vs `MAX(day_number)`** in `day_exists`/`require_session` — report "day N out of range — itinerary has M days". | all day-indexed writers | n/a | n/a | S | Low — pure message polish; fail-loud invariant already holds. |

**Excluded** (adversarial REJECTED as a write-time gate): `validate_map_links #4` (ambiguous bare-name stop) — Info heuristic with real false positives (famous 4-char landmarks); at most a non-blocking warn.

## (2) Keep as lint (genuine aggregate / whole-plan / temporal — cannot shift left)
- **check-maps-fresh / validate_maps_fresh_all_plans** — temporal: snapshot age vs `MAX(updated_at)`; the edit *is* what makes maps stale.
- **validate_day_packing** — whole-day aggregates (SUM >12h, COUNT >3/session, empty-day Info). Soft advisory.
- **validate_area_efficiency** — A→B→A bounce + DISTINCT-area sprawl over the full day sequence; `reorder-activities` only knows the id permutation.
- **validate_day_conflicts (overlap) / validate_logical_order (sorted-adjacency)** — pairwise cross-row over all timed activities. *(Their single-row slice — HH:MM validity, end>start — IS shifted left in #1.)*
- **check_hours** — date-aware (Closed-on-weekday needs `days.date→weekday`, date can change via set-dates). Whole-itinerary pre-trip audit.
- **validate_booking_deadlines** — time-relative (`book_by < today`) + cross-row. *(Optional cheap slice: warn at write when a new pending/booked gets a past `book_by`.)*
- **validate_destinations / reference_tables / ota_sources / holiday_calendars** — whole-table non-empty / referential completeness; several target tables have **no live writer at all** (nothing to shift into). Post-migrate health gates.
- **validate_completed_items** — docs↔filesystem drift over CLAUDE.md prose; no CLI write produces it.
- **validate_map_links #6 (meal reservation)** — cross-row join, self-clearing as bookings are added.

## (3) Highest-value first change
**Item #1** — shared `validate_hhmm` + `start <= end`, wired into set-activity-time, add-activity, set-tod-time-range, set-flight. It's the one gap that **actively corrupts the safety net**: a garbage time both renders wrong AND makes the activity invisible to the overlap/order/packing/business-hours lints (false "clean" validation). Mirrors the established strict `validate_iso_date` pattern, and the same helper is reused by #6 — so doing it first unblocks the cheapest follow-ups.

**Recommended sequence:** #1 → shared-helper extraction → #2 → route-segment bundle (#3+#4+#5, one command/INSERT site, retires the only Error-severity map lint by construction).

**Relevant files:** `set_activity.rs`, `set_tod.rs`, `set_route_segment.rs`, `set_flight.rs`, `shaping.rs`, `update_offer.rs`, `cascade/select_offer.rs`, `validate.rs` (`validate_iso_date`/`validate_date_range` `:1044-1085`), `validate_itinerary.rs` (pure predicates `:486`/`:505`/`:605`/`:664`/`:682`/`:689`/`:710`).
