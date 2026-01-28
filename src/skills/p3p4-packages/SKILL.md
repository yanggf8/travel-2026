---
name: p3p4-packages
description: Search and compare OTA flight+hotel packages, write normalized offers to travel-plan.json, and trigger cascade population on selection.
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

## Scraper Tools

Use Python/Playwright scrapers to fetch package data:

```bash
# Scrape BestTour package
python scripts/scrape_package.py "https://www.besttour.com.tw/itinerary/<CODE>" data/besttour-<CODE>.json

# Scrape Lion Travel
python scripts/scrape_liontravel_dated.py --start YYYY-MM-DD --end YYYY-MM-DD data/liontravel.json
```

After scraping, normalize to `CanonicalOffer` and update `travel-plan.json`.

## Selection workflow

1. Scrape offers → normalize to `CanonicalOffer`
2. Write to `process_3_4_packages.results.offers`
3. User selects: `sm.selectOffer(offerId, date)`
4. Cascade populates P3 (transport) and P4 (accommodation)

## Legacy spec

Full spec (historical, detailed): `references/legacy-spec.md`

