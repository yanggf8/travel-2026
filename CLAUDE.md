# Japan Travel Project

## Trip Details
- **Dates**: February 13-17, 2026 (confirmed, booked)
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
- Every agent output should include: current status, what changed, and the single best "next action".

### Skill Contracts (Agent Discovery)

Before invoking CLI operations, agent should check `src/contracts/skill-contracts.ts`:

```typescript
import { SKILL_CONTRACTS, STATE_MANAGER_METHODS, validateStateManagerInterface } from './contracts';

// List available CLI commands
Object.keys(SKILL_CONTRACTS);  // ['set-dates', 'select-offer', 'mark-booked', ...]

// Get contract for a command
SKILL_CONTRACTS['mark-booked'].mutates;  // ['state.next_actions', ...]

// Validate StateManager interface (catch drift early)
const missing = validateStateManagerInterface(stateManager);
if (missing.length > 0) throw new Error(`Missing methods: ${missing}`);
```

Contract version: `1.1.0` (semver: breaking/feature/fix)

### Build Gate

Pre-commit hook runs `npm run typecheck`. Install with:
```bash
npm run hooks:install
```

### Configuration Discovery APIs

The skill pack provides discovery APIs for multi-destination and multi-OTA support:

```typescript
import {
  getAvailableDestinations,    // List all configured destinations
  getDestinationConfig,        // Get full config for a destination
  resolveDestinationRefPath,   // Get path to POI/cluster reference
  getAvailableOtaSources,      // List all OTA sources
  getSupportedOtaSources,      // List OTAs with working scrapers
  getOtaSourceCurrency,        // Get currency for an OTA (TWD, JPY)
} from './config/loader';

// Example: Add new destination support
const destinations = getAvailableDestinations();  // ['tokyo_2026', 'nagoya_2026', 'osaka_2026']
const tokyoRef = resolveDestinationRefPath('tokyo_2026');  // Absolute path to tokyo.json
```

Configuration files:
- `data/destinations.json` - Destination mapping (slug → reference path, currency, airports)
- `data/ota-sources.json` - OTA registry (source_id → URL, currency, scraper script)
- `src/config/constants.ts` - Default values (pax, pace, project name)

Notes:
- `ref_path` / `scraper_script` must be **repo-relative** paths (no absolute paths); they’re resolved from the project root so commands work from any CWD.
- `getSupportedOtaSources()` means: `supported=true` **and** `scraper_script` is set **and** the script exists on disk.

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

| Source ID | Name | Type | Supported | Scraper |
|-----------|------|------|-----------|---------|
| `besttour` | 喜鴻假期 | package | ✅ | ✅ |
| `liontravel` | 雄獅旅遊 | package, flight, hotel | ✅ | ✅ |
| `tigerair` | 台灣虎航 | flight | ✅ | ❌ |
| `eztravel` | 易遊網 | package, flight, hotel | ❌ | ❌ |

### Lion Travel Promo
- Code: `FITPKG` - TWD 400 off on Thursdays (min TWD 20,000)

## Project Structure
```
/
├── CLAUDE.md                  # AI assistant context (this file)
├── data/
│   ├── travel-plan.json       # Main travel plan (v4.2.0)
│   ├── state.json             # Event-driven state tracking
│   ├── destinations.json      # Destination configuration (multi-destination)
│   ├── ota-sources.json       # OTA source registry (multi-OTA)
│   ├── besttour-*.json        # BestTour scrape results (date-specific pricing)
│   ├── liontravel-*.json      # Lion Travel scrape results
│   ├── eztravel-*.json        # ezTravel scrape results
│   ├── tigerair-*.json        # Tigerair scrape results
│   └── flights-cache.json     # Legacy flight cache (Nagoya research)
├── src/
│   ├── config/                # Skill pack configuration
│   │   ├── index.ts           # Module exports
│   │   ├── constants.ts       # Configurable defaults (pax, pace, currency)
│   │   └── loader.ts          # Config discovery APIs
│   ├── contracts/             # Skill contracts for agent discovery
│   │   ├── index.ts           # Module exports
│   │   └── skill-contracts.ts # CLI operation contracts (v1.1.0)
│   ├── cascade/               # Cascade runner library
│   │   ├── index.ts           # Module exports
│   │   ├── runner.ts          # Core cascade logic
│   │   ├── types.ts           # TypeScript definitions
│   │   └── wildcard.ts        # Schema-driven path expansion
│   ├── cli/
│   │   ├── cascade.ts         # Cascade CLI
│   │   ├── p3p4-test.ts       # Package skill test CLI
│   │   └── travel-update.ts   # Travel plan update CLI
│   ├── state/                 # State management
│   │   ├── index.ts           # Module exports
│   │   ├── state-manager.ts   # StateManager class
│   │   ├── types.ts           # TypeScript definitions
│   │   ├── schemas.ts         # Zod runtime validation
│   │   └── destination-ref-schema.ts  # POI/cluster validation
│   ├── skills/                # Reusable planning skills
│   │   ├── travel-shared/     # Shared references (bundle)
│   │   │   ├── SKILL.md
│   │   │   └── references/destinations/  # Per-destination POI/cluster refs
│   │   ├── p3-flights/
│   │   │   ├── SKILL.md
│   │   │   └── references/legacy-spec.md
│   │   └── p3p4-packages/
│   │       ├── SKILL.md
│   │       └── references/legacy-spec.md
│   ├── status/
│   │   ├── rule-evaluator.ts
│   │   └── status-check.ts
│   ├── types/                 # Shared type utilities
│   │   ├── index.ts
│   │   └── result.ts          # Result<T,E> for error handling
│   └── types/                 # Shared utilities (Result, validation)
├── tests/
│   └── integration/           # Integration/regression tests
│       └── state-manager.regression.test.ts
├── scripts/
│   ├── hooks/pre-commit       # Pre-commit TypeScript check
│   ├── scrape_package.py      # Generic Playwright OTA scraper
│   └── scrape_liontravel_dated.py  # Lion Travel date-specific scraper
├── vitest.config.ts           # Test configuration
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
| P3+4 Packages | ✅ **booked** | ⏳ pending (archived) |
| P3 Transportation | 🎫 booked | 🔄 researched |
| P4 Accommodation | 🎫 booked | ⏳ pending |
| P5 Itinerary | 🔄 researched (teamLab moved to Sat) | ⏳ pending |

### Airport Transfers (Tokyo)
| Direction | Status | Selected |
|-----------|--------|----------|
| Arrival | planned | Limousine Bus (NRT T2 → Shiodome) - ¥3,200, ~85min |
| Departure | planned | Limousine Bus (Shiodome → NRT T2) - ¥3,200, ~85min |

### Itinerary Summary (Feb 13-17, 2026)

| Day | Date | Morning | Afternoon | Evening |
|-----|------|---------|-----------|---------|
| 1 | Fri 13 | ✈️ TPE → NRT | Arrival + Narita dinner | Hotel check-in |
| **2** | **Sat 14** | **teamLab Borderless** | Asakusa (Senso-ji) | Harajuku |
| 3 | Sun 15 | Azabudai Hills | Roppongi + Shibuya | Roppongi |
| 4 | Mon 16 | KOMEHYO (Chanel) | Isetan omiyage | Omoide Yokocho |
| 5 | Tue 17 | Pack + Checkout | Shiodome area | ✈️ NRT → TPE |

**Booking Links:**
- teamLab Borderless: https://www.teamlab.art/e/borderless-azabudai/ (book by Feb 10)
- Limousine Bus: https://www.limousinebus.co.jp/en/ (arrival & departure)

### ✅ BOOKED: Tokyo Feb 13-17, 2026
```
Package: besttour_TYO06MM260213AM2
Dates:   Fri Feb 13 → Tue Feb 17 (5 days)
Price:   TWD 27,888/person (TWD 55,776 for 2 pax)

