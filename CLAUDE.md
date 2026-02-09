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
"find packages" / "search OTA"       → check-freshness first
  fresh data in Turso?                  → query-offers (show existing)
  stale/no data?                        → /p3p4-packages (scrape + auto-import)
"find flights only"                  → /p3-flights (uses /scrape-ota)
"compare offers" / "which is cheaper"→ read process_3_4_packages.results
"query offers" / "what do we have"   → npm run travel -- query-offers --region <r>
"is data fresh" / "when last scraped"→ npm run travel -- check-freshness --source <s>
"book separately" / "split booking"  → /separate-bookings
"how many leave days"                → npm run leave-calc
"book this" / "select offer"         → npm run travel -- select-offer
"plan the days" / "itinerary"        → /p5-itinerary
"show bookings" / "booking status"   → npm run travel -- query-bookings (from DB, not JSON)
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
│    Output: scrapes/{ota}-{code}.json                  │
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
│ 4.5 TURSO: auto-import to Turso offers table         │
│    turso-service.importOffersFromFiles()              │
│    Enables: cross-device query, freshness checks     │
└───────────────────┬──────────────────────────────────┘
                    ▼
┌──────────────────────────────────────────────────────┐
│ 5. SELECT: StateManager.selectOffer(id, date)        │
│    Writes: chosen_offer                              │
│    Triggers: cascade (populate P3 + P4 from offer)   │
└───────────────────┬──────────────────────────────────┘
                    ▼
┌──────────────────────────────────────────────────────┐
│ 5.5 TURSO: sync booking to bookings table            │
│    turso-service.syncBooking()                        │
│    Tracks: selected → booked → confirmed             │
└───────────────────┬──────────────────────────────────┘
                    ▼
┌──────────────────────────────────────────────────────┐
│ 6. CASCADE: runner auto-populates downstream         │
│    P3 transport ← offer.flight                       │
│    P4 accommodation ← offer.hotel                    │
│    Clears dirty flags                                │
└───────────────────┬──────────────────────────────────┘
                    ▼
