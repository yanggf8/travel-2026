# Phase B impl + test plan — GET-only `ota_source_workflow` + `travel ota run --capture-only`

Spec: `docs/superpowers/specs/2026-06-30-ota-workflow-nodes.md` (Codex-reviewed ACCEPT-WITH-CHANGES;
besttour validation showed all 3 verified sources are direct GET → **GET-only, NO form sub-schema**).

> **Codex reviewed THIS plan (ACCEPT-WITH-CHANGES) — corroborated by Claude vs source. Two blocking
> fixes folded in below:**
> 1. **`run` must claim the SPECIFIC job it enqueued.** The existing `repo::ota_jobs::claim` claims
>    the GLOBAL oldest queued job (`ota_jobs.rs:116` `WHERE status='queued' ORDER BY created_at ASC
>    LIMIT 1`) — so enqueue-then-`claim` would capture under the WRONG job when the queue is
>    non-empty. → add `repo::ota_jobs::claim_specific(conn, job_id, worker, now, lease)` (token-guarded
>    conditional UPDATE on that exact job_id; `==1` ⇒ owned).
> 2. **`{region_id}` is provider CONFIG, resolved via the existing `ota_source_region_codes` lookup —
>    NEVER identity-mapped from `region_code` and NEVER stored as a job param.** `ota_source_region_codes
>    (source_id, product_type, region_label, region_code)` is seeded (`ota_coverage.seed.sql:28`:
>    `besttour/group_tour/東京→295`). The job carries `region_label` (東京); `{region_id}` ←
>    `ota_source_region_codes.region_code` looked up by `(source_id, product_type, region_label)`.
>    (`ota_job_params` CHECK only allows depart_date/return_date/nights/pax/region_code/region_label —
>    adding `region_id` there is wrong; don't.)

## Scope (deliberately minimal)

Build the config-driven front half of the OTA workflow for **GET-resolvable sources only**:
- a new `ota_source_workflow` config table (one row per `(source_id, product_type)`),
- a `travel ota run --capture-only <source_id> <product_type> [search params]` command that threads
  nodes 0–4 (claim → resolve-url → navigate → settle → capture) and STOPS, printing the `capture_id`
  + `agent_extraction_note` for the agent to take over (the agent then runs the existing
  `ota write-offers`).

**Out of scope (do NOT build):** `nav_kind='form'`, `ota_source_form_step`, any form-step action enum
(zero proven need — all verified sources are GET). A `nav_kind` column may be added as a fixed `'get'`
default for forward-compat, but no form path is implemented.

## Conventions (must match the existing repo)

- DAL boundary: SQL + row mapping in `travel-db` (`repo::ota_source_workflow`); the command in
  `travel-cli/src/ota/run.rs` parses args, orchestrates, shells gwebcdb. (Same split as the other
  `ota/*` commands; `enqueue.rs` is the closest sibling.)
- Bound params only (`libsql::params!`); no `sql_quote`/interpolation.
- Migration: add the table in `db_migrate.rs` as `CREATE TABLE IF NOT EXISTS` beside the other `ota_*`
  tables (TEXT/composite PK, no AUTOINCREMENT, no JSON). Seed settour+eztravel+besttour rows from a
  checked-in seed (insert-if-absent), mirroring `scripts/seed/ota_*.seed.sql` style.
- Shelling a subprocess from the CLI is an established pattern (`weather.rs`, `snapshot_maps.rs`).
- Dispatch: add `"run"` arm to `ota/mod.rs`.
- Real-Turso integration tests with **panic-safe `Guard` teardown** (`mod common; use common::Guard;`
  — never a trailing `teardown()`); skip cleanly if creds absent.

## IMPL TASKS

### I1 — `ota_source_workflow` table (migration + seed) [Grok]
Files: `rust/crates/travel-cli/src/db_migrate.rs`; new `scripts/seed/ota_source_workflow.seed.sql`.
Schema (GET-only):
```
CREATE TABLE IF NOT EXISTS ota_source_workflow (
  source_id             TEXT NOT NULL,
  product_type          TEXT NOT NULL,
  nav_kind              TEXT NOT NULL DEFAULT 'get' CHECK(nav_kind IN ('get')),  -- form deferred
  url_template          TEXT NOT NULL,   -- {placeholders} interpolate by-name from ota_job_params
  capture_url_contains  TEXT,            -- substring for ota_capture --url-contains (NULL = omit flag)
  settle_marker         TEXT,            -- text whose ABSENCE means "still loading" (preferred)
  settle_ms             INTEGER NOT NULL DEFAULT 0,  -- fallback cap
  agent_extraction_note TEXT,            -- human/agent-read ONLY; deterministic code MUST NOT consume it
  updated_at            TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (source_id, product_type)
)
```
Seed rows (insert-if-absent, from the live-verified recipes):
- settour/fit: url_template `https://fit.settour.com.tw/product/v2?tripType=RT&directFlightOnly=true&roomQty=1&depAirportCode=TPE&arrAirportCode={dest_code}&depDate={depart},{return}&hotelCheckInDate={depart}&hotelCheckOutDate={return}&adtCount={pax}&chdCount=0&regionId={region_id}`, capture_url_contains `product/v2`, settle_marker `正在努力查詢`, settle_ms 25000, note = the settour quirk.
- eztravel/fit: url_template `https://packages.eztravel.com.tw/roundtrip-TPE-{dest_code}?checkin={depart}&checkout={return}&adult={pax}&child=0`, capture_url_contains `roundtrip-TPE`, settle_ms 25000, note = "SPA ignores GET dates; record page dates".
- besttour/group_tour: url_template `https://www.besttour.com.tw/e_web/search?v=//////{region_id}///////`, capture_url_contains `e_web/search`, settle_ms 25000, note = "group-tour listing; skip 滿/no-price tours".
(Seed-file rule: one statement per line, no `;`/`'` inside comments — the splitter splits on `;` before stripping comments.)

### I2 — repos: `ota_source_workflow`, `ota_jobs::get_params`, `ota_jobs::claim_specific`, region lookup [Grok]
Files: new `rust/crates/travel-db/src/repo/ota_source_workflow.rs`; add `pub mod` in `repo/mod.rs`;
add fns to `repo/ota_jobs.rs`; region lookup may live in `repo/ota_source_workflow.rs` or a small
`repo/ota_regions.rs`.
- `WorkflowRow { source_id, product_type, nav_kind, url_template, capture_url_contains: Option<String>,
  settle_marker: Option<String>, settle_ms: i64, agent_extraction_note: Option<String> }`.
- `repo::ota_source_workflow::get(conn, source_id, product_type) -> Result<Option<WorkflowRow>, String>`.
- `repo::ota_jobs::get_params(conn, job_id) -> Result<Vec<(String,String)>, String>` (NEW — no reader
  exists today; `enqueue` only writes `ota_job_params`).
- **`repo::ota_jobs::claim_specific(conn, job_id, worker, now, lease) -> Result<Option<ClaimResult>, String>`**
  (NEW — token-guarded conditional UPDATE on the EXACT job_id: `UPDATE ota_jobs SET status='claimed',
  claimed_by=?, claim_token=<new_run_id>, claimed_at=?, heartbeat_at=?, lease_expires_at=?, updated_at=?
  WHERE job_id=? AND status='queued'`; `affected==1` ⇒ owned + return token, else `None`). This is the
  blocking fix — the existing `claim` is global-oldest.
- **`repo::ota_source_workflow::region_id(conn, source_id, product_type, region_label) -> Result<Option<String>, String>`**
  (lookup `SELECT region_code FROM ota_source_region_codes WHERE source_id=?1 AND product_type=?2 AND
  region_label=?3` — note the column is named `region_code` but its VALUE is the source-specific id
  like `295`). Used only when a `{region_id}` placeholder is present.

### I3 — `travel ota run --capture-only` command [Grok]
Files: new `rust/crates/travel-cli/src/ota/run.rs`; add `"run"` arm to `ota/mod.rs`.
Behavior (`run --capture-only <source_id> <product_type> [--depart .. --return .. --nights .. --pax .. --region-code .. --region-label ..]`):
1. **claim** — enqueue a job (`repo::ota_jobs::enqueue`, same param mapping as `enqueue.rs`) → `job_id`;
   then **`repo::ota_jobs::claim_specific(job_id, ...)`** (NOT the global `claim`). Get `claim_token`.
   Fail loud if claim_specific returns `None` (lost the race / not queued).
2. **resolve-url** — load the `ota_source_workflow` row (fail loud if absent → "no workflow row for
   (source,product_type); add one"). **Build the param map** (a pure step then a DB step):
   - load the job's params (`get_params`) → base map.
   - add aliases: `depart`←`depart_date`, `return`←`return_date`.
   - **if the template contains `{region_id}`**, resolve it via
     `repo::ota_source_workflow::region_id(source, product_type, region_label)` (region_label from the
     job params) and add `region_id`→`<value>` to the map. Fail loud if the lookup misses.
   - if an explicit job param and a derived param collide on the same key, **fail loud**.
   Then call the **pure** `resolve_url(template, &map)` (see Build Spec). **Fail loud naming every
   unfilled `{placeholder}`.** (No `{dest_code}` identity rule — only resolve placeholders the
   template actually has, from params or the region lookup.)
3. **navigate** — shell `python bridge/navigate.py "<url>"` from `~/b/gwebcdb` with TURSO_URL/TURSO_TOKEN
   exported (read from the travel `.env` or inherit). (gwebcdb path: a const or env `GWEBCDB_DIR`
   default `~/b/gwebcdb`.) Fail loud if the subprocess errors.
4. **settle** — **v1: sleep `settle_ms`** (a fixed sleep). Do NOT half-implement marker polling —
   `settle_marker` is stored for a future marker-poll but v1 ignores it (documented). (Codex: "do not
   half-claim marker support.")
5. **capture** — shell `python bridge/ota_capture.py --source <source_id> [--url-contains <capture_url_contains>]`,
   parse the printed `capture_id` line. Fail loud if no capture_id.
6. **STOP** — print: `job_id\t<>`, `claim_token\t<>`, `capture_id\t<>`, `source_id`, `product_type`,
   and `agent_extraction_note\t<note>` (so the agent sees the quirk). Do NOT write offers — the agent
   does that next via `ota write-offers`.

Token-guard note: the job stays `claimed` (not finished) — the agent's later `write-offers` uses the
printed `claim_token`. If `--capture-only` fails after claim, leave the job claimed (reap-stale will
requeue it); optionally record an `ota_observations` block row on a navigate/capture failure (reuse
`repo::observations::record`).

## TEST TASKS (Codex writes these)

### T1 — migration/schema test (real-Turso) — `tests/ota_source_workflow_schema.rs`
- After `db migrate`, `ota_source_workflow` exists with the documented columns (assert via `db schema`).
- The 3 seed rows (settour/fit, eztravel/fit, besttour/group_tour) are present after migrate
  (insert-if-absent), with the expected `url_template`/`capture_url_contains`/`settle_ms`.
- CHECK rejects a bad `nav_kind` (e.g. 'form' currently → constraint error).
- Panic-safe Guard; skip if credless.

### T2 — `repo::ota_source_workflow::get` + `ota_jobs::get_params` (real-Turso) — `tests/repo_workflow.rs`
- Seed a zztest workflow row + a zztest job with params; `get` returns the typed row; missing →
  `None`. `get_params` returns the (key,value) pairs for the job; empty for a job with no params.
- Guard teardown of the zztest rows.

### T3 — `resolve_url` interpolation (PURE unit test, no Turso) — in `ota/run.rs` `#[cfg(test)]`
`resolve_url(template, &map) -> Result<String,String>` is a **pure fn** (see Build Spec). Tests:
  - all placeholders filled (incl. a pre-resolved `region_id=295`) → correct settour/eztravel/besttour URL.
  - an unfilled `{placeholder}` → `Err` naming ALL missing keys.
  - no placeholders → template unchanged.
  - **NO `{region_id}`←`region_code` identity test** (region_id comes pre-resolved in the map; the
    lookup itself is a DB concern, tested separately if at all).

### T3b — `claim_specific` does not steal another job (real-Turso) — `tests/repo_workflow.rs`
- Seed an OLDER queued zztest job A, then enqueue job B; `claim_specific(B)` must claim **B** (not A);
  the row for B is `claimed` with a token, A stays `queued`. (Locks the blocking-bug fix.)
- Also: `claim_specific` on an already-claimed/nonexistent job → `None`.

### T4 — `run --capture-only` end-to-end (real-Turso + browser, GATED) — `tests/ota_run_capture_only.rs`
- GATED/skippable: needs WSLg Chrome. If Chrome not on :9222 OR credless → **skip cleanly** (don't fail).
- Prefer a seeded zztest GET workflow row pointing at a **stable simple page** (not besttour by default,
  per Codex — the live site can be down/change). Run `ota run --capture-only`, assert: prints a
  `capture_id`; a `captures` row exists; the job is `claimed` with a `claim_token`; **NO offers written**.
- **Teardown `captures` rows too** (not just jobs). Guard, not trailing.

## VERIFICATION (I, Claude, do this before commit)
- `make check` (build) + `make validate` green; `cargo test -p travel-db` + the new integration tests
  green against live Turso (creds exported).
- Read every Grok/Codex line vs. this plan + the existing `ota/*` conventions; pressure-test:
  fail-loud on missing workflow row + unfilled placeholder; no `sql_quote`; bound params; Guard
  teardown present (not trailing); `agent_extraction_note` is never machine-consumed (only printed);
  the command does NOT write offers.
- Golden (manual, post-build): `ota run --capture-only besttour group_tour --region-label 東京`
  resolves `{region_id}`→`295`, produces a capture + claimed job + the note, **no offers** — matching
  the manual Phase-A flow. (`--region-label`, NOT `--region-code`, since the lookup keys on the label.)

## RESOLVED BUILD SPEC (Codex, corroborated — build exactly this)

**`resolve_url(template: &str, params: &BTreeMap<String,String>) -> Result<String, String>`** — PURE:
- no DB / env / time / subprocess / source-specific branching.
- placeholder syntax `{name}`, `name` = ASCII alnum/underscore.
- replace every `{name}` occurrence from `params` (already fully built by the caller).
- if ANY placeholder is unfilled → `Err` naming ALL missing keys.
- values inserted verbatim (caller ensures URL-ready); never consumes `agent_extraction_note`.

**Caller (`run`) builds the param map BEFORE resolve_url:**
- `get_params(job_id)` → base map; add aliases `depart`←`depart_date`, `return`←`return_date`.
- if template has `{region_id}`: `region_id(source, product_type, region_label)` lookup → add
  `region_id`. (region_label from job params; provider region ids are CONFIG, from
  `ota_source_region_codes`, NEVER identity from `region_code`, NEVER a job param.)
- explicit-vs-derived collision on the same key → fail loud.

**`run --capture-only`:** enqueue → `claim_specific(job_id)` (NOT global `claim`) → resolve_url →
shell `navigate.py` → sleep `settle_ms` (v1; no marker-poll) → shell `ota_capture.py --source …
[--url-contains …]` (parses the printed `capture_id\t…` line; gwebcdb `ota_capture.py` prints it) →
print job_id/claim_token/capture_id/source_id/product_type/agent_extraction_note → **do NOT write
offers.**
