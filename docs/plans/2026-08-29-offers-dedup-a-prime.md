# offers re-ingest dedup (A′: content hash + `last_seen_at`)

Date: 2026-08-29. Status: **approved (P0 + freshness switch)** — follow-up to the 2026-08-29 OTA
observability commits (`4c5042d` / `949875e` / `8f7cb4e`). Design consulted with Codex
(baicodex/modelstudio), every keystone claim corroborated against source before adoption.

## Problem

`offers` PK is `(id, scraped_at)`; `write-offers` stamps `scraped_at = now_iso()` once per run.
Re-ingesting the same TSV through a new job/attempt gets a fresh `scraped_at`, so the PK never
collides and `ON CONFLICT(id, scraped_at) DO NOTHING` never fires — the whole batch **silently
duplicates** (`query-offers` shows the same package N times, `promote-offers` can promote
duplicates, freshness is diluted). The under-extraction WARN cannot catch it.

Key corroborated facts that shape the fix:

- `id` embeds price + flight number (`ota/common.rs::disambiguate_ids`, `{base}_{fno}_{price}`),
  so a price change is a *different* id — `(id)` latest-wins upsert (option B) could not update
  prices and would break PK-keyed readers. Rejected.
- The final `id` carries batch-order-dependent `_2`/`_3` tie suffixes — it is **not** a stable
  content key and must never be hashed.
- `repo::offers::insert` is the sole production writer of global `offers` (only caller
  `ota/common.rs::write_offers`) — one choke point covers every writer.
- `disambiguate_ids` runs *inside* `common::write_offers` and overwrites `row.id`; the stable
  pre-disambiguation id (`offer_row_id(source_id, product_code, departure, nights)`) must be
  captured into `offer_key` before that happens.
- SQLite UNIQUE treats NULLs as distinct → the new UNIQUE index cannot fail on the pre-backfill
  all-NULL column. A *targeted* `ON CONFLICT(id, scraped_at)` clause would RAISE on a
  `dedup_key` violation instead of ignoring — insert must switch to bare `ON CONFLICT DO NOTHING`.
- SQLite has no sha256 UDF → backfill must be a Rust pass, not a seed file.

## Change (P0)

1. **Three new `offers` columns** (via `add_column()` in `db_migrate.rs`):
   `offer_key TEXT` (stable logical identity = pre-disambiguation base id), `dedup_key TEXT`
   (sha256 content fingerprint), `last_seen_at TEXT` (bumped when content is re-observed).
2. **Indexes**: `CREATE UNIQUE INDEX IF NOT EXISTS idx_offers_dedup_key ON offers(dedup_key)`;
   `idx_offers_offer_key ON offers(offer_key, scraped_at)` (price history lookup);
   `idx_offers_last_seen ON offers(last_seen_at)`.
3. **`offer_key` capture**: `parsed_to_offer_row` sets `offer_key = Some(base_id)` alongside
   `id` so the identity survives `disambiguate_ids`.
4. **`repo::offers::dedup_key(&OfferRow)`**: sha256 over `offer-v1` + length-prefixed,
   whitespace/ASCII-case-normalized components: `offer_key, source_id, type, name,
   price_per_person, currency, destination, departure_date, return_date, nights, availability,
   hotel_name, airline, flight_outbound, flight_return`.
   **Price is INSIDE the hash** — with price outside, UNIQUE+DO NOTHING would silently drop real
   price changes; with price inside, identical re-ingest is a no-op and a price change is a new
   row. Excluded (provenance/temporal/derived): `id, source_file, region, scraped_at, capture_*,
   produced_by_*, parser_*, normalizer_version, created_at, raw_data, hotel_area, includes`
   (region is derived per-job → nondeterministic; hotel_area/includes are live-schema columns
   not present on `OfferRow`).
5. **`insert()`**: 28 params, bare `ON CONFLICT DO NOTHING`; on `affected == 0` →
   `UPDATE offers SET last_seen_at = <incoming scraped_at> WHERE dedup_key = ?` so the
   inserted/deduped counters stay honest (a DO UPDATE would report every touch as inserted and
   break `under_extraction_warn` + `ota_attempts.deduped_count`).
6. **Backfill** `backfill_offers_dedup_keys` (Rust pass in `db_migrate.rs`, called after the new
   columns/indexes): oldest-first over `dedup_key IS NULL` rows; conditional
   `UPDATE … WHERE dedup_key IS NULL AND NOT EXISTS (… o2.dedup_key = ?)` keeps the first-seen
   row canonical; pre-existing content duplicates are left `dedup_key IS NULL` (visible debt —
   **never DELETE in migrate on the shared live DB**); prints plain-text counts.
7. **Freshness switch (approved into P0)**: all liveness readers read
   `COALESCE(last_seen_at, scraped_at)` — `OfferFilter::fresh_within_hours`,
   `repo/freshness.rs offers_freshness MAX`, `db_status.rs offers_last_scraped_at`,
   `offers.rs` (query-offers) ORDER BY, `db_query_offers.rs` age_hours + ORDER BY. Without this,
   an unchanged re-scrape leaves the surviving row at first-seen time and freshness falsely
   reports stale.
8. **WARN text**: with A′ the `inserted == 0` branch finally means "identical content already
   ingested" — message updated accordingly.

## Tests

- Unit (travel-db): key determinism; provenance/temporal fields don't change the key; price
  change does; normalization equivalence; golden vector; pinned hash-field list.
- Unit (travel-cli): WARN text; migrate backfill on in-memory libsql (idempotency, dup-keeps-first,
  no-delete) mirroring `ota_coverage_backfill_tests`.
- Integration (shared Turso, `Guard` + zz-prefix discipline): re-insert same content with a later
  `scraped_at` → `deduped=1`, one row, `last_seen_at` bumped while `scraped_at` holds; price
  change → two rows; column-drift guard pinning `PRAGMA table_info(offers)` to the expected set.
- CLI e2e (`ota_cluster_a.rs`): second job re-running the same TSV → `inserted=0 deduped=N`,
  `db query-offers` shows the package once.

## Out of scope (later)

P2: explicit collapse command for legacy `dedup_key IS NULL` duplicates (dry-run SELECT first,
keep `MIN(scraped_at)`). SKILL.md procedural note (defense in depth only).
