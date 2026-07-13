# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> Routing user requests → jump to **Skill Decision Tree**.
> Commands → see **CLI Quick Reference** (full list in `docs/reference/CLI.md`).
> Past trip details (Tokyo, Kyoto Feb 2026) → `docs/trips/`.

# Japan Travel Project

## Trip Details
- **Schema**: `4.2.0` — destination-scoped with canonical offer model.
- **Live trip status** — run `./bin/travel plans` / `status --full`; per-trip records (Tokyo/Kyoto Feb 2026, Okinawa Jun 2026) live in `docs/trips/`. Don't track trip state here.
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

**DAL boundary (`travel-db` repos).** Domain reads/writes go through `travel-db` repos; the audit
triad stays in `cascade::common` (no repo writes it). `db_migrate`/`db exec` are the only inline SQL
by design. Add a new offer predicate to `repo::offers::OfferFilter` (the parameterized WHERE builder),
never a fresh string-built/`sql_quote` query. `repo::process_statuses::upsert` is the single home for
the status ON-CONFLICT upsert — don't re-roll a per-module copy.

- **Turso cloud is sole source of truth** — fully normalized, no JSON blobs, no config JSON files
- **No local data — fail loud, never fall back** — NO command may read trip/project data (destinations, shaping/constraints, research, ranked candidates, selected offers) from a local file as its source of truth. If a Turso table/row is missing, the command THROWS — it must not silently fall back to `research/*.json`, `data/*.json`, or any local export. A `if (!dbRow) readLocalJson()` path is the bug, not the fix. `scrapes/` is a raw landing zone whose only legal next step is import→Turso→read-from-Turso. A destination MUST be registered in `destination_config` (via `/new-destination`) **before** Shaping Stage researches it. "Is X saved?" → check Turso; local files existing ≠ saved.
- **CLI agent first; plain text only** — User-facing CLI command output must be plain text/table lines, not JSON. Do not introduce JSON files, JSON fixtures, or JSON as the pipeline boundary. If structured data is needed, store it in normalized Turso tables and render a plain-text CLI view from Turso. JSON is allowed only where an external protocol/library requires it internally (e.g. the chromeport capture envelope, the shaping export/import handoff) — never as a user-facing artifact or source of truth.
- **No JSON in the RDB** — every former `*_json` column was re-normalized into child rows or typed scalar columns. Don't reintroduce `*_json` columns or parse JSON out of a DB column.
- **Destination config in DB** — `destination_config` table (no `data/destinations.json`); loaded from Turso. **28+ normalized tables** (see Data Model above). `flight_legs` holds fully-normalized flight data incl. `departure_terminal`/`arrival_terminal`.
- **Audit trail** — `plans.version` is a monotonic counter bumped per mutation; `operation_runs` records run_id/command/status/version_before-after; domain events go to `plan_events` (+ `plan_event_data` KV).
- **Delegates (Grok/Codex subagents) run NO git operations** — not reset/add/commit/checkout/push. The agent gates every commit itself. (A delegate once ran `git reset --soft` and un-committed finished work.)
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
                              #   (chromeport still BUILDS but is RETIRED for OTA — only browser/screenshot/db
                              #    remain, used by snapshot-maps; never use it for OTA scraping — that's gwebcdb)
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
"is the drill/plan rich enough" / "compare depth to a real trip"  → ./bin/travel compare content-depth --plan-id <id> [--against okinawa-2026]  (read-only oracle; 3 depth axes + ZH completeness gate — NOT a 4th %-axis; loop-until-BETTER; web page is final gate)
"derive routes" / "add transit between activities"  → ./bin/travel derive-routes [--day N] [--dest slug]   (cascade ai_recommended legs from the activity skeleton; run after populate)
"add a transit time" / "derived leg has no minutes"  → ./bin/travel add-transit <slug> <from> <to> --minutes N [--line ..] [--kind ..]   (add a destination_transit station pair so derive-routes attaches its time; slug-keyed reference data, no --plan-id, idempotent. Use this instead of raw db exec when a derived leg came back without a duration.)
"empty dashboard map" / "link itinerary POIs"  → ./bin/travel set-activity-poi --auto [--dest slug] first; then manually link any reported misses with ./bin/travel set-activity-poi <day> <session> <poi_id> --match "<title substring>"   (add-activity also prints a 💡 hint at add time when the new title unambiguously matches a geocoded POI)
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
| Any OTA (besttour / liontravel / lifetour / settour / …) | Start Chrome, drive the page, capture, then **the agent reads the capture text and writes offers** (no in-CLI parser): <br>`./scripts/start-chrome-cdp-wslg.sh` (idempotent; CDP on :9222) <br>→ `python bridge/navigate.py "<url>"` (+ `form_fill`/`combo_select`/`form_click` for SPA searches; let async price/hotel SPAs settle ~25s) <br>→ `python bridge/ota_capture.py --source <id> [--url-contains <s>]` (UNREDACTED text → `captures`; prints `capture_id`) <br>→ **agent reads `captures.raw_text`, extracts the offers, emits TSV** <br>→ `./bin/travel ota write-offers <job_id> --capture <capture_id> --claim-token <tok> --tsv <path> --dest <slug>` (under a claimed `ota_jobs` job; writes `offers` + provenance + attempt audit) |
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
#   → AGENT reads captures.raw_text, emits TSV → ./bin/travel ota write-offers <job> --capture <id> --claim-token <tok> --tsv <path> --dest <slug>
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
./bin/travel compare content-depth --plan-id <drill> [--against okinawa-2026]   # depth oracle: 3 depth axes (activities/meals/routes) vs reference + ZH slot-completeness GATE; SHORT/ALIGNED/BETTER

