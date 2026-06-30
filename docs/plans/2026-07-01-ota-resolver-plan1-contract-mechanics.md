# Plan 1: OTA resolver — product_type contract + mechanics

**Date:** 2026-07-01 · **Status:** READY TO BUILD (Codex fanned out, Claude corroborated vs source).
**Design:** `docs/superpowers/specs/2026-07-01-ota-resolver-extension-design.md` (type covers IN→PROCESS→OUT).
**Scope:** mechanics ONLY. Does NOT onboard google_flights/agoda — that's Plan 2.
**Pipeline:** this plan → Codex writes tests → Grok writes impl → Claude verifies line-by-line.

## Goal
`product_type_inputs` becomes the canonical input contract; the resolver is contract-driven (COMMON from
caller→DB-default→code-default; DISTINCT token roles from `ota_source_url_token` by the contract's
token_key roles). **No behavior change for the seeded sources** (settour/eztravel/besttour still resolve).

## Two corroborated corrections to fold in (verified vs source)
1. **CHECK-guard trap (Codex, CONFIRMED):** `migrate_ota_job_params_destination` (db_migrate.rs:113)
   returns early when the DDL `contains("destination")`. The LIVE `ota_job_params` DDL already contains
   `destination` (Tier-1) but NOT `origin`/`currency`/`rooms`/`hotel`. So a guard checking
   `contains("destination")` would **silently skip the new widening** → `--origin` etc. fail the CHECK at
   insert. The new migration's idempotency guard MUST probe for the FINAL key set (e.g.
   `ddl.contains("origin") && ddl.contains("currency") && ddl.contains("rooms") && ddl.contains("hotel")`),
   not `destination`. Update the fresh `CREATE TABLE IF NOT EXISTS` DDL (db_migrate.rs:823) to the same
   final CHECK so fresh + existing DBs converge.
2. **travel4u seed/live gap (Codex, CONFIRMED):** travel4u is in 0 seed files but 1 live workflow row (I
   registered it via CLI in the registry step). Seeding it is Plan-2 work. So **Plan 1 verifies against
   the 3 SEEDED sources** (settour/eztravel/besttour), reproducible from a fresh migrate; travel4u stays a
   live-only bonus until Plan 2 seeds it. (The capture-only acceptance test stays at 3 sources for Plan 1.)

## Work items (dependency-ordered)

### 1. `product_type_inputs` table + seed (the 4 contracts)
- Files: `db_migrate.rs` (CREATE near `ota_source_workflow` ~789-887; `seed_*` helpers ~1487;
  `run_seed_file_stmts` ~1501); new `scripts/seed/product_type_inputs.seed.sql`.
- Change: `CREATE TABLE IF NOT EXISTS product_type_inputs (product_type TEXT NOT NULL, input_name TEXT
  NOT NULL, input_class TEXT NOT NULL CHECK(input_class IN ('common','token_key')), required INTEGER NOT
  NULL DEFAULT 1 CHECK(required IN (0,1)), default_source TEXT CHECK(default_source IS NULL OR
  default_source IN ('caller','db','code')), sort_order INTEGER NOT NULL DEFAULT 0, PRIMARY KEY
  (product_type, input_name))`. Add `seed_product_type_inputs()` (mirror `seed_ota_url_token`,
  `include_str!`), called after create. Seed the 4 contracts (flight=5, hotel=7, fit=4, group_tour=1
  rows). Seed file: one stmt/line, NO `;`/apostrophe in comments OR values.
- Done-check: fresh+existing migrate yields `flight=5, hotel=7, fit=4, group_tour=1`.

### 2. Widen OTA inputs / flags
- Files: `ota/run.rs` (`PARAM_FLAGS`:10, `RUN_USAGE`:20, arg-parse:98); `ota/common.rs:9` `VALID_PARAM_KEYS`.
- Change: add `--origin`/`--currency`/`--rooms`/`--hotel` to flags+usage; parse into params as
  `origin`/`currency`/`rooms`/`hotel`; add those 4 to `VALID_PARAM_KEYS`. Keep `--depart`→`depart_date`,
  `--return`→`return_date`.
- Done-check: `ota run … --origin TPE --currency TWD --rooms 1 --hotel foo` passes validation + enqueues.

