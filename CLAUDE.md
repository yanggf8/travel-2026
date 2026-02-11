# Japan Travel Project

## Trip Details
- **Dates**: February 13-17, 2026 (confirmed, booked)
- **Active Destination**: Tokyo, Japan
- **Schema**: `4.2.0` — Destination-scoped with canonical offer model

## Architecture

### Data Model
```
travel-plan.json
├── schema_version: "4.2.0"
├── active_destination: "tokyo_2026"
├── process_1_date_anchor          # Shared across destinations
├── destinations/
│   ├── tokyo_2026/                # ACTIVE (P2-P5)
│   └── nagoya_2026/               # ARCHIVED
├── cascade_rules/                 # Machine-checkable rules
├── cascade_state/                 # Per-destination dirty flags
└── canonical_offer_schema/        # All scrapers normalize to this
```

### Cascade Rules
| Trigger | Reset | Scope |
|---------|-------|-------|
| `active_destination_change` | `process_5_*` | new destination |
| `process_1_date_anchor_change` | `process_3_*`, `process_4_*`, `process_5_*` | all destinations |
| `process_2_destination_change` | `process_3_*`, `process_4_*`, `process_5_*` | current destination |
| `process_3_4_packages_selected` | populate P3+P4 from chosen offer | current destination |

### Data Flow
`URL → scrape (Playwright) → normalize (CanonicalOffer[]) → StateManager.importPackageOffers() → Turso auto-import → selectOffer() → cascade (populate P3+P4) → save() (DB write → derived sync)`

Canonical offer schema: `src/state/types.ts`. Skill contracts: `src/contracts/skill-contracts.ts` (v1.9.0).

### Repository Architecture (v2.0.0)
```
CLI / Skills / Dashboard
        ↓ commands
   StateManager          ← state machine: validate, transition, cascade
        ↓ repository calls
   StateRepository       ← interface (abstract)
        ↓
   TursoRepository       ← normalized tables (itinerary) + blob (offers/transport)
        ↓
   BlobBridgeRepository  ← in-memory plan + blob persistence + dual-write
```

```
WRITE:  mutate → await save() → write blob (blocking) → write normalized tables (blocking) → sync bookings+events (fire-and-forget)
READ:   await StateManager.create() → TursoRepository.create() → load blob + overlay itinerary from normalized tables → memory
```

- **Turso cloud is sole source of truth** — no JSON file reads/writes in runtime path
- **Normalized tables** for itinerary: `itinerary_days`, `itinerary_sessions`, `activities` (+ 7 supporting tables)
- **Blob still written** for backward compat — dashboard and cascade runner read from it via reconstructed plan object
- `StateManager.save()` is async — blob write + normalized table write must succeed or command fails
- `StateManager.saveWithTracking(cmd, summary)` wraps `save()` with operation audit trail in `operation_runs` table; CLI commands use this instead of raw `save()`
- `plans.version` is a monotonic counter bumped on each save (audit trail only, no lock)
- `StateManager.create()` is async factory — reads blob + normalized tables from DB
- `dispatch(command)` entry point — 25 command types as discriminated union
- Plan ID: `"<trip-id>"` | `"path:<sha1-12>"` (derived from file path, e.g., `tokyo-2026`, `kyoto-2026`)
- Tests use `skipSave: true` — DB calls skipped entirely
- DB info messages use `console.error` (stderr) to avoid polluting JSON output

## Agent-First Workflow

