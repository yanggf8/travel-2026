# Plan 2: OTA resolver — source onboarding

**Date:** 2026-07-01 · **Status:** READY TO BUILD, CAPTURE-GATED.
**Design:** `docs/superpowers/specs/2026-07-01-ota-resolver-extension-design.md` (Plan 2 source onboarding).
**Builds on:** `docs/plans/2026-07-01-ota-resolver-plan1-contract-mechanics.md`.
**Scope:** data onboarding only. No resolver/schema/command changes expected.
**Pipeline:** Codex wrote this plan → Claude reviewed + corroborated → Codex corroborated (below) →
Codex writes tests → Grok writes impl (travel4u seed + test scaffolding) → Claude verifies. The
google_flights/agoda live captures are agent-first work Claude drives (Chrome + gwebcdb), not a delegated
code step.

**Corroboration (Codex + Claude, 2026-07-01):** every core claim CONFIRMED vs source — seeds lack
travel4u/google_flights/agoda; helpers already wired (no new helper); product_type_inputs contracts exact;
the google_flights/agoda templates map onto their flight/hotel contracts with NO resolver code change
(aliases produce checkin←depart, adults←pax, depart_date/return_date via `caller_value`); the ambiguity
hazard is real (agoda `hotel_slug` MUST be `input_name='hotel'` only). Two refinements folded into the
guardrails below: (a) `resolve_url` does NO URL-encoding, so captured url_values must be seeded already
URL-safe; (b) the seed-splitter's hard rule is `;` (comments and value literals), apostrophes softer.

## Goal
Seed reproducible OTA workflow + URL-param rows for the next sources, with no invented provider values:
`travel4u/group_tour` immediately, then `google_flights/flight` and `agoda/hotel` only after live capture
proves their real URL-param values. The final acceptance target is `ota run --capture-only` resolving and
capturing all 6 seeded sources: `besttour`, `settour`, `eztravel`, `travel4u`, `google_flights`, `agoda`.

## Scope decision: Option A vs Option B

**Recommendation: Option A — Plan 2 includes live capture discovery for `google_flights` + `agoda`, then
seeds and tests all 6 sources.**

Reasoning:
1. The approved design says Plan 2 onboards `travel4u`, `google_flights`, and `agoda`; Option B would only
   close the already-known `travel4u` seed gap and defer the hard honest-seed problem to another plan.
2. The repo already treats gwebcdb + WSLg Chrome as the standing OTA capture backend (`CLAUDE.md` URL
   Routing), and `rust/crates/travel-cli/tests/ota_run_capture_only.rs` is already a gated real-Turso +
   browser test with clean skips for missing creds, Chrome, or `TURSO_URL`/`TURSO_TOKEN`.
3. The capture step is not new resolver machinery. It is the data-provenance gate needed before committing
   `ota_source_url_param` rows for values the repo does not currently know.

What can be seeded with known-good values now:
- `travel4u/group_tour` workflow:
  `https://www.travel4u.com.tw/group/area/{area_code}/japan/`, `capture_url_contains='group/area'`,
  `settle_ms=25000`.
- `travel4u/group_tour` URL-param:
  `area_code / destination / tokyo / 41`.

What needs live capture first:
- `google_flights/flight` URL-param `dest / destination / tokyo / <observed dest>`. The template is known,
  but the real `{dest}` value is not; the note says this is a natural-language query value, not necessarily
  an airport code or the internal slug `tokyo`.
- `agoda/hotel` URL-params:
  `hotel_slug / hotel / <observed hotel input> / <observed hotel_slug>`,
  `city_slug / destination / tokyo / <observed city_slug>`,
  `country / destination / tokyo / <observed country>`.
  These must come from one concrete live Agoda hotel URL, not from retired examples or placeholder names.

Option B fallback:
If Chrome/site behavior prevents proving `google_flights` or `agoda` during implementation, do not commit
their URL-param seed rows. Land only the `travel4u` seed + 4-source reproducibility test, and explicitly
mark `google_flights`/`agoda` deferred. Do not merge guessed slugs to keep the 6-source test green.

