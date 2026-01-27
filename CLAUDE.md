# Japan Travel Project

## Trip Details
- **Dates**: February 11-15, 2026 (flexible: Feb 21-22 preferred due to CNY pricing)
- **Active Destination**: Tokyo, Japan
- **Archived Destination**: Nagoya, Japan

## Schema Version
- **Current**: `4.2.0`
- **Architecture**: Destination-scoped with canonical offer model

## Project Goals

### Primary Objectives
1. **Status Check Program** - Check and report travel project status
2. **Process Program as Agent Tool** - Automation for travel planning tasks
3. **Claude Skill Conversion** - Reusable travel planning skill

## Architecture (v4.2.0)

### Data Model
```
travel-plan.json
├── schema_version: "4.2.0"
├── active_destination: "tokyo_2026"
├── process_1_date_anchor          # Shared across destinations
├── destinations/
│   ├── tokyo_2026/                # ACTIVE
│   │   ├── process_2_destination
│   │   ├── process_3_4_packages   # Package-first path
│   │   ├── process_3_transportation
│   │   ├── process_4_accommodation
│   │   └── process_5_daily_itinerary
│   └── nagoya_2026/               # ARCHIVED
├── cascade_rules/                 # Machine-checkable rules
├── cascade_state/                 # Per-destination dirty flags
├── canonical_offer_schema/        # All scrapers normalize to this
├── ota_sources/                   # Plugin registry for OTAs
├── skill_io_contracts/            # Standardized IO for skills
└── comparison/                    # DERIVED (regenerate from destinations)
```

### Process Flow
```
┌─────────────────────────────────────────────────────┐
│  P1 Dates (shared)                                  │
└────────────────────┬────────────────────────────────┘
                     │
         ┌───────────┴───────────┐
         ▼                       ▼
   ┌───────────┐          ┌───────────┐
   │ Tokyo     │          │ Nagoya    │
   │ (active)  │          │ (archived)│
   └─────┬─────┘          └───────────┘
         │
    ┌────┴────┐
    ▼         ▼
┌────────┐  ┌────────┐
│Packages│  │Separate│
│ (P3+4) │  │P3 + P4 │
└───┬────┘  └────────┘
    │ populate_on_select
    ▼
┌────────────────┐
│ P5 Itinerary   │
└────────────────┘
```

### Cascade Rules
| Trigger | Reset | Scope |
|---------|-------|-------|
| `active_destination_change` | `process_5_*` | new destination |
| `process_1_date_anchor_change` | `process_3_*`, `process_4_*`, `process_5_*` | all destinations |
| `process_2_destination_change` | `process_3_*`, `process_4_*`, `process_5_*` | current destination |
| `process_3_4_packages_selected` | populate P3+P4 from chosen offer | current destination |

### Canonical Offer Schema (Required Fields)
```typescript
{
  id: string;              // {source_id}_{product_code}
  source_id: string;
  type: 'package' | 'flight' | 'hotel';
  currency: string;
  price_per_person: number;
  availability: 'available' | 'sold_out' | 'limited';
  flight?: { airline, outbound, return };
  hotel?: { name, slug, area, access[] };
  includes?: ['light_breakfast', ...];
  date_pricing: { [date]: { price, availability } };
  best_value: { date, price_per_person, price_total };
}
```

## Skills

### Available
| Skill | Path | Purpose |
|-------|------|---------|
| `/p3-flights` | `src/skills/p3-flights.md` | Search flights separately |
| `/p3p4-packages` | `src/skills/p3p4-packages.md` | Search OTA packages (flight+hotel) |

### Skill IO Contract
```typescript
// Common Input
{
  active_destination: string;
  date_filters: { start_date, end_date, flexible, preferred_dates, avoid_dates };
  pax: number;
  budget: { total_cap, per_person_cap };
  constraints: { avoid_red_eye, prefer_direct, require_breakfast };
}

// Common Output
{
  offers: CanonicalOffer[];
  chosen_offer: CanonicalOffer | null;
  provenance: [{ source_id, scraped_at, offers_found }];
  warnings: string[];
}
```

## OTA Sources (Plugin Registry)

| Source ID | Name | Type | Supported |
|-----------|------|------|-----------|
| `besttour` | 喜鴻假期 | package | ✅ |
| `liontravel` | 雄獅旅遊 | package, flight, hotel | ✅ |
| `tigerair` | 台灣虎航 | flight | ✅ (limited) |
| `eztravel` | 易遊網 | package, flight, hotel | ❌ |

### Lion Travel Promo
- Code: `FITPKG` - TWD 400 off on Thursdays (min TWD 20,000)

