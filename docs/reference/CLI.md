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
./bin/travel query-recommendations [--day N] [--session s] [--kind activity|meal|route] [--dest slug]    # READ-ONLY: list AI-recommended (source='ai_recommended') meals/routes/activities awaiting confirmation, grouped by kind with a "Day N session" scope hint. Same filters as confirm-recommendations (this previews what it would confirm). No --json.
./bin/travel status --travel-date 2026-06-20
./bin/travel itinerary --travel-start 2026-06-18 --travel-end 2026-06-25
./bin/travel view-prices --flights scrapes/date-range-prices.json --hotel-per-night 3000 --nights 4 --package 40740
```

## Comparison
```bash
./bin/travel compare-offers --region osaka
./bin/travel compare trips --input <your-comparison-file.json> [--detailed]   # input file is BYO
./bin/travel compare dates --start 2026-02-24 --end 2026-02-28 --nights 4
./bin/travel compare true-cost --region kansai --pax 2 --date 2026-02-24
./bin/travel compare content-depth --plan-id <drill> [--against okinawa-2026]   # read-only depth oracle: 3 depth axes (activities / real meals / routes w/ metadata) compared vs reference + a ZH slot-completeness GATE (aligned with validate publish's ZH gate — eligible day/session = has activities/meals/routes/transit; must all be translated). BETTER = gate PASS + all 3 depth axes >= ref + >=1 strictly >; gate FAIL → SHORT: …, ZH-gate
```

## Scraping

The Python scrapers (`scraper:*`, `scrape_date_range.py`, `scrape_google_flights.py`, …) are
**DECOMMISSIONED and archived** under `archive/broken-python-scrapers/` — their URL/region/template
construction 404s or lands on the wrong page. Do not run them.

Replacement: **gwebcdb** on WSLg (`~/b/gwebcdb`) drives the live OTA page in a real Chrome
(`127.0.0.1:9222`) and captures plain text → Turso `captures`. Extraction is **agent-first**: the
coding agent reads `captures.raw_text`, emits TSV, then persists normalized `offers` via
`travel ota write-offers` (there is no in-CLI parser — the `parser_rules`/regex path is retired).

```bash
# from ~/b/gwebcdb (export TURSO_URL/TURSO_TOKEN from this repo's .env first)
./scripts/start-chrome-cdp-wslg.sh                                # CDP on :9222
python bridge/navigate.py "<url>"                                 # + form_fill/combo_select for SPAs; settle ~25s
python bridge/ota_capture.py --source <id> [--url-contains <s>]   # → capture_id; UNREDACTED → captures
# AGENT reads captures.raw_text, extracts offers, emits TSV, then:
./bin/travel ota show-capture <capture_id>                        # read-only: raw_text → stdout; source_id/url/captured_at → stderr
./bin/travel ota write-offers <job_id> --capture <capture_id> --claim-token <tok> --tsv <path> --dest <slug>   # → Turso offers + provenance
```

See `src/skills/scrape-ota/SKILL.md` and `docs/plans/2026-06-24-ota-migration-chromeport.md`.

## Turso DB
```bash
./bin/travel import-offers --dir scrapes --dest tokyo_2026 [--start 2026-02-13 --end 2026-02-17] [--dry-run]
# Bridge OTA-scraped offers (global `offers` table) → plan-scoped `plan_offers` so select-offer
# can consume them. Reads latest snapshot per id; maps hotel + a synthesized date_pricing row;
# writes plan_offer_flights ONLY when flight_outbound/return are non-NULL (no id-string parsing);
# skips offers with NULL price/date; flips P3_4→researched; audit triad. (--dest is the
# global offers.destination; --plan-id owns the resulting plan_offers rows.)
./bin/travel promote-offers --from-offers --dest tokyo_sep_2026 --plan-id tokyo-sep-2026 \
  [--source eztravel] [--start 2026-09-01 --end 2026-09-30] [--pax 2] [--dry-run]
