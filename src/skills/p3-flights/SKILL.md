---
name: p3-flights
description: Search and compare flight-only options (standalone), writing candidates into the P3 transportation process for the active destination.
version: 1.1.0
requires_skills: [travel-shared, scrape-ota]
requires_processes: [process_1_date_anchor, process_2_destination]
provides_processes: [process_3_transportation]
---

# /p3-flights

> **⚠️ Python scrapers DECOMMISSIONED** (archived in `archive/broken-python-scrapers/`; constructed
> URLs 404). Do NOT run `python scripts/scrape_*.py`. Get flight data via the Rust CDP driver:
> `./rust/target/debug/chromeport fetch interact "<url>" --source <id> --step ...` (or
> `browser snapshot`) → `parse capture <id> --source <id>`. NOTE: flight-only sources currently
> need a flight rule shape (parser_rules has_custom_parser=1). Python commands below are historical.

Search and compare standalone flight options for P3 (transportation).

## Shared references

Read first unless request is extremely narrow:
- `../travel-shared/references/io-contracts.md`
- `../travel-shared/references/date-filters.md`
- `../travel-shared/references/state-manager.md`
- `../travel-shared/references/cascade-triggers.md`
- `references/legacy-spec.md` — detailed I/O contract examples and airport tables (read when matching exact interface shapes)

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

Writes via StateManager (DB-backed, no direct JSON):
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
# Capture flight prices via the chromeport CDP driver (Python scrapers decommissioned — see /scrape-ota)
#   ./rust/target/debug/chromeport fetch interact "<trip.com url>" --source trip --step ...
#   ./rust/target/debug/chromeport parse capture <id> --source trip
# (Tigerair: same flow with --source tigerair)

# Normalize and rank flight results
./bin/travel normalize flights scrapes/trip-feb24-out.json --top 5

# Manually record selected flight
./bin/travel set-flight outbound --dest <slug> \
  --flight <number> --airline "<name>" --airline-code <code> \
  --from <IATA> --dep-terminal <T> --dep HH:MM \
  --to <IATA> --arr-terminal <T> --arr HH:MM --date YYYY-MM-DD
./bin/travel set-flight return --dest <slug> [same flags]
```

## Workflow Examples

### Example 1: Basic Flight Search

```bash
# 1. Ensure dates are set (P1)
./bin/travel set-dates 2026-02-24 2026-02-28

# 2. Capture flights from Trip.com via the chromeport CDP driver (see /scrape-ota)
#   ./rust/target/debug/chromeport fetch interact "<trip.com url>" --source trip --step ...
#   ./rust/target/debug/chromeport parse capture <id> --source trip

# 3. Review ranked results
./bin/travel normalize flights scrapes/date-range-prices.json --top 5

# 4. Record selected flight in DB
./bin/travel set-flight outbound --dest kyoto_2026 \
  --flight SL396 --airline "Thai Lion Air" --airline-code SL \
  --from TPE --dep-terminal T1 --dep 09:00 \
  --to KIX --arr-terminal T1 --arr 12:30 --date 2026-02-24
./bin/travel set-flight return --dest kyoto_2026 \
  --flight SL397 --airline "Thai Lion Air" --airline-code SL \
  --from KIX --dep-terminal T1 --dep 13:30 \
  --to TPE --arr-terminal T1 --arr 15:40 --date 2026-02-28

# 5. Verify
./bin/travel transport
```

### Example 2: Tigerair LCC Search

```bash
# Capture Tigerair prices via the chromeport CDP driver (see /scrape-ota)
#   ./rust/target/debug/chromeport fetch interact "<tigerair url>" --source tigerair --step ...
#   ./rust/target/debug/chromeport parse capture <id> --source tigerair

# Review output, then record best option
./bin/travel set-flight outbound --dest <slug> --flight IT201 --airline "Tigerair" ...
```

## Error Handling

| Error | Cause | Solution |
|-------|-------|----------|
| `P1 not confirmed` | Dates not set | Run `set-dates` first |
| `No flights found` | No availability or filters too strict | Adjust dates or budget |
| `Scraper failed` | OTA site changed | Check scraper logs, update scraper |

## State Changes

- **StateManager**: Updates `process_3_transportation.flight.candidates`
- **Event log**: Emits `flight_candidates_added` event
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

## Notes

- Flight search is separate from package search (P3+4)
- Use this skill when booking flights independently
- For package deals (flight + hotel), use `/p3p4-packages` instead
