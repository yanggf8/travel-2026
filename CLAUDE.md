# Japan Travel Project

## Trip Details
- **Next Dates**: February 24-28, 2026 (Kyoto, confirmed, booked)
- **Active Destination**: Kyoto, Japan (Tokyo Feb 13-17 ✅ completed)
- **Schema**: `4.2.0` — Destination-scoped with canonical offer model

## Architecture

### Data Model
```
Turso normalized tables (no JSON blobs)
├── plan_metadata                  # plan_id, schema_version, active_destination
├── plan_destinations              # slug, display_name per plan
├── destination_details            # region, country, airport_code
├── destination_cities             # city list per destination
├── date_anchors                   # P1 confirmed dates (start, end, days)
├── process_statuses               # per-destination process status
├── cascade_dirty_flags            # per-destination dirty flags
├── plan_offers                    # normalized offer records
├── plan_offer_date_pricing        # price per date per offer
├── plan_offer_selection           # chosen offer + date
├── flight_legs                    # outbound/return per destination
├── hotels                         # hotel name, access, check-in
├── hotel_access_lines             # hotel access directions
├── airport_transfers              # arrival/departure transfer info
├── airport_transfer_candidates    # transfer options
├── itinerary_days                 # day cards + weather
├── itinerary_sessions             # morning/afternoon/evening
├── activities                     # per-session activities + booking info
├── itinerary_metadata             # transit_summary, timestamps
└── (+ supporting: budget, triggers, contracts, event_log_*)
```

### Cascade Rules
| Trigger | Reset | Scope |
|---------|-------|-------|
| `active_destination_change` | `process_5_*` | new destination |
| `process_1_date_anchor_change` | `process_3_*`, `process_4_*`, `process_5_*` | all destinations |
| `process_2_destination_change` | `process_3_*`, `process_4_*`, `process_5_*` | current destination |
| `process_3_4_packages_selected` | populate P3+P4 from chosen offer | current destination |

### Data Flow
`URL → scrape (Playwright) → normalize (CanonicalOffer[]) → Turso import → selectOffer() → cascade (populate P3+P4) → save() (normalized tables → bookings sync)`

Canonical offer schema: `src/state/types.ts`. Skill contracts: `src/contracts/skill-contracts.ts` (v2.0.0).

### Repository Architecture (v4.1.0)
```
CLI / Skills / Dashboard
        ↓ commands
   StateManager          ← state machine: validate, transition, cascade (DB-only, no file I/O)
        ↓ repository calls
   StateRepository       ← interface (abstract)
        ↓
   TursoRepository       ← reads all data in one batch HTTP request (38 queries → 1 round-trip)
        ↓
   PlanRepository        ← in-memory plan object + write-back via syncNormalizedTables()
```

```
WRITE:  mutate → await save() → write normalized tables (blocking) → sync bookings+events (fire-and-forget)
READ:   await StateManager.create() → TursoRepository.create() → executeBatch(38 queries) → assemblePlan() → memory
```

- **Turso cloud is sole source of truth** — fully normalized, no JSON blobs, no config JSON files
- **No file-based state** — `StateManager` constructor throws without `repo` or `plan`; all legacy file I/O removed
- **Destination config in DB** — `destination_config` table (replaces `data/destinations.json`); loaded at startup via `loadDestinationConfigFromDb()` into in-memory cache; all sync APIs (`getDestinationConfig()` etc.) read from cache
- **28+ normalized tables** — see Data Model above
- **`flight_legs` table** — fully normalized flight data with `departure_terminal`/`arrival_terminal` columns
- `StateManager.create()` is async factory — reads from normalized tables via `TursoRepository.create()`
- `StateManager.saveWithTracking(cmd, summary)` wraps `save()` with operation audit trail in `operation_runs` table
- `plans.version` is a monotonic counter bumped on each save (audit trail only)
- `dispatch(command)` entry point — 25 command types as discriminated union
- Plan ID: `"<trip-id>"` (e.g., `tokyo-2026`, `kyoto-2026`)
- Tests use `skipSave: true` with `options.plan` passed in — DB calls skipped entirely
- DB info messages use `console.error` (stderr) to avoid polluting JSON output

## Agent-First Workflow

- Proactively run next logical step; only ask user when a preference materially changes the result
- Prefer `StateManager` methods / CLI wrappers over direct JSON edits
- Every output: current status, what changed, single best next action

