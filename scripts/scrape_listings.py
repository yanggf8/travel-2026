#!/usr/bin/env python3
"""
Listing Scraper - Extract package links from OTA listing pages

Scrapes listing/search pages to get package metadata (title, price, date)
before doing expensive detail scrapes.

Usage:
    python scripts/scrape_listings.py --source besttour --dest kansai --date 2026-02-24
    python scripts/scrape_listings.py --source liontravel --dest osaka --date 2026-02-24 --days 5
    python scripts/scrape_listings.py --url "https://www.besttour.com.tw/e_web/activity?v=japan_kansai"

Options:
    --refresh   Force fresh scrape (bypass cache)

Examples:
    # BestTour Kansai packages
    python scripts/scrape_listings.py --source besttour --dest kansai -o data/besttour-kansai-listings.json
    
    # LionTravel FIT packages for specific dates
    python scripts/scrape_listings.py --source liontravel --dest osaka --date 2026-02-24 --days 5
    
    # Force fresh scrape
    python scripts/scrape_listings.py --source besttour --dest kansai --refresh
"""

import argparse
import asyncio
import json
import re
import sys
from datetime import datetime

try:
    from playwright.async_api import async_playwright
except ImportError:
    print("Playwright not installed. Run: pip install playwright && playwright install chromium")
    sys.exit(1)

from scrapers import detect_ota, get_parser, create_browser
from scrapers.base import navigate_with_retry, safe_extract_text


# OTA-specific listing URL builders
def build_listing_url(source_id: str, destination: str, depart_date: str = "", days: int = 5) -> str:
    """Build listing URL for an OTA."""
    
    if source_id == "besttour":
        # BestTour listing by region
        region_map = {
            "tokyo": "japan_tokyo",
            "kansai": "japan_kansai",
            "osaka": "japan_kansai",
            "hokkaido": "japan_hokkaido",
            "kyushu": "japan_kyushu",
            "okinawa": "japan_okinawa",
        }
        region = region_map.get(destination.lower(), "japan_kansai")
        return f"https://www.besttour.com.tw/e_web/activity?v={region}"
    
    elif source_id == "liontravel":
        # LionTravel FIT search requires dates
        if not depart_date:
            raise ValueError("LionTravel requires --date parameter")
        
        dest_map = {
            "tokyo": f"JP_TYO_{days}",
            "osaka": f"JP_OSA_{days}",
            "kansai": f"JP_OSA_{days}",
            "hokkaido": f"JP_CTS_{days}",
            "okinawa": f"JP_OKA_4",
        }
        dest_code = dest_map.get(destination.lower(), f"JP_OSA_{days}")
        
        from_date = depart_date.replace("-", "")
        # Calculate return date
        from datetime import datetime, timedelta
        dep = datetime.strptime(depart_date, "%Y-%m-%d")
        ret = dep + timedelta(days=days - 1)
        to_date = ret.strftime("%Y%m%d")
        
        return (
            f"https://vacation.liontravel.com/search"
            f"?Destination={dest_code}"
            f"&FromDate={from_date}"
            f"&ToDate={to_date}"
            f"&Days={days}"
            f"&roomlist=2-0-0"
        )
    
    elif source_id == "lifetour":
        # Lifetour group tour listing
        region_map = {
            "tokyo": "0001-0001",
            "kansai": "0001-0003",
            "osaka": "0001-0003",
            "hokkaido": "0001-0002",
            "kyushu": "0001-0004",
            "okinawa": "0001-0005",
        }
        region = region_map.get(destination.lower(), "0001-0003")
        return f"https://tour.lifetour.com.tw/searchlist/tpe/{region}"
    
    elif source_id == "settour":
        # Settour group tour search
        dest_map = {
            "tokyo": "JX_1",
            "kansai": "JX_3",
            "osaka": "JX_3",
            "hokkaido": "JX_2",
            "kyushu": "JX_4",
            "okinawa": "JX_5",
        }
        dest_code = dest_map.get(destination.lower(), "JX_3")
        return f"https://tour.settour.com.tw/search?destinationCode={dest_code}"
    
    else:
        raise ValueError(f"Listing URL builder not implemented for {source_id}")


