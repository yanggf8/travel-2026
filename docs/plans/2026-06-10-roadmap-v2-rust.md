# Roadmap V2 in Rust — StateManagerV2 via Rust Port + npm Retirement

**Date:** 2026-06-10
**Supersedes:** the TypeScript StateManagerV2 refactor (CLAUDE.md "Next Steps"; TS slice reverted in 54720e8).
**Spec it implements:** ADR-001 (`src/skills/travel-shared/references/architecture-decisions.md`).
**End state:** zero npm/TS at the repo root — CLI, scripts, tests, and git hooks all Rust. Only `workers/trip-dashboard/` keeps its own self-contained `package.json`. The Worker's own Rust port is **parked but on the agenda** (see §6).

---

## 1. Why this plan replaces the TS V2 refactor

ADR-001's goal: each command = one targeted SELECT (validate) + one targeted UPDATE/INSERT. No in-memory plan object, no `syncNormalizedTables()` flush. That kills:

- dual-path sync bugs (read assembler vs write INSERT drifting apart)
- silent column wipes (`INSERT OR REPLACE` flush dropping columns it doesn't know)
- the false source-of-truth window (DB only correct after `save()`)
- 38-query load + full-table rewrite to change one field

**The Rust CLI already implements this pattern natively.** Every ported command in `rust/crates/travel-cli` does targeted SQL via `libsql` (e.g. `set_tod.rs` single-column UPDATE; `set_day_theme.rs` explicitly does NOT touch sibling rows, unlike `syncNormalizedTables`). Cascades are per-command modules with shared write primitives (`src/cascade/common.rs`). No write path uses an assembled plan object. Read-only views (`status`, `itinerary`) assemble display structs — fine; reads can't lose data.

Therefore V2 is achieved **by construction** in the Rust port. Rewriting the TS internals would refactor code scheduled for deletion. The remaining work is finishing the port so the V1 TS spine (`PlanRepository`, `plan-assembler.ts`, `syncNormalizedTables()`) is **deleted, not refactored**.

> **Exposure until cutover:** `package.json` still routes every command through the TS V1 path, so the silent-wipe bug class remains live in daily use. Worse, mixing Rust targeted writes with TS full-flush saves on the same plan can overwrite Rust writes. Sequence below is therefore tests → cutover → archive, strictly in order.

## 2. ADR-001 → Rust mapping

| ADR-001 element | Rust equivalent | Status |
|---|---|---|
| `DbClient` interface (queryOne/queryMany/execute) | `libsql::Connection` via `turso-util::connect()` (token-minted creds, no static token) | ✅ in use |
| Targeted SELECT + UPDATE per command | All 60 dispatch arms in `travel-cli/src/main.rs` | ✅ built |
| Cascade via targeted reads | `travel-cli/src/cascade/{common,date_change,select_offer}.rs` | ✅ built |
| Operation tracking (`operation_runs`) | `cascade/common.rs` insert + `plans.version` bump | ✅ built |
| Remove `PlanRepository` / `plan-assembler.ts` / `syncNormalizedTables()` / `TravelPlanMinimal` | Archive the TS tree (Phase 5) | ⬜ pending |
| Testing: seed → dispatch → SELECT → assert → teardown, no mocks | `cargo test` against real Turso (pattern started: `travel-cli/tests/holiday_turso.rs`; 84 inline `#[test]`s) | ⬜ expand |
| Dashboard reads Turso directly, unaffected | `workers/trip-dashboard/src/turso.ts` — out of scope, stays TS | ✅ unchanged |

## 3. Phases

### Phase 1 — Parity verification (cheap, mostly done)
The audit's 27-command gap is closed (incl. `delete-activity`, all `shaping-*`, `fetch-weather`, `sync-bookings`, `mark-booked`, `swap-days`). Remaining:
- Diff TS vs Rust output for the handful of doubtful commands (known intentional diff: import-offers — TS path crashes by design of the bug, Rust implements intended behavior).
- Record `scrape-package` as **dropped by design** (superseded by chromeport).

### Phase 2 — Rust integration tests (the cutover gate)
Port `tests/integration/*.test.ts` (8 files: state-manager, plan-resolver, itinerary-validator, shaping-service, shaping-baseline-cli, tour-group-{bridge,import,service}) to `rust/crates/travel-cli/tests/`:
- Pattern per ADR-001: seed minimal rows for `plan_id='test-plan'` → run command fn → SELECT affected row → assert → teardown (`DELETE WHERE plan_id='test-plan'`). Real Turso, no mocks.
- Reuse `tests/holiday_turso.rs` as the harness template.

### Phase 3 — Port `scripts/` (last ts-node dependency)
Into `travel-db` / `travel-validate` binaries (CLAUDE.md naming):
- **Port:** `turso-migrate`, `turso-status`, `turso-exec`, `turso-query-offers`, `gen-schema-sql`, `validate-data` (→ `travel-validate`, includes doctor mode), `seed-plans-current`, `seed-destination-refs`, `seed-ota-knowledge`, `seed-test-plan`, `turso-sync-destinations`, `turso-sync-events`, `import-offers-to-turso`, `fetch-taiwan-holidays` (live fetch only — no hand-crafted data).
- **Archive without porting** (one-shot, already run): `migrate-add-noon`, `migrate-rename-itinerary-tables`, `backfill-local-reference-data`, `extract-bookings`, `set-kyoto-zh-sessions-v2` (keep as documented pattern reference in archive).
- `turso-pipeline.ts` dies with its callers (Rust uses `libsql` directly).

### Phase 4 — Cutover + npm retirement (only after Phase 2 green)
1. Build release binaries to `./bin/` (`travel`, `travel-validate`, `travel-db`, …).
2. Rewrite `scripts/hooks/pre-commit`: `npm run typecheck` + `npm run validate:data` → `cargo check --workspace` (or `cargo clippy`) + `./bin/travel-validate data`.
3. Replace hook installation (`postinstall`) with a plain script (`scripts/hooks/install.sh`) or `cargo xtask`.
4. Update CLAUDE.md / skills / docs: `npm run travel -- X` → `./bin/travel X` everywhere.
5. **Delete** root `package.json`, `package-lock.json`, `node_modules/`, `tsconfig.json`, `vitest.config.ts`.

### Phase 5 — Archive TS
Move `src/cli`, `src/state`, `src/services`, `src/scrapers`, `src/utils`, `src/config`, `src/contracts`, `src/cascade`, `src/validation`, `src/types`, `src/templates`, `scripts/*.ts`, `tests/integration` → `archive/ts-v1/`. ADR-001's "What this removes" list is fulfilled here. `src/skills/` (markdown) stays.

### Phase 6 — PARKED: Worker → workers-rs (on the agenda, not scheduled)
Audited 2026-06-10. Port is feasible: 2,921 dependency-free TS lines (`styles.ts` 753 trivial, `index.ts` 189 easy, `turso.ts` 772 moderate — same pattern as `travel-cli/src/plan.rs`, `render.ts` 1,207/36 fns is the real work and regression-prone). `workers-rs` is officially supported and ships default panic recovery since v0.6 (2026). **Why parked:** wrangler (npm) remains required for dev/deploy even with a Rust Worker, so the port unifies language but does not eliminate npm; the dashboard is read-mostly so none of the V2 data-integrity benefits apply. Revisit after Phase 5 lands. Until then the Worker stays TS, self-contained.

## 4. Hard constraints (unchanged)
- CF Worker stays TS/JS, self-contained — out of scope for Phases 1–5 (Rust port parked, §3 Phase 6).
- No JSON as pipeline boundary or source of truth; plain-text CLI output.
- Turso-only data; fail loud, no local fallback.
- Creds via `turso-util` mint broker.
- Live holiday fetch — never hand-crafted reference data.

## 5. What we gain when done

1. **Silent data loss eliminated** — one write path per command; new column = one UPDATE statement, compiler-checked.
2. **DB true after every command** — no in-memory window; dashboard and CLI never see half-synced state.
3. **Speed** — 1–3 SQL statements vs 38-query load + full flush; ~10ms native binary vs ~2s ts-node startup per command.
4. **One codebase** — no TS/Rust parity drift, no double maintenance; root npm/node toolchain gone.
5. **Real tests** — `cargo test` against live schema; wrong SQL fails the test, no mocks to hide it.
