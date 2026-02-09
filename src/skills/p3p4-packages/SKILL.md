---
name: p3p4-packages
description: Search and select package deals (flight + hotel) that populate both P3 and P4 processes
version: 1.1.0
requires_skills: [travel-shared, scrape-ota]
requires_processes: [process_1_date_anchor, process_2_destination]
provides_processes: [process_3_4_packages, process_3_transportation, process_4_accommodation]
---

# /p3p4-packages

Search and select package deals (flight + hotel combined).

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

Writes to `travel-plan.json`:
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
# Search packages
npm run travel -- search-packages

# Search with filters
npm run travel -- search-packages --type fit --max-price 25000

# Select a package
npm run travel -- select-offer <offer-id> <date>

# View package details
npm run travel -- show-offer <offer-id>
```

### Command Reference

| Command | Description | Required Args | Optional Args |
|---------|-------------|---------------|---------------|
| `search-packages` | Search OTA packages | None | `--type`, `--max-price` |
| `select-offer` | Select package for booking | `<offer-id>`, `<date>` | None |
| `show-offer` | Display offer details | `<offer-id>` | None |

## Workflow Examples

### Example 1: Package Search and Selection

```bash
# 1. Set travel dates
npm run travel -- set-dates 2026-02-24 2026-02-28

# 2. Search packages
npm run travel -- search-packages --type fit

# 3. Review offers
npm run view:status

# 4. Select package
npm run travel -- select-offer besttour_TYO05MM260224AM 2026-02-24

# 5. Verify P3/P4 populated
npm run view:transport
```

### Example 2: Budget-Constrained Search

```bash
# Search with budget limit
npm run travel -- search-packages --max-price 20000 --type fit

# Filter scraped results
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

- **travel-plan.json**: 
  - Updates `process_3_4_packages.results.offers`
  - On selection: populates `process_3_transportation` and `process_4_accommodation`
- **state.json**: 
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
- **Required files**: `data/ota-sources.json`

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

## Shared References

- `../travel-shared/references/io-contracts.md`
- `../travel-shared/references/state-manager.md`
- `../travel-shared/references/cascade-triggers.md`
- `../scrape-ota/SKILL.md` - OTA scraping details

## DB Integration

After package selection, bookings are automatically synced to Turso:
- `StateManager.save()` writes to both JSON and DB
- Query bookings: `npm run travel -- query-bookings --category package`
- The agent should use `query-bookings` to check status, not read JSON paths
- Manual sync: `npm run travel -- sync-bookings`

## Notes

- Package selection automatically populates P3 and P4 (cascade)
- Use this skill for bundled deals; use `/p3-flights` + `/p4-hotels` for separate bookings
- Package offers include `date_pricing` for availability tracking
- StateManager handles offer availability updates via `updateOfferAvailability()`
