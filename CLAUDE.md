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
├── CLAUDE.md
├── data/
│   ├── travel-plan.json       # v4.2.0 destination-scoped
│   ├── state.json             # Event-driven state
│   ├── flights-cache.json     # Legacy flight cache
│   ├── liontravel-*.json      # Lion Travel scrape results
│   └── tigerair-*.json        # Tigerair scrape results
├── src/
│   ├── cascade/               # Cascade runner library
│   │   ├── types.ts           # Type definitions
│   │   ├── wildcard.ts        # Schema-driven expansion
│   │   ├── runner.ts          # Core logic
│   │   └── index.ts           # Module exports
│   ├── cli/
│   │   └── cascade.ts         # Cascade CLI
│   ├── status/
│   ├── process/
│   ├── questionnaire/definitions/
│   └── skills/
│       ├── p3-flights.md
│       └── p3p4-packages.md
├── scripts/
│   └── scrape_package.py      # Playwright OTA scraper
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
| P3+4 Packages | 🔄 researched (4 offers) | ⏳ pending |
| P3 Transportation | ⏳ pending | 🔄 researched |
| P4 Accommodation | ⏳ pending | ⏳ pending |
| P5 Itinerary | ⏳ pending | ⏳ pending |

### Tokyo Package Offers (2 pax)
| Source | Price | Type | Note |
|--------|-------|------|------|
| Lion Travel | TWD 19,560起 | Package | Kawaguchiko area |
| Lion Travel | TWD 19,860起 | Package | Skytree + 24hr metro |
| Lion Travel | TWD 29,776起 | Package | Disney + ticket |
| Besttour | TWD 36,776 | Package | Feb 22, Hamamatsucho |

## Completed
- ✅ Cascade runner (TypeScript library + CLI)
- ✅ Lion Travel OTA integration
- ✅ Tigerair OTA integration (limited - no date-specific pricing)
- ✅ Canonical offer schema normalization

## Next Steps
1. **Add eztravel scraper** - Normalize to canonical_offer_schema
2. **Build comparison tool** - Derive rankings from destinations/*
3. **Package selection flow** - Select offer and trigger cascade populate