Flight (Scoot):
  去程: TR874 TPE 13:55 → NRT 18:00 (Feb 13)
  回程: TR875 NRT 19:55 → TPE 23:10 (Feb 17)

Hotel:   TAVINOS Hamamatsucho
         Area: Shimbashi / Hamamatsucho
         Access: JR Hamamatsucho 8min, Yurikamome Takeshiba 1min
         Includes: Light breakfast
```

### CLI Quick Reference
```bash
# === VIEWS (read-only) ===
npm run view:status         # Booking overview + fixed-time activities
npm run view:itinerary      # Daily plan with transport
npm run view:transport      # Transport summary (airport + daily)
npm run view:bookings       # Pending/confirmed bookings only

# === MUTATIONS (write) ===
npm run travel -- set-dates 2026-02-13 2026-02-17
npm run travel -- select-offer <offer-id> <date>
npm run travel -- set-activity-booking <day> <session> "<activity>" <status> [--ref "..."] [--book-by YYYY-MM-DD]
npm run travel -- set-airport-transfer <arrival|departure> <planned|booked> --selected "title|route|duration|price|schedule"
npm run travel -- set-activity-time <day> <session> "<activity>" [--start HH:MM] [--end HH:MM] [--fixed true]
npm run travel -- set-session-time-range <day> <session> --start HH:MM --end HH:MM
```

### Agent Output Pattern

Claude Code CLI collapses long Bash/Read tool output (`+N lines, ctrl+o to expand`).
To ensure visibility, agent must output content as direct text:

```
1. Bash: npm run view:* > /tmp/view.txt   (capture to file)
2. Read: /tmp/view.txt                     (agent sees content)
3. Text: paste content in response         (user sees it)
```

**Rule**: When user asks "show me X", always use this pattern — never rely on collapsed tool output.

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
- ✅ Zod runtime validation for travel-plan.json schema
- ✅ Activity booking status tracking (booking_status, booking_ref, book_by)
- ✅ Airport transfer schema (selected + candidates pattern)
- ✅ Destination reference validation (`src/state/destination-ref-schema.ts`)
- ✅ Skill contracts for agent CLI discovery (`src/contracts/skill-contracts.ts`)
- ✅ Pre-commit hook for TypeScript type checking
- ✅ Multi-destination configuration (`data/destinations.json`)
- ✅ Multi-OTA source registry (`data/ota-sources.json`)
- ✅ Configuration discovery APIs (`src/config/loader.ts`)
- ✅ Configurable defaults extraction (`src/config/constants.ts`)
- ✅ Time-aware scheduling (start_time, end_time, is_fixed_time on Activity)
- ✅ Session time boundaries (time_range on DaySession)
- ✅ Fixed-time activities in `status --full` (reservations/deadlines at a glance)
- ✅ Integration test framework (Vitest, `tests/integration/`)
- ✅ StateManager in-memory testing support (`StateManagerOptions`)
- ✅ Activity search helper extraction (`findActivityIndex`)
- ✅ Result type for error handling (`src/types/result.ts`)
- ✅ Input validation utilities (`src/types/validation.ts`)
- ✅ CLI argument validation (dates, numbers, times)

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

## Next Steps
1. **Book teamLab Borderless** - Feb 15, 2026 (most time-sensitive, can sell out)
2. **Book Limousine Bus** - Low-risk, can buy day-of
3. **Restaurant reservations** - Based on area/cuisine preferences
4. **Build comparison tool** - Derive rankings from destinations/*
