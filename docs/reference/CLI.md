# CLI Reference

Full reference for the `travel` CLI and supporting scripts. CLAUDE.md keeps a short list of the most-used commands; everything else lives here.

> **Execution:** the npm→Rust cutover is **done** (2026-06-10). The CLI is the single Rust binary `./bin/travel` (subcommands), built via `make build`. The root `package.json` is gone; the old TS CLI is read-only under `archive/ts-cli-retired/`. The Cloudflare Worker keeps its own self-contained `package.json` + wrangler.

## Plan resolution
`--plan-id` and `$TRAVEL_PLAN_ID` win. Without those, the CLI uses `--travel-date`, `--travel-start/--travel-end`, or exactly one active or upcoming DB date anchor/planning window. Use `--travel-*` for plan selection; plain `--start/--end` remain command-specific filters (e.g. offer search ranges). If several plans match, the CLI fails with a plan list instead of silently loading a legacy default.

## Views

Each view is a separate subcommand — pick one:

```bash
./bin/travel plans                               # list DB plans and date anchors
./bin/travel status --full                       # booking overview
./bin/travel itinerary                           # daily plan
./bin/travel transport                           # transport summary
./bin/travel bookings                            # booking ledger
./bin/travel status --travel-date 2026-06-20
./bin/travel itinerary --travel-start 2026-06-18 --travel-end 2026-06-25
./bin/travel view-prices --flights scrapes/date-range-prices.json --hotel-per-night 3000 --nights 4 --package 40740
```

## Comparison
```bash
./bin/travel compare-offers --region osaka [--json]
./bin/travel compare trips --input <your-comparison-file.json> [--detailed]   # input file is BYO
./bin/travel compare dates --start 2026-02-24 --end 2026-02-28 --nights 4
./bin/travel compare true-cost --region kansai --pax 2 --date 2026-02-24
```

## Scraping

The Python scrapers (`scraper:*`, `scrape_date_range.py`, `scrape_google_flights.py`, …) are
**DECOMMISSIONED and archived** under `archive/broken-python-scrapers/` — their URL/region/template
construction 404s or lands on the wrong page. Do not run them.

Replacement: the Rust **chromeport** CDP driver attaches to a real Chrome (`127.0.0.1:9222`),
drives the live OTA page, captures plain text → Turso `captures`, then rule-parses → Turso `offers`.

```bash
./bin/chromeport fetch interact "<url>" --source <id> --step ...   # drive the live page (or: browser snapshot)
./bin/chromeport verify <source-id> <capture-id>                  # read-only regex diagnostics
./bin/chromeport parse capture <capture-id> --source <id>         # rule-parse → import to Turso
```

See `src/skills/scrape-ota/SKILL.md` and `docs/plans/2026-06-05-rust-cdp-scraper-migration.md`.

## Turso DB
```bash
./bin/travel import-offers --dir scrapes --dest tokyo_2026 [--start 2026-02-13 --end 2026-02-17] [--dry-run]
./bin/travel query-offers --plan-id tokyo-2026 --dest tokyo_2026 [--max-price 30000] [--json]
./bin/travel query-offers --region kansai --start 2026-02-24 --end 2026-02-28 [--max-price 30000] [--json]
./bin/travel check-freshness --source besttour --plan-id tokyo-2026 --dest tokyo_2026
./bin/travel check-freshness --source besttour --region kansai
# (the old `db:import:turso` raw-offers loader is retired — the chromeport CDP path now
#  imports directly to Turso: `./bin/chromeport parse capture <id> --source <id>`. See Scraping.)
./bin/travel db status                           # show DB state
./bin/travel db migrate                          # create/upgrade tables (idempotent)
./bin/travel db seed plans                       # one-time plan seed
./bin/travel db exec "<sql>"                     # one-shot raw SQL (migrations/backfills only)
```

