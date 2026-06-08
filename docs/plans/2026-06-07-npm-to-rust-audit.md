# Root npm to Rust Audit and Conversion Plan

**Date:** 2026-06-07  
**Decision:** Convert all root-level npm/TypeScript surfaces to Rust. Delete the root `package.json`
only after every in-scope script has a parity-proven Rust path.  
**Explicit exception:** `workers/trip-dashboard/` stays JavaScript/TypeScript because Cloudflare
Workers run on a JS runtime. Its `workers/trip-dashboard/package.json` is independent of the root
`package.json` and remains out of scope.

This supersedes `docs/plans/2026-06-05-rust-cli-migration.md` where it conflicts. That older plan
preserved npm as the user-facing interface. This plan removes root npm last, after Rust replacements
are complete.

## Scope

In scope:
- Root `package.json` and its 34 npm scripts.
- `src/cli/travel-update.ts` and the 33 modules under `src/cli/commands/`.
- `src/state/`, including `StateManager` and Turso repositories.
- `src/cascade/`.
- Root comparison/util CLIs in `src/cli/compare-*` and `src/utils/*`.
- Root TS scripts under `scripts/`: Turso migration/query/exec/status/import/sync/seed/fetch/validate.

Out of scope:
- `workers/trip-dashboard/`: independent package, independent npm scripts, Cloudflare Worker JS
  runtime. Do not convert this package to a Rust binary.
- `rust/crates/travel-scraper`: already the template for the target style: no root npm, no Python,
  no JSON boundary, Turso-backed, plain-text CLI, tiered token minting through vendored `turso-util`.
- `/home/yanggf/b/gwebcdb`: read-only reference only.

Target rules:
- Plain-text CLI output by default. Remove user-facing JSON output in the root CLI migration.
- Turso is the source of truth. No local data fallback.
- Credentials fail loud and resolve through `turso-util` minted tier tokens. Worker CF secret bindings
  remain separate and out of scope.
- Parity before deletion. No big-bang removal.

## Current Surface Summary

| Area | Current size | Notes |
|---|---:|---|
| Root npm scripts | 34 | All in root `package.json`; Worker scripts excluded. |
| `src/cli/travel-update.ts` | 125 LOC | Root travel command dispatcher. |
| `src/cli/commands/*.ts` | 5,043 LOC | 33 command modules, many with multiple command names. |
| `src/state/state-manager.ts` | 1,648 LOC | Main mutation/read orchestration surface. |
| `src/cascade/runner.ts` | 565 LOC | Cascade invalidation/execution core. |
| `src/state/*.ts` total | 7,327 LOC | Repositories, schemas, managers, command model, types. |
| `src/cascade/*.ts` total | 803 LOC | Runner, wildcard matching, types. |
| `src/cli/shared/*.ts` | 599 LOC | Arg parsing, plan resolution, shared validation/helpers. |
| `src/services/turso-service.ts` | 1,275 LOC | Shared TS Turso query/import service used by comparison and CLI code. |
| `scripts/turso-pipeline.ts` | 288 LOC | TS Turso client helper used by root scripts. |
| Root standalone scripts/utilities listed below | 5,619 LOC | `compare-*`, utility CLIs, DB scripts, validator. |

The hard core is not the shell wrapper. It is `StateManager` (1,648 LOC) + cascade runner (565 LOC)
+ the 33 command modules (5,043 LOC) + state/repository/schema code. That is the main risk and the
bulk of the migration.

## Root npm Script Audit

Difficulty scale:
- `trivial`: pure or near-pure function/formatting; no durable state mutation.
- `medium`: DB reads/writes or standalone CLI logic, but no full StateManager/cascade port.
- `hard`: StateManager/cascade/domain mutation surface.
- `special`: tooling replacement, not a direct app command.

