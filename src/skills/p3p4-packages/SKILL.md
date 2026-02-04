---
name: p3p4-packages
description: Search and compare OTA flight+hotel packages, write normalized offers to travel-plan.json, and trigger cascade population on selection.
version: 1.0.0
requires_skills: [travel-shared, scrape-ota]
requires_processes: [process_1_date_anchor, process_2_destination]
provides_processes: [process_3_transportation, process_4_accommodation]
---

# /p3p4-packages

## Shared references

Read these first unless the request is extremely narrow:

- `../travel-shared/references/io-contracts.md`
- `../travel-shared/references/canonical-offer.md`
- `../travel-shared/references/date-filters.md`
- `../travel-shared/references/ota-registry.md` — includes scraper tool docs
- `../travel-shared/references/state-manager.md`
- `../travel-shared/references/cascade-triggers.md`

## Data Acquisition

Use `/scrape-ota` skill for all OTA scraping. See `../scrape-ota/SKILL.md` for:
- Supported OTAs and URL patterns
- Scraper commands per OTA
- Output schema

After scraping, normalize to `CanonicalOffer` and update `travel-plan.json`.

## Selection workflow

1. Scrape offers via `/scrape-ota` → normalize to `CanonicalOffer`
2. Write to `process_3_4_packages.results.offers`
3. Agent recommends the best offer/date (based on constraints + availability)
4. Apply selection via `sm.selectOffer(offerId, date)` (or CLI `select-offer`)
5. Cascade populates P3 (transport) and P4 (accommodation)

## Agent-first output format

When running this skill, default to an agent-first response:

- **What I did**: scraped, normalized, wrote offers into `process_3_4_packages.results.offers`
- **What I recommend**: 1 offer + 1 date, with a short reason
- **What changed**: any status/dirty flag changes
- **Next action**: either `select-offer ...` or proceed to P5

## Legacy spec

Full spec (historical, detailed): `references/legacy-spec.md`
