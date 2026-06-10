# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> Routing user requests → jump to **Skill Decision Tree**.
> Commands → see **CLI Quick Reference** (full list in `docs/reference/CLI.md`).
> Past trip details (Tokyo, Kyoto Feb 2026) → `docs/trips/`.

# Japan Travel Project

## Trip Details
- **Schema**: `4.2.0` — destination-scoped with canonical offer model
- **Completed**: Tokyo Feb 13-17, Kyoto Feb 24-28 (see `docs/trips/`)
- **Active**: no trip *plan* locked, but a paused Shaping run exists — `shaping-20260525-093508` (June 2026, Osaka/Sendai-Akita/Okinawa) with a selected LionTravel Okinawa offer. Resume it or `/shaping-research` for a new one (see Next Steps)
- **Naming caveat**: legacy artifacts may say `yokohama-travel-2026`; the project is Japan-wide. (Root `package.json` is retired — see CLI Execution below.)

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
`URL → chromeport (CDP capture) → parse capture (parser_rules) → Turso offers → normalize (CanonicalOffer[]) → selectOffer() → cascade (populate P3+P4) → save() (normalized tables → bookings sync)`

### CLI Architecture (Rust, fine-grained SQL)
```
./bin/travel <cmd> [args]              # single binary, subcommand dispatch
        ↓
   main.rs                             ← arg-slice match → one module per command
        ↓
   <command>::run(args, plan_id)       ← targeted SELECT (validate) + targeted UPDATE/INSERT
        ↓
   crate::db::connect_read|write()     ← libsql connection; token via turso-util (env → cache → mint)
        ↓
   Turso (normalized tables)
```

Each command is a self-contained module under `rust/crates/travel-cli/src/` that opens its own
libsql connection and runs exactly the SQL it needs — **one targeted SELECT to validate, one
targeted UPDATE/INSERT to write**. There is no in-memory plan object, no assemble step, no
coarse flush. Mutating commands also append a `plan_events` row, bump `plans.version`, and write
an `operation_runs` audit row (the audit triad — mirror it in any new mutation). Cascade lives in
`rust/crates/travel-cli/src/cascade/` (e.g. `select_offer` populates P3+P4). This is ADR-001's target pattern, achieved
by construction in Rust — there is nothing left to refactor. (The retired TS `StateManager` /
`PlanRepository` / `syncNormalizedTables()` flow is read-only under `archive/ts-cli-retired/`.)

- **Turso cloud is sole source of truth** — fully normalized, no JSON blobs, no config JSON files
- **No local data — fail loud, never fall back** — NO command may read trip/project data (destinations, shaping/constraints, research, ranked candidates, selected offers) from a local file as its source of truth. If a Turso table/row is missing, the command THROWS — it must not silently fall back to `research/*.json`, `data/*.json`, or any local export. A `if (!dbRow) readLocalJson()` path is the bug, not the fix. `scrapes/` is a raw landing zone whose only legal next step is import→Turso→read-from-Turso. A destination MUST be registered in `destination_config` (via `/new-destination`) **before** Shaping Stage researches it. "Is X saved?" → check Turso; local files existing ≠ saved.
- **CLI agent first; plain text only** — User-facing CLI command output must be plain text/table lines, not JSON. Do not introduce JSON files, JSON fixtures, or JSON as the pipeline boundary. If structured data is needed, store it in normalized Turso tables and render a plain-text CLI view from Turso. JSON is allowed only where an external protocol/library requires it internally (e.g. the chromeport capture envelope, the shaping export/import handoff) — never as a user-facing artifact or source of truth.
- **No JSON in the RDB** — every former `*_json` column was re-normalized into child rows or typed scalar columns. Don't reintroduce `*_json` columns or parse JSON out of a DB column.
- **Destination config in DB** — `destination_config` table (no `data/destinations.json`); loaded from Turso. **28+ normalized tables** (see Data Model above). `flight_legs` holds fully-normalized flight data incl. `departure_terminal`/`arrival_terminal`.
- **Audit trail** — `plans.version` is a monotonic counter bumped per mutation; `operation_runs` records run_id/command/status/version_before-after; domain events go to `plan_events` (+ `plan_event_data` KV).
- Plan ID: `"<trip-id>"` (e.g. `tokyo-2026`, `kyoto-2026`). CLI output is stdout plain text; diagnostics go to stderr.
- **Plan resolution** — view/mutation commands resolve the plan via `plan_resolver::resolve_plan_id`: `--plan-id` > `$TRAVEL_PLAN_ID` > `--travel-date`/`--travel-start`/`--travel-end` > active-today > upcoming > most-recent. It ignores flags it doesn't own (so `status --full`, `bookings --dest x` pass through).
- **Tests** — real-Turso integration tests in `rust/crates/travel-cli/tests/*.rs`: seed → run binary → SELECT → assert → teardown; skip cleanly if creds absent. Unit tests inline per module.