| npm script | Category | Backing code + LOC | StateManager/cascade? | Difficulty | Target Rust mapping |
|---|---|---:|---|---|---|
| `typecheck` | dev/infra | `tsconfig.json` 17 + full TS graph | No | special | `cargo check --workspace` + `cargo clippy --workspace` |
| `test` | dev/infra | `vitest.config.ts` 20 + TS tests | Mixed | special | `cargo test --workspace` |
| `test:integration` | dev/infra | `vitest.config.ts` 20 + integration tests | Mixed | special | `cargo test --workspace --test ...` |
| `test:watch` | dev/infra | `vitest.config.ts` 20 | Mixed | special | Optional `cargo watch -x test`; not required for deletion |
| `test:coverage` | dev/infra | `vitest.config.ts` 20 | Mixed | special | Optional Rust coverage; not a blocker unless actively used |
| `hooks:install` | dev/infra | `scripts/hooks/pre-commit` 24 | No | special | Rust-aware hook or no root npm hook; install via checked-in hook doc/tool |
| `postinstall` | dev/infra | root `package.json` hook 52 | No | special | Delete with root package; no postinstall in target |
| `status` | travel-CLI command | `travel-update.ts` 125 + `status.ts` 242 + state/cascade core 2,213 | Yes | hard | `travel status --full` |
| `status:json` | travel-CLI command | same as `status` | Yes | hard | Remove JSON output; provide plain `travel status --full` |
| `status:ascii` | travel-CLI command | same as `status` | Yes | hard | `travel status` |
| `travel` | travel-CLI command | dispatcher 125 + commands 5,043 + state/cascade core 2,213 | Mixed, mostly yes | hard | `travel <subcommand>` |
| `view:status` | travel-CLI command | `status.ts` 242 + state/cascade core 2,213 | Yes | hard | `travel status --full` |
| `view:itinerary` | travel-CLI command | `itinerary.ts` 222 + state/cascade core 2,213 | Yes | hard | `travel itinerary` |
| `view:transport` | travel-CLI command | `transport.ts` 118 + state/cascade core 2,213 | Yes | hard | `travel transport` |
| `view:bookings` | travel-CLI command | `bookings.ts` 116 + state/cascade core 2,213 | Yes | hard | `travel bookings` |
| `view:prices` | travel-CLI command | `view-prices.ts` 268 + `turso-service.ts` 1,275 | No StateManager; DB reads | medium | `travel prices view` |
| `validate:data` | dev/infra | `scripts/validate-data.ts` 576 + `turso-pipeline.ts` 288 | No | medium | `travel validate data` |
| `doctor` | dev/infra | `scripts/validate-data.ts` 576 + `--doctor` | No | medium | `travel doctor` |
| `compare-trips` | comparison/util | `compare-trips.ts` 401 + leave calculator 330 | No | trivial/medium | `travel compare trips` — Phase 1 Rust-built, text-first input, TS deletion pending |
| `compare-dates` | comparison/util | `compare-dates.ts` 415 + `turso-service.ts` 1,275 | No StateManager; DB reads | medium | `travel compare dates` |
| `compare-true-cost` | comparison/util | `compare-true-cost.ts` 361 + `turso-service.ts` 1,275 | No StateManager; DB reads | medium | `travel compare true-cost` |
| `normalize-flights` | comparison/util | `flight-normalizer.ts` 293 | No | trivial/medium | `travel normalize flights` — Phase 1 Rust-built, rendered-text input, TS deletion pending |
| `leave-calc` | comparison/util | `leave-calculator.ts` 330 + `turso-service.ts` 1,275 holiday reads | No StateManager; DB reads | medium | `travel leave calc` — Phase 1 Rust-built, exact plain-text parity, TS deletion pending |
| `db:import:turso` | DB script | `import-offers-to-turso.ts` 540 + `turso-pipeline.ts` 288 | No | medium | obsolete for scraper path; otherwise `travel db import-offers` |
| `db:migrate:turso` | DB script | `turso-migrate.ts` 1,737 + `turso-pipeline.ts` 288 | No | hard | `travel db migrate` |
| `db:status:turso` | DB script | `turso-status.ts` 73 + `turso-pipeline.ts` 288 | No | trivial/medium | `travel db status` |
| `db:query:turso` | DB script | `turso-query-offers.ts` 318 + `turso-pipeline.ts` 288 | No | medium | `travel db query-offers` |
| `db:sync:bookings` | travel-CLI command / DB script | `turso.ts` 250 + StateManager/TursoRepository | Yes | hard | `travel bookings sync` |
| `db:query:bookings` | DB script / read command | `turso.ts` 250 + `TursoRepository` 267 | No StateManager; DB reads | medium | `travel bookings query` |
| `db:sync:destinations` | DB script | `turso-sync-destinations.ts` 16 + `turso-pipeline.ts` 288 | No | trivial/medium | `travel db sync destinations` |
| `db:sync:events` | DB script | `turso-sync-events.ts` 114 + `turso-pipeline.ts` 288 | Local-file bootstrap today | medium | `travel db sync events` or retire if obsolete |
| `db:seed:plans` | DB script | `seed-plans-current.ts` 52 + `turso-pipeline.ts` 288 | No | medium | `travel db seed plans` |
| `db:fetch:holidays:tw` | DB script | `fetch-taiwan-holidays.ts` 147 + `turso-pipeline.ts` 288 | No | medium | `travel db fetch holidays tw` |
| `db:exec` | DB script | `turso-exec.ts` 121 | No | trivial/medium | `travel db exec` |

