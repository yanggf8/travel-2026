---
name: scrape-ota
description: Scrape travel package details from OTA websites using Playwright. Auto-detects OTA from URL and runs appropriate scraper.
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

- User provides an OTA URL (besttour, liontravel, lifetour, etc.)
- `/p3p4-packages` or `/p3-flights` needs OTA data
- WebFetch fails due to JavaScript rendering
- Need structured tour data (flights, hotel, itinerary, pricing)
- Comparing packages across multiple OTAs

## Supported OTAs

| OTA | Display Name | URL Pattern | Scraper |
|-----|--------------|-------------|---------|
| `besttour` | 喜鴻假期 | `besttour.com.tw/itinerary/*` | `scrape_package.py` |
| `liontravel` | 雄獅旅遊 | `liontravel.com/*`, `vacation.liontravel.com/*` | `scrape_liontravel_dated.py` |
| `lifetour` | 五福旅遊 | `tour.lifetour.com.tw/detail*` | `scrape_package.py` |

## URL Pattern Detection

```python
# Auto-detect OTA from URL
if "besttour.com.tw" in url:
    ota = "besttour"
elif "liontravel.com" in url:
    ota = "liontravel"
elif "lifetour.com.tw" in url:
    ota = "lifetour"
```

## Workflow

### 1. Detect OTA and run scraper

```bash
# Generic scraper (BestTour, Lifetour)
python scripts/scrape_package.py "<url>" data/<ota>-<code>.json

# Lion Travel dated search
python scripts/scrape_liontravel_dated.py --start YYYY-MM-DD --end YYYY-MM-DD data/liontravel-search.json
```

### 2. Read and parse output

```bash
# Check the scraped result
cat data/<ota>-<code>.json | jq '.extracted'
```

### 3. Extract structured data

The scraper returns:
- `raw_text`: Full page text (for manual parsing if needed)
- `extracted`: Structured data (flight, hotel, price, dates, inclusions)
- `extracted_elements`: CSS-selected elements (price_class, flight_class, hotel_class)

## OTA-Specific Parsing

### BestTour (`besttour.com.tw`)
- Full date-specific pricing calendar
- Flight details from 交通方式 tab
- Hotel info with access details

### Lifetour (`tour.lifetour.com.tw`)
- Daily itinerary with guided vs free days
- Multiple hotel options (random assignment)
- Date availability with seats remaining

### Lion Travel (`liontravel.com`)
- Package search results
- Date-specific pricing
- Promo code support (FITPKG: TWD 400 off on Thursdays)

## Output Schema

```json
{
  "url": "https://...",
  "scraped_at": "2026-02-04T...",
  "title": "Tour name",
  "raw_text": "Full page text...",
  "extracted": {
    "flight": {
      "outbound": { "date", "flight_number", "airline", "departure_time", "arrival_time" },
      "return": { ... }
    },
    "hotel": { "name", "area", "access": [] },
    "price": {},
    "dates": {},
    "inclusions": ["light_breakfast"],
    "date_pricing": { "2026-02-13": { "price": 27888, "availability": "available" } }
  },
  "extracted_elements": {
    "price_class": [...],
    "flight_class": [...],
    "hotel_class": [...]
  }
}
```

## Example Session

```
User: "Check this tour: https://tour.lifetour.com.tw/detail?mg=OSA05D268889&sg=OSA05D726227T78"

Agent:
1. Detect OTA: lifetour.com.tw → lifetour
2. Run: python scripts/scrape_package.py "https://..." data/lifetour-OSA05D726227T78.json
3. Read result and extract:
   - Tour: 限量出清三都伴自由5日
   - Price: NT$21,999
   - Flights: AirAsia D7378/D7379
   - Dates: Feb 27 - Mar 3, 2026
4. Present structured summary to user
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
2. Add URL detection pattern to this skill
3. Either:
   - Add OTA-specific parsing to `scrape_package.py`, or
   - Create dedicated scraper script
4. Test with sample URL
