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

## Capture Tool (chromeport CDP driver)

The Python/Playwright scrapers are **decommissioned** (archived under
`archive/broken-python-scrapers/`). OTA capture now runs through the chromeport CDP driver
(`rust/crates/chromeport`), which attaches to a real Chrome, drives the page, and writes
captures → Turso `captures`, then rule-parses them (`parser_rules`) → Turso `offers`.

**Usage** (full flow in `/scrape-ota`):
```bash
# Drive + capture an OTA page (clicks/fills the actual UI — no URL templates)
./rust/target/debug/chromeport fetch interact "<url>" --source <source_id> --step 'click:SEL' --step 'fill:SEL=VALUE'

# Parse the capture (rule-driven via parser_rules) → Turso offers
./rust/target/debug/chromeport parse capture <capture-id> --source <source_id>
```

**BestTour page structure:**
- `交通方式` → 去程 (outbound) / 回程 (return) flights
- `住宿` → Hotel name, area, amenities
- `價格` → Per-person pricing, calendar availability

**Known limitations:**
- Pages are JS-rendered; the chromeport CDP driver attaches to a real Chrome to render them
- Return flight may need manual extraction from raw_text
- Date-specific pricing requires calendar interaction (drive via `--step` clicks)

## Normalization expectations

- Each scraper maps raw offers to `CanonicalOffer` consistently.
- `id` stays stable across runs for the same `product_code`.
- Capture scrape metadata in `provenance[]` even on partial failure.

