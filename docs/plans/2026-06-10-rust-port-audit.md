# Rust Port Audit — TS/JS Elimination (CLI-first)

**Date:** 2026-06-10
**Goal:** Eliminate all JavaScript/TypeScript outside the Cloudflare Worker. Everything → Rust, CLI first.
**Excluded by design:** `workers/trip-dashboard/` (CF Worker — stays TS/JS, reads Turso directly, needs a JS runtime).

> **STATUS UPDATE (later 2026-06-10): P1 COMPLETE — the §3 command gap is CLOSED and verified.** `main.rs` now has **63 dispatch arms**; **zero `not yet implemented` stubs remain**. All 27 gap commands were ported or alias-wired (incl. `delete-activity`/`remove-activity`, `set-activity-booking`, `scaffold-itinerary`, `populate-itinerary`, `swap-days`, `mark-booked`, `sync-bookings`, `check-booking-integrity`, `run-status`/`run-list`, `validate-itinerary`, all `shaping-*`, `query-tour-group-offers`, `fetch-weather`, `view-prices`, `compare-offers`/`search-offers`, `chat-format`, `list-plans`); `scrape-package` **dropped by design** (already a decommissioned no-op pointing at chromeport).
>
> **How it landed (commits):** `235f13e` (list-plans alias) → `e32264c` (collision-free scaffold: main.rs pre-wired + 14 stub modules) → `b25f4e1` (batches 1–3: activity/itinerary/bookings) → `f61cbc0` (batches 4–5: shaping/weather/prices/compare). Ported in 5 parallel agent batches over disjoint module files with **no main.rs edits per agent** (front-loaded dispatch + fixed signatures = zero merge collisions).
>
> **Verification:** `cargo build -p travel-cli` clean; 115 unit + 1 integration test pass; read commands byte-parity vs TS (validate-itinerary VALID, run-list 7 results, check-booking-integrity 4 matches, shaping/chat-format byte-identical); write commands tested on throwaway plans/runs with cleanup — real trips (tokyo-2026/kyoto-2026) never mutated. All Rust commands emit the same `plan_events` + `operation_runs` + `plans.version` audit trail as the TS path.
>
> **NOT done by P1 (active roadmap = §5 steps 3–6, see `docs/plans/2026-06-10-roadmap-v2-rust.md`):** the `package.json` cutover has NOT happened — npm still runs everything via `ts-node`; the Rust binaries exist and work but aren't wired into any npm script. Remaining: port `scripts/` → `travel-db`/`travel-validate`, port `tests/integration/` → `cargo test`, then flip `package.json`, then archive the TS. The numbers in §1 below are the morning-of-2026-06-10 snapshot — kept for history.

---

## 1. Headline numbers

- **124** `.ts`/`.js` files outside the Worker (excl. `node_modules`, `archive`, `rust/target`, `.git`).
- **`package.json` still runs 100% via `ts-node`** — no script points at a Rust binary yet. The Rust CLI exists and builds but is not wired into any npm script. **This is the cutover that hasn't happened.**
- Rust `travel-cli`: **47** dispatch subcommands implemented and building (`cargo build -p travel-cli` ✅).
- TS CLI: **59** registered command names.
- **Real command gap: 27 TS command names have no Rust dispatch arm** (26 real + `help`; list below). The rest are at parity or are aliases/view duplicates.

---

## 2. Surface inventory (TS/JS by area)

| Area | Files | Role | Port target |
|---|---:|---|---|
| `src/cli/` | 47 | The CLI: `travel-update.ts` + `commands/` (per-command modules) + standalone `compare-*.ts`, `cascade.ts` | **Rust `travel-cli`** (priority 1) |
| `scripts/` | 20 | DB migrate/seed/sync, validate-data, schema-gen, holidays fetch, turso-pipeline | **Rust `travel-db` / `travel-validate` / shared** (priority 2) |
| `src/state/` | 15 | StateManager, repositories, cascade, managers, types | Rust (logic behind commands; some already in `travel-cli/src/cascade/`) |
| `src/services/` | 5 | turso-service, weather, tour-group, shaping | Rust |
| `src/scrapers/` | 5 | scrape-file-parser, registry, base | Rust (`chromeport` owns live capture; parser → Rust) |
| `src/utils/` | 6 | date, flight-normalizer, holiday, leave, plan-id | Rust (`leave`, `normalize` already in Rust) |
| `src/cascade/` | 4 | cascade runner | Rust (`cascade/` dir exists in travel-cli) |
| `src/validation/`, `src/types/`, `src/config/`, `src/contracts/`, `src/templates/` | 14 | validators, domain types, config loader, skill contracts | Rust |
| `tests/integration/` | 8 | vitest real-Turso tests | **Rust integration tests** (`cargo test`) |
| root (`vitest.config.ts`) + `tmp/` | 2 | test config / scratch | drop after test port |

