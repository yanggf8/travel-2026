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

### Skill Decision Tree

```
User intent                          → Skill / Action
──────────────────────────────────────────────────────
"plan a trip to [place]"             → Check destinations.json
  destination exists?                   → /p1-dates (if dates not set)
  destination missing?                  → create ref + /p2-destination

"set dates" / "change dates"         → /p1-dates
"which city" / "how many nights"     → /p2-destination
"find packages" / "search OTA"       → /p3p4-packages (uses /scrape-ota)
"find flights only"                  → /p3-flights (uses /scrape-ota)
"compare offers" / "which is cheaper"→ read process_3_4_packages.results
"book this" / "select offer"         → npm run travel -- select-offer
"plan the days" / "itinerary"        → /p5-itinerary
"show status" / "where are we"       → npm run view:status
"show schedule" / "daily plan"       → npm run view:itinerary

User provides OTA URL                → /scrape-ota (see URL Routing below)
User provides booking confirmation   → npm run travel -- set-activity-booking
```

### Data Flow

```
┌──────────────────────────────────────────────────────┐
│ 1. INPUT: User provides URL or search constraints    │
└───────────────────┬──────────────────────────────────┘
                    ▼
┌──────────────────────────────────────────────────────┐
│ 2. SCRAPE: /scrape-ota → Python/Playwright           │
│    Output: data/{ota}-{code}.json                    │
│    Contains: raw_text + extracted (flight/hotel/etc)  │
└───────────────────┬──────────────────────────────────┘
                    ▼
┌──────────────────────────────────────────────────────┐
│ 3. NORMALIZE: extracted → CanonicalOffer[]           │
│    Map: source_id, currency, date_pricing, flight,   │
│         hotel, inclusions                            │
└───────────────────┬──────────────────────────────────┘
                    ▼
┌──────────────────────────────────────────────────────┐
│ 4. WRITE: StateManager.importPackageOffers()         │
│    Updates: process_3_4_packages.results.offers      │
│    Emits: event_log entry                            │
│    Marks: cascade_state dirty                        │
└───────────────────┬──────────────────────────────────┘
                    ▼
┌──────────────────────────────────────────────────────┐
│ 5. SELECT: StateManager.selectOffer(id, date)        │
│    Writes: chosen_offer                              │
│    Triggers: cascade (populate P3 + P4 from offer)   │
└───────────────────┬──────────────────────────────────┘
                    ▼
┌──────────────────────────────────────────────────────┐
│ 6. CASCADE: runner auto-populates downstream         │
│    P3 transport ← offer.flight                       │
│    P4 accommodation ← offer.hotel                    │
│    Clears dirty flags                                │
└──────────────────────────────────────────────────────┘
```

### URL Routing Rules

When user provides a URL, **do not use WebFetch for OTA sites** (they require JavaScript). Instead:

| URL Contains | Action |
|-------------|--------|
| `besttour.com.tw` | `python scripts/scrape_package.py "<url>" data/besttour-<code>.json` |
| `liontravel.com` | `python scripts/scrape_liontravel_dated.py` or `scrape_package.py` |
| `lifetour.com.tw` | `python scripts/scrape_package.py "<url>" data/lifetour-<code>.json` |
| Other travel OTA | Try `scrape_package.py` first (generic Playwright scraper) |
| Non-OTA URL | Use WebFetch as normal |

The scraper outputs structured JSON with `extracted.flight`, `extracted.hotel`, `extracted.price`, `extracted.itinerary`.
Full skill reference: `src/skills/scrape-ota/SKILL.md`

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

