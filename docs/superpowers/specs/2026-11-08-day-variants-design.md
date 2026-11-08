# Day variants — two plans for one day, both on the dashboard

Status: REVIEWED DRAFT (2026-11-08) — self-review + Codex (frontend, 8 findings) + K3.1 Pro (backend, 7 findings; Muse out of quota) folded in; APPROVED by Yang 2026-11-08. Plan: docs/superpowers/plans/2026-11-08-day-variants.md.

## Intent

Yang wants a day to carry more than one complete plan — e.g. kyoto-nov-2026 Day 4
(Sun 11/15): a rainy-day plan near Kyoto Station AND a fine-weather plan (Ohara by
taxi) — and wants the dashboard to show **both at the same time**, so the choice can
be made on the day without asking anyone. Today a day holds exactly one plan; the only
"backup" the data can express is a second meal line or a sentence in a focus note.
The fine-weather option for 11/15 currently lives only in a chat.

## Goals

1. A day can have one or more **variants**: full alternative plans with the same
   building blocks a day already has — theme, four sessions (focus / transit / time
   range), activities, meals, route segments, landmarks, and its own day map.
2. Every existing content command works on a variant unchanged (add-activity,
   set-meals, set-tod-*, set-route-segments-bulk, derive-routes, set-day-theme,
   set-place-geocode, snapshot-maps, check-hours …). An agent writes a variant exactly
   like a day.
3. The dashboard renders the main plan and its variants together under the one day:
   each with its label (雨天方案 / 晴天方案), its sessions, its route block and its map.
   SSR-only, ZH default, `?lang=en`.
4. Promoting a variant (the forecast flipped) is one command and keeps labels with
   their content.
5. Validators, verify-site and copy-plan treat variants as real content: same ZH /
   weather / map gates as any day.

## Non-goals

- No automatic weather-based switching. The dashboard shows both; the weather strip
  already says what the forecast is.
- No per-variant date, weather or day_type — a variant is the same calendar day.
- No nested variants (a variant of a variant).
- No change to how a plan with no variants looks.

## Anti-goals (would be a failure even if it "works")

- A variant shows up as an extra trip day ("第 6 天") anywhere — dashboard, plan map,
  day counts, content-depth denominators.
- A variant's stops leak onto plan.png / plan-excursion.png / plan-airport.png /
  plan-logistics.png (the trip overview must show the trip, not every alternative).
- A variant shifts or steals another day's data (weather by position, populate
  round-robin).
- A second parallel set of commands (`add-variant-activity` …) — duplication the agent
  has to learn.
- Hardcoded labels or trip text in worker code (labels are content → Turso).

## Design

### Data model — a variant is a `days` row that points at its main day

`days` gets three nullable columns (idempotent `ALTER TABLE ADD COLUMN` in
`db_migrate`):

| column | meaning |
|---|---|
| `variant_of INTEGER` | NULL = a real trip day; N = this row is a variant of day N (same plan + destination) |
| `variant_label TEXT` | EN label of this plan option ("Rainy-day plan") |
| `variant_label_zh TEXT` | ZH label ("雨天方案") |

A variant row has its own `day_number` (assigned as `MAX(day_number)+1` for the
plan+destination at creation), the main day's `date` and `day_type`, and its own
content rows in `timesofday` / `activities` / `session_meals` /
`session_activities_zh` / `day_route_segments` / `day_landmarks` keyed by that
day_number. Nothing else in the schema changes; no PK changes; no table rebuilds.

The labels live on BOTH sides: the main day carries the label of its own plan
(雨天方案) once it has variants, each variant carries its own (晴天方案). A label is
content, so it moves with the content on a swap (below).

Why a row and not a `variant` column on every day table: the alternative (add
`variant` to six PKs) needs table rebuilds and a `--variant` flag on every content
command, and any reader that forgets the filter silently mixes two plans. A variant
row reuses every writer as-is; the readers that must treat it differently are few and
listed below.

### Invariants (enforced by `validate data`, BLOCK in `validate publish`)

- `variant_of` points at an existing day of the same plan+destination whose own
  `variant_of` is NULL (no chains, no self-reference).
- A variant's `date` and `day_type` equal its main day's.
- A variant has non-empty `variant_label` and `variant_label_zh`; a main day that has
  variants has both labels too.

### CLI (all audited: plan_events + plans.version + operation_runs)