## Bookings
```bash
./bin/travel sync-bookings [--dry-run]
./bin/travel query-bookings --dest tokyo_2026 [--category activity --status pending]
./bin/travel check-booking-integrity
./bin/travel validate-itinerary --dest tokyo_2026  # historical days skip booking-deadline failures
```

## Utilities
```bash
make test                                        # full Rust test suite (or: cd rust && cargo test -p travel-cli)
./bin/travel leave calc 2026-02-24 2026-02-28
./bin/travel normalize flights scrapes/trip-feb24-out.json --top 5
./bin/travel validate data                       # data integrity check
./bin/travel doctor                              # full system health check
```

## Mutations
```bash
./bin/travel set-dates 2026-02-13 2026-02-17
./bin/travel select-offer <offer-id> <date>
./bin/travel set-activity-booking <day> <session> "<activity>" <status> [--ref "..."] [--book-by YYYY-MM-DD]
./bin/travel set-airport-transfer <arrival|departure> <planned|booked> --selected "title|route|duration|price|schedule"
./bin/travel set-activity-time <day> <session> "<activity>" [--start HH:MM] [--end HH:MM] [--fixed true]
./bin/travel set-activity-title <day> <session> "<activity>" "<new_title>" [--plan-id <id>]
./bin/travel set-tod-time-range <day> <session> --start HH:MM --end HH:MM    # (alias: set-session-time-range)
./bin/travel set-day-theme <day> [theme] [--zh "<zh_title>"] [--dest slug]
./bin/travel set-route-segment <day> <sort_order> <from> <to> <mode> [--duration <min>] [--notes "<text>"] [--start-time HH:MM]
./bin/travel set-route-segments-bulk <day> --json '[{"from":"A","to":"B","mode":"walking","duration":5},...]'
./bin/travel set-tod-zh <day> <session> [--zh "<focus_zh>"] [--transit-zh "<transit_notes_zh>"] [--activities-zh-json '[...]'] [--meals-zh-json '[...]'] [--plan-id <id>]    # (alias: set-session-zh)
./bin/travel set-tod-focus <day> <session> "<focus_text>" [--plan-id <id>]    # (alias: set-session-focus)
./bin/travel delete-activity <day> <session> "<activity_id_or_title>" [--plan-id <id>]    # (alias: remove-activity)
./bin/travel swap-days <dayA> <dayB> [--dest slug]
./bin/travel fetch-weather [--dest slug] [--all]
```

## Operation tracking
```bash
./bin/travel run-status [run-id]
./bin/travel run-list [--status completed|failed|started] [--limit N]
```

## Shaping Stage — Triangle research (pre-plan; unscoped)
Explore departure date × destination × flight price together before any plan exists. These commands run before any plan is created, so they don't take `--plan-id` and don't resolve a plan from the DB.

```bash
./bin/travel shaping-init --origin TPE --start 2026-06-18 --end 2026-06-20 \
  --dest KIX:"Osaka (KIX)" --dest NRT:"Tokyo (NRT)" --nights 6 --nights 7 [--pax 2] [--rate 32] \
  [--shaping ASPECT:ROLE:KIND:VALUE[:NOTES] ...]   # e.g. date:hard_constraint:return_no_later_than:2026-06-27
# After shaping-init: scrape via chromeport, then import + compare:
#   ./bin/chromeport fetch interact "<url>" --source <id> --step ...
#   → ./bin/chromeport parse capture <capture-id> --source <id>
#   → ./bin/travel shaping-import --run <run_id> --file <handoff.json>
./bin/travel shaping-compare --run <run_id> [--json] [--limit N]
./bin/travel shaping-adopt <candidate_id> <plan_id> --create-plan --dest <slug>   # seed new plan with P1/P2
./bin/travel shaping-adopt <candidate_id> <plan_id>   # link to an existing plan only
# Internal (aggregator handoff — usually not run by hand):
./bin/travel shaping-export --run <run_id> --json
./bin/travel shaping-import --run <run_id> --file <path>
```