./bin/travel query-offers --plan-id tokyo-2026 --dest tokyo_2026 [--max-price 30000] [--capture-id <id>] [--job-id <id>] [--attempt-id <id>]
./bin/travel query-offers --region kansai --start 2026-02-24 --end 2026-02-28 [--max-price 30000]
./bin/travel db query-offers --destination osaka_2026 [--capture-id <id>] [--job-id <id>] [--attempt-id <id>] [--sql]
./bin/travel check-freshness --source besttour --plan-id tokyo-2026 --dest tokyo_2026
./bin/travel check-freshness --source besttour --region kansai
# (the old `db:import:turso` raw-offers loader is retired — the gwebcdb capture + agent
#  `ota write-offers` path writes offers directly to Turso. See Scraping.)
./bin/travel db status                           # show DB state
./bin/travel db token-status                      # diagnose Turso creds: which TRAVEL_TURSO_* env vars are set (values never printed) + live read/write probe; prints the export fix + exits 1 on failure
./bin/travel db migrate                          # create/upgrade tables (idempotent)
./bin/travel db seed plans                       # one-time plan seed
./bin/travel db exec "<sql>"                     # one-shot raw SQL (migrations/backfills only)
./bin/travel db schema [<table>]                 # list tables, or a table's columns (name/type/notnull/pk) — discover col names before db exec
```

## Bookings
```bash
./bin/travel sync-bookings [--dry-run]
./bin/travel query-bookings --dest tokyo_2026 [--category activity --status pending]
./bin/travel check-booking-integrity
./bin/travel validate-itinerary --dest tokyo_2026 [--severity error|warning|info]  # also a MAP-LINK lint: cross-country legs (error), &-truncating URLs / walking-mode rail (warning), ambiguous bare stops + meals with no map pin (info). Plus a RESERVATION lint (info): sit-down lunch/dinner restaurants not yet tracked in the booking ledger — walk-ins (ramen, 公設市場/食堂, 屋台村, supermarket, casual そば) are never flagged; self-clears once the session has a booked/pending activity. historical days skip booking-deadline failures
```

**Restaurant reservations reuse the activity booking lifecycle** (no separate meal-booking subsystem): a sit-down restaurant that needs booking is enrolled as an `activity` with `add-activity`, then `set-activity-booking <day> <session> "<restaurant>" pending` (→ `booked --ref` once confirmed). It then flows into `sync-bookings` → `bookings_current` → `query-bookings`/`status`/`check-booking-integrity`, renders a 待訂/已訂 badge on the dashboard, and disappears from the reservation lint. Walk-in venues are simply left as `session_meals` lines (not enrolled). The booking extract includes the `noon` session, so bookable lunches are tracked too.

## Utilities
```bash
make test                                        # full Rust test suite (or: cd rust && cargo test -p travel-cli)
./bin/travel leave calc 2026-02-24 2026-02-28
./bin/travel normalize flights scrapes/trip-feb24-out.json --top 5
./bin/travel validate data                       # data integrity check
./bin/travel validate publish --plan-id <id> [--dest slug]    # publish-readiness gate (Stage 4). BLOCKS on: no P5 days, itinerary errors, missing ZH (theme_zh/focus_zh on non-empty sessions), no map path. WARNS on: stale/never-snapshotted maps, missing weather within the 16-day window (upcoming trips). INFO (never blocks): count of AI-recommended items awaiting confirmation; weather-pending for a trip beyond the 16-day forecast window. Exit 1 only when blockers>0. Past-trip + empty-session guards.
./bin/travel doctor                              # full system health check (also runs the map-link lint across all plans; cross-country/ocean-route legs fail as errors). Emits a per-plan [reservations] info line for sit-down restaurants not yet tracked in the booking ledger (advisory — never fails the check)
```

## Dashboard / Maps
The trip dashboard is a Cloudflare Worker (`workers/trip-dashboard-rs/`, **Rust**) serving Turso directly — live at **`https://trip-dashboard-rs.yanggf.workers.dev`**. Owner access is GitHub OAuth-gated; `?token=` is only for per-plan viewer share links. See CLAUDE.md "Trip Dashboard" for deploy.
```bash
# Grant/share tokens (gate per-plan dashboard viewing). Plan uses normal resolver:
# --plan-id > $TRAVEL_PLAN_ID > date/destination fallbacks.
./bin/travel share-token                          # mint a NEW per-plan view token + print its ready-to-open dashboard URL
./bin/travel share-token --show                   # list token fingerprints + active/inactive status (read-only)
./bin/travel share-token --show-full              # print full bearer URLs (sensitive; use only when you need to re-copy one)
./bin/travel share-token deactivate <token>       # mark one active token inactive
#   URL host defaults to trip-dashboard-rs.yanggf.workers.dev; override with TRAVEL_DASHBOARD_HOST (e.g. after the primary-URL cutover)
#   After minting: logged-in owner can also copy the viewer URL from the dashboard/plan page UI (Copy share link button).
#   Recipients open the copied link logged-out — no GitHub login. Plan: docs/plans/2026-06-25-dashboard-share-link-copy.md

# Route maps (per-day + plan PNGs: numbered markers + route polyline, auto-framed; chromeport→Leaflet→R2).
./bin/travel snapshot-maps [--dest <slug>]        # (re)capture + upload the route-map PNGs (wraps scripts/snapshot-maps.sh). Needs Chrome at the chromeport CDP endpoint + wrangler auth.
./bin/travel mark-maps-snapshotted <plan_id>      # stamp the freshness timestamp (snapshot-maps does this automatically on success)
./bin/travel set-poi-coords <slug> <poi_id> <lat> <lon> [--source <s>] [--confidence <c>]    # geocode a destination_pois row (feeds the POI-coord map path). GLOBAL/slug-keyed reference data — takes NO --plan-id, NO audit triad. `validate data` WARNs on ungeocoded POIs.
./bin/travel add-transit <slug> <from_station> <to_station> --minutes N [--line "<t>"] [--kind metro|rail|walk|bus|estimate] [--source <s>] [--confidence verified|reviewed|estimate]    # add a destination_transit station pair (transit time/line that derive-routes attaches to auto-derived legs). GLOBAL/slug-keyed reference data — NO --plan-id, NO audit triad. Idempotent (INSERT OR REPLACE). pair_key uses derive-routes' own normalization, so the pair is found by the next `derive-routes` run — no more raw `db exec INSERT` for discovered pairs.
./bin/travel add-omiyage <slug> <item_id> --buy-at <poi_id> --location-source-url <url> --location-confidence verified|reviewed [--name <t>] [--category <t>] [--item-source-url <url>] [--item-confidence verified|reviewed] [--notes <t>] [--purchase-note <t>]    # add/update an omiyage (souvenir) item + purchase location. GLOBAL/slug-keyed reference data — NO --plan-id, NO audit triad. First write for an item_id requires the full item bundle (name/category/item-source-url/item-confidence); subsequent sellers omit the bundle (or must MATCH if supplied). location is always upserted. confidence ∈ {verified, reviewed} only; source URLs must start with http(s)://.
./bin/travel query-omiyage --slug <slug>    # read-only plain-text view of sourced omiyage for a destination, grouped by category then item. Prints item provenance once + per-seller POI (title/area/station/address/hours) + location provenance. Nullable fields render as `—`. Fail-loud: unknown dest vs empty (no rows) vs corrupt orphan location (poi_id with no destination_pois row). GLOBAL/slug-keyed — NO --plan-id.
./bin/travel omiyage-worklist --slug <slug>    # READ-ONLY omiyage research worklist — lists the destination's omiyage-tagged POIs, prints their notes VERBATIM as UNVERIFIED hints (not facts), an already-sourced count per POI, a VERIFY-BEFORE-ADDING checklist, and a filled add-omiyage template (slug/poi filled; item/URL/confidence left as placeholders). WRITES NOTHING — the agent gwebcdb-verifies each candidate's item page + seller floor-guide, then persists via add-omiyage; unverifiable candidates are left out (honest gap). Fail-loud: unknown dest / no omiyage-tagged POI. GLOBAL/slug-keyed — NO --plan-id. This is a destination CATALOG, not a purchase schedule (no plan/day/timing); WHEN/WHERE to buy is a per-plan itinerary-activity decision (food/short-shelf-life → departure-day route or airport seller POI). Stage 3 (`/stage3-expand-itinerary`) runs this automatically, right after the skeleton (before assigning activities).
./bin/travel check-maps-fresh [--plan-id <id>]    # lint: flag map PNGs that are stale vs the latest itinerary edit (advisory; never fails)
```
Only activities linked to a POI with lat/lon appear on the maps; non-place lines (flights, airport steps, bare meals) are excluded from both the maps and the per-stop Google-Maps links.

