# Impl + Test Plan: OTA Tier-1 — DB-registered URL-token resolver

**Date:** 2026-07-01 · **Status:** READY TO BUILD (Codex fanned out, Claude corroborated vs source).
**Design-of-record:** `docs/superpowers/specs/2026-07-01-ota-source-registry-and-strategies.md`.
**Build pipeline:** this plan → Codex writes the tests → Grok writes the impl → Claude verifies
line-by-line vs this plan before commit. Tier 2 (`custom:` strategy) is DEFERRED — not built here.

## Goal (the acceptance test)
After this, all 3 verified sources' `ota run --capture-only <src> <pt> --destination tokyo --depart …
--return …` resolve their URL from DB rows with **zero per-source code**, and a brand-new normal OTA
is onboarded by `set-ota-workflow` + `set-ota-url-token` calls only (no rebuild).

## Corroborated prerequisite the spec missed (Codex finding, verified vs source)
The resolver keys token lookups on a `destination` standard input, but **today there is none**:
`VALID_PARAM_KEYS` (`rust/crates/travel-cli/src/ota/common.rs:9-16`) and the `ota_job_params.param_key`
CHECK (`rust/crates/travel-cli/src/db_migrate.rs:739`) both allow only
`depart_date/return_date/nights/pax/region_code/region_label`. Work item 0 adds it. Without it the
whole Tier-1 lookup is dead.

## CHECK-widening hazard (verified): both CHECK changes need a table-rebuild migration
`CREATE TABLE IF NOT EXISTS` is a no-op on an existing table and SQLite can't `ALTER` a CHECK. Both
`ota_job_params.param_key` (item 0) and `ota_source_workflow.nav_kind` (item 6) hit live tables → each
needs an idempotent rebuild migration: create `*_new` with the new CHECK → `INSERT … SELECT *` copy →
`DROP` old → `ALTER … RENAME`. Guard the rebuild so it runs only when the old CHECK is still present
(e.g. probe the table's SQL via `sqlite_master`), to stay idempotent across re-migrates.

---

## Work items (dependency-ordered)

### 0. Add `destination` as a standard OTA input (PREREQUISITE)
- Files: `ota/common.rs:9` (`VALID_PARAM_KEYS` += `"destination"`); `ota/run.rs` (parse `--destination`
  into `params["destination"]`, add to usage text + `PARAM_FLAGS`); `db_migrate.rs:737-742`
  (table-rebuild migration widening `ota_job_params.param_key` CHECK to include `'destination'`).
- Done-check: `travel ota run --capture-only … --destination tokyo` enqueues `ota_job_params(job,
  destination, tokyo)`; an invalid key still fails loud.

### 1. NEW table `ota_source_url_token`
- Files: `db_migrate.rs` (near the `ota_source_workflow` CREATE at :789); new
  `scripts/seed/ota_source_url_token.seed.sql`.
- Change: `CREATE TABLE IF NOT EXISTS ota_source_url_token (source_id TEXT NOT NULL, product_type TEXT
  NOT NULL, placeholder TEXT NOT NULL, input_key TEXT NOT NULL, input_value TEXT NOT NULL, token_value
  TEXT NOT NULL, updated_at TEXT NOT NULL DEFAULT (datetime('now')), PRIMARY KEY (source_id,
  product_type, placeholder, input_key, input_value))`. Do NOT touch `ota_source_region_codes`.
- Done-check: `db schema ota_source_url_token` shows all 7 columns + the 5-col PK.

### 2. Repo: `url_token` lookup
- Files: `rust/crates/travel-db/src/repo/ota_source_workflow.rs` (keep `get`, keep `region_id`).
- Change: add `pub async fn url_token(conn, source_id, product_type, placeholder, input_key,
  input_value) -> Result<Option<String>, String>` → `SELECT token_value FROM ota_source_url_token
  WHERE source_id=?1 AND product_type=?2 AND placeholder=?3 AND input_key=?4 AND input_value=?5`
  (bound params, mirrors `region_id`).
- Done-check: seeded `besttour/group_tour/region_id/destination/tokyo → 295`; missing row → `None`.

### 3. Resolver: replace the region-only branch with a generic token loop
- Files: `ota/run.rs:174-196` (KEEP `resolve_url`/`find_placeholders`/`insert_param`).
- Change: delete the `{region_id}` special branch (`run.rs:186-194`). After the `depart`/`return`
  aliases, for EVERY placeholder in `find_placeholders(&workflow.url_template)` not already in `map`:
  require `map["destination"]` (fail loud "missing --destination for token resolution" if absent),
  call `ota_source_workflow::url_token(&conn, source_id, product_type, &placeholder, "destination",
  &destination)`, fail loud naming the placeholder if `None`, else `insert_param`. **No `if source ==`
  anywhere.** Then `resolve_url` as today.
- Done-check: settour fills `{dest_code}`+`{region_id}`, eztravel `{dest_code}`, besttour `{region_id}`
  — all from rows, no source conditionals.

### 4. `nav_kind` dispatch in `run`
- Files: `ota/run.rs` (right after the workflow row is loaded, before resolve/navigate).
- Change: `match workflow.nav_kind.as_str() { "get" => { /* current flow */ }, other =>
  return Err(format!("nav_kind '{other}' has no registered strategy")) }`. No trait/dyn/async_trait.
- Done-check: a stored `custom:test` workflow loads but `run --capture-only` fails loud BEFORE navigate.

### 5. Widen `ota_source_workflow.nav_kind` CHECK (table-rebuild)
- Files: `db_migrate.rs:789-800`.
- Change: rebuild migration to `CHECK(nav_kind = 'get' OR nav_kind LIKE 'custom:%')` (idempotent,
  guarded on the old CHECK still being present). Keep the `DEFAULT 'get'` + PK.
- Done-check: inserting `nav_kind='custom:test'` succeeds; `nav_kind='form'` still rejected.

### 6. `set-ota-workflow` command
- Files: `rust/crates/travel-cli/src/set_ota_catalog.rs` (mirror `run_set_region`); `main.rs:169-176`
  (new dispatch arm).
- Change: `travel set-ota-workflow <source> <product_type> --nav <kind> --url-template <t>
  [--capture-url-contains s] [--settle-ms N] [--settle-marker m] [--note ...]`. Validate product_type
  exists, template nonempty, `nav == "get" || nav.starts_with("custom:")`, `settle_ms` parses as int.
  UPSERT `ota_source_workflow` (ON CONFLICT(source_id, product_type) DO UPDATE … COALESCE). Then
  `record_catalog_run(&conn, "set-ota-workflow", &format!("{source}/{product_type}"))`. Plain-text line.
- Done-check: writes the row + a `catalog_runs` audit row.

### 7. `set-ota-url-token` command
- Files: `set_ota_catalog.rs`; `main.rs` (new dispatch arm).
- Change: `travel set-ota-url-token <source> <product_type> <placeholder> <input_key> <input_value>
  <token_value>` (6 positionals). Validate product_type exists and (v1) `input_key == "destination"`
  fail-loud otherwise. UPSERT `ota_source_url_token`. `record_catalog_run(&conn, "set-ota-url-token",
  …)`. Plain-text line.
- Done-check: round-trips `zztest/fit/dest_code/destination/tokyo/TYO`; non-`destination` input_key
  fails and writes nothing.

### 8. Seed the verified token rows
- Files: new `scripts/seed/ota_source_url_token.seed.sql`; `db_migrate.rs:1384` (add
  `seed_ota_url_token()` mirroring `seed_ota_workflow`, call it after the table is created).
- Change: `INSERT OR IGNORE` rows, one statement per line, NO `;`/`'` inside comments (the
  `run_seed_file_stmts` splitter splits on `;` before stripping comments). All
  `input_key='destination', input_value='tokyo'`:
  - `besttour/group_tour/region_id → 295`
  - `settour/fit/region_id → 179900`
  - `settour/fit/dest_code → NRT`
  - `eztravel/fit/dest_code → TYO`