## Development

### CLI Execution (Root npm Retired — 2026-06-10)
```
Rust binary invoked directly — no npm at the repo root:
  ./bin/travel <cmd>        (built from rust/crates/travel-cli)
  ./bin/chromeport <cmd>    (the CDP OTA capture driver)
Worker (workers/trip-dashboard/) keeps its own self-contained package.json (wrangler).
Python/other → none (Python scrapers archived; chromeport is Rust)
```

**Current state:** the root `package.json` is gone — the npm→Rust cutover is **done**. The Rust CLI is the sole write path (each command = targeted SELECT + UPDATE/INSERT). The old TS CLI is read-only under `archive/ts-cli-retired/`. Build with `make build`; the Makefile is the build/dev entry.

**Single-binary CLI:** there is ONE binary, `travel`, with subcommands (no per-area binaries). Examples:
- views/status → `travel status --full`, `travel itinerary`, `travel transport`, `travel bookings`
- validation → `travel validate data`, `travel doctor`
- comparison → `travel compare trips ...`, `travel compare dates ...`, `travel compare true-cost ...`
- utilities → `travel normalize flights ...`, `travel leave calc`
- DB ops → `travel db migrate`, `travel db status`, `travel db seed plans`, `travel db exec "<sql>"`

**Build the binaries to:** `./bin/` (gitignored) via `make build`.

**Reference:** `docs/plans/2026-06-10-roadmap-v2-rust.md` (active roadmap: tests → scripts port → cutover → archive TS; read before any Rust work). Historical: `docs/plans/2026-06-05-rust-cli-migration.md`, `docs/plans/2026-06-10-rust-port-audit.md`.

### Setup
> **npm RETIRED (2026-06-10):** the root `package.json` is gone. The CLI is the
> Rust `travel` binary; the build/dev entry is the root **`Makefile`**. (The
> Cloudflare Worker keeps its own self-contained `package.json` + wrangler.)
> The old TS CLI lives read-only under `archive/ts-cli-retired/`.
```bash
make setup                    # build ./bin/{travel,chromeport} + install git hooks
# or piecemeal:
make build                    # release binaries → ./bin/ (gitignored)
make dev                      # fast debug build of travel-cli
```

### Tests
```bash
make test                     # full Rust suite (real Turso; ported from the retired vitest suite)
# or directly:
cd rust && cargo test -p travel-cli
cargo test -p travel-cli --test shaping_service           # one integration test file
cargo test -p travel-cli ranks_candidates                 # one test by name (substring)
```

- **Real-Turso integration tests** — `rust/crates/travel-cli/tests/*.rs`; seed → run binary → SELECT → assert → teardown; skip cleanly if creds absent. Unit tests live inline in each `src/*.rs` module.
- Fixtures: `rust/crates/travel-cli/tests/fixtures/`.

### Pre-commit
```bash
make check                    # cargo build -p travel-cli (the old typecheck)
make validate                 # ./bin/travel validate data
make doctor                   # ./bin/travel doctor (full health check)
```
Pre-commit hook (installed by `make hooks`) runs the Rust build check + `validate data`.

### Docs
- `docs/API.md` — complete API reference
- `docs/EXTENDING.md` — how to add destinations, OTAs, validators
- `docs/SKILL_TEMPLATE.md` — skill authoring guide
- `docs/plans/` — implementation plans for major refactors
- `docs/superpowers/specs/` — methodology specs (Shaping Stage design, price-baseline/rhythm method, tour-group scraper, decision methodology). Read these when the user asks "how should we approach X" rather than "what's the command for X."
- `docs/plans/2026-05-22-new-planning-flow.md` — **adopted** research-first staged planning model (date/destination/flight explored together before plan lock). Existing P1–P5 skills remain implementation tools inside the stages.