┌──────────────────────────────────────────────────────┐
│ 7. SAVE: StateManager.save() → JSON + DB             │
│    Writes: travel-plan.json + state.json              │
│    Auto-syncs: bookings_current via fire-and-forget   │
│    Extracts: package + transfer + activity bookings   │
│    Query: npm run travel -- query-bookings            │
└──────────────────────────────────────────────────────┘
```

### URL Routing Rules

When user provides a URL, **do not use WebFetch for OTA sites** (they require JavaScript). Instead:

| URL Contains | Action |
|-------------|--------|
| `besttour.com.tw` | `python scripts/scrape_package.py "<url>" scrapes/besttour-<code>.json` |
| `liontravel.com` | `python scripts/scrape_liontravel_dated.py` or `scrape_package.py` |
| `lifetour.com.tw` | `python scripts/scrape_package.py "<url>" scrapes/lifetour-<code>.json` |
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

Contract version: `1.6.0` (semver: breaking/feature/fix)

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
- `src/config/constants.ts` - Default values (pax, pace, project name, exchange rates)
- `src/skills/travel-shared/references/ota-knowledge.json` - OTA domain knowledge (baggage rules, platform behaviors)

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
| `/separate-bookings` | `src/skills/separate-bookings/SKILL.md` | Compare package vs split flight+hotel booking |

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
| `trip` | Trip.com | flight | ⚠️ scrape-only | ✅ | See URL templates below |
| `booking` | Booking.com | hotel | ⚠️ scrape-only | ✅ | See URL templates below |
| `tigerair` | 台灣虎航 | flight | ✅ | ✅ | Form-based scraper (no URL deep-linking) |
| `agoda` | Agoda | hotel | ✅ | ✅ | Direct hotel URLs work reliably; search may fail for far-future dates |
| `skyscanner` | Skyscanner | flight | ❌ | ❌ | Hard captcha redirect (captcha-v2) blocks all requests |
| `google_flights` | Google Flights | flight | ✅ | ✅ | Natural-language query URL (`?q=Flights to DEST from ORIGIN`) |
| `eztravel` | 易遊網 | flight | ✅ | ✅ | Flight search results parser |
| `travel4u` | 山富旅遊 | package | ✅ | ✅ | `travel4u.com.tw/group/area/{area_code}/japan/` |
| `jalan` | じゃらん | hotel | ❌ | ❌ | Japan domestic OTA, for local hotel bookings |
| `rakuten_travel` | 楽天トラベル | hotel, package | ❌ | ❌ | Japan domestic OTA |

### Individual Booking OTA Notes

**Trip.com** (flights):
- Roundtrip search only shows outbound — always scrape return as separate one-way (`flighttype=ow`)
- Prices in USD, convert to TWD (×32)
- URL: `trip.com/flights/{origin_city}-to-{dest_city}/tickets-{origin}-{dest}?dcity={origin}&acity={dest}&ddate={YYYY-MM-DD}&flighttype=ow&class=y&quantity={pax}`

**Booking.com** (hotels):
- Use `zh-tw` locale, `selected_currency=TWD`
- Requires `dest_id` (not city name): Osaka=-240905, Tokyo=-246227, Kyoto=-235402
- URL: `booking.com/searchresults.zh-tw.html?dest_id={id}&dest_type=city&checkin={YYYY-MM-DD}&checkout={YYYY-MM-DD}&group_adults={n}&no_rooms=1&selected_currency=TWD`
- First search may fail → retry or add `&nflt=class%3D3` filter

**Agoda** (hotels):
- Direct hotel URLs most reliable (search pages may return empty for far-future dates)
- Known city_ids: Osaka=14811, Tokyo=5765, Kyoto=5814, Nagoya=17285, Sapporo=10570, Fukuoka=5788, Okinawa=17074
- URL: `agoda.com/{hotel_slug}/hotel/{city}-jp.html?checkIn={YYYY-MM-DD}&los={nights}&adults={n}&rooms=1&currency=TWD`

**Google Flights** (flights):
- Uses natural-language query URL — no form interaction needed
- URL: `google.com/travel/flights?q=Flights+to+{DEST}+from+{ORIGIN}+on+{YYYY-MM-DD}+through+{YYYY-MM-DD}&curr=TWD&hl=zh-TW`
- Returns all-inclusive TWD prices with airline, times, duration, nonstop flags
- Parser normalizes 16 Chinese airline names to IATA codes

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
├── data/                          # Persistent config + state only
│   ├── travel-plan.json       # Main travel plan (v4.2.0)
│   ├── state.json             # Event-driven state tracking
│   ├── destinations.json      # Destination + origin config (v1.1.0)
│   ├── ota-sources.json       # OTA registry with limitations/price_factors
│   ├── holidays/              # Holiday calendars by country/year
│   │   └── taiwan-2026.json   # Taiwan 2026 holidays + makeup workdays
│   ├── osaka-trip-comparison.json  # Sample trip comparison input
│   └── trips/                 # Multi-plan trip data
│       └── osaka-kyoto-2026/
├── scrapes/                       # Ephemeral scraper outputs (gitignored)
│   ├── cache/                 # Scraper result cache (TTL-based)
│   ├── besttour-*.json        # BestTour scrape results
│   ├── liontravel-*.json      # Lion Travel scrape results
│   ├── trip-*.json            # Trip.com flight scrape results
│   └── booking-*.json         # Booking.com hotel scrape results
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
│   ├── utils/                 # Shared utility modules
│   │   ├── index.ts           # Module exports
│   │   ├── flight-normalizer.ts   # Trip.com flight data → structured flights
│   │   └── leave-calculator.ts    # Leave day calculator with holiday support
│   ├── cli/
│   │   ├── cascade.ts         # Cascade CLI
│   │   ├── compare-dates.ts   # Multi-date FIT vs separate comparison
│   │   ├── compare-trips.ts   # Trip comparison CLI (package vs separate)
│   │   ├── compare-true-cost.ts # True cost comparison (pkg + baggage + transport)
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
│   │   ├── p3p4-packages/
│   │   │   ├── SKILL.md
│   │   │   └── references/legacy-spec.md
│   │   └── separate-bookings/
│   │       └── SKILL.md       # Compare package vs split booking
│   ├── utilities/             # Canonical utility modules
│   │   └── holiday-calculator.ts  # Holiday-aware date ops (cached, config-driven)
│   ├── scrapers/              # OTA scraper registry and base classes
│   │   ├── index.ts           # Module exports
│   │   ├── base-scraper.ts    # Base scraper class
│   │   ├── registry.ts        # Global scraper registry
│   │   └── types.ts           # Scraper type definitions
│   ├── validation/            # Itinerary and data validation
│   │   ├── index.ts           # Module exports
│   │   ├── itinerary-validator.ts  # Itinerary constraint checker
│   │   └── types.ts           # Validation type definitions
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
│   ├── validate-data.ts       # Data consistency validator
│   ├── scrape_package.py      # Generic Playwright OTA scraper
│   ├── scrape_liontravel_dated.py  # Lion Travel date-specific scraper
│   └── scrape_date_range.py   # Multi-date flight price comparison (Trip.com)
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
| P1 Dates | ✅ confirmed (Feb 13-17) | ✅ confirmed | ⏳ pending (Feb 24-28, 3 leave days) |
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

### 🔄 RESEARCHED: Osaka+Kyoto Feb 24–28, 2026

**Plan file**: `data/trips/osaka-kyoto-2026/travel-plan.json`
**Dates**: Feb 24 (Tue) → Feb 28 (Sat), 5 days
**Leave days**: 3 (Tue + Wed + Thu) — leverages 228 holiday weekend
**Pax**: 2, **Airport**: KIX

#### FIT Offers (scraped 2026-02-09)

| Source | Hotel | Price/person | Airline | Flight |
|--------|-------|-------------|---------|--------|
| LionTravel | Just Sleep Osaka Shinsaibashi | TWD 20,792 | Thai Lion Air SL396/397 | 09:00-12:30 / 13:30-15:40 |
| LionTravel | APA Hotel Kyoto Ekimae | TWD 21,796 | Thai Lion Air | 09:00-12:30 / 13:30-15:40 |
| Lifetour | Hotel Tavinos Kyoto | TWD 25,990 | Peach MM024/027 | 09:30-13:20 / 15:35-17:50 |

#### FIT vs Separate Booking Comparison (Feb 24-28)

| Option | Type | Total (2 pax) | Per Person | $/Leave |
|--------|------|---------------|------------|---------|
| **Separate** | 分開訂 | TWD 38,946 | 19,473 | 12,982 |
| LionTravel FIT (Shinsaibashi) | 套餐 | TWD 41,584 | 20,792 | 13,861 |
| LionTravel FIT (APA Kyoto) | 套餐 | TWD 43,592 | 21,796 | 14,531 |

**Separate booking saves TWD 2,638** vs cheapest FIT (Shinsaibashi).

#### Separate Booking Breakdown
```
Flights: AirAsia (out) + Thai Vietjet (return)
         US$213 + US$390 = US$603 × 32.8 = TWD 19,778
         + Baggage: TWD 7,000 (2×2 bags × TWD 1,750)
         = TWD 26,778

