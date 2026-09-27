# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

**This file holds RULES only.** Recipes, how-tos, routing tables, and operational detail live in the owning docs — follow the pointers:
- Routing user requests (Skill Decision Tree, default planning path, OTA URL routing) → `docs/reference/routing.md`
- Commands (canonical full CLI reference) → `docs/reference/CLI.md`
- Architecture narrative + repo tree → `docs/reference/architecture.md`
- Dashboard worker ops, sharing, maps, troubleshooting → `docs/reference/dashboard.md`
- Past trip details → `docs/trips/`

# Travel Project (Japan + domestic Taiwan)

## Facts
- **Schema**: `4.2.0` — destination-scoped with canonical offer model. DB: Turso `travel-2026`, region `aws-ap-northeast-1`, creds in `.env` (gitignored, provisioned out-of-band).
- **Live trip status** — run `./bin/travel plans` / `status --full`; per-trip records live in `docs/trips/`. Don't track trip state here.
- **Naming**: legacy artifacts may say `yokohama-travel-2026`; the project started Japan-only and was extended to domestic Taiwan. Japan-specific machinery (OTA package shopping, JP entry rows, flight legs) doesn't apply to a self-drive domestic trip. `plan_id` uses hyphens (`tokyo-2026`), `destination` uses underscores (`tokyo_2026`) — convert by swapping `-`↔`_`.

