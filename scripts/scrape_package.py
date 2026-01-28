#!/usr/bin/env python3
"""
Generic OTA Package Scraper

Scrape travel package details from various OTA websites using Playwright.
Supports JavaScript-rendered pages and extracts raw text + structured elements.

Supported OTAs:
- BestTour (besttour.com.tw) - Full calendar pricing
- Lion Travel (liontravel.com) - Package search results
- ezTravel (eztravel.com.tw) - Limited support
- Any URL with standard page structure

Usage:
    python scrape_package.py <url> [output.json]

Examples:
    python scrape_package.py "https://www.besttour.com.tw/itinerary/TYO05MM260211AM" data/besttour.json
    python scrape_package.py "https://vacation.liontravel.com/search?Destination=JP_TYO_6" data/liontravel.json

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


async def scrape_package(url: str) -> dict:
    """Scrape package details from the given URL."""

    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        page = await browser.new_page()

        print(f"Fetching: {url}")
        try:
            await page.goto(url, wait_until="networkidle", timeout=60000)
        except Exception as e:
            print(f"Networkidle timeout, trying domcontentloaded: {e}")
            await page.goto(url, wait_until="domcontentloaded", timeout=60000)

        # Wait for content to load
        await page.wait_for_timeout(3000)

        # Scroll down to load lazy content
        print("Scrolling to load full page content...")
        await page.evaluate("window.scrollTo(0, document.body.scrollHeight)")
        await page.wait_for_timeout(2000)
        # Scroll in steps to trigger lazy loading
        for i in range(5):
            await page.evaluate(f"window.scrollTo(0, {(i+1) * 1000})")
            await page.wait_for_timeout(500)
        # Scroll to bottom again
        await page.evaluate("window.scrollTo(0, document.body.scrollHeight)")
        await page.wait_for_timeout(2000)

        # BestTour specific: click 交通方式 tab to load flight details
        if "besttour.com.tw" in url:
            print("BestTour detected: clicking 交通方式 tab...")
            try:
                # Try multiple selectors for the transportation tab
                tab_selectors = [
                    "text=交通方式",
                    "text=交通",
                    "[class*='tab']:has-text('交通')",
                    "button:has-text('交通')",
                    "a:has-text('交通')",
                    "div:has-text('交通方式')",
                ]
                for selector in tab_selectors:
                    try:
                        tab = await page.query_selector(selector)
                        if tab:
                            await tab.click()
                            print(f"  Clicked tab with selector: {selector}")
                            await page.wait_for_timeout(2000)
                            break
                    except Exception:
                        continue
            except Exception as e:
                print(f"  Could not click 交通方式 tab: {e}")

        # Extract page content
        content = await page.content()

        # Try to extract structured data
        result = {
            "url": url,
            "scraped_at": datetime.now().isoformat(),
            "title": await page.title(),
            "raw_text": "",
            "extracted": {
                "flight": {},
                "hotel": {},
                "price": {},
                "dates": {},
                "inclusions": []
            }
        }

        # Get all visible text
        result["raw_text"] = await page.evaluate("() => document.body.innerText")

        # Try to find specific elements (adjust selectors based on actual page structure)
        selectors_to_try = [
            # Common patterns for travel sites
            (".price", "price_element"),
            (".itinerary", "itinerary_element"),
            (".flight-info", "flight_element"),
            (".hotel-info", "hotel_element"),
            ("[class*='price']", "price_class"),
            ("[class*='flight']", "flight_class"),
            ("[class*='hotel']", "hotel_class"),
            # Table data
            ("table", "tables"),
            # Generic content areas
            (".content", "content"),
            ("main", "main"),
            ("#content", "content_id"),
        ]

        extracted_elements = {}
        for selector, name in selectors_to_try:
            try:
                elements = await page.query_selector_all(selector)
                if elements:
                    texts = []
                    for el in elements[:5]:  # Limit to first 5
                        text = await el.inner_text()
                        if text.strip():
                            texts.append(text.strip())
                    if texts:
                        extracted_elements[name] = texts
            except Exception:
                pass

        result["extracted_elements"] = extracted_elements

        # BestTour specific: parse 交通方式 section for flight details
        if "besttour.com.tw" in url:
            result["extracted"]["flight"] = parse_besttour_flights(result["raw_text"])

        await browser.close()

        return result


def parse_besttour_flights(raw_text: str) -> dict:
    """Parse BestTour 交通方式 section for flight details."""
    import re

    flight_info = {"outbound": {}, "return": {}}

    # Pattern: 去程\n日期\n航班\n航空\n機場(CODE)\n時間\n→\n機場(CODE)\n時間
    lines = raw_text.split('\n')

    for i, line in enumerate(lines):
        line = line.strip()

        if line == '去程' and i + 8 < len(lines):
            flight_info["outbound"] = {
                "date": lines[i+1].strip(),
                "flight_number": lines[i+2].strip(),
                "airline": lines[i+3].strip(),
                "departure_airport": lines[i+4].strip(),
                "departure_time": lines[i+5].strip(),
                "arrival_airport": lines[i+7].strip(),  # skip → at i+6
                "arrival_time": lines[i+8].strip(),
            }
            # Extract airport codes
            dep_match = re.search(r'\(([A-Z]{3})\)', flight_info["outbound"]["departure_airport"])
            arr_match = re.search(r'\(([A-Z]{3})\)', flight_info["outbound"]["arrival_airport"])
            if dep_match:
                flight_info["outbound"]["departure_code"] = dep_match.group(1)
            if arr_match:
                flight_info["outbound"]["arrival_code"] = arr_match.group(1)

        elif line == '回程' and i + 8 < len(lines):
            flight_info["return"] = {
                "date": lines[i+1].strip(),
                "flight_number": lines[i+2].strip(),
                "airline": lines[i+3].strip(),
                "departure_airport": lines[i+4].strip(),
                "departure_time": lines[i+5].strip(),
                "arrival_airport": lines[i+7].strip(),  # skip → at i+6
                "arrival_time": lines[i+8].strip(),
            }
            # Extract airport codes
            dep_match = re.search(r'\(([A-Z]{3})\)', flight_info["return"]["departure_airport"])
            arr_match = re.search(r'\(([A-Z]{3})\)', flight_info["return"]["arrival_airport"])
            if dep_match:
                flight_info["return"]["departure_code"] = dep_match.group(1)
            if arr_match:
                flight_info["return"]["arrival_code"] = arr_match.group(1)
            break  # Found both, done

    return flight_info


def save_result(result: dict, output_path: str):
    """Save the scraped result to a JSON file."""
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)
    print(f"Saved to: {output_path}")


async def main():
    url = sys.argv[1] if len(sys.argv) > 1 else "https://www.besttour.com.tw/itinerary/TYO05MM260211AM"
    output = sys.argv[2] if len(sys.argv) > 2 else "data/package-scrape-result.json"

    result = await scrape_package(url)

    # Print summary
    print("\n" + "="*60)
    print(f"Title: {result['title']}")
    print("="*60)
    print("\nRaw Text (first 2000 chars):")
    print("-"*60)
    print(result["raw_text"][:2000])
    print("-"*60)

    if result.get("extracted_elements"):
        print("\nExtracted Elements:")
        for name, texts in result["extracted_elements"].items():
            print(f"\n[{name}]")
            for t in texts[:3]:
                print(f"  - {t[:200]}...")

    save_result(result, output)


if __name__ == "__main__":
    asyncio.run(main())
