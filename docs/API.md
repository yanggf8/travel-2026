# Travel Skill Pack API Reference

## Overview

This skill pack provides a complete framework for travel planning automation:

- **StateManager**: Unified state management with dirty flags and event logging
- **Scrapers**: Extensible OTA scraper framework with canonical offer format
- **Validators**: Itinerary validation for time conflicts, business hours, etc.
- **CLI Operations**: Rich set of CLI commands with discoverable contracts

## Quick Start

```bash
# Install dependencies
npm install

# Initialize a new trip
npx ts-node src/templates/project-init.ts --dest tokyo_2026 --start 2026-04-01 --end 2026-04-05

# View status
npm run view:status

# Validate itinerary
npm run travel -- validate-itinerary
```

---

## StateManager API

The `StateManager` class is the central hub for all state operations.

### Import

```typescript
import { StateManager } from './src/state';
```

### Constructor

```typescript
// File-based (default)
const sm = new StateManager();
const sm = new StateManager('data/travel-plan.json', 'data/state.json');

// Options-based (for testing)
const sm = new StateManager({
  planPath: 'data/travel-plan.json',
  statePath: 'data/state.json',
  skipSave: true,  // Don't write files
});

// In-memory (for testing)
const sm = new StateManager({
  plan: { /* TravelPlanMinimal */ },
  state: { /* TravelState */ },
  skipSave: true,
});
```

### Core Methods

#### Dates

```typescript
// Set travel dates (triggers cascade)
sm.setDateAnchor('2026-02-13', '2026-02-17', 'User requested');

// Get current dates
const dates = sm.getDateAnchor();
// { start: '2026-02-13', end: '2026-02-17', days: 5 }
```

#### Process Status

```typescript
// Set status
sm.setProcessStatus('tokyo_2026', 'process_3_4_packages', 'selected');

// Get status
const status = sm.getProcessStatus('tokyo_2026', 'process_3_4_packages');
// 'selected'
```

#### Dirty Flags

```typescript
// Mark process as needing update
sm.markDirty('tokyo_2026', 'process_5_daily_itinerary');

// Check if dirty
if (sm.isDirty('tokyo_2026', 'process_5_daily_itinerary')) {
  // Regenerate itinerary
}

// Clear after handling
sm.clearDirty('tokyo_2026', 'process_5_daily_itinerary');
```

#### Offers

```typescript
// Import scraped offers
sm.importPackageOffers('tokyo_2026', 'besttour', offers, 'Scraped via script');

// Select an offer
sm.selectOffer('besttour_TYO05MM260211AM', '2026-02-13', true);

// Update availability
sm.updateOfferAvailability('besttour_TYO05MM260211AM', '2026-02-13', 'sold_out');
```

#### Itinerary

```typescript
// Create day skeletons
sm.scaffoldItinerary('tokyo_2026', 5);

// Add activity
const actId = sm.addActivity('tokyo_2026', 2, 'morning', {
  title: 'teamLab Borderless',
  poi_id: 'teamlab_borderless',
  duration_min: 180,
  booking_required: true,
});

// Set booking status
sm.setActivityBookingStatus('tokyo_2026', 2, 'morning', 'teamLab Borderless', 'booked', 'TLB-12345');

// Set times
sm.setActivityTime('tokyo_2026', 2, 'morning', 'teamLab Borderless', {
  startTime: '10:00',
  endTime: '13:00',
  isFixedTime: true,
});
```

#### Persistence

```typescript
// Save both files
sm.save();

// Get plan object (for inspection)
const plan = sm.getPlan();
```

---

## Scraper Framework

### Types

```typescript
import {
  IOtaScraper,
  OtaSearchParams,
  ScrapeResult,
  CanonicalOffer,
} from './src/scrapers';
```

### Creating a Scraper

```typescript
import { BaseScraper } from './src/scrapers';

class MyOtaScraper extends BaseScraper {
  constructor() {
    super('myota'); // Must match source_id in ota-sources.json
  }

  async search(params: OtaSearchParams): Promise<ScrapeResult> {
    const startTime = Date.now();

    try {
      // Implement search logic
      const offers = await this.fetchOffers(params);

      return this.createSuccessResult(params, offers, startTime);
    } catch (err) {
      return this.createErrorResult(params, [err.message], startTime);
    }
  }

  async scrapeProduct(url: string): Promise<ScrapeResult> {
    // Implement single product scrape
  }
}
```