---

## 3. CLI command parity matrix (priority 1)

### ✅ At parity (Rust dispatch arm exists) — ~31 commands
`add-besttour-offer · add-lifetour-offer · add-offer · bookings · check-freshness · destination-ref · import-offers · import-tour-group-offers · itinerary · plans · query-bookings · query-destination-ref · query-offers · select-offer · set-activity-time · set-activity-title · set-airport-transfer · set-dates · set-day-theme · set-flight · set-hotel · set-route-segment · set-route-segments-bulk · set-session-focus · set-session-time-range · set-session-zh · set-tod-focus · set-tod-time-range · set-tod-zh · status · transport · update-offer · validate`
Plus Rust-side: `leave calc · compare {trips,dates,true-cost} · normalize flights · resolve-plan · db {status,exec,query-offers} · doctor`.

### ❌ Gap — 27 TS command names with NO Rust dispatch (26 real + `help`; must port)

**Itinerary mutation (6):**
`scaffold-itinerary · populate-itinerary · delete-activity · remove-activity · swap-days · set-activity-booking`

**Shaping Stage (7):**
`shaping-init · shaping-compare · shaping-adopt · shaping-baseline · shaping-export · shaping-import` (+ `query-tour-group-offers`)

**Bookings / status (5):**
`mark-booked · sync-bookings · check-booking-integrity · validate-itinerary · run-status`/`run-list`

**Weather (1):** `fetch-weather`

**Compare/search/view duplicates (4):** `compare-offers · search-offers · view-prices · list-plans` (likely thin aliases over existing Rust query/compare paths — confirm, may be cheap)

**Other (2):** `chat-format · scrape-package` (`scrape-package` overlaps `chromeport`; confirm if still needed)

> Note: some gaps are aliases (`list-plans`→`plans`, `compare-offers`→`query-offers`+compare). Triage each: true-port vs alias-wire.

---

## 4. Supporting `scripts/` (priority 2) — 20 files

Must move to Rust before `package.json` can drop `ts-node` entirely:

- **DB lifecycle:** `turso-migrate.ts`, `turso-status.ts`, `turso-query-offers.ts`, `turso-exec.ts`, `turso-pipeline.ts` (→ already partly in `turso-util`), `import-offers-to-turso.ts`, `gen-schema-sql.ts`
- **Seeds:** `seed-plans-current.ts`, `seed-destination-refs.ts`, `seed-ota-knowledge.ts`, `seed-test-plan.ts`, `set-kyoto-zh-sessions-v2.ts`
- **Sync:** `turso-sync-destinations.ts`, `turso-sync-events.ts`
- **One-shot migrations (may archive, not port):** `migrate-add-noon.ts`, `migrate-rename-itinerary-tables.ts`, `backfill-local-reference-data.ts`, `extract-bookings.ts`
- **Live-data:** `fetch-taiwan-holidays.ts` (must fetch live — see [[no-hand-crafted-reference-data]])
- **validate-data.ts** → `travel-validate` (Rust) — note the OTA-table parser we just fixed lives here.

---

## 5. Recommended sequence

1. **Triage the 28-command gap**: split into (a) true ports, (b) alias-wires, (c) drop/merge. Cheapest wins first (aliases).
2. **Port itinerary-mutation + shaping + bookings commands** (the substantive 18-ish).
3. **Port `scripts/` DB+seed+validate** into `travel-db` / `travel-validate`.
4. **Port `tests/integration/` to `cargo test`** (real-Turso, no mocks) — parity proof before cutover.
5. **Cutover `package.json`**: flip each script `ts-node …` → `./bin/<tool>`-first-with-TS-fallback (per CLAUDE.md naming: `travel`, `travel-validate`, `travel-compare`, `travel-utils`, `travel-db`). Do this **only after** parity + tests pass ([[ts-archive-pending-phase2]], [[kill-npm-on-hold-worker]]).
6. **Archive `src/cli`, `src/state`, `scripts/`, `tests/integration` TS** once binaries are the default and green.

## 6. Hard constraints (carry into the plan)

- CF Worker (`workers/`) stays TS/JS — **not** in scope. Reads Turso directly; independent of CLI.
- No JSON as pipeline boundary / source of truth ([[no-json-in-rdb]], [[no-local-data-turso-only]]).
- Turso creds via `turso-util` mint broker, not static token ([[turso-util-token-minting]]).
- Live data only — `fetch-taiwan-holidays` must hit the real source ([[no-hand-crafted-reference-data]]).
- The Rust port is the roadmap, not the TS StateManagerV2 refactor ([[rust-port-is-the-roadmap]]).
