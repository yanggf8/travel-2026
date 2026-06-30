# Rust-First OTA Execution — Implementation Fan-Out Plan

> **STATUS: READY TO FAN OUT (2026-06-29).** This plan implements
> `docs/superpowers/specs/2026-06-29-rust-first-ota-db-architecture-design.md` after the
> Codex/Claude corroboration pass. It uses the corrected order: schema/DAL/job lifecycle first, then
> parse/write-offers under a real claimed job.

**Goal:** Move OTA execution from the gwebcdb Python bridge into Rust without weakening the repo's DB
contract: jobs, attempts, observations, offer provenance, and parser outputs are normalized Turso rows;
the only free-text payloads remain `captures.raw_text`, agent TSV before normalization, and human error
messages.

**Source facts checked before planning:**
- `chromeport` no longer contains the stale `product_kind` parser-rules reader/writer; only a comment
  references the retired OTA parse path (`rust/crates/chromeport/src/main.rs:21-29`).
- `parser_rules` is already keyed by `(source_id, product_type)` in schema
  (`scripts/schema.sql:565-583`) and migrated in `db_migrate.rs`.
- `offers` still has no execution provenance columns and keeps its original `type` CHECK
  (`scripts/schema.sql:516`).
- No `ota_jobs` / `ota_attempts` / `ota_observations` execution tables exist yet.
- `sha2` is already available in `travel-cli` (`rust/crates/travel-cli/Cargo.toml:17`) for checksums.

## Global Constraints

- **DB is sole source of truth.** No JSON files, JSON DB columns, blobs, facts in Rust const arrays, or
  status facts in docs.
- **Plain text CLI.** User-facing output is plain text/table lines. TSV is allowed only as the transient
  agent-parse input before normalization into rows.
- **Real Turso tests.** Integration tests in `rust/crates/travel-cli/tests/*.rs` must skip cleanly if
  creds are absent and must use `mod common; use common::Guard;` for any rows they mutate.
- **Shared production DB caution.** Test rows use `zz*` / `test-*` identifiers and are cleaned by Guard.
  Never delete real plans or real offers.
- **Per-task commits.** Each task ends green and is committed separately.
- **Build/test oracle:** `make check` plus the named integration test. Before handoff/final merge, run
  `cd rust && cargo test -p travel-cli`.
- **Env:** export `TRAVEL_TURSO_URL`, `TRAVEL_TURSO_READ_TOKEN`, and `TRAVEL_TURSO_WRITE_TOKEN` from
  `.env` before live tests.

## Corrected Dependency Graph

The implementation order is:

1. **T1/T2 schema**: execution tables + offer provenance.
2. **T3 DAL skeleton**.
3. **T4 core repositories**: offers, captures, parser_rules, observations, jobs/attempts.
4. **T5/T6 job lifecycle**: enqueue, claim, heartbeat, terminal, reap.
5. **T7/T8 execution**: Rust regex parse and agent TSV write-offers under a real job.
6. **T9 read view**: observations.
7. **T10 opportunistic DAL adoption**: `view_bookings`.
8. **T11 deferred**: D1 read-mirror/compatibility pilot, not started without sign-off.

This deliberately puts job/attempt lifecycle before parse/write-offers. Do not implement the stale
embedded spec order that parses before job lifecycle.

---

## T1 — Execution Schema Tables [DELEGATABLE]

**Files**
- Modify: `rust/crates/travel-cli/src/db_migrate.rs`
- Add test: `rust/crates/travel-cli/tests/ota_execution_schema.rs`

**Add tables**
- `ota_jobs`
- `ota_job_params`
- `ota_attempts`
- `ota_observations`

**Required DDL shape**
- `ota_jobs`: `job_id TEXT PRIMARY KEY`, `source_id`, `product_type`, `status`, `claimed_by`,
  `claimed_at`, `claim_token`, `lease_expires_at`, `heartbeat_at`, `attempts`, `max_attempts`,
  `next_retry_at`, `blocked_reason_code`, `created_at`, `updated_at`.
- `ota_job_params`: `job_id`, `param_key`, `param_value`, PK `(job_id, param_key)`, and DB-level
  `CHECK(param_key IN ('depart_date','return_date','nights','pax','region_code','region_label'))`.