Contract version: `1.4.0` (semver: breaking/feature/fix)

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
| `/p1-dates` | `src/skills/p1-dates/SKILL.md` | Set trip dates and flexibility |
| `/p2-destination` | `src/skills/p2-destination/SKILL.md` | Set destination cities and night allocation |
| `/p3-flights` | `src/skills/p3-flights/SKILL.md` | Search flights separately |
| `/p3p4-packages` | `src/skills/p3p4-packages/SKILL.md` | Search OTA packages (flight+hotel) |
| `/p5-itinerary` | `src/skills/p5-itinerary/SKILL.md` | Build and validate daily itinerary |
| `/scrape-ota` | `src/skills/scrape-ota/SKILL.md` | Scrape OTA sites with Playwright (JS rendering) |

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

| Source ID | Name | Type | Supported | Scraper | Search URL |
|-----------|------|------|-----------|---------|------------|
| `besttour` | 喜鴻假期 | package | ✅ | ✅ | `besttour.com.tw/e_web/activity?v=japan_kansai` |
| `liontravel` | 雄獅旅遊 | package, flight, hotel | ✅ | ✅ | `vacation.liontravel.com/search?Destination=JP_OSA_5&...` |
| `lifetour` | 五福旅遊 | package, flight, hotel | ✅ | ✅ | `tour.lifetour.com.tw/searchlist/tpe/0001-0003` |
| `settour` | 東南旅遊 | package, flight, hotel | ✅ | ✅ | `tour.settour.com.tw/search?destinationCode=JX_3` |
| `tigerair` | 台灣虎航 | flight | ✅ | ❌ | — |
| `eztravel` | 易遊網 | package, flight, hotel | ❌ | ❌ | — |

### OTA Search URL Patterns
- **BestTour**: Uses activity pages (`/e_web/activity?v=japan_kansai`), NOT `/e_web/DOM/` (404)
- **LionTravel FIT**: `vacation.liontravel.com/search?Destination={code}&FromDate={YYYYMMDD}&ToDate={YYYYMMDD}&Days={n}&roomlist={adults}-0-0`
- **LionTravel Group**: URL unknown (group tour search returns 404 on `travel.liontravel.com`)
- **Lifetour**: `tour.lifetour.com.tw/searchlist/tpe/{region-code}` (Kansai = `0001-0003`)
- **Settour**: `tour.settour.com.tw/search?destinationCode={code}` (Kansai = `JX_3`)

### Lion Travel Destination Codes
| Code | Destination |
|------|-------------|
| `JP_TYO_5` | Tokyo 5 days |
| `JP_TYO_6` | Tokyo 6 days |
| `JP_OSA_5` | Osaka 5 days |

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
│   └── types/                 # Shared utilities (Result, validation)
│       ├── index.ts
│       └── result.ts          # Result<T,E> for error handling
├── tests/
│   └── integration/           # Integration/regression tests
│       └── state-manager.regression.test.ts
├── scripts/
│   ├── hooks/pre-commit       # Pre-commit TypeScript check
│   ├── migrate-state-keys.ts  # State key migration (legacy → v4.2.0)
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

| Process | Tokyo | Nagoya | Osaka+Kyoto |
|---------|-------|--------|-------------|
| P1 Dates | ✅ confirmed (Feb 13-17) | ✅ confirmed | ⏳ pending (Feb 26 or 27) |
| P2 Destination | ✅ confirmed | ✅ confirmed | ✅ confirmed |
| P3+4 Packages | ✅ **booked** | ⏳ pending (archived) | 🔄 researched (4 OTAs scraped) |
| P3 Transportation | 🎫 booked | 🔄 researched | ⏳ pending |
| P4 Accommodation | 🎫 booked | ⏳ pending | ⏳ pending |
| P5 Itinerary | 🔄 researched (teamLab moved to Sat) | ⏳ pending | ⏳ pending |

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

### 🔄 RESEARCHED: Osaka+Kyoto Feb 26–Mar 2, 2026

**Plan file**: `data/trips/osaka-kyoto-2026/travel-plan.json`
**Dates**: Feb 26 (Thu) or Feb 27 (Fri) → Mar 2 (Mon), 5 days
**Pax**: 2, **Airport**: KIX

#### Package Comparison (Feb 26–27 only, Taipei departure)

