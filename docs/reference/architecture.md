# Architecture & Repo Layout

> Moved from CLAUDE.md 2026-09-28 (CLAUDE.md now holds rules only). This file holds the
> architectural narrative — data model, data flow, CLI layering, and the repo tree.
> The normative rules (audit triad, DAL boundary, fail-loud) live in CLAUDE.md.

## Data Model
Turso: 130+ fully-normalized tables, no JSON blobs. Schema: `scripts/schema.sql` (auto-generated MIRROR of the live `sqlite_master`; regeneration recipe is in its header — the old TS generator is retired); live state: `./bin/travel db status`.

Config/reference data all live in Turso (no JSON files): `destination_config`, `ota_sources`, `origin_config`, `global_config`; OTA rules in `airlines`/`booking_types`/`platform_behaviors`/`comparison_rules`; destination reference (areas/POIs/clusters/transit/tips) in `destination_areas` (+ child tables), `destination_pois`, `destination_clusters`, `destination_transit`, `destination_tips` — read via `./bin/travel query-destination-ref`. `flight_legs` holds fully-normalized flight data incl. `departure_terminal`/`arrival_terminal`.

Re-seeding a fresh/empty DB uses the seed pipeline (the original TS seed scripts are under `archive/ts-cli-retired/scripts/`; reusable seeders are `./bin/travel db seed …` + inline `seed_*` in `db_migrate.rs`). The OTA provider catalog cold-start is checked-in SQL run insert-if-absent on every `db migrate`: `scripts/seed/ota_catalog.seed.sql` (product types + block reasons) and `scripts/seed/ota_coverage.seed.sql` (the per-`(source, product_type)` coverage matrix + region codes); the notes audit rows come from `backfill_ota_notes_audit` in `db_migrate.rs`. Seed-file rule: one statement per line, no `;` or `'` inside comments (the splitter splits on `;` before stripping comment lines). Note: `ref_path`/`scraper_script` must be repo-relative paths.

## Data Flow
`URL → gwebcdb on WSLg (navigate + ota_capture, CDP) → captures.raw_text → AGENT reads + extracts → travel ota write-offers (TSV → Turso offers + provenance) → normalize (CanonicalOffer[]) → selectOffer() → cascade (populate P3+P4) → save() (normalized tables → bookings sync)` — extraction is agent-first (the coding agent is the parser); the old in-CLI regex/`parser_rules` parse step is RETIRED (see `docs/reference/routing.md` → URL Routing).

## CLI Architecture (Rust, fine-grained SQL)
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

Each command is a self-contained module under `rust/crates/travel-cli/src/` that opens its own libsql connection and runs exactly the SQL it needs. There is no in-memory plan object, no assemble step, no coarse flush. Cascade lives in `rust/crates/travel-cli/src/cascade/` (e.g. `select_offer` populates P3+P4). This is ADR-001's target pattern, achieved by construction in Rust — there is nothing left to refactor. (The retired TS `StateManager` / `PlanRepository` / `syncNormalizedTables()` flow is read-only under `archive/ts-cli-retired/`.)

Single-binary CLI: there is ONE binary, `travel`, with subcommands (no per-area binaries). Examples:
- views/status → `travel status --full`, `travel itinerary`, `travel transport`, `travel bookings`
- validation → `travel validate data`, `travel doctor`
- comparison → `travel compare trips ...`, `travel compare dates ...`, `travel compare true-cost ...`, `travel compare content-depth ...`
- utilities → `travel normalize flights ...`, `travel leave calc`
- DB ops → `travel db migrate`, `travel db status`, `travel db seed plans`, `travel db exec "<sql>"`

Roadmap/reference: `docs/plans/2026-06-10-roadmap-v2-rust.md` (active roadmap: tests → scripts port → cutover → archive TS; read before any Rust work). Historical: `docs/plans/2026-06-05-rust-cli-migration.md`, `docs/plans/2026-06-10-rust-port-audit.md`.

## Project Structure
```
/
├── data/                          # only legacy trip notes (tokyo-trip-plan{,-zh}.md); NOT read by the CLI.
│                                  # Holiday/hotel-area/transport-route reference data lives in Turso tables
│                                  # (hotel_areas / transport_routes), read by compare true-cost — no JSON files.
├── scrapes/                       # LEGACY raw-capture JSON landing zone (gitignored) — import→Turso only; not live scraping
├── scripts/                       # NON-TS keepers only: hooks/, schema.sql, *.sql, *.ps1, *.sh, README
│   └── hooks/pre-commit           # cargo build -p travel-cli + ./bin/travel validate data
├── workers/                       # trip-dashboard-rs (LIVE Rust worker), trip-dashboard-redirect (301)
├── Makefile                       # npm-free build/dev entry: build, dev, test, check, validate, hooks, setup
├── bin/                           # built binaries (gitignored): travel, chromeport — via `make build`
├── rust/crates/                   # the LIVE codebase (Cargo workspace)
│   ├── travel-cli/                # the `travel` binary — ALL CLI commands (main.rs dispatch → one module/command;
│   │                              #   db.rs connect_read/write, plan_resolver.rs, db_migrate.rs, cascade/, tests/)
│   ├── chromeport/                # RETIRED CDP OTA driver (still builds; OTA now = gwebcdb on WSLg)
│   └── turso-util/                # Turso token mint/cache + libsql connect + migrate runner
├── src/skills/                    # LIVE skill defs (SKILL.md + references) — ONLY live part of src/
├── archive/ts-cli-retired/        # retired TS CLI (read-only)
└── docs/                          # API.md, EXTENDING.md, reference/{CLI,routing,architecture,dashboard}.md, plans/, superpowers/, history/, trips/
```