- `ota_attempts`: `attempt_id TEXT PRIMARY KEY`, `job_id`, `attempt_no`, `claim_token`, `outcome`,
  `capture_id`, `candidate_count`, `inserted_count`, `deduped_count`, `error_detail`, `started_at`,
  `finished_at`, UNIQUE `(job_id, attempt_no)`.
- `ota_observations`: columns from spec, including typed nullable facts:
  `http_status`, `field_name`, `selector`, `expected_value`, `observed_value`, `duration_ms`,
  `freshness_reference_at`; `detail` is human-message-only.

**Checks**
- `ota_jobs.status IN ('queued','claimed','running','succeeded','failed','blocked')`.
- `status!='blocked' OR blocked_reason_code IS NOT NULL`.
- `ota_attempts.outcome IN ('succeeded','failed','blocked')`.
- `ota_observations.observation_type` and `severity` enums as in the spec.
- No `AUTOINCREMENT`; no `REFERENCES` unless the repo changes its no-FK-enforcement stance.

**Test oracle**
- `db migrate` creates all four tables.
- `db schema <table>` shows the documented columns.
- Bad `ota_jobs.status`, bad `ota_job_params.param_key`, bad `ota_observations.observation_type`, and
  blocked-without-reason inserts fail.
- Guard cleanup for any `zz*` rows.

**Commit:** `feat(db): add OTA execution job attempt observation tables`

---

## T2 — Offer Provenance Columns [CLAUDE]

**Files**
- Modify: `rust/crates/travel-cli/src/db_migrate.rs`
- Add test: `rust/crates/travel-cli/tests/offers_provenance_cols.rs`

**Add nullable columns to `offers`**
- `capture_id TEXT`
- `produced_by_job_id TEXT`
- `produced_by_attempt_id TEXT`
- `parser_method TEXT CHECK(parser_method IS NULL OR parser_method IN ('agent_parse','regex'))`
- `capture_checksum TEXT`
- `parser_rule_checksum TEXT`
- `normalizer_version TEXT`

**Why Claude:** live load-bearing table; must preserve `(id, scraped_at)` PK and all existing insert/read
paths.

**Test oracle**
- Columns exist after migrate.
- A legacy insert that omits the new columns still succeeds.
- A provenance-bearing insert round-trips all seven new columns.
- `parser_method='bad'` fails.

**Commit:** `feat(db): add reproducible OTA provenance to offers`

---

## T3 — `travel-db` Crate Skeleton [CLAUDE]

**Files**
- Add: `rust/crates/travel-db/Cargo.toml`
- Add: `rust/crates/travel-db/src/lib.rs`
- Add: `rust/crates/travel-db/src/repo/mod.rs`
- Modify: `rust/Cargo.toml`
- Modify: `rust/crates/travel-cli/Cargo.toml`

**Boundary contract**
- `travel-db` owns business-table SQL and row mapping.
- `travel-cli` keeps orchestration and operational SQL: migrate, `db exec`, `db schema`, `validate`,
  and plan audit/event composition via `cascade::common`.
- `travel-db` does not write `plan_events`, `operation_runs`, or bump `plans.version`.

**Test oracle**
- `cd rust && cargo build -p travel-db`
- `make check`
- A small unit test can construct a public row struct.

**Commit:** `feat(travel-db): add DAL crate skeleton`

---

## T4 — Core Repositories [CLAUDE]

**Files**
- Add: `rust/crates/travel-db/src/repo/offers.rs`
- Add: `rust/crates/travel-db/src/repo/captures.rs`
- Add: `rust/crates/travel-db/src/repo/parser_rules.rs`
- Add: `rust/crates/travel-db/src/repo/observations.rs`
- Add: `rust/crates/travel-db/src/repo/ota_jobs.rs`
- Add tests under `rust/crates/travel-db/src/` or `rust/crates/travel-cli/tests/` as appropriate.

**Interfaces**
- `repo::offers::{insert, latest}` with typed `OfferRow` including provenance columns.
- `repo::captures::get(conn, capture_id)` returns source/url/raw_text and computes or allows computing
  `capture_checksum`.