Hotel:   Onyado Nono Namba
         TWD 3,042/night × 4 nights = TWD 12,168

Total:   TWD 38,946
```

**Notes:**
- Comparison data from: `data/osaka-trip-comparison.json`
- LionTravel FIT returns from Kobe UKB (not KIX) — extra transit needed
- Separate booking uses LCC (AirAsia/Thai Vietjet) — baggage fee included in total

### CLI Quick Reference
```bash
# === VIEWS (read-only) ===
npm run view:status         # Booking overview + fixed-time activities
npm run view:itinerary      # Daily plan with transport
npm run view:transport      # Transport summary (airport + daily)
npm run view:bookings       # Pending/confirmed bookings only
npm run view:prices -- --flights scrapes/date-range-prices.json --hotel-per-night 3000 --nights 4 --package 40740

# === COMPARISON ===
npm run travel -- compare-offers --region osaka   # Compare scraped offers by region
npm run travel -- compare-offers --region kansai --json  # JSON output

# === LEAVE CALCULATOR ===
npm run leave-calc 2026-02-24 2026-02-28       # Calculate leave days for date range
npm run leave-calc 2026-02-27 2026-03-03       # Uses data/holidays/taiwan-2026.json

# === TRIP COMPARISON ===
npm run compare-trips -- --input data/osaka-trip-comparison.json
npm run compare-trips -- --input data/osaka-trip-comparison.json --detailed

