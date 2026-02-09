#!/usr/bin/env python3
"""
Scrape Lion Travel packages with specific date selection.
Uses Playwright to interact with the booking calendar.

Thin wrapper around scrapers.parsers.liontravel.LionTravelParser.
"""

import argparse
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


def parse_args():
    """Parse CLI arguments with argparse for proper --dest support."""
    parser = argparse.ArgumentParser(description="Scrape Lion Travel packages with date selection")
    subparsers = parser.add_subparsers(dest="mode", help="Scrape mode")

    # Search mode
    search_parser = subparsers.add_parser("search", help="Search for packages by date range")
    search_parser.add_argument("dep_date", nargs="?", default="2026-02-11", help="Departure date (YYYY-MM-DD)")
    search_parser.add_argument("ret_date", nargs="?", default="2026-02-15", help="Return date (YYYY-MM-DD)")
    search_parser.add_argument("-o", "--output", default=None, help="Output file path")
    search_parser.add_argument("--dest", default="JP_TYO_6",
                               help="LionTravel destination code (e.g. JP_TYO_6, JP_OSA_5)")

    # Detail mode
    detail_parser = subparsers.add_parser("detail", help="Scrape a specific product detail page")
    detail_parser.add_argument("product_id", nargs="?", default="170525001", help="Product ID")
    detail_parser.add_argument("dep_date", nargs="?", default="2026-02-11", help="Departure date (YYYY-MM-DD)")
    detail_parser.add_argument("days", nargs="?", type=int, default=5, help="Trip duration in days")
    detail_parser.add_argument("-o", "--output", default=None, help="Output file path")

    args = parser.parse_args()

    # Default to search mode if no subcommand given
    if args.mode is None:
        args.mode = "search"
        args.dep_date = "2026-02-11"
        args.ret_date = "2026-02-15"
        args.output = None
        args.dest = "JP_TYO_6"

    return args


async def main():
    args = parse_args()
    parser = LionTravelParser()

    async with async_playwright() as p:
        browser, context, page = await create_browser(p)

        try:
            if args.mode == "search":
                dep_date = args.dep_date
                ret_date = args.ret_date
                output = args.output or f"scrapes/liontravel-search-{dep_date}.json"
                dest = args.dest

                print(f"Searching Lion Travel packages: {dep_date} to {ret_date} (dest={dest})")
                result = await parser.scrape_search(page, dep_date, ret_date, destination=dest)

            elif args.mode == "detail":
                product_id = args.product_id
                dep_date = args.dep_date
                days = args.days
                output = args.output or f"scrapes/liontravel-detail-{product_id}-{dep_date}.json"

                print(f"Fetching Lion Travel product detail: {product_id} for {dep_date} ({days} days)")
                result = await parser.scrape_detail(page, product_id, dep_date, days)

            else:
                print(f"Unknown mode: {args.mode}")
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
