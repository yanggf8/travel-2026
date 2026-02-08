#!/usr/bin/env python3
"""
Scraper Doctor - Health check for all OTA scrapers

Verifies:
1. Playwright is installed and working
2. Each supported OTA is reachable
3. Listing scrapers can extract packages
4. Reports any issues found

Usage:
    python scripts/scraper_doctor.py
    npm run scraper:doctor
"""

import asyncio
import json
import sys
from datetime import datetime
from pathlib import Path

# Check Playwright first
try:
    from playwright.async_api import async_playwright
    PLAYWRIGHT_INSTALLED = True
except ImportError:
    PLAYWRIGHT_INSTALLED = False

# Test URLs for each OTA (known-good pages)
TEST_URLS = {
    "besttour": {
        "url": "https://www.besttour.com.tw/e_web/activity?v=japan_kansai",
        "expected_selector": "a[href*='/itinerary/']",
        "min_results": 5,
    },
    "lifetour": {
        "url": "https://tour.lifetour.com.tw/searchlist/tpe/0001-0003",
        "expected_selector": "a[href*='/detail']",
        "min_results": 3,
    },
    "settour": {
        "url": "https://tour.settour.com.tw/search?destinationCode=JX_3",
        "expected_selector": ".product-item",
        "min_results": 5,
    },
    "liontravel": {
        "url": "https://vacation.liontravel.com/search?Destination=JP_OSA_5&FromDate=20260301&ToDate=20260305&Days=5&roomlist=2-0-0",
        "expected_selector": ".product-info, .flight-info",
        "min_results": 0,  # FIT configurator doesn't have listing items
        "notes": "FIT configurator - checks page loads",
    },
}


def print_status(name: str, status: str, message: str = ""):
    """Print formatted status line."""
    icons = {
        "ok": "✅",
        "warn": "⚠️",
        "fail": "❌",
        "skip": "⏭️",
        "info": "ℹ️",
    }
    icon = icons.get(status, "•")
    msg = f" - {message}" if message else ""
    print(f"  {icon} {name}{msg}")


async def check_ota(source_id: str, config: dict, browser) -> dict:
    """Test a single OTA scraper."""
    result = {
        "source_id": source_id,
        "status": "unknown",
        "message": "",
        "elements_found": 0,
        "response_time_ms": 0,
    }

    try:
        context = await browser.new_context(
            viewport={"width": 1280, "height": 720},
            user_agent="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"
        )
        page = await context.new_page()

        start_time = datetime.now()

        # Navigate to test URL
        response = await page.goto(config["url"], timeout=30000, wait_until="domcontentloaded")

        # Wait for content to load
        await page.wait_for_timeout(3000)

        response_time = (datetime.now() - start_time).total_seconds() * 1000
        result["response_time_ms"] = int(response_time)

        # Check response status
        if response and response.status >= 400:
            result["status"] = "fail"
            result["message"] = f"HTTP {response.status}"
            await context.close()
            return result

        # Check for expected elements
        elements = await page.query_selector_all(config["expected_selector"])
        result["elements_found"] = len(elements)

        if len(elements) >= config["min_results"]:
            result["status"] = "ok"
            result["message"] = f"{len(elements)} elements found"
        elif len(elements) > 0:
            result["status"] = "warn"
            result["message"] = f"Only {len(elements)} elements (expected {config['min_results']}+)"
        else:
            # Check if page has any content
            body_text = await page.inner_text("body")
            if len(body_text) > 1000:
                result["status"] = "warn"
                result["message"] = f"Page loaded but selector '{config['expected_selector']}' not found"
            else:
                result["status"] = "fail"
                result["message"] = "Page appears empty or blocked"

        await context.close()

    except Exception as e:
        result["status"] = "fail"
        result["message"] = str(e)[:100]

    return result


async def run_doctor():
    """Run all health checks."""
    print("\n" + "=" * 60)
    print("🩺 Scraper Doctor - Health Check")
    print("=" * 60 + "\n")

    results = {
        "timestamp": datetime.now().isoformat(),
        "playwright_installed": PLAYWRIGHT_INSTALLED,
        "ota_checks": {},
    }

    # 1. Check Playwright
    print("1. Playwright Installation")
    if PLAYWRIGHT_INSTALLED:
        print_status("playwright", "ok", "Installed")
        try:
            async with async_playwright() as p:
                browser = await p.chromium.launch(headless=True)
                await browser.close()
            print_status("chromium", "ok", "Browser launches successfully")
        except Exception as e:
            print_status("chromium", "fail", f"Browser launch failed: {e}")
            print("\n  Run: playwright install chromium")
            results["chromium_ok"] = False
            return results
        results["chromium_ok"] = True
    else:
        print_status("playwright", "fail", "Not installed")
        print("\n  Run: pip install playwright && playwright install chromium")
        return results

    # 2. Check OTA sources config
    print("\n2. OTA Sources Configuration")
    config_path = Path(__file__).parent.parent / "data" / "ota-sources.json"
    if config_path.exists():
        with open(config_path) as f:
            ota_config = json.load(f)
        sources = ota_config.get("sources", {})
        supported = [k for k, v in sources.items() if v.get("supported")]
        print_status("ota-sources.json", "ok", f"{len(supported)} supported OTAs")
    else:
        print_status("ota-sources.json", "fail", "File not found")
        return results

    # 3. Test each OTA
    print("\n3. OTA Connectivity & Scraping")

    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)

        for source_id, config in TEST_URLS.items():
            result = await check_ota(source_id, config, browser)
            results["ota_checks"][source_id] = result

            if result["status"] == "ok":
                print_status(
                    source_id,
                    "ok",
                    f"{result['elements_found']} items, {result['response_time_ms']}ms"
                )
            elif result["status"] == "warn":
                print_status(source_id, "warn", result["message"])
            else:
                print_status(source_id, "fail", result["message"])

        await browser.close()

    # 4. Summary
    print("\n" + "=" * 60)
    ok_count = sum(1 for r in results["ota_checks"].values() if r["status"] == "ok")
    warn_count = sum(1 for r in results["ota_checks"].values() if r["status"] == "warn")
    fail_count = sum(1 for r in results["ota_checks"].values() if r["status"] == "fail")

    total = len(results["ota_checks"])
    print(f"Summary: {ok_count}/{total} OK, {warn_count} warnings, {fail_count} failures")

    if fail_count > 0:
        print("\n⚠️  Some scrapers are not working. Check the errors above.")
        sys.exit(1)
    elif warn_count > 0:
        print("\n⚠️  Some scrapers have warnings. They may still work.")
        sys.exit(0)
    else:
        print("\n✅ All scrapers are healthy!")
        sys.exit(0)


if __name__ == "__main__":
    asyncio.run(run_doctor())
