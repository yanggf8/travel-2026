# CLI Reference

Full reference for the `travel` CLI and supporting scripts. CLAUDE.md keeps a short list of the most-used commands; everything else lives here.

## Plan resolution
`--plan-id` and `$TRAVEL_PLAN_ID` win. Without those, the CLI uses `--travel-date`, `--travel-start/--travel-end`, or exactly one active or upcoming DB date anchor/planning window. Use `--travel-*` for plan selection; plain `--start/--end` remain command-specific filters (e.g. offer search ranges). If several plans match, the CLI fails with a plan list instead of silently loading a legacy default.

## Views

Each `view:*` script is a separate command — pick one:

```bash
npm run travel -- plans                          # list DB plans and date anchors
npm run view:status                              # booking overview
npm run view:itinerary                           # daily plan
npm run view:transport                           # transport summary
npm run view:bookings                            # booking ledger
npm run travel -- status --travel-date 2026-06-20
npm run travel -- itinerary --travel-start 2026-06-18 --travel-end 2026-06-25
npm run view:prices -- --flights scrapes/date-range-prices.json --hotel-per-night 3000 --nights 4 --package 40740
```

## Comparison
```bash
npm run travel -- compare-offers --region osaka [--json]
npm run compare-trips -- --input <your-comparison-file.json> [--detailed]   # input file is BYO
npm run compare-dates -- --start 2026-02-24 --end 2026-02-28 --nights 4
npm run compare-true-cost -- --region kansai --pax 2 --date 2026-02-24
```

## Scraping
```bash
npm run scraper:setup                            # Install Playwright browsers (first-time)
npm run scraper:batch -- --dest kansai [--sources besttour,settour] [--date 2026-02-24 --type fit]
npm run scraper:doctor                           # Test all scrapers
npm run scraper:pipeline                         # Doctor + batch + import (end-to-end)
python scripts/scrape_date_range.py --depart-start 2026-02-24 --depart-end 2026-02-27 \
  --origin tpe --dest kix --duration 5 --pax 2 -o scrapes/date-range-prices.json
python scripts/scrape_google_flights.py --origin TPE --dest KIX,FUK \
  --depart-start 2026-06-18 --depart-end 2026-06-22 --duration 4,5 \
  -o scrapes/google-flights-jun.json
```

## Turso DB
```bash
npm run travel -- import-offers --dir scrapes --dest tokyo_2026 [--start 2026-02-13 --end 2026-02-17] [--dry-run]
npm run travel -- query-offers --plan-id tokyo-2026 --dest tokyo_2026 [--max-price 30000] [--json]
npm run travel -- query-offers --region kansai --start 2026-02-24 --end 2026-02-28 [--max-price 30000] [--json]
npm run travel -- check-freshness --source besttour --plan-id tokyo-2026 --dest tokyo_2026
npm run travel -- check-freshness --source besttour --region kansai
npm run db:import:turso -- --dir scrapes [--start 2026-02-24 --end 2026-02-28]   # legacy: writes offers table
npm run db:status:turso                          # show DB state
npm run db:migrate:turso                         # create/upgrade tables (idempotent)
npm run db:seed:plans                            # one-time plan seed
```

## Bookings
```bash
npm run travel -- sync-bookings [--dry-run]
npm run travel -- query-bookings --dest tokyo_2026 [--category activity --status pending]
npm run travel -- check-booking-integrity
npm run travel -- validate-itinerary --dest tokyo_2026  # historical days skip booking-deadline failures
```

## Utilities
```bash
npm test
npm run leave-calc 2026-02-24 2026-02-28
npm run normalize-flights -- scrapes/trip-feb24-out.json --top 5
npm run validate:data                            # data integrity check
npm run doctor                                   # full system health check
```

## Mutations
```bash
npm run travel -- set-dates 2026-02-13 2026-02-17
npm run travel -- select-offer <offer-id> <date>
npm run travel -- set-activity-booking <day> <session> "<activity>" <status> [--ref "..."] [--book-by YYYY-MM-DD]
npm run travel -- set-airport-transfer <arrival|departure> <planned|booked> --selected "title|route|duration|price|schedule"
npm run travel -- set-activity-time <day> <session> "<activity>" [--start HH:MM] [--end HH:MM] [--fixed true]
npm run travel -- set-activity-title <day> <session> "<activity>" "<new_title>" [--plan-id <id>]
npm run travel -- set-tod-time-range <day> <session> --start HH:MM --end HH:MM    # (alias: set-session-time-range)
npm run travel -- set-day-theme <day> [theme] [--zh "<zh_title>"] [--dest slug]
npm run travel -- set-route-segment <day> <sort_order> <from> <to> <mode> [--duration <min>] [--notes "<text>"] [--start-time HH:MM]
npm run travel -- set-route-segments-bulk <day> --json '[{"from":"A","to":"B","mode":"walking","duration":5},...]'
npm run travel -- set-tod-zh <day> <session> [--zh "<focus_zh>"] [--transit-zh "<transit_notes_zh>"] [--activities-zh-json '[...]'] [--meals-zh-json '[...]'] [--plan-id <id>]    # (alias: set-session-zh)
npm run travel -- set-tod-focus <day> <session> "<focus_text>" [--plan-id <id>]    # (alias: set-session-focus)
npm run travel -- delete-activity <day> <session> "<activity_id_or_title>" [--plan-id <id>]    # (alias: remove-activity)
npm run travel -- swap-days <dayA> <dayB> [--dest slug]
npm run travel -- fetch-weather [--dest slug] [--all]
```

## Operation tracking
```bash
npm run travel -- run-status [run-id]
npm run travel -- run-list [--status completed|failed|started] [--limit N]
```

## Stage 0 — Triangle research (pre-plan; unscoped)
Explore departure date × destination × flight price together before any plan exists. These commands run before any plan is created, so they don't take `--plan-id` and don't resolve a plan from the DB.

```bash
npm run travel -- stage0-init --origin TPE --start 2026-06-18 --end 2026-06-20 \
  --dest KIX:"Osaka (KIX)" --dest NRT:"Tokyo (NRT)" --nights 6 --nights 7 [--pax 2] [--rate 32]
python scripts/stage0_research.py --run <run_id>          # aggregator (no Turso I/O of its own)
npm run travel -- stage0-compare --run <run_id> [--json] [--limit N]
npm run travel -- stage0-adopt <candidate_id> <plan_id> --create-plan --dest <slug>   # seed new plan with P1/P2
npm run travel -- stage0-adopt <candidate_id> <plan_id>   # link to an existing plan only
# Internal (aggregator handoff — usually not run by hand):
npm run travel -- stage0-export --run <run_id> --json
npm run travel -- stage0-import --run <run_id> --file <path>
```
