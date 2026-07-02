# OTA Registry (Shared)

Source of truth: `ota_sources` table in Turso (global, not plan-scoped). `data/travel-plan.json` is deleted.

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

## Capture Tool (gwebcdb + agent `ota write-offers`)

The Python/Playwright scrapers are **decommissioned** (archived under
`archive/broken-python-scrapers/`). OTA capture now runs through gwebcdb on WSLg
(`~/b/gwebcdb`), which drives the page and writes raw page text to Turso `captures`.
The agent reads `captures.raw_text`, emits TSV, then persists normalized Turso `offers`
with `travel ota write-offers`.

**Usage** (full flow in `/scrape-ota`):
```bash
# Drive + capture an OTA page in ~/b/gwebcdb (clicks/fills the actual UI — no URL templates)
python bridge/navigate.py "<url>"
python bridge/ota_capture.py --source <source_id> [--url-contains <substr>]   # → capture_id

# Agent-extract TSV from captures.raw_text, then persist Turso offers
./rust/target/debug/travel ota write-offers <job_id> --capture <capture_id> --claim-token <token> --tsv <path>
```

**BestTour page structure:**
- `交通方式` → 去程 (outbound) / 回程 (return) flights
- `住宿` → Hotel name, area, amenities
- `價格` → Per-person pricing, calendar availability

**Known limitations:**
- Pages are JS-rendered; gwebcdb attaches to WSLg Chrome to render and capture them
- Return flight may need manual extraction from raw_text
- Date-specific pricing requires calendar interaction (drive via `--step` clicks)

## Normalization expectations

- Each scraper maps raw offers to `CanonicalOffer` consistently.
- `id` stays stable across runs for the same `product_code`.
- Capture scrape metadata in `provenance[]` even on partial failure.

