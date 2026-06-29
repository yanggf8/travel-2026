# DB-Centric Provider Architecture — Implementation Plan

> **STATUS: COMPLETE (2026-06-29).** All 9 tasks shipped (commits `47f5209`, `fa61364`, `f22cdd5`,
> `4ca9af4`, `d2d0be9`, `d244383`, `ab67436`, `c58577b`, `07daa84`). A post-implementation review
> (Claude, corroborating Codex's delegated work line-by-line vs source + live DB) found and fixed
> three regressions the green suite had masked — see **Post-review fixes** at the bottom (commits
> `49931bc`, `668ce24`). Provider coverage is now reproducible DB data; `travel ota-status` is the
> single source of truth.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the OTA provider catalog + coverage status normalized, queryable DB data (the source of truth) instead of Rust `const` arrays, a free-text `notes` column, and CLAUDE.md prose.

**Architecture:** New normalized tables (`product_types`, `ota_source_coverage`, `ota_source_region_codes`, `coverage_block_reasons`, `catalog_runs`); re-key `parser_rules` per product type; audit-tracked CLI mutations (`set-ota-source`, `set-ota-coverage`, `set-ota-region`); a `travel ota-status` read view; cold-start bootstrap from a checked-in seed SQL file instead of Rust arrays; two-phase migration of `notes` → typed rows.

**Tech Stack:** Rust (travel-cli, libsql), Turso (SQLite), inline-DDL migrations in `db_migrate.rs`, real-Turso integration tests in `tests/*.rs`.

**Design spec:** `docs/superpowers/specs/2026-06-29-db-centric-provider-architecture-design.md` (Codex-reviewed). Read it before any task.

## Global Constraints

- **No JSON, no blobs, no prose-as-data.** Every structured fact → typed column or child row. The ONLY allowed free TEXT is the LLM's raw input/output (`captures.raw_text`, agent-parse extraction) — untouched by this plan.
- **DB is the source of truth.** Catalog facts must NOT live in Rust `const` arrays or CLAUDE.md. Migrations are insert-if-absent on a populated DB; never overwrite live rows.
- **Audit triad for every mutation.** Global catalog edits write a `catalog_runs` row (the global analogue of `operation_runs`, which is plan-scoped / `plan_id NOT NULL`).
- **Canonical product types:** `flight | hotel | fit | group_tour` (in `product_types`). `offers.type` CHECK is UNCHANGED (`package|flight|hotel`); `fit`+`group_tour` store as `package`.
- **Migrations:** add `CREATE TABLE IF NOT EXISTS` blocks in `db_migrate.rs` (pattern: lines ~139+); idempotent; re-run safe.
- **Tests:** real-Turso integration tests in `rust/crates/travel-cli/tests/*.rs`; seed → run binary → SELECT → assert → teardown; skip cleanly if creds absent; **use `mod common; use common::Guard;` for panic-safe teardown** (never a trailing `teardown()`).
- **Build/validate oracle:** `make check` (build) + `cargo test -p travel-cli --test <name>` per task.
- **Env for live tests/commands:** export `TRAVEL_TURSO_URL` / `TRAVEL_TURSO_READ_TOKEN` / `TRAVEL_TURSO_WRITE_TOKEN` from `.env` (see CLAUDE.md "Token resolution").

---

## Phase 0 — Schema foundation (no behavior change yet)

### Task 1: Create the lookup + coverage tables [DELEGATABLE → Grok]

**Files:**
- Modify: `rust/crates/travel-cli/src/db_migrate.rs` (add CREATE TABLE blocks in the migrate fn alongside the existing `CREATE TABLE IF NOT EXISTS` list, ~line 139+)
- Test: `rust/crates/travel-cli/tests/ota_catalog_schema.rs` (new)

**Interfaces:**
- Produces tables: `product_types(code PK, description)`, `coverage_block_reasons(code PK, description)`, `ota_source_coverage(source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code, updated_at, PK(source_id,product_type))`, `ota_source_region_codes(source_id, product_type, region_label, region_code, PK(source_id,product_type,region_label))`, `catalog_runs(run_id PK, command_type, command_summary, status, changed_at)`.

- [ ] **Step 1: Write the failing test** — `tests/ota_catalog_schema.rs`: after `db migrate`, each new table exists and accepts a sample insert with the documented columns; `ota_source_coverage` rejects `proven=2` (CHECK) and rejects a `product_type` not in `product_types` (FK, if FKs on) — at minimum assert the column set via `db schema <table>`. Use the `db_seed.rs` test style (run binary, parse `db exec` output, skip if credless).

- [ ] **Step 2: Run test, verify it fails**
Run: `cd rust && cargo test -p travel-cli --test ota_catalog_schema`
Expected: FAIL — tables don't exist (`no such table: product_types`).

- [ ] **Step 3: Add the CREATE TABLE blocks** in `db_migrate.rs` (mirror the existing `CREATE TABLE IF NOT EXISTS bookings (...)` style). Exact DDL:

```sql
CREATE TABLE IF NOT EXISTS product_types (
  code TEXT PRIMARY KEY,
  description TEXT
);
CREATE TABLE IF NOT EXISTS coverage_block_reasons (
  code TEXT PRIMARY KEY,
  description TEXT
);
CREATE TABLE IF NOT EXISTS ota_source_coverage (
  source_id TEXT NOT NULL,
  product_type TEXT NOT NULL,
  proven INTEGER NOT NULL DEFAULT 0 CHECK(proven IN (0,1)),
  proven_at TEXT,
  method TEXT CHECK(method IS NULL OR method IN ('agent_parse','regex')),
  search_url TEXT,
  blocked_reason_code TEXT,
  updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (source_id, product_type)
);
CREATE TABLE IF NOT EXISTS ota_source_region_codes (
  source_id TEXT NOT NULL,
  product_type TEXT NOT NULL,
  region_label TEXT NOT NULL,
  region_code TEXT NOT NULL,
  PRIMARY KEY (source_id, product_type, region_label)
);
CREATE TABLE IF NOT EXISTS catalog_runs (
  run_id TEXT PRIMARY KEY,
  command_type TEXT NOT NULL,
  command_summary TEXT,
  status TEXT NOT NULL,
  changed_at TEXT NOT NULL
);
```
(FKs: SQLite enforces FKs only with `PRAGMA foreign_keys=ON`; this repo does not rely on that, so FKs are documented in the spec but enforcement is via the write CLI + a `validate data` check, NOT a hard FK. Do not add `REFERENCES` clauses unless the repo already uses them — check `schema.sql` first; it does not, so omit them.)

- [ ] **Step 4: Run test, verify it passes**
Run: `cd rust && cargo test -p travel-cli --test ota_catalog_schema`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add rust/crates/travel-cli/src/db_migrate.rs rust/crates/travel-cli/tests/ota_catalog_schema.rs
git commit -m "feat(db): add normalized OTA provider catalog tables"
```

### Task 2: Seed the lookup tables from a checked-in SQL file [DELEGATABLE → Grok]

**Files:**
- Create: `scripts/seed/ota_catalog.seed.sql`
- Modify: `rust/crates/travel-cli/src/db_migrate.rs` (run the seed file insert-if-absent on migrate)
- Test: extend `tests/ota_catalog_schema.rs`

**Interfaces:**
- Consumes: tables from Task 1.
- Produces: `product_types` rows (`flight`,`hotel`,`fit`,`group_tour`); `coverage_block_reasons` rows (`renderer_wedge`,`login_wall`,`captcha`,`cloudflare`,`redundant`,`unsupported`).

- [ ] **Step 1: Write the failing test** — assert `SELECT count(*) FROM product_types` ≥ 4 and the 4 codes present; `coverage_block_reasons` has the 6 codes.
- [ ] **Step 2: Run, verify fails** (`Expected: 0 rows`).
- [ ] **Step 3: Create `scripts/seed/ota_catalog.seed.sql`** with `INSERT OR IGNORE INTO product_types (code, description) VALUES ('flight','...'),('hotel','...'),('fit','機+酒 self-guided flight+hotel'),('group_tour','跟團 escorted package');` and the 6 `coverage_block_reasons` rows. Wire `db_migrate.rs` to read+exec this file's statements (insert-if-absent) during migrate. Match how the repo already runs SQL (exec_lenient per statement).
- [ ] **Step 4: Run, verify passes.**
- [ ] **Step 5: Commit** `feat(db): seed product_types + coverage_block_reasons from SQL file`.

---

## Phase 1 — Write surface (CLI mutations, audited)

### Task 3: `catalog_runs` audit helper [DELEGATABLE → Grok]

**Files:**
- Create: `rust/crates/travel-cli/src/catalog_audit.rs`
- Modify: `rust/crates/travel-cli/src/main.rs` (`mod catalog_audit;`)
- Test: `rust/crates/travel-cli/tests/catalog_audit.rs`

**Interfaces:**
- Produces: `pub async fn record_catalog_run(conn: &libsql::Connection, command_type: &str, summary: &str, now: &str) -> Result<String, String>` — inserts one `catalog_runs` row (run_id generated like `cascade::common::new_run_id`), returns run_id. (Global analogue of `cascade::common::record_operation`, but NO `plans.version` bump.)

- [ ] **Step 1: Failing test** — call the binary path that uses it (or a thin unit test): after a catalog mutation, exactly one `catalog_runs` row with the expected `command_type`. (If no command exists yet, write the unit test against `record_catalog_run` via a `#[tokio::test]` that opens a write conn, calls it, SELECTs.)
- [ ] **Step 2: Run, verify fails.**
- [ ] **Step 3: Implement** `record_catalog_run` (copy the shape of `cascade::common::record_operation`, drop the `plans` UPDATE; reuse `new_run_id`/`now_db_datetime`).
- [ ] **Step 4: Run, verify passes.**
- [ ] **Step 5: Commit** `feat(cli): catalog_runs audit helper for global catalog mutations`.

### Task 4: `set-ota-source` + `set-ota-coverage` + `set-ota-region` commands [DELEGATABLE → Grok]

**Files:**
- Create: `rust/crates/travel-cli/src/set_ota_catalog.rs`
- Modify: `rust/crates/travel-cli/src/main.rs` (`mod` + 3 dispatch arms, mirror the `promote-offers` arm at line 156)
- Test: `rust/crates/travel-cli/tests/set_ota_catalog.rs`

**Interfaces:**
- Consumes: Task 1 tables, Task 3 `record_catalog_run`.
- Produces commands:
  - `set-ota-source <source_id> --name <n> --status active|inactive` → UPSERT `ota_sources` identity (name/status only), 1 `catalog_runs` row.
  - `set-ota-coverage <source_id> <product_type> [--proven] [--proven-at YYYY-MM-DD] [--method agent_parse|regex] [--search-url <u>] [--blocked <reason_code>]` → UPSERT `ota_source_coverage`; ENFORCE `--proven ⇒ --proven-at AND --method` (fail loud otherwise); ENFORCE `product_type ∈ product_types` and `--blocked ∈ coverage_block_reasons` (SELECT-validate, fail loud); 1 `catalog_runs` row.
  - `set-ota-region <source_id> <product_type> <region_label> <region_code>` → UPSERT `ota_source_region_codes`; 1 `catalog_runs` row.

- [ ] **Step 1: Failing test** (`tests/set_ota_catalog.rs`, `mod common; use common::Guard;`): seed a source; run `set-ota-coverage acme fit --proven --proven-at 2026-06-29 --method agent_parse --search-url http://x`; assert the coverage row has proven=1/proven_at/method/search_url; assert `set-ota-coverage acme fit --proven` WITHOUT --proven-at/--method exits non-zero and writes nothing; assert a bad product_type fails loud. Teardown via Guard.
- [ ] **Step 2: Run, verify fails.**
- [ ] **Step 3: Implement** the three commands (arg parse like `promote_offers::parse_args`; SELECT-validate; UPSERT; `record_catalog_run`). Plain-text output only.
- [ ] **Step 4: Run, verify passes.**
- [ ] **Step 5: Commit** `feat(cli): set-ota-source/coverage/region — audited catalog mutations`.

---

## Phase 2 — Read surface + migration of existing facts

### Task 5: `travel ota-status` view [DELEGATABLE → Grok]

**Files:**
- Create: `rust/crates/travel-cli/src/ota_status.rs`
- Modify: `rust/crates/travel-cli/src/main.rs` (`mod` + dispatch arm)
- Test: `rust/crates/travel-cli/tests/ota_status.rs`

**Interfaces:**
- Consumes: Task 1 tables.
- Produces: `ota-status [--type flight|hotel|fit|group_tour]` → plain-text table from the JOIN of `ota_source_coverage` × `ota_sources` × `coverage_block_reasons` (columns: source, type, proven, proven_at, method, blocked, search_url). Read-only (`connect_read`).

- [ ] **Step 1: Failing test** — seed a coverage row, run `ota-status`, assert the row appears with its fields; `--type fit` filters. Guard teardown.
- [ ] **Step 2: Run, verify fails.**
- [ ] **Step 3: Implement** the SELECT JOIN + plain-text render (mirror an existing view like `view_*`/`offers.rs`).
- [ ] **Step 4: Run, verify passes.**
- [ ] **Step 5: Commit** `feat(cli): ota-status — DB-native provider coverage view`.

### Task 6: Re-key `parser_rules` per product type [NOT DELEGATABLE — schema migration of live data; Claude does this]

**Files:**
- Modify: `rust/crates/travel-cli/src/db_migrate.rs` (alter parser_rules), `add_besttour_offer.rs:159`, any reader of `product_kind`
- Test: `rust/crates/travel-cli/tests/parser_rules_rekey.rs`

**Why not delegatable:** SQLite can't rename a PK in place — needs create-new-table → copy → drop → rename, on a LIVE table with existing rows, plus reconciling besttour/travel4u `fit`→`group_tour`. Risk of data loss; requires judgment on each existing row. Claude executes with a Turso backup check first.

(Steps authored at execution time after a `SELECT * FROM parser_rules` snapshot; this task is a placeholder boundary, not delegated.)

### Task 7: Two-phase `notes` → coverage backfill [NOT DELEGATABLE — per-source human-judgment parse; Claude does this]

**Files:**
- Create: `rust/crates/travel-cli/src/db_migrate.rs` (add `ota_notes_migration_audit` table), a one-shot backfill (run via `db exec` or a `db seed` step)
- Test: `rust/crates/travel-cli/tests/notes_backfill.rs`

**Why not delegatable:** parsing 14 free-text `notes` into typed coverage/region rows requires reading each sentence and deciding proven/method/url/region/blocked vs. discard-as-recipe — judgment, not mechanical. Claude parses each, records raw+checksum+disposition in `ota_notes_migration_audit`, KEEPS `notes` (deprecated, read-only) this release. `notes` DROP is a SEPARATE later release after the audit table confirms full coverage.

---

## Phase 3 — Make DB authoritative + demote docs

### Task 8: Stop migrate-time overwrite of live catalog rows [NOT DELEGATABLE — changes seed semantics; Claude does this]

**Files:**
- Modify: `rust/crates/travel-cli/src/db_migrate.rs` (`seed_ota_sources` ~1198: ON CONFLICT DO UPDATE → insert-if-absent; child DELETE+reinsert ~1212-1218 → insert-if-absent)
- Test: `tests/ota_sources_seed_sync.rs` (this test currently asserts the OPPOSITE — that migrate RE-syncs notes; it must be inverted/replaced to assert migrate does NOT clobber a live edit)

**Why not delegatable:** this intentionally reverses behavior a committed test (`ota_sources_seed_sync.rs`, this session) currently asserts; requires understanding why that test existed and rewriting its intent. Claude does it.

### Task 9: Repoint `validate data` + demote CLAUDE.md to a pointer [NOT DELEGATABLE — touches the consistency contract; Claude does this]

**Files:**
- Modify: `rust/crates/travel-cli/src/validate.rs` (`parse_claude_ota_table` ~422, the ✅/scrape-only compare ~514) → assert CLAUDE.md contains ONLY the pointer + run DB-native coverage checks
- Modify: `CLAUDE.md` (replace OTA status table + sweep prose with: "Provider coverage is DB data — `travel ota-status`")
- Test: `tests/validate_ota_pointer.rs`

**Why not delegatable:** changing the consistency check + the canonical doc is the load-bearing "docs stop being the record" step; needs care that `validate data` still passes the pre-commit hook. Claude does it.

---

## Self-Review

- **Spec coverage:** product_types (T2) ✓, ota_source_coverage (T1) ✓, region_codes (T1) ✓, block_reasons (T1/T2) ✓, catalog_runs (T1/T3) ✓, CLI mutations (T4) ✓, ota-status view (T5) ✓, parser_rules re-key (T6) ✓, seed-from-SQL (T2) ✓, two-phase notes (T7) ✓, stop migrate-overwrite (T8) ✓, repoint validate + demote CLAUDE.md (T9) ✓, offers.type unchanged (Global Constraints) ✓.
- **Delegation split:** T1–T5 are self-contained (new tables/commands/view, clear test oracle) → Grok-able. T6–T9 mutate live data / reverse existing test intent / touch the consistency contract → Claude (judgment + backup needed).
- **Placeholder note:** T6/T7 steps are intentionally authored at execution time (they depend on a live-data snapshot) — flagged NOT-delegatable, not left as vague TODOs for a delegate.

---

## Post-review fixes (2026-06-29)

After Tasks 3–9 were committed and pushed, a line-by-line review against source + the live DB
(not just "the suite is green") surfaced three regressions the green suite had masked because the
catalog rows already existed in the shared production Turso DB. All fixed in commits `49931bc`
(chromeport) and `668ce24` (db). Review write-up: `tmp/claude-review-codex-tasks-3-9.md`.

- **F1 (was BLOCKER) — T7 had no backfill, only `CREATE TABLE`.** The coverage/region/audit rows
  existed only from manual `set-ota-*` CLI runs, not reproducible from the tree; a fresh / re-seeded
  DB would have rendered an empty `ota-status`. FIXED: checked-in `scripts/seed/ota_coverage.seed.sql`
  (14 coverage + 10 region rows, INSERT-OR-IGNORE, run on migrate) + a `backfill_ota_notes_audit()`
  migrate step that records each note + sha256 checksum + `disposition='normalized'`, idempotent via
  `NOT EXISTS` and self-healing the duplicate audit rows the naive insert had been appending every
  migrate (the audit table has no PK). Also fixed a latent seed-splitter trap (a `;` inside a comment
  silently dropped one coverage row).
- **F2 (MUST-FIX) — T6 re-key could lose data.** `migrate_parser_rules_product_type` copied rows
  with `exec_lenient` then DROPped the original unconditionally, so a swallowed copy failure would
  replace the live table with an empty one. FIXED: error-propagating `exec` for the copy + a
  row-count guard (`1 ≤ new ≤ old`) before the DROP; aborts and keeps the original otherwise.
- **F3 (dead-crate, real) — chromeport still wrote the OLD `parser_rules` schema.** Its retired
  `parse`/`verify`/`parser rules` subcommands assumed single-PK `(source_id)` + `product_kind` and
  would break against the re-keyed table. FIXED: those subcommands now fail loud (exit 1, point to
  gwebcdb); the dead OTA parser stage (~1.6k lines) was deleted. `snapshot-maps`' browser/screenshot/db
  path is untouched.

**Verification:** new in-memory libsql unit tests for F1 (seed populates 14/10 from code; backfill
idempotent + dedups) and F2 (re-key preserves rows, reconciles besttour/travel4u → group_tour, is
idempotent, count_rows guard) — no shared-DB mutation, so no cross-test collision. Full serial suite:
30 binaries, all green. `validate data` 0/0/0; live DB 14 coverage / 14 audit / 10 region.
