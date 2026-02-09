# OTA Registry (Shared)

Source of truth (data): `data/travel-plan.json` → `ota_sources`.

## Expected fields

```ts
interface OtaSourceRegistryEntry {
  source_id: string;
  display_name: string;
  type: Array<'package' | 'flight' | 'hotel' | 'activity'>;
  base_url: string;
  markets: string[];   // e.g. ["TW"]
  currency: string;    // e.g. "TWD"
  rate_limit: { requests_per_minute: number };
  auth_required: boolean;
  supported: boolean;
}
```

## Scraper Tools

Python/Playwright scrapers for fetching OTA data:

| Script | OTA | Output |
|--------|-----|--------|
| `scripts/scrape_package.py` | BestTour, generic | Raw text + elements |
| `scripts/scrape_liontravel_dated.py` | Lion Travel | Date-specific pricing |

**Usage:**
```bash
# Generic package scraper
python scripts/scrape_package.py "<url>" scrapes/<output>.json

# Lion Travel with date range
python scripts/scrape_liontravel_dated.py --start YYYY-MM-DD --end YYYY-MM-DD scrapes/<output>.json
```

**BestTour page structure:**
- `交通方式` → 去程 (outbound) / 回程 (return) flights
- `住宿` → Hotel name, area, amenities
- `價格` → Per-person pricing, calendar availability

**Known limitations:**
- Pages are JS-rendered; requires Playwright
- Return flight may need manual extraction from raw_text
- Date-specific pricing requires calendar interaction

## Normalization expectations

- Each scraper maps raw offers to `CanonicalOffer` consistently.
- `id` stays stable across runs for the same `product_code`.
- Capture scrape metadata in `provenance[]` even on partial failure.