## Corroborated source facts to preserve
1. Current names are `ota_source_url_param` and `set-ota-url-param`, not the old `ota_source_url_token`
   naming. The migration helper `migrate_ota_source_url_token_to_url_param` preserves old live rows, and
   `seed_ota_url_param()` embeds `scripts/seed/ota_source_url_param.seed.sql`.
2. Seed helpers already exist in `rust/crates/travel-cli/src/db_migrate.rs`: `seed_ota_workflow`,
   `seed_ota_url_param`, `seed_product_type_inputs`, and `run_seed_file_stmts`. Plan 2 should not add a
   helper.
3. `scripts/seed/ota_source_workflow.seed.sql` currently has `settour`, `eztravel`, `besttour`; it does
   not have `travel4u`, `google_flights`, or `agoda`.
4. `scripts/seed/ota_source_url_param.seed.sql` currently has destination rows for `besttour`,
   `settour`, and `eztravel`; it does not have `travel4u`, `google_flights`, or `agoda`.
5. `product_type_inputs` already declares the contracts:
   `flight` = `destination` token_key + `depart`/`return`/`origin`/`currency` common;
   `hotel` = `destination` + `hotel` token_keys + `depart`/`nights`/`pax`/`rooms`/`currency` common;
   `group_tour` = `destination` token_key.

## Work items (dependency-ordered)

### 1. Seed `travel4u/group_tour`
- Files: `scripts/seed/ota_source_workflow.seed.sql`, `scripts/seed/ota_source_url_param.seed.sql`.
- Symbols already wired: `db_migrate.rs::seed_ota_workflow`, `db_migrate.rs::seed_ota_url_param`,
  `db_migrate.rs::run_seed_file_stmts`.
- Change: add one workflow seed row:
  `travel4u`, `group_tour`, `get`,
  `https://www.travel4u.com.tw/group/area/{area_code}/japan/`,
  `capture_url_contains='group/area'`, `settle_marker=NULL`, `settle_ms=25000`, note with no apostrophe.
- Change: add one URL-param seed row:
  `source_id='travel4u'`, `product_type='group_tour'`, `url_param_name='area_code'`,
  `input_name='destination'`, `input_value='tokyo'`, `url_value='41'`.
- Done-check: after `travel db migrate`, `ota_source_workflow` has `travel4u/group_tour`, and
  `ota_source_url_param` has `travel4u/group_tour/area_code/destination/tokyo -> 41`.

### 2. Capture-gate `google_flights` and `agoda` values
- Files: no repo file changes in this step; evidence comes from live `captures` rows through gwebcdb.
- Commands/workflow: from `~/b/gwebcdb`, export `TURSO_URL`/`TURSO_TOKEN`, run
  `./scripts/start-chrome-cdp-wslg.sh`, navigate/capture with `bridge/navigate.py` and
  `bridge/ota_capture.py`, then inspect `captures.url` and `captures.raw_text` through `./bin/travel db exec`.
- `google_flights`: prove the exact destination query value for internal destination `tokyo`. Accept only
  a capture whose final URL/raw text shows a valid Google Flights results page for the intended
  TPE-to-Tokyo date window and currency. Record the observed `dest` URL value and capture ID.
- `agoda`: prove one concrete real hotel page. Extract `hotel_slug`, `city_slug`, and `country` from the
  final Agoda URL path, and choose the internal `--hotel` input value from the observed hotel identity.
  Accept only a capture whose raw text shows the hotel page settled for the intended check-in/nights/pax.
- Done-check: implementation notes or PR description list the capture IDs, final URLs, and exact extracted
  values. No seed row for either source is written before this evidence exists.

