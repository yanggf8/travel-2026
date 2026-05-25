# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> Routing user requests → jump to **Skill Decision Tree**.
> Commands → see **CLI Quick Reference** (full list in `docs/reference/CLI.md`).
> Past trip details (Tokyo, Kyoto Feb 2026) → `docs/trips/`.

# Japan Travel Project

## Trip Details
- **Schema**: `4.2.0` — destination-scoped with canonical offer model
- **Completed**: Tokyo Feb 13-17, Kyoto Feb 24-28 (see `docs/trips/`)
- **Active**: no upcoming trip locked; use `/stage0-research` to start a new one
- **Package name caveat**: `package.json` `name` is `yokohama-travel-2026` (legacy, project is Japan-wide)

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
├── days                           # day cards + weather
├── timesofday                     # morning/noon/afternoon/evening
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

> **Target pattern (ADR-001)**: each command = one targeted SELECT (validate) + one targeted UPDATE/INSERT. No assembled plan object. No flush. Dashboard reads Turso directly — StateManager internals have zero impact on web pages. See `src/skills/travel-shared/references/architecture-decisions.md`.

- **Turso cloud is sole source of truth** — fully normalized, no JSON blobs, no config JSON files
- **No file-based state** — `StateManager` constructor throws without `repo` or `plan`; all legacy file I/O removed
- **Destination config in DB** — `destination_config` table (replaces `data/destinations.json`); loaded at startup via `loadDestinationConfigFromDb()` into in-memory cache; all sync APIs (`getDestinationConfig()` etc.) read from cache
- **28+ normalized tables** — see Data Model above
- **`flight_legs` table** — fully normalized flight data with `departure_terminal`/`arrival_terminal` columns
- `StateManager.createFromPlanId(planId)` is the async factory — reads normalized tables via `TursoRepository.create()`
- `StateManager.saveWithTracking(cmd, summary)` wraps `save()` with operation audit trail in `operation_runs` table
- `plans.version` is a monotonic counter bumped on each save (audit trail only)
- `dispatch(command)` entry point — 25 command types as discriminated union
- Plan ID: `"<trip-id>"` (e.g., `tokyo-2026`, `kyoto-2026`)
- Tests use `skipSave: true` with `options.plan` passed in — DB calls skipped entirely
- DB info messages use `console.error` (stderr) to avoid polluting JSON output

## Development

### Setup
```bash
npm install                   # also runs postinstall: git hooks + Playwright check
npm run scraper:setup         # install Playwright browsers (if postinstall warned)
```

### Tests
```bash
npm test                      # integration/regression tests only (vitest)
npm run test:watch            # watch mode
npm run test:coverage         # coverage report (focused on src/state/)
```

- **Integration-only** — no unit test suite; tests live in `tests/integration/`
- **`skipSave: true`** — tests pass `options.plan` to StateManager, skipping all DB calls
- **Vitest config**: `vitest.config.ts` — node environment, only includes `tests/integration/**/*.test.ts`

### Pre-commit
```bash
npm run typecheck             # tsc --noEmit (runs automatically via git hook)
npm run validate:data         # data integrity check
npm run doctor                # full system health check
```
Pre-commit hook (installed by `postinstall`) runs `typecheck` + `validate:data`.

### Docs
- `docs/API.md` — complete API reference
- `docs/EXTENDING.md` — how to add destinations, OTAs, validators
- `docs/SKILL_TEMPLATE.md` — skill authoring guide
- `docs/plans/` — implementation plans for major refactors
- `docs/plans/2026-05-22-new-planning-flow.md` — **adopted** research-first staged planning model (date/destination/flight explored together before plan lock). Existing P1–P5 skills remain implementation tools inside the stages.

## Agent-First Workflow

- Proactively run next logical step; only ask user when a preference materially changes the result
- Prefer `StateManager` methods / CLI wrappers over direct JSON edits
- Every output: current status, what changed, single best next action