## Root TS Script Heat Map

Hot path, used regularly:
- `src/cli/travel-update.ts` and command modules for status/views/mutations.
- `scripts/validate-data.ts` via hooks.
- `scripts/turso-status.ts`, `scripts/turso-query-offers.ts`, `scripts/turso-exec.ts`.
- `src/cli/compare-dates.ts`, `src/cli/compare-true-cost.ts`, `src/cli/commands/view-prices.ts`.
- `src/utils/leave-calculator.ts`, `src/utils/flight-normalizer.ts` when used as CLI tools.

One-shot or rare:
- `scripts/turso-migrate.ts`.
- `scripts/seed-plans-current.ts`.
- `scripts/fetch-taiwan-holidays.ts`.
- `scripts/turso-sync-destinations.ts`.
- `scripts/turso-sync-events.ts`.
- `scripts/import-offers-to-turso.ts` should shrink as the Rust scraper imports directly to Turso.
- Historical/bootstrap helpers not directly named in package scripts can be retired or ported only if
  still used by a current root command.

Tooling replacement:
- `tsc --noEmit` → `cargo check --workspace` and `cargo clippy --workspace`.
- `vitest` → `cargo test --workspace`.
- Pre-commit hook → cargo checks plus Rust `travel validate data`; no npm after root deletion.

## Target Shape

Primary binary:
- `travel`: replaces root `npm run travel`, status/view aliases, comparison utilities, DB scripts, and
  validation.
- Development path: `./rust/target/debug/travel ...`.
- Installed/release path after migration: `./bin/travel ...` or `cargo install --path rust/crates/travel`.

Existing separate binary:
- `travel-scraper`: keep as-is for CDP capture/parser/import work. It is already npm-free and should
  not be re-planned here. `travel` may later shell out to it or expose a thin alias, but the scraper
  remains an independently tested Rust crate.

Suggested workspace crates:
- `travel-domain`: Rust types and validation logic migrated from `src/state/types.ts`,
  `src/state/schemas.ts`, and command payload types.
- `travel-db`: Turso repository layer using vendored `turso-util`.
- `travel-cli`: command dispatch and plain-text rendering.
- `travel-cascade`: cascade runner and dirty-flag logic.
- `travel-tools`: comparison utilities and pure helpers if they are too large for `travel-cli`.

User invocation mapping:

| Current npm | Target |
|---|---|
| `npm run travel -- <cmd>` | `./bin/travel <cmd>` |
| `npm run status` | `./bin/travel status --full` |
| `npm run view:itinerary` | `./bin/travel itinerary` |
| `npm run compare-dates -- ...` | `./bin/travel compare dates ...` |
| `npm run db:migrate:turso` | `./bin/travel db migrate` |
| `npm run db:exec -- "<sql>"` | `./bin/travel db exec "<sql>"` |
| `npm run validate:data` | `./bin/travel validate data` |
| `npm run typecheck` | `cargo check --workspace && cargo clippy --workspace` |
| `npm test` | `cargo test --workspace` |

## The Core Problem

The largest risk is the StateManager/cascade domain port:
- `StateManager` is 1,648 LOC.
- `cascade/runner.ts` is 565 LOC.
- `src/state/*.ts` is 7,327 LOC.
- `src/cli/commands/*.ts` is 5,043 LOC across 33 modules.

