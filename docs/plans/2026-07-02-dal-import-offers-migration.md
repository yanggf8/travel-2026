# DAL migration: import-offers → repo::plan_offers (the full offer family) + dedup + audit

**Date:** 2026-07-02 · **Status:** READY (Codex-reviewed + corroborated; corrections folded in). Fourth
mutation DAL migration; the big offer-pipeline one. **A 2-for-1-plus:** DAL move + dedup two
locally-duplicated helpers + switch hand-rolled audit to `record_operation`. Byte-identical. **ONE
migration, not split** (Codex — the import offer family must move as one unit; no partial DAL ownership).
**Pipeline:** plan → Codex behavior-lock test → Grok impl → Claude verify.

## Corrected facts (Codex review, corroborated vs source) — my draft had errors
- **INPUT SOURCE (my draft was WRONG):** import-offers reads JSON scrape FILES via `--files`/`--dir`
  (import_offers.rs:139/195), NOT the global `offers` table. It has **no `--plan-id`** — plan comes from
  `TRAVEL_PLAN_ID` env (import_offers.rs:117). So the test creates a temp scrape JSON + sets
  `TRAVEL_PLAN_ID`, runs `import-offers --files <tmp> --dest <dest>`.
- **A WRITE I MISSED:** `insert_provenance` (import_offers.rs:251) — provenance is another plan_offer_*
  -family write to migrate (repo needs `insert_import_provenance`).
- **product_code is NOT persisted** — `url` persists (offer.url), `product_code` is hardcoded NULL (same
  as promote). Do NOT start persisting product_code (import_offers.rs:449).
- **warnings:** `plan_offer_warnings (plan_id, destination, warning_type, message)` with warning_type
  hardcoded 'parse', offer_id NULL; schema PK is an autoincrement `id` (schema.sql:763). Append-only —
  NOT in delete_offer_rows, no other DELETE. CAVEAT: the parser may have no CLI-reachable Err path
  (scrape_parser.rs:194), so the warnings insert may be effectively DEAD via the public CLI — the lock
  can't fully exercise it; cover repo `insert_warning` separately + assert import doesn't delete a
  pre-seeded warning.
- delete_offer_rows + upsert_process_status LOCAL copies are behavior-identical to the repo's (same table
  list/order + same excluded.* ON CONFLICT) → dedup is byte-safe (source bytes differ only by formatting).
- date_pricing here is MULTI-row (one INSERT per offer.date_pricing entry), NO currency, NO ON CONFLICT
  (import_offers.rs:565) — DIFFERS from insert_offer's single synthesized row (which has currency) AND
  from upsert_date_pricing (which has ON CONFLICT). Import needs its OWN bulk date_pricing write.
- new_run_id becomes dead after the record_operation swap (import_offers.rs:40 import + the block).