## Project Structure
```
/
├── README.md                  # Project overview & quick start
├── CLAUDE.md                  # AI assistant context (this file)
├── data/
│   ├── travel-plan.json       # Main travel plan (v4.2.0)
│   ├── state.json             # Event-driven state tracking
│   ├── besttour-*.json        # BestTour scrape results (date-specific pricing)
│   ├── liontravel-*.json      # Lion Travel scrape results
│   ├── eztravel-*.json        # ezTravel scrape results
│   ├── tigerair-*.json        # Tigerair scrape results
│   └── flights-cache.json     # Legacy flight cache (Nagoya research)
├── src/
│   ├── cascade/               # Cascade runner library
│   │   ├── index.ts           # Module exports
│   │   ├── runner.ts          # Core cascade logic
│   │   ├── types.ts           # TypeScript definitions
│   │   └── wildcard.ts        # Schema-driven path expansion
│   ├── cli/
│   │   ├── cascade.ts         # Cascade CLI
│   │   └── p3p4-test.ts       # Package skill test CLI
│   ├── process/               # Process handlers
│   │   ├── accommodation.ts
│   │   ├── itinerary.ts
│   │   ├── plan-updater.ts
│   │   ├── transportation.ts
│   │   └── types.ts
│   ├── questionnaire/
│   │   └── definitions/
│   │       └── p3-transportation.json
│   ├── skills/                # Reusable planning skills
│   │   ├── p3-flights.md      # Standalone flight search (v1.1.0)
│   │   └── p3p4-packages.md   # Package search (v1.0.0)
│   └── status/
│       ├── rule-evaluator.ts
│       └── status-check.ts
├── scripts/
│   ├── scrape_package.py           # Generic Playwright OTA scraper
│   └── scrape_liontravel_dated.py  # Lion Travel date-specific scraper
└── tsconfig.json
```

### Cascade CLI Usage
```bash
# Dry-run (default)
npx ts-node src/cli/cascade.ts

# Apply changes
npx ts-node src/cli/cascade.ts --apply

# Custom input/output
npx ts-node src/cli/cascade.ts -i data/travel-plan.json --apply -o data/output.json
```

## Current Status

| Process | Tokyo | Nagoya |
|---------|-------|--------|
| P1 Dates | ✅ confirmed | ✅ confirmed |
| P2 Destination | ✅ confirmed | ✅ confirmed |
| P3+4 Packages | 🔄 researched (date-specific pricing) | ⏳ pending (archived) |
| P3 Transportation | ⏳ pending | 🔄 researched |
| P4 Accommodation | ⏳ pending | ⏳ pending |
| P5 Itinerary | ⏳ pending | ⏳ pending |

### ⚠️ UPDATE: Agent offered Feb 13 (Feb 11-12 sold out)
Original dates (Feb 11) sold out. Agent offered **Feb 13** as alternative.

### Tokyo Package - BestTour Date-Specific Pricing (2 pax, updated 2026-01-26)
| Date | Price (TWD) | Availability | Note |
|------|-------------|--------------|------|
| Feb 11 | 42,776 | ❌ Sold Out | Original preferred date |
| Feb 12 | 46,776 | ❌ Sold Out | |
| **Feb 13** | **55,776** | ✅ Available (2) | **★ AGENT OFFERED** |
| Feb 14 | 69,776 | ✅ Available (2) | CNY peak - expensive |
| Feb 20 | 46,776 | ✅ Available (2) | Post-CNY |
| Feb 21 | 39,776 | ✅ Available (2) | Budget option |
| Feb 22 | 36,776 | ✅ Available (2) | Cheapest |
| Feb 24 | 38,776 | ✅ Available (2) | |

### Decision Options
1. **Feb 13** - TWD 55,776/2pax - Closest to original dates, CNY pricing
2. **Feb 22** - TWD 36,776/2pax - Best value, post-CNY

### Lion Travel Base Prices (starting from, 2-3 nights)
| Package | Base Price | Note |
|---------|------------|------|
| 富士山/河口湖 | TWD 9,780起 | Mt. Fuji area |
| 晴空塔+24hr Metro | TWD 9,930起 | Central Tokyo |
| teamLab | TWD 10,800起 | Includes museum ticket |
| Disney | TWD 14,888起 | Includes park ticket |

*Note: Lion Travel prices are base rates. 5-day actual pricing requires booking flow.*

## Completed
- ✅ Cascade runner (TypeScript library + CLI)
- ✅ Lion Travel OTA integration
- ✅ Tigerair OTA integration (limited - no date-specific pricing)
- ✅ Canonical offer schema normalization
- ✅ BestTour date-specific pricing scraper (full Feb 2026 calendar)
- ✅ Lion Travel dated search scraper (`scripts/scrape_liontravel_dated.py`)

## Next Steps
1. **Select travel dates** - Feb 22 is best value (TWD 36,776/2pax), Feb 21 is 2nd best
2. **Book package** - BestTour TAVINOS Hamamatsucho if dates work
3. **Build comparison tool** - Derive rankings from destinations/*
