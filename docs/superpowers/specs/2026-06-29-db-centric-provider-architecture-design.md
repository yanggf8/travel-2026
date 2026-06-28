# Design: DB-Centric Architecture — Provider Catalog & Coverage as Normalized DB Data

**Date:** 2026-06-29
**Status:** Codex-reviewed 2026-06-29; all findings corroborated against source and folded
in (see "Review corrections"). Ready for implementation fan-out.
**Author:** Claude (with Yang)

## The principle (Yang's, restated)

The whole reason the DB exists is **traceability**: every record we produce must be a
queryable DB row so we can trace what happened, the current state, and what's next. The
ONLY things allowed to live outside normalized DB columns are:

1. **The LLM's raw input/output text** — the capture `raw_text` the agent reads, and the
   agent's parsed text result *before* it is normalized into typed offer rows. These stay
   in plain `TEXT` columns (this is the documented `de-json-unknown-to-text-column` rule:
   the LLM's free-form artifact is the one legitimate text payload).

Everything else — provider definitions, which product types a provider serves, whether a
type is proven, the canonical search URL, the blocked/deferred reason, the work agenda —
**must be normalized into typed columns or child rows.** Hard rules:

- **No JSON.** No `*_json` columns, no JSON-encoded values in any cell.
- **No blobs.** No opaque payloads.
- **No prose-as-data.** A free-text `notes` column holding structured facts
  ("PROVEN REAL (2026-06-26, agent-parse, group-tour, url=...)") is the same anti-pattern
  as a JSON blob: discrete facts hiding in an unstructured field. Each fact → its own
  typed column or child row.
- **No facts in code.** A provider/destination catalog defined in a Rust `const` array
  (`OTA_SOURCES`, `DESTINATIONS` in `db_migrate.rs`) makes *code* the source of truth and
  the DB a downstream copy. The DB must be authoritative.
- **No facts in documents.** Provider status / coverage / agenda must not live in
  CLAUDE.md tables/prose or memory files. Those become *pointers to a CLI view*, not the
  record.

## The current violations (inventoried against live source/DB, 2026-06-29)

| # | Violation | Where it lives now | Evidence |
|---|---|---|---|
| V1 | Provider catalog born in **code** | `OTA_SOURCES` array, `db_migrate.rs:1171` (14 entries); `DESTINATIONS` array, `db_migrate.rs:1115` | `db migrate` re-asserts the array onto the DB via INSERT…ON CONFLICT — code is authoritative, DB is downstream |
| V2 | Provider facts as **prose in a column** | `ota_sources.notes` (free TEXT, 18–445 chars/row) | "PROVEN REAL (2026-06-26, gwebcdb agent-parse path)…" — proven flag, date, method, product_kind, search_url, blocked_reason all crammed into one sentence |
| V3 | Coverage / proven status in **documents** | CLAUDE.md OTA table + Next-Steps prose (11 status strings); memory files | `grep -c "PROVEN REAL\|DEFERRED\|renderer-wedge" CLAUDE.md` = 11; not queryable |
| V4 | **Drift** between DB and prose (the proof the split fails) | `parser_rules.product_kind` says besttour=`fit`, but CLAUDE.md/notes call besttour "group tours (跟團)" | Two sources of truth already disagree |
| V5 | **No mutation path** for providers | no `add/edit-ota-source` CLI exists | The only way to define/change a provider is edit the Rust array + migrate |

Net effect: the question "do we have a working flight / hotel / FIT / package agent for the
important sources?" **cannot be answered by a query** — only by reading prose that drifts.

## Design

### 1. Normalize the provider catalog (kill V2, the prose column)

`ota_sources` keeps only true scalar provider identity; per-type coverage and recipe facts
move to child rows. **Drop `notes` entirely** (free-text prose is not data).

**`ota_sources`** (parent — provider identity only):
- `source_id TEXT PK`, `name TEXT NOT NULL`
- `status TEXT NOT NULL` — `active | inactive` (provider is reachable at all)
- `updated_at DATETIME`
- (remove `notes`; move `scraper_script`/`url_template` per the rules below)

**`product_types`** (NEW canonical lookup — the single list of product types, so coverage
AND parser_rules FK to ONE truth; kills V4 by construction):
- `code TEXT PK` — `flight | hotel | fit | group_tour` (Codex finding #1: `fit` 機+酒 and
  `group_tour` 跟團 are distinct; `package` is NOT a product type here — it is the legacy
  `offers.type` storage bucket. See "offers.type mapping" below.)
- `description TEXT` — reference text (enum doc, not per-provider prose)

**`ota_source_coverage`** (NEW child — one row per `(source_id, product_type)`):
- `source_id TEXT` — FK → `ota_sources.source_id`
- `product_type TEXT` — FK → `product_types.code` (PK pair `(source_id, product_type)`)
- `proven INTEGER NOT NULL DEFAULT 0 CHECK(proven IN (0,1))` — 1 = a real offer was
  produced & verified
- `proven_at TEXT` — ISO date proven (NULL until proven)
- `method TEXT CHECK(method IN ('agent_parse','regex'))` — how offers are produced
- `search_url TEXT` — canonical search/listing URL TEMPLATE for this `(source,type)`
  (may contain `{depart_date}` etc.; **region IDs do NOT go here** — see child table)
- `blocked_reason_code TEXT` — FK → `coverage_block_reasons.code`; NULL when ok
- `updated_at DATETIME`
- **CHECK / invariant:** `proven=1` ⇒ `proven_at IS NOT NULL AND method IS NOT NULL`
  (enforced in the write CLI; a row-level CHECK if SQLite expresses it cleanly).

**`ota_source_region_codes`** (NEW grandchild — Codex finding #3: region maps like
travel4u `41=東京｜東北` and besttour `295=東京` are multi-valued facts hiding in `notes`
and in `{region_id}` URL templates; normalize them, never a list-in-a-string):
- `source_id TEXT`, `product_type TEXT`, `region_label TEXT` — PK triple
- `region_code TEXT NOT NULL` — the provider's numeric/string area code
- (FK `(source_id, product_type)` → `ota_source_coverage`)

**`coverage_block_reasons`** (NEW lookup, so the reason is normalized data not prose):
- `code TEXT PK` — `renderer_wedge | login_wall | captcha | cloudflare | redundant |
  unsupported`
- `description TEXT` — reference expansion (NOT per-provider prose)

**offers.type mapping (Codex finding #1 — do NOT overload `package`).** `offers.type` keeps
its existing CHECK `('package','flight','hotel')` UNCHANGED. The canonical `product_type`
maps to storage: `flight→flight`, `hotel→hotel`, `fit→package`, `group_tour→package`. The
distinction we lost lives in `product_type` (coverage) + `parser_rules.product_type`, not in
`offers.type`. No existing CHECK constraint or offer query changes.

This makes the coverage matrix a JOIN, e.g.:
```sql
SELECT c.source_id, c.product_type, c.proven, c.proven_at, c.method,
       c.search_url, b.description AS blocked
FROM ota_source_coverage c
LEFT JOIN coverage_block_reasons b ON b.code = c.blocked_reason_code
ORDER BY c.product_type, c.source_id;
```

### 2. Make the DB authoritative, not the Rust array (kill V1 + V5)

The `OTA_SOURCES` / `DESTINATIONS` arrays stop being the source of truth. Two parts:

- **One-time bootstrap from a checked-in SQL file, not Rust (Codex Q4).** The cold-start
  catalog moves OUT of the `OTA_SOURCES`/`DESTINATIONS` Rust `const` arrays into a
  checked-in **`scripts/seed/ota_catalog.seed.sql`** (and `destination_catalog.seed.sql`) of
  `INSERT OR IGNORE` statements — replayable, DB-native, zero catalog facts in code. `db
  migrate` runs it ONLY to fill an empty catalog; it MUST NOT overwrite live rows.
- **Stop the migrate-time overwrite (Codex finding/Q4 hazard).** Today `seed_ota_sources`
  does `INSERT…ON CONFLICT DO UPDATE` (db_migrate.rs:1198, re-asserting code over the DB) and
  DELETE+reinsert of child type/region rows every migrate (db_migrate.rs:1212-1218). Both
  must change to insert-if-absent so a live edit (via the CLI above) is never clobbered by a
  later `db migrate`. This is the change that actually makes the DB authoritative.
- **CLI mutation path** (the missing write surface). Add audit-tracked commands so the DB
  is edited through the CLI, never by editing Rust:
  - `set-ota-source <source_id> --name <..> --status active|inactive`
  - `set-ota-coverage <source_id> <product_type> [--proven] [--proven-at <date>]
     [--method agent_parse|regex] [--search-url <url>] [--blocked <reason_code>]`
  - `set-ota-region <source_id> <product_type> <region_label> <region_code>`
  - **Global-catalog audit (Codex finding/Q1):** `operation_runs.plan_id` is `NOT NULL`
    (schema.sql:518) so it can't audit a global catalog edit, and legacy `events.data_text`
    is a JSON-ish text column we won't touch. Add a dedicated **`catalog_runs`** audit table
    (`run_id TEXT PK, command_type TEXT, command_summary TEXT, status TEXT, changed_at TEXT`)
    — the global analogue of `operation_runs`. Every catalog CLI mutation writes one
    `catalog_runs` row (the audit pattern, made global). No `plans.version` bump (catalog is
    not plan-scoped).

### 2a. Re-key `parser_rules` per product type (Codex finding #2)

`parser_rules` is currently `PRIMARY KEY (source_id)` with `product_kind TEXT DEFAULT 'fit'`
(schema.sql:565-567), so a multi-product source (eztravel = flight+hotel+fit) cannot hold
distinct rules per type. Re-key to **`PRIMARY KEY (source_id, product_type)`** and rename
`product_kind → product_type` with an FK → `product_types.code`. This makes parser rules and
coverage share the ONE canonical product-type list (so V4 cannot recur). Migration: existing
single-row-per-source rules become the row for that source's primary type; back-reconcile the
besttour/travel4u rows from `fit` → `group_tour` (Codex finding #4: `add_besttour_offer.rs:159`
writes `product_kind="fit"` for what is actually a group tour — fix that writer too).

### 3. Move proven-status OUT of documents (kill V3 + V4)

- **CLAUDE.md**: replace the OTA Sources status table + the per-source "PROVEN REAL/…"
  prose with a single pointer: "Provider coverage is DB data — run `travel ota-status`."
  Keep ONLY non-data guidance (how-to recipes can stay as docs; *status facts* cannot).
- **New read view** `travel ota-status [--type flight|hotel|fit|package]` — renders the
  coverage matrix (plain text, agent-first) straight from the JOIN above. This becomes the
  single answer to "which agents work for which sources."
- **Fix V4 by construction**: there is ONE canonical type list (`product_types`), and BOTH
  `ota_source_coverage.product_type` and (renamed) `parser_rules.product_type` FK to it — so
  they cannot disagree. No "reconcile two lists" rule needed; the FK is the enforcement.
- The `validate data` CLAUDE.md↔DB consistency check (which currently parses the prose
  table) is repointed: it asserts CLAUDE.md contains the pointer and does NOT re-encode
  status, so there is nothing to drift.

### 4. LLM text stays text (the one allowed exception)

No change to `captures.raw_text` (LLM input) or the agent-parse TSV→`offers` path. The
agent's raw extraction is the legitimate text payload; once normalized into typed `offers`
rows it is data. We do NOT add any JSON/blob anywhere.

## What this explicitly does NOT do (scope guard)

- Does not re-architect `offers`/`plan_offers` (already normalized).
- Does not touch the LLM/agent capture+parse pipeline.
- Does not move how-to *recipes* out of docs — only *status/coverage facts*.
- Does not change `event_log_*`/`events` (the work-state system already exists and is
  correct; using it more is a separate follow-up, not this design).

## Migration / data preservation

- **Two-phase notes migration (Codex finding/Q5 — do NOT parse-and-drop in one step).**
  Phase A: parse each of the 14 `ota_sources.notes` strings into the new
  coverage/region/block rows, AND record the raw string + a checksum + `normalized_at` in a
  temporary **`ota_notes_migration_audit`** table (`source_id, raw_note, checksum,
  normalized_at, disposition`) where `disposition ∈ {normalized, discarded_recipe}` — every
  fact must land in a typed column OR be explicitly marked a non-data recipe by review.
  `notes` is KEPT (read-only, deprecated) this release. Phase B (a later release, after the
  audit table confirms full coverage): DROP `notes`. This guarantees no fact is lost to an
  irreversible drop.
- The `tokyo_sep_2026` package/FIT offers were tagged to a TEST plan and removed with it;
  raw captures survive (eztravel/settour/besttour/travel4u), so coverage `proven=1` is
  still truthful (a real offer WAS produced). Re-populating offers is separate product
  work, not part of this schema change.

## Resolved questions (Codex recommendations, adopted)

1. **Global-catalog audit** → new `catalog_runs` table (NOT `operation_runs`: its `plan_id`
   is NOT NULL; NOT legacy `events`: its `data_text` is a JSON-ish column we won't touch).
2. **`ota_source_types`** → deprecate as a base table; if any reader still needs it, expose
   a VIEW over `ota_source_coverage` (one type list, no fresh V4).
3. **Canonical product type** → `product_types.code`; BOTH `ota_source_coverage` and
   `parser_rules` FK to it. `parser_rules.product_kind` is renamed `product_type`.
4. **Cold start** → checked-in seed SQL file (not Rust constants), empty-DB-only, never
   updates live rows.
5. **`search_url`** → per-`(source, product_type)` in coverage; region IDs in the
   `ota_source_region_codes` child (not embedded in the URL string).

## Review corrections (Codex 2026-06-29, each corroborated against source before folding in)

- **#1 (BLOCKING) `offers.type` CHECK has no `fit`** (schema.sql:516 = `package|flight|hotel`),
  and shaping already splits `group_tour` vs `fit` (shaping.rs:1065/1070). FIXED: canonical
  `product_types = flight|hotel|fit|group_tour`; `offers.type` UNCHANGED with `fit`+`group_tour`
  → stored as `package`. No CHECK/query change.
- **#2 (BLOCKING) `parser_rules` PK is `source_id` only** (schema.sql:566) → can't hold per-type
  rules for multi-product sources. FIXED: re-key to `(source_id, product_type)` + FK to
  `product_types` (§2a).
- **#3 (HIGH) `notes` carry region maps + recipe facts** beyond proven/method/url (e.g.
  lifetour `{region}` template). FIXED: `ota_source_region_codes` child + two-phase notes
  migration with an `ota_notes_migration_audit` table; nothing dropped until every fact lands.
- **#4 `operation_runs.plan_id` NOT NULL** (schema.sql:518) → can't audit global edits. FIXED:
  `catalog_runs`.
- **#4-drift confirmed worse than documented:** `add_besttour_offer.rs:159` WRITES
  `product_kind="fit"` for a group tour. FIXED: reconcile that writer + the rule to `group_tour`.
- **Migrate-overwrite hazard** (db_migrate.rs:1198 ON CONFLICT DO UPDATE; 1212-1218 DELETE+reinsert
  children every migrate) — must become insert-if-absent or the DB is never truly authoritative.
  FIXED in §2.
- Diagnosis V1–V5 all confirmed real against cited source; core `(source_id, product_type)`
  shape confirmed sound subject to the FKs/CHECKs above.