### 3. Seed `google_flights/flight`
- Files: `scripts/seed/ota_source_workflow.seed.sql`, `scripts/seed/ota_source_url_param.seed.sql`.
- Change: add workflow seed row for:
  `https://www.google.com/travel/flights?q=Flights+to+{dest}+from+{origin}+on+{depart_date}+through+{return_date}&curr={currency}&hl=zh-TW`.
  Use a stable `capture_url_contains` observed during capture, expected to be `travel/flights` if confirmed.
- Change: add URL-param row:
  `google_flights / flight / dest / destination / tokyo / <observed dest>`.
- Done-check: `ota run --capture-only google_flights flight --destination tokyo --depart 2026-09-01 --return 2026-09-05`
  resolves with no `{...}` placeholders, uses DB/default `origin=TPE` and `currency=TWD`, and writes one
  `captures` row without writing offers.

### 4. Seed `agoda/hotel`
- Files: `scripts/seed/ota_source_workflow.seed.sql`, `scripts/seed/ota_source_url_param.seed.sql`.
- Change: add workflow seed row for:
  `https://www.agoda.com/{hotel_slug}/hotel/{city_slug}-{country}.html?checkIn={checkin}&los={nights}&adults={adults}&rooms={rooms}&currency={currency}`.
  Use a stable `capture_url_contains` observed during capture, expected to include `agoda.com` or `/hotel/`
  if confirmed.
- Change: add URL-param rows:
  `agoda / hotel / hotel_slug / hotel / <observed hotel input> / <observed hotel_slug>`,
  `agoda / hotel / city_slug / destination / tokyo / <observed city_slug>`,
  `agoda / hotel / country / destination / tokyo / <observed country>`.
- Done-check: `ota run --capture-only agoda hotel --destination tokyo --hotel <observed hotel input> --depart 2026-09-01 --nights 4 --pax 2`
  resolves with no placeholders, includes the observed Agoda path pieces, fills `checkin` from `depart`,
  fills `adults` from `pax`, fills `rooms=1` by code default, and fills `currency=TWD` by DB default.

### 5. Extend the 6-source acceptance test
- Files: `rust/crates/travel-cli/tests/ota_run_capture_only.rs`.
- Change: refactor `cases` from `(&str, &str, &[&str])` to a case struct or tuple that carries
  source, product_type, per-source CLI args, and expected URL fragments.
- Change: keep existing cases for `besttour`, `settour`, `eztravel`; add `travel4u`, `google_flights`,
  and `agoda` only after their seed rows exist.
- Done-check: with Turso creds, `TURSO_URL`/`TURSO_TOKEN`, and Chrome on `127.0.0.1:9222`, the test runs
  all 6 capture-only cases; without those prerequisites, it keeps the existing clean skips.

### 6. Extend seed-row schema assertions
- Files: `rust/crates/travel-cli/tests/ota_source_workflow_schema.rs`.
- Change: add expected workflow assertions for `travel4u`, `google_flights`, and `agoda`.
- Change: add expected URL-param assertions for:
  `travel4u area_code -> 41`, `google_flights dest -> <observed dest>`, and the three Agoda params.
- Change: keep `product_type_inputs` counts unchanged at `flight=5`, `hotel=7`, `fit=4`, `group_tour=1`.
- Done-check: `db migrate` followed by schema test proves a fresh DB reproduces all onboarded source rows.

### 7. Update the verification checklist truthfully
- Files: `docs/plans/2026-06-30-ota-source-verification-checklist.md`.
- Change: after Plan 2 capture-only onboarding, either tick rows only if the implementation also performs
  full `ota write-offers`, or leave them queued with a note that URL resolver onboarding is complete but
  Rust `write-offers` verification is still pending.
- Done-check: the checklist does not claim Rust-verified end-to-end unless the source passed the stricter
  checklist definition.

## Test plan
- `rust/crates/travel-cli/tests/ota_source_workflow_schema.rs`: assert seed schema still has the current
  `ota_source_workflow`, `ota_source_url_param`, and `product_type_inputs` columns and PKs; assert the new
  workflow and URL-param seed rows.