# === DATE COMPARISON (FIT vs Separate) ===
npm run compare-dates -- --start 2026-02-24 --end 2026-02-28 --nights 4
npm run compare-dates -- --start 2026-02-24 --end 2026-02-28 --nights 4 --hotel-per-night 3500

# === TRUE COST COMPARISON ===
npm run compare-true-cost -- --region kansai --pax 2 --date 2026-02-24
npm run compare-true-cost -- --region kansai --pax 2 --itinerary "kyoto:2,osaka:2"

# === FLIGHT NORMALIZER ===
npm run normalize-flights -- scrapes/trip-feb24-out.json --top 5
npm run normalize-flights -- --scan                   # Scan all flight data

# === DATA VALIDATION & HEALTH CHECK ===
npm run validate:data                          # Check CLAUDE.md ↔ code consistency
npm run doctor                                 # Full health check (includes dependency + env checks)
npm run scraper:doctor                         # Test all OTA scrapers are working
npm run scraper:setup                          # Install Playwright if missing

# === BATCH SCRAPER ===
npm run scraper:batch -- --dest kansai                     # Scrape all OTAs for Kansai
npm run scraper:batch -- --dest osaka --sources besttour,settour  # Specific OTAs
npm run scraper:batch -- --dest tokyo --date 2026-02-24 --type fit  # FIT only with date

# === FLIGHT DATE RANGE SCRAPER ===
python scripts/scrape_date_range.py --depart-start 2026-02-24 --depart-end 2026-02-27 \
  --origin tpe --dest kix --duration 5 --pax 2 -o scrapes/date-range-prices.json

# === TURSO DB ===
npm run travel -- query-offers --region kansai --start 2026-02-24 --end 2026-02-28
npm run travel -- query-offers --sources besttour,liontravel --max-price 30000 --json
npm run travel -- check-freshness --source besttour --region kansai
npm run db:import:turso -- --dir scrapes
npm run db:status:turso
npm run db:migrate:turso

# === BOOKINGS DB ===
npm run travel -- sync-bookings                                    # Plan JSON → Turso bookings_current
npm run travel -- sync-bookings --dry-run                          # Preview without writing
npm run travel -- query-bookings --dest tokyo_2026                 # All bookings for Tokyo
npm run travel -- query-bookings --category activity --status pending  # Pending activities
npm run travel -- snapshot-plan --trip-id japan-2026                # Archive plan+state
npm run travel -- check-booking-integrity                          # Plan vs DB drift check
npm run db:sync:bookings                                           # Shortcut for sync-bookings
npm run db:query:bookings                                          # Shortcut for query-bookings

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
| `scripts/scrape_package.py` | Generic package scraper (detail) | BestTour, LionTravel, Lifetour, Settour, Travel4U |
| `scripts/scrape_listings.py` | Fast listing scraper (metadata) | BestTour, LionTravel, Lifetour, Settour, Travel4U |
| `scripts/scrape_eztravel.py` | EzTravel FIT scraper | EzTravel |
| `scripts/filter_packages.py` | Filter scraped packages by criteria | All |
| `scripts/scrape_liontravel_dated.py` | Date-specific pricing | Lion Travel |
| `scripts/scrape_tigerair.py` | Flight price scraper (form-based) | Tigerair |
| `scripts/scrape_date_range.py` | Multi-date flight comparison | Trip.com |

