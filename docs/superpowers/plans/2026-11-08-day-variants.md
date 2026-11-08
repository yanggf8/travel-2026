# Day Variants Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A day can carry alternative plans (variants) that every content command writes unchanged and the dashboard shows beside the main plan.

**Architecture:** A variant is a `days` row with `variant_of = <main day>` and its own day_number; all content tables key off that number as today. A short list of readers become trip-day-only or variant-aware (weather, populate, scaffold, maps, views, worker). Two parallel lanes: **Lane A** = CLI / DB (`rust/crates/**`, including verify-site), **Lane B** = dashboard worker (`workers/trip-dashboard-rs/**`) only. Lanes touch disjoint files and meet only at the contract in Global Constraints.

**Tech Stack:** Rust (travel-cli, travel-db on libsql/Turso), workers-rs (trip-dashboard-rs), wrangler R2.

**Spec:** `docs/superpowers/specs/2026-11-08-day-variants-design.md` — read it first; this plan does not repeat its rationale.

## Global Constraints (the inter-lane contract)

- Columns on `days`: `variant_of INTEGER` (NULL = trip day), `variant_label TEXT` (EN), `variant_label_zh TEXT` (ZH). Added with `db_migrate.rs` `add_column(...)` (idempotent).
- "Trip day" predicate everywhere: `variant_of IS NULL`.
- A variant's day_number = `MAX(day_number)+1` for plan+destination at creation; its `date`/`day_type` = its main day's.
- EN label: printable ASCII, 1–32 chars (it is drawn into PNG titles with the ASCII bitmap font). ZH label: 1–16 chars.
- Visible day identity of a variant: ZH 「第 {main} 天 · {label_zh}」, EN "Day {main} · {label}". The internal number never appears in visible text.
- PNG titles (`snapshot_maps::map_title`): variant day map "Day {main} route · {label}", logistics "Day {main} hotel round trip · {label}", away "Day {main} beyond the area · {label}". Trip days unchanged ("Day 3 route").
- Worker markup: main card `<section class="day day-{n}" data-day="{n}">` (add `data-day` to the existing card, `render/day.rs:201`); main option chip `<span class="day-option-label">`; variant `<details class="day-variant" data-variant-of="{main}" open>` → `<summary class="day-variant-summary">` → `<div class="day-variant-body">`.
- ZH chrome in `i18n.rs` keys: `dayVariant` = 「替代方案：{label}」 / "Alternative: {label}", `dayOption` = 「第 {day} 天 · {label}」 / "Day {day} · {label}".
- Delegates run NO git commands. "Hand back" ends each task; the coordinator reviews and commits.
- Real-Turso integration tests: per-run `zz_test_<nanos>_*` ids, RAII `Guard` armed right after the plan id is bound, `common::teardown_plan`, run in the background.
- R2 deletes go through one helper; test seam `TRAVEL_MAPS_DRY_DELETE=1` prints `would delete <key>` instead of running wrangler.

## Review Focus

- A variant of a NON-final day must not stop the final trip day getting weather (position mapping today) → Task A4 test.
- Removing a variant then adding a new one must not serve the old `day-{n}.png` → Task A2 test.
- swap-days of two trip days where one has a variant keeps the variant next to its content → Task A3 test.
- A plan with no variants renders byte-identical HTML to today → Task B2 test.
- A variant flagged `excursion=1` alone must produce no plan-excursion expectation → Task A5 test.

---

## Lane A — CLI / DB

### Task A1: Schema, trip-day helpers, invariants

**Files:**
- Modify: `rust/crates/travel-cli/src/db_migrate.rs` (add the 3 columns via `add_column`)
- Modify: `rust/crates/travel-db/src/repo/days.rs` (helpers)
- Modify: `rust/crates/travel-cli/src/validate.rs` (invariants in `validate data`; BLOCK in `validate publish`)
- Regenerate: `scripts/schema.sql` with the recipe in its own header comment (live `sqlite_master` dump; never hand-edit)
- Test: `rust/crates/travel-cli/tests/day_variants.rs` (new)

**Interfaces — Produces:**
- `repo::days::variant_parent(conn, plan, dest, day) -> Result<Option<i64>, String>` (None = trip day; Err when the day does not exist)
- `repo::days::variants_of(conn, plan, dest, main_day) -> Result<Vec<i64>, String>` (ascending)
- `repo::days::next_day_number(conn, plan, dest) -> Result<i64, String>` (`MAX+1`)