## Scope (import_offers.rs)
import-offers materializes scraped `CanonicalOffer`s into the full `plan_offer_*` family. Domain writes:
1. **plan_offers** INSERT (449): RICHER than promote's — writes `url` (?10) + reads product_code path
   (both NULL in promote's bridge shape). So it needs its OWN write shape, NOT promote's PlanOfferWrite.
2. **plan_offer_includes** (477), **plan_offer_flights** (496), **plan_offer_hotels** (526) — same shapes
   the repo already has via insert_offer, but driven from CanonicalOffer.
3. **plan_offer_hotel_access** (549): `(plan_id, destination, offer_id, sort_order, line)` — NEW to repo.
4. **plan_offer_date_pricing** (568): bulk (multi-date) — NEW vs promote's single synthesized row; import
   writes one row PER date in the offer's pricing. Confirm the column set (Codex).
5. **plan_offer_best_value** (589): `(plan_id, destination, offer_id, best_date, best_price, currency,
   updated_at)` — NEW to repo.
6. **plan_offer_warnings** (643): `(plan_id, destination, warning_type, message)` with warning_type='parse'
   — NEW to repo. NOTE: keyed on (plan_id, destination, warning_type), NOT offer_id — a PLAN-level warning,
   not per-offer, and NOT in the delete_offer_rows list.
7. **delete_offer_rows** LOCAL (407): const table list IDENTICAL to repo::plan_offers::delete_offer_rows
   (verified same 7 tables/order). → call the repo's; delete the local copy (dedup).
8. **upsert_process_status** LOCAL (685): ON CONFLICT excluded.* IDENTICAL to repo's. → call the repo's;
   delete the local copy (dedup).
9. **hand-rolled audit** (345-367): new_run_id + operation_runs INSERT + plans.version UPDATE →
   `cascade::common::record_operation`. Remove local new_run_id if it becomes dead.

## Repo API (Codex-resolved: NEW shape, do NOT extend PlanOfferWrite)
- REUSE as-is: `repo::plan_offers::delete_offer_rows`, `upsert_process_status` (dedup the local copies).
- ADD `insert_import_offer(conn, &ImportPlanOfferWrite, now_db)` + child structs, covering: plan_offers
  (with url, product_code=NULL), includes, flights (if parsed), hotels + hotel_access (if parsed),
  date_pricing (MULTI-row bulk INSERT, no currency, no ON CONFLICT), best_value (if present). Map
  CanonicalOffer → ImportPlanOfferWrite IN THE CLI — travel-db must NOT depend on CanonicalOffer.
- ADD `insert_import_provenance(conn, ...)` for the provenance write (import_offers.rs:251).
- ADD `insert_warning(conn, plan_id, dest, warning_type, message)` for plan_offer_warnings (plan-level,
  offer_id NULL, append-only).

## What STAYS in the command
CanonicalOffer parsing/normalization (the whole scrape→CanonicalOffer path), the per-offer loop, the
promotable/skip decisions, best-value computation, warning collection, record_operation call, version
read, println.

## BYTE-IDENTITY RISKS (Codex to corroborate the specifics)
- plan_offers INSERT writes url (?10) — NOT NULL like promote; product_code path per current code.
- date_pricing is MULTI-date (one row per date) here, vs promote's single synthesized row — different loop;
  confirm column set + whether it has currency/ON CONFLICT.
- plan_offer_warnings keyed on (plan_id, destination, warning_type), hardcoded 'parse', NOT per-offer, NOT
  in the delete list — its lifecycle differs; preserve exactly.
- hotel_access / best_value column sets verbatim.
- delete_offer_rows + upsert_process_status must be BYTE-IDENTICAL to the repo's (they are — verified) so
  the dedup is safe.
- record_operation identical to the hand-rolled block (verified for prior migrations).
- No new transaction.

## Test oracle (Codex writes; NONE exists) — corrected seed model
new rust/crates/travel-cli/tests/import_offers.rs — behavior lock, PASS against current code. import-offers
reads a JSON scrape FILE (`--files`), plan from `TRAVEL_PLAN_ID` env. So: create a temp scrape JSON at
runtime (Format A `{ "offers": [...] }` with a hotel + flights + multi-date pricing + best_value so all
child tables fire), seed a plan + plan_metadata + process_statuses, set `TRAVEL_PLAN_ID`, run
`travel import-offers --files <tmp> --dest <dest>`, then assert: plan_offers.url present + product_code
NULL, includes, flights, hotels, hotel_access, TWO date_pricing rows, best_value, provenance, P3_4 status,
ONE operation_runs row, plans.version +1. WARNINGS: can't behavior-lock through the CLI (parser Err path
may be unreachable) — instead pre-seed a plan_offer_warnings row and assert import does NOT delete it;
cover `insert_warning` in a repo-level test after migration. Guard + credless skip (CLAUDE.md:118).

## Open questions — RESOLVED by Codex review
1. Repo API → NEW `insert_import_offer` + `ImportPlanOfferWrite` (+ insert_import_provenance,
   insert_warning); do NOT extend PlanOfferWrite (import's shape genuinely differs). travel-db must not
   depend on CanonicalOffer — map in the CLI.
2. date_pricing → MULTI-row bulk INSERT, no currency, no ON CONFLICT (differs from both insert_offer and
   upsert_date_pricing). Preserve import's exact bulk INSERT.
3. warnings → append-only, not in any delete path; possibly dead via the CLI (parser Err unreachable).
   Test asserts non-deletion + repo insert_warning separately.
4. input → JSON scrape files (`--files`/`--dir`), TRAVEL_PLAN_ID env, no --plan-id. Test builds a temp
   scrape JSON.

## Migration boundary: ONE unit (Codex)
Do NOT split by table. The whole import offer-family write + provenance + warnings + the dedup + the audit
swap move together — a partial migration would leave split DAL ownership of the plan_offer_* family.
