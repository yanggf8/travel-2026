---
name: scrape-ota
description: Scrape OTA pages via the Rust CDP driver against real Chrome. (Python scrapers decommissioned.)
version: 3.0.0
requires_skills: [travel-shared]
requires_processes: []
provides_processes: []
---

# /scrape-ota

> **⚠️ DECOMMISSIONED PYTHON — DO NOT RUN ANY `python scripts/scrape_*.py`.**
> All Python scrapers are archived under `archive/broken-python-scrapers/`: their
> constructed URLs 404 / land on the wrong page. **Use the Rust CDP driver instead:**
> ```
> # 1) drive the real OTA page in Chrome (clicks/fills the actual UI — no URL templates)
> ./rust/target/debug/chromeport fetch interact "<url>" --source <id> --step 'click:SEL' --step 'fill:SEL=VALUE'
> #    (or, if you've already navigated the tab manually:)
> ./rust/target/debug/chromeport browser snapshot --page <N> --source <id>
> # 2) parse the captured page (rule-driven via the parser_rules Turso table) + import to Turso
> ./rust/target/debug/chromeport parse capture <capture-id> --source <id>
> ```
> Captures → Turso `captures` table; offers → `offers` table. Per-OTA rules → `parser_rules`.
> Flight/hotel-only OTAs (tigerair, google_flights, trip, agoda, eztravel) are seeded
> `has_custom_parser=1` and currently fail loud — they need a flight/hotel rule shape first.
> Everything below describes this chromeport CDP flow (the Python pipeline is fully retired).

## Shared references

Read first unless request is a single known-URL scrape:
- `../travel-shared/references/ota-registry.md` — source IDs, region codes, rate limits
- OTA baggage rules / booking notes — from Turso tables `airlines`, `booking_types`, `platform_behaviors`, `comparison_rules` (query directly, e.g. `./bin/travel db exec "SELECT * FROM airlines"`)
- `references/adding-ota.md` — step-by-step guide for registering a new OTA parser (read only when adding new OTA support)

## Role in Process Flow

```
P1 Dates → P2 Destination → [/scrape-ota] → P3+P4 Packages → P5 Itinerary
                                   ↑
                            Data acquisition layer
                            Used by /p3p4-packages and /p3-flights
```

This skill is the **shared data acquisition layer** for all OTA interactions.
Other skills reference this skill instead of duplicating scraper commands.

## When to Use

- User provides an OTA URL (besttour, liontravel, lifetour, settour, etc.)
- `/p3p4-packages` or `/p3-flights` needs OTA data
- WebFetch fails due to JavaScript rendering
- Need structured tour data (flights, hotel, itinerary, pricing)
- Comparing packages across multiple OTAs

## Supported OTAs

Capture every OTA via the chromeport CDP driver (`--source <id>`); parsing is rule-driven
from the `parser_rules` Turso table (no per-OTA Python module).

| OTA | Display Name | URL Pattern | chromeport `--source` |
|-----|--------------|-------------|-----------------------|
| `besttour` | 喜鴻假期 | `besttour.com.tw/itinerary/*` | `besttour` |
| `liontravel` | 雄獅旅遊 | `liontravel.com/*`, `vacation.liontravel.com/*` | `liontravel` |
| `lifetour` | 五福旅遊 | `tour.lifetour.com.tw/detail*` | `lifetour` |
| `settour` | 東南旅遊 | `tour.settour.com.tw/product/*` | `settour` |
| `tigerair` | 台灣虎航 | `booking.tigerairtw.com/*` | `tigerair` |
| `trip` | Trip.com | `trip.com/flights/*` | `trip` |
| `google_flights` | Google Flights | `google.com/travel/flights*` | `google_flights` |
| `agoda` | Agoda | `agoda.com/*` | `agoda` |

### Unsupported OTAs

| OTA | Display Name | Status | Reason |
|-----|--------------|--------|--------|
| `skyscanner` | Skyscanner | ❌ Blocked | Hard captcha redirect on all requests. Stealth flags don't help. Last tested: 2026-02-06 |

## Module Architecture

```
rust/crates/chromeport/        # Rust CDP driver (attaches to real Chrome :9222)
  src/                         # fetch interact / browser snapshot / verify / parse capture
captures (Turso table)         # plain-text captures landed by the driver
parser_rules (Turso table)     # per-OTA parse rules (keyed by source_id) — drives `parse capture`
offers (Turso table)           # rule-parsed offers (the source-of-truth output)
```

### Key Design Principles

