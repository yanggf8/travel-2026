---
name: p3p4-packages
description: Search and select package deals (flight + hotel) that populate both P3 and P4 processes
version: 1.1.0
requires_skills: [travel-shared, scrape-ota]
requires_processes: [process_1_date_anchor, process_2_destination]
provides_processes: [process_3_4_packages, process_3_transportation, process_4_accommodation]
---

# /p3p4-packages

> **⚠️ Python scrapers DECOMMISSIONED** (archived in `archive/broken-python-scrapers/`; constructed
> URLs 404). Do NOT run `python scripts/scrape_*.py`. Get OTA offers via the Rust CDP driver:
> `./rust/target/debug/chromeport fetch interact "<url>" --source <id> --step ...` (or
> `browser snapshot`) → `parse capture <id> --source <id>` (imports to Turso). Python commands below
> are historical reference only.

Search and select package deals (flight + hotel combined).

## Shared references

Read first unless request is extremely narrow:
- `../travel-shared/references/io-contracts.md`
- `../travel-shared/references/state-manager.md`
- `../travel-shared/references/cascade-triggers.md`
- `../scrape-ota/SKILL.md` — OTA scraping commands and URL patterns
- `references/legacy-spec.md` — full canonical offer schema + scraper interface spec (read when implementing or validating offer shapes)

## Overview

This skill handles package tours that bundle transportation and accommodation. When a package is selected, it automatically populates P3 (transportation) and P4 (accommodation) via cascade.

## Input Schema

```typescript
interface PackageSearchInput {
  destination: string;      // Destination slug
  dates: {
    start: string;          // YYYY-MM-DD
    end: string;            // YYYY-MM-DD
  };
  budget?: number;          // Max price per person
  type?: 'fit' | 'group';   // Package type
}
```

## Output Schema

Writes via StateManager (DB-backed, no direct JSON):
```typescript
process_3_4_packages: {
  status: 'researched' | 'selected';
  results: {
    offers: PackageOffer[];
    chosen_offer?: PackageOffer;
  };
  selected_offer_id?: string;
  updated_at: string;
}
```

## CLI Commands

```bash
# Scrape OTA packages (batch — all sources for a region)
npm run scraper:batch -- --dest kansai [--sources besttour,liontravel] [--date 2026-02-24 --type fit]

# Scrape a specific package URL
python scripts/scrape_package.py "<url>" scrapes/besttour-<code>.json

# Scrape package listings page
python scripts/scrape_listings.py --source besttour --dest kansai

# Import scraped JSON files into Turso
npm run travel -- import-offers --dir scrapes --dest <slug>

# Query available offers in DB
npm run travel -- query-offers --plan-id <id> --dest <slug> [--max-price 25000] [--json]

# Check if data is fresh
npm run travel -- check-freshness --source besttour --region kansai

# Select a package (populates P3+P4 via cascade)
npm run travel -- select-offer <offer-id> <date>
```

### Command Reference

| Command | Description | Required Args | Optional Args |
|---------|-------------|---------------|---------------|
| `scraper:batch` | Scrape all OTAs for a region | `--dest` | `--sources`, `--date`, `--type` |
| `import-offers` | Import scraped JSON into DB | `--dir`, `--dest` | `--start`, `--end`, `--dry-run` |
| `query-offers` | List offers from DB | `--plan-id`, `--dest` | `--max-price`, `--json` |
| `check-freshness` | Check if scraped data is stale | `--source`, `--region` | `--plan-id` |
| `select-offer` | Select package for booking | `<offer-id>`, `<date>` | None |

## Workflow Examples

### Example 1: Package Search and Selection

```bash
# 1. Set travel dates
npm run travel -- set-dates 2026-02-24 2026-02-28

# 2. Scrape packages from OTAs
npm run scraper:batch -- --dest kansai --type fit

# 3. Import scraped files into DB
npm run travel -- import-offers --dir scrapes --dest kyoto_2026

# 4. Review available offers
npm run travel -- query-offers --plan-id kyoto-2026 --dest kyoto_2026

# 5. Select package (auto-populates P3+P4 via cascade)
npm run travel -- select-offer liontravel_190620015 2026-02-24

# 6. Verify P3/P4 populated
npm run view:transport
```

### Example 2: Budget-Constrained Search

```bash
# Scrape with budget filter
npm run scraper:batch -- --dest kansai --type fit

# Import and query with max-price filter
npm run travel -- import-offers --dir scrapes --dest kyoto_2026
npm run travel -- query-offers --plan-id kyoto-2026 --dest kyoto_2026 --max-price 20000

# Or filter scraped JSON directly
python scripts/filter_packages.py scrapes/*-scrape.json --max-price 20000 --type fit
```

## Error Handling

| Error | Cause | Solution |
|-------|-------|----------|
| `P1 not confirmed` | Dates not set | Run `set-dates` first |
| `Offer not found` | Invalid offer ID | Check available offers with `view:status` |
| `Date not available` | Selected date sold out | Choose different date or package |
| `Scraper timeout` | OTA site slow/down | Retry or use cached results |

## State Changes

- **StateManager**: 
  - Updates `process_3_4_packages.results.offers`
  - On selection: populates `process_3_transportation` and `process_4_accommodation`
- **Event log**: 
  - Emits `package_offers_imported` event
  - Emits `offer_selected` event
  - Emits `cascade_populated` event
- **Cascade triggers**: 
  - Marks P3, P4 dirty when dates change
  - Clears P3, P4 dirty when package selected

## Dependencies

- **Required processes**: P1 (dates), P2 (destination)
- **Required skills**: `/scrape-ota` for OTA integration
- **External tools**: Python scrapers (`scripts/scrape_package.py`, `scripts/scrape_listings.py`)
- **Required DB**: `ota_sources` table in Turso (OTA registry — replaces removed `data/ota-sources.json`)

## Data Acquisition

### Supported OTAs

| OTA | Type | Status | Scraper |
|-----|------|--------|---------|
| BestTour (喜鴻假期) | Package | ✅ Full | `scrape_package.py` |
| Lion Travel (雄獅旅遊) | Package | ✅ Base | `scrape_package.py` |
| ezTravel (易遊網) | Package | ⚠️ Limited | `scrape_listings.py` |

### Scraping Commands

```bash
# Scrape specific package URL
python scripts/scrape_package.py <url>

# Scrape listings page
python scripts/scrape_listings.py --source besttour --dest kansai

# Filter results
python scripts/filter_packages.py scrapes/*.json --type fit --date 2026-02-24
```

## DB Integration

After package selection, bookings are automatically synced to Turso:
- `StateManager.save()` writes to normalized tables (no JSON)
- Query bookings: `npm run travel -- query-bookings --category package`
- The agent should use `query-bookings` to check status, not read JSON paths
- Manual sync: `npm run travel -- sync-bookings`

## Notes

- Package selection automatically populates P3 and P4 (cascade)
- Use this skill for bundled deals; use `/p3-flights` + `/separate-bookings` for separate bookings
- Package offers include `date_pricing` for availability tracking
- StateManager handles offer availability updates via `updateOfferAvailability()`