This is stateful domain behavior, not syntax translation. It includes:
- Plan resolution and destination config loading.
- Turso repository reads/writes.
- Cascade dirty flags and process invalidation.
- Offer selection and booking synchronization.
- Itinerary/session/day/activity mutations.
- Plain-text formatting and user-facing command behavior.

Port strategy:
1. Create Rust types that mirror the persisted Turso schema and current command payloads.
2. Build a Rust repository façade with the same read/write operations that commands need.
3. Port read-only commands first behind the Rust repository.
4. Port mutation commands one at a time, using parity tests and real runs before deleting TS.
5. Port cascade last inside the mutation phase, because many commands depend on its side effects.

Do not attempt a big-bang StateManager rewrite. Put a Rust façade in front of the DB and migrate one
command family at a time.

## Phased Plan

### Phase 1 — Pure and Near-Pure Utilities

Scope:
- `compare-trips`.
- `normalize-flights`.
- `leave-calc` core date math, with Turso holiday lookup isolated behind `travel-db`.
- Pure helper pieces from `compare-dates` and `compare-true-cost`.

Why first:
- Lowest behavioral risk.
- Builds Rust CLI ergonomics and plain-text renderer conventions.
- Gives quick replacement wins without touching StateManager.

Deliverables:
- `travel compare trips`.
- `travel normalize flights`.
- `travel leave calc`.
- Unit tests for date/flight parsing and formatting.
- Output diff against existing TS commands on fixed inputs.

Progress as of 2026-06-07:
- New crate `rust/crates/travel-cli` added with binary name `travel`.
- `travel leave calc` reads holidays from Turso through vendored `turso-util` read-tier token minting
  and matches the TS plain-text output on the fixed 2026-06-20 to 2026-06-24 Taiwan range.
- `travel compare trips` is Rust-built using repeatable plain-text trip specs instead of the old
  JSON-only TS `--trips` surface. It preserves the TS summary/detail rendering and leave/cost math.
- `travel normalize flights` is Rust-built for rendered plain text via `--text` or `--stdin`; it does
  not preserve the old TS dependency on legacy `offers.raw_data` JSON.
- Phase 1 TS files are Rust-parity-verified for behavior/output shape, but TS deletion remains pending
  until user sign-off. Root `package.json` is unchanged.

Estimate: 2-4 days.

### Phase 2 — Read-Only Views

Scope:
- `status`, `status:ascii`, `view:status`.
- `view:itinerary`, `view:transport`, `view:bookings`, `view:prices`.
- `plans`, `query-destination-ref`, `query-offers`, `check-freshness`, `query-bookings`.
- Comparison commands with DB reads: `compare-dates`, `compare-true-cost`.

Why second:
- Exercises Turso read model without mutation risk.
- Forces the Rust repository layer to become real.
- Plain-text output can be diffed directly against TS.

Deliverables:
- `travel status`, `travel itinerary`, `travel transport`, `travel bookings`, `travel prices view`.
- `travel db query-offers`, `travel bookings query`, `travel plans`.
- Read-only Turso integration tests that skip-not-fail if credentials are absent.
- Same-input TS vs Rust output diff for at least one active plan and one historical plan.

Estimate: 1-2 weeks.

**Phase 2 progress — ✅ COMPLETE (2026-06-08):**
All 7 read commands are Rust-built with **byte-identical parity** vs the TS originals.
The Rust read-repository pattern is `db::connect_read` (turso-util read-tier minting) +
per-command module + plain-text formatter.
- ✅ `travel plans` — byte-parity incl. `window=` suffix. Commit d7a188a.
- ✅ `travel query-offers` — byte-parity with `printTursoOfferTable`, count-parity. Commit f2d09c0.
- ✅ `travel query-destination-ref` — byte-parity; reads de-JSON'd child rows (no serde_json). Commit 7f86763.
- ✅ `travel query-bookings` — byte-parity with `printBookingsTable`. Commit a184ad0.
- ✅ `travel check-freshness` — byte-parity (plan-provenance + legacy-offers paths). Commit a184ad0.
- ✅ `travel compare dates` — byte-parity (offers + holiday calendar + leave math). Commit eca4395.
- ✅ `travel compare true-cost` — byte-parity (offers + transport routes/hubs + hotel_area_keywords). Commit eca4395.
- ⚠️ The assembled-plan views (`status`, `view:itinerary`, `view:transport`, `view:bookings`) depend on
  StateManager/plan-assembler — they are **Phase 4 work**, not pure reads. Out of Phase 2 scope.