- [ ] Test `variant_invariants_block_publish`: seed a 2-day fixture plan, insert a days row with `variant_of=1` but `date` ≠ day 1's → `validate publish` exit ≠ 0 with a line containing `variant`; fix the date, blank `variant_label` → still blocked; give both labels to variant and main → that category passes. Also `variant_of` pointing at a variant → blocked.
- [ ] Run it (fails: columns missing). Implement migration + helpers + invariants. Run `./bin/travel db migrate` against Turso (idempotent) and the test (passes).
- [ ] Hand back.

### Task A2: `add-day-variant`, `set-day-variant-label`, `remove-day-variant`

**Files:**
- Create: `rust/crates/travel-cli/src/day_variant.rs` (the three commands)
- Create: `rust/crates/travel-cli/src/map_objects.rs` — move the R2 delete out of `snapshot_maps::remove_stale_map` (`snapshot_maps.rs:3213`) into `pub async fn purge_map_key(plan, key, reason) -> Result<bool, String>` (deletes the R2 object only when `map_artifacts.status='uploaded'`, then deletes that key's `map_artifacts` + `map_legend_stops` rows; honors `TRAVEL_MAPS_DRY_DELETE`). `remove_stale_map` keeps its current behavior by calling it and then recording `skipped`.
- Modify: `rust/crates/travel-cli/src/main.rs` (dispatch + `wants_help` usage lines, `reject_unknown_flags`), `docs/reference/CLI.md` (three entries)
- Test: `tests/day_variants.rs`

**Interfaces — Consumes:** A1 helpers. **Produces:** `pub async fn purge_day_maps(plan: &str, day: i64) -> Result<(), String>` in `map_objects.rs` (the three `day-{n}*.png` keys).

Behavior (spec §CLI): add creates the days row + 4 empty `timesofday` rows, sets main labels when `--main-label/--main-label-zh` given and REQUIRES them when the main day has none, calls `purge_day_maps` for the new number BEFORE inserting, prints `variant day {n} of day {main} created — write it with the usual commands on day {n}`. remove calls `purge_day_maps` first (abort on error, nothing deleted), then deletes the variant's rows from `activity_tags` (via its activities), `activities`, `session_meals`, `session_activities_zh`, `timesofday`, `day_route_segments`, `day_landmarks`, `days`; clears the main labels when no variants remain. Every command writes the audit triad (`plan_events` event `day_variant_added` / `day_variant_label_set` / `day_variant_removed` + `record_operation`).

- [ ] Tests: `add_variant_creates_row_and_sessions` (date/day_type copied, 4 sessions, labels on both rows, plans.version +1, an operation_runs row); `add_variant_requires_main_label_first_time`; `add_variant_refuses_a_variant_as_main`; `label_rules` (non-ASCII / 33-char EN label rejected); `remove_refuses_a_trip_day`; `remove_deletes_all_variant_rows_and_clears_main_labels`; `reused_number_purges_old_maps` (seed `map_artifacts` status uploaded + `map_legend_stops` for `day-{n}.png` of the number add will hand out, `TRAVEL_MAPS_DRY_DELETE=1` → stdout has `would delete …/day-{n}.png` and both tables have no rows for that key).
- [ ] Run (fail) → implement → run (pass). `cargo test -p travel-cli --lib` still green for `snapshot_maps`.
- [ ] Hand back.

### Task A3: swap-days with variants

**Files:** Modify `rust/crates/travel-cli/src/swap_days.rs`; Test `tests/day_variants.rs`

Rules (spec §CLI): allowed = main↔own variant, or two trip days; refused otherwise (error names both days and their `variant_of`). Labels swap with content (extend the theme swap at `swap_days.rs:184`). Trip-day swap re-points each side's variants: `variant_of` a→b / b→a and their `date`/`day_type` to the new main's. Emit the existing `days_swapped` event; add kv `variants_repointed`.

- [ ] Tests: `promote_variant_swaps_content_and_labels`; `trip_day_swap_carries_variants` (D2↔D4, D6 variant_of 4 → after: D6.variant_of=2, D6.date = D2's date, labels on D2); `swap_refuses_foreign_variant_and_two_variants`.
- [ ] Run (fail) → implement → run (pass). Existing `tests/swap_days.rs` + `swap_days_repoint_behavior_lock.rs` still pass.
- [ ] Hand back.

**Added 2026-11-08 (Yang: fix the stale bookings_current rows too).** Root cause: `bookings_current` is derived from the plan tables (activity booking keys embed day_number + session), but only `sync-bookings` / `mark-booked` refresh it, so `swap-days` left stale keys (kyoto-nov data already re-synced by the coordinator).
- Extract the body of `sync_bookings::run` (after arg parsing) into `pub async fn resync_plan_bookings(conn: &Connection, plan_id: &str, trip_id: &str) -> Result<usize, String>` in `sync_bookings.rs`; `run` calls it (output unchanged).
- `swap-days` calls `resync_plan_bookings` (trip_id = plan_id with '-'→'_') after the swap succeeds and prints `bookings re-synced (N)`.
- `remove-day-variant` (Task A2's command, already in the tree) calls it too after its deletes.
- `validate.rs:221` and `:233` drift warnings end with ` — run: travel sync-bookings --plan-id {plan_id}` so an agent can act in one step.
- Test `swap_days_resyncs_bookings`: fixture plan with a pending booking activity on day 1, run `sync-bookings`, then `swap-days 1 2` → `bookings_current` holds the key with `activity:2:` and none with `activity:1:` for that activity id.

### Task A4: Readers — weather, populate, scaffold, counts

**Files:** Modify `weather.rs` (`load_days` `:240`, both write loops `:125-145`, `:182-196`), `populate_itinerary.rs` (`read_days` `:648`, `allocate_clusters_to_days` `:397`), `travel-db/src/repo/itinerary.rs` (`replace_skeleton` `:31-59`), `compare_content_depth.rs` (`:56`, `:91`, `:96`), `validate.rs` (`missing_weather_in_window` `:2440`, `count_days` `:2285`); Test `tests/day_variants.rs` + `tests/scaffold_itinerary_behavior_lock.rs`

- weather: load trip days only; write each forecast to the trip day whose `date == forecast.date` (no positional zip). Variant rows untouched.
- populate: automatic allocation over trip days; explicit `--assign cluster:<day>` may name a variant.
- replace_skeleton: also delete `day_route_segments`, `day_landmarks`, `map_legend_stops` (`map_key LIKE 'day-%'`) for plan+dest.
- content-depth denominators, `count_days`, weather-window gate: trip days only. ZH / map-coverage / itinerary checks keep including variants.

- [ ] Tests: `weather_matches_by_date_with_variant_of_non_final_day` (3-day plan + variant of day 1 → after a weather write via the repo path with a stub forecast list keyed by date, day 3 gets its forecast, variant row's weather stays NULL — use the existing forecast-write seam in `weather.rs`; if none exists, extract `fn assign_forecasts(days: &[(i64,String)], f: &[Forecast]) -> Vec<(i64, &Forecast)>` and unit-test it); `populate_skips_variants_in_round_robin`; scaffold behavior-lock extended: route segments + landmarks + legends gone after a forced re-scaffold; `content_depth_ignores_variants`.
- [ ] Run (fail) → implement → run (pass).
- [ ] Hand back.

### Task A5: snapshot-maps, excursion predicates, PNG titles

**Files:** Modify `snapshot_maps.rs` (multi-day assemblies for plan / plan-excursion / plan-airport; `query_route_endpoints` `:3076`; `query_days` `:2075`; `map_title` `:436` + callers `:393`, `:3368`, `:3393`), `travel-db/src/repo/itinerary.rs` (`has_plan_excursion` `:1137`, `excursion_days` `:1150`), `check_maps_fresh.rs` (`:328-352`), `verify_site.rs` (`:1058` + tests `:1413/1430/1476`); Test: unit tests in `snapshot_maps.rs`, `tests/day_variants.rs`

**Interfaces — Produces:** `pub(crate) fn map_title(key: &str, variant: Option<(i64, &str)>) -> Option<String>` — `variant = (main_day, en_label)` for a variant's day keys, None otherwise. verify_site looks up the day row for a `day-{n}*` key and passes the same tuple.

- Variants: day maps generated as for any day; excluded from every multi-day map and from plan-logistics endpoints; both excursion predicates trip-day-only; check-maps-fresh expected slots = day slots for every row, multi-day slots from trip days.

- [ ] Tests: `map_title_is_parent_aware` (`("day-6.png", Some((4,"Sunny-day plan")))` → `"Day 4 route · Sunny-day plan"`, logistics/away likewise, `("day-3.png", None)` → `"Day 3 route"`); `variant_stops_stay_off_overview_and_logistics` (fixture with a variant whose stops are unique → plan.png / plan-logistics point lists exclude them; use the assembly functions with a stub resolver as existing tests do); `variant_only_excursion_expects_no_plan_excursion`.
- [ ] Run (fail) → implement → run (pass). Full `cargo test -p travel-cli --lib` green.
- [ ] Hand back.

### Task A6: CLI views grouped

**Files:** Modify `rust/crates/travel-cli/src/plan.rs` (`load` `:427-518`: `PlanView.days` = trip days, new field `variants: Vec<DayView>` on each day), `view_itinerary.rs` (`:91-93`, `:196`, `:231`, `:504`), `view_bookings.rs` (`:238`), `view_transport.rs` (`:107`), and the `status --full` path; Test `tests/day_variants.rs`

- Output: under a main day, `  ↳ 替代方案 [晴天方案] (day {n})` followed by its content in the existing per-day format, indented; trip length counts trip days; bookings/transport list variant items under the main day prefixed `[{label_zh}]`.

- [ ] Tests: `itinerary_groups_variant_under_main` (header day count = trip days; variant label line under day 1; no `Day {n}` header for the variant); `bookings_label_variant_items`.
- [ ] Run (fail) → implement → run (pass).
- [ ] Hand back.

### Task A7: verify-site viewer assertions (Lane A — same crate as A1–A6; depends on B2's markup contract only)

**Files:** Modify `rust/crates/travel-cli/src/verify_site_page.rs` (page checks near `:71`, `:138`, `:158`, slot rules `:347`)

- New checks on the viewer page: exactly one `.day[data-day]` per trip day; each `details.day-variant[data-variant-of=N]` is inside the card with `data-day=N` and has `open`; its summary text is non-empty; no visible text `第 {n} 天` / `Day {n}` for a variant's internal number. Slot rules unchanged otherwise (variant numbers are already in `SlotFacts.day_numbers`).

- [ ] Unit tests on HTML fixtures written from the Global Constraints markup: `variant_page_passes`, `variant_outside_its_card_fails`, `collapsed_variant_fails`, `internal_number_visible_fails`.
- [ ] Run `cargo test -p travel-cli --lib verify_site_page` (fail) → implement → pass.
- [ ] Hand back.

---

## Lane B — Dashboard worker

### Task B1: Load, attach, map slots

**Files:** Modify `workers/trip-dashboard-rs/src/router.rs` (days query `:847`; `check_map_status` / `map_status_keys` `:680-730`), `model.rs` (`Day` `:55`; plan build `:583-620`)

**Interfaces — Produces:** `model::Day { variant_of: Option<i64>, variant_label: String, variant_label_zh: String, variants: Vec<Day>, .. }`; `plan.days` = trip days, each `variants` ascending. `map_status_keys(plan_id, days)` walks main then its variants, so `MapStatus.days` / `day_logistics` / `day_away` hold variant numbers too.

- [ ] Tests (inline, host-testable like `model.rs:1174`): `variant_rows_attach_to_main_day` (rows for days 1,2 + 3 with variant_of=1 → `plan.days` = [1,2], day 1 `.variants` = [3] with its own sessions/meals/routes/landmarks); `map_status_keys_include_variants` (order: 4 overview keys, then day-1 ×3, day-3 ×3, day-2 ×3).
- [ ] Run `cd workers/trip-dashboard-rs && cargo test` (fail) → implement → pass.
- [ ] Hand back.

### Task B2: Render

**Files:** Modify `render/day.rs` (`render_with_logistics` `:182`, heading `:205-213`), `render/map.rs` (captions/alt `:280/313/326`, legend heading `:117`), `render/alerts.rs` (`:186-229`), `i18n.rs`, `styles.css`

- One content renderer for main and variant (order: maps → landmarks → route block → sessions); weather strip + clothing tip only on the main card; markup/classes/data attributes and i18n keys exactly as Global Constraints; captions/alt for a variant's maps use `dayOption`; alerts traverse main + variants, classify + extract the Maps URL from the unchanged title, then prefix `dayOption｜` (main option too when the day has variants); EN label empty → ZH label.
- CSS: `.day-variant { grid-column: 1 / -1; min-width: 0; max-width: 100%; }`, one inner inset, summary wraps, visible marker + focus ring; no horizontal overflow at 320px / 375px.

- [ ] Tests: `no_variant_plan_html_unchanged` (render a fixture plan with no variants; compare to the output of the pre-change renderer captured as a committed snapshot string in the test module); `variant_block_renders_open_under_main` (one `class="day"` per trip day, `data-variant-of="1"` inside day 1's card, `open`, label text, no `第 3 天` anywhere); `variant_map_caption_uses_main_day`; `alerts_prefix_variant_label`; `?lang=en` variants of the last two.
- [ ] Run (fail) → implement → pass.
- [ ] Hand back.

---

## Integration (coordinator)

- [ ] Review each lane line by line; backend review Muse (K3.1 Pro when Muse is out of quota), frontend review Codex; corroborate every finding against source.
- [ ] `make build`, full `make test` in the background, worker `cargo test`; commit explicit pathspecs; push.
- [ ] Deploy the worker; `verify-site` all plans (0 FAIL).
- [ ] kyoto-nov-2026 Day 4: main labels 雨天方案 / Rainy-day plan; `add-day-variant 4 --label "Sunny-day plan" --label-zh 晴天方案`; author Ohara (taxi direct) content with `--recommended`, meals with backups, route legs; snapshot-maps 0 ⚠; validate publish 0 blockers; verify-site PASS.
