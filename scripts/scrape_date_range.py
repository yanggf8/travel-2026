#!/usr/bin/env python3
"""
Scrape flight prices across a date range.

Given a departure range, scrapes one-way outbound and return flight prices
from Trip.com for each date, then outputs a ranked comparison table.

Usage:
    python scripts/scrape_date_range.py --depart-start 2026-02-24 --depart-end 2026-02-27 \\
        --origin tpe --dest kix --duration 5 --pax 2 [--output data/date-range-prices.json]

Requirements:
    pip install playwright
    playwright install chromium
"""

import argparse
import asyncio
import json
import sys
from datetime import datetime, timedelta
from typing import Any

try:
    from playwright.async_api import async_playwright
except ImportError:
    print("Playwright not installed. Run: pip install playwright && playwright install chromium")
    sys.exit(1)


# City name mapping for Trip.com URL
CITY_NAMES = {
    "tpe": "taipei",
    "kix": "osaka",
    "nrt": "tokyo",
    "hnd": "tokyo",
    "knh": "kinmen",
    "cts": "sapporo",
    "oka": "okinawa",
    "fuk": "fukuoka",
    "ngo": "nagoya",
}


def date_range(start: str, end: str) -> list[str]:
    """Generate list of dates from start to end inclusive."""
    s = datetime.strptime(start, "%Y-%m-%d")
    e = datetime.strptime(end, "%Y-%m-%d")
    dates = []
    current = s
    while current <= e:
        dates.append(current.strftime("%Y-%m-%d"))
        current += timedelta(days=1)
    return dates


def add_days(date_str: str, days: int) -> str:
    """Add N days to a date string."""
    d = datetime.strptime(date_str, "%Y-%m-%d")
    return (d + timedelta(days=days)).strftime("%Y-%m-%d")


def day_of_week(date_str: str) -> str:
    """Get day of week for a date."""
    d = datetime.strptime(date_str, "%Y-%m-%d")
    days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
    return days[d.weekday()]