## Agent-First Workflow

- Proactively run next logical step; only ask user when a preference materially changes the result
- Prefer `./bin/travel` subcommands over direct SQL for reusable content edits (raw `db exec` is fine for one-shot migrations/backfills — see "DB Operation Decision")
- Every output: current status, what changed, single best next action

### Skill Decision Tree
```
User intent                          → Skill / Action
──────────────────────────────────────────────────────
"plan a trip to [place]"             → Shaping Stage/1 staged flow
  loose dates/destination/price?         → /shaping-research
  fixed dates + destination?             → create/verify plan, then /p1-dates + /p2-destination
  destination missing?                  → /new-destination, then continue Shaping Stage/1
"cheapest week to go to X"           → /shaping-research (pre-lock triangle research)
"Osaka or Tokyo, depends on price"   → /shaping-research (compare destinations + dates + price together)
"what dates are cheapest"            → /shaping-research
"set dates" / "change dates"         → /p1-dates
"which city" / "how many nights"     → /p2-destination
"lock this Shaping Stage candidate"        → ./bin/travel shaping-adopt <candidate_id> <new_plan_id> --create-plan --dest <slug>
"draft the trip" / "rough itinerary" → /stage1-itinerary-draft
"find packages" / "search OTA"       → /stage2-shop-transport (check freshness first)
  fresh data in Turso?                  → query-offers (show existing)
  stale/no data?                        → /p3p4-packages (scrape + auto-import)
"find flights only"                  → /stage2-shop-transport (uses /p3-flights)
"compare offers"                     → /stage2-shop-transport
"query offers"                       → ./bin/travel query-offers --plan-id <id> --dest <slug>
"import scraped files"               → ./bin/travel import-offers --dir scrapes --dest <slug>
"is data fresh"                      → ./bin/travel check-freshness --source <s>
"book separately"                    → /stage2-shop-transport (uses /separate-bookings)
"how many leave days"                → ./bin/travel leave calc
"book this" / "select offer"         → ./bin/travel select-offer
"plan the days" / "itinerary"        → /stage3-expand-itinerary
"show bookings"                      → ./bin/travel query-bookings (from DB)
"show status"                        → ./bin/travel status --full
"show schedule"                      → ./bin/travel itinerary
"weather" / "forecast"               → ./bin/travel fetch-weather [--dest slug] [--all]
User provides OTA URL                → /scrape-ota (see URL Routing)
User provides booking confirmation   → ./bin/travel set-activity-booking
"deploy dashboard" / "publish trip"  → /stage4-publish-dashboard
```

### URL Routing
**Do not use WebFetch for OTA sites** (they require JavaScript). **The Python scrapers are
DECOMMISSIONED and archived** (`archive/broken-python-scrapers/`) — their constructed
URLs 404 / hit the wrong page. Scrape via the Rust CDP driver against real Chrome:

| URL Contains | Action |
|-------------|--------|
| Any OTA (besttour / liontravel / lifetour / settour / …) | Drive the real page in Chrome, then capture + verify + parse: <br>`./rust/target/debug/chromeport fetch interact "<url>" --source <id> --step ...` (or `browser snapshot` on an open tab) <br>→ `./rust/target/debug/chromeport verify <source-id> <capture-id>` (read-only regex diagnostics) <br>→ `./rust/target/debug/chromeport parse capture <capture-id> --source <id>` (imports to Turso) |
| Non-OTA URL | Use WebFetch as normal |

