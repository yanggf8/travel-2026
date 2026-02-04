---
name: p3-flights
description: Search and compare flight-only options (standalone), writing candidates into the P3 transportation process for the active destination.
version: 1.0.0
requires_skills: [travel-shared, scrape-ota]
requires_processes: [process_1_date_anchor, process_2_destination]
provides_processes: [process_3_transportation]
---

# /p3-flights

## Shared references

- `../travel-shared/references/io-contracts.md`
- `../travel-shared/references/date-filters.md`
- `../travel-shared/references/state-manager.md`
- `../travel-shared/references/cascade-triggers.md`

## Agent-First Defaults

- Run the next step and report results; ask only for preferences that change the outcome (dates, budget, constraints).
- Use `StateManager` (status + dirty flags + event log) rather than direct JSON edits.
- End every run with one clear “next action” (select a flight candidate, or proceed to P4/P5).

## Data Acquisition

Use `/scrape-ota` skill for OTA scraping. See `../scrape-ota/SKILL.md` for:
- Supported OTAs and URL patterns
- Scraper commands per OTA
- Output schema

## Workflow

1. Search flights via `/scrape-ota` → normalize candidates
2. Write into `process_3_transportation.flight.candidates`
3. Update `process_3_transportation.status` + `updated_at`
4. If user selects, record selection in P3 and move toward `selected` / `booked`

## Legacy spec

Full spec (historical, detailed): `references/legacy-spec.md`