- Proactively run next logical step; only ask user when a preference materially changes the result
- Prefer `StateManager` methods / CLI wrappers over direct JSON edits
- Every output: current status, what changed, single best next action

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
"compare offers"                     → read process_3_4_packages.results
"query offers"                       → npm run travel -- query-offers --region <r>
"is data fresh"                      → npm run travel -- check-freshness --source <s>
"book separately"                    → /separate-bookings
"how many leave days"                → npm run leave-calc
"book this" / "select offer"         → npm run travel -- select-offer
"plan the days" / "itinerary"        → /p5-itinerary
"show bookings"                      → npm run travel -- query-bookings (from DB)
"show status"                        → npm run view:status
"show schedule"                      → npm run view:itinerary
"weather" / "forecast"               → npm run travel -- fetch-weather [--dest slug]
User provides OTA URL                → /scrape-ota (see URL Routing)
User provides booking confirmation   → npm run travel -- set-activity-booking
"deploy dashboard" / "publish trip"  → cd workers/trip-dashboard && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy
```

### URL Routing
**Do not use WebFetch for OTA sites** (they require JavaScript):

| URL Contains | Action |
|-------------|--------|
| `besttour.com.tw` | `python scripts/scrape_package.py "<url>" scrapes/besttour-<code>.json` |
| `liontravel.com` | `python scripts/scrape_liontravel_dated.py` or `scrape_package.py` |
| `lifetour.com.tw` | `python scripts/scrape_package.py "<url>" scrapes/lifetour-<code>.json` |
| Other travel OTA | Try `scrape_package.py` first (generic Playwright scraper) |
| Non-OTA URL | Use WebFetch as normal |

Full skill reference: `src/skills/scrape-ota/SKILL.md`

### Agent Output Pattern
Run CLI commands directly via Bash and show the output. No need to redirect to temp files.

## Available Skills
| Skill | Path | Purpose |
|-------|------|---------|
| `travel-shared` | `src/skills/travel-shared/SKILL.md` | Shared references |
| `/p1-dates` | `src/skills/p1-dates/SKILL.md` | Set trip dates |
| `/p2-destination` | `src/skills/p2-destination/SKILL.md` | Set destination cities |
| `/p3-flights` | `src/skills/p3-flights/SKILL.md` | Search flights separately |
| `/p3p4-packages` | `src/skills/p3p4-packages/SKILL.md` | Search OTA packages (flight+hotel) |
| `/p5-itinerary` | `src/skills/p5-itinerary/SKILL.md` | Build daily itinerary |
| `/scrape-ota` | `src/skills/scrape-ota/SKILL.md` | Scrape OTA sites (Playwright) |
| `/separate-bookings` | `src/skills/separate-bookings/SKILL.md` | Compare package vs split booking |

## OTA Sources

| Source ID | Name | Type | Status |
|-----------|------|------|--------|
| `besttour` | 喜鴻假期 | package | ✅ scraper |
| `liontravel` | 雄獅旅遊 | package, flight, hotel | ✅ scraper |
| `lifetour` | 五福旅遊 | package, flight, hotel | ✅ scraper |
| `settour` | 東南旅遊 | package, flight, hotel | ✅ scraper |
| `trip` | Trip.com | flight | ⚠️ scrape-only |
| `booking` | Booking.com | hotel | ⚠️ scrape-only |
| `tigerair` | 台灣虎航 | flight | ✅ scraper |
| `agoda` | Agoda | hotel | ✅ scraper |
| `google_flights` | Google Flights | flight | ✅ scraper |
| `eztravel` | 易遊網 | flight | ✅ scraper |
| `travel4u` | 山富旅遊 | package | ✅ scraper |
| `skyscanner` | Skyscanner | flight | ❌ captcha |
| `jalan` | じゃらん | hotel | ❌ unsupported |
| `rakuten_travel` | 楽天トラベル | hotel, package | ❌ unsupported |

### OTA URL Templates & Notes
- **BestTour**: `/e_web/activity?v=japan_kansai` (NOT `/e_web/DOM/`)
- **LionTravel FIT**: `vacation.liontravel.com/search?Destination={code}&FromDate={YYYYMMDD}&ToDate={YYYYMMDD}&Days={n}&roomlist={adults}-0-0`
- **LionTravel codes**: JP_TYO_5/6 (Tokyo), JP_OSA_5 (Osaka). Promo: `FITPKG` TWD 400 off Thu (min 20k)
- **Lifetour**: `tour.lifetour.com.tw/searchlist/tpe/{region}` (Kansai=`0001-0003`)
- **Settour**: `tour.settour.com.tw/search?destinationCode={code}` (Kansai=`JX_3`)
- **Trip.com**: One-way only (`flighttype=ow`), prices in USD (x32). URL: `trip.com/flights/{origin}-to-{dest}/tickets-{IATA}-{IATA}?ddate={date}&flighttype=ow&class=y&quantity={pax}`
- **Booking.com**: `zh-tw` locale, `selected_currency=TWD`. dest_ids: Osaka=-240905, Tokyo=-246227, Kyoto=-235402
- **Agoda**: Direct hotel URLs most reliable. city_ids: Osaka=14811, Tokyo=5765, Kyoto=5814
- **Google Flights**: `google.com/travel/flights?q=Flights+to+{DEST}+from+{ORIGIN}+on+{date}+through+{date}&curr=TWD&hl=zh-TW`

### Scraper Scripts
| Script | Purpose | OTA |
|--------|---------|-----|
| `scrape_package.py` | Detail scraper | BestTour, LionTravel, Lifetour, Settour, Travel4U |
| `scrape_listings.py` | Fast listing scraper | BestTour, LionTravel, Lifetour, Settour, Travel4U |
| `scrape_eztravel.py` | EzTravel FIT | EzTravel |
| `filter_packages.py` | Filter by criteria | All |
| `scrape_liontravel_dated.py` | Date-specific | Lion Travel |
| `scrape_tigerair.py` | Flight prices | Tigerair |
| `scrape_date_range.py` | Multi-date flights | Trip.com |

Requires: `pip install playwright && playwright install chromium`

## Current Status

| Process | Tokyo | Nagoya | Kyoto |
|---------|-------|--------|-------------|
| P1 Dates | ✅ confirmed (Feb 13-17) | ✅ confirmed | ✅ confirmed (Feb 24-28, 3 leave days) |
| P2 Destination | ✅ confirmed | ✅ confirmed | ✅ confirmed |
| P3+4 Packages | ✅ **booked** | ⏳ pending (archived) | ✅ **booked** (LionTravel FIT) |
| P3 Transportation | 🎫 booked | 🔄 researched | 🎫 booked |
| P4 Accommodation | 🎫 booked | ⏳ pending | 🎫 booked |
| P5 Itinerary | 🔄 researched | ⏳ pending | 🔄 researched |

### BOOKED: Tokyo Feb 13-17
```
Package: besttour_TYO06MM260213AM2 — TWD 27,888/person (55,776 for 2 pax)
Flight:  Scoot TR874 TPE 13:55→NRT 18:00 / TR875 NRT 19:55→TPE 23:10
Hotel:   TAVINOS Hamamatsucho (light breakfast, JR Hamamatsucho 8min)
```

Airport transfers: Limousine Bus ¥3,200 each way (NRT T2 ↔ Shiodome, ~85min), status: planned

### Itinerary (Feb 13-17)
| Day | Date | Morning | Afternoon | Evening |
|-----|------|---------|-----------|---------|
| 1 | Fri 13 | ✈️ TPE→NRT | Arrival + Narita dinner | Hotel check-in |
| 2 | Sat 14 | **teamLab Borderless** | Asakusa (Senso-ji) | Harajuku |
| 3 | Sun 15 | Azabudai Hills | Roppongi + Shibuya | Roppongi |
| 4 | Mon 16 | KOMEHYO (Chanel) | Isetan omiyage | Omoide Yokocho |
| 5 | Tue 17 | Pack + Checkout | Shiodome area | ✈️ NRT→TPE |

**Book by Feb 10**: teamLab Borderless (https://www.teamlab.art/e/borderless-azabudai/)
**Limousine Bus**: https://www.limousinebus.co.jp/en/

### BOOKED: Kyoto Feb 24-28
```
Package: liontravel_190620015 — TWD 23,348/person (46,696 for 2 pax)
Order:   2026-1311130
Flight:  Thai Lion Air TPE→KIX / KIX→TPE
Hotel:   APA Hotel Kyoto Ekimae (APA京都站前, JR Kyoto Station 3min)
Includes: Kyoto Yumeyakata Kimono Experience, eSIM data
```

Airport transfers: JR Haruka Express ¥450/trip/person round-trip (KIX ↔ Kyoto Station, ~75min), included in package, status: booked

## CLI Quick Reference
```bash
# === VIEWS ===
npm run view:status | view:itinerary | view:transport | view:bookings
npm run view:prices -- --flights scrapes/date-range-prices.json --hotel-per-night 3000 --nights 4 --package 40740