- **Drives the real UI**: the driver navigates/clicks/fills the actual OTA page in Chrome — no fragile URL templates
- **Capture/parse separated**: `fetch interact` / `browser snapshot` land a plain-text capture; `parse capture` rule-parses it (no browser needed)
- **Rule-driven parsing**: per-OTA rules live in the `parser_rules` Turso table, not in code
- **Turso source of truth**: captures → `captures`, offers → `offers`; no JSON files in the pipeline

## URL → source_id

`--source <id>` selects the OTA. Match the URL against the **Supported OTAs** table above to
pick the `source_id` (e.g. a `trip.com/flights/*` URL → `--source trip`), then pass it to both
`fetch interact` and `parse capture`. Parse rules for that source are looked up in `parser_rules`.

## Workflow

### 0. Pre-flight checks (REQUIRED)

Before any capture, confirm the driver is built and a real Chrome is listening on CDP:

```bash
# Build the chromeport CDP driver if needed
cd rust && cargo build -p chromeport

# A real Chrome must be reachable at 127.0.0.1:9222 (the driver attaches to it)
./rust/target/debug/chromeport browser tabs   # lists open tabs — fails loud if Chrome :9222 is down
```

**Why this matters:**
- No Chrome on :9222 → the driver fails loud (it attaches, it does not launch a hidden browser)
- OTA site changes → update the source's rows in the `parser_rules` Turso table, not code
- Captures are plain text landed in the `captures` table — inspect there if a parse looks wrong

### 1. Detect OTA and capture the page

Pick `--source <id>` from the Supported OTAs table, drive the real page, then parse the capture:

```bash
# Drive + capture (clicks/fills the actual UI — no URL templates)
./rust/target/debug/chromeport fetch interact "<url>" --source <id> --step 'click:SEL' --step 'fill:SEL=VALUE'

# Or, if you already navigated the tab manually, snapshot the open page:
./rust/target/debug/chromeport browser snapshot --page <N> --source <id>

# Read-only regex diagnostics on a capture (optional)
./rust/target/debug/chromeport verify <id> <capture-id>

# Parse the capture (rule-driven via parser_rules) → Turso offers
./rust/target/debug/chromeport parse capture <capture-id> --source <id>
```

Examples (same flow, different `--source`): `--source liontravel` for Lion Travel,
`--source tigerair` for Tigerair, `--source trip` for Trip.com, `--source google_flights`
for Google Flights, `--source agoda` for an Agoda hotel page.

### 2. Read the parsed output

`parse capture` writes rows into the Turso `offers` table. Inspect them with the CLI:

```bash
./bin/travel query-offers --source <id>
# or raw: ./bin/travel db exec "SELECT * FROM offers WHERE source_id='<id>' ORDER BY rowid DESC LIMIT 20"
```

The raw plain-text capture stays in the `captures` table if you need to debug a parse rule.

### 3. Extract structured data

Each parsed offer row carries the normalized fields (flight, hotel, price, dates,
inclusions, date-pricing, itinerary). Per-source field extraction is governed by the
`parser_rules` Turso table — adjust rules there, not in code.

## Offer Fields (conceptual)

The `offers` table holds the normalized equivalents of:
- **flight** — outbound/return: date, flight_number, airline, departure_time, arrival_time, departure_code, arrival_code
- **hotel** — name, area, access, room_type, bed_width_cm
- **price** — per_person, currency, deposit, seats_available, min_travelers
- **dates** — duration_days, duration_nights, year, departure_month, departure_day
- **inclusions** — breakfast, travel_insurance, airport_tax, …
- **date_pricing** — per-date price / availability / seats_remaining
- **itinerary** — per-day content (is_free / is_guided)

## Testing

Parse rules are exercised against real captures in the `captures` table. To re-run a parse:
```bash
./rust/target/debug/chromeport parse capture <capture-id> --source <id>
```

## Requirements

- The chromeport CDP driver built: `cd rust && cargo build -p chromeport`
- A real Chrome reachable at `127.0.0.1:9222` (the driver attaches; it does not launch its own)
- No Python / Playwright — those scrapers are decommissioned

## Registry Reference

OTA configuration lives in the `ota_sources` table in Turso (no JSON files):
```bash
./bin/travel db exec "SELECT source_id, name, status, url_template FROM ota_sources"
```
- `source_id`: Unique identifier (used as `--source <id>` for chromeport)
- `name`: Display name
- `status`: `active` once a live capture path exists
- `url_template`: Base/listing URL for the source

## Adding New OTA Support

See `references/adding-ota.md` for the full step-by-step guide.
