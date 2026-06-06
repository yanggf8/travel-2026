#!/usr/bin/env python3
"""Scrape Google Flights for cheapest flights across destinations/date ranges."""
import argparse
import asyncio
import json
import re
import sys
from datetime import datetime, timedelta
from playwright.async_api import async_playwright

DEFAULT_DESTINATIONS = {
    "KIX": "Osaka (Kyoto)",
    "FUK": "Fukuoka",
    "NGO": "Nagoya",
    "IBR": "Ibaraki",
}


def parse_date(value):
    try:
        return datetime.strptime(value, "%Y-%m-%d")
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"invalid date {value!r}; expected YYYY-MM-DD") from exc


def parse_csv(values, transform=str):
    items = []
    for value in values:
        for part in value.split(","):
            part = part.strip()
            if part:
                items.append(transform(part))
    return items


def build_parser():
    parser = argparse.ArgumentParser(
        description="Scrape Google Flights round-trip prices across destination/date combinations.",
    )
    parser.add_argument("--origin", default="TPE", help="Origin airport IATA code (default: TPE)")
    parser.add_argument(
        "--dest",
        action="append",
        default=[],
        help="Destination IATA code. Repeat or comma-separate. Defaults to KIX,FUK,NGO,IBR.",
    )
    parser.add_argument(
        "--depart-start",
        type=parse_date,
        required=True,
        help="First departure date, YYYY-MM-DD.",
    )
    parser.add_argument(
        "--depart-end",
        type=parse_date,
        required=True,
        help="Last departure date, YYYY-MM-DD.",
    )
    parser.add_argument(
        "--duration",
        action="append",
        default=[],
        help="Trip duration in days. Repeat or comma-separate (default: 4,5).",
    )
    parser.add_argument("--currency", default="TWD", help="Google Flights currency (default: TWD)")
    parser.add_argument("--hl", default="zh-TW", help="Google Flights locale (default: zh-TW)")
    parser.add_argument("--timeout-ms", type=int, default=30000, help="Page load timeout (default: 30000)")
    parser.add_argument("--wait-sec", type=float, default=6.0, help="Post-load render wait (default: 6)")
    parser.add_argument("-o", "--output", help="Write JSON results to this file instead of stdout")
    return parser


def date_range(start, end):
    if end < start:
        raise ValueError("--depart-end must be on or after --depart-start")
    days = []
    current = start
    while current <= end:
        days.append(current.strftime("%Y-%m-%d"))
        current += timedelta(days=1)
    return days


async def scrape_google_flights(page, origin, dest, date_str, currency, hl, timeout_ms, wait_sec):
    """Scrape Google Flights for one-way price on a specific date."""
    url = (
        f"https://www.google.com/travel/flights"
        f"?q=Flights+to+{dest}+from+{origin}+on+{date_str}"
        f"&curr={currency}&hl={hl}"
    )
    result = {"date": date_str, "dest": dest, "cheapest_twd": None, "airline": None, "flight_info": None, "error": None}

    try:
        print(f"  Loading {origin}→{dest} {date_str}...", file=sys.stderr)
        await page.goto(url, wait_until="domcontentloaded", timeout=timeout_ms)
        await asyncio.sleep(wait_sec)

        # Try to get price info from the page
        text = await page.inner_text("body")

        # Parse TWD prices from text
        twd_prices = re.findall(r"NT\$([\d,]+)", text)
        if twd_prices:
            prices = [int(p.replace(",", "")) for p in twd_prices]
            prices.sort()
            result["cheapest_twd"] = prices[0] if prices else None
            result["all_prices"] = prices[:5]

        # Try to get the cheapest flight card info
        try:
            # Google Flights uses specific class patterns - try multiple selectors
            for selector in [
                'li[role="listitem"]',
                '.gws-flights-results__result-item',
                '[jsname="Ak6kYe"]',
            ]:
                elements = await page.query_selector_all(selector)
                if elements:
                    first = await elements[0].inner_text()
                    result["flight_info"] = first[:200]
                    break
        except Exception as exc:
            result["flight_info_error"] = str(exc)

        if result["cheapest_twd"]:
            print(f"    Cheapest: TWD {result['cheapest_twd']:,}", file=sys.stderr)
        else:
            print(f"    No price found", file=sys.stderr)

    except Exception as e:
        result["error"] = str(e)
        print(f"    Error: {e}", file=sys.stderr)

    return result