# Scraping — Python scrapers DECOMMISSIONED + chromeport RETIRED; use gwebcdb (WSLg) from ~/b/gwebcdb.
# Extraction is AGENT-FIRST: capture, then the coding agent reads raw_text and writes offers (no in-CLI parser).
#   export TURSO_URL=$(grep '^TURSO_URL=' ~/b/travel-2026/.env | cut -d= -f2-)
#   export TURSO_TOKEN=$(grep '^TURSO_TOKEN=' ~/b/travel-2026/.env | cut -d= -f2-)
#   ./scripts/start-chrome-cdp-wslg.sh && python bridge/navigate.py "<url>"
#   → python bridge/ota_capture.py --source <id>            # → capture_id (UNREDACTED → captures)
#   → AGENT reads captures.raw_text, extracts offers, emits TSV
#   → ./bin/travel ota write-offers <job> --capture <capture_id> --claim-token <tok> --tsv <path> --dest <slug>  # → Turso offers + provenance
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
├── data/                          # only legacy trip notes (tokyo-trip-plan{,-zh}.md); NOT read by the CLI.
│                                  # Holiday/hotel-area/transport-route reference data lives in Turso tables
│                                  # (hotel_areas / transport_routes), read by compare true-cost — no JSON files.
├── scrapes/                       # LEGACY raw-capture JSON landing zone (gitignored) — import→Turso only; not live scraping (see "Scrape terminology")
├── scripts/                       # NON-TS keepers only: hooks/, schema.sql, *.sql, *.ps1, *.sh, README
│   └── hooks/pre-commit           # cargo build -p travel-cli + ./bin/travel validate data
├── workers/                       # trip-dashboard-rs (LIVE Rust worker), trip-dashboard (legacy TS, RETIRED), trip-dashboard-redirect
├── Makefile                       # npm-free build/dev entry: build, dev, test, check, validate, hooks, setup
├── bin/                           # built binaries (gitignored): travel, chromeport — via `make build`
├── rust/crates/                   # the LIVE codebase (Cargo workspace)
│   ├── travel-cli/                # the `travel` binary — ALL CLI commands (main.rs dispatch → one module/command;
│   │                              #   db.rs connect_read/write, plan_resolver.rs, db_migrate.rs, cascade/, tests/)
│   ├── chromeport/                # RETIRED CDP OTA driver (still builds; OTA now = gwebcdb on WSLg)
│   └── turso-util/                # Turso token mint/cache + libsql connect + migrate runner
├── src/skills/                    # LIVE skill defs (SKILL.md + references) — ONLY live part of src/
├── archive/ts-cli-retired/        # retired TS CLI (read-only)
└── docs/                          # API.md, EXTENDING.md, reference/CLI.md, plans/, superpowers/, history/, trips/
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
- `workers/trip-dashboard/` — **legacy TS**, **RETIRED 2026-07-02** (undeployed; source kept pending a 2026-08-02 archive-or-delete review). The old `trip-dashboard.yanggf.workers.dev` 301-redirects → `-rs` via `workers/trip-dashboard-redirect/` (preserves path+query so old `?plan=…&token=…` share links still land). Operational details: [`docs/history/dashboard.md`](docs/history/dashboard.md).