### Skill Decision Tree
```
User intent                          → Skill / Action
──────────────────────────────────────────────────────
"plan a trip to [place]"             → Check destination_config in Turso
  destination exists?                   → /p1-dates (if dates not set)
  destination missing?                  → add to destination_config + /p2-destination
"set dates" / "change dates"         → /p1-dates
"which city" / "how many nights"     → /p2-destination
"find packages" / "search OTA"       → check-freshness first
  fresh data in Turso?                  → query-offers (show existing)
  stale/no data?                        → /p3p4-packages (scrape + auto-import)
"find flights only"                  → /p3-flights (uses /scrape-ota)
"compare offers"                     → read process_3_4_packages.results
"query offers"                       → npm run travel -- query-offers --plan-id <id> --dest <slug>
"import scraped files"               → npm run travel -- import-offers --dir scrapes --dest <slug>
"is data fresh"                      → npm run travel -- check-freshness --source <s>
"book separately"                    → /separate-bookings
"how many leave days"                → npm run leave-calc
"book this" / "select offer"         → npm run travel -- select-offer
"plan the days" / "itinerary"        → /p5-itinerary
"show bookings"                      → npm run travel -- query-bookings (from DB)
"show status"                        → npm run view:status
"show schedule"                      → npm run view:itinerary
"weather" / "forecast"               → npm run travel -- fetch-weather [--dest slug] [--all]
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
| `/booking-confirmation` | `src/skills/booking-confirmation/SKILL.md` | Post-booking verification workflow |
| `/post-pull-fix` | `src/skills/post-pull-fix/SKILL.md` | Health checks after git pull |
| `/weather-update` | `src/skills/weather-update/SKILL.md` | Fetch weather with pre-checks |
| `/deploy-dashboard` | `src/skills/deploy-dashboard/SKILL.md` | Deploy trip dashboard to CF Workers |
| `/pre-trip-checklist` | `src/skills/pre-trip-checklist/SKILL.md` | Pre-departure verification |
| `/new-destination` | `src/skills/new-destination/SKILL.md` | Add destination to config |

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
Flight:  Scoot TR874 TPE 13:55 → NRT T2 18:00 / TR875 NRT T1 19:55 → TPE 23:10
Hotel:   TAVINOS Hamamatsucho (light breakfast, JR Hamamatsucho 8min)
```

Airport transfers:
- Arrival: Skyliner + JR山手線 ¥2,720 (NRT → 日暮里 36min + 日暮里 → 浜松町 20min), status: booked
- Departure: 海鷗線+淺草線 ¥1,520 (竹芝 → 新橋 → 成田空港, ~100min), status: booked
Note: TR874 arrives NRT **Terminal 2**, TR875 departs NRT **Terminal 1**

### Itinerary (Feb 13-17)
| Day | Date | Morning | Afternoon | Evening |
|-----|------|---------|-----------|---------|
| 1 | Fri 13 | ✈️ TPE→NRT T2 | Arrival + Narita dinner | Hotel check-in |
| 2 | Sat 14 | **teamLab Borderless** | Asakusa (Senso-ji) | Harajuku |
| 3 | Sun 15 | Azabudai Hills | Roppongi + Shibuya | Roppongi |
| 4 | Mon 16 | KOMEHYO (Chanel) | Isetan omiyage | Omoide Yokocho |
| 5 | Tue 17 | Pack + Checkout | Shiodome area | ✈️ NRT T1→TPE |