**Requirements:**
```bash
pip install playwright
playwright install chromium
```

**Usage:**
```bash
# Fast listing scrape (metadata only)
python scripts/scrape_listings.py --source besttour --dest kansai -o listings.json

# Filter packages by criteria
python scripts/filter_packages.py scrapes/*.json --type fit --date 2026-02-24 --max-price 25000

# Detail scrape (full package info)
python scripts/scrape_package.py "https://www.besttour.com.tw/itinerary/<CODE>" scrapes/besttour-<CODE>.json

# Scrape Lion Travel with dates
python scripts/scrape_liontravel_dated.py --start 2026-02-13 --end 2026-02-17 scrapes/liontravel-search.json
```

**Scraper Features:**
- **Package Type Classification**: FIT vs Group detection (3/9 OTAs: besttour, lifetour, liontravel)
- **Date Extraction**: Structured departure_date in ISO format (lifetour, liontravel)
- **Two-Stage Workflow**: Fast listing scrape → filter → detail scrape selected packages
- **Cache Management**: File-based cache with TTL, `--refresh` flag to bypass
- **Staleness Detection**: Warns when cached data >24h old

**Classification Keywords** (listing scraper, heuristic):
- **Group**: 團體, 跟團, 精緻團, 品質團, 領隊, 導遊, 自由活動, 自由時間
- **FIT**: 自由行, 機加酒, 自助, 半自由, 伴自由, 自由配, fit

**Accuracy**: Detail scrape (parser logic) > Listing scrape (title keywords, heuristic)

**Usage (date range flight scraper):**
```bash
# Compare 4 departure dates, outbound + return one-way prices
python scripts/scrape_date_range.py --depart-start 2026-02-24 --depart-end 2026-02-27 \
  --origin tpe --dest kix --duration 5 --pax 2 -o scrapes/date-range-prices.json
```

**Output:** Raw text + extracted elements saved to JSON. Manual parsing may be needed for:
- 交通方式 (flights): 去程/回程 sections
- 住宿 (hotel): name, area, amenities
- 價格 (price): per-person and total

