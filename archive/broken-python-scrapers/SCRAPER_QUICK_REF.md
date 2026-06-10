# chromeport Quick Reference

`chromeport` (`rust/crates/chromeport`) is the live OTA capture tool — a Rust CDP driver
(`chromiumoxide`) that **attaches to a real Windows Chrome at `127.0.0.1:9222`**, drives
the actual page (navigate / click / fill), writes plain-text captures to the Turso
`captures` table, then rule-parses them (`parser_rules` table) into the Turso `offers`
table. There are no JSON files in this pipeline and no Python.

> The old Python scrapers (`scripts/scrape_package.py`, the `scrapers/` package, etc.) are
> **DECOMMISSIONED and archived** under `archive/broken-python-scrapers/` — their
> URL/region templates 404 or land on the wrong page. Do not run them. `chromeport` is the
> replacement.

Build: `cd rust && cargo build -p chromeport` → binary at `./rust/target/debug/chromeport`.

## OTA Sources (parser_rules)

| Source ID | Display Name | Type | Custom parser |
|-----------|--------------|------|---------------|
| besttour | 喜鴻假期 | Package | ✅ |
| liontravel | 雄獅旅遊 | Package/Flight/Hotel | ✅ |
| lifetour | 五福旅遊 | Package | ✅ |
| settour | 東南旅遊 | Package | ✅ |
| travel4u | 山富旅遊 | Package | ✅ |
| tigerair | 台灣虎航 | Flight | ✅ |
| google_flights | Google Flights | Flight | ✅ |
| agoda | Agoda | Hotel | ✅ |
| eztravel | 易遊網 | Flight | ✅ |
| trip | Trip.com | Flight | ⚠️ scrape-only (no custom parser yet) |
| booking | Booking.com | Hotel | ⚠️ scrape-only |

Authoritative OTA registry: Turso `ota_sources` table (see `CLAUDE.md` → OTA Sources).

## CLI Commands

```bash
# Pre-flight: confirm Chrome is reachable on the CDP port
chromeport browser doctor
chromeport browser pages                       # list open tabs

# Passive capture of a single URL (no interaction)
chromeport fetch url "<url>" --source <id> [--html]

# Drive the real page (navigate/click/fill), then capture
chromeport fetch interact "<url>" --source <id> \
  --step 'fill:#dep=2026-02-13' \
  --step 'click:.search-btn' \
  --step 'waitfor:.result-list' [--html] [--i-understand-profile]

# Capture an already-open tab instead of navigating
chromeport browser snapshot --page <N> --source <id> [--html]

# Read-only diagnostics: print rule regexes + per-field extraction status
chromeport verify <source-id> <capture-id>

# Parse a capture via parser_rules and import offers → Turso (--dry-run prints instead)
chromeport parse capture <capture-id> --source <id> [--dry-run]

# Seed / refresh the default parser_rules rows
chromeport parser rules seed-defaults
```

### Interaction steps (`--step`)

| Step | Meaning |
|------|---------|
| `fill:SEL=VALUE` | type VALUE into the element matching CSS selector SEL |
| `click:SEL` | click the element matching SEL |
| `wait:MS` | sleep MS milliseconds |
| `waitfor:SEL` | block until SEL appears |

### Endpoint / env

- Default CDP endpoint: `http://127.0.0.1:9222`
- Override per run: `--endpoint http://127.0.0.1:<port>`
- Override via env: `CHROMEPORT_CDP_ENDPOINT=http://127.0.0.1:9222`
- `--i-understand-profile` overrides the dedicated-automation-profile guard (only when you
  know the attached Chrome is the `C:\chrome-profiles\travel-browser` profile).

## End-to-end workflow

```bash
# 1. Confirm Chrome is up on the debug port
chromeport browser doctor

# 2. Drive the OTA page and capture it (writes a row to Turso `captures`)
chromeport fetch interact "https://vacation.liontravel.com/search?..." \
  --source liontravel --step 'waitfor:.product-card'

# 3. Inspect what the parser will extract before committing
chromeport verify liontravel <capture-id>

# 4. Dry-run the parse, then import for real
chromeport parse capture <capture-id> --source liontravel --dry-run
chromeport parse capture <capture-id> --source liontravel

# 5. Read the imported offers back from Turso (TS CLI)
npm run travel -- query-offers --plan-id <id> --dest <slug>
```

## DB access (chromeport)

```bash
chromeport db query "<sql>"                    # read
chromeport db exec "<sql>"                     # write
chromeport db token-status <read|write|secrets>  # check minted tier token
```

Credentials are resolved through minted tier tokens via `turso-util` (no static `.env`
token). If token resolution fails, run `turso auth login`.

## Storage model

- **Captures** → Turso `captures` table (plain text, no JSON files).
- **Offers** → Turso `offers` table, written by `parse capture`.
- **Parser rules** → Turso `parser_rules` table (one rule row per source). A missing rule
  row for a source makes `parse capture` fail with
  `missing parser_rules row for source_id='<id>'; run chromeport parser rules seed-defaults`.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| `browser doctor` can't connect | Chrome not started with `--remote-debugging-port=9222` | Relaunch Chrome with the debug port on the dedicated profile |
| "interactive fetch refused: … profile" | attached Chrome is not the dedicated automation profile | Relaunch with `--user-data-dir=C:\chrome-profiles\travel-browser`, or pass `--i-understand-profile` |
| `no capture with capture_id=…` | parse run before any capture | run a `fetch`/`snapshot` first |
| `missing parser_rules row …` | source has no rule | `chromeport parser rules seed-defaults` (or insert a rule row) |
| `verify` shows empty fields | page structure changed / wrong selectors | re-capture with `--html` and inspect; adjust the source's `parser_rules` regexes |
| Token resolution fails | minted tier token unavailable | `turso auth login`, then re-check with `chromeport db token-status read` |
```
