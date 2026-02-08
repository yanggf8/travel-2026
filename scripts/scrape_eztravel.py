#!/usr/bin/env python3
"""
EzTravel FIT Package Scraper

Scrapes FIT (機加酒) packages from packages.eztravel.com.tw.

Usage:
    python scripts/scrape_eztravel.py --dest OSA --checkin 2026-02-24 --checkout 2026-02-28 --pax 2 -o data/eztravel-osaka.json
    python scripts/scrape_eztravel.py --url "https://packages.eztravel.com.tw/roundtrip-TPE-OSA" -o data/eztravel.json
"""

import argparse
import asyncio
import json
import re
from datetime import datetime
from pathlib import Path

from playwright.async_api import async_playwright


async def scrape_eztravel_fit(
    dest_code: str = "OSA",
    checkin: str = None,
    checkout: str = None,
    pax: int = 2,
    url: str = None,
    output_path: str = None,
) -> dict:
    """Scrape EzTravel FIT packages."""

    if url is None:
        params = f"adult={pax}&child=0&infant=0"
        if checkin and checkout:
            params += f"&checkin={checkin}&checkout={checkout}"
        url = f"https://packages.eztravel.com.tw/roundtrip-TPE-{dest_code}?{params}"

    print(f"Scraping: {url}")

    result = {
        "source_id": "eztravel",
        "package_type": "fit",
        "url": url,
        "scraped_at": datetime.now().isoformat(),
        "destination": dest_code,
        "pax": pax,
        "offers": [],
    }

    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        context = await browser.new_context(
            viewport={"width": 1920, "height": 1080},
            user_agent=(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) "
                "AppleWebKit/537.36 (KHTML, like Gecko) "
                "Chrome/120.0.0.0 Safari/537.36"
            ),
        )
        page = await context.new_page()

        try:
            await page.goto(url, timeout=60000)
            await page.wait_for_timeout(5000)

            # Scroll to load content
            for _ in range(3):
                await page.evaluate("window.scrollTo(0, document.body.scrollHeight)")
                await page.wait_for_timeout(1000)

            result["title"] = await page.title()
            result["final_url"] = page.url

            # Get raw text
            raw_text = await page.evaluate("() => document.body.innerText")
            result["raw_text"] = raw_text[:15000]

            # Check baggage
            if "無免費託運行李" in raw_text or "不含行李" in raw_text:
                result["baggage_included"] = False
            elif "託運行李" in raw_text and "公斤" in raw_text:
                result["baggage_included"] = True

            # Extract flight info
            flight_match = re.search(
                r'(\d+)月(\d+)日.*?(\d{2}:\d{2}).*?TPE.*?(\d{2}:\d{2}).*?(KIX|NRT|NGO|CTS|OKA|FUK)',
                raw_text, re.DOTALL
            )
            if flight_match:
                result["flight_info"] = {
                    "departure_time": flight_match.group(3),
                    "arrival_time": flight_match.group(4),
                    "dest_airport": flight_match.group(5),
                }

            # Extract airline
            airlines = {
                "泰越捷": "Thai Vietjet",
                "泰國獅子": "Thai Lion Air",
                "台灣虎航": "Tigerair Taiwan",
                "樂桃": "Peach",
                "捷星": "Jetstar",
                "長榮": "EVA Air",
                "華航": "China Airlines",
                "星宇": "Starlux",
                "酷航": "Scoot",
            }
            for cn, en in airlines.items():
                if cn in raw_text:
                    result["airline"] = en
                    break

            # Extract hotel offers
            hotel_pattern = re.compile(
                r'([^\n]{5,50}(?:酒店|飯店|旅館|Hotel|Inn|Hostel)[^\n]*)\n'
                r'[^\n]*\n'
                r'[^\n]*(\d+\.?\d?)/5[^\n]*\n'
                r'[^\n]*\n'
                r'[^\n]*(?:不含早餐|含早餐)[^\n]*\n'
                r'[^\n]*\n'
                r'[^\n]*TWD\s*([\d,]+)',
                re.MULTILINE
            )

            for match in hotel_pattern.finditer(raw_text):
                hotel_name = match.group(1).strip()
                rating = float(match.group(2))
                price = int(match.group(3).replace(',', ''))

                result["offers"].append({
                    "hotel": hotel_name,
                    "rating": rating,
                    "price_per_person": price,
                    "price_total": price * pax,
                    "currency": "TWD",
                })

            # If no structured offers, extract prices
            if not result["offers"]:
                prices = re.findall(r'TWD\s*([\d,]+)', raw_text)
                valid_prices = [
                    int(p.replace(',', ''))
                    for p in prices
                    if 10000 < int(p.replace(',', '')) < 100000
                ]
                if valid_prices:
                    result["price_range"] = {
                        "min": min(valid_prices),
                        "max": max(valid_prices),
                        "currency": "TWD",
                    }

            print(f"Title: {result['title']}")
            print(f"Baggage: {'NOT included' if result.get('baggage_included') == False else 'included' if result.get('baggage_included') else 'unknown'}")
            print(f"Airline: {result.get('airline', 'N/A')}")
            print(f"Offers found: {len(result['offers'])}")

            if result["offers"]:
                min_offer = min(result["offers"], key=lambda x: x["price_per_person"])
                print(f"Cheapest: {min_offer['hotel'][:30]} @ TWD {min_offer['price_per_person']:,}/person")

        except Exception as e:
            result["error"] = str(e)
            print(f"Error: {e}")

        await browser.close()

    # Save result
    if output_path:
        output = Path(output_path)
        output.parent.mkdir(parents=True, exist_ok=True)
        with open(output, "w", encoding="utf-8") as f:
            json.dump(result, f, ensure_ascii=False, indent=2)
        print(f"\nSaved to: {output}")

    return result


def main():
    parser = argparse.ArgumentParser(description="Scrape EzTravel FIT packages")
    parser.add_argument("--dest", default="OSA", help="Destination code (OSA, TYO, KIX, etc.)")
    parser.add_argument("--checkin", help="Check-in date (YYYY-MM-DD)")
    parser.add_argument("--checkout", help="Check-out date (YYYY-MM-DD)")
    parser.add_argument("--pax", type=int, default=2, help="Number of adults")
    parser.add_argument("--url", help="Direct URL to scrape (overrides other options)")
    parser.add_argument("-o", "--output", help="Output JSON file path")

    args = parser.parse_args()

    asyncio.run(scrape_eztravel_fit(
        dest_code=args.dest,
        checkin=args.checkin,
        checkout=args.checkout,
        pax=args.pax,
        url=args.url,
        output_path=args.output,
    ))


if __name__ == "__main__":
    main()
