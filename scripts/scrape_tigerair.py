#!/usr/bin/env python3
"""
Tigerair Taiwan Flight Scraper

Scrape flight prices from booking.tigerairtw.com using Playwright.
Unlike URL-based OTA scrapers, this fills the booking form interactively
since Tigerair's SPA doesn't support URL-parameterized search.

Usage:
    python scripts/scrape_tigerair.py --origin TPE --dest NRT --date 2026-02-13 --pax 2 -o data/tigerair-tpe-nrt.json
    python scripts/scrape_tigerair.py --origin TPE --dest KIX --date 2026-02-13 --return-date 2026-02-17 --pax 2

Options:
    --origin       Departure airport code (default: TPE)
    --dest         Arrival airport code (required)
    --date         Departure date YYYY-MM-DD (required)
    --return-date  Return date YYYY-MM-DD (omit for one-way)
    --pax          Number of adult passengers (default: 2)
    --lang         Language: en-US or zh-TW (default: zh-TW)
    -o, --output   Output JSON file path
    --debug        Save screenshot on each step

Requirements:
    pip install playwright
    playwright install chromium
"""

import argparse
import asyncio
import json
import re
import sys
from datetime import datetime

try:
    from playwright.async_api import async_playwright, Page
except ImportError:
    print("Playwright not installed. Run: pip install playwright && playwright install chromium")
    sys.exit(1)


# Tigerair route map — known destinations from TPE
TIGERAIR_ROUTES = {
    "TPE": {
        "NRT": "東京(成田)",
        "KIX": "大阪(關西)",
        "NGO": "名古屋(中部)",
        "OKA": "沖繩(那霸)",
        "CTS": "札幌(新千歲)",
        "FUK": "福岡",
        "ICN": "首爾(仁川)",
        "MFM": "澳門",
        "BKK": "曼谷(廊曼)",
    },
}


async def fill_search_form(page: Page, args: argparse.Namespace, debug: bool = False) -> bool:
    """Fill Tigerair booking search form and submit."""

    booking_url = f"https://booking.tigerairtw.com/{args.lang}/index"
    print(f"Navigating to: {booking_url}")

    try:
        await page.goto(booking_url, wait_until="networkidle", timeout=60000)
    except Exception:
        print("Networkidle timeout, trying domcontentloaded...")
        await page.goto(booking_url, wait_until="domcontentloaded", timeout=60000)

    # Wait for form to render
    await page.wait_for_timeout(3000)

    if debug:
        await page.screenshot(path="/tmp/tigerair-01-loaded.png")
        print("  Screenshot: /tmp/tigerair-01-loaded.png")

    # --- Trip type ---
    trip_type = "roundTrip" if args.return_date else "oneWay"
    print(f"Setting trip type: {trip_type}")

    # Try common selectors for trip type radio/button
    trip_selectors = [
        f"input[value='{trip_type}']",
        f"label:has-text('{trip_type}')",
        f"[data-trip-type='{trip_type}']",
        f"button:has-text('{'來回' if args.return_date else '單程'}')",
        f"label:has-text('{'來回' if args.return_date else '單程'}')",
        f"div:has-text('{'來回' if args.return_date else '單程'}'):not(:has(div))",
    ]
    for sel in trip_selectors:
        try:
            el = await page.query_selector(sel)
            if el:
                await el.click()
                print(f"  Clicked trip type: {sel}")
                await page.wait_for_timeout(500)
                break
        except Exception:
            continue

    # --- Origin airport ---
    print(f"Setting origin: {args.origin}")
    origin_set = await try_set_airport(page, "origin", args.origin, debug)
    if not origin_set:
        print("  WARNING: Could not set origin airport via form interaction")

    # --- Destination airport ---
    print(f"Setting destination: {args.dest}")
    dest_set = await try_set_airport(page, "destination", args.dest, debug)
    if not dest_set:
        print("  WARNING: Could not set destination airport via form interaction")

    # --- Departure date ---
    print(f"Setting departure date: {args.date}")
    await try_set_date(page, "departure", args.date, debug)

    # --- Return date ---
    if args.return_date:
        print(f"Setting return date: {args.return_date}")
        await try_set_date(page, "return", args.return_date, debug)

    # --- Passengers ---
    if args.pax != 1:
        print(f"Setting passengers: {args.pax}")
        await try_set_passengers(page, args.pax, debug)

    if debug:
        await page.screenshot(path="/tmp/tigerair-02-filled.png")
        print("  Screenshot: /tmp/tigerair-02-filled.png")

    # --- Submit search ---
    print("Submitting search...")
    search_selectors = [
        "button[type='submit']",
        "button:has-text('搜尋')",
        "button:has-text('Search')",
        "button:has-text('查詢')",
        "[class*='search'] button",
        "[class*='submit']",
    ]
    for sel in search_selectors:
        try:
            el = await page.query_selector(sel)
            if el and await el.is_visible():
                await el.click()
                print(f"  Clicked search: {sel}")
                break
        except Exception:
            continue

    return True