### Registering Scrapers

```typescript
import { globalRegistry, MyOtaScraper } from './src/scrapers';

globalRegistry.register(new MyOtaScraper());
```

### Searching

```typescript
const results = await globalRegistry.searchAll({
  destination: 'tokyo_2026',
  startDate: '2026-02-13',
  endDate: '2026-02-17',
  pax: 2,
});
```

### Canonical Offer Format

All offers are normalized to this format (source of truth: `src/state/schemas.ts` → `OfferSchema`):

```typescript
type Availability = 'available' | 'sold_out' | 'limited' | 'unknown';
type PackageSubtype = 'fit' | 'group' | 'semi_fit' | 'unknown';

interface CanonicalOffer {
  // ── Identity ──
  id: string;                    // {source_id}_{product_code}
  source_id: string;
  product_code?: string;
  url?: string;
  scraped_at?: string;           // ISO-8601

  // ── Classification ──
  type: 'package' | 'flight' | 'hotel' | 'activity';
  package_subtype?: PackageSubtype; // FIT vs group distinction
  guided?: boolean;                 // Has tour guide/leader
  meals_included?: number;          // Number of meals included
  duration_days?: number;

  // ── Pricing ──
  currency: string;              // default TWD for TW-market OTAs
  price_per_person: number;
  price_total?: number;

  // ── Availability ──
  availability: Availability;
  seats_remaining?: number | null;

  // ── Baggage ──
  baggage_included?: boolean | null;
  baggage_kg?: number | null;

  // ── Components ──
  flight?: {
    airline: string;
    airline_code?: string;
    outbound: FlightLeg;
    return?: FlightLeg;
  };

  hotel?: {
    name: string;
    area: string;
    area_type?: 'central' | 'airport' | 'suburb' | 'unknown';
    star_rating?: number;
    access?: string[];
  };

  includes?: string[];

  date_pricing?: Record<string, DatePricingEntry> | null;

  best_value?: {
    date: string;
    price_per_person: number;
    price_total: number;
  };

  // ── Evaluation ──
  pros?: string[];
  cons?: string[];
  note?: string;
}
```

#### Package Subtype Classification

| Subtype | Description | Keywords |
|---------|-------------|----------|
| `fit` | Free Independent Travel (機加酒) | 自由行, 機加酒, 自助 |
| `group` | Guided group tour (跟團) | 團體, 跟團, 領隊, 導遊 |
| `semi_fit` | Hybrid with free days (伴自由) | 半自由, 伴自由, 自由時間 |
| `unknown` | Cannot determine | — |

#### Hotel Area Type

| Value | Description |
|-------|-------------|
| `central` | City centre / main station area |
| `airport` | Airport-adjacent hotel |
| `suburb` | Suburban or outlying area |
| `unknown` | Cannot determine |

---

## Itinerary Validator

### Usage

```typescript
import { ItineraryValidator, DaySummary } from './src/validation';

const validator = new ItineraryValidator({
  minTransitMinutes: 30,
  bookingWarningDays: 7,
  maxActivitiesPerSession: 3,
  maxHoursPerDay: 12,
  checkAreaEfficiency: true,
  checkBusinessHours: true,
});

const result = validator.validate(days);

if (!result.valid) {
  console.log('Errors:', result.summary.errors);
  for (const issue of result.issues) {
    console.log(`[${issue.severity}] ${issue.message}`);
    if (issue.suggestion) {
      console.log(`  Suggestion: ${issue.suggestion}`);
    }
  }
}
```

### Issue Categories

| Category | Description |
|----------|-------------|
| `time_conflict` | Overlapping activity times |
| `unrealistic_timing` | Not enough time for activity |
| `transport_gap` | Missing transit between areas |
| `business_hours` | Activity outside operating hours |
| `booking_deadline` | Deadline approaching or passed |
| `overcrowded_day` | Too many activities |
| `underpacked_day` | No activities planned |
| `area_inefficiency` | Back-and-forth travel |
| `logical_order` | Session/time mismatch |