async def search_roundtrip(page, origin, dest, depart_date, return_date, currency, hl, timeout_ms, wait_sec):
    """Search round-trip flights."""
    url = (
        f"https://www.google.com/travel/flights"
        f"?q=Flights+to+{dest}+from+{origin}"
        f"+on+{depart_date}"
        f"+return+on+{return_date}"
        f"&curr={currency}&hl={hl}"
    )
    result = {
        "depart": depart_date, "return": return_date, "dest": dest,
        "cheapest_twd": None, "airline": None, "stops": None, "error": None,
    }

    try:
        print(f"  Loading {origin}↔{dest} {depart_date}→{return_date}...", file=sys.stderr)
        await page.goto(url, wait_until="domcontentloaded", timeout=timeout_ms)
        await asyncio.sleep(wait_sec)

        text = await page.inner_text("body")

        twd_prices = re.findall(r"NT\$([\d,]+)", text)
        if twd_prices:
            prices = [int(p.replace(",", "")) for p in twd_prices]
            prices.sort()
            result["cheapest_twd"] = prices[0]
            result["all_prices_twd"] = prices[:5]
            print(f"    Round-trip: TWD {result['cheapest_twd']:,}", file=sys.stderr)

        # Try to extract flight details
        for sel in ['[jsname="Ak6kYe"]', '.gws-flights-results__result-item', '[role="listitem"]']:
            els = await page.query_selector_all(sel)
            if els:
                text = await els[0].inner_text()
                # extract airline
                for airline in ["Peach", "Jetstar", "Tigerair", "Scoot", "EVA", "China Airlines", "Starlux", "Cathay", "HK Express", "AirAsia", "Thai Lion"]:
                    if airline.lower() in text.lower():
                        result["airline"] = airline
                        break
                # extract stops
                if "nonstop" in text.lower() or "直飞" in text.lower():
                    result["stops"] = "nonstop"
                elif "1 stop" in text.lower():
                    result["stops"] = "1 stop"
                break

    except Exception as e:
        result["error"] = str(e)

    return result


async def run(args):
    dest_codes = parse_csv(args.dest) if args.dest else list(DEFAULT_DESTINATIONS)
    durations = parse_csv(args.duration, int) if args.duration else [4, 5]
    depart_dates = date_range(args.depart_start, args.depart_end)

    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        context = await browser.new_context(
            viewport={"width": 1280, "height": 800},
            user_agent="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"
        )
        page = await context.new_page()

        results = []

        for dest_code in dest_codes:
            dest_name = DEFAULT_DESTINATIONS.get(dest_code.upper(), dest_code.upper())
            print(f"\n{'='*60}", file=sys.stderr)
            print(f"Searching {dest_name} ({dest_code})...", file=sys.stderr)
            print(f"{'='*60}", file=sys.stderr)

            for dep_date in depart_dates:
                for duration in durations:
                    ret = datetime.strptime(dep_date, "%Y-%m-%d") + timedelta(days=duration)
                    ret_date = ret.strftime("%Y-%m-%d")

                    r = await search_roundtrip(
                        page,
                        args.origin,
                        dest_code.upper(),
                        dep_date,
                        ret_date,
                        args.currency,
                        args.hl,
                        args.timeout_ms,
                        args.wait_sec,
                    )
                    r["duration"] = duration
                    results.append(r)
                    await asyncio.sleep(1)  # be gentle

        await browser.close()

        # Sort by price
        results.sort(key=lambda x: x["cheapest_twd"] or 999999)
        return results


def main():
    parser = build_parser()
    args = parser.parse_args()
    results = asyncio.run(run(args))
    output = json.dumps(results, indent=2, ensure_ascii=False)
    if args.output:
        with open(args.output, "w", encoding="utf-8") as f:
            f.write(output)
            f.write("\n")
    else:
        print(output)

if __name__ == "__main__":
    main()