The driver navigates/clicks the actual UI (no fragile URL templates). Captures live in the
Turso `captures` table; offers go to the `offers` table. Parser rules per OTA: `parser_rules` table.
NOTE: flight/hotel-only OTAs (tigerair, google_flights, trip, agoda, eztravel) are seeded
with `has_custom_parser=0`. The generic parser has flight/hotel-specific required fields now; each
source still needs a real live Chrome scrape before decommission status can advance.

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
| `/scrape-ota` | `src/skills/scrape-ota/SKILL.md` | Scrape OTA sites (chromeport CDP driver) |
| `/separate-bookings` | `src/skills/separate-bookings/SKILL.md` | Compare package vs split booking |
| `/booking-confirmation` | `src/skills/booking-confirmation/SKILL.md` | Post-booking verification workflow |
| `/post-pull-fix` | `src/skills/post-pull-fix/SKILL.md` | Health checks after git pull |
| `/weather-update` | `src/skills/weather-update/SKILL.md` | Fetch weather with pre-checks |
| `/deploy-dashboard` | `src/skills/deploy-dashboard/SKILL.md` | Deploy trip dashboard to CF Workers |
| `/pre-trip-checklist` | `src/skills/pre-trip-checklist/SKILL.md` | Pre-departure verification |
| `/new-destination` | `src/skills/new-destination/SKILL.md` | Add destination to config |
| `/shaping-research` | `src/skills/shaping-research/SKILL.md` | Pre-lock triangle research (date/destination/flight) |
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

### Scrapers — DECOMMISSIONED (use the Rust CDP driver)
All Python scrapers (`scrape_package.py`, `scrape_listings.py`, the `scrapers/` package, etc.) are
**archived under `archive/broken-python-scrapers/`** and must NOT be run — their URL/region/template
construction 404s or lands on the wrong page. The replacement is the Rust CDP driver
(`rust/crates/chromeport`): drive the real OTA page in Chrome (`fetch interact` / `browser
snapshot`) → `parse capture <id>` (rule-driven, `parser_rules` table) → Turso. No `pip install
playwright`, no Python. See `docs/plans/2026-06-05-rust-cdp-scraper-migration.md`.

> **"Scrape" terminology — three distinct things, don't conflate them:**
> 1. **`rust/crates/chromeport`** (THE live CDP driver) — a Rust CDP driver (`chromiumoxide`)
>    that ATTACHES to a real Windows Chrome at `127.0.0.1:9222`, navigates/clicks/fills, and
>    writes **plain-text captures → Turso `captures` table**, then `parse capture` rule-parses
>    them (`parser_rules`) into the Turso **`offers`** table. This is the source-of-truth path.
> 2. **`scrapes/*.json`** — a LEGACY, gitignored landing zone of raw captured JSON that the
>    `import-offers` / `import-tour-group-offers` commands parse into `plan_offers` /
>    `shaping_tour_group_offers`. Being phased out in favor of (1)'s `captures`→`offers` path;
>    its only legal next step is import→Turso. It is NOT live scraping.
> 3. **Python scrapers** — DEAD (archived above). Never run.
>
> **Relationship to `gwebcdb`** (`/home/yanggf/b/gwebcdb`): travel-2026 shares **only**
> `crates/turso-util` (the Turso token-minting library that `travel-cli` + `chromeport`
> vendor). gwebcdb's `bridge/` (Python Playwright, read-only finance/decision inspection with
> deny-by-default click guardrails, writes to local files only — NEVER Turso) is a SEPARATE tool;
> it is deliberately NOT the OTA scraper and is not used by travel-2026. Both happen to attach to
> the same Chrome `:9222`, but `chromeport` is the write-capable OTA pipeline, gwebcdb's bridge
> is not. Do not try to route OTA scraping through gwebcdb's bridge.

## Current Status

Completed trips — full bookings, itinerary, and weather notes archived:
- **Tokyo Feb 13-17** → `docs/trips/2026-tokyo.md`
- **Kyoto Feb 24-28** → `docs/trips/2026-kyoto.md`

No upcoming trip locked. Plan status for any active plan: `./bin/travel status --full`.

## CLI Quick Reference

Most-used commands inline; the **canonical full reference** (every mutation, comparison view, scraping flag, Shaping Stage aggregator handoff) lives in **`docs/reference/CLI.md`**. Add new commands there, not here.

