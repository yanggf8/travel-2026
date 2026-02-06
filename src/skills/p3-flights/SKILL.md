---
name: p3-flights
description: Search and compare flight-only options (standalone), writing candidates into the P3 transportation process for the active destination.
version: 1.1.0
requires_skills: [travel-shared, scrape-ota]
requires_processes: [process_1_date_anchor, process_2_destination]
provides_processes: [process_3_transportation]
---

# /p3-flights

Search and compare standalone flight options for P3 (transportation).

## Input Schema

```typescript
interface FlightSearchInput {
  destination: string;      // Destination slug (e.g., 'tokyo_2026')
  dates: {
    start: string;          // YYYY-MM-DD
    end: string;            // YYYY-MM-DD
  };
  budget?: number;          // Max price per person
  airline?: string;         // Preferred airline
}
```

## Output Schema

Writes to `travel-plan.json`:
```typescript
process_3_transportation: {
  status: 'researched' | 'selected';
  flight: {
    candidates: NormalizedFlight[];
    selected?: NormalizedFlight;
  };
  updated_at: string;
}
```

## CLI Commands

```bash
# Search flights for active destination
npm run travel -- search-flights

# Search with filters
npm run travel -- search-flights --max-price 15000 --airline "Tigerair"
```

## Workflow Examples

### Example 1: Basic Flight Search

```bash
# 1. Ensure dates are set (P1)
npm run travel -- set-dates 2026-02-24 2026-02-28

# 2. Search flights
npm run travel -- search-flights

# 3. Review candidates
npm run view:transport

# 4. Select a flight
npm run travel -- select-flight <flight-id>
```

### Example 2: Budget-Constrained Search

```bash
# Search with budget limit
npm run travel -- search-flights --max-price 12000

# Filter results
npm run travel -- filter-flights --type lcc
```

## Error Handling

| Error | Cause | Solution |
|-------|-------|----------|
| `P1 not confirmed` | Dates not set | Run `set-dates` first |
| `No flights found` | No availability or filters too strict | Adjust dates or budget |
| `Scraper failed` | OTA site changed | Check scraper logs, update scraper |

## State Changes

- **travel-plan.json**: Updates `process_3_transportation.flight.candidates`
- **state.json**: Emits `flight_candidates_added` event
- **Cascade triggers**: Marks P5 dirty if dates change

## Dependencies

- **Required processes**: P1 (dates), P2 (destination)
- **Required skills**: `/scrape-ota` for OTA integration
- **External tools**: Python scrapers in `scripts/`

## Data Acquisition

Use `/scrape-ota` skill for OTA scraping. See `../scrape-ota/SKILL.md` for:
- Supported OTAs and URL patterns
- Scraper commands per OTA
- Output schema

## Shared References

- `../travel-shared/references/io-contracts.md`
- `../travel-shared/references/date-filters.md`
- `../travel-shared/references/state-manager.md`
- `../travel-shared/references/cascade-triggers.md`

## Notes

- Flight search is separate from package search (P3+4)
- Use this skill when booking flights independently
- For package deals (flight + hotel), use `/p3p4-packages` instead