- TS for all 7 kept (parity-verified, deletion pending the package.json → Rust cutover). A non-destructive
  snapshot lives in `archive/ts-ported-phase2/`.

> **Bugs found & fixed during the port** (faithful parity surfaced latent TS bugs):
> `queryOffers` returned numeric fields as strings (`number | null` type lied), causing string-concat
> (`"17999"+409 → "17999409"`) and broken `.toLocaleString()`. Fixed at the source with `mapTursoOfferRow`
> numeric coercion (commit 51115cc); Rust quirk-replication removed. Also fixed the long-standing
> `best_value` shape mismatch + save write-back gap (c59e7a7) and the save-path child-table cleanup +
> duplicate-writer removal (ecfe501) — `StateManager.save()` for tokyo-2026 now returns SAVE_OK.

> **Prerequisite done — the RDB de-JSON program (6 batches A–F, 2026-06):** before/alongside this port,
> every JSON-encoded value was removed from Turso (44 `*_json` columns + 4 content-scan-found non-`*_json`
> ones) → child tables / typed columns / `*_text` fields. Whole-DB scan confirms 0 JSON values in 601 text
> columns. The misnamed event "log" was unified+renamed to the `plan_events` event store. `scripts/schema.sql`
> regenerated from the live DB (`scripts/gen-schema-sql.ts`). See memory `no-json-in-rdb`,
> `de-json-unknown-to-text-column`. Commits 7f86763, 941c39b, e7ec642, 46ac837, ef899f8, 2fcf56e, cf560ba.

### Phase 3 — DB Scripts and One-Shots

Scope:
- `db:exec`, `db:status:turso`, `db:query:turso`.
- `validate:data` and `doctor`.
- `db:migrate:turso`.
- `db:seed:plans`.
- `db:sync:destinations`.
- `db:sync:events` or retire if obsolete.
- `db:fetch:holidays:tw`.
- Remaining `db:import:turso` behavior only if still needed after `travel-scraper` direct import.

Why third:
- Uses the Rust Turso credential path as the default.
- Removes the TS `scripts/turso-pipeline.ts` dependency.
- Keeps high-risk mutation commands separate from schema/admin utilities.

Deliverables:
- `travel db exec/status/query-offers/migrate/seed/fetch/sync`.
- `travel validate data` and `travel doctor`.
- Migration tests for SQL splitter / idempotency.
- Real dry-run and live-run records for one migration/status/query path.

Estimate: 1-2 weeks. `turso-migrate.ts` is the largest single file here at 1,737 LOC and may dominate.

**Phase 3 progress — easy subset ✅ DONE (2026-06-08):**
The four low-risk admin/read DB commands are Rust-built and verified:
- ✅ `travel db status` (ports db:status:turso) — byte-identical. Commit 766a179.
- ✅ `travel db exec "<sql>"` (ports db:exec) — affected-count + multi-statement parity;
  SELECT rows are plain `col: val` text (no JSON, per plan). Added `db::connect_write()`
  (write-tier token: env → cache → mint, mirrors connect_read). Commit dd48e83.
- ✅ `travel db query-offers` (ports db:query:turso) — same row set/ordering as TS; dropped
  the `--json` flag and fixed a TS `{"type":"null"}` null-leak (null → `-`/empty). Commit 9dbc857.
- ✅ `travel validate data` / `travel doctor` (ports validate:data + doctor) — byte-identical,
  one module behind two entry points (`validate::Mode::{Validate,Doctor}`). `validateDependencies`
  / `validateCliScripts` ported as no-op stubs (npm goes away in Phase 5). Commit 5bf759f.

Verified: cargo build/clippy clean (only pre-existing db.rs/plans.rs warnings), 6+1 tests pass,
byte-parity diffs on db-status + validate-data, db-exec write path resolves write-tier creds.

⏳ **Deferred (larger / one-shot, out of the easy subset):** `db:migrate:turso` (1,737 LOC),
`db:seed:plans`, `db:sync:destinations`, `db:sync:events`, `db:fetch:holidays:tw`, `db:import:turso`.