async def try_set_airport(page: Page, field: str, code: str, debug: bool) -> bool:
    """Try to set airport in the search form."""

    # Strategy 1: Click the field to open dropdown, then select airport
    field_selectors = [
        f"[data-field='{field}']",
        f"[id*='{field}']",
        f"[name*='{field}']",
        f"[class*='{field}']",
        f"input[placeholder*='{'出發' if field == 'origin' else '到達'}']",
        f"input[placeholder*='{'From' if field == 'origin' else 'To'}']",
        f"div:has-text('{'出發地' if field == 'origin' else '目的地'}'):not(:has(div))",
    ]

    for sel in field_selectors:
        try:
            el = await page.query_selector(sel)
            if el:
                await el.click()
                await page.wait_for_timeout(800)

                # Try to find and click the airport code in dropdown
                code_selectors = [
                    f"text={code}",
                    f"[data-code='{code}']",
                    f"li:has-text('{code}')",
                    f"option[value='{code}']",
                    f"div:has-text('{code}'):not(:has(div:has-text('{code}')))",
                ]
                for code_sel in code_selectors:
                    try:
                        code_el = await page.query_selector(code_sel)
                        if code_el and await code_el.is_visible():
                            await code_el.click()
                            print(f"  Selected {field}: {code} via {sel} → {code_sel}")
                            await page.wait_for_timeout(500)
                            return True
                    except Exception:
                        continue
        except Exception:
            continue

    # Strategy 2: Try typing the code into an input field
    input_selectors = [
        f"input[id*='{field}']",
        f"input[name*='{field}']",
        f"input[class*='{field}']",
    ]
    for sel in input_selectors:
        try:
            el = await page.query_selector(sel)
            if el:
                await el.fill("")
                await el.type(code, delay=100)
                await page.wait_for_timeout(1000)
                # Press enter or click first suggestion
                await page.keyboard.press("Enter")
                print(f"  Typed {field}: {code} via {sel}")
                return True
        except Exception:
            continue

    return False


async def try_set_date(page: Page, field: str, date_str: str, debug: bool) -> bool:
    """Try to set a date in the search form."""

    # Parse date
    year, month, day = date_str.split("-")

    # Strategy 1: Click date field to open calendar, then navigate and select
    date_field_selectors = [
        f"[data-field='{field}-date']",
        f"[id*='{field}'][id*='date']",
        f"input[name*='{field}']",
        f"[class*='{field}'][class*='date']",
        f"div:has-text('{'出發日期' if field == 'departure' else '回程日期'}'):not(:has(div))",
    ]

    for sel in date_field_selectors:
        try:
            el = await page.query_selector(sel)
            if el:
                await el.click()
                await page.wait_for_timeout(800)

                # Try to find the specific date in calendar
                day_int = int(day)
                day_selectors = [
                    f"[data-date='{date_str}']",
                    f"td[data-day='{day_int}']",
                    f"button:has-text('{day_int}'):not(:has-text('/'))",
                    f"[aria-label*='{date_str}']",
                ]

                # May need to navigate calendar to correct month
                target_month = f"{year}-{month}"
                await navigate_calendar(page, target_month)

                for day_sel in day_selectors:
                    try:
                        day_el = await page.query_selector(day_sel)
                        if day_el and await day_el.is_visible():
                            await day_el.click()
                            print(f"  Selected {field} date: {date_str}")
                            await page.wait_for_timeout(500)
                            return True
                    except Exception:
                        continue
        except Exception:
            continue

    # Strategy 2: Direct input
    input_selectors = [
        f"input[id*='{field}']",
        f"input[name*='date']",
    ]
    for sel in input_selectors:
        try:
            el = await page.query_selector(sel)
            if el:
                await el.fill(date_str)
                print(f"  Filled {field} date input: {date_str}")
                return True
        except Exception:
            continue

    return False


async def navigate_calendar(page: Page, target_ym: str):
    """Navigate calendar to the target year-month."""
    # Try clicking next month button up to 12 times
    for _ in range(12):
        # Check if current calendar shows target month
        cal_header = await page.query_selector("[class*='calendar'] [class*='header'], [class*='month-year'], [class*='title']")
        if cal_header:
            text = await cal_header.inner_text()
            if target_ym.replace("-", "/") in text or target_ym in text:
                return

        # Click next month
        next_selectors = [
            "button[class*='next']",
            "[class*='next-month']",
            "button:has-text('>')",
            "[aria-label='Next month']",
        ]
        clicked = False
        for sel in next_selectors:
            try:
                el = await page.query_selector(sel)
                if el and await el.is_visible():
                    await el.click()
                    await page.wait_for_timeout(300)
                    clicked = True
                    break
            except Exception:
                continue
        if not clicked:
            break


