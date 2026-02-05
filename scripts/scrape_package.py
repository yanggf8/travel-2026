#!/usr/bin/env python3
"""
Generic OTA Package Scraper

Scrape travel package details from various OTA websites using Playwright.
Auto-detects OTA from URL and delegates to the appropriate parser module.

Supported OTAs:
- BestTour (besttour.com.tw) - Full calendar pricing
- Lion Travel (liontravel.com) - Package search results
- Lifetour (tour.lifetour.com.tw) - Package with itinerary
- Settour (tour.settour.com.tw) - Package with itinerary
- Trip.com (trip.com) - Flight search
- Any URL with standard page structure

Usage:
    python scrape_package.py <url> [output.json] [--refresh] [--quiet]

Options:
    --refresh   Bypass cache and force fresh scrape
    --quiet     Suppress output

Examples:
    python scrape_package.py "https://www.besttour.com.tw/itinerary/TYO05MM260211AM" data/besttour.json
    python scrape_package.py "https://vacation.liontravel.com/search?Destination=JP_TYO_6" --refresh

Requirements:
    pip install playwright
    playwright install chromium
"""

import asyncio
import json
import sys
from datetime import datetime

try:
    from playwright.async_api import async_playwright
except ImportError:
    print("Playwright not installed. Run: pip install playwright && playwright install chromium")
    sys.exit(1)

from scrapers import detect_ota, get_parser, create_browser
from scrapers.base import (
    navigate_with_retry, scroll_page, safe_extract_text,
    extract_generic_elements, extract_package_links,
)
from scrapers.schema import ScrapeResult, validate_result


async def scrape_package(url: str, use_cache: bool = True) -> dict:
    """Scrape package details from the given URL."""
    source_id = detect_ota(url)

    async with async_playwright() as p:
        browser, context, page = await create_browser(p)

        try:
            if source_id:
                # Use the dedicated parser
                parser = get_parser(source_id)
                result = await parser.scrape(page, url, use_cache=use_cache)
            else:
                # Generic scrape for unknown URLs
                result = await _generic_scrape(page, url)

            # Validate and attach warnings
            warnings = validate_result(result)
            result.warnings.extend(warnings)

        finally:
            await browser.close()

    # Return legacy dict format for backward compatibility
    return result.to_legacy_dict()


async def _generic_scrape(page, url: str) -> ScrapeResult:
    """Generic scraper for URLs that don't match any known OTA."""
    result = ScrapeResult(
        source_id="generic",
        url=url,
        scraped_at=datetime.now().isoformat(),
    )

    success = await navigate_with_retry(page, url)
    if not success:
        result.success = False
        result.errors.append(f"Failed to navigate to {url}")
        return result

    await page.wait_for_timeout(3000)
    await scroll_page(page)

    try:
        result.title = await page.title()
    except Exception:
        pass

    result.raw_text = await safe_extract_text(page)
    result.extracted_elements = await extract_generic_elements(page)
    result.package_links = await extract_package_links(page, url)

    return result


def save_result(result: dict, output_path: str):
    """Save the scraped result to a JSON file."""
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)
    print(f"Saved to: {output_path}")


async def main():
    quiet = "--quiet" in sys.argv
    refresh = "--refresh" in sys.argv
    argv = [a for a in sys.argv[1:] if a not in ("--quiet", "--refresh")]

    url = argv[0] if len(argv) > 0 else "https://www.besttour.com.tw/itinerary/TYO05MM260211AM"
    output = argv[1] if len(argv) > 1 else "data/package-scrape-result.json"

    if refresh:
        print("🔄 Refresh mode: bypassing cache")
    
    result = await scrape_package(url, use_cache=not refresh)

    if not quiet:
        print("\n" + "=" * 60)
        print(f"Title: {result['title']}")
        print("=" * 60)
        print("\nRaw Text (first 2000 chars):")
        print("-" * 60)
        print(result["raw_text"][:2000])
        print("-" * 60)

        if result.get("extracted_elements"):
            print("\nExtracted Elements:")
            for name, texts in result["extracted_elements"].items():
                print(f"\n[{name}]")
                for t in texts[:3]:
                    print(f"  - {t[:200]}...")

    save_result(result, output)


if __name__ == "__main__":
    asyncio.run(main())
