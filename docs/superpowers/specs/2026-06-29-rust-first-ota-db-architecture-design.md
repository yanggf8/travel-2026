# Design: Rust-First OTA Execution — Jobs, Observations, Offers & the Shared DAL

**Date:** 2026-06-29
**Status:** LARGELY IMPLEMENTED (as of 2026-06-30). Phases A–E shipped (schema, `travel-db` DAL
crate + core repos, `enqueue`/`claim`/`heartbeat`/`finish`/`reap-stale`, `parse` incl. the **settour
custom parser** in `ota/settour_parse.rs`, `write-offers`, `observations` view), hardened by a
multi-agent xhigh code review (14 defects fixed). Phase F (DAL adoption) STARTED — `view_bookings`
migrated to `repo::bookings`. Still open: live end-to-end WSLg per-source verification, the
`besttour`/`travel4u` custom-parser ports, the gradual DAL migration of the remaining ~57 modules,
and the gated Phase G D1 pilot. Companion to the committed
`2026-06-29-db-centric-provider-architecture-design.md` (the *catalog* spec); this spec covers the
*execution* layer the catalog spec explicitly left out.
**Author:** Claude (with Yang)

> **Scope orientation (read first).** Plan tools are **already** Rust (`./bin/travel` is the sole
> write path; the TS CLI is archived read-only — CLAUDE.md "CLI Execution"). The provider *catalog &
> coverage* is normalized and committed (the catalog spec, Tasks 1–5). What remains unbuilt is **OTA
> *execution* as Rust** — the capture→parse→offer path is still Python in gwebcdb — plus the shared
> **DAL** both plan and OTA tools should sit on, and the **D1-portability** rules that keep a later
> Cloudflare move cheap. This spec designs exactly those, and nothing already done.

## The principle (inherited from the catalog spec, restated for execution)

Every OTA execution artifact — a queued job, each attempt, every block/warning/freshness signal, the
produced offer, and its provenance back to the raw capture — **must be a queryable relational row.**
The ONE allowed free-text payload is the LLM's raw input/output (`captures.raw_text`, and the agent's
TSV before it is normalized into typed `offers` rows). Hard rules carried over verbatim:

- **No JSON.** No `*_json` columns, no JSON-encoded cells.
- **No blobs.** No opaque payloads.
- **No prose-as-data.** Structured facts get typed columns/child rows, never a packed sentence.
- **No facts in code.** No provider/job state born in a Rust `const`.
- **No facts in documents.** Status/coverage/job-state is DB data behind a CLI view.

Two execution-specific rules this spec adds:

- **D1-portable by construction.** New `ota_*` tables use TEXT or composite PKs (run-id style via
  `cascade::common::new_run_id`), **no `AUTOINCREMENT`** (the wider schema uses it at
  `schema.sql:156,417,699,857`; Cloudflare discourages it), no JSON1-dependent logic, and concurrency
  via **claim-by-conditional-UPDATE** (never `SELECT … FOR UPDATE` or advisory locks — neither libSQL
  remote nor D1 offers them).
- **OTA execution is local-only.** Capture needs a real browser (gwebcdb on WSLg) and parse needs the
  LLM agent; neither runs in a Cloudflare Worker. Cloudflare's scope stays **publish/read only**.

## The gaps (what the catalog spec left out, verified against source 2026-06-29)

| # | Gap | Evidence |
|---|-----|----------|
| G1 | **No Rust capture→parse→offer path.** Only `import_offers` (JSON files) + `promote_offers` write offers; the parse path is Python (`gwebcdb/bridge/ota_parse.py`). No Rust module SELECTs `captures.raw_text` to write `offers`. | `grep "FROM captures" rust/crates/travel-cli/src/` → only `flights.rs`/plumbing; CLAUDE.md "URL Routing" puts parse in gwebcdb |
| G2 | **No OTA job queue.** `operation_runs` is an audit row (`plan_id NOT NULL`, `status IN ('started','completed','failed')`, no claim/attempt/lease) — cannot model a claimable, retryable, *global* OTA job. | `scripts/schema.sql:518-531` |
| G3 | **No observation/provenance rows.** Blocks/captcha/login-wall/parse-warnings/freshness live as prose in `ota_sources.notes` and CLAUDE.md, not as queryable rows. | catalog spec V2/V3; `db_migrate.rs:1224` |
| G4 | **No capture↔offer linkage.** `offers` has no `capture_id`/job/parser-method columns, so a produced offer is not traceable to the raw capture or the run that made it — not reproducible. | `scripts/schema.sql:516` (offer columns) |
| G5 | **No DAL.** Every command writes raw inline SQL via `db::connect_read/write`; `view_bookings.rs:71-95` even hand-escapes with `sql_quote()`+`format!`. SQL is scattered across ~58 modules. | `rust/crates/travel-cli/src/db.rs:82-104`; `view_bookings.rs:71-95` |

## Design

### 1. The OTA job lifecycle — `ota_jobs` + `ota_attempts` (kills G2)

A **job** = "produce offers for one `(source_id, product_type)` search with given params." Jobs are
**global** (not plan-scoped); a plan consumes the *resulting offers* later via the existing bridge.

