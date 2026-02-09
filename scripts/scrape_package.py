#!/usr/bin/env python3
"""
Generic OTA Package Scraper

Scrape travel package details from various OTA websites using Playwright.
Auto-detects OTA from URL and delegates to the appropriate parser module.
**Results are written directly to Turso DB** (primary storage).

Supported OTAs:
- BestTour (besttour.com.tw) - Full calendar pricing
- Lion Travel (liontravel.com) - Package search results
- Lifetour (tour.lifetour.com.tw) - Package with itinerary
- Settour (tour.settour.com.tw) - Package with itinerary
- Travel4U (travel4u.com.tw) - Group tours
- Trip.com (trip.com) - Flight search
- Any URL with standard page structure

Usage:
    python scrape_package.py <url> [--refresh] [--quiet] [--json] [--no-db]

Options:
    --refresh   Bypass cache and force fresh scrape
    --quiet     Suppress output
    --json      Also save result to JSON file (for debugging)
    --no-db     Skip Turso DB import (JSON-only mode)

Examples:
    # Scrape and import to Turso (default)
    python scrape_package.py "https://www.travel4u.com.tw/group/product/SPK05260223D/"
    
    # Also save JSON for debugging
    python scrape_package.py "https://www.besttour.com.tw/itinerary/TYO05MM260211AM" --json
    
    # JSON only, no DB import
    python scrape_package.py "https://vacation.liontravel.com/search?..." output.json --no-db

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

from scrapers import detect_ota, get_parser, create_browser
from scrapers.base import (
    navigate_with_retry, scroll_page, safe_extract_text,
    extract_generic_elements, extract_package_links,
)
from scrapers.schema import ScrapeResult, validate_result


async def scrape_package(url: str, use_cache: bool = True) -> dict:
    """Scrape package details from the given URL."""
    source_id = detect_ota(url)

    async with async_playwright() as p:
        browser, context, page = await create_browser(p)

        try:
            if source_id:
                # Use the dedicated parser
                parser = get_parser(source_id)
                result = await parser.scrape(page, url, use_cache=use_cache)
            else:
                # Generic scrape for unknown URLs
                result = await _generic_scrape(page, url)

            # Validate and attach warnings
            warnings = validate_result(result)
            result.warnings.extend(warnings)

        finally:
            await browser.close()

    # Return legacy dict format for backward compatibility
    return result.to_legacy_dict()


async def _generic_scrape(page, url: str) -> ScrapeResult:
    """Generic scraper for URLs that don't match any known OTA."""
    result = ScrapeResult(
        source_id="generic",
        url=url,
        scraped_at=datetime.now().isoformat(),
    )

    success = await navigate_with_retry(page, url)
    if not success:
        result.success = False
        result.errors.append(f"Failed to navigate to {url}")
        return result

    await page.wait_for_timeout(3000)
    await scroll_page(page)

    try:
        result.title = await page.title()
    except Exception:
        pass

    result.raw_text = await safe_extract_text(page)
    result.extracted_elements = await extract_generic_elements(page)
    result.package_links = await extract_package_links(page, url)

    return result


def save_result(result: dict, output_path: str):
    """Save the scraped result to a JSON file."""
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)
    print(f"Saved to: {output_path}")


async def main():
    quiet = "--quiet" in sys.argv
    refresh = "--refresh" in sys.argv
    save_json = "--json" in sys.argv
    no_db = "--no-db" in sys.argv
    argv = [a for a in sys.argv[1:] if a not in ("--quiet", "--refresh", "--json", "--no-db")]

    url = argv[0] if len(argv) > 0 else "https://www.besttour.com.tw/itinerary/TYO05MM260211AM"
    output = argv[1] if len(argv) > 1 else None

    if refresh:
        print("🔄 Refresh mode: bypassing cache")
    
    result = await scrape_package(url, use_cache=not refresh)

    if not quiet:
        print("\n" + "=" * 60)
        print(f"Title: {result['title']}")
        print("=" * 60)
        print("\nRaw Text (first 2000 chars):")
        print("-" * 60)
        print(result["raw_text"][:2000])
        print("-" * 60)

        if result.get("extracted_elements"):
            print("\nExtracted Elements:")
            for name, texts in result["extracted_elements"].items():
                print(f"\n[{name}]")
                for t in texts[:3]:
                    print(f"  - {t[:200]}...")

    # Write to Turso DB (primary storage) unless --no-db
    if not no_db:
        import_to_turso(result, url)
    
    # Save JSON only if --json flag or explicit output path given
    if save_json or output:
        output_path = output or f"scrapes/{result.get('source_id', 'unknown')}-{datetime.now().strftime('%Y%m%d-%H%M%S')}.json"
        save_result(result, output_path)


