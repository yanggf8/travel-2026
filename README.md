# Travel Skill Pack

A reusable skill pack for AI-assisted travel planning. Provides StateManager, OTA scrapers, and itinerary validation.

## Features

- **StateManager**: Unified state management with dirty flags, cascade rules, and event logging
- **Scraper Framework**: Extensible OTA scraper with canonical offer format
- **Itinerary Validator**: Time conflict detection, business hours, booking deadlines
- **CLI Operations**: Rich set of commands with discoverable contracts
- **Multi-destination**: Support for multiple destinations in one plan

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

# Run cascade checker
npx ts-node src/cli/cascade.ts --apply
```

## Documentation

- [API Reference](docs/API.md) - Complete API documentation
- [Extension Guide](docs/EXTENDING.md) - How to add destinations, OTAs, and validators
- [CLAUDE.md](CLAUDE.md) - AI assistant context & architecture

## Project Structure

```
├── README.md                 # This file
├── CLAUDE.md                 # AI assistant context & architecture
├── docs/
│   ├── API.md                # API reference documentation
│   └── EXTENDING.md          # Extension guide
├── data/
│   ├── travel-plan.json      # Main travel plan (v4.2.0)
│   ├── state.json            # Event-driven state tracking
│   ├── destinations.json     # Destination configuration
│   ├── ota-sources.json      # OTA source registry
│   └── *-scrape.json         # OTA scrape results cache
├── src/
│   ├── cascade/              # Cascade rule engine
│   ├── cli/                  # CLI commands
│   ├── config/               # Configuration loaders
│   ├── contracts/            # Skill contracts for agent discovery
│   ├── scrapers/             # OTA scraper framework
│   ├── skills/               # Reusable planning skills
│   ├── state/                # StateManager
│   ├── templates/            # Project & destination templates
│   └── validation/           # Itinerary validators
├── scripts/
│   ├── scrape_package.py           # Generic OTA scraper (Playwright)
│   ├── scrape_listings.py          # Listing page scraper (fast metadata)
│   ├── scrape_liontravel_dated.py  # Lion Travel date-specific scraper
│   ├── scrape_date_range.py        # Multi-date flight comparison
│   ├── scrape_tigerair.py          # Tigerair form-based scraper
│   └── filter_packages.py          # Filter scraped packages by criteria
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
| ezTravel (易遊網) | Flight | ✅ Flight search parser |

See [Extension Guide](docs/EXTENDING.md) for adding new OTAs.

## Itinerary Validation

Validate your itinerary for common issues:

```bash
npm run travel -- validate-itinerary
```

Checks for:
- ⏰ Time conflicts between activities
- 🏢 Business hours compliance
- 📅 Booking deadline warnings
- 🗺️ Area efficiency (minimize back-and-forth)
- 📊 Day packing (over/under scheduled)

## CLI Quick Reference

```bash
# Views (read-only)
npm run view:status         # Booking overview
npm run view:itinerary      # Daily plan
npm run view:transport      # Transport summary

# Mutations
npm run travel -- set-dates 2026-02-13 2026-02-17
npm run travel -- select-offer <offer-id> <date>
npm run travel -- validate-itinerary
npm run travel -- set-activity-booking <day> <session> "<activity>" <status>

# Scraping (Python)
python scripts/scrape_listings.py --source besttour --dest kansai
python scripts/scrape_package.py <url> [--refresh]
python scripts/filter_packages.py data/*.json --type fit --date 2026-02-24 --max-price 25000
```

## Tests

This repo uses cost-effective integration/regression tests (no unit test suite).

```bash
npm test
```

## Data Schema

The project uses schema version `4.2.0` with destination-scoped architecture.

See `CLAUDE.md` for detailed schema documentation.

## Storage Decision (DB)

**Decision criteria**
- No native DB installs required on agent machines.
- Strong CLI story for skills (inspect/query/update).
- JS-native integration with existing Node/ts-node tooling.
- Keep StateManager as the single write path.

**Comparison (final)**
| Option | CLI strength | Install requirement | Fit for skills |
|--------|--------------|---------------------|----------------|
| DuckDB | Strong (native CLI) | Requires binary install | ❌ (install not allowed) |
| SQLite | Strong (sqlite3 CLI) | Requires native install | ❌ (install not allowed) |
| Postgres | Strong (psql) | Requires server install | ❌ (install not allowed) |
| Redis/Valkey | Strong (redis-cli) | Requires server + CLI | ❌ (install not allowed) |
| LokiJS | None built-in (provide our own) | Pure JS dependency | ✅ (build CLI wrapper) |

**Decision**
Use **LokiJS** as the future embedded DB (JS-only). Provide a small Node CLI wrapper for inspection and
updates so skills have a strong CLI surface without native DB installs.

## License

Private project - not for distribution.