## Completed
- ✅ Cascade runner (TypeScript library + CLI)
- ✅ Lion Travel OTA integration
- ✅ Tigerair OTA integration (`scripts/scrape_tigerair.py`) — form-based Playwright scraper
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
- ✅ Osaka+Kyoto FIT vs Separate comparison (Feb 24-28, 3 leave days)
- ✅ OTA search URL templates in `data/ota-sources.json` for all 4 supported OTAs
- ✅ `compare-offers` CLI command (`npm run travel -- compare-offers --region osaka`)
- ✅ Package link extraction in scraper for listing pages
- ✅ Staleness warning for offers older than 24 hours
- ✅ Scraper enhancements: package_type classification (FIT/group), departure_date extraction, listing scraper, filter CLI
- ✅ Holiday calculator (`src/utils/holiday-calculator.ts`) — cached calendar loading, isHoliday/isWorkday/isMakeupWorkday queries, calculateLeave convenience wrapper, config-driven via destinations.json
- ✅ Leave day calculator CLI (`src/utils/leave-calculator.ts`)
- ✅ Multi-date flight scraper (`scripts/scrape_date_range.py`)
- ✅ `/separate-bookings` skill — compare package vs split flight+hotel
- ✅ Trip.com, Booking.com, Agoda, Skyscanner, Google Flights in OTA registry
- ✅ `view-prices` CLI command — package vs separate booking comparison matrix
- ✅ Taiwan 2026 holiday calendar (`data/holidays/taiwan-2026.json`)
- ✅ Leave day calculator with holiday awareness (`src/utils/leave-calculator.ts`)
- ✅ Trip comparison CLI (`src/cli/compare-trips.ts`) — package vs separate with leave day analysis
- ✅ Data consistency validator (`scripts/validate-data.ts`) — CLAUDE.md ↔ ota-sources.json
- ✅ OTA limitations and price_factors in `ota-sources.json` (Trip.com, Booking.com, LionTravel)
- ✅ Origin config with holiday calendar reference in `data/destinations.json` (v1.1.0)
- ✅ Exchange rates and currency conversion in `src/config/constants.ts` (USD_TWD, JPY_TWD)
- ✅ OTA domain knowledge reference (`src/skills/travel-shared/references/ota-knowledge.json`)
- ✅ Flight data normalizer (`src/utils/flight-normalizer.ts`) — Trip.com → structured flights
- ✅ Compare-dates CLI (`src/cli/compare-dates.ts`) — FIT vs separate across date range with leave days
- ✅ `npm run doctor` health check — validates completed items exist, skill files exist, CLI scripts resolve, node_modules ready
- ✅ Pre-commit hook enhanced — runs both typecheck and validate:data
- ✅ Destination reference stubs for nagoya and osaka (`src/skills/travel-shared/references/destinations/`)
- ✅ `/separate-bookings` skill SKILL.md created (`src/skills/separate-bookings/SKILL.md`)
- ✅ Scraper doctor health check (`npm run scraper:doctor`) — tests all OTAs, verifies Playwright
- ✅ Batch scraper (`npm run scraper:batch -- --dest kansai`) — scrape multiple OTAs in one command
- ✅ Package subtype schema (`package_subtype: 'fit' | 'group' | 'semi_fit'`) — FIT vs group distinction
- ✅ OTA product_lines config — separate FIT vs group URL handling in `ota-sources.json`
- ✅ Config-based listing selectors — move CSS selectors to `ota-sources.json` for easier maintenance
- ✅ Playwright install check (`npm run scraper:setup`) — auto-install with postinstall hook
- ✅ Settour scraper fix — uses `.product-item` containers with slider ID extraction
- ✅ EzTravel FIT scraper (`scripts/scrape_eztravel.py`) — packages.eztravel.com.tw with baggage detection
- ✅ Lifetour FIT search URL (`package.lifetour.com.tw/searchlist/all/{region}`) — separate from group tours
- ✅ True cost comparison CLI (`src/cli/compare-true-cost.ts`) — package + baggage + transport costs
- ✅ Region aliases in compare-true-cost — kansai matches osaka/kyoto/kobe/kix filenames
- ✅ Thai Vietjet airline added to ota-knowledge.json (code: VZ, baggage: TWD 700/direction)
- ✅ Baggage calculation respects explicit `baggage_included: false` (EzTravel FIT case)
- ✅ Offers array priority in compare-true-cost — preserves individual hotel names from Lifetour FIT
- ✅ Turso full integration: query-offers, check-freshness, auto-import after scrape, booking sync
- ✅ Bookings table for cross-device booking decision tracking
- ✅ Freshness check before scraping (skip if <24h old, bypass with --force)
- ✅ Skill contracts v1.5.0 — Turso DB operations
- ✅ DB-primary bookings — flat bookings_current table replaces nested JSON reads
- ✅ Booking extractor (`scripts/extract-bookings.ts`) — package + transfer + activity extraction
- ✅ StateManager.save() auto-syncs bookings to Turso (fire-and-forget)
- ✅ CLI: sync-bookings, query-bookings, snapshot-plan, check-booking-integrity
- ✅ Plan snapshots table for versioned plan archival
- ✅ Bookings events audit trail (bookings_events table)
- ✅ Skill contracts v1.6.0 — booking sync/query operations
- ✅ Travel4U (山富旅遊) OTA integration — group tour scraper + parser + ota-sources.json
- ✅ turso-status.ts enhanced — monitors bookings_current, bookings_events, plan_snapshots
- ✅ Turso sync scripts — turso-sync-destinations.ts, turso-sync-events.ts
- ✅ Scrape data cleanup — moved from data/ to scrapes/ (gitignored), added scrapes/ to .gitignore
- ✅ LionTravel Osaka rescrape (Feb 09) — Just Sleep Shinsaibashi TWD 20,792/person

