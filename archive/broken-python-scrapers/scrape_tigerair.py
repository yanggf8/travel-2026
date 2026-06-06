#!/usr/bin/env python3
"""
Tigerair Taiwan Flight Scraper

Scrape flight prices from booking.tigerairtw.com using Playwright.
Thin wrapper around scrapers.parsers.tigerair.

Usage:
    python scripts/scrape_tigerair.py --origin TPE --dest NRT --date 2026-02-13 --pax 2 -o scrapes/tigerair-tpe-nrt.json
    python scripts/scrape_tigerair.py --origin TPE --dest KIX --date 2026-02-13 --return-date 2026-02-17 --pax 2

Requirements:
    pip install playwright
    playwright install chromium
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
from scrapers.parsers.tigerair import (
    fill_search_form, extract_flight_results, parse_tigerair_flights,
)


async def scrape_tigerair(args: argparse.Namespace) -> dict:
    """Main scrape function."""
    result = {
        "source": "tigerair",
        "scraped_at": datetime.now().isoformat(),
        "params": {
            "origin": args.origin,
            "destination": args.dest,
            "departure_date": args.date,
            "return_date": args.return_date,
            "pax": args.pax,
        },
        "outbound": {"flights": [], "raw_text": ""},
        "inbound": {"flights": [], "raw_text": ""},
    }

    async with async_playwright() as p:
        browser, context, page = await create_browser(
            p, viewport={"width": 1280, "height": 800}
        )

        try:
            # Fill form and search
            await fill_search_form(
                page,
                origin=args.origin,
                dest=args.dest,
                date=args.date,
                return_date=args.return_date,
                pax=args.pax,
                lang=args.lang,
                debug=args.debug,
            )

            # Wait and extract outbound results
            outbound = await extract_flight_results(page, debug=args.debug)
            result["outbound"]["flights"] = outbound["flights"]
            result["outbound"]["raw_text"] = outbound["raw_text"][:5000]

            print(f"\nOutbound flights found: {len(outbound['flights'])}")
            for f in outbound["flights"]:
                price_str = f"TWD {f['price']:,}" if f.get("price") else "?"
                print(f"  {f.get('flight_number', '?')} "
                      f"{f.get('departure_time', '?')} → {f.get('arrival_time', '?')} "
                      f"  {price_str}")

            # If roundtrip, select cheapest outbound and get return flights
            if args.return_date and outbound["flights"]:
                print("\nSelecting outbound to see return flights...")
                flight_options = await page.query_selector_all(
                    "[class*='flight-option'], [class*='fare'], [class*='journey']"
                )
                if flight_options:
                    try:
                        await flight_options[0].click()
                        await page.wait_for_timeout(3000)

                        inbound = await extract_flight_results(page, debug=args.debug)
                        result["inbound"]["flights"] = inbound["flights"]
                        result["inbound"]["raw_text"] = inbound["raw_text"][:5000]

                        print(f"\nReturn flights found: {len(inbound['flights'])}")
                        for f in inbound["flights"]:
                            price_str = f"TWD {f['price']:,}" if f.get("price") else "?"
                            print(f"  {f.get('flight_number', '?')} "
                                  f"{f.get('departure_time', '?')} → {f.get('arrival_time', '?')} "
                                  f"  {price_str}")
                    except Exception as e:
                        print(f"  Could not get return flights: {e}")

        except Exception as e:
            result["error"] = str(e)
            print(f"\nError during scrape: {e}")
            if args.debug:
                await page.screenshot(path="/tmp/tigerair-error.png")

        finally:
            await browser.close()

    # Summary
    out_count = len(result["outbound"]["flights"])
    in_count = len(result["inbound"]["flights"])
    cheapest_out = min((f["price"] for f in result["outbound"]["flights"] if f.get("price")), default=None)
    cheapest_in = min((f["price"] for f in result["inbound"]["flights"] if f.get("price")), default=None)

    result["summary"] = {
        "outbound_options": out_count,
        "inbound_options": in_count,
        "cheapest_outbound": cheapest_out,
        "cheapest_inbound": cheapest_in,
        "cheapest_roundtrip": (cheapest_out or 0) + (cheapest_in or 0) if cheapest_out and cheapest_in else None,
    }

    return result


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Scrape Tigerair Taiwan flights")
    parser.add_argument("--origin", default="TPE", help="Departure airport (default: TPE)")
    parser.add_argument("--dest", required=True, help="Destination airport (e.g., NRT, KIX)")
    parser.add_argument("--date", required=True, help="Departure date (YYYY-MM-DD)")
    parser.add_argument("--return-date", default=None, help="Return date (YYYY-MM-DD, omit for one-way)")
    parser.add_argument("--pax", type=int, default=2, help="Number of adult passengers (default: 2)")
    parser.add_argument("--lang", default="zh-TW", help="Language: en-US or zh-TW (default: zh-TW)")
    parser.add_argument("-o", "--output", default=None, help="Output JSON file")
    parser.add_argument("--debug", action="store_true", help="Save screenshots at each step")
    return parser.parse_args()


async def main():
    args = parse_args()

    print(f"Tigerair Scraper: {args.origin} → {args.dest}")
    print(f"Date: {args.date}" + (f" → {args.return_date}" if args.return_date else " (one-way)"))
    print(f"Pax: {args.pax}")
    print()

    result = await scrape_tigerair(args)

    output_path = args.output or f"scrapes/tigerair-{args.origin.lower()}-{args.dest.lower()}-{args.date}.json"
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)

    print(f"\nSaved to: {output_path}")

    s = result.get("summary", {})
    if s.get("cheapest_outbound"):
        print(f"Cheapest outbound: TWD {s['cheapest_outbound']:,}")
    if s.get("cheapest_inbound"):
        print(f"Cheapest return:   TWD {s['cheapest_inbound']:,}")
    if s.get("cheapest_roundtrip"):
        print(f"Cheapest total:    TWD {s['cheapest_roundtrip']:,}/person")


if __name__ == "__main__":
    asyncio.run(main())
