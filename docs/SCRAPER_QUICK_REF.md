# Scraper Quick Reference

## Available Parsers (9 total)

| Source ID | Display Name | Type | Status | Notes |
|-----------|--------------|------|--------|-------|
| besttour | 喜鴻假期 | Package | ✅ | Full calendar pricing |
| liontravel | 雄獅旅遊 | Package/Flight/Hotel | ✅ | Price-only (no flight/hotel extraction) |
| lifetour | 五福旅遊 | Package | ✅ | Full extraction |
| settour | 東南旅遊 | Package | ✅ | Full extraction |
| tigerair | 台灣虎航 | Flight | ✅ | Form-based scraper |
| trip | Trip.com | Flight | ✅ | USD pricing |
| google_flights | Google Flights | Flight | ✅ | Multi-airline comparison |
| agoda | Agoda | Hotel | ✅ | Direct hotel URLs work best |
| eztravel | 易遊網 | Flight | ✅ | Flight search results |

## CLI Commands

### Generic Package Scraper
```bash
python scripts/scrape_package.py <url> [output.json]

# Examples
python scripts/scrape_package.py "https://www.besttour.com.tw/itinerary/TYO05MM260211AM" data/besttour.json
python scripts/scrape_package.py "https://www.agoda.com/hotel/osaka" data/agoda.json
python scripts/scrape_package.py "https://flight.eztravel.com.tw/tickets-tpe-nrt?..." data/eztravel.json
```

### Lion Travel (Date-Specific)
```bash
python scripts/scrape_liontravel_dated.py search [dep_date] [ret_date] [output]
python scripts/scrape_liontravel_dated.py detail [product_id] [dep_date] [days] [output]

# Example
python scripts/scrape_liontravel_dated.py search 2026-02-11 2026-02-15 data/liontravel-feb11.json
```

### Tigerair
```bash
python scripts/scrape_tigerair.py --origin TPE --dest NRT --date 2026-02-13 --pax 2 -o data/tigerair.json
python scripts/scrape_tigerair.py --origin TPE --dest KIX --date 2026-02-13 --return-date 2026-02-17 --pax 2
```

### Date Range Comparison
```bash
python scripts/scrape_date_range.py \
  --depart-start 2026-02-24 \
  --depart-end 2026-02-27 \
  --origin tpe --dest kix \
  --duration 5 --pax 2 \
  --exchange-rate 32.0 \
  -o data/date-range-prices.json
```

## Python API

### Basic Scraping
```python
from scrapers import get_parser, create_browser
from playwright.async_api import async_playwright

async with async_playwright() as p:
    browser, context, page = await create_browser(p)
    
    parser = get_parser("besttour")
    result = await parser.scrape(page, url)
    
    print(f"Price: {result.price.per_person} {result.price.currency}")
    print(f"Flight: {result.flight.outbound.flight_number}")
    
    await browser.close()
```

### With Caching
```python
from scrapers import get_parser
from scrapers.cache import get_cache

parser = get_parser("besttour")
cache = get_cache()

# Check cache first
result = cache.get("besttour", url, date="2026-02-11")
if not result:
    result = await parser.scrape(page, url, use_cache=True)
    # Automatically cached by scrape()

print(f"Warnings: {result.warnings}")  # Shows if from cache
```

### Schema Conversion
```python
from scrapers.converter import convert_to_canonical_offer, convert_scrape_result_file

# In-memory conversion
canonical = convert_to_canonical_offer(result, offer_id="besttour_001")

# File conversion
convert_scrape_result_file(
    "data/besttour-scrape.json",
    "data/besttour-canonical.json",
    "besttour_tyo_feb13"
)
```

### Pure Parsing (No Browser)
```python
from scrapers.parsers.besttour import BestTourParser

parser = BestTourParser()
result = parser.parse_raw_text(raw_text, url=url)

# Useful for testing with fixture data
```

## Cache Management

### Cache Location
```
data/cache/
  ├── a1b2c3d4e5f6g7h8.json  # SHA256 hash of source_id + url + params
  └── ...
```

### Cache API
```python
from scrapers.cache import get_cache

cache = get_cache()

# Get (returns None if expired or missing)
result = cache.get("besttour", url, date="2026-02-11")

# Set
cache.set(result, date="2026-02-11")

# Invalidate
cache.invalidate("besttour", url, date="2026-02-11")

# Clear all
cache.clear()
```

### Cache TTL
- Default: 24 hours
- Configurable: `ScrapeCache(cache_dir="data/cache", default_ttl_hours=48)`
- Age shown in warnings: "Loaded from cache (age: 2h)"

## Schema Mapping

### Python → TypeScript

| Python (snake_case) | TypeScript (camelCase) |
|---------------------|------------------------|
| `departure_airport` | `departureAirport` |
| `arrival_code` | `arrivalCode` |
| `flight_number` | `flightNumber` |
| `per_person` | `pricePerPerson` |
| `price_total` | `priceTotal` |
| `star_rating` | `starRating` |
| `room_type` | `roomType` |
| `return_` | `return` |
| `date_pricing` (dict) | `datePricing` (array) |

### Data Structure Changes

**Python**:
```python
{
  "date_pricing": {
    "2026-02-11": {"price": 18000, "availability": "available"},
    "2026-02-12": {"price": 19000, "availability": "limited"}
  }
}
```

**TypeScript**:
```typescript
{
  "datePricing": [
    {"date": "2026-02-11", "pricePerPerson": 18000, "availability": "available"},
    {"date": "2026-02-12", "pricePerPerson": 19000, "availability": "limited"}
  ]
}
```

## Testing

### Run All Parser Tests
```bash
pytest tests/scrapers/test_parsers.py -v
```

### Run Specific Parser Tests
```bash
pytest tests/scrapers/test_parsers.py::TestBestTourParser -v
pytest tests/scrapers/test_parsers.py::TestEzTravelParser -v
```

### Test with Fixture Data
```python
# Add fixture to tests/scrapers/conftest.py
@pytest.fixture
def my_ota_data():
    return _load_fixture("my-ota-scrape.json")

# Use in test
def test_my_parser(my_ota_data):
    parser = MyOtaParser()
    result = parser.parse_raw_text(my_ota_data["raw_text"])
    assert result.price.per_person > 0
```

## Troubleshooting

### Parser Returns Empty Results
1. Check if page structure changed (inspect `result.raw_text`)
2. Add debug prints in parsing functions
3. Check for bot detection (Cloudflare, captcha)

### Cache Not Working
1. Check `data/cache/` directory exists and is writable
2. Verify cache key params match (case-sensitive)
3. Check TTL hasn't expired

### Schema Conversion Errors
1. Ensure `scraped_at` field is valid ISO datetime
2. Check for missing required fields (`source_id`, `url`)
3. Verify `date_pricing` structure matches expected format

## Rate Limits (from ota-sources.json)

| Source | Requests/Minute |
|--------|-----------------|
| besttour | 10 |
| liontravel | 5 |
| lifetour | 5 |
| settour | 5 |
| tigerair | 10 |
| trip | 5 |
| google_flights | 5 |
| agoda | 3 |
| eztravel | 5 (estimated) |

**Note**: Rate limiting not yet enforced - manual throttling recommended.