async def scrape_listings(
    source_id: str,
    url: str,
    depart_date: str = "",
    max_results: int = 50
) -> list[dict]:
    """
    Scrape listing page and extract package metadata.
    
    Returns: [{"url": "...", "title": "...", "price": 18000, "date": "2026-02-24"}]
    """
    async with async_playwright() as p:
        browser, context, page = await create_browser(p)
        
        try:
            print(f"Scraping listing: {url}")
            success = await navigate_with_retry(page, url, timeout=60000)
            if not success:
                print(f"Failed to load {url}")
                return []
            
            await page.wait_for_timeout(3000)
            
            # Scroll to load lazy content
            for _ in range(3):
                await page.evaluate("window.scrollTo(0, document.body.scrollHeight)")
                await page.wait_for_timeout(1000)
            
            raw_text = await safe_extract_text(page)
            
            # Extract package links directly (don't call parser.scrape which navigates again)
            from scrapers.base import extract_package_links
            package_links = await extract_package_links(page, url)
            
            packages = []
            for link in package_links[:max_results]:
                pkg = {
                    "url": link.get("url", ""),
                    "code": link.get("code", ""),
                    "title": link.get("title", ""),
                    "price": _extract_price_from_title(link.get("title", "")),
                    "date": depart_date,
                    "source_id": source_id,
                }
                
                # Try to extract price from nearby text if not in title
                if not pkg["price"]:
                    pkg["price"] = _extract_price_near_link(raw_text, link.get("title", ""))
                
                packages.append(pkg)
            
            print(f"Found {len(packages)} packages")
            return packages
            
        finally:
            await browser.close()


def _extract_price_from_title(title: str) -> int | None:
    """Extract price from title string."""
    # Match patterns like: NT$18,000, TWD 18000, $18,000
    patterns = [
        r"NT\$\s*([\d,]+)",
        r"TWD\s*([\d,]+)",
        r"\$\s*([\d,]+)",
        r"([\d,]+)\s*元",
    ]
    
    for pattern in patterns:
        match = re.search(pattern, title)
        if match:
            price_str = match.group(1).replace(",", "")
            try:
                price = int(price_str)
                if 5000 < price < 200000:  # Sanity check
                    return price
            except ValueError:
                pass
    
    return None


def _extract_price_near_link(raw_text: str, title: str) -> int | None:
    """Extract price from text near the link title."""
    if not title:
        return None
    
    # Find title in raw text and look for price nearby
    idx = raw_text.find(title)
    if idx == -1:
        return None
    
    # Look 200 chars before and after
    context = raw_text[max(0, idx - 200):idx + 200]
    return _extract_price_from_title(context)


def save_listings(listings: list[dict], output_path: str):
    """Save listings to JSON file."""
    data = {
        "scraped_at": datetime.now().isoformat(),
        "count": len(listings),
        "listings": listings,
    }
    
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)
    
    print(f"Saved {len(listings)} listings to {output_path}")


async def main():
    parser = argparse.ArgumentParser(description="Scrape OTA listing pages")
    parser.add_argument("--source", help="OTA source ID (besttour, liontravel, lifetour, settour)")
    parser.add_argument("--dest", help="Destination (tokyo, osaka, kansai, hokkaido)")
    parser.add_argument("--date", help="Departure date YYYY-MM-DD (required for liontravel)")
    parser.add_argument("--days", type=int, default=5, help="Trip duration in days (default: 5)")
    parser.add_argument("--url", help="Direct listing URL (overrides --source/--dest)")
    parser.add_argument("--max", type=int, default=50, help="Max results (default: 50)")
    parser.add_argument("--refresh", action="store_true", help="Force fresh scrape (bypass cache)")
    parser.add_argument("-o", "--output", help="Output JSON file")
    args = parser.parse_args()
    
    if args.refresh:
        print("🔄 Refresh mode: bypassing cache")
    
    # Determine URL
    if args.url:
        url = args.url
        source_id = detect_ota(url)
        if not source_id:
            print(f"Could not detect OTA from URL: {url}")
            sys.exit(1)
    elif args.source and args.dest:
        source_id = args.source
        try:
            url = build_listing_url(source_id, args.dest, args.date or "", args.days)
        except ValueError as e:
            print(f"Error: {e}")
            sys.exit(1)
    else:
        print("Error: Provide either --url or both --source and --dest")
        parser.print_help()
        sys.exit(1)
    
    print(f"Source: {source_id}")
    print(f"URL: {url}")
    if args.date:
        print(f"Date filter: {args.date}")
    print()
    
    # Scrape listings
    listings = await scrape_listings(source_id, url, args.date or "", args.max)
    
    # Print summary
    if listings:
        print(f"\n{'='*60}")
        print(f"Found {len(listings)} packages")
        print(f"{'='*60}\n")
        
        for i, pkg in enumerate(listings[:10], 1):
            price_str = f"TWD {pkg['price']:,}" if pkg['price'] else "Price N/A"
            print(f"{i:2d}. {pkg['title'][:60]}")
            print(f"    {price_str} | {pkg['url'][:80]}")
            print()
    
    # Save to file
    output = args.output or f"data/{source_id}-listings-{datetime.now().strftime('%Y%m%d')}.json"
    save_listings(listings, output)
    
    # Print cheapest
    if listings:
        with_prices = [p for p in listings if p['price']]
        if with_prices:
            cheapest = min(with_prices, key=lambda x: x['price'])
            print(f"\nCheapest: TWD {cheapest['price']:,} - {cheapest['title'][:60]}")


if __name__ == "__main__":
    asyncio.run(main())