**`ota_jobs`** (parent — one row per requested search):
- `job_id TEXT PRIMARY KEY` — run-id style (`new_run_id()`)
- `source_id TEXT NOT NULL` — the OTA provider
- `product_type TEXT NOT NULL` — FK-by-convention → `product_types.code` (the catalog spec's
  canonical list; CLI-validated, not a hard FK, consistent with the repo's no-`foreign_keys` posture)
- `status TEXT NOT NULL DEFAULT 'queued'
   CHECK(status IN ('queued','claimed','running','succeeded','failed','blocked'))`
- `claimed_by TEXT` — worker/agent identity (NULL until claimed)
- `claimed_at TEXT` — ISO; NULL until claimed
- `claim_token TEXT` — **ownership token** (random per claim, e.g. `new_run_id()`); NULL when not
  claimed. This is the ABA guard (Codex second-opinion Part A): every heartbeat / terminal
  (`succeeded`/`failed`/`blocked`) / reap update is gated on `claim_token`, so a worker whose lease
  was reclaimed CANNOT overwrite the result of the worker that re-claimed the job.
- `lease_expires_at TEXT` — ISO; the lease deadline (NOT `claimed_at - ttl`). Reap requeues only when
  `lease_expires_at < now`, so a slow-but-live worker that heartbeats is never stolen from.
- `heartbeat_at TEXT` — ISO; last liveness ping. Heartbeat extends `lease_expires_at`.
- `attempts INTEGER NOT NULL DEFAULT 0` — bumped per attempt
- `max_attempts INTEGER NOT NULL DEFAULT 3`
- `next_retry_at TEXT` — ISO; NULL unless backing off
- `blocked_reason_code TEXT` — → `coverage_block_reasons.code` (reuses the catalog lookup) when
  `status='blocked'`
- `created_at TEXT NOT NULL`, `updated_at TEXT NOT NULL`
- **CHECK:** `status='blocked' ⇒ blocked_reason_code IS NOT NULL` (clean SQLite:
  `CHECK(status!='blocked' OR blocked_reason_code IS NOT NULL)`).

**`ota_job_params`** (child — the search inputs as rows, never a URL string blob; kills "params hiding
in a URL"):
- `job_id TEXT NOT NULL`, `param_key TEXT NOT NULL`, `param_value TEXT NOT NULL`,
  PK `(job_id, param_key)`. **Closed key set** validated by the CLI: `depart_date`, `return_date`,
  `nights`, `pax`, `region_code`, `region_label`. (This is a *closed enum of keys* with the CLI
  rejecting anything else — NOT an open EAV bag; see §3 on why that distinction matters.)

**`ota_attempts`** (child — one row per try, so retries are rows not a mutated counter):
- `attempt_id TEXT PRIMARY KEY` (`new_run_id()`)
- `job_id TEXT NOT NULL`
- `attempt_no INTEGER NOT NULL` — 1-based
- `claim_token TEXT NOT NULL` — the `ota_jobs.claim_token` this attempt ran under, so a terminal
  result can be proven to belong to the worker that actually held the lease (Codex Part A).
- `outcome TEXT NOT NULL CHECK(outcome IN ('succeeded','failed','blocked'))`
- `capture_id TEXT` — → `captures.capture_id` when a capture was produced (the provenance link)
- `candidate_count INTEGER NOT NULL DEFAULT 0`, `inserted_count INTEGER NOT NULL DEFAULT 0`,
  `deduped_count INTEGER NOT NULL DEFAULT 0` — so an `affected_row_count==0` dedup is recorded as a
  number, not lost (Codex Part C: distinguishes a real dedup from a silently-dropped parser regression).
- `error_detail TEXT` — the ONE free-text column (a human error message; the allowed text payload)
- `started_at TEXT NOT NULL`, `finished_at TEXT`
- PK + `(job_id, attempt_no)` UNIQUE.

**Claim semantics (token-guarded; safe for libSQL remote AND D1).** No row locks exist on either
backend, so concurrency is **conditional UPDATE + affected-rows check**, and ownership is carried by a
random `claim_token` (Codex second-opinion Part A — the first `queued→claimed` transition was already
safe; the hazard was stale reclaim with no token).

```sql
-- 1) Claim: mint a token + lease, flip queued→claimed atomically.
UPDATE ota_jobs
   SET status='claimed', claimed_by=?, claim_token=?, claimed_at=?,
       heartbeat_at=?, lease_expires_at=?, updated_at=?
 WHERE job_id=? AND status='queued';
-- in Rust: execute(...).await? == 1 ⇒ we own it (and we know our token); 0 ⇒ lost the race.

-- 2) Heartbeat (extend the lease) — ONLY if we still own it.
UPDATE ota_jobs
   SET heartbeat_at=?, lease_expires_at=?, updated_at=?
 WHERE job_id=? AND claim_token=? AND status IN ('claimed','running');

-- 3) Terminal (succeeded/failed/blocked) — ONLY if we still own it.
UPDATE ota_jobs
   SET status=?, blocked_reason_code=?, updated_at=?
 WHERE job_id=? AND claim_token=? AND status IN ('claimed','running');

-- 4) Reap a crashed worker: requeue only EXPIRED leases, and CLEAR the token so the
--    old worker's terminal update (guarded on the old token) can no longer land.
UPDATE ota_jobs
   SET status='queued', claimed_by=NULL, claim_token=NULL,
       claimed_at=NULL, lease_expires_at=NULL, updated_at=?
 WHERE status IN ('claimed','running') AND lease_expires_at < ?;
```

`libsql::Connection::execute` returns the affected-row count (D1 exposes the same via `meta.changes`),
so each step's `== 1` proves the guard held. The token closes the ABA hazard: after a reap requeues
and a *new* worker claims (minting a fresh token), the *old* worker's heartbeat/terminal updates match
0 rows (their token was cleared) and silently no-op instead of clobbering the new result. Reap keys on
`lease_expires_at`, not `claimed_at - ttl`, so a slow-but-live worker that keeps heartbeating is never
stolen from. No daemon: `travel ota reap-stale` is an agent-run CLI (agent-first posture). The
`ota_attempts.claim_token` records which lease produced each terminal result.

**D1 note (Codex Part A / Part E):** "ports with a backend adapter," not "unchanged" — affected-row
counts match, but the transaction/execution envelope differs, and `db_exec` already special-cases
`RETURNING` (`db_exec.rs:160,180`). The pilot must run this exact claim SQL on D1, not assume parity.

### 2. `ota_observations` — typed signals, NOT EAV (kills G3)

One row per discrete signal observed during execution. **Typed, closed enum — explicitly not a
`(key,value)` bag** (an open bag would be the JSON-blob anti-pattern wearing a relational costume,
violating the inherited "no prose-as-data" rule).

**`ota_observations`:**
- `observation_id TEXT PRIMARY KEY`
- `source_id TEXT NOT NULL`, `product_type TEXT` (nullable — some signals are source-wide)
- `job_id TEXT`, `attempt_id TEXT` — link to the run that observed it (nullable for standalone checks)
- `observation_type TEXT NOT NULL CHECK(observation_type IN
   ('block','captcha','login_wall','render_error','rate_limit','parse_warning','freshness','empty_result'))`
- `block_reason_code TEXT` — → `coverage_block_reasons.code` when type is a block-class
- `severity TEXT NOT NULL CHECK(severity IN ('info','warn','error')) DEFAULT 'warn'`
- typed nullable fact columns (Codex Part B — keep structured facts OUT of `detail` so it can't become
  a key/value bag): `http_status INTEGER`, `field_name TEXT`, `selector TEXT`, `expected_value TEXT`,
  `observed_value TEXT`, `duration_ms INTEGER`, `freshness_reference_at TEXT`. Populate the ones a given
  `observation_type` needs (a `parse_warning` sets `field_name`/`expected_value`/`observed_value`; a
  `freshness` sets `freshness_reference_at`; a `block` sets `http_status`).
- `detail TEXT` — the ONE free-text column, a **human message ONLY** (e.g. "captcha appeared on search
  submit"). Structured facts like `selector=.foo; http=403; field=price` go in the typed columns above
  — putting them in `detail` is the same prose-as-data the catalog spec banned for `ota_sources.notes`.
- `observed_at TEXT NOT NULL`

**Why this is not EAV:** the *type* is a closed set of ≤8 values with known semantics, each row has the
same fixed typed columns, and the only free text is the human message. If a type needs structured
fields beyond those above, **add a typed column or split a table** (e.g. a dedicated
`ota_parse_warnings`), never add a generic `key`. (Contrast with the rejected design:
`ota_observations(entity, key, value)` — that *is* EAV and is forbidden.)

### 3. Offers + provenance — reuse `offers`, add traceability (kills G4; answers Q1/Q4)

**Decision: do NOT create a parallel `ota_offers` table.** The repo already has a working
research-landing → plan-bridge separation: global `offers` (`schema.sql:516`) is the durable,
unscoped research landing; `promote-offers` bridges it to plan-scoped `plan_offers`
(`schema.sql:705`); `shaping_*` is fed separately. A second offer table would create two sources of
truth for "an observed offer." Instead, **extend the offer-write path with provenance columns** so a
produced offer is reproducible:

- `offers.capture_id TEXT` — → `captures.capture_id` (which raw capture produced it)
- `offers.produced_by_job_id TEXT` — → `ota_jobs.job_id` (which run)
- `offers.produced_by_attempt_id TEXT` — → `ota_attempts.attempt_id` (which exact try)
- `offers.parser_method TEXT CHECK(parser_method IS NULL OR parser_method IN ('agent_parse','regex'))`
  — how it was extracted (mirrors `ota_source_coverage.method`)
- **Immutability checksums (Codex Part C / Part3.2 — necessary because captures and parser rules are
  MUTABLE):** `offers.capture_checksum TEXT` (sha256 of `captures.raw_text` at parse time),
  `offers.parser_rule_checksum TEXT` (sha256 of the `parser_rules` row used), `offers.normalizer_version
  TEXT` (the parser/normalizer code version). Without these, "same capture + same parser" is NOT
  reproducible: `chromeport` overwrites a capture on `ON CONFLICT(capture_id) DO UPDATE`
  (`chromeport/src/turso.rs:119`) and `parser_rules` rows are editable, so a `capture_id`/rule pointer
  alone can silently change meaning after an edit. The checksum pins the exact bytes that produced the
  offer.

These are additive nullable columns (existing rows/queries unaffected; consistent with the catalog
spec's "no CHECK/query change" discipline). With them, a single JOIN answers "show me the raw capture,
job, attempt, and parser behind this offer — and prove they haven't changed since" — full
reproducibility without a blob.

> If a clean break to a dedicated `ota_offers` is ever wanted, it must **supersede** `offers` via a
> migration (rename + repoint `import_offers`/`promote_offers`/`select_offer`) — **never run both**.
> This spec recommends *extend*, not *fork*.

### 4. The Rust capture→parse→offer port (kills G1 — the actual "rust-first OTA" milestone)

A new `travel ota` command family in a Rust `ota` module that finally reads `captures` + applies
`parser_rules` to write `offers` (today Python in gwebcdb):

- `travel ota parse <capture_id> --source <id> [--dry-run]` — load `captures.raw_text`, load the
  `parser_rules` row for `(source_id, product_type)` (re-keyed by the catalog spec §2a), apply the
  regex ruleset, write `offers` rows stamped with `capture_id`/`produced_by_job_id`/`parser_method`.
  Writes an `ota_attempts` row + `ota_observations` for any parse warning. `affected_row_count==0` on
  the offer INSERT is a real ON-CONFLICT dedup, not a failure (carry over the gwebcdb gotcha).
- `travel ota write-offers --job <job_id> --tsv <path>` — the **agent-parse** path: the agent reads a
  capture's `raw_text`, emits TSV, this command normalizes TSV → typed `offers` rows (the Rust analogue
  of `gwebcdb/bridge/ota_write_llm_offers.py`). The TSV is the allowed free-text artifact *before*
  normalization; once written, offers are typed rows.
- `travel ota enqueue <source> <product_type> [--depart … --nights … --region-code …]` — INSERT an
  `ota_jobs` row + `ota_job_params` children, status `queued`.
- `travel ota claim [--worker <id>]` / `travel ota reap-stale` — the claim + lease-reclaim primitives
  (§1).

**First source to port:** `settour` — it is PROVEN REAL and owns the *only* custom parser
(`has_custom_parser=1`, `parse_settour`), so porting it exercises both the generic-regex and
custom-parser branches in one source with a contained blast radius. Second wave:
`besttour`/`travel4u` (group_tour — also drives the `fit→group_tour` reconcile from the catalog spec).
Defer `liontravel`/`lifetour` (renderer-wedge, parked) and flight-only deferred sources.

### 5. The shared DAL — a new `travel-db` crate (kills G5)

Today SQL is hand-written inline in ~58 modules with no abstraction; `cascade::common` shares only the
audit/event primitives. Introduce a **`travel-db` library crate** in the existing workspace
(`rust/Cargo.toml`) as the single typed access layer:

- **Typed row structs** per table family (`OfferRow`, `OtaJobRow`, `OtaAttemptRow`, `PlanOfferRow`,
  …) — replaces positional `row.get(N)` unpacking scattered everywhere.
- **Repository functions** — `repo::offers::insert(...)`, `repo::ota_jobs::claim(worker)`,
  `repo::ota_jobs::enqueue(...)`, `repo::observations::record(...)` — each owning one bound-parameter
  SQL statement. This is where the `view_bookings.rs:71-95` `sql_quote()`+`format!` pattern dies:
  `repo::bookings::book_by(plan_id, dest)` binds params.
- **Boundary discipline:** `travel-db` owns all SQL strings + row mapping; `travel-cli` commands call
  repo functions and never write a SQL string. `cascade::common` stays in `travel-cli` as the
  *mutation/audit orchestration* on top of the DAL (it composes repo writes + event/version bumps —
  that logic is command-specific and stays out of the DAL).
- **turso-util stays plumbing** (token mint/connect/migrate); the DAL is a separate concern and a
  separate crate. `travel-db` depends on `turso-util` for connections.

This is a **gradual** migration (see below), not a big-bang rewrite: new `ota_*` commands are written
DAL-first; existing commands migrate module-by-module behind unchanged CLI behavior.

### 6. Workspace split

```
rust/
├── Cargo.toml                 # workspace members below
├── crates/
│   ├── turso-util/            # plumbing: token mint, connect, migrate runner (unchanged)
│   ├── travel-db/   (NEW)     # the DAL: typed rows + repository fns + all SQL strings
│   ├── travel-cli/            # commands: parse args → call repo fns → orchestrate (cascade::common)
│   └── chromeport/            # RETIRED — drop from default members (only snapshot_maps shells to it)
```

`chromeport` leaves the default workspace members (kept buildable on demand for `snapshot_maps` only,
or that capture moves to gwebcdb too — separate decision). The split is coherent for **all** plan +
OTA tools: every command, plan or OTA, parses args in `travel-cli` and reads/writes through
`travel-db`; the only difference is which repo module it calls.

### 7. Turso primary now; D1 later, gated (answers Q2)

- **Keep Turso primary.** It is the working single source of truth; creds/tooling exist; migrating now
  buys nothing and risks the live DB.
- **Do not rule D1 out.** The portability rules above (TEXT/composite PKs, no AUTOINCREMENT in new
  tables, claim-by-conditional-UPDATE, no JSON1 logic) keep the door cheap.
- **Pilot, don't migrate.** When/if D1 is pursued, scope a pilot to a **read-only mirror** of one or
  two tables behind the dashboard worker (publish-side) to measure the libSQL↔D1 SQL-dialect delta on
  real data. The **OTA write path never moves to D1/Workers** (OTA execution is local-only, §"Design"
  rule 2 / Q6). A pilot is a gate, not a commitment.

## The bridge: `ota_*` ↔ `shaping_*` / `plan_offer_*` (answers Q1, focus area #8)

Keep the separation the repo already has — this spec does **not** collapse research into decisions:

```
capture (raw_text)
   └─ travel ota parse / write-offers ─▶ offers  (global, durable, unscoped research landing;
                                                   now stamped capture_id/job/method)
                                            │
                  promote-offers ──────────┤──▶ plan_offers (+ child tables)  ──▶ select-offer (P3/P4)
                  shaping-import/baseline ──┘──▶ shaping_tour_group_offers / shaping_candidates ──▶ shaping-adopt
```

- OTA execution writes **only** `offers` (+ `ota_jobs`/`ota_attempts`/`ota_observations`). It never
  writes `plan_offers` or `shaping_*` directly.
- The existing audited bridges (`promote-offers`, `shaping-import`, `tour_group_bridge`) remain the
  *only* path from research into a plan/shaping run. They gain nothing new except richer provenance to
  read.
- This preserves the invariant "an observed offer" (research, re-promotable) ≠ "a selected offer"
  (decision, plan-scoped).

## What this explicitly does NOT do (scope guard)

- Does not re-port plan tools — they are already Rust (`./bin/travel`).
- Does not touch the committed catalog tables/spec (it builds on them: reuses `product_types`,
  `coverage_block_reasons`, the catalog CLI/view).
- Does not move OTA execution to Cloudflare (impossible: browser + agent are local).
- Does not migrate the DB to D1 (only keeps the door cheap + defines a later pilot).
- Does not collapse `offers`/`plan_offers`/`shaping_*` into one table (the separation is intentional).
- Does not rewrite all 58 modules at once (DAL adoption is gradual, behind unchanged CLI behavior).

## Migration / sequencing (additive, no big-bang)

Phased so each phase ships independently and nothing existing breaks:

1. **Phase A — schema (additive).** `CREATE TABLE IF NOT EXISTS` for `ota_jobs`, `ota_job_params`,
   `ota_attempts`, `ota_observations` in `db_migrate.rs` (mirror the catalog tables at
   `db_migrate.rs:647-683`); add nullable `capture_id`/`produced_by_job_id`/
   `produced_by_attempt_id`/`parser_method`/`capture_checksum`/`parser_rule_checksum`/
   `normalizer_version` to `offers`. No behavior change; real-Turso integration test asserts the
   tables/columns exist (panic-safe `Guard` teardown per CLAUDE.md).
2. **Phase B — `travel-db` crate skeleton + core repos.** Stand up the DAL with typed repos for offers,
   captures, parser_rules, observations, jobs, and attempts; route the new `ota` commands through it
   from day one (DAL-first).
3. **Phase D — job lifecycle before parsing.** `enqueue`/`claim`/`heartbeat`/`finish`/`reap-stale` +
   token-guarded conditional UPDATEs; tests assert exactly one of N concurrent claims wins, stale-token
   terminal updates no-op, and heartbeat prevents reap.
4. **Phase C — `travel ota parse`/`write-offers` for `settour`, under a real claimed job.** The G1
   milestone: capture→offer in Rust, stamped with job/attempt/checksum provenance, writing
   `ota_attempts`/`ota_observations`. Oracle: parity with the gwebcdb Python parse on the same capture
   (settour custom-parser).
5. **Phase E — observations + CLI view.** `travel ota observations [--source …]` plain-text view;
   repoint any prose status to the view (consistent with the catalog spec's docs-demotion).
6. **Phase F (gradual, ongoing) — DAL adoption for existing commands.** Migrate inline-SQL modules to
   `travel-db` one at a time behind unchanged CLI output; start with `view_bookings.rs` (retires the
   `sql_quote()`+`format!` pattern). No deadline; opportunistic.
7. **Phase G (deferred, gated) — D1 read-mirror pilot.** Only if/when pursued; publish-side, read-only,
   one or two tables.

## Implementation plan (task-by-task)

> Same conventions as the committed catalog plan (`docs/superpowers/plans/2026-06-29-db-centric-provider-architecture.md`):
> TDD per task (failing test → run-red → implement → run-green → commit); real-Turso integration tests
> with **panic-safe `Guard` teardown** (`mod common; use common::Guard;` — never a trailing `teardown()`);
> plain-text CLI output only; `make check` (build) + `cargo test -p travel-cli --test <name>` as the
> oracle; export `TRAVEL_TURSO_URL`/`TRAVEL_TURSO_READ_TOKEN`/`TRAVEL_TURSO_WRITE_TOKEN` for live tests.
> **Delegation:** `[→ Grok]` = self-contained new tables/commands with a clear test oracle (delegatable);
> `[Claude]` = touches live-data migration, concurrency correctness, or reverses existing behavior (judgment).
> Maps to the phases in "Migration / sequencing" above.

### Phase A — schema foundation (additive, no behavior change)

**Task A1: `ota_jobs` + `ota_job_params` + `ota_attempts` + `ota_observations` tables [→ Grok]**
- Files: modify `rust/crates/travel-cli/src/db_migrate.rs` (add `CREATE TABLE IF NOT EXISTS` blocks
  beside the catalog tables at `db_migrate.rs:647-683`); test `rust/crates/travel-cli/tests/ota_jobs_schema.rs` (new).
- DDL: exactly the four tables in §1–§2 (TEXT/composite PKs, **no AUTOINCREMENT**; the status/outcome/
  observation_type/severity CHECKs; the `status='blocked' ⇒ blocked_reason_code` and
  `status='blocked'`→reason implication CHECKs written clean per F4-style `CHECK(status!='blocked' OR …)`).
- Steps: (1) failing test — after `db migrate` each table exists, accepts a documented insert, and
  rejects a bad `status`/`observation_type` (CHECK) — assert via `db schema <table>` in the `db_seed.rs`
  test style, skip if credless. (2) run-red (`no such table: ota_jobs`). (3) add the DDL blocks.
  (4) run-green. (5) commit `feat(db): add OTA job/attempt/observation tables`.
- FKs: documented, NOT enforced (repo doesn't run `PRAGMA foreign_keys=ON` — match the catalog plan's
  Task 1 note; omit `REFERENCES`). Enforcement is CLI SELECT-validate + `validate data`.

**Task A2: offer provenance columns [Claude]**
- Files: `db_migrate.rs` (ALTER `offers` ADD COLUMN ×7, idempotent guard); test `tests/offers_provenance_cols.rs`.
- Why Claude: `offers` is a live, load-bearing table on the shared production Turso DB; adding columns
  must be idempotent and must not disturb the composite PK `(id, scraped_at)` or existing
  `import_offers`/`promote_offers`/`select_offer` reads.
- Add nullable `capture_id TEXT`, `produced_by_job_id TEXT`, `produced_by_attempt_id TEXT`,
  `parser_method TEXT CHECK(parser_method IS NULL OR parser_method IN ('agent_parse','regex'))`,
  `capture_checksum TEXT`, `parser_rule_checksum TEXT`, `normalizer_version TEXT`.
- Steps: failing test asserts the 7 columns exist and existing offer inserts still succeed → run-red →
  add columns with an "already exists" tolerant exec (the repo's `exec_lenient` pattern) → run-green →
  commit `feat(db): add capture_id/job/parser_method provenance to offers`.

### Phase B — the DAL crate (`travel-db`)

**Task B1: `travel-db` crate skeleton + workspace wiring [Claude]**
- Files: new `rust/crates/travel-db/{Cargo.toml,src/lib.rs}`; modify `rust/Cargo.toml` (add member);
  modify `rust/crates/travel-cli/Cargo.toml` (depend on `travel-db`).
- Why Claude: workspace-member + cross-crate dependency change; must build cleanly and not perturb
  `turso-util`/`chromeport`.
- Contents: `travel-db` depends on `libsql` + `turso-util` (for connections); exposes a `repo` module
  tree and typed `Row*` structs. `lib.rs` re-exports `pub mod repo;`. No SQL yet — just the skeleton +
  one trivial typed row to prove the build.
- Steps: failing test = `cargo build -p travel-db` + a unit test constructing a `Row` struct → run-red
  (crate doesn't exist) → scaffold crate + wire workspace → run-green (`make check` passes) →
  commit `feat(travel-db): DAL crate skeleton + workspace wiring`.
- **Boundary contract (Open Q2):** `travel-db` owns *all SQL strings + row mapping*; it does NOT emit
  `plan_events`/`operation_runs`/`plans.version` — that audit/event orchestration stays in
  `travel-cli::cascade::common` and composes repo calls. Write this contract as a doc-comment in
  `travel-db/src/lib.rs` so it's enforced by review.

**Task B2: core repositories (`repo::offers`, `captures`, `parser_rules`, `observations`, `ota_jobs`) [Claude]**
- Files: `rust/crates/travel-db/src/repo/offers.rs`; test `tests/repo_offers.rs`.
- Produces: `OfferRow` struct (typed columns incl. the new provenance fields) +
  `repo::offers::{insert,latest}`, `repo::captures::get`, `repo::parser_rules::get(source, product_type)`,
  `repo::observations::record`, and `repo::ota_jobs` primitives for enqueue/claim/heartbeat/finish/reap.
  **Bound params only** (`libsql::params!`). `repo::offers::latest` lifts the latest-snapshot self-join
  from `promote_offers.rs:308`; parser-rule checksum generation must use a deterministic ordered field
  string, not debug formatting.
- Steps: failing tests assert offers round-trip with provenance, parser_rules lookup requires
  product_type, stale claim-token terminal update affects zero rows, and repo calls use bound params →
  run-red → implement → run-green →
  commit `feat(travel-db): OTA core repositories (typed rows, bound params)`.

### Phase D — job lifecycle + claim semantics

**Task D1: `travel ota enqueue` + `repo::ota_jobs` [→ Grok]**
- Files: `rust/crates/travel-db/src/repo/ota_jobs.rs`; `ota/enqueue.rs`; `main.rs` arm; test `tests/ota_enqueue.rs`.
- Produces: `repo::ota_jobs::enqueue(conn, source, product_type, &[(param_key, param_value)]) -> job_id`
  (INSERT `ota_jobs` status `queued` + `ota_job_params` children; CLI-validate the closed param-key set
  and `product_type ∈ product_types`, fail loud). `travel ota enqueue <source> <product_type>
  [--depart … --return … --nights … --pax … --region-code … --region-label …]`.
- Steps: failing test (enqueue, assert one queued job + the param rows; bad param-key fails loud) →
  run-red → implement → run-green → commit `feat(cli): travel ota enqueue — OTA job queue`.

**Task D2: `travel ota claim` + `heartbeat` + `finish` + `reap-stale` — token-guarded conditional UPDATE claim [Claude]**
- Files: `repo::ota_jobs::{claim,reap_stale}`; `ota/claim.rs`; `main.rs` arms; test `tests/ota_claim.rs`.
- Why Claude: concurrency correctness — the claim must be race-safe on libSQL remote AND portable to D1.
- Behavior: `claim` runs the §1 token-guarded `UPDATE ota_jobs SET status='claimed', claimed_by=?,
  claim_token=?, claimed_at=?, heartbeat_at=?, lease_expires_at=?, updated_at=? WHERE job_id=? AND
  status='queued'` and treats `execute() == 1` as "we own it", `0` as "lost the race". `heartbeat` and
  terminal `finish` update only when `claim_token` matches. `reap-stale` requeues only rows where
  `lease_expires_at < now` and clears `claim_token`; do NOT use the old `claimed_at < now - ttl` rule.
- Steps: failing tests — enqueue 1 job, fire N concurrent `claim` calls through separate write
  connections, assert **exactly one** returns owned and the row is `claimed`; heartbeat prevents reap;
  reap clears the token; stale-token heartbeat/finish affects zero rows; blocked finish requires
  `blocked_reason_code`. → run-red → implement → run-green →
  commit `feat(cli): travel ota claim lifecycle — token-guarded lease`.

### Phase C — OTA execution: capture→parse→offer in Rust (the G1 milestone)

**Task C1: `travel ota parse <job_id> <capture_id> --source <id> --product-type <type> --claim-token <token>` (regex path) [Claude]**
- Files: new `rust/crates/travel-cli/src/ota/mod.rs` + `ota/parse.rs`; `main.rs` dispatch arm
  (mirror the `promote-offers` arm); test `tests/ota_parse.rs`.
- Why Claude: this is the load-bearing port of the gwebcdb Python parse path; correctness is judged
  against a Python oracle, not a self-contained spec.
- Behavior: validate the job exists and `claim_token` still owns it; load `captures.raw_text` (via
  `repo::captures::get`); load the `parser_rules` row for `(source_id, product_type)`; compute
  `capture_checksum`, `parser_rule_checksum`, and `normalizer_version`; apply the regex ruleset; write
  `offers` via `repo::offers::insert` stamped with capture/job/attempt/checksum provenance and
  `parser_method='regex'`; write one `ota_attempts` row and an `ota_observations` row per parse warning;
  finish the job token-guarded unless `--dry-run`. `affected_row_count==0` on the offer INSERT = real
  ON-CONFLICT dedup, NOT failure.
- Oracle: parity with `gwebcdb/bridge/ota_parse.py` on the SAME capture (start with a `settour` capture —
  the only `has_custom_parser=1` source, exercising both branches).
- Steps: failing test (seed a known capture + parser_rules, run `ota parse`, assert the expected offers
  rows with provenance) → run-red → implement → run-green → commit `feat(cli): travel ota parse — Rust capture→offer (regex)`.

**Task C2: `travel ota write-offers <job_id> --capture <capture_id> --claim-token <token> --tsv <path>` (agent-parse path) [Claude]**
- Files: `ota/write_offers.rs`; `main.rs` arm; test `tests/ota_write_offers.rs`.
- Why Claude: normalizes the agent's free-text TSV → typed offers (the Rust analogue of
  `gwebcdb/bridge/ota_write_llm_offers.py`); the TSV→column mapping needs care to stay non-JSON.
- Behavior: validate the job exists and `claim_token` still owns it; read a TSV file (the allowed
  free-text artifact *before* normalization); validate columns; write typed `offers` rows via
  `repo::offers::insert` stamped `parser_method='agent_parse'`, capture/job/attempt/checksum provenance;
  write one `ota_attempts` row with candidate/inserted/deduped counts and finish token-guarded.
- Steps: failing test (fixture TSV → assert typed rows) → run-red → implement → run-green →
  commit `feat(cli): travel ota write-offers — agent TSV → typed offers`.

### Phase E — observations read view

**Task E1: `travel ota observations` view + `repo::observations` [→ Grok]**
- Files: `repo/observations.rs`; `ota/observations.rs`; `main.rs` arm; test `tests/ota_observations_view.rs`.
- Produces: `repo::observations::record(...)` (used by C1/C2/D2) + a read-only `travel ota observations
  [--source <id>] [--type <observation_type>]` plain-text view (JOIN to `coverage_block_reasons` for the
  block expansion). Read via `connect_read`.
- Steps: failing test (seed observations, run view, assert rows + `--type` filter) → run-red → implement
  → run-green → commit `feat(cli): travel ota observations — DB-native execution-signal view`.

### Phase F — gradual DAL adoption (ongoing, no deadline)

**Discipline:** delete `sql_quote()` + value-interpolating `format!` SQL over business tables; replace
with a `travel-db` repo fn using **bound params**; verify CLI output is **byte-identical**
(committed `./bin/travel` vs new build, with a transient-Turso re-read guard). Operational/diagnostic
SQL (`db migrate`/`db exec`/`db schema`/`validate`/`db_seed*`/`db_sync_events`/`db_fetch_holidays`)
is EXEMPT (Codex Part D) — it stays inline in `travel-cli`.

**`sql_quote()` migration ledger** (the business-table offenders the review flagged):

| Module | Repo fn | Status |
|--------|---------|--------|
| `view_bookings.rs` (`bookings` view) | `repo::bookings::book_by_deadlines` | ✅ done (2026-06-30) |
| `offers.rs` (`query-offers`) | `repo::offers::OfferFilter` (parameterized WHERE builder) | ✅ done (2026-06-30) |
| `compare_dates.rs` | `repo::offers::OfferFilter` | ✅ done (2026-06-30) |
| `compare_true_cost.rs` | `repo::offers::OfferFilter` | ✅ done (2026-06-30) |
| `db_query_offers.rs` | `repo::offers::OfferFilter` (+ `departure_window`/`fresh_within_hours`) | ✅ done (2026-06-30) — added `build_where`/`build_sql` unit tests (none existed); `--sql` debug output now shows `?N` placeholders + a `PARAMS:` line (rendered rows unchanged) |
| `freshness.rs` | `repo::freshness` (`offers_freshness` via `OfferFilter` + `plan_provenance_freshness`) | ✅ done (2026-06-30) — both query paths parameterized; golden byte-identical (legacy + plan) |
| `bookings.rs` (`query-bookings`) | `repo::bookings::query_current` (`BookingsCurrentFilter`) | ✅ done (2026-06-30) — **also fixed a latent prod bug**: the SELECT referenced a phantom `payload_text` column that does not exist on `bookings_current` (17 cols), so `query-bookings` errored "no such column" in production; the migrated projection drops it and the command now works |
| `destination_ref.rs` | `repo::destination_ref` (9 slug-keyed reads) | ✅ done (2026-06-30) — all 9 queries `?1`-bound; golden byte-identical across all 6 slugs + unknown/quote/positional cases |
| `plan.rs` | `repo::plan` (16 reads, full DAL extraction) | ✅ done (2026-06-30) — all 16 reads moved to `repo::plan` (SQL + row mapping there, `?1/?2[/?3]`-bound); `plan::load` keeps only the `PlanView` assembly. Golden byte-identical across `status --full`/`itinerary`/`bookings`/`transport` for okinawa/tokyo/kyoto + unknown-plan fail-loud |

**`sql_quote()` is fully retired** — `grep -rn 'sql_quote' rust/crates/travel-cli/src/` returns nothing as of 2026-06-30. Every dynamic business-table query now binds its values. (Remaining inline business SQL that already used bound params or has no interpolated values was never part of this anti-pattern sweep.)

`repo::offers::OfferFilter` is the shared parameterized WHERE builder (`destination`/`region`/
`offer_type`/`source_id`/`source_id_in_csv`/`departure_from`/`departure_to`/`max_price`) covering the
common offer-query predicates; new offer predicates get a `with_*` method there, not a fresh
`sql_quote`. Subsequent modules migrate one per commit, same byte-identical discipline; no big-bang.

**Mutation-command DAL adoption (the read views are done; mutations are the next, optional, layer).**
Mutations were never `sql_quote` offenders (they already bind params), so this is a consistency
refactor, not a bug fix. The pattern: move the **domain-table** writes into a `travel-db` repo, and
LEAVE the audit triad (`plan_events`/`plan_event_data`/`operation_runs`/`plans.version`) in
`travel-cli` (`cascade::common`) per the §5 boundary contract. Started:
- `set-route-segment` / `set-route-segments-bulk` → `repo::route_segments`
  (`day_exists` guard + `delete_slot`/`delete_all_for_day`/`insert_segment`/`touch_day`); the
  `plan_events`/`operation_runs`/version bump stay in the command. Verified by the existing
  `route_segment_guard` (8) + `set_mutation_bugs` integration tests (single + bulk happy-path
  persist, missing-day fail-loud, whole-batch-reject, `--plan-id` honoring). 2026-06-30.
- `set-day-theme` → `repo::days` (`exists` guard + `set_theme` — the theme+zh / theme / theme_zh /
  touch-only 4-way variant); audit stays in the command. `repo::days::exists` is now the single
  `days`-existence query (`route_segments::day_exists` delegates to it). Added a happy-path
  integration test (`set_day_theme_persists_and_preserves_zh`): theme+zh persists, and a theme-only
  follow-up must NOT clobber `theme_zh` — coverage the command lacked before. 2026-06-30.

`repo::days::exists` is the shared "days row exists" guard for itinerary mutations — reuse it, don't
re-roll a `SELECT 1 FROM days`. The other `set_*`/`mark_*`/`swap_days` mutations remain inline
(bound-param, safe); migrate opportunistically with the same domain-writes-to-repo /
audit-stays-in-cascade split.

### Phase G — D1 read-mirror pilot [deferred, gated — do NOT start without sign-off]

Placeholder boundary only (not delegated, not scheduled). When/if pursued: a read-only mirror of 1–2
tables behind the dashboard worker (publish-side), to measure the libSQL↔D1 SQL-dialect delta on real
data. The OTA write path never moves to D1/Workers (OTA execution is local-only). Steps authored at
execution time after a go decision.

### Self-review (plan coverage vs design)

- §1 `ota_jobs`/`ota_job_params`/`ota_attempts` → A1 ✓; §2 `ota_observations` → A1 ✓; §3 offer
  provenance → A2 ✓ (extend `offers`, not fork); §4 capture→parse port → C1 (regex) + C2 (agent) ✓;
  §1 claim semantics → D2 ✓; §5 DAL crate → B1 + B2 + F1 ✓; §6 workspace split → B1 (member add) +
  the `chromeport` drop (Open Q4, folded into B1 or a trailing chore) ✓; §7 Turso-now/D1-later → G
  (gated placeholder) ✓.
- Delegation split: A1, D1, E1 are self-contained new tables/commands/views with clear oracles →
  Grok-able. A2, B1, B2, C1, C2, D2, F1 touch live data / concurrency / cross-crate wiring / behavior
  parity → Claude.
- Depends on the catalog plan's **Task 6** (`parser_rules` re-key to `(source_id, product_type)`) landing
  first — C1 reads the per-`product_type` rule. Sequence the catalog plan's T6 before Phase C.

## Open questions (only the design-changing ones)

1. **Reuse `offers` vs. fork `ota_offers`?** This spec recommends *extend `offers`* (§3). If Yang
   wants a clean break, it must supersede via migration, not run in parallel — confirm before Phase B.
2. **Where does `travel-db` draw the line vs `cascade::common`?** Proposed: DAL owns SQL+row mapping;
   `cascade::common` owns event/version/audit orchestration. Confirm this boundary before Phase B
   (it's load-bearing for every later migration).
3. **Lease TTL + who runs `reap-stale`?** Agent-run CLI (matching agent-first posture) vs. a future
   daemon. Proposed: agent-run, TTL ~15 min. Confirm before Phase D.
4. **Drop `chromeport` from default members now, or after `snapshot_maps` moves to gwebcdb?** Proposed:
   drop from default members now (still buildable on demand). Confirm.

## Codex second opinion — changes adopted (2026-06-29)

Codex reviewed this spec (verdict: ACCEPT-WITH-CHANGES) and I corroborated every finding against
current source (`tmp/codex-second-opinion-rust-first-ota.md` →
`tmp/claude-corroboration-of-codex-second-opinion.md`). Adopted, folded in above:

- **Claim-token + lease (Part A) — the load-bearing fix.** `ota_jobs` gains `claim_token` /
  `lease_expires_at` / `heartbeat_at`; heartbeat/terminal/reap all guard on `claim_token`; reap keys on
  `lease_expires_at`, clears the token. Closes the stale-reclaim ABA hazard. `ota_attempts.claim_token`
  pins which lease produced each terminal result. (§1, rewritten claim SQL.)
- **Reproducibility checksums (Part C).** `offers` gains `produced_by_attempt_id`, `capture_checksum`,
  `parser_rule_checksum`, `normalizer_version`; `ota_attempts` gains
  `candidate_count`/`inserted_count`/`deduped_count`. Needed because captures (`ON CONFLICT(capture_id)
  DO UPDATE`) and `parser_rules` are mutable. (§3, §1.)
- **Typed observation columns (Part B).** `ota_observations` gains `http_status`/`field_name`/
  `selector`/`expected_value`/`observed_value`/`duration_ms`/`freshness_reference_at`; `detail` is a
  human message only. (§2.)
- **Phase reorder (Part F):** A1/A2 schema → B1 DAL → **D1/D2 job+claim+attempt → C1/C2 parse under a
  real job** (C1 no longer writes an attempt row with a NULL job). Update the Implementation-plan order
  accordingly when executing.
- **DAL rule narrowed (Part D):** "commands never write SQL" applies to **business-table** SQL;
  operational/diagnostic SQL (migrate, `db exec`, `db schema`, `validate`) stays in `travel-cli`.
- **D1 = testable contract (Part E):** replace "portable by construction" with a defined portable SQL
  subset + a pilot that runs the real claim/migration/read SQL (existing migrations use
  `PRAGMA table_info` / `sqlite_master` / `ALTER TABLE DROP COLUMN` — `db_migrate.rs:59-91`).
- **`ota_job_params` closed-key CHECK (Part3.5):** add `CHECK(param_key IN (...))` in DB, not just CLI.

Codex's one "blocker" (the chromeport parser_rules contradiction) was already fixed in commit
`49931bc` before its run — verified obsolete; no action.

## Corroboration targets (for the Codex pass)

Same factual checklist as the review (`tmp/claude-review-result-rust-first-ota-db-architecture.md`,
"Claims to corroborate") — every G#/§ claim here maps to one of those `file:line` checks. Verify those
first; this spec's design rests on them. NOTE (2026-06-29): three of those checklist facts are now
STALE because the catalog plan's Tasks 6–9 shipped — `parser_rules` is re-keyed to
`(source_id, product_type)` (no `product_kind`); `add_besttour_offer.rs:159` writes `group_tour`; the
AUTOINCREMENT sites are `schema.sql:156,417,702,860`. The design conclusions are unaffected.