## Planning Flow
- **The default path is the known-flights fast-path**: create plan → `set-dates` + `set-flight`/`set-hotel` → straight to the itinerary. There is no "classify the trip" step — trips don't vary in kind. Shaping and offer-shopping are OPTIONAL side-tools, only for trips where flights/dates are NOT yet decided. How transport is acquired is a Stage 2 purchase mode (`shop` | `ingest-known` | `defer`), NOT a top-level router.
- **Stage 2 modes match `flow_decision.rs` MODES**: package/direct COMPARISON is optional; transport/accommodation VALIDATION is mandatory in every mode. Each mode records `travel flow-decision shop mode --mode <m>` (skip it if it's just noise on a single-pattern trip).

## Domestic (Taiwan) trips
A domestic trip is a plan whose destination has a non-JPY currency in `destination_config`. Differences from the Japan path:
- **No OTA package shopping.** `travel flow-decision shop mode --mode defer` on a domestic destination AUTO-ADVANCES P3/P34 to `skipped` (P4 accommodation is NOT skipped — a domestic trip still books a stay).
- **Accommodation is its own table family**, not `offers`: `domestic_accommodations` (+ child `domestic_accommodation_images`, `domestic_accommodation_ratings`). Slug-keyed GLOBAL reference data — **no `--plan-id`, no audit triad** (same family as `add-transit` / `add-omiyage`).
- **Booking a stay for a plan** is the separate audited pair `set-accommodation` (P4 -> `booked`) / `clear-accommodation` (P4 -> `selecting`), which DO write the audit triad.
- **One row per review SOURCE** in `domestic_accommodation_ratings` — the scale is stored with the score. Never average sources into a single number; that publishes a rating nobody gave.
- **Comparison content is data, not prose-in-chat**: `notes`（優劣比較與推薦理由）+ `ranking`（推薦順位，1=首選，NULL=未列入推薦）are what the dashboard renders. Write via `update-accommodation --notes/--rank`; the CLI reminds when missing.
- **A published price needs its read date.** `price_source` + `price_checked_at` render on the dashboard card; `update-accommodation --price-source` without `--price-checked` stamps today. `validate data` WARNs once a rate is older than 21 days.
- **A candidate card has a 位置小圖.** `latitude`/`longitude` (WGS84) render the location minimap on each dashboard candidate card — hub-centered (旅遊地 🚩 at the image center, stay 📍 on the quarter line, dashed connector between them, 距hub distance in the caption); a bare street-grid crop around the stay answers nothing, and so does a both-points-unmarked overview. `--lat`/`--lon` must be given together on `add/update-accommodation` (`clear` on both unsets); geocode via Google Maps place search or OSM/Overpass — never guess; (0,0) null-island is rejected.

## Data Rules
- **Turso cloud is sole source of truth** — fully normalized, 130+ tables, no JSON blobs, no config JSON files.
- **No trip content in the migrator** — `db_migrate.rs` creates SCHEMA and registers destinations in `destination_config`; it must NEVER `INSERT` trip/reference content (a hardcoded seed once RESURRECTED CLI-deleted rows on every integration-test migrate). Content enters only through a CLI command.
- **No local data — fail loud, never fall back** — NO command may read trip/project data from a local file as its source of truth. If a Turso table/row is missing, the command THROWS — never fall back to `research/*.json`, `data/*.json`, or any local export. `if (!dbRow) readLocalJson()` is the bug, not the fix. `scrapes/` is a raw landing zone whose only legal next step is import→Turso→read-from-Turso. A destination MUST be registered in `destination_config` (via `/new-destination`) **before** Shaping Stage researches it. "Is X saved?" → check Turso; local files existing ≠ saved.
- **CLI agent first; plain text only** — user-facing CLI output must be plain text/table lines, not JSON. No JSON files, fixtures, or pipeline boundaries. If structured data is needed, store it in normalized Turso tables and render a plain-text CLI view. JSON is allowed only where an external protocol requires it internally (e.g. the capture envelope, the shaping export/import handoff) — never user-facing.
- **No JSON in the RDB** — don't reintroduce `*_json` columns or parse JSON out of a DB column.
- **Audit trail** — `plans.version` is a monotonic counter bumped per mutation; `operation_runs` records run_id/command/status/version_before-after; domain events go to `plan_events` (+ `plan_event_data` KV).
- **Delegates (Grok/Codex subagents) run NO git operations** — not reset/add/commit/checkout/push. The agent gates every commit itself.
- **DB Operation Decision** — reusable operation (itinerary content, themes, activities) → build a CLI interface; one-shot (migration, schema change, backfill) → raw SQL via `db exec` is acceptable. Before raw SQL for content edits, ask: "Will this be done again?"
- Schema: `scripts/schema.sql` (auto-generated DDL, read-only; do not hand-edit). Discover table columns with `db schema <table>` before any raw `db exec`.

## CLI Rules (Rust)
- Root npm is RETIRED — the Rust binary is the sole write path: `./bin/travel <cmd>` (built from `rust/crates/travel-cli`; `make build` → `./bin/`, gitignored). ONE binary with subcommands; the old TS CLI is read-only under `archive/ts-cli-retired/`. Workers keep their own wrangler/npm setup.
- **One targeted SELECT to validate, one targeted UPDATE/INSERT to write** per command — no in-memory plan object, no assemble step, no coarse flush. CLI output is stdout plain text; diagnostics go to stderr.
- **Audit triad** — every mutating command appends `plan_events` (+ `plan_event_data`), bumps `plans.version`, and writes an `operation_runs` row. Mirror it in any new mutation.
- **Shared mutation helpers — do not re-roll them.** `cascade::common::record_operation(...)` writes the audit-triad back half in one call (emit the `plan_events`/`plan_event_data` rows first; for a freshly created plan pass `version_before=0, version_after=1`). `cascade::common::resolve_active_destination(...)` is the only destination resolver — never reintroduce a private copy.
- **DAL boundary** — domain reads/writes go through `travel-db` repos; the audit triad stays in `cascade::common` (no repo writes it). `db_migrate`/`db exec` are the only inline SQL. Add offer predicates to `repo::offers::OfferFilter`, never string-built/`sql_quote` queries. `repo::process_statuses::upsert` is the single home of the status ON-CONFLICT upsert.
- **Plan resolution** — `plan_resolver::resolve_plan_id`: `--plan-id` > `$TRAVEL_PLAN_ID` > `--travel-date`/`--travel-start`/`--travel-end` > active-today > upcoming > most-recent. Use `--travel-*` for plan selection; plain `--start/--end` are command-specific filters. If several plans match, the CLI fails with a plan list — never a silent legacy default.
- Cascade (normative):
  | Trigger | Reset | Scope |
  |---------|-------|-------|
  | `active_destination_change` | `process_5_*` | new destination |
  | `process_1_date_anchor_change` | `process_3_*`, `process_4_*`, `process_5_*` | all destinations |
  | `process_2_destination_change` | `process_3_*`, `process_4_*`, `process_5_*` | current destination |
  | `process_3_4_packages_selected` | populate P3+P4 from chosen offer | current destination |

## Turso Token Rules
- `./bin/travel` resolves its token via turso-util: `TRAVEL_TURSO_{READ,WRITE}_TOKEN` env → cache file → mint via the `turso` CLI broker. **In a sandbox the broker/cache/login usually fail** — export the static `.env` token into the env vars instead:
  ```bash
  export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-)
  export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
  export TRAVEL_TURSO_WRITE_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
  ```
- The pre-commit hook runs `validate data` via the CLI — set these before committing or the hook fails on a token error (not a real data error). `.env` is gitignored; a fresh clone/sandbox has none. For ad-hoc reads/writes when the CLI can't get a token, the `/v2/pipeline` HTTP API with the `.env` `TURSO_TOKEN` works (but prefer the audit-trail CLI for mutations).
- Migration: `./bin/travel db migrate` (idempotent) | Seed: `./bin/travel db seed plans` (one-time, already run).

## Test Rules
- Real-Turso integration tests in `rust/crates/travel-cli/tests/*.rs`: seed → run binary → SELECT → assert → teardown; skip cleanly if creds absent. Unit tests inline per module.
- **Use the canonical `common::teardown_plan(plan, dest)` — never hand-roll a table list** (it queries `sqlite_master` LIVE; hand-rolled lists DRIFT and leak prod rows). NON-plan-keyed rows a test seeds MUST be torn down locally (teardown_plan only covers `plan_id`-column tables); run such deletes BEFORE `teardown_plan`. Use `common::teardown_offers(&ids)` for global `offers` rows.
- **Never assert on PRODUCTION rows; make fixtures per-run unique** — seed `zz_test_<nanos>_*` ids so a real-price correction can't turn the suite red, and parallel tests in a file can't delete each other's fixtures. Keep fixture names short enough to survive the CLI's 16-char `hotel_name` truncation.
- **Arm the RAII `Guard` right after the plan-id is bound** — never call teardown as the last statement (a panicking assertion unwinds past it and leaks). Optional pre-clean goes BEFORE the guard; never a trailing one.
- **Run the serialized Turso tests in the BACKGROUND** — a foreground timeout SIGTERMs mid-run, the `Guard`'s `Drop` never fires, and an orphan row leaks.
- Fixtures: `rust/crates/travel-cli/tests/fixtures/`. Proof/self-test: `tests/teardown_plan.rs`.

## Build & Pre-commit
```bash
make build        # release binaries → ./bin/ (gitignored)
make dev          # fast debug build
make test         # full Rust suite (real Turso)
make check        # cargo build -p travel-cli
make validate     # ./bin/travel validate data
make doctor       # ./bin/travel doctor
```
Pre-commit hook (installed by `make hooks`) runs the Rust build check + `validate data`.

## OTA Rules
Provider coverage is DB data — run `travel ota-status` (catalog edited via `travel set-ota-*`).
- **All OTA scraping goes through gwebcdb** (`~/b/gwebcdb`, WSLg CDP). The Python scrapers are DECOMMISSIONED (never run) and `chromeport` is RETIRED for OTA (never run/repair/treat as fallback; only its browser/screenshot/db remain, for snapshot-maps). Recipe: gwebcdb `CLAUDE.md` → "OTA scraping — end-to-end usage"; full routing table: `docs/reference/routing.md` → URL Routing.
- **Extraction is agent-first** — the coding agent IS the parser: gwebcdb captures `raw_text`, the agent reads it and persists offers via `travel ota write-offers` (TSV). The in-CLI regex/`parser_rules` path is RETIRED; do not reintroduce it.
- **Do not use WebFetch for OTA sites** (they require JavaScript).

## Dashboard Rules
- The LIVE worker is `workers/trip-dashboard-rs` (Rust, SSR, reads Turso directly). Full ops/sharing/maps/troubleshooting → `docs/reference/dashboard.md`.
- **Deploy**: `cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy`. Always `unset CLOUDFLARE_API_TOKEN` first.
- **Never request or save `tile.openstreetmap.org` raster tiles** (OSM tile policy prohibits automated/headless tile fetching). Composite the ArcGIS static basemap instead. Keep visible `© OpenStreetMap contributors` credit for OSM-derived data plus `© Esri` whenever a basemap is composited.
- **SSR-only, default ZH** — no client JS for viewers (owner pages get the copy-share-link inline JS only); Traditional Chinese by default, `?lang=en` for English. All ZH content stored in DB, never hardcoded in worker code. `lang="zh-TW"` + notranslate meta.
- **DOMESTIC plans have no plan-logistics map** — the worker renders the logistics slot only when flights exist; snapshot-maps merges the booked stay's pin into plan.png.
- Map PNGs are served `max-age=86400` — verify a re-snapshot with `curl`, not a browser holding the old image.
- Sharing: per-plan share tokens (`./bin/travel share-token`) → `?token=` viewer-only; owner access is OAuth-session-only.

## Docs
- `docs/API.md` — complete API reference
- `docs/EXTENDING.md` — how to add destinations, OTAs, validators
- `docs/SKILL_TEMPLATE.md` — skill authoring guide
- `docs/reference/` — CLI.md (commands), routing.md (workflow + decision tree), architecture.md (repo + layering), dashboard.md (worker ops)
- `docs/plans/` — implementation plans for major refactors
- `docs/superpowers/specs/` — methodology specs (Shaping Stage design, price-baseline/rhythm method, tour-group scraper, decision methodology). Read these when the user asks "how should we approach X" rather than "what's the command for X."
- `docs/plans/2026-05-22-new-planning-flow.md` — **adopted** research-first staged planning model. Existing P1–P5 skills remain implementation tools inside the stages.

## Historical Records
Historical implementation records are non-normative and may be superseded: see [`docs/history/README.md`](docs/history/README.md). Current sources (CLAUDE.md / the owning SKILL.md / source code / tests) always win on conflict. Genuinely-open items are in that README's **Open items** section.