**Book by Feb 10**: teamLab Borderless (https://www.teamlab.art/e/borderless-azabudai/)
**Skyliner**: https://www.keisei.co.jp/keisei/tetudou/skyliner/
**Limousine Bus**: https://www.limousinebus.co.jp/en/ (not used — took 海鷗線+淺草線 instead)

### BOOKED: Kyoto Feb 24-28
```
Package: liontravel_190620015 — TWD 23,348/person (46,696 for 2 pax)
Order:   2026-1311130
Flight:  Thai Lion Air SL396 TPE T1 09:00 → KIX T1 13:30 / SL397 KIX T1 13:30 → TPE T1 15:40
Hotel:   APA Hotel Kyoto Ekimae (APA京都站前, JR Kyoto Station 3min)
Includes: Kyoto Yumeyakata Kimono Experience (Day 4), JR Haruka round-trip, eSIM data
```

Airport transfers: JR Haruka Express ¥450/trip/person round-trip (KIX ↔ Kyoto Station, ~75min), included in package, status: booked

### Kyoto Itinerary (Feb 24-28)
⚠️ **Sagano Scenic Railway (嵯峨野トロッコ) CLOSED until March 1** — use JR山陰線 to 亀岡 instead
🍽️ **16:8 fasting** — eating window starts 11:00, no morning meals

| Day | Date | Morning | Afternoon | Evening |
|-----|------|---------|-----------|---------|
| 1 | Tue 24 | ✈️ SL396 TPE→KIX 09:00 | 13:30 KIX arrival, 16:30 Haruka→京都 | Kyoto Station / Ramen Alley ([tabelog](https://tabelog.com/tw/kyoto/A2601/A260101/)) |
| 2 | Wed 25 | 金閣寺 + 可選龍安寺 (subway→北大路) | 伏見稲荷 + 可選東福寺 | 先斗町, 晚餐, 預訂保津川遊船 |
| 3 | Thu 26 | JR山陰線→亀岡, **11:30 保津川遊船**→嵐山 | 嵐山: 竹林, 天龍寺, 渡月橋 | キモノフォレスト, 返回京都, **名代豬排 Katsukura** (JR京都伊勢丹 11F) |
| 4 | Fri 27 | Sleep in, 可選東寺, 11:30 東山區午餐 | 夢館kimono, 三年坂, 二年坂, 可選清水寺 | 祇園, Donki (UTENA 魔髮球), 預換Haruka回程票 |
| 5 | Sat 28 | 退房09:00, 09:30 Haruka→KIX | KIX T1 check-in (10:30開放) | ✈️ SL397 KIX→TPE 13:30 |

Day 3 保津川遊船: depart 京都 10:50 → JR山陰線 25min → 亀岡 → **11:30 boat** → 嵐山 13:30
Book: https://ars-hozugawa.triplabo.jp ¥4,100/person, book by day before

Weather check (as of **Feb 18, 2026**; Kyoto 10-day forecast):
- Tue, Feb 24: **17C / 9C**, precip chance **24%**
- Wed, Feb 25: **13C / 7C**, precip chance **60%** (rain risk)
- Thu, Feb 26: **14C / 6C**, precip chance **17%**
- Winter boat section can feel colder than forecast due to wind/splash; re-check 24h before.

## CLI Quick Reference
```bash
# === VIEWS ===
npm run view:status | view:itinerary | view:transport | view:bookings
npm run view:prices -- --flights scrapes/date-range-prices.json --hotel-per-night 3000 --nights 4 --package 40740

# === COMPARISON ===
npm run travel -- compare-offers --region osaka [--json]
npm run compare-trips -- --input data/osaka-trip-comparison.json [--detailed]  # file removed
npm run compare-dates -- --start 2026-02-24 --end 2026-02-28 --nights 4
npm run compare-true-cost -- --region kansai --pax 2 --date 2026-02-24

# === SCRAPING ===
npm run scraper:batch -- --dest kansai [--sources besttour,settour] [--date 2026-02-24 --type fit]
npm run scraper:doctor                         # Test all scrapers
npm run scraper:pipeline                       # Doctor + batch + import (end-to-end)
python scripts/scrape_date_range.py --depart-start 2026-02-24 --depart-end 2026-02-27 \
  --origin tpe --dest kix --duration 5 --pax 2 -o scrapes/date-range-prices.json

# === TURSO DB ===
npm run travel -- import-offers --dir scrapes --dest tokyo_2026 [--start 2026-02-13 --end 2026-02-17] [--dry-run]
npm run travel -- query-offers --plan-id tokyo-2026 --dest tokyo_2026 [--max-price 30000] [--json]
npm run travel -- query-offers --region kansai --start 2026-02-24 --end 2026-02-28 [--max-price 30000] [--json]
npm run travel -- check-freshness --source besttour --plan-id tokyo-2026 --dest tokyo_2026
npm run travel -- check-freshness --source besttour --region kansai
npm run db:import:turso -- --dir scrapes [--start 2026-02-24 --end 2026-02-28]   # legacy: writes offers table
npm run db:status:turso | db:migrate:turso | db:seed:plans

# === BOOKINGS ===
npm run travel -- sync-bookings [--dry-run]
npm run travel -- query-bookings --dest tokyo_2026 [--category activity --status pending]
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
npm run travel -- set-activity-title <day> <session> "<activity>" "<new_title>" [--plan-id <id>]
npm run travel -- set-session-time-range <day> <session> --start HH:MM --end HH:MM
npm run travel -- set-day-theme <day> [theme] [--zh "<zh_title>"] [--dest slug]
npm run travel -- set-route-segment <day> <sort_order> <from> <to> <mode> [--duration <min>] [--notes "<text>"] [--start-time HH:MM]
npm run travel -- set-session-zh <day> <session> [--zh "<focus_zh>"] [--transit-zh "<transit_notes_zh>"] [--activities-zh-json '[...]'] [--plan-id <id>]
npm run travel -- swap-days <dayA> <dayB> [--dest slug]
npm run travel -- fetch-weather [--dest slug] [--all]

# === OPERATION TRACKING ===
npm run travel -- run-status [run-id]
npm run travel -- run-list [--status completed|failed|started] [--limit N]
```

## Project Structure
```
/
├── data/
│   ├── holidays/taiwan-2026.json  # Holiday calendar
│   ├── hotel-areas.json           # Zone categorization (used by compare-true-cost)
│   └── transport-routes.json      # Transit routes (used by compare-true-cost)
├── scrapes/                       # Ephemeral scraper outputs (gitignored)
├── scripts/                       # Python scrapers + migration tools
│   └── hooks/pre-commit           # Runs typecheck + validate:data
├── workers/trip-dashboard/        # Cloudflare Worker — live trip dashboard
│   ├── wrangler.toml              # Worker config + secret bindings
│   ├── src/index.ts               # Request handler + router + favicon
│   ├── src/turso.ts               # Turso HTTP pipeline client (17-query pipeline)
│   ├── src/render.ts              # SSR HTML renderer (ZH from DB, no hardcoded content)
│   └── src/styles.ts              # Mobile-first inline CSS
├── src/
│   ├── cli/travel-update.ts       # Main CLI entry
│   ├── state/
│   │   ├── state-manager.ts       # State machine: validate, transition, cascade, dispatch() (DB-only)
│   │   ├── repository.ts          # StateRepository interface (StateReader + StateWriter)
│   │   ├── turso-repository.ts    # Reads all data in single batch request; delegates writes to PlanRepository
│   │   ├── plan-repository.ts     # In-memory plan + write-back via syncNormalizedTables()
│   │   ├── plan-assembler.ts      # Assembles plan object from table row arrays
│   │   ├── sql-helpers.ts         # Shared SQL helpers (rowsToObjects, sqlText/Int/Real/Bool)
│   │   ├── commands.ts            # 25-type Command discriminated union
│   │   ├── types.ts               # Domain types, status transitions
│   │   ├── itinerary-manager.ts   # Itinerary domain logic
│   │   ├── offer-manager.ts       # Offer domain logic
│   │   ├── transport-manager.ts   # Transport domain logic
│   │   └── event-query.ts         # Event log queries
│   ├── config/                    # loader.ts, constants.ts
│   ├── contracts/skill-contracts.ts
│   ├── cascade/runner.ts          # Cascade logic (DB-only via runAsync)
│   ├── services/turso-service.ts  # DB access layer (all Turso queries go through here)
│   ├── utils/                     # flight-normalizer, leave-calculator
│   ├── skills/                    # Skill SKILL.md files + references
│   ├── scrapers/                  # Registry + base classes + scrape-file-parser.ts
│   ├── validation/                # Itinerary validator
│   └── types/result.ts            # Result<T,E>
└── tests/integration/
```

Config: `src/config/constants.ts` (defaults/exchange rates), `src/skills/travel-shared/references/ota-knowledge.json` (baggage rules).
Destination/OTA config: stored in Turso (`destination_config`, `ota_sources`, `origin_config`, `global_config` tables — no JSON files).
Note: `ref_path`/`scraper_script` must be repo-relative paths.

## Turso DB
```
Database: travel-2026 | Region: aws-ap-northeast-1 | Creds: .env (gitignored)
```

Tables:
- **Plan core**: `plans` (PK=plan_id, `version` monotonic counter — no JSON blobs), `plan_metadata`, `plan_destinations`, `destination_details`, `destination_cities`
- **Dates/Status**: `date_anchors`, `process_statuses`, `cascade_dirty_flags`, `plan_root_date_anchor`
- **Offers**: `plan_offers`, `plan_offer_flights`, `plan_offer_hotels`, `plan_offer_date_pricing`, `plan_offer_selection`, `plan_offer_best_value`, `plan_offer_provenance` (source audit trail), `plan_offer_warnings`
- **Itinerary**: `itinerary_days` (+ weather + `theme_zh`), `itinerary_sessions` (+ `focus_zh`, `transit_notes_zh`, `meals_zh_json`, `activities_zh_json`), `activities`, `itinerary_metadata` (+ `transit_summary_zh`), `day_route_segments` (+ `duration_min`, `notes`, `start_time`), `day_landmarks`, `session_meals`, `activity_tags`
- **Transport**: `flight_legs` (PK: plan_id+destination+direction+leg_order), `airport_transfers`, `airport_transfer_candidates`, `transportation_extras`
- **Accommodation**: `hotels` (+ `name_zh`), `hotel_access_lines`, `accommodation_location_zone`
- **Cascade**: `cascade_triggers`, `cascade_global_state`, `plan_schema_contract`, `plan_process_precedence`, `plan_budget`
- **Event log**: `event_log_state`, `event_log_global_processes`, `event_log_destinations`, `event_log_dest_processes`, `event_log_process_events`
- **Bookings**: `bookings_current` (flat rows: package/transfer/activity), `bookings_events` (audit)
- **Operation tracking**: `operation_runs` (audit trail: run_id, plan_id, command_type, status, version_before/after, timestamps)
- **Global config** (not plan-scoped): `destination_config` (slug PK, coordinates/timezone/airports), `origin_config` (taiwan origin), `global_config` (default_destination, default_origin), `ota_sources` (OTA registry — replaces ota-sources.json)
- **Other**: `offers`, `destinations`, `events`, `bookings`
- **Dead**: `flights` (old JSON blob table — no writes, kept for reference only)

Schema reference: `scripts/schema.sql` (read-only DDL reference, extracted from migration script)
Schema/migration: `npm run db:migrate:turso` (creates all tables idempotently)
Seed: `npm run db:seed:plans` (one-time, already run)

### DB Operation Decision
- **Reusable operation** (editing itinerary content, updating themes, managing activities) → build a UI/CLI interface
- **One-shot operation** (migration, schema change, data backfill) → direct SQL via turso-exec is acceptable
Before running raw SQL for content edits, ask: "Will this be done again?" If yes, build the interface first.

## Multi-Plan
All plans live in the `plans` table in Turso (no local JSON files).
`plan_id` uses hyphens (`tokyo-2026`), `destination` uses underscores (`tokyo_2026`). Converters: `toPlanId()` / `toDestSlug()` in `src/utils/plan-id.ts`.
CLI defaults to `tokyo-2026`; use `--plan-id <id>` for others.

## Trip Dashboard (Cloudflare Worker)

Live web dashboard at `workers/trip-dashboard/` — reads directly from Turso DB, always up-to-date.

```
Browser → Cloudflare Worker (SSR HTML) → Turso HTTP Pipeline API → 15 normalized tables → assemble plan object → render
```

- **SSR-only** — zero client-side JS in read mode; minimal inline JS in edit mode only
- **Edit mode** — `?edit=TOKEN` activates inline editing when TOKEN matches `ADMIN_TOKEN` secret. Pencil icons appear next to editable fields (theme, focus, activities, meals, transit notes). POSTs to `/api/edit` with token in JSON body. Set token: `wrangler secret put ADMIN_TOKEN`
- **Mobile-first** — phone-optimized day cards with weather (including feels-like temperature), transit, meals
- **Default ZH** — Traditional Chinese by default; `?lang=en` for English
- **ZH content** — All Chinese content stored in DB (`theme_zh`, `focus_zh`, `activities_zh_json`, `meals_zh_json`, `transit_notes_zh` on normalized tables). No hardcoded content in Worker code. Content updates take effect instantly without redeploy. Use `set-day-theme --zh` for day themes, `set-session-zh` for session focus/transit/activities, `set-route-segment` for Chinese place names. For bulk new-destination ZH population: copy `scripts/set-kyoto-zh-sessions-v2.ts` pattern (parameterized Turso pipeline queries — required for Unicode/emoji content).
- **Anti-translate** — `lang="zh-TW"` + `<meta name="google" content="notranslate">` prevents browser auto-translation of the ZH page.
- **Multi-plan** — each plan accessed via `?plan=<slug>` (e.g., `tokyo-2026`, `kyoto-2026`). Slug derived from `active_destination` (underscores → hyphens). Root `/` shows plan index page listing all plans.
- **Plan nav** — hidden by default for privacy (shareable links show single plan only); add `&nav=1` to show pill-style plan switcher (plan list from DB via `listPlans()`)
- **Flight links** — Flight numbers in booking summary are clickable Google search links (opens new tab)
- **Activity links** — `https://` URLs embedded in activity text are auto-linkified (`renderActivityText()`); `\n` in activity text renders as `<br>`
- **Day card accents** — colored left border by day type: blue (arrival), green (full day), amber (departure)
- **Routes**: `/` (plan index), `/?plan=<slug>` (single plan, shareable), `/?plan=<slug>&nav=1` (with plan switcher), `/?plan=<slug>&lang=en` (EN), `/?plan=<slug>&edit=TOKEN` (edit mode), `/api/plan/<id>` (raw JSON), `POST /api/edit` (write field)
- **Maps links** — Per-segment Google Maps direction links (transit/walking/driving) for every stop. Route segments stored in `day_route_segments` table, landmarks in `day_landmarks` table. Transit pill text must use place names, not service names (e.g., `成田T2 → 日暮里` not `Skyliner → 日暮里`)
- **Secrets**: `TURSO_URL` + `TURSO_TOKEN` + `GOOGLE_MAPS_KEY` (optional) + `ADMIN_TOKEN` (edit mode) via `wrangler secret put` (server-side only, never sent to browser — except Maps key which is browser-visible by design; restrict via GCP Console referrer policy)
- **Self-contained** — no dependency on `src/` code, own `package.json` + `tsconfig.json`
- **Live URLs**: `https://trip-dashboard.yanggf.workers.dev/?plan=tokyo-2026` | `/?plan=kyoto-2026`
- **Itinerary formats**: Supports both session-based (Tokyo) and schedule-based (Kyoto) formats. See `src/skills/travel-shared/references/itinerary-formats.md`

### Dashboard Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Itinerary shows blank/empty | Schedule-based format not converted | Check `render.ts` handles both formats |
| Wrong plan content | Plan not synced to Turso | Run `npm run db:seed:plans` |
| "Plan not found" error | Plan ID mismatch (underscore vs hyphen) | URL uses `tokyo-2026`, DB uses `tokyo_2026` |
| ZH content not showing | Missing `_zh` columns in DB | Run `set-session-zh` CLI per session, or bulk-populate via `scripts/set-kyoto-zh-sessions-v2.ts` pattern |
| ZH UPDATE silently fails (rows_affected=0) | Inline SQL with Unicode/emoji fails encoding | Use parameterized Turso queries: `args:[{type:"text",value:"..."},{type:"integer",value:"1"}]` — integer value must be a string |
| Weather missing | Weather not fetched | Run `npm run travel -- fetch-weather --dest <slug>` |
| Maps embed not showing | No `GOOGLE_MAPS_KEY` secret | `wrangler secret put GOOGLE_MAPS_KEY` (restrict key to Maps Embed API + referrer in GCP Console) |

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

### Tokyo (Feb 13-17) — ✅ completed

### Kyoto (Feb 24-28)
1. **Book Hozugawa River Boat** — Day 3, 11:30 slot (https://ars-hozugawa.triplabo.jp)
2. ~~Restaurant reservations~~ ✅ Partial — Day 1 & Day 3 evening Tabelog link added; Katsukura (名代豬排 JR京都伊勢丹 11F) added to Day 3 dinner
3. ~~Fetch weather forecast~~ ✅ Done (Feb 19: Day1 23%/Day2 71%/Day3 42%/Day4 32%/Day5 52%) — re-run `npm run travel -- fetch-weather --all` closer to trip
4. Set `GOOGLE_MAPS_KEY` worker secret for embedded maps (optional)