### Phase 4 — StateManager-Backed Mutation Commands and Cascade

Scope:
- `set-dates`, `set-flight`, `set-hotel`, `set-airport-transfer`.
- Offer commands: `update-offer`, `select-offer`.
- Itinerary commands: `scaffold-itinerary`, `populate-itinerary`, `set-route-*`, `set-day-theme`,
  `swap-days`, session commands, activity commands.
- Booking commands: `mark-booked`, `sync-bookings`, `check-booking-integrity`.
- Weather if still part of plan mutation.
- Shaping and tour-group commands that mutate Turso.
- Cascade runner and all dirty-flag side effects.

Why fourth:
- This is the hard core. By this point the Rust repository, renderer, validation, and read-only views
  should already be stable.

Deliverables:
- Rust command handlers behind the same plain-text command names.
- Golden parity tests against Turso-backed fixtures or live records, not JSON files.
- For each command: TS run vs Rust run on the same plan state, diff DB rows and CLI output.
- Cascade parity: dirty flags, dependent process invalidation, and derived rows match TS behavior.

Estimate: 3-6 weeks depending on how many mutation commands are still actively used.

**Phase 4 progress — read foundation started (2026-06-08):**
- ✅ `travel status [--full]` (ports status / view:status) — BYTE-IDENTICAL on tokyo-2026
  + kyoto-2026. Commit 288a01d. First full assembled-plan-style READ in Rust:
  `rust/crates/travel-cli/src/plan.rs` (new) assembles a `PlanView` from the 14 tables
  status touches (plan_metadata, process_statuses, cascade_dirty_flags, date_anchors,
  flight_legs, airport_transfers + _candidates, hotels + hotel_access_lines,
  plan_offer_selection, plan_offer_includes, days, timesofday, activities) — all from
  de-JSON'd child tables/scalars, no JSON parsing. `status.rs` is the formatter.
  Covers getters: ActiveDestination/DateAnchor/DirtyFlags/ProcessStatus/FlightInfo/
  HotelInfo/AirportTransfers. Mirrors a TS quirk (Selected Offer / Includes not
  rendered on a fresh read because chosen_offer is only set by setOfferSelection(),
  not rehydrated by the assembler).
- ✅ `travel bookings` (ports view:bookings) — BYTE-IDENTICAL tokyo+kyoto. Commit b2d157e. Reuses plan.rs (0 new tables).
- ✅ `travel transport` (ports view:transport) — BYTE-IDENTICAL tokyo+kyoto. Commit 06e8c61. Extended plan.rs (DayView.theme + SessionView.transit_notes).
- ✅ `travel itinerary` (ports view:itinerary) — BYTE-IDENTICAL tokyo+kyoto, incl. BOTH itinerary formats (session-based Tokyo + schedule-based Kyoto). Commit e284f4d. Added day_route_segments; session_meals reused. (A latent TS flight-line bug was caught by parity: outbound+return must be ONE line, not two console.logs.)
- ⏭️ `travel view-prices` — DEFERRED. The TS command needs `--start`/`--end` and then filters out flight offers with null departure_date; the current dataset's 4 flight offers ALL have null departure_date, so TS produces no verifiable output. No code blocker — when dated flight offers land it's a thin wrapper over the already-ported `queryOffers`/`db query-offers`.
- Plan resolution: still TRAVEL_PLAN_ID env only (read views don't need the full
  resolver). Port `src/cli/shared/plan-resolver.ts` (--plan-id/--travel-date/active-plan,
  ~200 LOC, pure DB read) when the first MUTATION lands — mutations need it.
