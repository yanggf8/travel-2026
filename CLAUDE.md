# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> Routing user requests → jump to **Skill Decision Tree**.
> Commands → see **CLI Quick Reference** (full list in `docs/reference/CLI.md`).
> Past trip details (Tokyo, Kyoto Feb 2026) → `docs/trips/`.

# Japan Travel Project

## Trip Details
- **Schema**: `4.2.0` — destination-scoped with canonical offer model
- **Completed**: Tokyo Feb 13-17, Kyoto Feb 24-28 (see `docs/trips/`)
- **Active**: `okinawa-2026` — Naha, **2026-06-12 → 06-16** (CI120/CI121, HOTEL AZAT NAHA, 5-day itinerary populated). `okinawa_2026` is registered in `destination_config`. Status: `./bin/travel status --full --plan-id okinawa-2026`. (Flights/hotel were pre-decided and entered directly via `set-flight`/`set-hotel`; the `shaping-20260525-093508` run was exploratory only — 0 candidates adopted. Corrected 2026-07-02 vs the DB record; see the Okinawa entry under Next Steps.)
- **Naming caveat**: legacy artifacts may say `yokohama-travel-2026`; the project is Japan-wide. (Root `package.json` is retired — see CLI Execution below.)

## Architecture

### Data Model
Turso: 28+ fully-normalized tables, no JSON blobs. Schema: `scripts/schema.sql`; live state: `./bin/travel db status`.

### Cascade Rules
| Trigger | Reset | Scope |
|---------|-------|-------|
| `active_destination_change` | `process_5_*` | new destination |
| `process_1_date_anchor_change` | `process_3_*`, `process_4_*`, `process_5_*` | all destinations |
| `process_2_destination_change` | `process_3_*`, `process_4_*`, `process_5_*` | current destination |
| `process_3_4_packages_selected` | populate P3+P4 from chosen offer | current destination |

### Data Flow
`URL → gwebcdb on WSLg (navigate + ota_capture, CDP) → captures.raw_text → AGENT reads + extracts → travel ota write-offers (TSV → Turso offers + provenance) → normalize (CanonicalOffer[]) → selectOffer() → cascade (populate P3+P4) → save() (normalized tables → bookings sync)` — extraction is agent-first (the coding agent is the parser); the old in-CLI regex/`parser_rules` parse step is RETIRED (see URL Routing).

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

**Shared mutation helpers (use these — do not re-roll them).** `cascade::common` is the single
source of truth for the boilerplate every mutation needs; copy-pasting it into a new command is the
bug, not the convention:
- `record_operation(conn, plan_id, command_type, summary, version_before, version_after, now_db)`
  writes the audit-triad **back half** in one call — the `operation_runs` INSERT + the
  `plans.version` bump. Emit the `plan_events`/`plan_event_data` rows first (their count/order is
  command-specific via `insert_event` / `insert_kv_rows`), then call this once. For a freshly
  created plan (e.g. `shaping-adopt`) pass `version_before=0, version_after=1`.