## Storage Decision (DB)

**Decision criteria**
- Strong CLI story for skills (inspect/query/update) — top priority.
- Always warm (no cold-start, no daemon babysitting).
- Native JSON output for curl responses.
- Claude Code agent ergonomics — curl is Claude Code's strongest tool.
- Cross-machine access (plan from laptop, phone, work).
- Keep StateManager as the single write path for mutations.

**Comparison (final)**
| Option | CLI | Always Warm | JSON | Indexes | Setup |
|--------|-----|-------------|------|---------|-------|
| Turso | curl (HTTP) | ✅ Cloud | ✅ | ✅ SQLite B-tree | CLI + signup |
| SurrealDB | curl (HTTP) | ❌ Local daemon | ✅ | ✅ Native | Single binary |
| CouchDB | curl (HTTP) | ❌ Local daemon | ✅ | ✅ Mango/Views | Heavy install |
| PostgreSQL | psql | ❌ Local daemon | JSONB | ✅ | Server install |
| SQLite | sqlite3 | ❌ Cold start | Via JSON1 | ✅ | None |
| PocketBase | curl (HTTP) | ❌ Local daemon | ✅ | ✅ SQLite | Single binary |

**Decision**
Use **Turso** as the skill pack database.

**Why Turso:**
- **curl as CLI** — HTTP API; Claude Code drives curl fluently
- **Always warm** — cloud-hosted, no daemon to start/stop
- **SQLite-compatible** — proven indexes, standard SQL (Claude excels at SQL)
- **Cross-machine** — access travel plans from any device
- **Free tier** — 500 databases, 9GB storage, 1B reads/month
- **Built-in backup** — no manual export needed

**Setup: ✅ COMPLETED (2026-02-06)**
```
Database: travel-2026
Region:   aws-ap-northeast-1 (Tokyo)
URL:      libsql://travel-2026-yanggf8.aws-ap-northeast-1.turso.io
Creds:    .env (gitignored)
```

**Tables:**
- `offers` - Package/flight/hotel offers with pricing
- `destinations` - Tokyo, Osaka configured
- `events` - Audit trail
- `bookings` - Legacy booking decision sync (package-level)
- `bookings_current` - Flat queryable booking rows (package + transfer + activity)
- `bookings_events` - Audit trail for booking status changes
- `plan_snapshots` - Versioned archive of full plan+state JSON

**Usage:**
```bash
# Interactive shell
turso db shell travel-2026

# Query offers for your trip (first-class CLI)
npm run db:query:turso -- --region kansai --start 2026-02-24 --end 2026-02-28
npm run db:query:turso -- --max-price 30000 --sources besttour,liontravel
npm run db:query:turso -- --fresh-hours 24 --json

# Import scraped JSON (with trip-aware date filtering)
npm run db:import:turso -- --dir scrapes
npm run db:import:turso -- --dir scrapes --start 2026-02-24 --end 2026-02-28

# Sanity-check counts / last import timestamps
npm run db:status:turso

# Raw SQL query helper
./scripts/turso-query.sh "SELECT * FROM offers WHERE price_per_person < 35000"
```