def import_to_turso(result: dict, url: str):
    """Import scraped result directly to Turso DB."""
    import subprocess
    import tempfile
    import os
    
    # Write temp file for the importer
    with tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False, encoding='utf-8') as f:
        json.dump(result, f, ensure_ascii=False, indent=2)
        temp_path = f.name
    
    try:
        # Call the TypeScript importer
        cmd = [
            'npx', 'ts-node', 'scripts/import-offers-to-turso.ts',
            '--files', temp_path
        ]
        
        # Infer region from URL
        url_lower = url.lower()
        region = None
        
        # Travel4U uses area codes in URL for listing pages
        # Product codes: SPK=Sapporo/Hokkaido, OSA=Osaka, TYO=Tokyo, FUK=Fukuoka, OKA=Okinawa
        if 'travel4u.com.tw' in url_lower:
            if '/area/39/' in url_lower:
                region = 'hokkaido'
            elif '/area/40/' in url_lower:
                region = 'kansai'
            elif '/area/41/' in url_lower:
                region = 'tokyo'
            elif '/area/42/' in url_lower:
                region = 'kyushu'
            elif '/area/43/' in url_lower:
                region = 'okinawa'
            elif '/product/' in url_lower:
                # Extract product code from URL like /product/SPK05260223D/
                import re
                prod_match = re.search(r'/product/([A-Z]{3})', url)
                if prod_match:
                    prefix = prod_match.group(1).upper()
                    region_map = {
                        'SPK': 'hokkaido',  # Sapporo
                        'CTS': 'hokkaido',  # Chitose
                        'OSA': 'kansai',
                        'KIX': 'kansai',
                        'TYO': 'tokyo',
                        'NRT': 'tokyo',
                        'HND': 'tokyo',
                        'FUK': 'kyushu',
                        'OKA': 'okinawa',
                        'NGO': 'chubu',
                    }
                    region = region_map.get(prefix)
        
        # Generic region inference for other OTAs
        if not region:
            if any(r in url_lower for r in ['kansai', 'osaka', 'kyoto', 'kix', '_osa_']):
                region = 'kansai'
            elif any(r in url_lower for r in ['tokyo', 'nrt', 'hnd', '_tyo_']):
                region = 'tokyo'
            elif any(r in url_lower for r in ['hokkaido', 'cts', 'sapporo']):
                region = 'hokkaido'
            elif any(r in url_lower for r in ['kyushu', 'fuk', 'fukuoka']):
                region = 'kyushu'
            elif any(r in url_lower for r in ['okinawa', 'oka', 'naha']):
                region = 'okinawa'
        
        if region:
            cmd.extend(['--region', region])
        
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
        
        if proc.returncode == 0:
            # Parse output for import count
            output = proc.stdout + proc.stderr
            if 'imported' in output.lower() or 'rows' in output.lower():
                print(f"✅ Imported to Turso DB")
            else:
                print(f"✅ Wrote to Turso DB")
        else:
            # Check if it's just a "no offers" situation
            if 'skipped' in proc.stdout.lower():
                print(f"⚠️  No offers to import (single package scrape)")
            else:
                print(f"❌ Turso import failed: {proc.stderr[:200] if proc.stderr else 'unknown'}")
                sys.exit(1)
    except subprocess.TimeoutExpired:
        print("❌ Turso import timed out")
        sys.exit(1)
    except Exception as e:
        print(f"❌ Turso import error: {e}")
        sys.exit(1)
    finally:
        # Clean up temp file
        try:
            os.unlink(temp_path)
        except:
            pass


if __name__ == "__main__":
    asyncio.run(main())
