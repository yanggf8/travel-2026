#!/usr/bin/env python3
"""
Scrape flight prices across a date range.

Given a departure range, scrapes one-way outbound and return flight prices
from Trip.com for each date, then outputs a ranked comparison table.

Thin wrapper around scrapers.parsers.trip_com.

Usage:
    python scripts/scrape_date_range.py --depart-start 2026-02-24 --depart-end 2026-02-27 \\
        --origin tpe --dest kix --duration 5 --pax 2 --exchange-rate 32 [--output data/date-range-prices.json]

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
from scrapers.base import navigate_with_retry, safe_extract_text
from scrapers.parsers.trip_com import (
    parse_nonstop_flights, build_oneway_url,
    date_range, add_days, day_of_week,
)


async def scrape_flight_price(page, origin: str, dest: str, date: str, pax: int) -> dict:
    """Scrape one-way flight prices for a specific date."""
    url = build_oneway_url(origin, dest, date, pax)

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
        success = await navigate_with_retry(page, url, max_retries=2, timeout=45000)
        if not success:
            result["error"] = f"Failed to load {url}"
            return result

        await asyncio.sleep(3)

        raw_text = await safe_extract_text(page)
        flights = parse_nonstop_flights(raw_text, pax)
        result["flights"] = flights

        if flights:
            cheapest = min(flights, key=lambda f: f["total_usd"])
            result["nonstop_cheapest_usd"] = cheapest["total_usd"]
            result["nonstop_cheapest_airline"] = cheapest["airline"]
            result["nonstop_cheapest_time"] = f"{cheapest['depart']}→{cheapest['arrive']}"

        # Overall cheapest from any price on page
        import re
        for line in raw_text.split("\n"):
            price_match = re.search(r"US\$(\d[\d,]*)", line)
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
    args,
) -> list[dict]:
    """Scrape outbound + return prices for each departure date."""
    async with async_playwright() as p:
        browser, _context, page = await create_browser(p)

        results = []
        for depart_date in depart_dates:
            return_date = add_days(depart_date, duration - 1)
            print(f"\n📅 {depart_date} ({day_of_week(depart_date)}) → {return_date} ({day_of_week(return_date)})")

            outbound = await scrape_flight_price(page, origin, dest, depart_date, pax)
            inbound = await scrape_flight_price(page, dest, origin, return_date, pax)

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

            out_price = outbound.get("nonstop_cheapest_usd")
            in_price = inbound.get("nonstop_cheapest_usd")
            if out_price and in_price:
                combined = out_price + in_price
                entry["combined_cheapest_usd"] = combined
                entry["combined_cheapest_twd"] = round(combined * args.exchange_rate)

            results.append(entry)

        await browser.close()

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
        out_str = f"US${out['nonstop_cheapest_usd']}" if out["nonstop_cheapest_usd"] else "N/A"
        in_str = f"US${inb['nonstop_cheapest_usd']}" if inb["nonstop_cheapest_usd"] else "N/A"
        combined_str = f"US${r['combined_cheapest_usd']}" if r["combined_cheapest_usd"] else "N/A"
        twd_str = f"TWD {r['combined_cheapest_twd']:,}" if r["combined_cheapest_twd"] else "N/A"
        marker = " 🏆" if i == 0 and r["combined_cheapest_usd"] else ""
        print(
            f"| {r['depart_date']} ({r['depart_day']}) | {r['return_date']} ({r['return_day']}) "
            f"| {out_str:>9} | {in_str:>9} | {combined_str:>7} | {twd_str:>10}{marker} |"
        )

    print()
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
    parser.add_argument("--exchange-rate", type=float, default=32.0, help="USD to TWD exchange rate (default: 32)")
    parser.add_argument("--output", "-o", help="Output JSON file path")
    args = parser.parse_args()

    depart_dates = date_range(args.depart_start, args.depart_end)
    print(f"Scraping {len(depart_dates)} departure dates: {', '.join(depart_dates)}")
    print(f"Route: {args.origin.upper()} → {args.dest.upper()}, {args.duration} days, {args.pax} pax")
    print(f"Exchange rate: USD 1 = TWD {args.exchange_rate}")

    results = asyncio.run(scrape_date_range(
        depart_dates=depart_dates,
        origin=args.origin.lower(),
        dest=args.dest.lower(),
        duration=args.duration,
        pax=args.pax,
        args=args,
    ))

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

    print_summary(results, args.pax)


if __name__ == "__main__":
    main()
