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

## Agent-First Workflow

Default mode for this repo is **agent-first**:

- The agent proactively runs the next logical step (scrape → normalize → write → select → cascade) and only asks the user when a preference materially changes the result (dates/budget/constraints/which offer).
- Prefer calling `StateManager` methods (or the CLI wrappers) over direct JSON edits, so `travel-plan.json` and `state.json` stay consistent and the audit trail stays accurate.
- Treat schema as canonical and migrate/normalize legacy shapes on load where needed; avoid duplicating path strings in multiple places.
- Every agent output should include: current status, what changed, and the single best “next action”.

## Separate Trips (Multi-Plan)

Multi-destination is handled inside one plan via `destinations.*`. For a truly separate trip (e.g., “second trip of 2026”), use separate files:

- `data/trips/<trip-id>/travel-plan.json`
- `data/trips/<trip-id>/state.json`

Tooling:

- `travel-update` supports `--plan` + `--state` (or set `$TRAVEL_PLAN_PATH` / `$TRAVEL_STATE_PATH`).
- Most other commands already support `--file` / `--input` pointing at the desired `travel-plan.json`.

Example:

```bash
npx ts-node src/cli/travel-update.ts status --plan data/trips/japan-2026-2/travel-plan.json --state data/trips/japan-2026-2/state.json
```

### Available
| Skill | Path | Purpose |
|-------|------|---------|
| `travel-shared` | `src/skills/travel-shared/SKILL.md` | Shared references used by all travel skills |
| `/p3-flights` | `src/skills/p3-flights/SKILL.md` | Search flights separately |
| `/p3p4-packages` | `src/skills/p3p4-packages/SKILL.md` | Search OTA packages (flight+hotel) |

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
| `liontravel` | 雄獅旅遊 | package, flight, hotel | ❌ |
| `tigerair` | 台灣虎航 | flight | ✅ (limited) |
| `eztravel` | 易遊網 | package, flight, hotel | ❌ |

### Lion Travel Promo
- Code: `FITPKG` - TWD 400 off on Thursdays (min TWD 20,000)

## Project Structure
```
/
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
│   │   ├── p3p4-test.ts       # Package skill test CLI
│   │   └── travel-update.ts   # Travel plan update CLI
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
│   │   ├── travel-shared/     # Shared references (bundle)
│   │   │   ├── SKILL.md
│   │   │   └── references/
│   │   ├── p3-flights/
│   │   │   ├── SKILL.md
│   │   │   └── references/legacy-spec.md
│   │   └── p3p4-packages/
│   │       ├── SKILL.md
│   │       └── references/legacy-spec.md
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
| P1 Dates | ✅ confirmed (Feb 13-17) | ✅ confirmed |
| P2 Destination | ✅ confirmed | ✅ confirmed |
| P3+4 Packages | ✅ **selected** | ⏳ pending (archived) |
| P3 Transportation | 📦 populated (from package) | 🔄 researched |
| P4 Accommodation | 📦 populated (from package) | ⏳ pending |
| P5 Itinerary | ⏳ pending | ⏳ pending |

### ✅ BOOKED: Tokyo Feb 13-17, 2026
```
Package: besttour_TYO06MM260213AM2
Dates:   Fri Feb 13 → Tue Feb 17 (5 days)
Price:   TWD 27,888/person (TWD 55,776 for 2 pax)

Flight (red-eye both ways):
  去程: MM620 TPE 02:25 → NRT 06:30 (Feb 13)
  回程: MM627 NRT 22:05 → TPE 01:25+1 (Feb 17→18)

Hotel:   TAVINOS Hamamatsucho
         Area: Shimbashi / Hamamatsucho
         Access: JR Hamamatsucho 8min, Yurikamome Takeshiba 1min
         Includes: Light breakfast
```

### CLI Quick Reference
```bash
# View status
npx ts-node src/cli/travel-update.ts status

# View full booking details
npx ts-node src/cli/travel-update.ts status --full

# Update dates (triggers cascade)
npx ts-node src/cli/travel-update.ts set-dates 2026-02-13 2026-02-17

# Select an offer
npx ts-node src/cli/travel-update.ts select-offer <offer-id> <date>
```

### Scraper Tools (Python/Playwright)

| Script | Purpose | OTA |
|--------|---------|-----|
| `scripts/scrape_package.py` | Generic package scraper | BestTour, any OTA |
| `scripts/scrape_liontravel_dated.py` | Date-specific pricing | Lion Travel |

**Requirements:**
```bash
pip install playwright
playwright install chromium
```

**Usage:**
```bash
# Scrape BestTour package
python scripts/scrape_package.py "https://www.besttour.com.tw/itinerary/<CODE>" data/besttour-<CODE>.json

# Scrape Lion Travel with dates
python scripts/scrape_liontravel_dated.py --start 2026-02-13 --end 2026-02-17 data/liontravel-search.json
```

**Output:** Raw text + extracted elements saved to JSON. Manual parsing may be needed for:
- 交通方式 (flights): 去程/回程 sections
- 住宿 (hotel): name, area, amenities
- 價格 (price): per-person and total

## Completed
- ✅ Cascade runner (TypeScript library + CLI)
- ✅ Lion Travel OTA integration
- ✅ Tigerair OTA integration (limited - no date-specific pricing)
- ✅ Canonical offer schema normalization
- ✅ BestTour date-specific pricing scraper (full Feb 2026 calendar)
- ✅ Lion Travel dated search scraper (`scripts/scrape_liontravel_dated.py`)
- ✅ StateManager with type-safe ProcessId/ProcessStatus
- ✅ Plan normalization for legacy schema migration
- ✅ Travel Update CLI (`src/cli/travel-update.ts`)
- ✅ Tokyo package selected (Feb 13, BestTour)

## Next Steps
1. **Plan daily itinerary** - P5 for Tokyo (5 days)
2. **Build comparison tool** - Derive rankings from destinations/*