async def scrape_flight_price(page, origin: str, dest: str, date: str, pax: int) -> dict:
    """Scrape one-way flight prices for a specific date."""
    origin_name = CITY_NAMES.get(origin, origin)
    dest_name = CITY_NAMES.get(dest, dest)

    url = (
        f"https://www.trip.com/flights/{origin_name}-to-{dest_name}/"
        f"tickets-{origin}-{dest}?dcity={origin}&acity={dest}"
        f"&ddate={date}&flighttype=ow&class=y&quantity={pax}"
    )

    result = {
        "date": date,
        "day": day_of_week(date),
        "direction": f"{origin.upper()}→{dest.upper()}",
        "url": url,
        "cheapest_price_usd": None,
        "cheapest_airline": None,
        "cheapest_time": None,
        "nonstop_cheapest_usd": None,
        "nonstop_cheapest_airline": None,
        "nonstop_cheapest_time": None,
        "flights": [],
        "error": None,
    }

    try:
        print(f"  Scraping {origin.upper()}→{dest.upper()} {date} ({day_of_week(date)})...")
        try:
            await page.goto(url, wait_until="networkidle", timeout=45000)
        except Exception:
            await page.goto(url, wait_until="domcontentloaded", timeout=30000)
            await asyncio.sleep(5)

        # Wait for flight results to load
        await asyncio.sleep(3)

        # Extract text content
        raw_text = await page.evaluate("document.body.innerText")

        # Parse cheapest price from price calendar
        # Trip.com format: "Mon, Feb 24\nUS$140"
        import re
        lines = raw_text.split("\n")

        # Extract nonstop flights
        flights = []
        i = 0
        while i < len(lines):
            line = lines[i].strip()
            # Look for airline names followed by time patterns
            if re.match(r'^\d{1,2}:\d{2}$', line):
                # This is a departure time, look back for airline
                airline = ""
                for j in range(max(0, i - 3), i):
                    candidate = lines[j].strip()
                    if candidate and not re.match(r'^(Carry-on|Included|Checked|<\d|CO2)', candidate):
                        airline = candidate

                depart_time = line
                # Look ahead for route details
                if i + 4 < len(lines):
                    origin_airport = lines[i + 1].strip() if i + 1 < len(lines) else ""
                    duration = lines[i + 2].strip() if i + 2 < len(lines) else ""
                    nonstop = "Nonstop" in (lines[i + 3].strip() if i + 3 < len(lines) else "")
                    arrive_time = lines[i + 4].strip() if i + 4 < len(lines) else ""

                    # Find price - look ahead for US$ pattern
                    price = None
                    for k in range(i + 4, min(i + 10, len(lines))):
                        price_match = re.match(r'US\$(\d[\d,]*)', lines[k].strip())
                        if price_match:
                            price = int(price_match.group(1).replace(",", ""))
                            break

                    # Also look for Total price
                    total_price = None
                    for k in range(i + 4, min(i + 12, len(lines))):
                        total_match = re.match(r'Total US\$(\d[\d,]*)', lines[k].strip())
                        if total_match:
                            total_price = int(total_match.group(1).replace(",", ""))
                            break

                    if price and nonstop:
                        flights.append({
                            "airline": airline,
                            "depart": depart_time,
                            "arrive": arrive_time,
                            "duration": duration,
                            "nonstop": True,
                            "price_per_person_usd": price,
                            "total_usd": total_price or price * pax,
                        })
            i += 1

        result["flights"] = flights

        if flights:
            cheapest = min(flights, key=lambda f: f["total_usd"])
            result["nonstop_cheapest_usd"] = cheapest["total_usd"]
            result["nonstop_cheapest_airline"] = cheapest["airline"]
            result["nonstop_cheapest_time"] = f"{cheapest['depart']}→{cheapest['arrive']}"

        # Also get overall cheapest from calendar
        for line in lines:
            price_match = re.search(r'US\$(\d[\d,]*)', line)
            if price_match and result["cheapest_price_usd"] is None:
                result["cheapest_price_usd"] = int(price_match.group(1).replace(",", ""))
                break

    except Exception as e:
        result["error"] = str(e)
        print(f"    Error: {e}")

    return result


async def scrape_date_range(
    depart_dates: list[str],
    origin: str,
    dest: str,
    duration: int,
    pax: int,
) -> list[dict]:
    """Scrape outbound + return prices for each departure date."""

    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        page = await browser.new_page()

        results = []

        for depart_date in depart_dates:
            return_date = add_days(depart_date, duration - 1)
            print(f"\n📅 {depart_date} ({day_of_week(depart_date)}) → {return_date} ({day_of_week(return_date)})")

            # Scrape outbound
            outbound = await scrape_flight_price(page, origin, dest, depart_date, pax)

            # Scrape return
            inbound = await scrape_flight_price(page, dest, origin, return_date, pax)

            # Combine
            entry = {
                "depart_date": depart_date,
                "return_date": return_date,
                "depart_day": day_of_week(depart_date),
                "return_day": day_of_week(return_date),
                "outbound": outbound,
                "inbound": inbound,
                "combined_cheapest_usd": None,
                "combined_cheapest_twd": None,
            }

            # Calculate combined cheapest (nonstop)
            out_price = outbound.get("nonstop_cheapest_usd")
            in_price = inbound.get("nonstop_cheapest_usd")
            if out_price and in_price:
                combined = out_price + in_price
                entry["combined_cheapest_usd"] = combined
                entry["combined_cheapest_twd"] = round(combined * 32)

            results.append(entry)

        await browser.close()

    # Sort by combined price
    results.sort(key=lambda r: r["combined_cheapest_usd"] or float("inf"))

    return results