```bash
# Views (run any one)
./bin/travel plans                               # list DB plans and date anchors
./bin/travel status --full                       # booking overview
./bin/travel itinerary                           # daily plan
./bin/travel transport                           # transport summary
./bin/travel bookings                            # booking ledger
./bin/travel status --travel-date 2026-06-20

# Shaping Stage (pre-plan triangle research)
./bin/travel shaping-init --origin TPE --start 2026-06-18 --end 2026-06-20 \
  --dest KIX:"Osaka (KIX)" --dest NRT:"Tokyo (NRT)" --nights 6 --nights 7 [--pax 2]
# After shaping-init: scrape offers via chromeport, then import + compare:
#   ./bin/chromeport fetch interact "<url>" --source <id> --step ...
#   → ./bin/chromeport parse capture <capture-id> --source <id>
#   → ./bin/travel shaping-import --run <run_id> --file <handoff.json>
./bin/travel shaping-compare --run <run_id>
./bin/travel shaping-adopt <candidate_id> <plan_id> --create-plan --dest <slug>

# Offers (Turso)
./bin/travel import-offers --dir scrapes --dest tokyo_2026 [--start ... --end ...] [--dry-run]
./bin/travel query-offers --plan-id tokyo-2026 --dest tokyo_2026 [--max-price 30000]
./bin/travel check-freshness --source besttour --plan-id tokyo-2026 --dest tokyo_2026

# Bookings
./bin/travel sync-bookings [--dry-run]
./bin/travel query-bookings --dest tokyo_2026 [--category activity --status pending]
./bin/travel validate-itinerary --dest tokyo_2026

# Scraping — Python scrapers DECOMMISSIONED; use the chromeport CDP driver:
#   ./bin/chromeport fetch interact "<url>" --source <id> --step ...
#   → ./bin/chromeport verify <source-id> <capture-id>
#   → ./bin/chromeport parse capture <capture-id> --source <id>   # imports to Turso
# See URL Routing + src/skills/scrape-ota/SKILL.md.

# Tour-group / FIT offers (manual entry for sources without a full scraper)
./bin/travel import-tour-group-offers --run <run_id> --file <path>
./bin/travel query-tour-group-offers --run <run_id> [--source <id>] [--nights N] [--max-price TWD] [--json]
./bin/travel shaping-baseline --run <run_id>                # methodology comparison view
./bin/travel add-besttour-offer --url <url> --price <twd> --hotel "<name>"
./bin/travel add-lifetour-offer --url <url> --price <twd> --hotel "<name>"

# Mutations — only the 4 most common shown here.
# Full list (set-airport-transfer, set-activity-time, set-day-theme, set-route-segment,
# set-tod-zh, delete-activity, swap-days, run-status, check-booking-integrity, …)
# lives in docs/reference/CLI.md. Add new mutation examples THERE, not here.
./bin/travel set-dates 2026-02-13 2026-02-17
./bin/travel select-offer <offer-id> <date>
./bin/travel set-activity-booking <day> <session> "<activity>" <status> [--ref "..."]
./bin/travel fetch-weather [--dest slug] [--all]

# DB + tests (run any one)
./bin/travel db status                           # show DB state
./bin/travel db migrate                          # create/upgrade tables (idempotent)
./bin/travel db seed plans                       # one-time plan seed
make test                                        # full Rust test suite
./bin/travel validate data                       # data integrity check
./bin/travel doctor                              # full system health check
```

Plan resolution: `--plan-id` and `$TRAVEL_PLAN_ID` win. Without those, the CLI uses `--travel-date`, `--travel-start/--travel-end`, or exactly one active or upcoming DB date anchor/planning window. Use `--travel-*` for plan selection; plain `--start/--end` are command-specific filters (e.g. offer search ranges). If several plans match, the CLI fails with a plan list instead of silently loading a legacy default.