- `add-day-variant <day> --label <en> --label-zh <zh> [--main-label <en> --main-label-zh <zh>]`
  — creates the variant row (+ the four empty `timesofday` sessions scaffold creates),
  sets the main day's labels when given (required when the main day has none yet),
  prints the new day_number and the next step ("write it with the usual commands on
  day N"). Refuses a `<day>` that is itself a variant.
- `set-day-variant-label <day> --label <en> --label-zh <zh>` — on a main day or a
  variant.
- `remove-day-variant <variant_day>` — deletes the variant row and all its content rows
  (the same table set swap-days moves, + activity_tags, + its map_legend_stops). Refuses
  a day that is not a variant. When the main day has no variants left, its labels are
  cleared. (A user-invoked content command, like delete-activity — not an ad-hoc DB
  delete.)
- **Promote = `swap-days <main> <variant>`** — swap-days already swaps all content and
  keeps date/weather; it additionally swaps `variant_label(_zh)`. Allowed pairs:
  main ↔ its own variant, or two trip days. Refused: a variant with any day other than
  its own main, or two variants.
- **swap-days of two trip days carries their variants along** (self-review: variants
  belong to the content, not to the date). Swapping Day 2 ↔ Day 4 when Day 4 has
  variant D6 moves Day 4's content and labels to Day 2 AND re-points D6
  (`variant_of` 4→2, `date`/`day_type` → Day 2's), so the 晴天方案 stays next to the
  雨天方案 it is the alternative to. Without this the swap leaves D6 attached to the
  wrong content and Day 2's labels missing (an invariant break).
- **day_number reuse** (corrected after review — the first draft wrongly claimed
  freshness checks would hide a stale map; the worker serves whatever R2 holds, by
  size + ETag only, `router.rs:779`): `remove-day-variant` deletes the variant's
  `day-{n}.png`, `day-{n}-logistics.png`, `day-{n}-away.png` from the MAPS bucket and its
  `map_legend_stops` + `map_artifacts` rows BEFORE deleting the DB rows (same wrangler path snapshot-maps
  uses to drop obsolete objects); an R2 failure aborts the command with nothing
  deleted. The guarantee sits at ALLOCATION, not only removal (a forced scaffold also
  frees numbers): `add-day-variant` purges any existing `day-{n}*.png` objects and their
  `map_legend_stops` + `map_artifacts` rows for the number it is about to hand out, failing loud on an R2
  error. So a number handed out again by `MAX+1` never inherits an old map, whatever
  freed it. Test: create D6 + snapshot, remove, create D6 again → no map served for D6. (Content
  swaps leave maps stale until the next snapshot-maps — that is today's behavior for any
  day edit and is unchanged.)
- Read views: `itinerary` / `status` print variants under their main day
  (`Day 4 [雨天方案] … / ↳ variant D6 [晴天方案] …`), never as a separate trip day.

### Readers that must know about variants

| reader | rule |
|---|---|
| snapshot-maps | generates `day-{n}.png` (and day-logistics / day-away when the day rules call for it) for variants like any day; **excludes** variant days from plan.png, plan-excursion.png, plan-airport.png, plan-logistics.png (see the two rows below) |
| snapshot-maps `map_title` (drawn INTO the PNG, `snapshot_maps.rs:436`) + verify-site's title-strip sizing (`verify_site.rs` uses the same helper) | the title is parent-aware: a variant's `day-6.png` reads "Day 4 · Sunny-day plan route", never "Day 6 route". `map_title` takes the display day + EN label instead of parsing the key alone; verify-site calls it the same way |
| check-maps-fresh / verify-site map slots | the expected slot set mirrors snapshot-maps: variants have day slots, never multi-day slots |
| compare content-depth, plans/status day counts | count trip days only (`variant_of IS NULL`) |
| validate (ZH, map coverage, itinerary checks) | include variants — they are publishable content |
| validate weather-window gate | trip days only — the forecast belongs to the date, and the dashboard shows the main day's weather strip for all its plans |
| fetch-weather / weather refresh (`weather.rs:125-145`, `:182-196`) | **must change** (K3 review; the first draft wrongly said "unchanged"): today it takes the range from the first/last row and writes forecasts BY POSITION. Load trip days only (`variant_of IS NULL`) and match each forecast to `days.date` instead of by index. Variant weather columns stay unwritten (nothing reads them). Test: a variant of a non-final day must not stop the last trip day getting its forecast |
| populate-itinerary (`populate_itinerary.rs:397-434`, `:648-668`) | automatic round-robin allocation (full / arrival / departure) uses trip days only; an explicit `--assign cluster:<day>` may target a variant |
| scaffold-itinerary → `repo::itinerary::replace_skeleton` (`itinerary.rs:31-59`) | **must change**: its delete list misses `day_route_segments`, `day_landmarks` and `map_legend_stops` (an existing gap that variants make worse — a variant's routes would be orphaned). Delete every per-day content table, the same set swap-days moves |
| snapshot-maps `query_route_endpoints` / `assemble_logistics` (`snapshot_maps.rs:3076`, `:3130`) | trip-day route rows only — plan-logistics.png must not pick up a variant's hotel/airport endpoints |
| `repo::itinerary::excursion_days` + `has_plan_excursion` (`itinerary.rs:1137-1169`) | trip days only, both — the expected-map set (check-maps-fresh `:328`, verify-site) and the assembly must agree that a variant-only excursion produces no plan-excursion.png |
| CLI views on the shared `plan::load` (`plan.rs:427-518`): itinerary, status, bookings (`view_bookings.rs:238`), transport (`view_transport.rs:107`) | one grouped representation: `PlanView.days` = trip days, each with its `variants`; trip length = trip days; bookings and transport list variant items under their main day with the variant label |
| worker `check_map_status` (router.rs) | includes each variant's day slots, so the variant map renders and a missing one shows the usual "map missing" state |
| swap-days | see CLI above: labels swap; trip-day swaps re-point variants |
| copy-plan | unchanged: schema-driven, new columns copy automatically |
| derive-routes, check-hours, set-day-excursion | unchanged: per-day |

### Dashboard (trip-dashboard-rs)

Revised after Codex's frontend review (8 findings, all corroborated against source).

- **Model.** The days query (`router.rs:847`) selects the three new columns.
  `model::Day` gains `variant_of`, `variant_label`, `variant_label_zh`,
  `variants: Vec<Day>`. Build in two passes: assemble EVERY days row completely by its
  internal day_number (sessions, routes, landmarks — `model.rs:583`), then move each
  variant into its main day's `variants` (ordered by day_number). `plan.days` holds trip
  days only, so the outer card loop (`render/mod.rs:111`) stays trip-only.
- **Two identities.** The internal day_number (e.g. 6) is the storage key: map key
  `day-6.png`, `data-map-key`, legend lookup, content joins. Everything VISIBLE — the
  heading, map captions and alt text (`render/map.rs:280/313/326`), legend headings
  (`map.rs:117`), and the title drawn into the PNG — uses the main day's number + the
  localized label: 「第 4 天 · 晴天方案」.
- **One renderer.** Main and variant content go through the same content renderer in
  today's order (maps → landmarks → route block → sessions, `render/day.rs:214`), with
  today's empty / transit-only session rules. A day without variants renders
  byte-identical to today. The weather strip and clothing tip render once, on the main
  day. (Session time ranges are not rendered for any day today — `router.rs:852` does
  not load them; this design does not add them.)
- **Markup.** Main day with variants: a `.day-option-label` chip (雨天方案) under the
  heading. Each variant: `<details class="day-variant" open>` →
  `<summary class="day-variant-summary">替代方案：晴天方案</summary>` →
  `<div class="day-variant-body">` reusing `.theme`, `.day-map-stack`, `.route-block`,
  `.day-landmarks`, `.session` — no nested `.day` card / h2. CSS: `.day-variant` spans
  the full `.day` grid (`styles.css:1175` makes `.day` two-column on desktop),
  `min-width:0; max-width:100%`, one inner inset without doubling session padding;
  summary wraps, keeps the disclosure marker and keyboard focus. Page outer gutter is
  12px (`styles.css:38`), the day's inner gutter 16px — keep both; check 320px and
  375px widths for horizontal overflow without clipping.
- **Map slots — one contract for worker and verifier.** The worker's probe-key list
  (`router.rs:720`) and its result decoding (`router.rs:692`) iterate the same flattened
  main-then-its-variants sequence. Each variant gets its own three map versions and
  legends. Per day row (main or variant): `day-{n}.png` → exactly one figure (image or
  the missing-map frame); `day-{n}-logistics.png` / `day-{n}-away.png` → rendered only
  when uploaded. verify-site's `SlotFacts.day_numbers` keeps including variants (it
  already reads every row: `verify_site.rs:475`, `check_maps_fresh.rs:399`); plan-level
  maps stay single slots of trip-day content.
- **Alerts.** `render/alerts.rs:194` traverses main + variants. Classification
  (`is_meal_title`, `:200`) and embedded-Maps-URL extraction (`:203`) run on the
  unchanged activity title; the display title is then prefixed 「第 4 天 · 晴天方案｜」 —
  for the main option too when the day has variants, so two days' 「晴天方案」 stay
  distinguishable. Urgency, booking links and meal/non-meal placement unchanged.
  `session_meals` never create alerts (only activities carry booking metadata).
- **Companion options** (`render/mod.rs:146`, scans main-day activity notes): stays
  main-days-only; a note inside a variant renders locally only.
- **Chrome** strings (替代方案 / Alternative plan, the 「第 {day} 天 · {label}」 pattern) go in
  `i18n.rs`; labels come from Turso. A missing EN label falls back to the ZH label
  (validate blocks publishing without both, so this is only a draft-time fallback).
- **Viewer-page assertions (verify-site).** Variant blocks carry
  `data-variant-of="{main}"` and the main card `data-day="{main}"`; verify-site asserts:
  exactly one outer card per trip day, every variant under the right card, `open`
  present, the localized label shown, and no visible heading/caption with the internal
  number.

## Rollout

1. Implement + tests (CLI integration tests on real Turso with per-run fixtures and
   the RAII Guard; worker unit tests for build/attach and render).
2. Review: Muse (backend: migration, CLI, readers — K3.1 Pro when Muse is out of quota); Codex (frontend: worker render).
3. Deploy worker; verify-site all plans.
4. Use it: kyoto-nov-2026 Day 4 main = 雨天方案 (current content); add a 晴天方案
   variant (Ohara Sanzen-in / Jakko-in, taxi direct — Yang's earlier choice), meals with
   backups, route legs, map; snapshot-maps 0 ⚠; validate publish 0 blockers.

## Left to the builder

- Exact output wording of the new commands and the itinerary/status view.
- CSS of the variant block (must read as "same day, other option" and work at phone
  width).
- Whether `add-day-variant --copy-from-main` (seed the variant with the main day's
  content) is worth adding now — optional.