- Done-check: migrate inserts the 4 rows insert-if-absent without clobbering live edits.

---

## Test plan (Codex writes these)

### Unit (inline in `ota/run.rs` `#[cfg(test)] mod tests`)
- Standard-input aliases (`depart`/`return`) still fill.
- Token fill: a map with `destination=tokyo` + a stub token resolves a single placeholder. (Pure
  `resolve_url` already covers interpolation; the new loop's DB call is exercised in integration —
  keep unit tests on the PURE pieces: `find_placeholders`, `resolve_url`, `insert_param` collision.)
- Multi-placeholder fill + unresolved placeholder error NAMES the placeholder.
- (Keep the existing 3 `resolve_url` tests; extend, don't replace.)

### Integration (real-Turso; arm `common::Guard` immediately after ids are known — NO trailing teardown)
- `tests/ota_source_workflow_schema.rs`: assert `ota_source_url_token` exists with the 7 cols + PK;
  update the GET-only `nav_kind` CHECK test to assert `custom:test` INSERTs and `form` is REJECTED;
  assert the 4 seeded token rows present.
- `tests/repo_workflow.rs`: keep the `region_id` test; add `url_token` seeded-lookup hit + miss(`None`).
- `tests/set_ota_catalog.rs`: `set-ota-workflow` round-trip (row + audit row); `set-ota-url-token`
  round-trip (row + audit row); non-`destination` `input_key` fails loud AND writes nothing.
- `tests/ota_run_capture_only.rs`: extend the capture-only path with `--destination tokyo` for all 3
  verified sources (`besttour/group_tour`, `settour/fit`, `eztravel/fit`) — assert the command
  resolves the token-filled URL (assert on the resolved URL / no "missing placeholder" error). Skip
  cleanly when Chrome/Turso absent. Clean ONLY rows this test created (exact job_id/zztest source),
  never broad source deletes.

## Risks / notes (carry into the build)
- The two CHECK rebuilds are the only non-trivial migrations; everything else is additive `CREATE … IF
  NOT EXISTS` + seed.
- `./bin/travel` is a RELEASE binary — `make build` before any CLI repro (don't trust a stale binary).
- No auto-migration from `ota_source_region_codes`; the tokyo token seed is intentionally partial and
  authoritative.
