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
# Build the Rust CLI binary + install git hooks
make setup

# OTA capture uses the chromeport CDP driver (attaches to real Chrome :9222).
#   See: src/skills/scrape-ota/SKILL.md and CLAUDE.md → URL Routing

# See what's in the DB
./bin/travel plans
./bin/travel status --full

# Start a new trip — full Shaping Stage flow: research → compare → adopt → seeded plan
#   See: src/skills/shaping-research/SKILL.md and CLAUDE.md → Skill Decision Tree

# 1. Seed pending flight-scrape attempts for the date/destination matrix → prints <run_id>
./bin/travel shaping-init --origin TPE --start 2026-06-18 --end 2026-06-20 \
  --dest KIX:"Osaka (KIX)" --dest NRT:"Tokyo (NRT)" --nights 6 --nights 7

# 2. Run the aggregator (zero Turso I/O of its own — reads/writes via the CLI)
python scripts/shaping_research.py --run <run_id>

# 3. Compare candidates
./bin/travel shaping-compare --run <run_id>

# 4. Adopt one → creates the plan in Turso and seeds P1 dates + P2 destination
./bin/travel shaping-adopt <candidate_id> osaka-2026 --create-plan --dest osaka_2026
```

Plan state lives in Turso (no JSON state files). For the full command list see `docs/reference/CLI.md`.

## Documentation

- [API Reference](docs/API.md) - Complete API documentation
- [Extension Guide](docs/EXTENDING.md) - How to add destinations, OTAs, and validators
- [CLAUDE.md](CLAUDE.md) - AI assistant context & architecture

## Project Structure

```
├── README.md                 # This file
├── CLAUDE.md                 # AI assistant context & architecture (start here for routing/CLI)
├── docs/
│   ├── API.md                # API reference documentation
│   ├── EXTENDING.md          # Extension guide
│   ├── reference/CLI.md      # Full CLI reference
│   └── trips/                # Past trip archives (Tokyo, Kyoto Feb 2026)
├── data/
│   ├── holidays/             # Holiday calendars (taiwan-2026.json)
│   ├── hotel-areas.json      # Zone categorization (used by compare-true-cost)
│   └── transport-routes.json # Transit routes (used by compare-true-cost)
├── scrapes/                  # Ephemeral scraper outputs (gitignored)
├── src/
│   ├── cascade/              # Cascade rule engine
│   ├── cli/                  # CLI commands (registry-based)
│   ├── config/               # Configuration loaders
│   ├── contracts/            # Skill contracts for agent discovery
│   ├── scrapers/             # OTA scraper framework
│   ├── services/             # turso-service, weather-service, shaping-service
│   ├── skills/               # Reusable planning skills
│   ├── state/                # StateManager + repository (Turso-only)
│   ├── templates/            # Project & destination templates
│   └── validation/           # Itinerary validators
├── scripts/
│   ├── scrape_package.py           # Generic OTA scraper (Playwright)
│   ├── scrape_listings.py          # Listing page scraper (fast metadata)
│   ├── scrape_liontravel_dated.py  # Lion Travel date-specific scraper
│   ├── scrape_date_range.py        # Multi-date flight comparison
│   ├── scrape_tigerair.py          # Tigerair form-based scraper
│   ├── shaping_research.py          # Shaping Stage aggregator (zero Turso I/O)
│   └── filter_packages.py          # Filter scraped packages by criteria
├── workers/trip-dashboard/   # Cloudflare Worker — live dashboard (reads Turso)
└── tsconfig.json
```

> Plan state lives in Turso, not JSON files. `data/` only holds reference data (holidays, hotel zones, transit routes). See **CLAUDE.md → Turso DB** for the schema.

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
./bin/travel validate-itinerary
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
./bin/travel status --full  # Booking overview
./bin/travel itinerary      # Daily plan
./bin/travel transport      # Transport summary

# Mutations
./bin/travel set-dates 2026-02-13 2026-02-17
./bin/travel select-offer <offer-id> <date>
./bin/travel validate-itinerary
./bin/travel set-activity-booking <day> <session> "<activity>" <status>

# OTA capture (chromeport CDP driver — attaches to real Chrome :9222; Python scrapers are decommissioned)
./rust/target/debug/chromeport fetch interact "<url>" --source <id> --step ...
./rust/target/debug/chromeport verify <source-id> <capture-id>
./rust/target/debug/chromeport parse capture <capture-id> --source <id>   # imports to Turso
```

## Tests

This repo uses cost-effective integration/regression tests (no unit test suite).

```bash
make test
```

## Data Schema

The project uses schema version `4.2.0` with destination-scoped architecture.

See `CLAUDE.md` for detailed schema documentation.

## Storage (Turso DB-first)

**Turso cloud is the sole source of truth.** All plan state lives in 28+ normalized tables. There are no local JSON state files — `StateManager` throws if a file path is passed.

- Reads: single batch HTTP round-trip (38 queries → 1 request via `TursoRepository`)
- Writes: `syncNormalizedTables()` inside a transaction — no JSON blobs
- `StateManager.create()` / `StateManager.createFromPlanId()` are the entry points (async factory)

**Common commands**
```bash
./bin/travel db status
./bin/travel db migrate
./bin/travel db query-offers --region kansai --start 2026-02-24 --end 2026-02-28
./bin/travel db exec "SELECT source_id, offer_count FROM plan_offer_provenance WHERE plan_id='tokyo-2026'"
```

## License

Private project - not for distribution.