## Project Structure
```
/
├── data/
│   ├── holidays/taiwan-2026.json  # Holiday calendar
│   ├── hotel-areas.json           # Zone categorization (used by compare-true-cost)
│   └── transport-routes.json      # Transit routes (used by compare-true-cost)
├── scrapes/                       # LEGACY raw-capture JSON landing zone (gitignored) — import→Turso only; not live scraping (see "Scrape terminology")
├── scripts/                       # NON-TS keepers only: hooks/, schema.sql, *.sql, *.ps1, *.sh, README
│   └── hooks/pre-commit           # cargo build -p travel-cli + ./bin/travel validate data
├── workers/trip-dashboard/        # Cloudflare Worker — live trip dashboard (own package.json + wrangler)
│   ├── src/index.ts               # Request handler + router + favicon
│   ├── src/turso.ts               # Turso HTTP pipeline client (18-query pipeline)
│   ├── src/render.ts              # SSR HTML renderer (ZH from DB, no hardcoded content)
│   └── src/styles.ts              # Mobile-first inline CSS
├── Makefile                       # npm-free build/dev entry: build, dev, test, check, validate, hooks, setup
├── bin/                           # built binaries (gitignored): travel, chromeport — via `make build`
├── rust/crates/                   # the LIVE codebase (Cargo workspace)
│   ├── travel-cli/                # the `travel` binary — ALL CLI commands
│   │   ├── src/main.rs            #   arg-slice match dispatch → one module per command
│   │   ├── src/<command>.rs       #   ~58 modules (set_dates, select_offer, shaping, weather,
│   │   │                          #     view_{bookings,itinerary,transport}, db_*, validate, …)
│   │   ├── src/db.rs              #   connect_read/connect_write (libsql via turso-util)
│   │   ├── src/plan_resolver.rs   #   plan resolution ladder
│   │   ├── src/db_migrate.rs      #   inline-DDL schema migrate (1:1 port of old turso-migrate.ts)
│   │   ├── src/cascade/           #   cascade logic (date_change, select_offer → populate P3+P4)
│   │   └── tests/                 #   real-Turso integration tests + fixtures/
│   ├── chromeport/                # CDP OTA capture driver (the `chromeport` binary)
│   └── turso-util/                # Turso token mint/cache + libsql connect + migrate runner
├── src/skills/                    # LIVE skill defs (SKILL.md + references) — ONLY live part of src/
├── archive/ts-cli-retired/        # retired TS CLI: src/ (minus skills), tests/, scripts/*.ts (read-only)
├── data/                          # holiday calendar + zone/route reference JSON read by the binary
└── docs/                          # API.md, EXTENDING.md, SKILL_TEMPLATE.md, reference/CLI.md, plans/
```

Config/reference data all live in Turso (no JSON files): `destination_config`, `ota_sources`, `origin_config`, `global_config`; OTA rules in `airlines`/`booking_types`/`platform_behaviors`/`comparison_rules`; destination reference (areas/POIs/clusters/transit/tips) in `destination_areas` (+ child tables), `destination_pois`, `destination_clusters`, `destination_transit`, `destination_tips` — read via `./bin/travel query-destination-ref`. Re-seeding a fresh/empty DB uses the seed pipeline (the original TS seed scripts are under `archive/ts-cli-retired/scripts/`; reusable seeders are `./bin/travel db seed …` + inline `seed_*` in `db_migrate.rs`).
Note: `ref_path`/`scraper_script` must be repo-relative paths.

## Turso DB
```
Database: travel-2026 | Region: aws-ap-northeast-1 | Creds: .env (gitignored)
```