# === COMPARISON ===
npm run travel -- compare-offers --region osaka [--json]
npm run compare-trips -- --input data/osaka-trip-comparison.json [--detailed]
npm run compare-dates -- --start 2026-02-24 --end 2026-02-28 --nights 4
npm run compare-true-cost -- --region kansai --pax 2 --date 2026-02-24

# === SCRAPING ===
npm run scraper:batch -- --dest kansai [--sources besttour,settour] [--date 2026-02-24 --type fit]
npm run scraper:doctor                         # Test all scrapers
python scripts/scrape_date_range.py --depart-start 2026-02-24 --depart-end 2026-02-27 \
  --origin tpe --dest kix --duration 5 --pax 2 -o scrapes/date-range-prices.json

# === TURSO DB ===
npm run travel -- query-offers --region kansai --start 2026-02-24 --end 2026-02-28 [--max-price 30000] [--json]
npm run travel -- check-freshness --source besttour --region kansai
npm run db:import:turso -- --dir scrapes [--start 2026-02-24 --end 2026-02-28]
npm run db:status:turso | db:migrate:turso | db:seed:plans

# === BOOKINGS ===
npm run travel -- sync-bookings [--dry-run]
npm run travel -- query-bookings --dest tokyo_2026 [--category activity --status pending]
npm run travel -- snapshot-plan --trip-id japan-2026
npm run travel -- check-booking-integrity