| Rank | OTA | Package | TWD/person | After Promo | Type | Date | Status |
|------|-----|---------|-----------|-------------|------|------|--------|
| 1 | LionTravel | 自由配 FIT (Eva Air + Hankyu Respire) | 32,142 | 32,142 | FIT | 02/26 | — |
| 2 | Settour | 漫步京阪奈半自由行 5日 | 33,900 | 32,900 | 半自由 | 02/27 | 餘27席 |
| 3 | Settour | 溫泉átoa和牛龍蝦螃蟹三都 5日 | 35,900 | 34,900 | 跟團 | 02/26 | 已成團, 餘8席 |
| 4 | Settour | 螃蟹吃到飽átoa四都 5日 | 37,900 | 36,400 | 跟團 | 02/27 | 即將成團 |
| 5 | Settour | 京阪神奈琵琶湖天橋立 6日 | 38,900 | 37,900 | 跟團 | 02/27 | 已成團, 餘2席 |
| 6 | Lifetour | 關西五都影城戲雪 6日 | 38,999 | 38,999 | 跟團 | 02/27 | 可售18人 |
| 7 | Lifetour | 海之京都丹後天橋立 5日 | 39,999 | 39,999 | 跟團 | 02/26 | 已成團, 餘3席 |
| 8 | Settour | 環球周邊天橋立美山町伊根 5日 | 40,900 | ~39,400 | 跟團 | 02/27 | 餘20席 |

**Notes:**
- BestTour has no Feb 26/27 departures for Kansai from Taipei
- LionTravel FIT return departs from Kobe UKB (not KIX) — extra transit needed
- Settour dominates with 7 options vs Lifetour's 2
- Scraped data in: `data/liontravel-osaka-feb26.json`, `data/lifetour-osaka-kansai.json`, `data/settour-osaka-kansai.json`, `data/besttour-kansai-refresh.json`

### CLI Quick Reference
```bash
# === VIEWS (read-only) ===
npm run view:status         # Booking overview + fixed-time activities
npm run view:itinerary      # Daily plan with transport
npm run view:transport      # Transport summary (airport + daily)
npm run view:bookings       # Pending/confirmed bookings only

# === COMPARISON ===
npm run travel -- compare-offers --region osaka   # Compare scraped offers by region
npm run travel -- compare-offers --region kansai --json  # JSON output

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
- ✅ Focus tracking with event emission (`setFocus` emits `focus_changed`)
- ✅ Session-level next actions (`setNextActions` with event logging)
- ✅ State key migration script (`scripts/migrate-state-keys.ts`)
- ✅ Skill contracts v1.4.0 — `data_freshness` tier (live/cached/static)
- ✅ Settour OTA integration (scraper URL: `tour.settour.com.tw/search?destinationCode=JX_3`)
- ✅ Lifetour search URL discovery (`tour.lifetour.com.tw/searchlist/tpe/{region}`)
- ✅ Osaka+Kyoto OTA package comparison (4 OTAs, Feb 26-27, 8 options found)
- ✅ OTA search URL templates in `data/ota-sources.json` for all 4 supported OTAs
- ✅ `compare-offers` CLI command (`npm run travel -- compare-offers --region osaka`)
- ✅ Package link extraction in scraper for listing pages
- ✅ Staleness warning for offers older than 24 hours

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

### Tokyo (Feb 13-17)
1. **Book teamLab Borderless** - Feb 15, 2026 (most time-sensitive, can sell out)
2. **Book Limousine Bus** - Low-risk, can buy day-of
3. **Restaurant reservations** - Based on area/cuisine preferences

### Osaka+Kyoto (Feb 26 – Mar 2)
1. **Select package** - Choose from 8 scraped options (see comparison above)
2. **Confirm departure date** - Feb 26 or Feb 27
3. **Scrape individual package pages** - Get flight/hotel details for shortlisted options
4. **Build P5 itinerary** - After package selection