### 3. CHECK rebuild for `ota_job_params` (WITH the corrected guard)
- Files: `db_migrate.rs` (`table_ddl`:99, `migrate_ota_job_params_destination`:109, DDL:823, call:831).
- Change: widen the CHECK to the final key set (`depart_date,return_date,nights,pax,region_code,
  region_label,destination,origin,currency,rooms,hotel`). **Guard on the FINAL keys, not `destination`**
  (correction #1) — idempotent table-rebuild (create `_new`, copy, drop, rename). Update the fresh
  IF-NOT-EXISTS DDL to the same CHECK.
- Done-check: a live DB with only the destination-widened CHECK rebuilds once to include the 4 new keys;
  fresh DB creates the final CHECK directly; a bogus key still fails.

### 4. DB-default reader (origin airport + currency)
- Files: new `rust/crates/travel-db/src/repo/origin.rs`; `repo/mod.rs` export.
- Change: `default_origin_airport_and_currency(conn) -> Result<DefaultOrigin{slug,airport,currency},String>`
  — `global_config.default_origin` → `origin_config.currency` + `origin_airports.airport ORDER BY
  sort_order LIMIT 1`. Fail loud if any row missing. Bound params.
- Done-check: seeded DB → `taiwan / TPE / TWD`.

### 5. `product_type_inputs` repo reader
- Files: new `rust/crates/travel-db/src/repo/product_type_inputs.rs`; `repo/mod.rs` export.
- Change: `ProductTypeInputRow` + `list_for_type(conn, product_type) -> Result<Vec<ProductTypeInputRow>,
  String>` ordered by `sort_order, input_name`. Bound params.
- Done-check: returns the seeded contract rows in deterministic order.

### 6. Resolver loop rewrite (contract-driven + ambiguity seam)
- Files: `ota/run.rs` (KEEP `resolve_url`:24/`find_placeholders`:45/`insert_param`:70; replace the
  destination-only loop ~188-210); may add a repo helper for multi-role token candidacy.
- Change: load `product_type_inputs` for the source's product_type. Build COMMON values:
  caller-flag → DB-default (origin/currency) → code-default (rooms=1) → fail-loud if required+missing.
  Apply CODE aliases (`depart`→depart/depart_date/checkin; `return`→return/return_date; `pax`→pax/adults).
  For each URL placeholder not filled by COMMON: determine the legal `token_key` roles from the contract;
  for each legal role with a caller value, look up `ota_source_url_token(source, product_type,
  placeholder, input_key=role, input_value=value)`. **Exactly one hit → insert. Zero → fail loud naming
  source/product_type/placeholder/role/value. >1 hit → fail loud "ambiguous — agent must disambiguate"
  (the LLM-judge SEAM: a fail-loud branch + TODO, NOT an actual agent call).** Validate a token's
  `input_key` is a declared `token_key` for the type (fail loud otherwise). Then `resolve_url`.
- Done-check: settour/eztravel/besttour still resolve their seeded destination tokens; no unresolved
  `{...}` reaches `resolve_url`.

### 7. `set-ota-url-token` hotel support
- Files: `set_ota_catalog.rs` (`run_set_url_token`:278, reject:301/303; UPSERT:315; audit:336); main.rs
  dispatch already wired.
- Change: validation `input_key != "destination"` → `input_key ∉ {destination, hotel}` fails (still
  BEFORE any write, so a bad key writes neither token nor audit row). Keep product_type validation,
  UPSERT, audit.
- Done-check: `set-ota-url-token <sid> hotel hotel_slug hotel my-hotel tok` writes the row + 1 audit row;
  `… origin …` still fails and writes nothing.

## Test plan (Codex writes; arm `common::Guard`; credless/Chrome/gwebcdb-TURSO skips)
- **Unit** (`ota/run.rs` `#[cfg(test)]`): COMMON resolution + aliases (`depart_date`→depart/depart_date/
  checkin; `return_date`→return/return_date; `pax`→pax/adults); caller overrides DB default; DB default
  fills origin/currency; code default rooms=1; required-missing fails loud; **ambiguous candidate → error
  containing "ambiguous" + placeholder/roles**. Keep existing resolve_url/insert_param tests.
- **Integration:**
  - schema/seed: `product_type_inputs` exists + the 4 contracts seeded (counts 5/7/4/1).
  - repo: `product_type_inputs::list_for_type` + `origin::default_origin_airport_and_currency` (→ TPE/TWD).
  - `set_ota_catalog`: url-token round-trip for `input_key='hotel'`; `origin` still rejected (writes
    nothing); `hotel` succeeds.
  - `ota_job_params`: positive inserts for origin/currency/rooms/hotel; bogus key still fails.
  - resolver: the 3 SEEDED sources (settour/eztravel/besttour) still resolve via the new contract path
    (correction #2 — NOT 4; travel4u is live-only until Plan 2). + one token_key-validation case (a token
    row whose input_key isn't a declared token_key for the type → resolver fails loud before nav).

## Gaps / risks
- The CHECK-guard correction (#1) is the one easy-to-miss bug — the guard MUST key on the final set.
- Seed-splitter rule on the new seed file (no `;`/apostrophe in comments or values).
- `./bin/travel` is RELEASE — `make build` before any CLI repro.