---

## Configuration Discovery

### Destinations

```typescript
import {
  getAvailableDestinations,
  getDestinationConfig,
  resolveDestinationRefPath,
} from './src/config/loader';

// List all destinations
const destinations = getAvailableDestinations();
// ['tokyo_2026', 'nagoya_2026', 'osaka_2026']

// Get config
const config = getDestinationConfig('tokyo_2026');
// { slug, display_name, timezone, currency, primary_airports, ... }

// Get POI reference file path
const refPath = resolveDestinationRefPath('tokyo_2026');
// '/path/to/src/skills/travel-shared/references/destinations/tokyo.json'
```

### OTA Sources

```typescript
import {
  getAvailableOtaSources,
  getSupportedOtaSources,
  getOtaSourceConfig,
} from './src/config/loader';

// All sources
const sources = getAvailableOtaSources();

// Only with working scrapers
const supported = getSupportedOtaSources();

// Get config
const config = getOtaSourceConfig('besttour');
// { source_id, display_name, base_url, currency, scraper_script, ... }
```

---

## CLI Operations

All CLI operations are documented in `src/contracts/skill-contracts.ts`.

### List Operations

```typescript
import { listSkills, getSkillContract } from './src/contracts/skill-contracts';

// Get all operation names
const skills = listSkills();

// Get contract for an operation
const contract = getSkillContract('set-dates');
console.log(contract.description);
console.log(contract.args);
console.log(contract.example);
```

### Common Commands

```bash
# View status
npm run view:status

# Set dates
npm run travel -- set-dates 2026-02-13 2026-02-17

# Select offer
npm run travel -- select-offer besttour_TYO05MM260211AM 2026-02-13

# Validate itinerary
npm run travel -- validate-itinerary

# Set activity booking
npm run travel -- set-activity-booking 2 morning "teamLab Borderless" booked --ref "TLB-12345"
```

---

## Error Handling

The skill pack uses a `Result<T, E>` type for operations that can fail:

```typescript
import { Result, ok, err, isOk, isErr, unwrap } from './src/types/result';

function divide(a: number, b: number): Result<number, string> {
  if (b === 0) return err('Division by zero');
  return ok(a / b);
}

const result = divide(10, 2);
if (isOk(result)) {
  console.log(unwrap(result)); // 5
}
```

---

## Extending the Skill Pack

### Adding a New Destination

1. Add entry to `data/destinations.json`:
```json
{
  "kyoto_2026": {
    "slug": "kyoto_2026",
    "display_name": "Kyoto",
    "ref_id": "kyoto",
    "ref_path": "src/skills/travel-shared/references/destinations/kyoto.json",
    "timezone": "Asia/Tokyo",
    "currency": "JPY",
    "markets": ["TW", "JP"],
    "primary_airports": ["KIX", "ITM"],
    "language": "ja"
  }
}
```

2. Create reference file from template:
```bash
cp src/templates/destination-template.json \
   src/skills/travel-shared/references/destinations/kyoto.json
```

3. Populate POIs, areas, and clusters in the reference file.

### Adding a New OTA

1. Add entry to `data/ota-sources.json`:
```json
{
  "klook": {
    "source_id": "klook",
    "display_name": "Klook",
    "types": ["package", "activity"],
    "base_url": "https://www.klook.com",
    "markets": ["TW", "HK", "SG"],
    "currency": "TWD",
    "supported": true,
    "scraper_script": "scripts/scrape_klook.py"
  }
}
```

2. Create scraper class extending `BaseScraper`.

3. Register with `globalRegistry`.

---

## Version History

| Version | Changes |
|---------|---------|
| 1.3.0 | Updated CanonicalOffer format: added `package_subtype`, `guided`, `meals_included`, `baggage_included`, `baggage_kg`, `hotel.area_type`; added `activity` offer type; aligned field names with Zod schema |
| 1.2.0 | Added itinerary validation and scraper registry |
| 1.1.0 | Added configuration discovery APIs |
| 1.0.0 | Initial release with StateManager, CLI, basic scrapers |