Note: the CLI's `crate::checks` module (shift-left audit) holds shared lint predicates (`check_stop_linkable`, `stop_link_problem`, …) used by BOTH the read-only lints AND write-time guards in `set-route-segment`/`set-tod` — so a stop that won't form a valid Maps link is rejected at write time, not just flagged later. (The former `../travel-2026-dashboard-rs` worktree was removed once the branch merged; all dashboard-rs work is on `master` now.)

```
Browser → Cloudflare Worker (SSR HTML) → Turso HTTP Pipeline API → normalized tables → assemble plan object → render
```

- **SSR-only** — no client JS for viewers; logged-in owner plan pages include minimal inline JS for **Copy share link** (clipboard). (The `-rs` worker has no edit mode; the retired legacy TS edit mode is in [`docs/history/dashboard.md`](docs/history/dashboard.md).)
- **Mobile-first** — phone-optimized day cards with weather (including feels-like temperature), transit, meals
- **Default ZH** — Traditional Chinese by default; `?lang=en` for English
- **ZH content** — All Chinese content stored in DB (`theme_zh`, `focus_zh`, `transit_notes_zh` scalars; ZH activities in `session_activities_zh`; meals in `session_meals`). No hardcoded content in Worker code. Content updates take effect instantly without redeploy. Use `set-day-theme --zh` for day themes, `set-tod-zh` (alias: `set-session-zh`) for session focus/transit/activities, `set-route-segment` for Chinese place names. For bulk new-destination ZH population: copy `scripts/set-kyoto-zh-sessions-v2.ts` pattern (parameterized Turso pipeline queries — required for Unicode/emoji content).
- **Anti-translate** — `lang="zh-TW"` + `<meta name="google" content="notranslate">` prevents browser auto-translation of the ZH page.
- **Multi-plan** — each plan accessed via `?plan=<slug>` (e.g., `tokyo-2026`, `kyoto-2026`). Slug derived from `active_destination` (underscores → hyphens). Root `/` shows plan index page listing all plans.
- **Plan nav** — hidden by default for privacy (shareable links show single plan only); add `&nav=1` to show pill-style plan switcher (plan list from DB via `listPlans()`)
- **Flight links** — Flight numbers in booking summary are clickable Google search links (opens new tab)
- **Activity links** — `https://` URLs embedded in activity text are auto-linkified (`renderActivityText()`); `\n` in activity text renders as `<br>`
- **Day card accents** — colored left border by day type: blue (arrival), green (full day), amber (departure)
- **Routes**: `/` (plan index), `/?plan=<slug>` (single plan, shareable), `/?plan=<slug>&nav=1` (with plan switcher), `/?plan=<slug>&lang=en` (EN), `/api/plan/<id>` (raw JSON)
- **Maps links** — Per-segment Google Maps direction links (transit/walking/driving) for every stop. Route segments stored in `day_route_segments` table, landmarks in `day_landmarks` table. Transit pill text must use place names, not service names (e.g., `成田T2 → 日暮里` not `Skyliner → 日暮里`)
- **Secrets**: `TURSO_URL` + `TURSO_TOKEN` + `GOOGLE_MAPS_KEY` (optional) via `wrangler secret put` (server-side only, never sent to browser — except Maps key which is browser-visible by design; restrict via GCP Console referrer policy)
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
# The LIVE worker is trip-dashboard-rs (Rust). Deploy:
cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy   # runs worker-build --release
# Set a secret (one-time): pipe from .env, e.g.
TURSO_TOKEN=$(grep '^TURSO_TOKEN=' ../../.env | cut -d= -f2-) && unset CLOUDFLARE_API_TOKEN && npx wrangler secret put TURSO_TOKEN <<< "$TURSO_TOKEN"
```

## Build Gate
Pre-commit: Rust build check + `validate data` (see Pre-commit above). Install hooks: `make hooks`.

## Historical Records
Historical implementation records (the retired Next Steps journal) are non-normative and may be superseded: see [`docs/history/README.md`](docs/history/README.md). Current sources (CLAUDE.md / the owning SKILL.md / source code / tests) always win on conflict. Genuinely-open items are in that README's **Open items** section.