## Mutations
```bash
# Plan lifecycle
./bin/travel create-plan <plan_id> --dest <slug> --start <d> --end <d> --airport <IATA> [--region <name>] [--display-name <name>] [--origin <code>] [--nights N]    # create a fast-path plan (plans + metadata + date_anchors + the 6-process ladder) so set-flight/set-hotel/itinerary work. Dest must be registered (/new-destination). Dates-inclusive — no separate set-dates. plan_id is POSITIONAL (no --plan-id).
./bin/travel set-plan-name <name> [--dest <slug>] [--plan-id <id> | --travel-date ...]    # rename a plan's display label (plan_destinations.display_name); --dest disambiguates a multi-destination plan. Audited, no plan_events.
./bin/travel set-active-destination <slug> [--plan-id <id> | --travel-date ...]    # switch plan_metadata.active_destination to one of the plan's destinations (fail-loud if the slug isn't a destination of the plan). Audited.
./bin/travel mark-plan-deleted <plan_id> [--force]    # soft-delete a plan (sets deleted_at; data retained; `db cleanup-deleted` wipes). plan_id POSITIONAL.
./bin/travel set-dates 2026-02-13 2026-02-17
./bin/travel select-offer <offer-id> <date>
./bin/travel set-process-status <process_id> <target_status> [--dest slug] [--plan-id <id>]    # advance the process ladder to a status via the SHORTEST LEGAL path (BFS over the state machine); walks hop-by-hop (e.g. pending→populated→booking→booked) emitting one status_changed event per hop; idempotent no-op if already there. process_id: the 6 ids or aliases (p1/date, p2/destination, p34/packages, p3/transport/flight, p4/hotel, p5/itinerary). status: pending|researching|researched|selecting|selected|populated|booking|booked|confirmed|skipped. Used by the ingest-known path (set-flight/set-hotel are no-cascade); select-offer auto-advances P3/P4 so it needs no manual move.
./bin/travel set-activity-booking <day> <session> "<activity>" <status> [--ref "..."] [--book-by YYYY-MM-DD]
./bin/travel set-airport-transfer <arrival|departure> <planned|booked> --selected "title|route|duration|price|schedule"
./bin/travel set-activity-time <day> <session> "<activity>" [--start HH:MM] [--end HH:MM] [--fixed true]
./bin/travel set-activity-title <day> <session> "<activity>" "<new_title>" [--plan-id <id>]
./bin/travel set-activity-poi <day> <session> <poi_id> [--match "<title substring>"] [--dest slug]    # link one activity to a destination_pois row; durable map/ticket POI FK; writes audit triad.
./bin/travel set-activity-poi --auto [--dest slug]    # batch-link NULL-poi activities to exactly-one geocoded POI by exact/title-substring match after stripping trailing CJK/kana/fullwidth gloss; never guesses; one operation_runs row for the batch; unambiguous misses stay manual.
./bin/travel set-tod-time-range <day> <session> --start HH:MM --end HH:MM    # (alias: set-session-time-range)
./bin/travel set-day-theme <day> [theme] [--zh "<zh_title>"] [--dest slug]
./bin/travel derive-routes [--day N] [--dest slug] [--plan-id <id>]    # CASCADE: derive ai_recommended transit legs between consecutive same-day activities (from POI nearest_station + destination_transit metadata). Idempotent; skips days with a confirmed route; re-run --day N after activity edits. Run once after populate-itinerary. Legs flow into the 🤖 badge / validate-publish INFO / query-confirm lifecycle. Ends with a `⚠ ... missing destination_transit metadata` worklist (a ready-to-run `add-transit` line per pair) when a derived leg has no transit time — fill via add-transit, then re-derive.
./bin/travel set-route-segment <day> <sort_order> <from> <to> <mode> [--duration <min>] [--notes "<text>"] [--start-time HH:MM] [--recommended]
./bin/travel set-route-segments-bulk <day> --seg "from|to|mode[|duration[|start_time[|notes]]]" [--seg ...] [--recommended]    # plain-text; repeat --seg per segment. NOTE: single command is POSITIONAL; bulk uses --seg. Both reject unknown flags (a typo'd --recommended fails loud, never writes 'confirmed' silently).
#   <mode> canonical: transit | walking | driving. Aliases are normalized: walk→walking; monorail/rail/train/bus/subway/metro/tram/ferry→transit; taxi/car/cab→driving (plus 步行/單軌/巴士/計程車…).
./bin/travel set-tod-zh <day> <session> [--zh "<focus_zh>"] [--transit-zh "<transit_notes_zh>"] [--activity-zh "<zh>" (repeatable)] [--clear-activities] [--plan-id <id>]    # (alias: set-session-zh); --clear-activities empties the ZH activity list (mutually exclusive with --activity-zh)
./bin/travel set-tod-focus <day> <session> "<focus_text>" [--zh "<focus_zh>"] [--plan-id <id>]    # (alias: set-session-focus); --zh sets focus_zh too (dashboard renders ZH by default)
./bin/travel set-meals <day> <session> --meal "<text>" [--meal "<text>"...] [--recommended] [--dest slug]    # replace session meals; a meal may carry a place pin: "<label>｜map:<query>"
./bin/travel add-activity <day> <session> "<title>" [--after <id|title>] [--recommended] [--area ..] [--station ..] [--duration MIN] [--start HH:MM] [--end HH:MM] [--fixed true|false] [--priority must|want|optional] [--notes ..] [--dest slug]    # add an activity (append, or --after to insert at a position); audit triad. Leaves poi_id NULL; if the title unambiguously matches a GEOCODED destination_pois row it prints a 💡 hint to link it (`set-activity-poi <day> <session> <poi_id>`) so the day's map pin renders — or run `set-activity-poi --auto` to link all matches at once.
#   --recommended (on set-meals / add-activity / set-route-segment(s-bulk)) marks the item AI-recommended (source='ai_recommended') vs user-confirmed. The dashboard badges it 🤖; `validate publish` counts it as INFO; the user flips accepted items with confirm-recommendations. Enriching a previously-empty session with a meal/route/activity makes it non-empty → it now REQUIRES focus_zh (set-tod-zh) or `validate publish` will BLOCK; set the ZH focus in the same pass.
./bin/travel confirm-recommendations [--day N] [--session morning|noon|afternoon|evening] [--kind activity|meal|route] [--dest slug]    # flip source='ai_recommended' → 'confirmed', scoped by the filters. Zero-scope is a clean no-op (no audit/version bump). --kind route + --session is rejected (routes have no session); with --kind absent, --session scopes activities/meals only while routes confirm by day. PREVIEW first with `query-recommendations` (same filters, read-only) to see exactly what will flip.
./bin/travel move-activity <day> <from-session> <to-session> <id|title> [--to-day N] [--dest slug]    # move an activity to another session/day, PRESERVING its id + poi link (vs delete+re-add)
./bin/travel reorder-activities <day> <session> <id-or-title> <id-or-title> ... [--dest slug]    # rewrite sort_order; list ALL activities in the session in the desired order
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
# After shaping-init: capture via gwebcdb (WSLg), agent-extract, then import + compare:
#   cd ~/b/gwebcdb && ./scripts/start-chrome-cdp-wslg.sh && python bridge/navigate.py "<url>"
#   → python bridge/ota_capture.py --source <id>   # → capture_id
#   → AGENT reads captures.raw_text, emits TSV → ./bin/travel ota write-offers <job_id> --capture <capture_id> --claim-token <tok> --tsv <path>
#   → ./bin/travel shaping-import --run <run_id> --file <handoff.json>
./bin/travel shaping-compare --run <run_id> [--limit N]
./bin/travel shaping-adopt <candidate_id> <plan_id> --create-plan --dest <slug>   # seed new plan with P1/P2
./bin/travel shaping-adopt <candidate_id> <plan_id>   # link to an existing plan only
# Internal (aggregator handoff — usually not run by hand):
./bin/travel shaping-export --run <run_id> [--file <path>]    # writes the machine handoff JSON to a FILE (default <run_id>-shaping.json) for shaping-import; the terminal shows a plain-text confirmation, not JSON
./bin/travel shaping-import --run <run_id> --file <path>
```
