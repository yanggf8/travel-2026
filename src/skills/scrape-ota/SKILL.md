---
name: scrape-ota
description: Capture OTA pages via gwebcdb on WSLg, then the agent extracts offers and writes them with `travel ota write-offers`. (Python scrapers + chromeport + the in-CLI regex parser are all retired.)
version: 4.0.0
requires_skills: [travel-shared]
requires_processes: []
provides_processes: []
---

# /scrape-ota

> **⚠️ DO NOT run `python scripts/scrape_*.py` (archived) and DO NOT run `./bin/chromeport` (retired).**
> The OTA pipeline lives in **gwebcdb on WSLg** (`~/b/gwebcdb`). Extraction is **agent-first**: the
> coding agent reads the captured page text and writes the offers — there is no in-CLI parser.
> The former regex/`parser_rules`/custom-`parse_settour` path is **retired** (`travel ota parse`
> fail-louds → use `write-offers`).

## The model (read this first)

```
gwebcdb (WSLg Chrome)          travel CLI (agent-first)         YOU (the agent)
─────────────────────          ────────────────────────        ─────────────────
navigate → ota_capture   ──▶   captures.raw_text         ──▶   read it, extract offers
                                                                emit TSV
                          ◀──   travel ota write-offers   ◀──   (TSV on --tsv)
                               → offers + provenance + audit
```

**The CLI does not parse.** Its job is (a) fetch the capture (via gwebcdb) and (b) persist the
offers you hand back as TSV (`write-offers`: normalized `offers` rows + `agent_parse` provenance +
a token-guarded `ota_jobs`/`ota_attempts` audit trail). You are the parser — there is no page text
you can't read once the capture returns.

## When to use

- User provides an OTA URL (besttour, liontravel, lifetour, settour, eztravel, …)
- `/p3p4-packages` or `/p3-flights` needs OTA data
- WebFetch fails due to JavaScript rendering

## End-to-end flow (the verified path)

Run gwebcdb steps from `~/b/gwebcdb`; export Turso creds first (`turso_db.py` has no `.env` loader):

```bash
export TURSO_URL=$(grep '^TURSO_URL=' ~/b/travel-2026/.env | cut -d= -f2-)
export TURSO_TOKEN=$(grep '^TURSO_TOKEN=' ~/b/travel-2026/.env | cut -d= -f2-)
```

1. **Queue + claim a job** (travel CLI — `ota` is in the debug binary until the next `make build`):
   ```bash
   ./rust/target/debug/travel ota enqueue <source_id> <product_type> [--depart … --return … --nights …]
   ./rust/target/debug/travel ota claim --worker <name> --lease-seconds 900   # → job_id + claim_token
   ```
2. **Drive the page + capture** (gwebcdb on WSLg):
   ```bash
   ./scripts/start-chrome-cdp-wslg.sh                 # idempotent; CDP on :9222 (WSLg-native Chrome)
   python bridge/navigate.py "<url>"                  # + form_fill/combo_select/form_click for SPA searches
   # For async price/hotel SPAs, let the page settle (~25s) before capturing — a too-early capture
   # shows placeholders like 正在努力查詢最優惠的價格.. and an amount of `--`.
   python bridge/ota_capture.py --source <source_id> [--url-contains <substr>]   # → capture_id (UNREDACTED → captures)
   ```
3. **You extract** — read `captures.raw_text` and pull the offer fields:
   ```bash
   ./bin/travel db exec "SELECT raw_text FROM captures WHERE capture_id='<capture_id>'"
   ```
   Read the real, decision-relevant numbers off the page (e.g. settour shows the per-person price as
   `每人機加酒含稅$NN,NNN` — use THAT, not an un-taxed total ÷ pax). Pull the real hotel name, the
   flight codes, dates, nights.
4. **Write the offers** (travel CLI persists your TSV):
   ```bash
   printf 'type\tprice_per_person\tdeparture_date\treturn_date\tnights\tairline\tflight_outbound\tflight_return\thotel_name\tcurrency\n%s\n' \
     "package\t16937\t2026-09-04\t2026-09-08\t4\t台灣虎航\tIT202\tIT201\tAPA飯店〈東日本橋站前〉\tTWD" \
     > /tmp/offer.tsv
   ./rust/target/debug/travel ota write-offers <job_id> --capture <capture_id> --claim-token <token> --tsv /tmp/offer.tsv
   ```

### TSV columns

Header (first line) names the columns; each later line is one offer. Known columns:
`type`, `price_per_person`, `departure_date`, `return_date`, `nights`, `airline`,
`flight_outbound`, `flight_return`, `hotel_name`, `currency`.

- **`type` is the OFFER KIND** — `package | flight | hotel` — NOT the job's `product_type`. A
  flight+hotel combo (settour `fit`, eztravel `fit`) is **`package`**. `write-offers` rejects
  anything else.
- `price_per_person` required, > 0, no thousands-confusion (commas are stripped).
- dates are `YYYY-MM-DD` (or `YYYY/MM/DD`); the row's field count must match the header.
- Multiple offers (e.g. a flight list) → one TSV line each; `write-offers` disambiguates the
  offer ids in-batch so distinct offers don't collapse.

## Agent-first, escalate-on-block

Drive the loop yourself. Only WARN the human on a real blocker: Chrome not on :9222, a login wall,
or a captcha (for sign-in OTAs the human logs in / settles 2FA in the WSLg Chrome window, or use
gwebcdb's approval-gated `login_assist`). Never default to "human-in-the-loop".

## Sources

Provider coverage is DB data: `./bin/travel ota-status` (and `ota_sources` / `ota_source_coverage`
in Turso). settour is live-verified end-to-end via this agent-parse path (2026-06-30). The other
sources each still need their own live WSLg capture + `write-offers` before their archived parsers
can be deleted.

## Reference

- gwebcdb `CLAUDE.md` → "OTA scraping — end-to-end usage" (form-driving recipe, per-agent Chrome
  sessions, backends)
- travel `CLAUDE.md` → "URL Routing" (the canonical table) + the OTA agenda bullet
- `../travel-shared/references/ota-registry.md` — source IDs, region codes