### Skill Decision Tree
```
User intent                          → Skill / Action
──────────────────────────────────────────────────────
"plan a trip to [place]"             → Stage 0/1 staged flow
  loose dates/destination/price?         → /stage0-research
  fixed dates + destination?             → create/verify plan, then /p1-dates + /p2-destination
  destination missing?                  → /new-destination, then continue Stage 0/1
"cheapest week to go to X"           → /stage0-research (pre-lock triangle research)
"Osaka or Tokyo, depends on price"   → /stage0-research (compare destinations + dates + price together)
"what dates are cheapest"            → /stage0-research
"set dates" / "change dates"         → /p1-dates
"which city" / "how many nights"     → /p2-destination
"lock this Stage 0 candidate"        → npm run travel -- stage0-adopt <candidate_id> <new_plan_id> --create-plan --dest <slug>
"draft the trip" / "rough itinerary" → /stage1-itinerary-draft
"find packages" / "search OTA"       → /stage2-shop-transport (check freshness first)
  fresh data in Turso?                  → query-offers (show existing)
  stale/no data?                        → /p3p4-packages (scrape + auto-import)
"find flights only"                  → /stage2-shop-transport (uses /p3-flights)
"compare offers"                     → /stage2-shop-transport
"query offers"                       → npm run travel -- query-offers --plan-id <id> --dest <slug>
"import scraped files"               → npm run travel -- import-offers --dir scrapes --dest <slug>
"is data fresh"                      → npm run travel -- check-freshness --source <s>
"book separately"                    → /stage2-shop-transport (uses /separate-bookings)
"how many leave days"                → npm run leave-calc
"book this" / "select offer"         → npm run travel -- select-offer
"plan the days" / "itinerary"        → /stage3-expand-itinerary
"show bookings"                      → npm run travel -- query-bookings (from DB)
"show status"                        → npm run view:status
"show schedule"                      → npm run view:itinerary
"weather" / "forecast"               → npm run travel -- fetch-weather [--dest slug] [--all]
User provides OTA URL                → /scrape-ota (see URL Routing)
User provides booking confirmation   → npm run travel -- set-activity-booking
"deploy dashboard" / "publish trip"  → /stage4-publish-dashboard
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
| `/stage0-research` | `src/skills/stage0-research/SKILL.md` | Pre-lock triangle research (date/destination/flight) |
| `/stage1-itinerary-draft` | `src/skills/stage1-itinerary-draft/SKILL.md` | Rough day-by-day itinerary draft after dates/destination lock |
| `/stage2-shop-transport` | `src/skills/stage2-shop-transport/SKILL.md` | Compare direct flights vs packages and choose booking path |
| `/stage3-expand-itinerary` | `src/skills/stage3-expand-itinerary/SKILL.md` | Detailed booking-aware itinerary expansion |
| `/stage4-publish-dashboard` | `src/skills/stage4-publish-dashboard/SKILL.md` | Explicit dashboard publish and verification |

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

Completed trips — full bookings, itinerary, and weather notes archived:
- **Tokyo Feb 13-17** → `docs/trips/2026-tokyo.md`
- **Kyoto Feb 24-28** → `docs/trips/2026-kyoto.md`

No upcoming trip locked. Plan status for any active plan: `npm run view:status`.

## CLI Quick Reference

Most-used commands inline; full reference (mutations, comparison, scraping internals, Stage 0 aggregator handoff) in **`docs/reference/CLI.md`**.

```bash
# Views (run any one)
npm run travel -- plans                          # list DB plans and date anchors
npm run view:status                              # booking overview
npm run view:itinerary                           # daily plan
npm run view:transport                           # transport summary
npm run view:bookings                            # booking ledger
npm run travel -- status --travel-date 2026-06-20

# Stage 0 (pre-plan triangle research)
npm run travel -- stage0-init --origin TPE --start 2026-06-18 --end 2026-06-20 \
  --dest KIX:"Osaka (KIX)" --dest NRT:"Tokyo (NRT)" --nights 6 --nights 7 [--pax 2]
python scripts/stage0_research.py --run <run_id>
npm run travel -- stage0-compare --run <run_id>
npm run travel -- stage0-adopt <candidate_id> <plan_id> --create-plan --dest <slug>

# Offers (Turso)
npm run travel -- import-offers --dir scrapes --dest tokyo_2026 [--start ... --end ...] [--dry-run]
npm run travel -- query-offers --plan-id tokyo-2026 --dest tokyo_2026 [--max-price 30000]
npm run travel -- check-freshness --source besttour --plan-id tokyo-2026 --dest tokyo_2026

