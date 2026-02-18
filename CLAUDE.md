# Japan Travel Project

## Trip Details
- **Dates**: February 13-17, 2026 (confirmed, booked)
- **Active Destination**: Tokyo, Japan
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

- **Turso cloud is sole source of truth** — fully normalized, no JSON blobs
- **No file-based state** — `StateManager` constructor throws without `repo` or `plan`; all legacy file I/O removed
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
npm run scraper:pipeline                       # Doctor + batch + import (end-to-end)
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
npm run travel -- swap-days <dayA> <dayB> [--dest slug]
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
- **Plan core**: `plans` (PK=plan_id, `version` monotonic counter — no JSON blobs), `plan_metadata`, `plan_destinations`, `destination_details`, `destination_cities`
- **Dates/Status**: `date_anchors`, `process_statuses`, `cascade_dirty_flags`, `plan_root_date_anchor`
- **Offers**: `plan_offers`, `plan_offer_flights`, `plan_offer_hotels`, `plan_offer_date_pricing`, `plan_offer_selection`, `plan_offer_best_value`, `plan_offer_provenance` (source audit trail), `plan_offer_warnings`
- **Itinerary**: `itinerary_days` (+ weather + `theme_zh`), `itinerary_sessions` (+ `focus_zh`, `transit_notes_zh`, `meals_zh_json`, `activities_zh_json`), `activities`, `itinerary_metadata` (+ `transit_summary_zh`), `day_route_segments`, `day_landmarks`, `session_meals`, `activity_tags`
- **Transport**: `flight_legs` (PK: plan_id+destination+direction+leg_order), `airport_transfers`, `airport_transfer_candidates`, `transportation_extras`
- **Accommodation**: `hotels` (+ `name_zh`), `hotel_access_lines`, `accommodation_location_zone`
- **Cascade**: `cascade_triggers`, `cascade_global_state`, `plan_schema_contract`, `plan_process_precedence`, `plan_budget`
- **Event log**: `event_log_state`, `event_log_global_processes`, `event_log_destinations`, `event_log_dest_processes`, `event_log_process_events`
- **Bookings**: `bookings_current` (flat rows: package/transfer/activity), `bookings_events` (audit)
- **Operation tracking**: `operation_runs` (audit trail: run_id, plan_id, command_type, status, version_before/after, timestamps)
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
- **ZH content** — All Chinese content stored in DB (`theme_zh`, `focus_zh`, `activities_zh_json`, `meals_zh_json`, `transit_notes_zh` on normalized tables). No hardcoded content in Worker code. Content updates take effect instantly without redeploy.
- **Multi-plan** — each plan accessed via `?plan=<slug>` (e.g., `tokyo-2026`, `kyoto-2026`). Slug derived from `active_destination` (underscores → hyphens). Root `/` shows plan index page listing all plans.
- **Plan nav** — hidden by default for privacy (shareable links show single plan only); add `&nav=1` to show pill-style plan switcher (plan list from DB via `listPlans()`)
- **Flight links** — Flight numbers in booking summary are clickable Google search links (opens new tab)
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
| ZH content not showing | Missing `_zh` columns in DB | Check `itinerary_sessions.focus_zh` etc. are populated for the destination |
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

### Tokyo (Feb 13-17) — departs today
1. **Book teamLab Borderless** — Feb 15 visit, OVERDUE (book-by was Feb 10)
2. ~~Book Limousine Bus~~ Arrival: Skyliner + 山手線 (booked); Departure: 海鷗線 + 淺草線 (booked)
3. Restaurant reservations
4. ~~Fetch weather forecast~~ ✅ Done (feels-like: 體感 -1.9–14.9°C, rain Day 4-5)
5. ~~Per-segment maps~~ ✅ Done (transit/walking per route segment, Tokyo+Kyoto)
6. Set `GOOGLE_MAPS_KEY` worker secret for embedded maps (optional)

### Kyoto (Feb 24-28)
1. Book Hozugawa River Boat Ride (Day 3)
2. Restaurant reservations
3. Fetch weather forecast — add `kyoto_2026` to `data/destinations.json` first
4. Set `GOOGLE_MAPS_KEY` worker secret for embedded maps (optional)