**Schema:**
```sql
-- Offers (append-only snapshots)
CREATE TABLE offers (
    id TEXT NOT NULL,              -- offer_key: {source_id}_{product_code}
    source_file TEXT,              -- input filename for import tracking
    source_id TEXT NOT NULL,
    type TEXT CHECK(type IN ('package', 'flight', 'hotel')),
    name TEXT,
    price_per_person INTEGER,
    currency TEXT DEFAULT 'TWD',
    region TEXT,
    destination TEXT,
    departure_date TEXT,
    return_date TEXT,
    nights INTEGER,
    availability TEXT,
    hotel_name TEXT,
    airline TEXT,
    raw_data TEXT,
    scraped_at DATETIME,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE UNIQUE INDEX idx_offers_dedup ON offers(id, scraped_at);

-- Destinations
CREATE TABLE destinations (
    slug TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    currency TEXT DEFAULT 'JPY',
    timezone TEXT,
    primary_airports TEXT
);

-- Events (audit)
CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    external_id TEXT UNIQUE, -- Stable hash for idempotency
    event_type TEXT NOT NULL,
    destination TEXT,
    process TEXT,
    data TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Bookings (decision sync)
CREATE TABLE bookings (
    destination TEXT NOT NULL,
    offer_id TEXT NOT NULL,
    selected_date TEXT NOT NULL,
    price_per_person INTEGER,
    price_total INTEGER,
    currency TEXT DEFAULT 'TWD',
    status TEXT CHECK(status IN ('selected', 'booked', 'confirmed')),
    source_id TEXT,
    hotel_name TEXT,
    airline TEXT,
    flight_out TEXT,
    flight_return TEXT,
    selected_at DATETIME,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (destination, offer_id)
);

-- Plan snapshots (versioned archive)
CREATE TABLE IF NOT EXISTS plan_snapshots (
    snapshot_id TEXT PRIMARY KEY,
    trip_id TEXT NOT NULL,
    schema_version TEXT NOT NULL,
    plan_json TEXT NOT NULL,
    state_json TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Bookings current (flat queryable rows — replaces nested JSON reads)
CREATE TABLE IF NOT EXISTS bookings_current (
    booking_key TEXT PRIMARY KEY,
    trip_id TEXT NOT NULL,
    destination TEXT NOT NULL,
    category TEXT NOT NULL CHECK(category IN ('package','transfer','activity')),
    subtype TEXT,
    title TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending','planned','booked','confirmed','waitlist','skipped','cancelled')),
    reference TEXT,
    book_by TEXT,
    booked_at TEXT,
    source_id TEXT,
    offer_id TEXT,
    selected_date TEXT,
    price_amount INTEGER,
    price_currency TEXT DEFAULT 'TWD',
    origin_path TEXT,
    payload_json TEXT,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Bookings events (audit trail)
CREATE TABLE IF NOT EXISTS bookings_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    booking_key TEXT NOT NULL,
    event_type TEXT NOT NULL,
    previous_status TEXT,
    new_status TEXT,
    reference TEXT,
    book_by TEXT,
    amount INTEGER,
    currency TEXT,
    event_data TEXT,
    event_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
```

**Alternative (local-first):** SurrealDB — single binary, no signup, works offline. Use if cloud dependency is a concern.

## Next Steps

### Tokyo (Feb 13-17)
1. **Book teamLab Borderless** - Feb 15, 2026 (most time-sensitive, can sell out)
2. **Book Limousine Bus** - Low-risk, can buy day-of
3. **Restaurant reservations** - Based on area/cuisine preferences

### Osaka+Kyoto (Feb 24 – 28)
1. **Verify flight prices** - Re-scrape Feb 24 outbound + Feb 28 return (prices may have changed)
2. **Confirm hotel availability** - Onyado Nono Namba for 4 nights
3. **Decide FIT vs Separate** - Separate saves TWD 1,808 but uses LCC
4. **Build P5 itinerary** - After booking decision
