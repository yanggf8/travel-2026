#!/usr/bin/env python3
"""
Scrape travel package details from besttour.com.tw
Uses Playwright for JavaScript rendering
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
        await page.wait_for_timeout(5000)

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

        await browser.close()

        return result


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
