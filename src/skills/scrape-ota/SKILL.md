---
name: scrape-ota
description: Scrape travel package details from OTA websites using Playwright. Auto-detects OTA from URL and runs appropriate scraper.
version: 2.0.0
requires_skills: [travel-shared]
requires_processes: []
provides_processes: []
---

# /scrape-ota

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

See `data/ota-sources.json` for full OTA configuration:
- `source_id`: Unique identifier
- `scraper_script`: Path to scraper (repo-relative)
- `supported`: Whether scraper is implemented
- `rate_limit`: Requests per minute

## Adding New OTA Support

1. Add entry to `data/ota-sources.json`
2. Create parser module in `scripts/scrapers/parsers/<ota>.py`
   - Subclass `BaseScraper`
   - Implement `parse_raw_text()` for pure parsing
   - Override `prepare_page()` for OTA-specific tab clicks etc.
3. Register in `scripts/scrapers/registry.py` (URL pattern + `_create_parser`)
4. Add to `scripts/scrapers/parsers/__init__.py`
5. Add tests in `tests/scrapers/test_parsers.py`
6. Test with sample URL: `python scripts/scrape_package.py "<url>" scrapes/<ota>-test.json`