Tables:
- **Plan core**: `plans` (PK=plan_id, `version` monotonic counter — no JSON blobs), `plan_metadata`, `plan_destinations`, `destination_details`, `destination_cities`
- **Dates/Status**: `date_anchors`, `process_statuses`, `cascade_dirty_flags`, `plan_root_date_anchor`
- **Offers**: `plan_offers`, `plan_offer_flights`, `plan_offer_hotels`, `plan_offer_date_pricing`, `plan_offer_selection`, `plan_offer_best_value`, `plan_offer_provenance` (source audit trail), `plan_offer_warnings`
- **Itinerary**: `days` (+ weather + `theme_zh`), `timesofday` (+ `focus_zh`, `transit_notes_zh`), `activities`, `itinerary_metadata` (+ `transit_hotel_station`/`transit_hotel_station_zh` scalars; transit key lines in `itinerary_transit_key_lines`), `day_route_segments` (+ `duration_min`, `notes`, `start_time`), `day_landmarks`, `session_meals`, `session_activities_zh`, `activity_tags`
- **Transport**: `flight_legs` (PK: plan_id+destination+direction+leg_order), `airport_transfers` (+ `selected_*` scalar cols), `airport_transfer_candidates`, `transportation_extras` (+ `*_status` scalars), `transport_extra_candidates`
- **Accommodation**: `hotels` (+ `name_zh`), `hotel_access_lines`, `accommodation_location_zone`, `location_zone_candidates`
- **Offer child rows**: `plan_offer_includes`, `plan_offer_hotel_access`
- **Cascade**: `cascade_triggers` (+ `condition_*` scalars), `cascade_trigger_resets`, `cascade_trigger_populate_map`, `cascade_global_state`, `plan_schema_contract` (+ `plan_schema_contract_nodes`), `plan_process_precedence` (+ `plan_process_precedence_entries`), `plan_root_date_anchor` (+ `flex_*` scalars + `plan_date_anchor_flex_dates`), `plan_budget`
- **Event store** (`plan_events`): unified domain event store — `plan_events` (scope ∈ timeline|global_process|dest_process) + `plan_event_data` (open payload as KV rows) + `event_log_next_actions`; status tables `event_log_state`, `event_log_global_processes`, `event_log_destinations`, `event_log_dest_processes`. (Renamed/unified from the old "event_log"; the flat `event_log_process_events` table was dropped.)
- **Bookings**: `bookings_current` (flat rows: package/transfer/activity), `bookings_events` (audit)
- **Operation tracking**: `operation_runs` (audit trail: run_id, plan_id, command_type, status, version_before/after, timestamps)
- **Shaping Stage** (formerly "Stage 0"; unscoped, keyed by `run_id`): `shaping_research_runs`, `shaping_research_destinations`, `shaping_research_durations`, `shaping_rules` (hard/soft shaping constraints), `shaping_candidates`, `shaping_candidate_flights`, `shaping_scrape_attempts`, `shaping_tour_group_offers` (+ `shaping_tour_group_offer_notes` — flat key/value child rows for free-form research annotations; the former `raw_json`/`raw_text` blob is now typed `raw_confidence`/`raw_note`/`raw_flight`/`raw_flight_outbound`/`raw_flight_return` columns + this notes table, NO JSON), `shaping_tour_group_scrape_attempts`, `shaping_research_artifacts`, `shaping_selected_offers` — pre-plan triangle-research + constraint-shaping domain. Commands: `shaping-init/compare/adopt/baseline/export/import`; skill `/shaping-research`. (see `docs/superpowers/specs/2026-05-22-stage0-shaping.md`)
- **Global config** (not plan-scoped): `destination_config` (slug PK, coordinates/timezone/airports), `origin_config` (taiwan origin), `global_config` (default_destination, default_origin), `ota_sources` (OTA registry — replaces ota-sources.json)
- **OTA knowledge** (de-JSON'd into child rows): `booking_types` + `booking_type_rules`, `platform_behaviors` + `platform_behavior_quirks`/`platform_behavior_baggage_labels`, `hotel_areas` + `hotel_area_keywords`, `airlines`, `transport_routes`, `transport_hubs`, `ota_sources` + `ota_source_types`/`ota_source_regions`
- **Reference data** (de-JSON'd): `destination_areas` + `destination_area_stations`/`destination_area_best_for`, `destination_pois` + `destination_poi_tags`, `destination_clusters` + `destination_cluster_pois`, `destination_transit`, `destination_config` + `destination_tips`/`destination_airports`/`destination_markets`, `origin_airports`
- **Other**: `offers`, `destinations`, `events`, `bookings`

> **No JSON in the RDB (de-JSON program, 2026-06):** every former `*_json` column was re-normalized — flat lists → child tables (one row per element), small objects → typed scalar columns, open/variable blobs → a single `*_text` column. A whole-DB content scan confirms zero JSON-encoded values in any column. The dead `flights` table (old JSON-blob flight store) was dropped. Don't reintroduce `*_json` columns or `JSON.parse`/`JSON.stringify` against DB column data.

Schema reference: `scripts/schema.sql` (read-only DDL reference, auto-generated from the live DB; the `gen-schema-sql.ts` generator is retired — see `archive/ts-cli-retired/`. Do not hand-edit; regenerate after migrations)
Schema/migration: `./bin/travel db migrate` (creates all tables idempotently)
Seed: `./bin/travel db seed plans` (one-time, already run)

### DB Operation Decision
- **Reusable operation** (editing itinerary content, updating themes, managing activities) → build a UI/CLI interface
- **One-shot operation** (migration, schema change, data backfill) → direct SQL via turso-exec is acceptable
Before running raw SQL for content edits, ask: "Will this be done again?" If yes, build the interface first.

## Multi-Plan
All plans live in the `plans` table in Turso (no local JSON files).
`plan_id` uses hyphens (`tokyo-2026`), `destination` uses underscores (`tokyo_2026`) — convert by swapping `-`↔`_`.
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
- **ZH content** — All Chinese content stored in DB (`theme_zh`, `focus_zh`, `transit_notes_zh` scalars; ZH activities in `session_activities_zh`; meals in `session_meals`). No hardcoded content in Worker code. Content updates take effect instantly without redeploy. Use `set-day-theme --zh` for day themes, `set-tod-zh` (alias: `set-session-zh`) for session focus/transit/activities, `set-route-segment` for Chinese place names. For bulk new-destination ZH population: copy `scripts/set-kyoto-zh-sessions-v2.ts` pattern (parameterized Turso pipeline queries — required for Unicode/emoji content).
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
| Wrong plan content | Plan not synced to Turso | Run `./bin/travel db seed plans` |
| "Plan not found" error | Plan ID mismatch (underscore vs hyphen) | URL uses `tokyo-2026`, DB uses `tokyo_2026` |
| ZH content not showing | Missing `_zh` columns in DB | Run `set-tod-zh` CLI per session, or bulk-populate via `scripts/set-kyoto-zh-sessions-v2.ts` pattern |
| ZH UPDATE silently fails (rows_affected=0) | Inline SQL with Unicode/emoji fails encoding | Use parameterized Turso queries: `args:[{type:"text",value:"..."},{type:"integer",value:"1"}]` — integer value must be a string |
| Weather missing | Weather not fetched | Run `./bin/travel fetch-weather --dest <slug>` |
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
Pre-commit: Rust build check + `validate data` (see Pre-commit above). Install hooks: `make hooks`.

## Next Steps

**The Rust port is DONE** (commits through `88385fb`): P1 command parity, P2 scripts, P3 real-Turso integration tests, the `package.json` cutover (root npm retired), TS archived, and docs/skills converted to `./bin/travel`. ADR-001 / "StateManagerV2" is achieved by construction (the Rust CLI *is* the targeted-SQL model) — do NOT refactor the archived TS `StateManager`; that work is complete and the code is read-only under `archive/`. Don't re-port `import-offers-to-turso` (intentionally dropped; replaced by `import-offers` + chromeport `parse capture`).

Remaining agenda (none blocking — the project is between trips and the live DB is fully seeded):

- **`--dest` honored in view commands** (small) — `bookings`/`itinerary`/`transport` parse `--dest` but ignore it (`plan::load` always keys on `active_destination`). Harmless today (all plans are single-destination) but a parity regression. Minimal fix: fail-loud on a mismatching `--dest`; full fix when a multi-destination plan exists.
- **PARKED (on agenda, now unblocked)** — Worker → `workers-rs` port (~2.9k LOC in `workers/trip-dashboard/src`). wrangler/npm stays for deploy regardless and the read-mostly dashboard gains no data-integrity benefit, so low priority. Revisit per `docs/plans/2026-06-10-roadmap-v2-rust.md`.
- **OTA decommission gate** (user-driven) — only `settour` is live-verified; the rest have snippet fixtures only. Their archived Python parsers can't be deleted until each passes a real `chromeport verify` against a live capture — needs human browser sessions, not code.
- **Product / paused run** — a real in-progress shaping run exists: `shaping-20260525-093508` (June 2026, Osaka/Sendai-Akita/Okinawa) with 90 ranked candidates and a **selected LionTravel offer** (Hotel Aqua Citta Naha, 2026-06-21 3n). `okinawa_2026` is NOT yet in `destination_config`. To resume: `/new-destination okinawa_2026`, then `shaping-adopt <candidate> okinawa-2026 --create-plan --dest okinawa_2026`. **User decides** dates/destination — don't pick autonomously.