- `rust/crates/travel-cli/tests/ota_run_capture_only.rs`: raise the seeded capture-only cases from 3 to 6
  under Option A. Keep `common::Guard` cleanup for produced jobs/captures. Keep skips for missing Turso
  creds, missing Chrome remote debugging, and missing gwebcdb `TURSO_URL`/`TURSO_TOKEN`.
- Existing resolver unit/integration tests from Plan 1 should not need changes except expected source count.
  No resolver/schema command tests should be added for Plan 2 unless a Plan 1 gap is discovered.
- Manual pre-merge proof: run `travel db migrate`, run the two tests above, then run direct CLI smoke for
  the three new sources and inspect each `captures.url` for unresolved braces and expected fragments.

## Honest-seed guardrails
1. Only `travel4u` rows are allowed before live discovery, because its `area_code=41` is already verified
   in the brief and current catalog notes.
2. `google_flights` and `agoda` seed rows must be blocked until a live capture ID and final URL are recorded
   in the implementation notes or PR description.
3. Do not copy Agoda values from `archive/ts-cli-retired/tests/scrapers/test_parsers.py`; those are retired
   examples, not current live proof.
4. Do not seed placeholder-looking values such as `<observed_dest_value>`, `hotel_slug`, `city_slug`,
   `country`, `my-hotel`, or an airport code for Google Flights unless the capture proves that exact value.
5. Keep the schema test assertions literal after discovery. The test should fail if a future edit changes
   an observed URL-param value casually.
6. Seed files must obey `run_seed_file_stmts` (splits on `;` BEFORE stripping `--` comment lines,
   `db_migrate.rs:1583-1589`): one statement per line; **no `;` inside comments OR value literals** (the
   real hazard). An apostrophe in a comment breaks the fragment; an apostrophe in a value literal only
   breaks if unescaped — keep both out of comments, and escape/avoid `'` in values.
7. **URL-safety (Codex corroboration — resolver does NO encoding).** `resolve_url` (`ota/run.rs:43-47`)
   substitutes url_values VERBATIM, no percent/URL encoding. So every captured url_value must be seeded
   in its **URL-safe form exactly as it appears in the working captured URL** — e.g. google_flights
   `{dest}` in the `Flights+to+{dest}` slot must be the `+`-joined/encoded token the live URL used, never
   a raw string with a space; agoda `{city_slug}`/`{country}`/`{hotel_slug}` must be the exact path
   segments from the final Agoda URL. Extract the value FROM the resolved capture URL, not from prose.

## Gaps / risks
- Live capture is environmental and site-dependent. The test already has credless/Chrome/gwebcdb skips, but
  the seed discovery itself is a real browser step and can fail for site changes, consent screens, or bot
  friction.
- `agoda` has one hotel-keyed placeholder and two destination-keyed placeholders. A mistaken
  `hotel_slug/destination/tokyo` row would create the ambiguity branch already tested in
  `ota_run_capture_only.rs`; seed only `hotel_slug/input_name='hotel'`.
- `agoda` `{country}` is a destination-keyed provider URL field, not a new common `country` input. Do not
  widen `product_type_inputs`; seed `country / destination / tokyo / <observed country>`.
- `google_flights` `{dest}` is source-owned placeholder spelling for the `destination` token_key. Do not
  add a `dest` product-type input.
- Template mapping is otherwise clean:
  `travel4u {area_code}` → `group_tour.destination`;
  `google_flights {dest}` → `flight.destination`, with `{origin}`, `{depart_date}`, `{return_date}`,
  `{currency}` covered by common inputs/defaults;
  `agoda {hotel_slug}` → `hotel.hotel`, `{city_slug}`/`{country}` → `hotel.destination`, and
  `{checkin}`/`{nights}`/`{adults}`/`{rooms}`/`{currency}` covered by common inputs/aliases/defaults.
- The verification checklist has a stricter meaning than capture-only resolver acceptance. Do not mark a
  source Rust-verified there unless it also completes `ota write-offers`.