- `repo::parser_rules::get(conn, source_id, product_type)` reads the current `(source_id, product_type)`
  row and exposes deterministic checksum input.
- `repo::observations::record`.
- `repo::ota_jobs::{enqueue, claim, heartbeat, mark_running, finish, reap_stale, next_attempt_no}`.

**Important implementation details**
- Bound params only.
- Offer insert must return inserted-vs-deduped count.
- Parser-rule checksum must be deterministic: use a stable ordered field string, not debug formatting.
- `finish` is token-guarded: `WHERE job_id=? AND claim_token=? AND status IN ('claimed','running')`.
- Reap clears token and lease fields.

**Test oracle**
- Repository tests use live Turso only when needed; pure row/checksum helpers can be unit tests.
- Verify `parser_rules` lookup requires product type.
- Verify stale `claim_token` terminal update affects zero rows.

**Commit:** `feat(travel-db): add OTA core repositories`

---

## T5 — `travel ota enqueue` [DELEGATABLE AFTER T4]

**Files**
- Add: `rust/crates/travel-cli/src/ota/mod.rs`
- Add: `rust/crates/travel-cli/src/ota/enqueue.rs`
- Modify: `rust/crates/travel-cli/src/main.rs`
- Add test: `rust/crates/travel-cli/tests/ota_enqueue.rs`

**Command**
`travel ota enqueue <source_id> <product_type> [--depart YYYY-MM-DD] [--return YYYY-MM-DD] [--nights N] [--pax N] [--region-code C] [--region-label L]`

**Behavior**
- Validate `source_id` exists in `ota_sources`.
- Validate `product_type` exists in `product_types`.
- Validate param keys are the closed DB enum.
- Insert one queued `ota_jobs` row plus `ota_job_params`.
- Plain-text output includes `job_id` and status.

**Test oracle**
- Enqueue creates exactly one `zz*` job and expected params.
- Bad source/type/param fails loud and writes nothing.

**Commit:** `feat(cli): add travel ota enqueue`

---

## T6 — Claim / Heartbeat / Terminal / Reap [CLAUDE]

**Files**
- Add: `rust/crates/travel-cli/src/ota/claim.rs`
- Add: `rust/crates/travel-cli/tests/ota_claim.rs`
- Modify: `main.rs`

**Commands**
- `travel ota claim [--worker <id>] [--lease-seconds N]`
- `travel ota heartbeat <job_id> <claim_token> [--lease-seconds N]`
- `travel ota finish <job_id> <claim_token> --status succeeded|failed|blocked [--blocked <code>]`
- `travel ota reap-stale [--now <iso>]`

**Semantics**
- Claim is `UPDATE ... WHERE status='queued'` and mints `claim_token`, `claimed_at`,
  `heartbeat_at`, `lease_expires_at`.
- Heartbeat extends the lease only when token matches.
- Terminal update succeeds only when token matches.
- Reap requeues only expired leases and clears `claim_token`, `claimed_by`, `claimed_at`,
  `lease_expires_at`.
- Do not implement the old `claimed_at < now - ttl` reclaim.

**Test oracle**
- N concurrent claims through separate write connections: exactly one wins.
- Stale token heartbeat/finish after reap affects zero rows.
- Live heartbeat prevents reap.
- Blocked terminal status requires valid `blocked_reason_code`.

**Commit:** `feat(cli): add token-guarded OTA claim lifecycle`

---

## T7 — Rust Regex Parse Under A Real Job [CLAUDE]

**Files**
- Add: `rust/crates/travel-cli/src/ota/parse.rs`
- Add test: `rust/crates/travel-cli/tests/ota_parse.rs`
- Modify: `ota/mod.rs`, `main.rs`

**Command**
`travel ota parse <job_id> <capture_id> --source <source_id> --product-type <product_type> --claim-token <token> [--dry-run]`