# === UTILITIES ===
npm run leave-calc 2026-02-24 2026-02-28
npm run normalize-flights -- scrapes/trip-feb24-out.json --top 5
npm run validate:data | npm run doctor

# === MUTATIONS ===
npm run travel -- set-dates 2026-02-13 2026-02-17
npm run travel -- select-offer <offer-id> <date>
npm run travel -- set-activity-booking <day> <session> "<activity>" <status> [--ref "..."] [--book-by YYYY-MM-DD]
npm run travel -- set-airport-transfer <arrival|departure> <planned|booked> --selected "title|route|duration|price|schedule"
npm run travel -- set-activity-time <day> <session> "<activity>" [--start HH:MM] [--end HH:MM] [--fixed true]
npm run travel -- set-session-time-range <day> <session> --start HH:MM --end HH:MM
npm run travel -- fetch-weather [--dest slug]

# === OPERATION TRACKING ===
npm run travel -- run-status [run-id]
npm run travel -- run-list [--status completed|failed|started] [--limit N]
```

## Project Structure
```
/
├── data/
│   ├── destinations.json          # Destination config (v1.1.0)
│   ├── ota-sources.json           # OTA registry
│   └── holidays/taiwan-2026.json  # Holiday calendar
├── scrapes/                       # Ephemeral scraper outputs (gitignored)
├── scripts/                       # Python scrapers + migration tools
│   └── hooks/pre-commit           # Runs typecheck + validate:data
├── workers/trip-dashboard/        # Cloudflare Worker — live trip dashboard
│   ├── wrangler.toml              # Worker config + secret bindings
│   ├── src/index.ts               # Request handler + router + favicon
│   ├── src/turso.ts               # Turso HTTP pipeline client (fetch-based)
│   ├── src/render.ts              # SSR HTML renderer (ZH default)
│   ├── src/zh-content.ts          # Chinese itinerary content overrides
│   └── src/styles.ts              # Mobile-first inline CSS
├── src/
│   ├── cli/travel-update.ts       # Main CLI entry
│   ├── state/
│   │   ├── state-manager.ts       # State machine: validate, transition, cascade, dispatch()
│   │   ├── repository.ts          # StateRepository interface (StateReader + StateWriter)
│   │   ├── turso-repository.ts    # Reads itinerary from normalized tables, delegates to BlobBridge
│   │   ├── blob-bridge-repository.ts # JSON blob ↔ repository bridge, dual-write to tables
│   │   ├── commands.ts            # 25-type Command discriminated union
│   │   ├── types.ts               # Domain types, status transitions
│   │   ├── itinerary-manager.ts   # Itinerary domain logic
│   │   ├── offer-manager.ts       # Offer domain logic
│   │   ├── transport-manager.ts   # Transport domain logic
│   │   └── event-query.ts         # Event log queries
│   ├── config/                    # loader.ts, constants.ts
│   ├── contracts/skill-contracts.ts
│   ├── cascade/runner.ts          # Cascade logic
│   ├── services/turso-service.ts  # DB access layer (all Turso queries go through here)
│   ├── utils/                     # flight-normalizer, leave-calculator
│   ├── skills/                    # Skill SKILL.md files + references
│   ├── scrapers/                  # Registry + base classes
│   ├── validation/                # Itinerary validator
│   └── types/result.ts            # Result<T,E>
└── tests/integration/
```

Config files: `data/destinations.json`, `data/ota-sources.json`, `src/config/constants.ts` (defaults/exchange rates), `src/skills/travel-shared/references/ota-knowledge.json` (baggage rules).
Note: `ref_path`/`scraper_script` must be repo-relative paths.

## Turso DB
```
Database: travel-2026 | Region: aws-ap-northeast-1 | Creds: .env (gitignored)
```

Tables:
- **Blob**: `plans` (DB-primary plan+state, PK=plan_id, `version` monotonic counter)
- **Normalized itinerary**: `itinerary_days`, `itinerary_sessions`, `activities` (PK composites on plan_id+destination+day_number)
- **Normalized supporting**: `plan_metadata`, `date_anchors`, `process_statuses`, `cascade_dirty_flags`, `airport_transfers`, `flights`, `hotels`
- **Bookings**: `bookings_current` (flat rows: package/transfer/activity), `bookings_events` (audit)
- **Operation tracking**: `operation_runs` (audit trail: run_id, plan_id, command_type, status, version_before/after, timestamps)
- **Other**: `offers`, `destinations`, `events`, `bookings`, `plan_snapshots` (versioned archive)

Schema/migration: `npm run db:migrate:turso` (creates all tables idempotently)
Seed from JSON: `npm run db:seed:plans` (one-time, already run — local JSON files removed)
Data migration: `npx ts-node scripts/migrate-itinerary-data.ts` (one-time, populates normalized tables from blob)

## Multi-Plan
All plans live in the `plans` table in Turso (no local JSON files).
Plan ID: `tokyo-2026`, `kyoto-2026`, etc. CLI defaults to `tokyo-2026`; use `--plan-id <id>` for others.

## Trip Dashboard (Cloudflare Worker)

Live web dashboard at `workers/trip-dashboard/` — reads directly from Turso DB, always up-to-date.

```
Browser → Cloudflare Worker (SSR HTML) → Turso HTTP Pipeline API → normalized tables + plans (fallback)
```

- **SSR-only** — zero client-side JS, no framework, no token/secret in HTML output
- **Mobile-first** — phone-optimized day cards with weather, transit, meals
- **Default ZH** — Traditional Chinese by default; `?lang=en` for English
- **ZH content** — `src/zh-content.ts` provides Tokyo-specific Chinese content, gated on `active_destination === 'tokyo_2026'`
- **Multi-plan** — each plan accessed via `?plan=<slug>` (e.g., `tokyo-2026`, `kyoto-2026`). Slug derived from `active_destination` (underscores → hyphens). Root `/` shows contact message, not a default plan.
- **Plan nav** — hidden by default; add `&nav=1` to show pill-style plan switcher (plan list from DB via `listPlans()`)
- **Routes**: `/?plan=<slug>` (dashboard), `/?plan=<slug>&lang=en` (EN), `/api/plan/<id>` (raw JSON), `/` (contact page)
- **Secrets**: `TURSO_URL` + `TURSO_TOKEN` via `wrangler secret put` (server-side only, never sent to browser)
- **Self-contained** — no dependency on `src/` code, own `package.json` + `tsconfig.json`
- **Live URLs**: `https://trip-dashboard.yanggf.workers.dev/?plan=tokyo-2026` | `/?plan=kyoto-2026`
- **Itinerary formats**: Supports both session-based (Tokyo) and schedule-based (Kyoto) formats. See `src/skills/travel-shared/references/itinerary-formats.md`

