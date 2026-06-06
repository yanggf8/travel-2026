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
> ./rust/target/debug/travel-scraper scrape interact "<url>" --source <id> --step 'click:SEL' --step 'fill:SEL=VALUE'
> #    (or, if you've already navigated the tab manually:)
> ./rust/target/debug/travel-scraper browser snapshot --page <N> --source <id>
> # 2) parse the captured page (rule-driven via the parser_rules Turso table) + import to Turso
> ./rust/target/debug/travel-scraper parse capture <capture-id> --source <id>
> ```
> Captures → Turso `captures` table; offers → `offers` table. Per-OTA rules → `parser_rules`.
> Flight/hotel-only OTAs (tigerair, google_flights, trip, agoda, eztravel) are seeded
> `has_custom_parser=1` and currently fail loud — they need a flight/hotel rule shape first.
> The Python tables/paths below are historical reference only.

## Shared references

Read first unless request is a single known-URL scrape:
- `../travel-shared/references/ota-registry.md` — source IDs, region codes, rate limits
- OTA baggage rules / booking notes — from Turso tables `airlines`, `booking_types`, `platform_behaviors`, `comparison_rules` (query directly, e.g. `npm run travel -- turso "SELECT * FROM airlines"`)
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

| OTA | Display Name | URL Pattern | Parser Module | Entry Script |
|-----|--------------|-------------|---------------|-------------|
| `besttour` | 喜鴻假期 | `besttour.com.tw/itinerary/*` | `parsers/besttour.py` | `scrape_package.py` |
| `liontravel` | 雄獅旅遊 | `liontravel.com/*`, `vacation.liontravel.com/*` | `parsers/liontravel.py` | `scrape_liontravel_dated.py` |
| `lifetour` | 五福旅遊 | `tour.lifetour.com.tw/detail*` | `parsers/lifetour.py` | `scrape_package.py` |
| `settour` | 東南旅遊 | `tour.settour.com.tw/product/*` | `parsers/settour.py` | `scrape_package.py` |
| `tigerair` | 台灣虎航 | `booking.tigerairtw.com/*` | `parsers/tigerair.py` | `scrape_tigerair.py` |
| `trip` | Trip.com | `trip.com/flights/*` | `parsers/trip_com.py` | `scrape_date_range.py` |
| `google_flights` | Google Flights | `google.com/travel/flights*` | `parsers/google_flights.py` | `scrape_package.py` |
| `agoda` | Agoda | `agoda.com/*` | `parsers/agoda.py` | `scrape_package.py` |

### Unsupported OTAs

| OTA | Display Name | Status | Reason |
|-----|--------------|--------|--------|
| `skyscanner` | Skyscanner | ❌ Blocked | Hard captcha redirect on all requests. Stealth flags don't help. Last tested: 2026-02-06 |

## Module Architecture

```
scripts/
  scrapers/                    # Python package
    __init__.py                # Public API exports
    schema.py                  # Unified ScrapeResult schema + validation
    base.py                    # BaseScraper class, retry logic, browser helpers
    registry.py                # URL → parser lookup (detect_ota / get_parser)
    parsers/
      __init__.py
      besttour.py              # BestTour: flights, hotel, calendar pricing
      lifetour.py              # Lifetour: flights, hotel, price, itinerary
      settour.py               # Settour: flights, hotel, price, itinerary
      liontravel.py            # Lion Travel: search + detail page scraping
      tigerair.py              # Tigerair: form-based flight search
      trip_com.py              # Trip.com: flight price comparison
      google_flights.py        # Google Flights: multi-airline flight search
      agoda.py                 # Agoda: hotel details and pricing
  scrape_package.py            # Entry point: auto-detects OTA, delegates to parser
  scrape_liontravel_dated.py   # Entry point: Lion Travel dated search
  scrape_tigerair.py           # Entry point: Tigerair form-based search
  scrape_date_range.py         # Entry point: Trip.com multi-date comparison
```

### Key Design Principles

- **Pure parsing separated from browser interaction**: Each parser has `parse_raw_text()` (testable without Playwright) and `scrape()` (needs browser)
- **Unified output schema**: All parsers produce `ScrapeResult` with validation
- **Retry with exponential backoff**: `navigate_with_retry()` handles transient failures
- **Backward compatible**: Entry scripts preserve existing CLI interfaces

## URL Pattern Detection

```python
from scrapers import detect_ota, get_parser

source_id = detect_ota(url)  # Returns "besttour", "liontravel", etc.
parser = get_parser(source_id)
result = parser.parse_raw_text(raw_text)  # Pure parsing, no browser
```

## Workflow

### 0. Pre-flight checks (REQUIRED)

Before any scraping operation, verify environment health:

```bash
# Check Playwright installation and browser availability
python scripts/check_playwright.py

# Auto-install if missing
python scripts/check_playwright.py --install

# Test all OTA scrapers (connectivity + CSS selectors)
npm run scraper:doctor
```

**Why this matters:**
- Playwright missing → Silent failures with cryptic errors
- Outdated CSS selectors → Empty results or parse errors
- OTA site changes → Scraper returns stale/wrong data

**Integration with skills:**
- `/scrape-ota` should run `scraper:doctor` before batch operations
- Single URL scrapes can skip (faster feedback loop)
- CI/CD should run `scraper:doctor` on schedule to detect breakage

### 1. Detect OTA and run scraper

```bash
# Generic scraper (auto-detects OTA from URL)
python scripts/scrape_package.py "<url>" scrapes/<ota>-<code>.json

# Lion Travel dated search
python scripts/scrape_liontravel_dated.py search 2026-02-11 2026-02-15 scrapes/liontravel-search.json

# Tigerair flight search
python scripts/scrape_tigerair.py --origin TPE --dest NRT --date 2026-02-13 --pax 2

# Trip.com date range comparison
python scripts/scrape_date_range.py --depart-start 2026-02-24 --depart-end 2026-02-27 \
    --origin tpe --dest kix --duration 5 --pax 2

# Google Flights (auto-detected from URL)
python scripts/scrape_package.py "https://www.google.com/travel/flights?q=Flights+to+KIX+from+TPE+on+2026-02-26+through+2026-03-02&curr=TWD&hl=zh-TW" scrapes/gf-tpe-kix.json

# Agoda hotel page (auto-detected from URL)
python scripts/scrape_package.py "https://www.agoda.com/cross-hotel-osaka/hotel/osaka-jp.html?checkIn=2026-02-26&los=4&adults=2&rooms=1&currency=TWD" scrapes/agoda-cross-hotel.json
```

### 2. Read and parse output

```bash
cat scrapes/<ota>-<code>.json | jq '.extracted'
```

### 3. Extract structured data

The scraper returns:
- `raw_text`: Full page text (for manual parsing if needed)
- `extracted`: Structured data (flight, hotel, price, dates, inclusions, itinerary)
- `extracted_elements`: CSS-selected elements (price_class, flight_class, hotel_class)

## Output Schema

```json
{
  "url": "https://...",
  "scraped_at": "2026-02-04T...",
  "title": "Tour name",
  "raw_text": "Full page text...",
  "extracted": {
    "flight": {
      "outbound": { "date", "flight_number", "airline", "departure_time", "arrival_time", "departure_code", "arrival_code" },
      "return": { ... }
    },
    "hotel": { "name", "names", "area", "access", "room_type", "bed_width_cm" },
    "price": { "per_person", "currency", "deposit", "seats_available", "min_travelers" },
    "dates": { "duration_days", "duration_nights", "year", "departure_month", "departure_day" },
    "inclusions": ["breakfast", "travel_insurance", "airport_tax"],
    "date_pricing": { "2026-02-13": { "price": 27888, "availability": "available", "seats_remaining": 10 } },
    "itinerary": [{ "day": 1, "content": "...", "is_free": false, "is_guided": true }]
  }
}
```

## Testing

```bash
# Run parser tests (no Playwright needed — pure parsing)
python -m pytest tests/scrapers/ -v
```

## Requirements

```bash
pip install playwright
playwright install chromium
```

## Registry Reference

OTA configuration lives in the `ota_sources` table in Turso (no JSON files):
```bash
npx ts-node scripts/turso-exec.ts "SELECT source_id, display_name, scraper_script, supported, rate_limit FROM ota_sources"
```
- `source_id`: Unique identifier
- `scraper_script`: Path to scraper (repo-relative)
- `supported`: Whether scraper is implemented
- `rate_limit`: Requests per minute

## Adding New OTA Support

See `references/adding-ota.md` for the full step-by-step guide.