**Behavior**
- Validate job exists and token matches.
- Load capture raw text.
- Verify capture source matches `--source` unless an explicit override flag is later added.
- Load parser rule for `(source_id, product_type)`.
- Compute `capture_checksum`, `parser_rule_checksum`, and `normalizer_version`.
- Parse into typed `offers` rows; map `fit|group_tour -> offers.type='package'`.
- Insert offers with `produced_by_job_id`, `produced_by_attempt_id`, checksums, and `parser_method='regex'`.
- Record `ota_attempts` with candidate/inserted/deduped counts.
- Record typed `ota_observations` for parse warnings.
- Finish the job token-guarded only if parse was not `--dry-run`.

**Test oracle**
- Seed one capture and parser rule under `zz*`.
- Enqueue/claim, run parse, assert offer rows carry provenance/checksums.
- Re-run parse and assert deduped count increments instead of failing.
- Stale token parse fails before writing offers.

**Commit:** `feat(cli): add travel ota parse under token-guarded job`

---

## T8 — Agent TSV `write-offers` [CLAUDE]

**Files**
- Add: `rust/crates/travel-cli/src/ota/write_offers.rs`
- Add test: `rust/crates/travel-cli/tests/ota_write_offers.rs`
- Modify: `ota/mod.rs`, `main.rs`

**Command**
`travel ota write-offers <job_id> --capture <capture_id> --claim-token <token> --tsv <path>`

**Behavior**
- TSV is a transient agent output, not a source of truth.
- Validate TSV headers and normalize into typed `offers` columns.
- Compute the same capture checksum; parser checksum can be NULL or a deterministic `agent_parse:<version>`
  checksum if no parser_rules row was used.
- Insert offers with `parser_method='agent_parse'`.
- Record attempt counts and finish token-guarded.

**Test oracle**
- Fixture TSV produces expected typed offer rows.
- Bad header / malformed price fails loud and writes no offers.
- Stale token writes nothing.

**Commit:** `feat(cli): add travel ota write-offers`

---

## T9 — Observations View [DELEGATABLE AFTER T4]

**Files**
- Add: `rust/crates/travel-cli/src/ota/observations.rs`
- Add test: `rust/crates/travel-cli/tests/ota_observations_view.rs`
- Modify: `ota/mod.rs`, `main.rs`

**Command**
`travel ota observations [--source <id>] [--type <observation_type>] [--severity info|warn|error]`

**Behavior**
- Read-only plain-text table from `ota_observations`.
- Show typed columns when populated; do not pack them into `detail`.

**Test oracle**
- Seed `zz*` observations, assert filters and rendered typed facts.

**Commit:** `feat(cli): add OTA observations view`

---

## T10 — First Existing Command DAL Migration: `view_bookings` [OPTIONAL / AFTER T3]

**Files**
- Add: `rust/crates/travel-db/src/repo/bookings.rs`
- Modify: `rust/crates/travel-cli/src/view_bookings.rs`
- Add or extend a golden-output test.

**Behavior**
- Replace local `sql_quote` + `format!` query with bound params in `repo::bookings::book_by`.
- CLI output must be byte-identical.

**Commit:** `refactor(travel-db): route view_bookings query through DAL`

---

## T11 — D1 Compatibility Contract [DEFERRED; DO NOT START WITHOUT SIGN-OFF]

**Scope**
- Write a small contract doc/test harness for the SQL subset actually intended to port:
  token-guarded claim/heartbeat/finish/reap, simple offer reads, and read-only dashboard queries.
- Do not move OTA execution to D1 or Workers.
- Do not attempt full DB migration; existing migrations use SQLite/libSQL-specific operational SQL.

---

## Parallelization Notes

- T1 and T3 can run in parallel.
- T2 can run after or alongside T1, but must merge before T4 `repo::offers`.
- T5 and T9 can be delegated once T4 lands.
- T6 is not delegatable; it is the concurrency-critical path.
- T7/T8 must wait for T5/T6 because they run under real jobs and claim tokens.
- T10 is optional and should not block Rust OTA execution.

## Final Gate

After T9:

```bash
make check
cd rust && cargo test -p travel-cli
./bin/travel validate data
./bin/travel doctor
```

Then run a live smoke path on test-prefixed rows:

```bash
./bin/travel ota enqueue zztest fit --depart 2026-07-01 --nights 4 --pax 2
./bin/travel ota claim --worker codex-smoke
./bin/travel ota observations --source zztest
```

Clean all `zz*` rows afterward.