def print_summary(results: list[dict], pax: int):
    """Print summary comparison table."""
    print("\n" + "=" * 80)
    print(f"機票價格比較 ({pax}人直飛)")
    print("=" * 80)
    print()
    print(f"| 出發日 | 回程日 | 去程最便宜 | 回程最便宜 | 來回合計 | TWD 估算 |")
    print(f"|--------|--------|-----------|-----------|---------|---------|")

    for i, r in enumerate(results):
        out = r["outbound"]
        inb = r["inbound"]

        out_str = f"US${out['nonstop_cheapest_usd']}" if out['nonstop_cheapest_usd'] else "N/A"
        in_str = f"US${inb['nonstop_cheapest_usd']}" if inb['nonstop_cheapest_usd'] else "N/A"
        combined_str = f"US${r['combined_cheapest_usd']}" if r['combined_cheapest_usd'] else "N/A"
        twd_str = f"TWD {r['combined_cheapest_twd']:,}" if r['combined_cheapest_twd'] else "N/A"
        marker = " 🏆" if i == 0 and r['combined_cheapest_usd'] else ""

        print(
            f"| {r['depart_date']} ({r['depart_day']}) | {r['return_date']} ({r['return_day']}) "
            f"| {out_str:>9} | {in_str:>9} | {combined_str:>7} | {twd_str:>10}{marker} |"
        )

    print()

    # Detailed flight options
    for r in results:
        print(f"\n--- {r['depart_date']} ({r['depart_day']}) → {r['return_date']} ({r['return_day']}) ---")
        if r["outbound"]["flights"]:
            print(f"  去程 ({r['depart_date']}):")
            for f in sorted(r["outbound"]["flights"], key=lambda x: x["total_usd"])[:5]:
                print(f"    {f['airline']:<20} {f['depart']}→{f['arrive']}  US${f['total_usd']} ({pax}人)")
        if r["inbound"]["flights"]:
            print(f"  回程 ({r['return_date']}):")
            for f in sorted(r["inbound"]["flights"], key=lambda x: x["total_usd"])[:5]:
                print(f"    {f['airline']:<20} {f['depart']}→{f['arrive']}  US${f['total_usd']} ({pax}人)")


def main():
    parser = argparse.ArgumentParser(description="Scrape flight prices across date range")
    parser.add_argument("--depart-start", required=True, help="First departure date (YYYY-MM-DD)")
    parser.add_argument("--depart-end", required=True, help="Last departure date (YYYY-MM-DD)")
    parser.add_argument("--origin", required=True, help="Origin airport code (e.g., tpe)")
    parser.add_argument("--dest", required=True, help="Destination airport code (e.g., kix)")
    parser.add_argument("--duration", type=int, required=True, help="Trip duration in days")
    parser.add_argument("--pax", type=int, default=2, help="Number of passengers (default: 2)")
    parser.add_argument("--output", "-o", help="Output JSON file path")

    args = parser.parse_args()

    depart_dates = date_range(args.depart_start, args.depart_end)
    print(f"Scraping {len(depart_dates)} departure dates: {', '.join(depart_dates)}")
    print(f"Route: {args.origin.upper()} → {args.dest.upper()}, {args.duration} days, {args.pax} pax")

    results = asyncio.run(scrape_date_range(
        depart_dates=depart_dates,
        origin=args.origin.lower(),
        dest=args.dest.lower(),
        duration=args.duration,
        pax=args.pax,
    ))

    # Save JSON
    if args.output:
        output_data = {
            "scraped_at": datetime.now().isoformat(),
            "params": {
                "depart_start": args.depart_start,
                "depart_end": args.depart_end,
                "origin": args.origin,
                "dest": args.dest,
                "duration": args.duration,
                "pax": args.pax,
            },
            "results": results,
        }
        with open(args.output, "w", encoding="utf-8") as f:
            json.dump(output_data, f, ensure_ascii=False, indent=2)
        print(f"\nSaved to: {args.output}")

    # Print summary table
    print_summary(results, args.pax)


if __name__ == "__main__":
    main()