- `resolve_active_destination(conn, plan_id, dest_override)` resolves an explicit `--dest` override
  else `plan_metadata.active_destination` (fail-loud, no local fallback). Every `set_*`/itinerary
  module's `read_destination` is a thin wrapper over this — don't reintroduce a private copy.

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
  (OTA capture: chromeport RETIRED — use gwebcdb's Python bridge tools on WSLg; see URL Routing)
Worker (workers/trip-dashboard/) keeps its own self-contained package.json (wrangler).
Python/other → OTA scraping = gwebcdb (~/b/gwebcdb) on WSLg; old Python scrapers archived
```

**Current state:** the root `package.json` is gone — the npm→Rust cutover is **done**. The Rust CLI is the sole write path (each command = targeted SELECT + UPDATE/INSERT). The old TS CLI is read-only under `archive/ts-cli-retired/`. Build with `make build`; the Makefile is the build/dev entry.

**Single-binary CLI:** there is ONE binary, `travel`, with subcommands (no per-area binaries). Examples:
- views/status → `travel status --full`, `travel itinerary`, `travel transport`, `travel bookings`
- validation → `travel validate data`, `travel doctor`
- comparison → `travel compare trips ...`, `travel compare dates ...`, `travel compare true-cost ...`, `travel compare content-depth ...`
- utilities → `travel normalize flights ...`, `travel leave calc`
- DB ops → `travel db migrate`, `travel db status`, `travel db seed plans`, `travel db exec "<sql>"`

**Build the binaries to:** `./bin/` (gitignored) via `make build`.

**Reference:** `docs/plans/2026-06-10-roadmap-v2-rust.md` (active roadmap: tests → scripts port → cutover → archive TS; read before any Rust work). Historical: `docs/plans/2026-06-05-rust-cli-migration.md`, `docs/plans/2026-06-10-rust-port-audit.md`.

### Setup
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
- **Panic-safe + drift-proof teardown (REQUIRED).** These tests hit the SHARED live Turso DB, so a leaked test row pollutes production. Two rules, both enforced by `rust/crates/travel-cli/tests/common/mod.rs`:
  1. **Use the canonical `common::teardown_plan(plan, dest)` — do NOT hand-roll a table list.** It queries `sqlite_master` LIVE for every table with a `plan_id` column and DELETEs from each (plans last), so it's drift-proof: any new plan-keyed table (or one a cascade newly writes) is covered automatically. Hand-rolled lists DRIFT and leak — that is exactly how the `zzdiag`/`test-promote` prod rows leaked (27 of 31 plan-creating tests had missed `plan_destinations`). For id-keyed global `offers` rows (not plan-keyed), call `common::teardown_offers(&ids)` too. Both go through the binary's `db exec` and never panic. **The migration is COMPLETE (2026-07-05):** every integration test uses the `common::` harness — one canonical `db_exec(sql)->Option<Rows>` (5 hand-rolled signatures collapsed to 1), one `is_credless` (fixed a false-fail-instead-of-skip gap), one `bin`/`nanos`, `teardown_plan` for plan-keyed rows + `db_exec_teardown` (best-effort) for non-plan-keyed ones (OTA `job_id`/`source_id`/…, `activity_tags` by `activity_id`, `shaping_*` by `run_id`, slug-keyed `destination_*`). Only `tests/teardown_plan.rs` (the proof test) keeps its own `db_exec` by design. **NON-plan-keyed rows a test seeds MUST be torn down locally** (teardown_plan only covers `plan_id`-column tables); an `activity_tags` delete whose subquery reads `activities` must run BEFORE `teardown_plan`. **Run these serialized Turso tests in the BACKGROUND** — a foreground timeout SIGTERMs the test mid-run and the Guard's `Drop` never fires (leaks an orphan row; the only remaining leak vector).
  2. **Arm the RAII `Guard` right after the plan-id is bound** — never call teardown as the last statement (a panicking assertion, i.e. every TDD RED run, unwinds past it and leaks). Pattern: `mod common; use common::Guard;` then `let _g = Guard::new({ let (plan,dest)=(plan.clone(),dest.clone()); move || common::teardown_plan(&plan,&dest) });`. Its `Drop` runs teardown on both return and panic. Keep an optional pre-clean `common::teardown_plan(...)` BEFORE the guard (clears a prior run's leftovers); never leave a trailing one. Proof/self-test: `tests/teardown_plan.rs` (seeds → teardown → asserts 0 rows across all plan-keyed tables).
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

### Default path — known-flights fast-path (2026-07-02)

Every real trip so far (tokyo/kyoto/okinawa/kyoto-jul) is the same shape: ~5-day Japan with **flights +
hotel decided before the tool touches them.** So the **default path IS the fast-path**: create plan →
`set-dates` + `set-flight`/`set-hotel` → straight to the itinerary. There is no "classify the trip"
step — trips don't vary in kind. **Shaping and offer-shopping are OPTIONAL side-tools**, only for the
rare trip where flights/dates are NOT yet decided (price-shopping "cheapest week", "Osaka vs Tokyo by
price"). *How* you acquire transport is a **Stage 2 purchase mode** (`shop` | `ingest-known` | `defer`
— see Stage 2), NOT a top-level router. (`travel flow-decision` optionally records the purchase mode;
skip it if it's just noise on a single-pattern trip.)

### Skill Decision Tree
```
User intent                          → Skill / Action
──────────────────────────────────────────────────────
"plan a trip to [place]"             → DEFAULT: known-flights fast-path (below). Shaping only if flights/dates unknown.
  known flights + hotel? (usual)         → create plan → set-dates + set-flight/set-hotel → /stage1-itinerary-draft
  loose dates/destination/price?         → /shaping-research (optional pre-lock triangle research)
  fixed dates + destination?             → create/verify plan (./bin/travel create-plan <id> --dest <slug> --start --end --airport <IATA>), then /p1-dates + /p2-destination
  destination missing?                  → /new-destination, then continue
"rename the plan" / "change display name"  → ./bin/travel set-plan-name "<name>" [--dest <slug>]   (plan_destinations.display_name; slug-keyed, no plan_events)
"switch active destination"          → ./bin/travel set-active-destination <slug>   (plan_metadata.active_destination; must be a registered destination of the plan)
"cheapest week to go to X"           → /shaping-research (pre-lock triangle research)
"Osaka or Tokyo, depends on price"   → /shaping-research (compare destinations + dates + price together)
"what dates are cheapest"            → /shaping-research
"set dates" / "change dates"         → /p1-dates
"which city" / "how many nights"     → /p2-destination
"lock this Shaping Stage candidate"        → ./bin/travel shaping-adopt <candidate_id> <new_plan_id> --create-plan --dest <slug>
"draft the trip" / "rough itinerary" → /stage1-itinerary-draft
"find packages" / "search OTA"       → /stage2-shop-transport — mode `shop` (check freshness first)
  fresh data in Turso?                  → query-offers (show existing)
  stale/no data?                        → /p3p4-packages (scrape + auto-import)
"find flights only"                  → /stage2-shop-transport — mode `shop` (uses /p3-flights)
"compare offers"                     → /stage2-shop-transport — mode `shop`
"which offer should we take" / "compare purchase options"  → ./bin/travel shaping-purchase-matrix --run <run_id> (when a shaping run has offers; read-only GATE/NUDGE scoring vs shaping_rules)
"flights/hotel already booked"       → /stage2-shop-transport — mode `ingest-known` (record + VALIDATE, no shopping)
"skip shopping for now"              → /stage2-shop-transport — mode `defer` (log skip reason)
  # Stage 2 has MODES (P4): shop | ingest-known | defer. Package/direct COMPARISON is optional;
  # transport/accommodation VALIDATION is mandatory in every mode. Each mode records
  # `travel flow-decision shop mode --mode <m>`. Modes match flow_decision.rs MODES.
"query offers"                       → ./bin/travel query-offers --plan-id <id> --dest <slug>
"import scraped files"               → ./bin/travel import-offers --dir scrapes --dest <slug>
"use scraped OTA offers in a plan"   → ./bin/travel promote-offers --from-offers --dest <slug> --plan-id <id>  (global offers → plan_offers; then select-offer)
"is data fresh"                      → ./bin/travel check-freshness --source <s>
"book separately"                    → /stage2-shop-transport (uses /separate-bookings)
"how many leave days"                → ./bin/travel leave calc
"book this" / "select offer"         → ./bin/travel select-offer
"plan the days" / "itinerary"        → /stage3-expand-itinerary   (populate activities → derive-routes cascades transit → agent authors AI-recommended meals, LABELED via --recommended)
"is the drill/plan rich enough" / "compare depth to a real trip"  → ./bin/travel compare content-depth --plan-id <id> [--against okinawa-2026]  (read-only oracle; loop-until-BETTER; web page is final gate)
"derive routes" / "add transit between activities"  → ./bin/travel derive-routes [--day N] [--dest slug]   (cascade ai_recommended legs from the activity skeleton; run after populate)
"add a transit time" / "derived leg has no minutes"  → ./bin/travel add-transit <slug> <from> <to> --minutes N [--line ..] [--kind ..]   (add a destination_transit station pair so derive-routes attaches its time; slug-keyed reference data, no --plan-id, idempotent. Use this instead of raw db exec when a derived leg came back without a duration.)
"empty dashboard map" / "link itinerary POIs"  → ./bin/travel set-activity-poi --auto [--dest slug] first; then manually link any reported misses with ./bin/travel set-activity-poi <day> <session> <poi_id> --match "<title substring>"
"show/review the AI suggestions" / "what did the agent recommend"  → ./bin/travel query-recommendations [--day N] [--session s] [--kind ...]   (read-only list; preview before confirming)
"confirm the AI suggestions" / "accept recommendations"  → ./bin/travel confirm-recommendations [--day N] [--session s] [--kind activity|meal|route]   (flip ai_recommended → confirmed)
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
URLs 404 / hit the wrong page.

> **OTA scraping = gwebcdb on WSLg (current, verified 2026-06-25) — read first.** The browser
> layer is **gwebcdb** (`~/b/gwebcdb`), the shared WSLg-based CDP toolset, AND it now owns the OTA
> extraction too (Phase 0 Python port SHIPPED). **`chromeport` is RETIRED** — its OTA
> `parse` / `verify` / `parser rules` subcommands are **removed** (they now fail loud, exit 1, and
> point to gwebcdb; the dead parser code was deleted 2026-06-29). chromeport only still provides
> `browser` / `screenshot` / `db` for `snapshot-maps`. It was the fragile Windows-Chrome path that
> WSLg replaces, not a fallback to keep working. WSLg-native Chrome is the **standing verified
> backend** (live on this host: `running_backend=wslg`, CDP up on :9222, bridge attached). Drive
> everything with gwebcdb's Python bridge tools. Full recipe + gotchas: **gwebcdb `CLAUDE.md` →
> "OTA scraping — end-to-end usage"**; per-source gate: `docs/plans/2026-06-24-ota-migration-chromeport.md`.
> For sign-in OTAs the human logs in / settles 2FA in the WSLg Chrome window (session persists in
> the `~/.local/share/gwebcdb/codex-browser` profile) or via gwebcdb's approval-gated `login_assist`.

OTA capture flow (run from `~/b/gwebcdb`; export `TURSO_URL`/`TURSO_TOKEN` from this repo's `.env`
first — gwebcdb's `turso_db.py` has no `.env` loader):

| URL Contains | Action |
|-------------|--------|
| Any OTA (besttour / liontravel / lifetour / settour / …) | Start Chrome, drive the page, capture, then **the agent reads the capture text and writes offers** (no in-CLI parser): <br>`./scripts/start-chrome-cdp-wslg.sh` (idempotent; CDP on :9222) <br>→ `python bridge/navigate.py "<url>"` (+ `form_fill`/`combo_select`/`form_click` for SPA searches; let async price/hotel SPAs settle ~25s) <br>→ `python bridge/ota_capture.py --source <id> [--url-contains <s>]` (UNREDACTED text → `captures`; prints `capture_id`) <br>→ **agent reads `captures.raw_text`, extracts the offers, emits TSV** <br>→ `./bin/travel ota write-offers <job_id> --capture <capture_id> --claim-token <tok> --tsv <path>` (under a claimed `ota_jobs` job; writes `offers` + provenance + attempt audit) |
| Non-OTA URL | Use WebFetch as normal |

The bridge navigates/clicks the actual UI (no fragile URL templates). Captures live in the Turso
`captures` table; offers go to the `offers` table.

**Extraction is agent-first — the coding agent IS the parser (2026-06-30).** The in-CLI
regex/custom-parser path is **RETIRED**: `travel ota parse`, the generic regex parser, the
per-source custom parsers (e.g. `parse_settour`), and the `parser_rules` table are gone from
travel-cli (`travel ota parse` now fail-louds → "use write-offers"; an orphaned `parser_rules`
table may remain on the live DB, unread). The CLI's job is to **fetch the capture** (gwebcdb) and
**persist the offers the agent hands back** (`ota write-offers`: TSV → normalized `offers` rows +
`agent_parse` provenance + token-guarded `ota_jobs`/`ota_attempts` audit). There is no text the
agent can't parse after the capture returns, so no in-CLI parser is needed — and the old regex path
was strictly worse (the retired `parse_settour` mis-read the real settour `/product/v2`: it divided
the un-taxed total by pax and grabbed UI chrome as the hotel; the correct value is the page's
`每人機加酒含稅$NN,NNN`). **`settour` (1 combo) and `eztravel` (10 combos) are live-verified
end-to-end on WSLg via this agent-parse path (2026-06-30)** — eztravel's 10 same-flight/same-date
combos all persisted as 10 distinct offers, proving the disambiguation fix in prod. Recipe + gotchas (`ota` is in the DEBUG binary
until the next `make build`; TSV `type` is the offer KIND `package|flight|hotel`, NOT the job's
`product_type` like `fit`): memory `settour-live-verified-agent-parse`. `affected_row_count==0` on a
write is a real ON-CONFLICT dedup, not a failure.

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
| `/scrape-ota` | `src/skills/scrape-ota/SKILL.md` | Scrape OTA sites (gwebcdb on WSLg; chromeport retired) |
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

Provider coverage is DB data — run `travel ota-status` (catalog edited via `travel set-ota-*`).

> OTA URL details and scraping patterns → `src/skills/scrape-ota/SKILL.md`

### Scrapers — DECOMMISSIONED; OTA pipeline lives in gwebcdb (WSLg)
Python scrapers archived under `archive/broken-python-scrapers/` — never run. **The entire OTA
pipeline now lives in `gwebcdb`** (`~/b/gwebcdb`): WSLg-native Chrome is the verified default
backend, and the extraction half (`parser_rules` → verify → parse → `offers`) was ported to Python
bridge tools (`turso_db.py`, `ota_capture.py`, `ota_parse.py`, `ota_cli.py` — Phase 0 SHIPPED).
**`chromeport` (the old Rust CDP driver) is RETIRED** — don't run `./bin/chromeport`, repair it, or
treat it as a fallback; WSLg replaced it because it was too fragile. The verified command recipe is
in gwebcdb's `CLAUDE.md` ("OTA scraping — end-to-end usage"); see also the URL Routing banner above
and `docs/plans/2026-06-24-ota-migration-chromeport.md`. gwebcdb is NOT "read-only finance only" —
it is the shared multi-function CDP toolset, and ALL OTA scraping goes through it.

## Current Status

Completed trips — full bookings, itinerary, and weather notes archived:
- **Tokyo Feb 13-17** → `docs/trips/2026-tokyo.md`
- **Kyoto Feb 24-28** → `docs/trips/2026-kyoto.md`

**Upcoming: Okinawa (Naha) 2026-06-12 → 06-16** — `okinawa-2026`, active and populated (flights CI120/CI121, HOTEL AZAT NAHA, 5-day itinerary). Status: `./bin/travel status --full --plan-id okinawa-2026`.

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
# After shaping-init: capture offers via gwebcdb (WSLg), agent-extract, then import + compare:
#   cd ~/b/gwebcdb && ./scripts/start-chrome-cdp-wslg.sh && python bridge/navigate.py "<url>"
#   → python bridge/ota_capture.py --source <id>   # → capture_id (UNREDACTED → captures)
#   → AGENT reads captures.raw_text, emits TSV → ./bin/travel ota write-offers <job> --capture <id> --claim-token <tok> --tsv <path>
#   → ./bin/travel shaping-import --run <run_id> --file <handoff.json>
./bin/travel shaping-compare --run <run_id>
./bin/travel shaping-adopt <candidate_id> <plan_id> --create-plan --dest <slug>

# Offers (Turso)
./bin/travel import-offers --dir scrapes --dest tokyo_2026 [--start ... --end ...] [--dry-run]
./bin/travel promote-offers --from-offers --dest tokyo_sep_2026 --plan-id tokyo-sep-2026 [--source <id>] [--dry-run]   # global offers → plan_offers (then select-offer)
./bin/travel query-offers --plan-id tokyo-2026 --dest tokyo_2026 [--max-price 30000]
./bin/travel check-freshness --source besttour --plan-id tokyo-2026 --dest tokyo_2026

# Bookings
./bin/travel sync-bookings [--dry-run]
./bin/travel query-bookings --dest tokyo_2026 [--category activity --status pending]
./bin/travel validate-itinerary --dest tokyo_2026

# Comparison (read-only)
./bin/travel compare content-depth --plan-id <drill> [--against okinawa-2026]   # depth oracle: SHORT/ALIGNED/BETTER vs reference

# Scraping — Python scrapers DECOMMISSIONED + chromeport RETIRED; use gwebcdb (WSLg) from ~/b/gwebcdb.
# Extraction is AGENT-FIRST: capture, then the coding agent reads raw_text and writes offers (no in-CLI parser).
#   export TURSO_URL=$(grep '^TURSO_URL=' ~/b/travel-2026/.env | cut -d= -f2-)
#   export TURSO_TOKEN=$(grep '^TURSO_TOKEN=' ~/b/travel-2026/.env | cut -d= -f2-)
#   ./scripts/start-chrome-cdp-wslg.sh && python bridge/navigate.py "<url>"
#   → python bridge/ota_capture.py --source <id>            # → capture_id (UNREDACTED → captures)
#   → AGENT reads captures.raw_text, extracts offers, emits TSV
#   → ./bin/travel ota write-offers <job> --capture <capture_id> --claim-token <tok> --tsv <path>  # → Turso offers + provenance
# (`travel ota parse` / the regex parser_rules path is RETIRED.) See URL Routing + gwebcdb CLAUDE.md + src/skills/scrape-ota/SKILL.md.

# Tour-group / FIT offers (manual entry for sources without a full scraper)
./bin/travel import-tour-group-offers --run <run_id> --file <path>
./bin/travel query-tour-group-offers --run <run_id> [--source <id>] [--nights N] [--max-price TWD]
./bin/travel shaping-baseline --run <run_id>                # methodology comparison view
./bin/travel shaping-purchase-matrix --run <run_id> [--qualified-only] [--limit N]   # purchase decision matrix: scores each option (flight + packages) vs shaping_rules (hard=GATE, soft=NUDGE); read-only
./bin/travel add-besttour-offer --url <url> --price <twd> --hotel "<name>"
./bin/travel add-lifetour-offer --url <url> --price <twd> --hotel "<name>"

# Mutations — only the 4 most common shown here.
# Full list (add-activity [--after], move-activity, reorder-activities,
# delete-activity, set-meals, set-airport-transfer, set-activity-time,
# set-day-theme, set-route-segment, set-tod-zh [--clear-activities],
# set-tod-focus [--zh], swap-days, run-status, check-booking-integrity,
# set-process-status <proc> <status> (advance the ladder via the shortest legal
#   path — the ingest-known ladder-mover; select-offer auto-advances P3/P4), …)
# lives in docs/reference/CLI.md. Add new mutation examples THERE, not here.
# Discover table columns with `db schema <table>` before any raw `db exec`.
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

Plan resolution: `--plan-id` and `$TRAVEL_PLAN_ID` win. Without those, the CLI uses `--travel-date`, `--travel-start/--travel-end`, or exactly one active or upcoming DB date anchor/planning window. Use `--travel-*` for plan selection; plain `--start/--end` are command-specific filters (e.g. offer search ranges). If several plans match, the CLI fails with a plan list instead of silently loading a legacy default. `plan_id` uses hyphens (`tokyo-2026`), `destination` uses underscores (`tokyo_2026`) — convert by swapping `-`↔`_`.

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
│   ├── chromeport/                # RETIRED CDP OTA driver (still builds; OTA now = gwebcdb on WSLg)
│   └── turso-util/                # Turso token mint/cache + libsql connect + migrate runner
├── src/skills/                    # LIVE skill defs (SKILL.md + references) — ONLY live part of src/
├── archive/ts-cli-retired/        # retired TS CLI: src/ (minus skills), tests/, scripts/*.ts (read-only)
├── data/                          # holiday calendar + zone/route reference JSON read by the binary
└── docs/                          # API.md, EXTENDING.md, SKILL_TEMPLATE.md, reference/CLI.md, plans/
```

Config/reference data all live in Turso (no JSON files): `destination_config`, `ota_sources`, `origin_config`, `global_config`; OTA rules in `airlines`/`booking_types`/`platform_behaviors`/`comparison_rules`; destination reference (areas/POIs/clusters/transit/tips) in `destination_areas` (+ child tables), `destination_pois`, `destination_clusters`, `destination_transit`, `destination_tips` — read via `./bin/travel query-destination-ref`. Re-seeding a fresh/empty DB uses the seed pipeline (the original TS seed scripts are under `archive/ts-cli-retired/scripts/`; reusable seeders are `./bin/travel db seed …` + inline `seed_*` in `db_migrate.rs`). The OTA provider catalog cold-start is checked-in SQL run insert-if-absent on every `db migrate`: `scripts/seed/ota_catalog.seed.sql` (product types + block reasons) and `scripts/seed/ota_coverage.seed.sql` (the per-`(source, product_type)` coverage matrix + region codes); the notes audit rows come from `backfill_ota_notes_audit` in `db_migrate.rs`. Seed-file rule: one statement per line, no `;` or `'` inside comments (the splitter splits on `;` before stripping comment lines).
Note: `ref_path`/`scraper_script` must be repo-relative paths.

## Turso DB
```
Database: travel-2026 | Region: aws-ap-northeast-1 | Creds: .env (gitignored)
```
Schema: `scripts/schema.sql` (auto-generated DDL, read-only; do not hand-edit). No JSON in any column — `*_json` columns were re-normalized; don't reintroduce them.
Migration: `./bin/travel db migrate` | Seed: `./bin/travel db seed plans` (one-time, already run)

**Token resolution / sandbox gotcha** — `./bin/travel` resolves its Turso token via turso-util in this order: `TRAVEL_TURSO_{READ,WRITE}_TOKEN` env → cache file → mint via the `turso` CLI broker (`turso auth login`). **In a sandbox the broker/cache/login usually fail** (`turso auth not available`), so the CLI can't reach Turso even when `.env` exists. Fix: export the static `.env` token into the env vars the CLI reads, e.g.
```bash
export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-)
export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
export TRAVEL_TURSO_WRITE_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
```
The pre-commit hook also runs `validate data` via the CLI, so set these before committing or the hook fails on a token error (not a real data error). NOTE: `.env` is gitignored — a fresh clone/sandbox won't have it at all; one must be provisioned out-of-band. For ad-hoc reads/writes when the CLI can't get a token, the `/v2/pipeline` HTTP API with the `.env` `TURSO_TOKEN` works (but prefer the audit-trail CLI for mutations).

### DB Operation Decision
- **Reusable operation** (editing itinerary content, updating themes, managing activities) → build a UI/CLI interface
- **One-shot operation** (migration, schema change, data backfill) → direct SQL via turso-exec is acceptable
Before running raw SQL for content edits, ask: "Will this be done again?" If yes, build the interface first.

## Trip Dashboard (Cloudflare Worker)

**Two workers exist** (both read Turso directly, SSR):
- `workers/trip-dashboard-rs/` — **Rust / workers-rs** (current; **at TS feature parity** as of 2026-06-29 — the booking-summary package-offer pricing, hotel access lines, per-day map landmarks, and Japan-only entry rows were the last render gaps, now ported with tests (commit `e7c2a89`); `airport_transfer_candidates` is intentionally out-of-scope — the legacy TS worker reads but never rendered it and no plan has rows. Audit: `.review/rs-worker-completeness-audit-2026-06-29.md`). Live at **`trip-dashboard-rs.yanggf.workers.dev`**. **Auth: GitHub OAuth for owner dashboard pages** (gated on immutable GitHub id `ALLOWED_GITHUB_ID` + `ALLOWED_LOGIN`; signed `__Host-td_session` cookie; styled sign-in / not-authorized pages; routes `/auth/login|callback|logout`) via the **shared `gwebcdb/crates/worker-github-oauth` crate** (the SAME crate the finance `plan-viewer-rs` worker uses — cross-repo path-dep like `turso-util`). **Sharing unchanged**: per-plan share tokens in `plan_share_tokens` → `?token=<tok>` still render for logged-out viewers (NOT OAuth-gated). `?token=` is viewer-only; owner access is OAuth-session-only; authenticated HTML is served `Cache-Control: private, no-store` because pages can contain bearer voucher/share URLs. OAuth config: secrets `SESSION_SECRET`/`GITHUB_CLIENT_ID`/`GITHUB_CLIENT_SECRET`/`PUBLIC_ORIGIN` + vars `ALLOWED_LOGIN`/`ALLOWED_GITHUB_ID` (deploy steps: `docs/plans/2026-06-23-dashboard-github-oauth.md`). Plus keyless route maps (per-day + plan PNGs with numbered markers + route polyline, chromeport→Leaflet→R2 buckets `MAPS`/`VOUCHERS`), meal-pin `<label>｜map:<query>` links, pending-booking alerts, transit cheat-sheet, clickable flight links. Non-place activities (flights/airport steps/bare meals) are excluded from stop links + maps. Deploy: `cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy` (runs `worker-build --release`). **Share a plan:** `./bin/travel share-token` (mint) / `share-token --show` (fingerprints + status) / `share-token --show-full` (full sensitive URLs) / `share-token deactivate <token>`; or on the signed-in dashboard/plan page, **Copy share link** (clipboard = `?plan=<slug>&token=<share_token>` for viewers — no login for recipients). Plan: `docs/plans/2026-06-25-dashboard-share-link-copy.md`. **Refresh maps:** `./bin/travel snapshot-maps`.
- `workers/trip-dashboard/` — **legacy TS** worker, **RETIRED 2026-07-02**: undeployed from Cloudflare (`wrangler delete`); source kept in-repo pending a 2026-08-02 archive-or-delete review. The section below still describes its behavior for reference. The old-URL **cutover was abandoned** as pointless (see Next Steps) — instead of reclaiming the URL for `-rs`, the old `trip-dashboard.yanggf.workers.dev` is now a **301 redirect → `-rs`** via a tiny redirect worker at `workers/trip-dashboard-redirect/` (0.31 KiB, reclaims the `trip-dashboard` name; preserves path+query so old `?plan=…&token=…` share links still land on `-rs`).

Note: the CLI's `crate::checks` module (shift-left audit) holds shared lint predicates (`check_stop_linkable`, `stop_link_problem`, …) used by BOTH the read-only lints AND write-time guards in `set-route-segment`/`set-tod` — so a stop that won't form a valid Maps link is rejected at write time, not just flagged later. (The former `../travel-2026-dashboard-rs` worktree was removed once the branch merged; all dashboard-rs work is on `master` now.)

```
Browser → Cloudflare Worker (SSR HTML) → Turso HTTP Pipeline API → normalized tables → assemble plan object → render
```

- **SSR-only** — no client JS for viewers; logged-in owner plan pages include minimal inline JS for **Copy share link** (clipboard); legacy TS worker edit mode used `?edit=TOKEN` (not in `-rs`)
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
- **Live URL** (canonical): `https://trip-dashboard-rs.yanggf.workers.dev/?plan=tokyo-2026` | `/?plan=kyoto-2026`. The old `trip-dashboard.yanggf.workers.dev` 301-redirects here (path+query preserved).
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

**The Rust port is DONE** (commits through `88385fb`): P1 command parity, P2 scripts, P3 real-Turso integration tests, the `package.json` cutover (root npm retired), TS archived, and docs/skills converted to `./bin/travel`. ADR-001 / "StateManagerV2" is achieved by construction (the Rust CLI *is* the targeted-SQL model) — do NOT refactor the archived TS `StateManager`; that work is complete and the code is read-only under `archive/`. Don't re-port `import-offers-to-turso` (intentionally dropped; replaced by `import-offers` + gwebcdb's `ota_cli.py parse`).

Remaining agenda (none blocking — the project is between trips and the live DB is fully seeded):

- **Test-harness decoupling — DONE (2026-07-05).** Root-caused a prod-row leak to copy-pasted test plumbing (each of ~46 integration tests hand-rolled `bin`/`nanos`/`is_credless`/`db_exec`/`teardown` → the teardown table lists DRIFTED and leaked, plus 5 incompatible `db_exec` signatures + inconsistent credless-skip that false-FAILed instead of skipping). Fixed by one shared harness in `rust/crates/travel-cli/tests/common/mod.rs` — canonical `db_exec(sql)->Option<Rows>` (retry + `[N/M]` strip + credless-skip), `is_credless`/`is_transient`/`bin`/`nanos`/`seed_plan`, the drift-proof `teardown_plan` (dynamic sqlite_master scan) + `teardown_offers`/`db_exec_teardown` for non-plan-keyed rows. Migrated in 12 reviewable batches (commits `7891894`→`8830283`, ~−2,400 net lines). Every test now uses it (only `tests/teardown_plan.rs` keeps its own `db_exec` by design). Convention + the SIGTERM-run-in-background lesson are in the Tests section above + memory `[[canonical-teardown-plan]]`.
- **`set-process-status` command — SHIPPED (2026-07-03).** The `ingest-known` fast-path records flights/hotel via `set-flight`/`set-hotel` (no-cascade), so the P3/P4 process ladder needed a repeatable audited mover (was hand-SQL'd before). `travel set-process-status <process_id> <target_status>` walks the SHORTEST LEGAL path over the state machine (e.g. `pending→populated→booking→booked`), one `status_changed` event per hop, idempotent no-op, fail-loud on no-path. Domain write via `repo::process_statuses::upsert`; audit in `cascade::common`. Wired into the ingest-known path in `/stage2-shop-transport` + CLI reference. (`validate_transition`/`emit_status_changed`/`allowed_transition_targets` promoted from `cascade::select_offer` into `cascade::common` — one shared state machine.)
- **`shaping-purchase-matrix` — WIRED into the flow (2026-07-03).** The read-only purchase-decision matrix (scores flight+packages vs `shaping_rules`, hard=GATE/soft=NUDGE) was shipped but orphaned — no skill invoked it. Now referenced CONDITIONALLY (only when a shaping run has offers) in `/shaping-research` (after ranking, before adopt) + `/stage2-shop-transport` (mode `shop`) + the Skill Decision Tree.
- **Drill = DIAGNOSTIC; 3 CLI/flow logic defects found + FIXED (2026-07-05).** The drill's purpose is to find misses in the toolset + flow (fix the LOGIC, not the one plan). A gwebcdb visual compare of the drill vs okinawa (via share-token viewer links) + DB diff exposed 3 root-cause defects, each fixed via the pipeline: **D1** (`9ecdd23`) — `populate-itinerary` had the poi_id in hand but `insert_populated_activity`'s INSERT omitted the column → every populated activity was poi_id=NULL → broke dashboard maps + POI linking for EVERY scaffold+populate plan; fixed by threading poi_id into the DAL insert. **D2** (`48f5c89`) — POI reference data had no coord contract (3 of 4 destinations 0-geocoded; the seeder's INSERT couldn't write lat/lon); fixed with `set-poi-coords` (slug-keyed, no audit) + `validate data` warns on ungeocoded POIs + seeder carries lat/lon. **All 4 destinations are now fully geocoded (2026-07-06): tokyo/kyoto/okinawa/osaka_kyoto — 0 ungeocoded POIs, `validate data` clean.** **D3** (`0cbfdb9`) — the flow promised "verify publish readiness" but nothing enforced it → the drill shipped a bare page; fixed with `travel validate publish --plan-id` (BLOCK on missing P5/itinerary-errors/ZH/map-path; WARN on stale-maps/missing-weather; past-trip + empty-session guards), and Stage-4 skill runs it. Live-verified: the gate caught the drill's real gap (arrival/departure transit had no ZH) then passed once filled. Confirmed EXPECTED (not bugs): scaffold/populate never seed route-segments/meals — always hand-authored. Memory: `[[drill-as-diagnostic]]`.
- **Drill `kyoto-jul-2026` — CLOSED OUT through all 4 stages (2026-07-05).** A rehearsal plan (synthetic `[DRILL]`-labeled data) driven end-to-end to exercise the flow: Stage 1 (dates+dest, known-flights fast-path), Stage 2 (ingest-known: drill flights SL396/SL397 + `[DRILL]` hotel, P3/P4→booked via `set-process-status`), Stage 3 (scaffold 5 days + 9 activities from real kyoto_2026 reference clusters, P5 researched), Stage 4 (readiness verified — validate-itinerary 0/0/1 — but publish DELIBERATELY SKIPPED and recorded via `flow-decision publish skip`: it's synthetic drill data, and deploy/share is Yang-gated (wrangler login + GitHub OAuth)). NOT a real trip — do not publish it. ZH content left empty (no-cheat; a real trip would need `set-day-theme --zh`/`set-tod-zh` before Stage 4).
- **`--dest` in view commands — RESOLVED (validation-only by design, 2026-07-02; Codex-advised + Claude-corroborated).** For `bookings`/`itinerary`/`transport`, `--dest` is **validation-only**: the view always renders the plan's active destination; a matching (or absent) `--dest` is a no-op and a mismatching one **fails loud** (`plan::assert_dest_matches(dest_opt, &view.active_destination)`, called right after `plan::load` in all three — `view_{bookings,transport,itinerary}.rs`; help text + comments say so). This is the correct, complete behavior for the current reality: **every plan is single-destination** (verified: tokyo/kyoto/okinawa each have exactly one `date_anchors.destination`), so there is no non-active destination to render, and the old silent-ignore bug (which showed the active dest for a wrong `--dest`) is fixed. Rendering a *non-active* destination is **deferred by design, not a bug** — building it now would be untestable speculative code (no multi-dest plan exists). **Future trigger:** the first real multi-destination plan. **Future recipe:** add a dest field to `plan_resolver::ResolveInput`, thread a `dest_override: Option<&str>` through `plan::load` (`plan.rs:273`) into the ~15 dest-keyed repo reads (`repo::{date_anchor,flight_legs,airport_transfers,hotel_*,offer_*,days,…}`), then verify against real multi-dest data.
- **Worker `workers-rs` port — DONE & DEPLOYED; legacy TS RETIRED (2026-07-02).** The Rust dashboard lives at `workers/trip-dashboard-rs/`, canonical at `trip-dashboard-rs.yanggf.workers.dev` (keyless maps, token auth, meal-pin links). **The old-URL cutover was ABANDONED as pointless** — `-rs` already served its own URL fully, so reclaiming the original URL (and putting OAuth in front of a previously-open URL) bought nothing. Instead: the legacy TS worker was **undeployed** (`cd workers/trip-dashboard && npx wrangler delete`; source kept for a 2026-08-02 archive-or-delete review), and the old `trip-dashboard.yanggf.workers.dev` now **301-redirects → `-rs`** via a 0.31 KiB redirect worker (`workers/trip-dashboard-redirect/`, `feat` commit `23d2e7d`; reclaims the `trip-dashboard` name, preserves path+query so old bookmarks + `?plan=…&token=…` share links land on `-rs`). Verified live (301 → HTTP 200 on follow). The `[env.production]` cutover block + `scripts/deploy-cutover.sh` remain in-repo as an unused record only. Redeploy the redirect if ever needed: `cd workers/trip-dashboard-redirect && npx wrangler deploy`. **D1 read-mirror pilot** (`scripts/deploy-d1-pilot.sh`) is independent + still optional.
- **OTA scraping pipeline (gwebcdb / WSLg).** Agent-parse reads capture `raw_text` → TSV → `bridge/ota_write_llm_offers.py`; regex `ota_cli parse` is the fallback. Per-agent Chrome uses `bridge/chrome_session.py acquire`. `chromeport` is RETIRED — don't run/repair it. The `promote-offers` bridge moves global `offers` into plan-scoped `plan_offers` for `select-offer`; spec: `docs/superpowers/specs/2026-06-28-promote-offers-bridge-design.md`. Provider coverage is DB data — run `travel ota-status` (catalog edited via `travel set-ota-*`). Form-driving recipe: gwebcdb `CLAUDE.md` "OTA scraping — end-to-end usage". Plan + gate: `docs/plans/2026-06-24-ota-migration-chromeport.md`.
- **Rust-first OTA execution layer (SHIPPED + hardened; extraction is agent-first).** The capture→offer path: gwebcdb captures the page → **the coding agent reads `captures.raw_text` and extracts offers** → `travel ota write-offers` persists them (TSV → normalized `offers` + provenance + token-guarded audit). Commands: `travel ota enqueue|claim|heartbeat|finish|reap-stale|write-offers|observations` (modules under `rust/crates/travel-cli/src/ota/`), token-guarded job lifecycle on the normalized `ota_jobs`/`ota_job_params`/`ota_attempts`/`ota_observations` tables + provenance columns on `offers` (`capture_id`/`produced_by_*`/`parser_method`/`*_checksum`/`normalizer_version`). Spec: `docs/superpowers/specs/2026-06-29-rust-first-ota-db-architecture-design.md`. A multi-agent xhigh code review (2026-06-30) found + fixed 14 defects: PK-collision disambiguation (Python `_disambiguate_ids` parity), job-failure recording so a mid-loop error parks the job `failed` (not wedged `running`), `attempts`/`max_attempts` enforced (claim skips exhausted; `reap-stale` parks them), `observations::record` wired into the failure path + the view renders all 17 columns, restored TSV guards (alignment/dup-header/digit-dates/negative-nights), flag-aware positional parsing, `--now`/`--lease-seconds` validation, airline-inference-from-flight#, 36-char UUID. **The in-CLI regex/custom-parser path is RETIRED (2026-06-30)** — `travel ota parse`, the generic regex parser, the per-source custom parsers (`parse_settour`), and the `parser_rules` table were deleted from travel-cli; the agent is the parser. **All 6 registered sources are live-verified** end-to-end via the agent-parse (`write-offers`) path (settour/eztravel/besttour 2026-06-30; travel4u/google_flights/agoda 2026-07-01) — see the summary at the end of this block. Onboarding a new source needs one live WSLg capture + `write-offers` to clear the bar (no per-source code — the agent extracts). **DAL (`travel-db`) adoption IN PROGRESS** (spec Phase F, with a `sql_quote()` migration ledger): `view_bookings` (`repo::bookings::book_by_deadlines`), the four offer-query commands `query-offers`/`db query-offers`/`compare dates`/`compare true-cost` (shared `repo::offers::OfferFilter` parameterized WHERE builder, incl. `departure_window`/`fresh_within_hours`), `check-freshness` (`repo::freshness`, both offers + plan-provenance paths), `query-bookings` (`repo::bookings::query_current` — this migration also **fixed a latent prod bug**: the old SELECT referenced a phantom `payload_text` column, so `query-bookings` errored "no such column"; now works), `query-destination-ref` (`repo::destination_ref`, 9 slug-keyed reads), and `plan.rs` (the load-bearing reader — all 16 reads now extracted into `repo::plan`; `plan::load` keeps only the `PlanView` assembly; golden byte-identical across `status --full`/`itinerary`/`bookings`/`transport`) are migrated. **`sql_quote()` is fully retired** — `grep -rn sql_quote rust/crates/travel-cli/src/` returns nothing; every dynamic business-table query binds its values. The DAL boundary for the read views is complete: all four view commands now read through `travel-db` repos. **Mutation-command DAL adoption has started** (optional consistency refactor — mutations already bind params, were never `sql_quote` offenders): `set-route-segment`/`-bulk` → `repo::route_segments`, `set-day-theme` → `repo::days`, `set-hotel` → `repo::hotels`, and `set-flight` → `repo::flight_legs` (domain writes moved; the audit triad `plan_events`/`operation_runs`/`plans.version` deliberately left in `cascade::common` per the DAL boundary contract). `repo::days::exists` is the shared "days row exists" guard for itinerary mutations (reuse it). Pattern for the rest: domain writes → `travel-db` repo, audit stays in `cascade::common`. Operational/diagnostic SQL like `db_migrate`/`db exec` is EXEMPT — stays inline. Add new offer predicates to `OfferFilter`, not a fresh `sql_quote`. **DAL mutation status (Codex inventory + Claude-corroborated 2026-07-01):** only the 4 listed commands are migrated; the large remaining inline-SQL surface (ranked, as of the start of the 2026-07-02 offer-pipeline sweep): (1) `shaping.rs`/`tour_group_bridge.rs` (plan creation/adoption — biggest), (2) offer pipeline (`import_offers`/`promote_offers`/`cascade::select_offer`/`update_offer`), (3) itinerary cluster (`set_activity`/`set_tod`/`scaffold_itinerary`/`populate_itinerary`/`swap_days`), (4) booking/status/weather/transfer/catalog (`mark_booked`/`sync_bookings`/`set_airport_transfer`/`weather`/`set_ota_catalog`/`mark_plan_deleted`). [SUPERSEDED — see the updated progress at the end of this paragraph: `promote_offers`/`update_offer`/`import_offers` from group (2) and `sync_bookings` from group (4) are now migrated.] No `travel-db` repo writes the audit triad (boundary intact). **Consistency nit FIXED (2026-07-02):** `set-day-theme`/`set-hotel`/`set-flight` now route their audit through `cascade::common::record_operation` (matching `set-route-segment`) instead of hand-rolling the `operation_runs` INSERT + `plans.version` UPDATE; 3 duplicate local `new_run_id` helpers removed (−106 lines). Verified: `set_mutation_bugs` 14/14 green + live smoke (version bump + `operation_runs` row land). So all migrated mutation commands now use the shared audit back-half. **Offer-pipeline DAL adoption is now well underway (2026-07-02):** `promote-offers` (`repo::plan_offers::insert_offer`/`delete_offer_rows`/`upsert_process_status`), `update-offer` (`repo::plan_offers::upsert_date_pricing` — the point-UPSERT variant), and now `import-offers` (`repo::plan_offers::insert_import_offer` + `insert_import_provenance` + `insert_warning`, deduping the byte-identical local `delete_offer_rows`/`upsert_process_status`, audit→`record_operation`; commit `2670fe0`, byte-identical, behavior-locked by `tests/import_offers.rs`) are migrated. Only `cascade::select_offer` remained in the offer-pipeline group. **UPDATE 2026-07-02 (later same day): the offer-pipeline group, the leaf/booking commands, AND the whole shaping family are now migrated.** All byte-identical, each behavior-locked (test written first) + reviewed line-by-line + verified serialized against shared Turso: `cascade::select_offer` (`flight_legs::replace_from_offer`, `hotels::replace_from_offer`, `plan_offers::set_selection`, new `repo::process_statuses::upsert`; `ac21aeb`); leaves `mark-plan-deleted`→`repo::plan_lifecycle::soft_delete` (`3599c7f`), `set-airport-transfer`→`repo::airport_transfers` (`26d2d6e`), `set-ota-*`→`repo::ota_catalog` (`69cc550`), `mark-booked`→`repo::bookings::resync_current` (`1a18fe2`); and staged **shaping** Units 1-5 (`9bf86bf`/`6ff52a2`/`8aa789a`/`427a7c1`/`f85ec8e`): `run_init`/`run_import`/simple-adopt-pointer → `repo::shaping`, `tour_group_bridge` sink → `repo::plan_offers::insert_bridge_offer`, `run_adopt --create-plan` seed → `repo::plan_lifecycle::create_plan_seed` (plain-INSERT process_statuses, NOT the upsert — a Codex-flagged trap). **UPDATE 2026-07-02 (later): the itinerary cluster is now MIGRATED too — DAL adoption is effectively COMPLETE.** All 5 itinerary commands moved into ONE shared `repo::itinerary` (built incrementally per step), each behavior-locked (test written first, all 5 committed) + byte-identical + verified serialized: `scaffold-itinerary` (`replace_skeleton` ordered `{tbl}`-delete + skeleton insert, `upsert_process_status_replace` INSERT-OR-REPLACE, `clear_dirty`; `647b5e6`), `set-tod` (`update_tod_field`+trailing touch / `update_tod_time_range` no-touch / `replace_session_activities_zh` / `replace_session_meals`; `84ab065`), `set-activity` family (`update_activity_field`/`_title` clear-poi branch/`delete_activity`/`move_activity` NO-updated_at/`shift_activities_after`/`insert_activity`/`update_activity_sort_order`; `fa93f67`), `set-activity-poi` (`poi_exists`+`set_activity_poi` rows_affected==1; `4ac7ced`), `populate-itinerary` (its DISTINCT activities column-set incl. booking_url/cost_estimate, `insert_activity_tag_ignore`; `9542ef3`), `swap-days` (`swap_day_theme` 2-col NO-updated_at — the observable updated_at change comes only from the separate `touch_day` calls — + `swap_session_day_numbers` TMP=-999999 3-step repoint; `5d22b85`). **UPDATE 2026-07-02 (final): the `weather` days.weather_* writer is migrated too (`f411f9d`) — DAL adoption is COMPLETE.** `weather.rs::write_day_weather`'s `UPDATE days SET weather_*` moved into `repo::itinerary::set_day_weather` (SQL verbatim: `weather_source_id='open_meteo'` kept a SQL LITERAL, `weather_sourced_at`=?8, `updated_at`=now_db ?9); the weather_updated events + open-meteo fetch + `finalize`/record_operation stay in the CLI. Locked by a DETERMINISTIC repo-level test (`tests/set_day_weather.rs`; an E2E `fetch-weather` lock would be flaky — live open-meteo + 16-day window). **There is now NO remaining inline domain-write SQL in any command module** — every command family (offer-pipeline, leaves, shaping, itinerary, weather) writes through `travel-db` repos; the audit triad stays in `cascade::common` (no repo writes it); `db_migrate`/`db exec` are the only inline SQL by design (EXEMPT). **DAL adoption is now FULLY DONE — no follow-ups remain (`88abbf8`, 2026-07-02):** the last item (fold promote/import's duplicate `plan_offers::upsert_process_status` into the canonical `repo::process_statuses::upsert`) is complete — both callers re-pointed, the byte-identical copy deleted, both behavior-locks green. So `repo::process_statuses::upsert` is the single home for the ON-CONFLICT status upsert (do NOT re-introduce a per-module copy); `scaffold_itinerary`'s `repo::itinerary::upsert_process_status_replace` is a distinct INSERT-OR-REPLACE fn. Every command family writes through `travel-db` repos with no duplicated domain-write helpers; `db_migrate`/`db exec` are the only inline SQL by design. Pipeline note: itinerary + weather migrations used fresh independent Claude Code agents (then Grok via grok-cc:rescue for weather) as implementers when Codex was rate-limited; each diff reviewed line-by-line + verified byte-identical live + committed with explicit pathspecs. (Pipeline used all session: Codex plan/advise → behavior-lock-test-FIRST → impl (Grok-in-worktree, or hand-from-source when Codex/Grok were rate-limited) → Claude verify byte-identical serialized; the audit triad stays in `cascade::common` — no repo writes it.) **Resolver extension SHIPPED (2026-07-01): the 4 product_types are the input contract (type covers IN/PROCESS/OUT).** `product_type_inputs` (DB table) declares each type's COMMON inputs + DISTINCT token-key roles; the resolver fills COMMON from caller→DB-default (`origin_airports`/`origin_config`)→code-default, and DISTINCT from `ota_source_url_param` (renamed from the muddy `ota_source_url_token`). Onboarding a normal OTA is now pure DB data — `set-ota-workflow` + `set-ota-url-param`, zero code. Plans: Tier-1 `089407a`, Plan-1 contract/mechanics `7e633a4`, rename `10d3629`, Plan-2 onboarding `94894de`/`0dd5429`. **All 6 sources resolve via `ota run --capture-only` with zero per-source code** (besttour/settour/eztravel/travel4u/google_flights/agoda); agoda/google_flights values came from live gwebcdb captures (honest-seed: no invented slugs; agoda `hotel_slug` is `input_name='hotel'` to avoid the ambiguity branch). Docs: `docs/superpowers/specs/2026-07-01-ota-resolver-extension-design.md`, `docs/plans/2026-07-01-ota-resolver-plan{1,2}-*.md`. **All 6 registered sources are ✅ Rust-verified end-to-end via `write-offers`** (settour/eztravel/besttour 2026-06-30; travel4u/google_flights/agoda 2026-07-01) — each taken through capture → agent-extract → TSV → `ota write-offers` → offers land with `agent_parse` provenance (travel4u 16 group-tours, google_flights 9 airlines, agoda 1 hotel). All 6 are direct-GET (no form-driving needed yet, so `nav_kind='form'`/`ota_source_form_step` stays deferred). Checklist: `docs/plans/2026-06-30-ota-source-verification-checklist.md`. Still open: onboarding NEW destinations/sources beyond tokyo (all current seed is tokyo-scoped). The gated D1 read-mirror pilot is **CODE-PREPARED (2026-07-02, deploy-gated)** — a compare-only, owner+flag-gated `/diag/d1-compare` route in the `-rs` dashboard worker (`src/d1_compare.rs`, `worker` `d1` feature; reads `plans`+`date_anchors` from BOTH Turso and a D1 mirror and reports the dialect delta; **D1 never serves**; inert until Yang runs `wrangler d1 create` + sets `D1_COMPARE_ENABLED`). Runbook: `docs/plans/2026-07-02-dashboard-d1-mirror-pilot.md`.
- **Product / Okinawa trip (COMPLETED + polished)** — `okinawa-2026` (Naha, 2026-06-12 → 06-16; `okinawa_2026` in `destination_config`; CI120/CI121, HOTEL AZAT NAHA, 5-day itinerary) was built from **pre-decided CI120/CI121 + HOTEL AZAT NAHA inputs entered directly via `set-flight`/`set-hotel`, then scaffolded** — NOT via `shaping-adopt`. The `shaping-20260525-093508` run was exploratory only (90 candidates, **0 adopted**; verified against `operation_runs`/`shaping_candidates` 2026-07-02, correcting an earlier false "adopted" claim — the n=1 evidence behind the planning-flow improvement plan). The trip is **PAST/completed**; the itinerary is now a retrospective record (activities marked `（實際）` = actual). Day 1 lunch was deliberately light/unbooked (CI120 in-flight meal; hotel restaurant breakfast-only). **Per-day polish DONE (2026-07-02):** ZH parity complete (0 gaps), maps fresh, all real activities recorded; cleaned a duplicate Day 5-evening departure-transit note (departure lives in Day 5 morning) and set Day 4 morning to a "slept-in / rest at hotel" focus (mirrors the Day 1-afternoon rest block). Legitimately-empty sessions (Day 1 afternoon rest, Day 5 post-departure noon/afternoon/evening) left empty — no fabricated content (no-cheat). Nothing structural remains.
- **CLI + flow hardening sweep — DONE (2026-07-06 → 07-08).** A drill-as-diagnostic + oracle-driven + multi-AI-pipeline pass that fixed real tool/flow defects (fix the LOGIC, not the plan). Shipped:
  - **`compare content-depth` oracle** (`3c5c1af`→`9b301c1`) — `travel compare content-depth --plan-id <drill> [--against okinawa-2026]`: quality-gated per-axis (activities / real meals / routes-with-metadata / weighted-ZH) SHORT/ALIGNED/BETTER verdict. The drill loop's "is this plan richer than a real trip?" test; **the web-rendered dashboard is the FINAL acceptance gate, this is the mid-loop oracle**. Proven: drove `tokyo-sep-2026` to BEAT okinawa on all 4 axes (37/11/22/100% vs 29/10/21/88%).
  - **`add-transit`** (`8052a40`) — `travel add-transit <slug> <from> <to> --minutes N [...]`: edits `destination_transit` (WAS hardcoded `&'static` in Rust — a no-hardcode/Turso-only violation). Slug-keyed reference data, no `--plan-id`, no audit, idempotent. `transit_key.rs` is the SHARED pair-key normalization both add-transit's write AND derive-routes' lookup use — **never re-inline it**.
  - **`derive-routes` missing-transit report** (`6e48b8f`) — ends with a `⚠ N station pair(s) missing destination_transit metadata` worklist (a ready-to-run `add-transit` line per pair). Closes the loop: derive → add-transit each → re-derive.
  - **reject-unknown-flags sweep** (`ff6c4cf`→`d02261e`) — every mutation command now fails loud on a typo'd flag via the shared `plan_resolver::reject_unknown_flags` (fixed the class where `mark-booked --dry-runn` → real write, `set-ota-coverage --provven` → silent proven=0). Connect-before-parse commands (mark-booked/sync-bookings/fetch-weather) preflight in the `main.rs` dispatch arm.
  - **`--dest` override validation** (`e7f88b8`) — `resolve_active_destination` now rejects a `--dest` that isn't a real destination of the plan (was writing orphaned rows + bumping version). `seed_plan` now seeds a `plan_destinations` row to match production.
  - Details + the remaining low-priority items (A4 populate→derive hint, B6 ✅ glyph parity) in `.review/2026-07-07-cli-improvement-opportunities.md`. Specs/plans under `docs/superpowers/{specs,plans}/2026-07-0{6,7}-*`.
- **Map-coverage WARN + `set-activity-poi --auto` — DONE (2026-07-08).** The osaka-kyoto-redrill fix (day 1/5 rendered empty `地圖(尚未產生)` maps because their `set-activity`-authored stops had `poi_id=NULL`, and NOTHING flagged it) exposed two real gaps, both fixed through the full pipeline (brainstorm → Codex design-review → spec → Codex task-by-task test-first plan → Grok impl → Claude line-by-line review + corroborate + serialized-live verify). Commits `82239f1` (A) + `581d391` (B); spec `097e532`, plan `7ef9cfb`.
  - **A — `validate publish` per-day map-coverage WARN** (`validate.rs::map_coverage_gaps`): a day that HAS activities but ZERO geocoded stops AND zero `day_route_segments` now WARNs that its dashboard map will render empty. Was plan-wide all-or-nothing (`has_map_path`), so 4/5 mapped days passed with 0 warnings. WARN not BLOCK (a pure arrival/departure day can be legitimately map-less); zero-activity days don't warn (content-depth's thin-day INFO owns those). SQL LESSON: driven from an `activity_days` CTE + LEFT JOIN mappable/route counts — the naive "GROUP BY the `has_map_path` inner-JOIN" DROPS exactly the `poi_id=NULL` days you must flag. Reuses the existing `query_day_numbers` helper.
  - **B — `set-activity-poi --auto [--dest]`** (`set_activity_poi.rs::execute_auto`): scans all `poi_id=NULL` activities and batch-links the DETERMINISTIC unambiguous matches — strip a TRAILING CJK/kana/fullwidth ZH gloss, then exact title equality else substring either direction; link ONLY when exactly ONE GEOCODED `destination_pois` row matches. 0/>1/ungeocoded-only → reported in the manual bucket, NEVER guessed (the leading-token rule was designed then DROPPED — Codex flagged `Universal CityWalk`→`Universal Studios Japan` as unsafe). Audit BATCHED like `set-route-segments-bulk`: ONE `operation_runs` row (`auto-linked N POI(s)`) + one `plans.version` bump for the whole run, per-link `activity_poi_linked` event. Stable scan `ORDER BY day_number,session_type,sort_order,id`; duplicate POI targets allowed; `--auto`+positionals fails loud; empty→`nothing to link`; idempotent. Live-smoked on okinawa: correctly declined all 25 meal/`（實際）`-narrative rows (no false links). Verified serialized live: map-coverage 2/2, auto 3/3, regressions 8/8+3/3, unit 10/10. Memory: `map-coverage-and-auto-poi-link`.
