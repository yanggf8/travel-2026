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
> URLs 404). Do NOT run `python scripts/scrape_*.py`. Get OTA offers via gwebcdb on WSLg,
> then have the agent extract offers from `captures.raw_text` and persist TSV with
> `./rust/target/debug/travel ota write-offers <job_id> --capture <capture_id> --claim-token <token> --tsv <path>`.
> Python commands below are historical reference only.

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
# Capture OTA packages via gwebcdb on WSLg, then agent-extract TSV and write offers (see /scrape-ota)
#   ./rust/target/debug/travel ota write-offers <job_id> --capture <capture_id> --claim-token <token> --tsv <path>

# Import scraped JSON files into Turso (legacy scrapes/ landing zone)
./bin/travel import-offers --dir scrapes --dest <slug>

# Query available offers in DB
./bin/travel query-offers --plan-id <id> --dest <slug> [--max-price 25000]

# Check if data is fresh
./bin/travel check-freshness --source besttour --region kansai

# Select a package (populates P3+P4 via cascade)
./bin/travel select-offer <offer-id> <date>
```

### Command Reference

| Command | Description | Required Args | Optional Args |
|---------|-------------|---------------|---------------|
| gwebcdb capture + `ota write-offers` | Capture an OTA page, agent-extract TSV, persist to Turso (see /scrape-ota) | `<url>`, `<job_id>`, `<capture_id>`, `<token>` | `--tsv` |
| `import-offers` | Import scraped JSON into DB | `--dir`, `--dest` | `--start`, `--end`, `--dry-run` |
| `query-offers` | List offers from DB | `--plan-id`, `--dest` | `--max-price` |
| `check-freshness` | Check if scraped data is stale | `--source`, `--region` | `--plan-id` |
| `select-offer` | Select package for booking | `<offer-id>`, `<date>` | None |

## Workflow Examples

### Example 1: Package Search and Selection

```bash
# 1. Set travel dates
./bin/travel set-dates 2026-02-24 2026-02-28

# 2. Capture packages from OTAs via gwebcdb, then agent-extract TSV and write offers (see /scrape-ota)
#   ./rust/target/debug/travel ota write-offers <job_id> --capture <capture_id> --claim-token <token> --tsv <path>

# 3. Import scraped files into DB (legacy scrapes/ landing zone)
./bin/travel import-offers --dir scrapes --dest kyoto_2026

# 4. Review available offers
./bin/travel query-offers --plan-id kyoto-2026 --dest kyoto_2026

# 5. Select package (auto-populates P3+P4 via cascade)
./bin/travel select-offer liontravel_190620015 2026-02-24

# 6. Verify P3/P4 populated
./bin/travel transport
```

### Example 2: Budget-Constrained Search

```bash
# Capture with gwebcdb, then agent-extract TSV and write offers (see /scrape-ota), then:

# Import and query with max-price filter
./bin/travel import-offers --dir scrapes --dest kyoto_2026
./bin/travel query-offers --plan-id kyoto-2026 --dest kyoto_2026 --max-price 20000
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
- **External tools**: gwebcdb on WSLg (`~/b/gwebcdb`) + agent TSV extraction + `travel ota write-offers` (see `/scrape-ota`)
- **Required DB**: `ota_sources` table in Turso (OTA registry — replaces removed `data/ota-sources.json`)

## Data Acquisition

### Supported OTAs

| OTA | Type | Status | Capture |
|-----|------|--------|---------|
| BestTour (喜鴻假期) | Package | ✅ Full | gwebcdb capture + `ota write-offers` (`source_id=besttour`) |
| Lion Travel (雄獅旅遊) | Package | ✅ Base | gwebcdb capture + `ota write-offers` (`source_id=liontravel`) |
| ezTravel (易遊網) | Package | ⚠️ Limited | gwebcdb capture + `ota write-offers` (`source_id=eztravel`) |

### Capture Commands

Python scrapers are decommissioned. Drive the real OTA page with gwebcdb on WSLg, then have
the agent read `captures.raw_text`, emit TSV, and persist offers (full flow in `/scrape-ota`):

```bash
# Drive + capture a package page in ~/b/gwebcdb (clicks/fills the actual UI — no URL templates)
python bridge/navigate.py "<url>"
python bridge/ota_capture.py --source besttour [--url-contains <substr>]   # → capture_id

# Agent-extract TSV from captures.raw_text, then persist Turso offers
./rust/target/debug/travel ota write-offers <job_id> --capture <capture_id> --claim-token <token> --tsv <path>
```

## DB Integration

After package selection, bookings are automatically synced to Turso:
- `StateManager.save()` writes to normalized tables (no JSON)
- Query bookings: `./bin/travel query-bookings --category package`
- The agent should use `query-bookings` to check status, not read JSON paths
- Manual sync: `./bin/travel sync-bookings`

## Notes

- Package selection automatically populates P3 and P4 (cascade)
- Use this skill for bundled deals; use `/p3-flights` + `/separate-bookings` for separate bookings
- Package offers include `date_pricing` for availability tracking
- StateManager handles offer availability updates via `updateOfferAvailability()`
