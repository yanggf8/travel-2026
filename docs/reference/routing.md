# Routing & Agent Workflow

> Moved from CLAUDE.md 2026-09-28 (CLAUDE.md now holds rules only). This file holds
> the request-routing guidance: the Skill Decision Tree, the default planning path,
> and OTA URL routing. The hard rules those rely on live in CLAUDE.md; commands live
> in `docs/reference/CLI.md`.

## Agent-First Workflow

- Proactively run next logical step; only ask user when a preference materially changes the result
- Prefer `./bin/travel` subcommands over direct SQL for reusable content edits (raw `db exec` is fine for one-shot migrations/backfills — see "DB Operation Decision" in CLAUDE.md)
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
"agency fell through" / "取消選定報價"  → ./bin/travel clear-offer   (undo select-offer; then re-stamp provenance: set-flight <dir> --source / set-hotel --source)
"booked elsewhere" / "已訂XX"          → set-flight/set-hotel ... --source booking:<agency>_<order>  + set-process-status p3|p4 booked
"which FIT offers to compare" / "FIT 比較放哪幾筆" → ./bin/travel set-fit-offer <offer-id> | --remove | --list   (+ set-fit-note for the reasons / criteria paragraph)
"swap two days" / "對調兩天"           → ./bin/travel swap-days A B  (moves routes + landmarks too) → snapshot-maps
"domestic stay" / "國內住宿" / "海景房候選"  → ./bin/travel list-accommodations --dest <slug>  (candidates + decision facts); add/update/delete-accommodation to edit; add-accommodation-image for the gallery; set-accommodation-rating per review source. All slug-keyed, NO --plan-id.
"book the domestic stay" / "訂這間"   → ./bin/travel set-accommodation ... (P4 -> booked, audited) | clear-accommodation to cancel
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
User provides OTA URL                → /scrape-ota (see URL Routing below)
User provides booking confirmation   → ./bin/travel set-activity-booking
"deploy dashboard" / "publish trip"  → /stage4-publish-dashboard
```

### URL Routing
**Do not use WebFetch for OTA sites** (they require JavaScript). **The Python scrapers are
DECOMMISSIONED and archived** (`archive/broken-python-scrapers/`) — their constructed
URLs 404 / hit the wrong page.

> **OTA scraping = gwebcdb on WSLg (current, verified 2026-06-25) — read first.** The browser
> layer is **gwebcdb** (`~/b/gwebcdb`), the shared WSLg-based CDP toolset, AND it now owns the OTA
> extraction too (now Rust `gwebcdb-*` CLIs; the Python bridge was archived 2026-07-24 and DELETED 2026-09-25 — never run `python bridge/*.py`). **`chromeport` is RETIRED** — its OTA
> `parse` / `verify` / `parser rules` subcommands are **removed** (they now fail loud, exit 1, and
> point to gwebcdb; the dead parser code was deleted 2026-06-29). chromeport only still provides
> `browser` / `screenshot` / `db` for `snapshot-maps`. It was the fragile Windows-Chrome path that
> WSLg replaces, not a fallback to keep working. WSLg-native Chrome is the **standing verified
> backend** (live on this host: `running_backend=wslg`, CDP up on :9222, bridge attached). Drive
> everything with gwebcdb's Rust CLIs (`armo` lists them). Full recipe + gotchas: **gwebcdb `CLAUDE.md` →
> "OTA scraping — end-to-end usage"**; per-source gate: `docs/plans/2026-06-24-ota-migration-chromeport.md`.
> For sign-in OTAs the human logs in / settles 2FA in the WSLg Chrome window (session persists in
> the `~/.local/share/gwebcdb/codex-browser` profile) or via gwebcdb's approval-gated `login_assist`.

OTA capture flow (run from `~/b/gwebcdb`; export `TURSO_URL`/`TURSO_TOKEN` from this repo's `.env`
first — gwebcdb has no `.env` loader):

| URL Contains | Action |
|-------------|--------|
| Any OTA (besttour / liontravel / lifetour / settour / …) | Start Chrome, drive the page, capture, then **the agent reads the capture text and writes offers** (no in-CLI parser): <br>`gwebcdb-chrome start` (idempotent; CDP on :9222) <br>→ `gwebcdb-bridge navigate "<url>"` (+ `gwebcdb-bridge form-fill`/`combo-select`/`form-click` for SPA searches; let async price/hotel SPAs settle ~25s) <br>→ `gwebcdb-ota capture --source <id> [--url-contains <s>]` (UNREDACTED text → `captures`; prints `capture_id`) <br>→ **agent reads `captures.raw_text`, extracts the offers, emits TSV** <br>→ `./bin/travel ota write-offers <job_id> --capture <capture_id> --claim-token <tok> --tsv <path> --dest <slug>` (under a claimed `ota_jobs` job; writes `offers` + provenance + attempt audit) |
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