# Bookings
npm run travel -- sync-bookings [--dry-run]
npm run travel -- query-bookings --dest tokyo_2026 [--category activity --status pending]
npm run travel -- validate-itinerary --dest tokyo_2026

# Scraping
npm run scraper:pipeline                         # doctor + batch + import (end-to-end)
npm run scraper:batch -- --dest kansai [--sources besttour,settour]

# Mutations — only the 4 most common shown here.
# Full list (set-airport-transfer, set-activity-time, set-day-theme, set-route-segment,
# set-tod-zh, delete-activity, swap-days, run-status, check-booking-integrity, …)
# lives in docs/reference/CLI.md. Add new mutation examples THERE, not here.
npm run travel -- set-dates 2026-02-13 2026-02-17
npm run travel -- select-offer <offer-id> <date>
npm run travel -- set-activity-booking <day> <session> "<activity>" <status> [--ref "..."]
npm run travel -- fetch-weather [--dest slug] [--all]

# DB + tests (run any one)
npm run db:status:turso                          # show DB state
npm run db:migrate:turso                         # create/upgrade tables (idempotent)
npm run db:seed:plans                            # one-time plan seed
npm test
npm run validate:data                            # data integrity check
npm run doctor                                   # full system health check
```

Plan resolution: `--plan-id` and `$TRAVEL_PLAN_ID` win. Without those, the CLI uses `--travel-date`, `--travel-start/--travel-end`, or exactly one active or upcoming DB date anchor/planning window. Use `--travel-*` for plan selection; plain `--start/--end` are command-specific filters (e.g. offer search ranges). If several plans match, the CLI fails with a plan list instead of silently loading a legacy default.

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
│   ├── src/turso.ts               # Turso HTTP pipeline client (18-query pipeline)
│   ├── src/render.ts              # SSR HTML renderer (ZH from DB, no hardcoded content)
│   └── src/styles.ts              # Mobile-first inline CSS
├── src/
│   ├── cli/
│   │   ├── travel-update.ts       # Thin CLI entry — loads command registry, resolves plan
│   │   ├── commands/              # ~26 command modules (one per command) + registry.ts
│   │   └── shared/                # args, output, plan-resolver, validation helpers
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
│   │   └── weather-service.ts
│   ├── utils/                     # date-utils, flight-normalizer, holiday-calculator, leave-calculator, plan-id
│   ├── skills/                    # Skill SKILL.md files + references
│   ├── scrapers/                  # Registry + base classes + scrape-file-parser.ts
│   ├── questionnaire/             # Trip questionnaire definitions
│   ├── templates/                 # destination-template.json, project-init.ts
│   ├── validation/                # Itinerary validator
│   └── types/result.ts            # Result<T,E>
├── tests/integration/
└── docs/                          # API.md, EXTENDING.md, SKILL_TEMPLATE.md, plans/
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
- **Itinerary**: `days` (+ weather + `theme_zh`), `timesofday` (+ `focus_zh`, `transit_notes_zh`, `meals_zh_json`, `activities_zh_json`), `activities`, `itinerary_metadata` (+ `transit_summary_zh`), `day_route_segments` (+ `duration_min`, `notes`, `start_time`), `day_landmarks`, `session_meals`, `activity_tags`
- **Transport**: `flight_legs` (PK: plan_id+destination+direction+leg_order), `airport_transfers`, `airport_transfer_candidates`, `transportation_extras`
- **Accommodation**: `hotels` (+ `name_zh`), `hotel_access_lines`, `accommodation_location_zone`
- **Cascade**: `cascade_triggers`, `cascade_global_state`, `plan_schema_contract`, `plan_process_precedence`, `plan_budget`
- **Event log**: `event_log_state`, `event_log_global_processes`, `event_log_destinations`, `event_log_dest_processes`, `event_log_process_events`
- **Bookings**: `bookings_current` (flat rows: package/transfer/activity), `bookings_events` (audit)
- **Operation tracking**: `operation_runs` (audit trail: run_id, plan_id, command_type, status, version_before/after, timestamps)
- **Stage 0 research** (unscoped, keyed by `run_id`): `stage0_research_runs`, `stage0_research_destinations`, `stage0_research_durations`, `stage0_candidates`, `stage0_candidate_flights`, `stage0_scrape_attempts` — pre-plan triangle-research domain (see `docs/superpowers/specs/2026-05-22-stage0-triangle-research-design.md`)
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
- **ZH content** — All Chinese content stored in DB (`theme_zh`, `focus_zh`, `activities_zh_json`, `meals_zh_json`, `transit_notes_zh` on normalized tables). No hardcoded content in Worker code. Content updates take effect instantly without redeploy. Use `set-day-theme --zh` for day themes, `set-tod-zh` (alias: `set-session-zh`) for session focus/transit/activities, `set-route-segment` for Chinese place names. For bulk new-destination ZH population: copy `scripts/set-kyoto-zh-sessions-v2.ts` pattern (parameterized Turso pipeline queries — required for Unicode/emoji content).
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
| ZH content not showing | Missing `_zh` columns in DB | Run `set-tod-zh` CLI per session, or bulk-populate via `scripts/set-kyoto-zh-sessions-v2.ts` pattern |
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

### Engineering — Stage 0 Triangle Research ✅
Spec: `docs/superpowers/specs/2026-05-22-stage0-triangle-research-design.md`
Plan: `docs/superpowers/plans/2026-05-22-stage0-triangle-research.md`

1. **6 unscoped Turso tables** ✅ — `stage0_research_runs`, `_destinations`, `_durations`, `stage0_candidates`, `stage0_candidate_flights`, `stage0_scrape_attempts` (keyed by `run_id`, not `plan_id` — research exists before any plan)
2. **TS service layer** ✅ — `src/services/stage0-service.ts` owns all DB reads/writes; runs are immutable; ranking is `flight_total_twd ASC, leave_days ASC, depart_date ASC`
3. **5 CLI commands** ✅ — `stage0-init` (seeds pending attempts), `stage0-export`/`stage0-import` (aggregator handoff), `stage0-compare`, `stage0-adopt` (can link to existing plans or `--create-plan --dest <slug>` to seed P1/P2). All `requiresState: false` (pre-plan)
4. **Python aggregator** ✅ — `scripts/stage0_research.py` performs zero Turso I/O of its own; reads via `stage0-export`, writes via `stage0-import` — all SQL stays in TypeScript under `sql-helpers.ts` escaping
5. **`/stage0-research` orchestration skill** ✅ — `src/skills/stage0-research/SKILL.md`, owns pre-lock research (`requires_processes: []`)
6. **`db:exec` fix** ✅ — incidental but load-bearing: `scripts/turso-exec.ts` now splits semicolon-delimited input and surfaces per-statement errors (was silently swallowing both)
7. **Stage flow adopted** ✅ — Stage 0 now hands off to a seeded plan via `stage0-adopt --create-plan --dest`; CLAUDE.md routes new planning through Stage 0→Stage 4 while keeping P1–P5 skill names as compatibility labels

### Engineering — Itinerary DAL Refactor
Plan: `docs/plans/2026-03-01-itinerary-dal-refactor.md`

1. **Phase A — Add `noon` session type** ✅ — `SessionType`, CLI arrays, DB migration, dashboard
2. **Phase B — Missing CLI commands** ✅ — `delete-activity`, `set-tod-focus`, `set-tod-zh`, `set-tod-time-range` aliases
3. **Phase C — DB table rename** ✅ — `itinerary_days` → `days`, `itinerary_sessions` → `timesofday`
4. **Phase D — Docs** ✅ — CLAUDE.md, skill SKILL.md files
5. **Post-implementation fixes** ✅ — `swap-days` now includes noon; `SessionTypeSchema`/`DEFAULTS.sessionOrder`/`itinerary-manager` noon gaps closed; `delete-activity` scoped to exact day+session (no title-collision false reject)
6. **Skill doc audit** ✅ — stale file paths (`data/destinations.json`, `data/ota-sources.json`, `src/utilities/`), broken relative refs, dangling `/p4-hotels`, and stale CLI names fixed across 6 SKILL.md files; p3-flights/p3p4-packages non-existent commands replaced with real scrape→import→query→select workflow; `booking-confirmation` `update-offer --source` flag corrected to positional arg
7. **StateManagerV2** (longer term) — fine-grained DB ops per ADR-001 (`src/skills/travel-shared/references/architecture-decisions.md`); remove `PlanRepository`, `syncNormalizedTables()`
8. **Integration tests** — seed / dispatch / SELECT / assert / teardown. No mocks. Real DB.
