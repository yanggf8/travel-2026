# Scripts

OTA (Online Travel Agency) scraping utilities for the Japan Travel project.

## Prerequisites

```bash
pip install playwright
playwright install chromium
```

## Available Scripts

### `scrape_package.py` - Generic OTA Scraper

A general-purpose scraper that works with any OTA website. Extracts raw page text and attempts to find structured elements.

**Usage:**
```bash
python scripts/scrape_package.py <url> [output.json]
```

**Examples:**
```bash
# BestTour package page (best for date-specific pricing)
python scripts/scrape_package.py \
  "https://www.besttour.com.tw/itinerary/TYO05MM260211AM" \
  scrapes/besttour-tokyo.json

# Lion Travel search results
python scripts/scrape_package.py \
  "https://vacation.liontravel.com/search?Destination=JP_TYO_6&roomlist=2-0-0" \
  scrapes/liontravel-search.json
```

**Output format:**
```json
{
  "url": "...",
  "scraped_at": "ISO-8601 timestamp",
  "title": "Page title",
  "raw_text": "Full page text content",
  "extracted": {
    "flight": {},
    "hotel": {},
    "price": {},
    "dates": {},
    "inclusions": []
  },
  "extracted_elements": {
    "price_element": ["..."],
    "tables": ["..."]
  }
}
```

---

### `scrape_liontravel_dated.py` - Lion Travel Date-Specific Scraper

Specialized scraper for Lion Travel vacation packages with date selection support.

**Modes:**

1. **Search mode** - Search for packages with specific dates
2. **Detail mode** - Scrape a specific product detail page

**Usage:**
```bash
# Search mode
python scripts/scrape_liontravel_dated.py search <dep_date> <ret_date> [output.json]

# Detail mode  
python scripts/scrape_liontravel_dated.py detail <product_id> <dep_date> <days> [output.json]
```

**Examples:**
```bash
# Search Tokyo packages for Feb 11-15
python scripts/scrape_liontravel_dated.py search 2026-02-11 2026-02-15 scrapes/liontravel-feb11.json

# Get detail for specific product
python scripts/scrape_liontravel_dated.py detail 170525001 2026-02-11 5 scrapes/liontravel-detail.json
```

**Search output format:**
```json
{
  "url": "...",
  "scraped_at": "ISO-8601 timestamp",
  "search_params": {
    "departure_date": "2026-02-11",
    "return_date": "2026-02-15",
    "destination": "JP_TYO_6",
    "days": 5,
    "adults": 2
  },
  "packages": [
    {
      "title": "Package name",
      "prices_found": ["9,430", "10,360"],
      "link": "/detail/..."
    }
  ],
  "raw_prices": ["9,430", "10,360", ...],
  "errors": []
}
```

---

## OTA URL Patterns

| OTA | URL Pattern | Notes |
|-----|-------------|-------|
| **BestTour** | `besttour.com.tw/itinerary/{product_code}` | Full calendar with date picker |
| **Lion Travel** | `vacation.liontravel.com/search?Destination={code}&FromDate={YYYYMMDD}&ToDate={YYYYMMDD}&roomlist={adults}-{children}-{infants}` | Search results |
| **Lion Travel** | `vacation.liontravel.com/detail/{product_id}?FromDate={YYYYMMDD}&Days={n}&roomlist=...` | Product detail |
| **ezTravel** | `flight.eztravel.com.tw/tickets-{origin}-{dest}?tripType=roundTrip&depDate={YYYY-MM-DD}&retDate={YYYY-MM-DD}&adult={n}` | Flight search |

### Destination Codes (Lion Travel)
- `JP_TYO_6` - Tokyo
- `JP_OSA_6` - Osaka
- `JP_NGO_6` - Nagoya
- `JP_FUK_6` - Fukuoka

---

## Troubleshooting

### Timeout errors
Some OTAs have slow JavaScript rendering. The scrapers will:
1. First try `networkidle` (wait for all requests to complete)
2. Fall back to `domcontentloaded` if timeout occurs

### Bot detection
Skyscanner and some aggregators block headless browsers. Use direct airline/OTA sites instead.

### Missing prices
Many OTAs show "starting from" prices in search results. For exact date-specific pricing:
1. Use BestTour (shows full calendar)
2. Or navigate to detail page and select dates
