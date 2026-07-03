# Extending the Travel Skill Pack

> ⚠️ **Legacy — partial accuracy.** This guide was written when destinations and OTAs lived in `data/destinations.json` / `data/ota-sources.json`. Those files no longer exist — both live in Turso tables (`destination_config`, `ota_sources`). The reference-file/scraper-class/validator/CLI sections still apply, but the "Step 1: Register in destinations.json" / "Register in ota-sources.json" steps do not.
>
> For new destinations, use the `/new-destination` skill (`src/skills/new-destination/SKILL.md`). For new OTA sources, insert a row into the `ota_sources` table — the table is described under **`## Turso DB`** in CLAUDE.md (search for `ota_sources`). The rest of this guide (reference files, scraper subclassing, validators, CLI command structure) is still current.

This guide explains how to extend the skill pack for new destinations, OTAs, and custom validation rules.

## Table of Contents

1. [Adding New Destinations](#adding-new-destinations)
2. [Adding New OTA Scrapers](#adding-new-ota-scrapers)
3. [Custom Validation Rules](#custom-validation-rules)
4. [Creating New CLI Commands](#creating-new-cli-commands)

---

## Adding New Destinations

### Step 1: Register in destinations.json

Add an entry to `data/destinations.json`:

```json
{
  "kyoto_2026": {
    "slug": "kyoto_2026",
    "display_name": "Kyoto",
    "ref_id": "kyoto",
    "ref_path": "",
    "timezone": "Asia/Tokyo",
    "currency": "JPY",
    "markets": ["TW", "JP"],
    "primary_airports": ["KIX", "ITM"],
    "language": "ja"
  }
}
```

### Step 2: Seed Reference Data into Turso

Reference data lives in normalized Turso tables (`destination_areas`,
`destination_pois`, `destination_clusters`, `destination_transit`, and
`destination_config.tips_json`) — **never a local JSON file**. Add the new
destination to the inline `DATA` constant in `scripts/seed-destination-refs.ts`
(keyed by slug), then run:

```bash
# (seed-destination-refs.ts retired; see archive/ts-cli-retired/)
./bin/travel query-destination-ref --slug kyoto_2026   # verify
```

### Step 3: Reference Data Shape

The reference data per destination (each object below = one table row):

#### Areas
Neighborhoods/districts with transport stations:

```json
{
  "areas": [
    {
      "id": "gion",
      "name": "Gion",
      "type": "traditional",
      "stations": ["Gion-Shijo", "Kawaramachi"],
      "vibe": "Geisha district, traditional architecture",
      "best_for": ["culture", "photography", "dining"]
    }
  ]
}
```

#### POIs (Points of Interest)
Individual attractions:

```json
{
  "pois": [
    {
      "id": "kinkakuji",
      "title": "Kinkaku-ji (Golden Pavilion)",
      "area": "kinkakuji_area",
      "nearest_station": "Kinkakuji-michi Bus Stop",
      "duration_min": 60,
      "booking_required": false,
      "cost_estimate": 500,
      "tags": ["temple", "unesco", "photography"],
      "notes": "Best early morning to avoid crowds",
      "hours": "09:00-17:00",
      "address": "1 Kinkakujicho, Kita Ward, Kyoto"
    }
  ]
}
```

#### Clusters
Logical groupings for trip planning:

```json
{
  "clusters": [
    {
      "id": "eastern_temples",
      "name": "Eastern Kyoto Temples",
      "theme": "culture",
      "areas": ["gion", "higashiyama"],
      "pois": ["kiyomizudera", "fushimi_inari", "sanjusangendo"],
      "half_day": false,
      "notes": "Full day recommended. Start early at Fushimi Inari."
    }
  ]
}
```

### Step 4: Initialize a Trip

> ⚠️ **Legacy.** `project-init.ts` writes the obsolete JSON plan/state files. Use the Shaping Stage → adopt flow instead, which seeds a Turso-backed plan from research:
>
> ```bash
> # 1. Seed pending flight-scrape attempts for the date/destination matrix
> ./bin/travel shaping-init --origin TPE --start 2026-04-01 --end 2026-04-05 \
>   --dest KIX:"Kyoto via KIX" --nights 4
>
> # 2. Capture offers via gwebcdb (WSLg), agent-extract, then import candidates
> cd ~/b/gwebcdb && ./scripts/start-chrome-cdp-wslg.sh && python bridge/navigate.py "<url>"
> → python bridge/ota_capture.py --source <id>   # → capture_id
> → AGENT reads captures.raw_text, emits TSV → ./bin/travel ota write-offers <job_id> --capture <capture_id> --claim-token <tok> --tsv <path>
> → ./bin/travel shaping-import --run <run_id> --file <handoff.json>
>
> # 3. Pick a candidate
> ./bin/travel shaping-compare --run <run_id>
>
> # 4. Adopt → creates the plan in Turso, seeds P1 dates + P2 destination
> ./bin/travel shaping-adopt <candidate_id> kyoto-2026 --create-plan --dest kyoto_2026
> ```
>
> The block below is preserved only for historical reference and no longer reflects how plans are created.

```bash
# (project-init.ts retired; see archive/ts-cli-retired/)
#   project-init.ts \
#     --dest kyoto_2026 \
#     --start 2026-04-01 \
#     --end 2026-04-05
```

---

## Adding New OTA Scrapers

### Step 1: Register in ota-sources.json

```json
{
  "klook": {
    "source_id": "klook",
    "display_name": "Klook",
    "display_name_en": "Klook",
    "types": ["activity", "package"],
    "base_url": "https://www.klook.com",
    "markets": ["TW", "HK", "SG", "MY"],
    "currency": "TWD",
    "supported": true,
    "scraper_script": "scripts/scrape_klook.py",
    "rate_limit": {
      "requests_per_minute": 5
    },
    "notes": "Activity tickets and some packages"
  }
}
```

### Step 2: Create Scraper Class

```typescript
// src/scrapers/klook-scraper.ts
import { BaseScraper, OtaSearchParams, ScrapeResult, CanonicalOffer } from './';

export class KlookScraper extends BaseScraper {
  constructor() {
    super('klook'); // Must match source_id
  }

  async search(params: OtaSearchParams): Promise<ScrapeResult> {
    const startTime = Date.now();

    try {
      // Build search URL
      const url = this.buildSearchUrl(params);

      // Use Python scraper via child_process or implement in TS
      const rawData = await this.callPythonScraper(url);

      // Normalize to canonical format
      const offers = this.normalizeOffers(rawData);

      return this.createSuccessResult(params, offers, startTime);
    } catch (err) {
      return this.createErrorResult(params, [(err as Error).message], startTime);
    }
  }

  async scrapeProduct(url: string): Promise<ScrapeResult> {
    const startTime = Date.now();
    // Implement single product scraping
    return this.createSuccessResult({ destination: '', startDate: '', endDate: '', pax: 1 }, [], startTime);
  }

  private buildSearchUrl(params: OtaSearchParams): string {
    // Build Klook-specific URL
    return `${this.config.baseUrl}/search?city=${params.destination}`;
  }

  private normalizeOffers(rawData: any[]): CanonicalOffer[] {
    return rawData.map((item) => ({
      id: this.generateOfferId(item.id),
      sourceId: this.sourceId,
      type: 'package',
      title: item.name,
      url: item.url,
      currency: this.config.currency,
      pricePerPerson: item.price,
      availability: 'available',
      scrapedAt: new Date().toISOString(),
    }));
  }
}
```

### Step 3: Register Scraper

```typescript
// src/scrapers/index.ts (add export)
export * from './klook-scraper';

// In your app initialization
import { globalRegistry, KlookScraper } from './scrapers';
globalRegistry.register(new KlookScraper());
```

### Step 4: Create Python Scraper (Optional)

If using Playwright for complex JavaScript rendering:

```python
# scripts/scrape_klook.py
#!/usr/bin/env python3
"""Klook scraper using Playwright."""

import asyncio
import json
import sys
from playwright.async_api import async_playwright

async def scrape_klook(url: str) -> dict:
    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        page = await browser.new_page()
        await page.goto(url, wait_until="networkidle")
        
        # Extract data
        items = await page.query_selector_all('.activity-card')
        results = []
        
        for item in items:
            title = await item.query_selector('.title')
            price = await item.query_selector('.price')
            results.append({
                'name': await title.inner_text() if title else '',
                'price': await price.inner_text() if price else '',
            })
        
        await browser.close()
        return {'items': results}

if __name__ == '__main__':
    url = sys.argv[1]
    output = sys.argv[2] if len(sys.argv) > 2 else None
    
    result = asyncio.run(scrape_klook(url))
    
    if output:
        with open(output, 'w') as f:
            json.dump(result, f, indent=2, ensure_ascii=False)
    else:
        print(json.dumps(result, indent=2, ensure_ascii=False))
```

---

## Custom Validation Rules

### Adding New Issue Categories

```typescript
// src/validation/types.ts
export type IssueCategory =
  | 'time_conflict'
  | 'weather_risk'        // NEW: Check weather forecasts
  | 'reservation_window'  // NEW: Activity requires advance booking
  // ... existing categories
```

### Creating Custom Validator

```typescript
// src/validation/weather-validator.ts
import { ValidationIssue, DaySummary } from './types';

export function validateWeatherRisks(days: DaySummary[]): ValidationIssue[] {
  const issues: ValidationIssue[] = [];

  for (const day of days) {
    // Check for outdoor activities during rainy season
    const hasOutdoor = day.activities.some(a => 
      a.tags?.includes('outdoor')
    );

    if (hasOutdoor && isRainySeason(day.date)) {
      issues.push({
        severity: 'info',
        category: 'weather_risk',
        day: day.dayNumber,
        message: `Day ${day.dayNumber} has outdoor activities during rainy season`,
        suggestion: 'Have indoor backup plans ready',
      });
    }
  }

  return issues;
}

function isRainySeason(date: string): boolean {
  const month = new Date(date).getMonth() + 1;
  return month >= 6 && month <= 7; // June-July in Japan
}
```

### Extending Validator

```typescript
// src/validation/extended-validator.ts
import { ItineraryValidator } from './itinerary-validator';
import { validateWeatherRisks } from './weather-validator';
import { DaySummary, ItineraryValidationResult } from './types';

export class ExtendedValidator extends ItineraryValidator {
  validate(days: DaySummary[], today?: Date): ItineraryValidationResult {
    const baseResult = super.validate(days, today);

    // Add custom validations
    const weatherIssues = validateWeatherRisks(days);

    return {
      ...baseResult,
      issues: [...baseResult.issues, ...weatherIssues],
      summary: {
        errors: baseResult.issues.filter(i => i.severity === 'error').length,
        warnings: baseResult.issues.filter(i => i.severity === 'warning').length,
        info: baseResult.issues.filter(i => i.severity === 'info').length + weatherIssues.length,
      },
    };
  }
}
```

---

## Creating New CLI Commands

### Step 1: Define Contract

```typescript
// src/contracts/skill-contracts.ts
export const SKILL_CONTRACTS: Record<string, SkillContract> = {
  // ... existing contracts
  
  'compare-offers': {
    name: 'compare-offers',
    description: 'Compare offers across OTAs with price and feature breakdown',
    args: [
      { name: '--dest', type: 'string', required: false, description: 'Destination slug' },
      { name: '--sort', type: 'string', required: false, description: 'Sort by: price|value|airline' },
      { name: '--limit', type: 'number', required: false, description: 'Max results to show' },
    ],
    output: { type: 'string', description: 'Formatted comparison table' },
    mutates: [],
    example: './bin/travel compare-offers --sort price --limit 5',
  },
};
```

### Step 2: Implement Handler

```typescript
// src/cli/travel-update.ts (add case)
case 'compare-offers': {
  const dest = getArg('--dest') || sm.getActiveDestination();
  const sort = getArg('--sort') || 'price';
  const limit = parseInt(getArg('--limit') || '10', 10);

  const offers = sm.getOffers(dest);
  const sorted = sortOffers(offers, sort);
  const display = sorted.slice(0, limit);

  console.log(formatComparisonTable(display));
  break;
}
```

---

## Best Practices

### 1. Always Use StateManager

Never edit `travel-plan.json` directly. Use StateManager methods to ensure:
- Cascade rules are applied
- Dirty flags are set correctly
- Events are logged for audit

### 2. Normalize All Data

Scrapers must output `CanonicalOffer` format. This ensures:
- Consistent data structure across OTAs
- Easy comparison and filtering
- Predictable display formatting

### 3. Document Contracts

Add every new operation to `SKILL_CONTRACTS` so agents can discover:
- Available operations
- Required arguments
- Expected outputs
- Example usage

### 4. Test with skipSave

Use in-memory StateManager for testing:

```typescript
const sm = new StateManager({
  plan: testPlan,
  state: testState,
  skipSave: true,
});
```

### 5. Version Your Changes

Update `CONTRACT_VERSION` when adding new operations:
- MAJOR: Breaking changes
- MINOR: New features (additive)
- PATCH: Bug fixes