### Dashboard Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Itinerary shows blank/empty | Schedule-based format not converted | Check `render.ts` handles both formats |
| Wrong plan content | Plan not synced to Turso | Run `npm run db:seed:plans` |
| "Plan not found" error | Plan ID mismatch (underscore vs hyphen) | URL uses `tokyo-2026`, DB uses `tokyo_2026` |
| ZH content not showing | `isTokyoPlan` gate | Only Tokyo has ZH overrides; add to `zh-content.ts` for other plans |
| Weather missing | Weather not fetched | Run `npm run travel -- fetch-weather --dest <slug>` |

```bash
cd workers/trip-dashboard

# Local dev
unset CLOUDFLARE_API_TOKEN && npx wrangler dev
# → http://localhost:8787/?plan=tokyo-2026 | http://localhost:8787/?plan=kyoto-2026

# Deploy
unset CLOUDFLARE_API_TOKEN && npx wrangler deploy

# Set secrets (one-time, or pipe from .env)
TURSO_URL=$(grep '^TURSO_URL=' ../../.env | cut -d= -f2-) && unset CLOUDFLARE_API_TOKEN && npx wrangler secret put TURSO_URL <<< "$TURSO_URL"
TURSO_TOKEN=$(grep '^TURSO_TOKEN=' ../../.env | cut -d= -f2-) && unset CLOUDFLARE_API_TOKEN && npx wrangler secret put TURSO_TOKEN <<< "$TURSO_TOKEN"
```

## Build Gate
Pre-commit: `npm run typecheck`. Install: `npm run hooks:install`

## Next Steps

### Tokyo (Feb 13-17) — departs in 2 days
1. **Book teamLab Borderless** — Feb 15 visit, OVERDUE (book-by was Feb 10)
2. Book Limousine Bus — low-risk, can buy day-of at NRT T2
3. Restaurant reservations
4. Fetch weather forecast (within 16-day window now)

### Kyoto (Feb 24-28)
1. Book Hozugawa River Boat Ride (Day 3)
2. Restaurant reservations
3. Fetch weather forecast (available ~Feb 12)