async def try_set_passengers(page: Page, pax: int, debug: bool) -> bool:
    """Try to set passenger count."""
    # Most airline sites default to 1 adult, need to increase
    pax_selectors = [
        "[class*='passenger']",
        "[id*='passenger']",
        "[data-field='adults']",
        "div:has-text('乘客'):not(:has(div))",
        "div:has-text('Passengers'):not(:has(div))",
    ]

    for sel in pax_selectors:
        try:
            el = await page.query_selector(sel)
            if el:
                await el.click()
                await page.wait_for_timeout(500)

                # Click + button (pax-1) times
                plus_selectors = [
                    "button:has-text('+')",
                    "[class*='increase']",
                    "[class*='plus']",
                    "[aria-label='Add adult']",
                ]
                for _ in range(pax - 1):
                    for plus_sel in plus_selectors:
                        try:
                            plus_el = await page.query_selector(plus_sel)
                            if plus_el and await plus_el.is_visible():
                                await plus_el.click()
                                await page.wait_for_timeout(200)
                                break
                        except Exception:
                            continue

                print(f"  Set passengers: {pax}")
                return True
        except Exception:
            continue

    return False


async def extract_results(page: Page, debug: bool = False) -> dict:
    """Wait for results and extract flight data."""

    print("Waiting for search results...")

    # Wait for results to load (URL change or content change)
    try:
        await page.wait_for_url("**/flight/select**", timeout=30000)
        print("  URL changed to flight select page")
    except Exception:
        print("  URL did not change, checking for results on current page...")

    # Wait for content to render
    await page.wait_for_timeout(5000)

    if debug:
        await page.screenshot(path="/tmp/tigerair-03-results.png")
        print("  Screenshot: /tmp/tigerair-03-results.png")

    # Get full page text
    raw_text = await page.evaluate("() => document.body.innerText")

    # Extract structured flight data
    flights = parse_tigerair_flights(raw_text)

    return {
        "raw_text": raw_text,
        "flights": flights,
    }


def parse_tigerair_flights(raw_text: str) -> list:
    """Parse flight options from Tigerair results page text."""

    flights = []
    lines = raw_text.split("\n")

    # Tigerair flight patterns:
    # - Flight number: IT2XX
    # - Time: HH:MM
    # - Price: TWD or NT$ followed by amount
    # - Duration: Xh Ym

    current_flight = {}

    for i, line in enumerate(lines):
        line = line.strip()
        if not line:
            continue

        # Flight number pattern (IT + 3-4 digits)
        flight_match = re.search(r'\b(IT\s*\d{3,4})\b', line)
        if flight_match:
            if current_flight.get("flight_number"):
                flights.append(current_flight)
                current_flight = {}
            current_flight["flight_number"] = flight_match.group(1).replace(" ", "")

        # Time pattern (departure → arrival)
        time_match = re.findall(r'(\d{1,2}:\d{2})', line)
        if time_match and current_flight.get("flight_number"):
            if len(time_match) >= 2 and "departure_time" not in current_flight:
                current_flight["departure_time"] = time_match[0]
                current_flight["arrival_time"] = time_match[1]
            elif len(time_match) == 1 and "departure_time" not in current_flight:
                current_flight["departure_time"] = time_match[0]
            elif len(time_match) == 1 and "arrival_time" not in current_flight:
                current_flight["arrival_time"] = time_match[0]

        # Price pattern
        price_match = re.search(r'(?:TWD|NT\$?)\s*([\d,]+)', line)
        if price_match and current_flight.get("flight_number"):
            price = int(price_match.group(1).replace(",", ""))
            if price > 500:  # Filter out taxes/fees shown separately
                if "price" not in current_flight or price < current_flight["price"]:
                    current_flight["price"] = price

        # Duration pattern
        dur_match = re.search(r'(\d+)\s*[hH小時]\s*(\d+)?\s*[mM分]?', line)
        if dur_match and current_flight.get("flight_number"):
            hours = int(dur_match.group(1))
            mins = int(dur_match.group(2) or 0)
            current_flight["duration_minutes"] = hours * 60 + mins

    # Don't forget the last flight
    if current_flight.get("flight_number"):
        flights.append(current_flight)

    return flights


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
        browser = await p.chromium.launch(headless=True)
        context = await browser.new_context(
            viewport={"width": 1280, "height": 800},
            locale=args.lang.replace("-", "_"),
        )
        page = await context.new_page()

        try:
            # Fill form and search
            await fill_search_form(page, args, debug=args.debug)

            # Wait and extract outbound results
            outbound = await extract_results(page, debug=args.debug)
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
                # Click first/cheapest flight option
                flight_options = await page.query_selector_all("[class*='flight-option'], [class*='fare'], [class*='journey']")
                if flight_options:
                    try:
                        await flight_options[0].click()
                        await page.wait_for_timeout(3000)

                        inbound = await extract_results(page, debug=args.debug)
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
                print("  Error screenshot: /tmp/tigerair-error.png")

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

    # Output
    output_path = args.output or f"data/tigerair-{args.origin.lower()}-{args.dest.lower()}-{args.date}.json"
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)

    print(f"\nSaved to: {output_path}")

    # Print summary
    s = result.get("summary", {})
    if s.get("cheapest_outbound"):
        print(f"Cheapest outbound: TWD {s['cheapest_outbound']:,}")
    if s.get("cheapest_inbound"):
        print(f"Cheapest return:   TWD {s['cheapest_inbound']:,}")
    if s.get("cheapest_roundtrip"):
        print(f"Cheapest total:    TWD {s['cheapest_roundtrip']:,}/person")


if __name__ == "__main__":
    asyncio.run(main())
