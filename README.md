# Japan Travel Planning Project

An automated travel planning system for a Tokyo trip in February 2026.

## Quick Start

```bash
# Install dependencies
npm install

# Run cascade checker (dry-run)
npx ts-node src/cli/cascade.ts

# Run cascade checker (apply changes)
npx ts-node src/cli/cascade.ts --apply
```

## Trip Overview

| Field | Value |
|-------|-------|
| **Destination** | Tokyo, Japan |
| **Dates** | Feb 11-15, 2026 (flexible: Feb 21-22 preferred) |
| **Travelers** | 2 adults |
| **Status** | Package research complete |

### Current Best Option

| Package | Price (2 pax) | Date | Status |
|---------|---------------|------|--------|
| BestTour TAVINOS Hamamatsucho | TWD 36,776 | Feb 22 | ✅ Available |

> ⚠️ **Note**: Original dates (Feb 11-13) are **sold out**. Feb 21-22 are recommended alternatives.

## Project Structure

```
├── README.md                 # This file
├── CLAUDE.md                 # AI assistant context & architecture
├── data/
│   ├── travel-plan.json      # Main travel plan (v4.2.0)
│   ├── state.json            # Event-driven state tracking
│   └── *-scrape.json         # OTA scrape results cache
├── src/
│   ├── cascade/              # Cascade rule engine
│   │   ├── runner.ts         # Core cascade logic
│   │   ├── types.ts          # TypeScript definitions
│   │   └── wildcard.ts       # Schema-driven path expansion
│   ├── cli/
│   │   └── cascade.ts        # CLI for cascade operations
│   ├── skills/               # Reusable planning skills
│   │   ├── p3-flights.md     # Flight search skill
│   │   └── p3p4-packages.md  # Package search skill
│   ├── process/              # Process handlers
│   └── status/               # Status checking utilities
├── scripts/
│   ├── scrape_package.py           # Generic OTA scraper (Playwright)
│   └── scrape_liontravel_dated.py  # Lion Travel date-specific scraper
└── tsconfig.json
```

## Architecture

### Process Flow

The travel planning follows a 5-process workflow:

```
P1 Dates (shared) ─────────────────────────────────
        │
        ▼
P2 Destination ────────────────────────────────────
        │
        ├─────────────────┬────────────────────────
        ▼                 ▼
P3+4 Packages        P3 Transport + P4 Hotels
(combined)           (separate)
        │                 │
        └────────┬────────┘
                 ▼
        P5 Daily Itinerary
```

### Cascade Rules

Changes to upstream processes automatically invalidate downstream data:

| Trigger | Resets |
|---------|--------|
| Date change | P3, P4, P5 |
| Destination change | P3, P4, P5 |
| Package selected | Populates P3 + P4 from offer |

## OTA Integrations

| Source | Type | Status |
|--------|------|--------|
| BestTour (喜鴻假期) | Package | ✅ Full calendar pricing |
| Lion Travel (雄獅旅遊) | Package | ✅ Base pricing |
| Tigerair (台灣虎航) | Flight | ⚠️ Limited |
| ezTravel (易遊網) | Package | ❌ Not integrated |

## Scripts

### Scrape OTA Packages

```bash
# Generic scraper
python scripts/scrape_package.py <url> <output.json>

# Lion Travel with date selection
python scripts/scrape_liontravel_dated.py search 2026-02-11 2026-02-15 data/output.json
python scripts/scrape_liontravel_dated.py detail <product_id> 2026-02-11 5 data/output.json
```

### Run Cascade

```bash
# Check for dirty processes (dry-run)
npx ts-node src/cli/cascade.ts

# Apply cascade resets
npx ts-node src/cli/cascade.ts --apply

# Custom input/output files
npx ts-node src/cli/cascade.ts -i data/travel-plan.json -o data/output.json --apply
```

## Data Schema

The project uses schema version `4.2.0` with destination-scoped architecture.

See `CLAUDE.md` for detailed schema documentation.

## License

Private project - not for distribution.
