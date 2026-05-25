# Tour-Group Scraper — Stage 0 Baseline Source

**Status:** design — not yet implemented
**Author:** Yang (+ assistant brainstorming pass)
**Date:** 2026-05-25
**Related:** `2026-05-25-price-baseline-and-rhythm-method.md` (the methodology this scraper unblocks); `2026-05-22-stage0-triangle-research-design.md` (the Stage 0 framework this extends)

## Motivation

The methodology spec (`price-baseline-and-rhythm-method.md`) defines the trip-price ceiling as **the cheapest comparable tour group**. Today the system has zero tour-group scrapers — every agency scraper in the repo targets FIT (機加酒/自由行) or hotel/flight products. Without a tour-group source, the baseline is permanently manual, which means the methodology cannot run inside Stage 0 and `stage0-compare` will never surface a defensible "discount vs baseline" number.

This spec builds the first tour-group scraping path end-to-end: scraper → JSON file → import → Turso storage → adopt-time bridge to plan-side offers. One agency (**BestTour 喜鴻**) is the engineering probe; the same shape extends to two more agencies (Lifetour 五福, Settour 東南) to give the baseline three independent sources for a defensible ceiling.

## Scope (in / out)

**In scope**
- Scraper for tour-group listings on BestTour, Lifetour, Settour (3 agencies).
- New unscoped Stage 0 tables for tour-group offers and scrape attempts.
- New CLI command to import scrape output into the unscoped tables.
- Schema extension on the plan side (`plan_offers.package_subtype`, new `plan_offer_group_meta` child table).
- Adopt-time bridge: at `stage0-adopt`, copy curated subset of tour-group offers into plan-scoped tables.

**Out of scope (deferred)**
- LionTravel group scrape (current LionTravel scraper is FIT; group URL/layout is unknown).
- Travel4U group scrape (catalog depth lower; add later if time permits).
- Detail-page fetch as primary scrape path (used only as fallback when listing fields are too thin — see §3).
- Stage 2 viewer changes (the existing `plan_offers` viewer will surface group tours once `package_subtype` is added; no new UI in this spec).
- `stage0-compare` ranking changes — exposing the baseline/discount in the compare view is the methodology spec's job, not this one.
- Automatic quality-floor selection — for now the bridge copies the multi-row audit set; deciding "which row is the baseline" stays in the methodology layer.

## 1. Agency selection and build order

Three agencies, sequenced for risk and decision quality:

1. **BestTour 喜鴻** — engineering probe. Lowest scrape risk (we've already successfully scraped their FIT calendar). Proves the output shape, the importer path, and the adopt-time bridge.
2. **Lifetour 五福** — second source, extends the proven path. Uses existing `scripts/scrape_listings.py` URL builder pattern.
3. **Settour 東南** — third source, same shape as Lifetour.

After step 3, the baseline ceiling is computed across three independent agencies — a defensible "cheapest comparable tour group" rather than a single-agency reading that could be misled by one outlier price.

LionTravel group and Travel4U are deferred. The deferred agencies do not block the methodology — three sources is already enough for a useful ceiling.

## 2. Schema changes

### 2.1 New unscoped tables (Stage 0 research side)

```sql
CREATE TABLE stage0_tour_group_offers (
  run_id                 TEXT    NOT NULL,
  offer_id               TEXT    NOT NULL,
  source_id              TEXT    NOT NULL,     -- besttour | lifetour | settour
  dest_region            TEXT    NOT NULL,     -- kansai | kanto | kyushu | tohoku | ...
  depart_date            TEXT    NOT NULL,     -- YYYY-MM-DD
  return_date            TEXT    NOT NULL,     -- YYYY-MM-DD
  nights                 INTEGER NOT NULL,
  price_per_person_twd   INTEGER NOT NULL,
  title                  TEXT    NOT NULL,
  url                    TEXT    NOT NULL,
  scraped_at             TEXT    NOT NULL,     -- ISO 8601
  -- nullable comparables (quality floor inputs)
  hotel_name             TEXT,
  hotel_star_rating      INTEGER,
  meals_included_count   INTEGER,
  departure_status       TEXT,                 -- available | guaranteed | waitlist | sold_out | unknown
  seats_available        INTEGER,
  min_group_size         INTEGER,
  group_size_cap         INTEGER,
  raw_json               TEXT,                 -- listing-card raw payload for audit/reparse
  parse_warnings_json    TEXT,                 -- optional: per-row parse issues
  PRIMARY KEY (run_id, offer_id),
  FOREIGN KEY (run_id) REFERENCES stage0_research_runs(run_id)
);

CREATE TABLE stage0_tour_group_scrape_attempts (
  run_id        TEXT    NOT NULL,
  source_id     TEXT    NOT NULL,
  dest_region   TEXT    NOT NULL,
  nights        INTEGER NOT NULL,
  status        TEXT    NOT NULL,    -- pending | ok | failed | partial
  offer_count   INTEGER,             -- total cards seen on listing
  parsed_count  INTEGER,             -- rows actually inserted into stage0_tour_group_offers
  skipped_count INTEGER,             -- rows skipped due to missing required fields
  error         TEXT,                -- error text if status=failed
  attempted_at  TEXT,                -- ISO 8601
  PRIMARY KEY (run_id, source_id, dest_region, nights),
  FOREIGN KEY (run_id) REFERENCES stage0_research_runs(run_id)
);
```

**Design notes:**
- Required fields (`source_id` through `scraped_at`) are NOT NULL — these are the baseline minimum the methodology needs.
- Nullable comparables (`hotel_name` through `group_size_cap`) are quality-floor inputs. The methodology computes two ceilings: raw cheapest (any row) and quality-floor cheapest (rows where comparables clear a threshold). Both work whether comparables are present or NULL.
- `raw_json` keeps the original listing-card payload so reparses are possible without re-scraping.
- `parse_warnings_json` records per-row issues (missing price, image-only star rating, etc.); does NOT prevent the row from being inserted unless a required field is missing — in that case the row is skipped and counted in `skipped_count`.
- `stage0_tour_group_scrape_attempts.status='partial'` exists for the important middle case: listing loaded, some rows imported, some rows skipped. Pure success/failure is too coarse.

### 2.2 Plan-side extension

```sql
-- Extend plan_offers
ALTER TABLE plan_offers ADD COLUMN package_subtype TEXT;
-- Values: 'group_tour' | 'fit' | 'flight_only'
-- Backfill existing rows: 'fit' (all current package offers are FIT)

CREATE TABLE plan_offer_group_meta (
  plan_id                TEXT    NOT NULL,
  offer_id               TEXT    NOT NULL,
  meals_included_count   INTEGER,
  departure_status       TEXT,
  seats_available        INTEGER,
  min_group_size         INTEGER,
  group_size_cap         INTEGER,
  source_offer_run_id    TEXT,        -- audit: which stage0 run the row came from
  source_offer_id        TEXT,        -- audit: stage0_tour_group_offers.offer_id
  PRIMARY KEY (plan_id, offer_id),
  FOREIGN KEY (plan_id, offer_id) REFERENCES plan_offers(plan_id, offer_id)
);
```

**Design notes:**
- `package_subtype` distinguishes tour groups from FIT in the plan-scoped table. Existing FIT-specific filters (`plan_offer_hotels.star_rating` etc.) remain on their child tables; group-specific fields go on `plan_offer_group_meta`.
- `source_offer_run_id` + `source_offer_id` preserve the audit trail from research-time to plan-time. If the methodology revisits the choice later ("why was THIS tour group the baseline?"), the back-pointer leads to the original `stage0_tour_group_offers` row.
- No `nullable plan_id` anywhere. No sentinel "research" plan. The `stage0_*` ↔ `plan_*` distinction stays clean: research data is unscoped and run-keyed; plan data is plan-keyed.

## 3. Scrape strategy

### 3.1 Listing-first

Each agency's tour-group listing page exposes most of the required fields per card: title, departure date, nights, price, URL, often hotel name and (sometimes) star rating. One listing-page fetch yields N cards across many dates.

**Default scrape unit:** one `(source_id, dest_region, nights)` listing fetch, sweeping all dates in the listing within the Stage 0 window.

**Fallback:** if required fields are missing on a card (e.g. price shown as "詢價" rather than a number), the row is **skipped** (counted in `skipped_count`), not detail-fetched. Detail-fetch fallback is a future enhancement, **not in scope for this build** — see §6.

### 3.2 Scrape attempt lifecycle

```
stage0_tour_group_scrape_attempts states:
  pending  → row created at run start (per agency × region × nights)
  ok       → listing loaded, parsed_count > 0, skipped_count == 0
  partial  → listing loaded, parsed_count > 0, skipped_count > 0
  failed   → listing did not load OR parsed_count == 0
```

Retry semantics: a `failed` attempt can be re-run by re-invoking the scraper for that `(run_id, source_id, dest_region, nights)`; the attempt row updates in place. A `partial` attempt is not retried automatically — the skipped rows usually need a code-level fix (parser change), not a re-fetch.

### 3.3 Output envelope

Scraper writes one JSON file per `(source_id, dest_region, nights)` attempt:

```json
{
  "run_id": "stage0-20260525-123000",
  "scraped_at": "2026-05-25T12:30:00Z",
  "source_id": "besttour",
  "dest_region": "kansai",
  "nights": 5,
  "tour_group_offers": [
    {
      "offer_id": "besttour-KIX-20260620-5n-abc123",
      "depart_date": "2026-06-20",
      "return_date": "2026-06-25",
      "nights": 5,
      "price_per_person_twd": 38900,
      "title": "關西超值五日｜大阪心齋橋｜京都嵐山｜含早晚餐",
      "url": "https://www.besttour.com.tw/...",
      "hotel_name": "Cross Hotel Osaka",
      "hotel_star_rating": 4,
      "meals_included_count": 6,
      "departure_status": "guaranteed",
      "seats_available": 12,
      "min_group_size": 16,
      "group_size_cap": 30,
      "raw_json": "{ ... listing card raw HTML or extracted dict ... }"
    }
  ]
}
```

**Envelope choices:**
- Top-level metadata (`run_id`, `source_id`, `dest_region`, `nights`) is the scrape-attempt identity — same key the attempts table uses. The importer validates that this top-level identity matches the existing `pending` attempt row for the run.
- `tour_group_offers` array: each element has all `stage0_tour_group_offers` columns (required + nullable comparables). Required fields missing → row is skipped at import time.
- `offer_id` is constructed by the scraper as `<source_id>-<dest_iata>-<depart_yyyymmdd>-<nights>n-<short_hash>` to be stable across re-scrapes of the same departure.

## 4. New CLI commands

```bash
# Import a single scrape output file into stage0_tour_group_offers + update the attempt row
npm run travel -- import-tour-group-offers --run <run_id> --file scrapes/besttour-kansai-5n.json

# Import all matching scrape files from a directory
npm run travel -- import-tour-group-offers --run <run_id> --dir scrapes/

# Show what's been collected for a run (read-only, debugging)
npm run travel -- query-tour-group-offers --run <run_id> [--source <id>] [--dest-region <region>] [--max-price <twd>]
```

`requiresState: false` for all three (Stage 0 commands are pre-plan).

The importer is **dedicated** to tour-group flow — it does not branch on `--run` vs `--plan-id`. It only writes to `stage0_tour_group_offers` and `stage0_tour_group_scrape_attempts`. The existing `import-offers` command (which writes to `plan_offers`) is **untouched**.

## 5. Adopt-time bridge (`stage0-adopt`)

When a Stage 0 candidate is adopted into a plan via `stage0-adopt <candidate_id> <plan_id> --create-plan --dest <slug>`, the existing logic already copies date anchor + destination into plan-scoped tables. This spec adds one step: copy the **curated subset** of tour-group offers for the candidate's `(dest_region, nights)` into `plan_offers` + `plan_offer_group_meta`.

**Which rows are copied (audit set):**
1. **Raw cheapest** — the absolute cheapest tour-group offer matching `(dest_region, nights, depart_date_within_run_window)`.
2. **Quality-floor cheapest** — the cheapest offer where `hotel_star_rating >= 4` (the default floor; configurable per spec §6).
3. **Top 3 per source/date** — for each `(source_id, depart_date)` combination, the three cheapest offers.

Rationale: copying only "the cheapest" loses auditability when the quality floor definition is later revised. The multi-row audit set preserves the data needed to recompute the baseline without re-scraping.

Each copied row writes `plan_offers.package_subtype='group_tour'` and a matching `plan_offer_group_meta` row with `source_offer_run_id` + `source_offer_id` pointing back to the originating `stage0_tour_group_offers` row.

## 6. Open / deferred decisions

These are the methodology spec's open questions made concrete in this build's terms:

1. **Quality-floor threshold default.** The spec uses `hotel_star_rating >= 4` for "quality-floor cheapest" in the adopt bridge. This is a default, not a fixed rule. The methodology spec's open Q1 still applies — when shopping a real trip, the threshold may shift (e.g. "≥3-star but with breakfast included"). For now: hardcode 4-star as the default, no configurability in this build.
2. **Detail-page fallback.** Out of scope for this build. The trigger condition (when to detail-fetch) and the retry semantics (separate attempt row? same attempt row updated?) need real-data evidence to design correctly. After the v1 build runs, if `skipped_count` is high on real BestTour data, that's the signal to add detail-fetch in a follow-up.
3. **Per-source offer ID stability.** Different agencies may not expose a stable per-offer ID on the listing card. The `<source_id>-<dest_iata>-<depart_yyyymmdd>-<nights>n-<short_hash>` construction is a fallback. If an agency exposes its own ID (BestTour's product code, Lion's tour ID), prefer that — fall back to the constructed form only when none is available.
4. **Dest-region normalization.** `dest_region` is a free-form string in the schema (e.g. `kansai`, `kanto`). The agency listing pages categorize differently (`日本-關西`, `日本/京阪神`, etc.). Each agency parser normalizes to the canonical region name. The canonical region vocabulary lives outside this spec (already used by `compare-offers --region kansai`).

## 7. Test plan

Integration tests (real DB, no mocks, following the existing test-pattern in `tests/integration/`):

1. **Importer happy path** — fixture JSON file → `import-tour-group-offers` → verify rows in `stage0_tour_group_offers`, attempt row in `stage0_tour_group_scrape_attempts` with `status='ok'`.
2. **Importer skipped rows** — fixture with missing required field → row skipped, `skipped_count` incremented, attempt `status='partial'`.
3. **Importer attempt-identity mismatch** — file with `source_id` not matching any pending attempt → importer rejects, no rows written.
4. **Adopt bridge** — seed `stage0_tour_group_offers` with N rows for a `(dest_region, nights)`, run `stage0-adopt --create-plan`, verify the audit set (raw cheapest + quality-floor cheapest + top-3 per source/date) appears in `plan_offers` + `plan_offer_group_meta`.
5. **Quality-floor handling when comparables are NULL** — rows with NULL `hotel_star_rating` are eligible for raw-cheapest but excluded from quality-floor; verify both selections work and adopt bridge handles the case where the quality-floor set is empty.

Scrapers themselves get **doctor tests** (network calls, may be flaky, run on demand) — same model as the existing `scripts/scraper_doctor.py`.

## 8. Build sequence

1. **Schema migration** — add the 4 tables/columns (2 new tables + `plan_offers.package_subtype` + `plan_offer_group_meta`). Backfill `package_subtype='fit'` for existing rows.
2. **Importer + query CLI** — `import-tour-group-offers`, `query-tour-group-offers`. Integration tests using fixture JSON, no scraper yet.
3. **BestTour scraper** — driven by the importer's expected envelope. Doctor test against the real site.
4. **Adopt bridge** — extend `stage0-adopt` to copy the audit set. Integration test.
5. **Lifetour + Settour scrapers** — extend the proven envelope to two more agencies. Doctor tests.
6. **Methodology spec hookup** — the methodology spec's manual workflow can now start using the baseline data. No automatic ceiling computation in this build — that comes later when `stage0-compare` is extended.