- plan.rs now reads 16 tables (~590 net lines).
- ✅ MUTATIONS started (2026-06-09): the plan-resolver is ported, and 8 mutations are done +
  verified by before/after DB-row diff on the disposable `test-set-dates-2026` plan
  (scripts/seed-test-plan.ts seeds/resets it; tokyo/kyoto never mutated):
  `set-dates` (the cascade write — date_anchors + 4 dirty flags + event + operation_runs +
  version, process_statuses/plan_root_date_anchor UNCHANGED) and the no-cascade setters
  `set-day-theme`, `set-hotel`, `set-flight`, `set-airport-transfer`, `set-route-segment(s)`,
  `set-tod-focus/time-range/zh`, `set-activity-time/title`. All byte-identical to TS (DB rows
  + CLI stdout). The write pattern (db::connect_write + targeted UPDATE/INSERT +
  operation_runs + version+1, mirroring syncNormalizedTables DELETE-then-reinsert) is the
  template for remaining mutations.
  > Verification caught (and fixed) parity bugs the handoff reports missed — the gate is the
  > DB-row diff, NOT passing unit tests (tests can encode the bug): e.g. set-tod-zh wrote
  > "null" vs TS "undefined" for omitted zh fields (fixed, commit 6112608); derive_plan_id,
  > validate_date_range, format_date had similar caught-in-review divergences earlier.
- ⏳ NOT STARTED: the CASCADE-triggering offer mutations + the rest. **`select-offer` /
  `update-offer` fire the populate-P3+P4 cascade (the most complex side-effect in the
  system) — me-led / closely-reviewed, NOT a pure handoff.** Then offer/tour-group ingestion,
  shaping (6), itinerary builders (scaffold/populate), ops (sync-bookings, fetch-weather,
  run-*), and ~10 remaining reads. Cascade runner generalization LAST.

> Verified: clean rebuild, 10+1 cargo tests pass (4 new status.rs unit tests: formatDate
> parity, locale_i64, status_icon, transfer-terminal logic), clippy clean (only the 2
> pre-existing db.rs/plans.rs warnings), working tree == pushed commit.

### Phase 5 — Test/Hook Replacement and Root npm Deletion

Scope:
- Replace `typecheck`, `test`, `test:*`, `hooks:install`, `postinstall`.
- Remove root `package.json`, `package-lock.json`, root TS dev dependencies, and root ts-node/vitest
  workflows only after every in-scope script is replaced.

Deliverables:
- Pre-commit hook runs:
  - `cargo check --workspace`
  - `cargo clippy --workspace`
  - `cargo test --workspace`
  - `./bin/travel validate data`
- Root npm deletion PR/commit with proof that every script has a Rust command.
- Worker npm remains untouched in `workers/trip-dashboard/`.

Estimate: 2-4 days after phases 1-4 are complete.

## Decommission Gate

Mirror the scraper policy:
- Do not delete a TS command when a Rust command merely builds.
- Delete a TS command only after:
  - Rust command exists.
  - Cargo tests cover its core behavior.
  - A real command run succeeds against Turso or an active plan.
  - Same-input TS vs Rust output diff is reviewed, or the difference is explicitly accepted.
- Root `package.json` is deleted last, only when every in-scope script has a proven Rust path.
- Worker package remains.

Suggested tracking table per command:

| Command/script | Rust command | cargo parity | real run | TS deleted | Notes |
|---|---|---:|---:|---:|---|
| `leave-calc` | `travel leave calc` | done | done | no | Phase 1, byte-parity |
| `compare-trips` | `travel compare trips` | done | done | no | Phase 1, text-spec input |
| `normalize-flights` | `travel normalize flights` | done | done | no | Phase 1, rendered-text input |
| `plans` | `travel plans` | done | done | no | Phase 2, byte-parity |
| `query-offers` | `travel query-offers` | done | done | no | Phase 2, count-parity (16=16) |
| `query-bookings` | `travel query-bookings` | done | done | no | Phase 2, byte-parity |
| `check-freshness` | `travel check-freshness` | done | done | no | Phase 2, byte-parity |
| `query-destination-ref` | `travel query-destination-ref` | done | done | no | Phase 2, byte-parity (de-JSON child rows) |
| `compare-dates` | `travel compare dates` | done | done | no | Phase 2, byte-parity |
| `compare-true-cost` | `travel compare true-cost` | done | done | no | Phase 2, byte-parity |
| `db:status:turso` | `travel db status` | done | done | no | Phase 3, byte-parity |
| `db:exec` | `travel db exec` | done | done | no | Phase 3, plain-text SELECT (no JSON) |
| `db:query:turso` | `travel db query-offers` | done | done | no | Phase 3, row/order parity; fixed null-leak |
| `validate:data` | `travel validate data` | done | done | no | Phase 3, byte-parity |
| `doctor` | `travel doctor` | done | done | no | Phase 3, byte-parity |
| `status` | `travel status` | done | done | no | Phase 4 read foundation, byte-parity (tokyo+kyoto) |
| `view:bookings` | `travel bookings` | done | done | no | Phase 4 read view, byte-parity (tokyo+kyoto) |
| `view:transport` | `travel transport` | done | done | no | Phase 4 read view, byte-parity (tokyo+kyoto) |
| `view:itinerary` | `travel itinerary` | done | done | no | Phase 4 read view, byte-parity both formats |
| `view:prices` | `travel view-prices` | deferred | n/a | no | Phase 4; no testable flight data (null departure_date) |
| `set-dates` | `travel set-dates` | done | done | no | Phase 4 first mutation; cascade write, DB-row parity (re-verified) |
| `set-day-theme` | `travel set-day-theme` | done | done | no | Phase 4 setter; no-cascade, DB-row parity |
| `set-hotel` | `travel set-hotel` | done | done | no | Phase 4 setter; hotels + hotel_access_lines, DB-row parity |
| `set-flight` | `travel set-flight` | done | done | no | Phase 4 setter; flight_legs (shared airline fields), DB-row parity |
| `set-airport-transfer` | `travel set-airport-transfer` | done | done | no | Phase 4 setter; transfers + candidates, djb2 int32 hash, DB-row parity |
| `set-route-segment(s)` | `travel set-route-segment[-s-bulk]` | done | done | no | Phase 4 setter; day_route_segments, DB-row parity |
| `set-tod-*` / `set-session-*` | `travel set-tod-focus/time-range/zh` | done | done | no | Phase 4 setter; timesofday + session_activities_zh, DB-row parity (set-tod-zh "undefined"-KV bug fixed in review) |
| `set-activity-time/title` | `travel set-activity-time/title` | done | done | no | Phase 4 setter; activities, DB-row parity |
| `select-offer` | `travel select-offer` | pending | pending | no | Phase 4; fires populate-P3+P4 cascade — me-led/closely-reviewed |
| `update-offer` | `travel update-offer` | pending | pending | no | Phase 4; cascade — me-led/closely-reviewed |

## Verification Method

For read-only commands:
- Run TS command and Rust command against the same plan/Turso state.
- Normalize volatile lines such as timestamps.
- Diff plain-text output.

For DB mutation commands:
- Run on a controlled Turso record or test plan.
- Query changed rows before and after.
- Assert row equality, cascade flags, and command output.
- Roll forward only; do not rely on local JSON fixtures.

For one-shot migrations:
- Dry-run SQL plan test.
- Idempotent live run in a non-production/test DB or guarded production run.
- Schema query confirms expected tables/indexes/columns.

For tooling:
- `cargo check`, `cargo clippy`, `cargo test`, and `travel validate data` replace TS tooling.

## Credentials

Rust already has the desired model through `rust/crates/turso-util`:
- Minted read/write/secrets tier tokens.
- Safe cache.
- Fail-loud if no bootstrap auth exists.

Once the root TS scripts are gone, this becomes the sole root credential path. No `.env` walk-up,
no static full-access token dependency. The Worker keeps Cloudflare secret bindings and remains out of
scope.

## Biggest Risk

The biggest risk is semantic drift in the StateManager/cascade port. The TypeScript code is not just a
CLI; it is a stateful domain engine. A naive line-by-line rewrite can silently change cascade invalidation,
offer selection, itinerary mutation, or booking sync behavior.

Mitigation:
- Migrate behind a Rust repository façade.
- Port one command family at a time.
- Keep TS until parity is proven.
- Prefer real Turso row comparisons over mocked local files.
- Delete root npm last.

## Effort Estimate

| Phase | Estimate | Risk |
|---|---:|---|
| Phase 1 pure utilities | 2-4 days | Low |
| Phase 2 read-only views | 1-2 weeks | Medium |
| Phase 3 DB scripts/one-shots | 1-2 weeks | Medium/high because migrations are large |
| Phase 4 StateManager/cascade mutations | 3-6 weeks | High |
| Phase 5 tooling/root npm deletion | 2-4 days | Medium, mostly coordination |

Total: roughly 5-10 weeks of focused work, depending on how much of the current TS command surface is
still active and how strict parity needs to be for older commands.
