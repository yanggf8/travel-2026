#!/usr/bin/env python3
"""
Scrape Lion Travel packages with specific date selection.
Uses Playwright to interact with the booking calendar.

Thin wrapper around scrapers.parsers.liontravel.LionTravelParser.
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

from scrapers import create_browser
from scrapers.parsers.liontravel import LionTravelParser


def save_result(result: dict, output_path: str):
    """Save the scraped result to a JSON file."""
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)
    print(f"Saved to: {output_path}")


async def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "search"
    parser = LionTravelParser()

    async with async_playwright() as p:
        browser, context, page = await create_browser(p)

        try:
            if mode == "search":
                dep_date = sys.argv[2] if len(sys.argv) > 2 else "2026-02-11"
                ret_date = sys.argv[3] if len(sys.argv) > 3 else "2026-02-15"
                output = sys.argv[4] if len(sys.argv) > 4 else f"scrapes/liontravel-search-{dep_date}.json"

                print(f"Searching Lion Travel packages: {dep_date} to {ret_date}")
                result = await parser.scrape_search(page, dep_date, ret_date)

            elif mode == "detail":
                product_id = sys.argv[2] if len(sys.argv) > 2 else "170525001"
                dep_date = sys.argv[3] if len(sys.argv) > 3 else "2026-02-11"
                days = int(sys.argv[4]) if len(sys.argv) > 4 else 5
                output = sys.argv[5] if len(sys.argv) > 5 else f"scrapes/liontravel-detail-{product_id}-{dep_date}.json"

                print(f"Fetching Lion Travel product detail: {product_id} for {dep_date} ({days} days)")
                result = await parser.scrape_detail(page, product_id, dep_date, days)

            else:
                print(f"Unknown mode: {mode}")
                print("Usage:")
                print("  python scrape_liontravel_dated.py search [dep_date] [ret_date] [output]")
                print("  python scrape_liontravel_dated.py detail [product_id] [dep_date] [days] [output]")
                sys.exit(1)

        finally:
            await browser.close()

    # Convert to legacy dict for output
    result_dict = result.to_legacy_dict()

    # Print summary
    print("\n" + "=" * 60)
    print("SCRAPE RESULTS")
    print("=" * 60)

    if result.package_links:
        print(f"\nFound {len(result.package_links)} packages:")
        for pkg in result.package_links[:5]:
            print(f"  - {pkg.get('title', 'No title')[:50]}: {pkg.get('prices_found', [])}")

    if result.price.is_populated:
        print(f"\nPricing: per_person={result.price.per_person}, total={result.price.total}")

    if result.errors:
        print(f"\nErrors: {result.errors}")

    save_result(result_dict, output)


if __name__ == "__main__":
    asyncio.run(main())
