#!/usr/bin/env python3
"""
Batch Scraper - Scrape multiple OTAs in one command

Usage:
    python scripts/scrape_batch.py --dest kansai
    python scripts/scrape_batch.py --dest osaka --sources besttour,settour
    python scripts/scrape_batch.py --dest tokyo --date 2026-02-24 --type fit
    npm run scraper:batch -- --dest kansai

Options:
    --dest          Destination (tokyo, osaka, kansai, hokkaido, etc.)
    --sources       Comma-separated OTA list (default: all supported)
    --date          Departure date for FIT searches (YYYY-MM-DD)
    --days          Trip duration (default: 5)
    --type          Package type filter: fit, group, or all (default: all)
    --output-dir    Output directory (default: scrapes/)
    --parallel      Run scrapers in parallel (default: sequential)
"""

import argparse
import asyncio
import json
import sys
from datetime import datetime
from pathlib import Path

try:
    from playwright.async_api import async_playwright
except ImportError:
    print("❌ Playwright not installed. Run: pip install playwright && playwright install chromium")
    sys.exit(1)

# Import our scraper modules
sys.path.insert(0, str(Path(__file__).parent))
from scrape_listings import scrape_listings, build_listing_url, save_listings

# OTAs that support listing scrapes
LISTING_OTAS = ["besttour", "lifetour", "settour"]
FIT_OTAS = ["liontravel"]


def load_ota_config():
    """Load OTA configuration."""
    config_path = Path(__file__).parent.parent / "data" / "ota-sources.json"
    with open(config_path) as f:
        return json.load(f)["sources"]


async def scrape_single_ota(
    source_id: str,
    destination: str,
    depart_date: str,
    days: int,
    output_dir: Path,
) -> dict:
    """Scrape a single OTA and return results."""
    result = {
        "source_id": source_id,
        "status": "pending",
        "count": 0,
        "output_file": None,
        "error": None,
    }

    try:
        # Build URL
        url = build_listing_url(source_id, destination, depart_date, days)
        print(f"\n📦 {source_id}: {url[:80]}...")

        # Scrape
        listings = await scrape_listings(source_id, url, depart_date, max_results=100)

        if listings:
            # Save results
            output_file = output_dir / f"{source_id}-{destination}-listings.json"
            save_listings(listings, str(output_file))
            result["status"] = "ok"
            result["count"] = len(listings)
            result["output_file"] = str(output_file)
        else:
            result["status"] = "empty"
            result["error"] = "No packages found"

    except ValueError as e:
        result["status"] = "skipped"
        result["error"] = str(e)
    except Exception as e:
        result["status"] = "error"
        result["error"] = str(e)[:200]

    return result


async def run_batch(args):
    """Run batch scrape for all specified OTAs."""
    start_time = datetime.now()

    print("\n" + "=" * 60)
    print(f"🚀 Batch Scraper - {args.dest.upper()}")
    print("=" * 60)

    # Determine which OTAs to scrape
    if args.sources:
        sources = [s.strip() for s in args.sources.split(",")]
    else:
        # Default: all listing OTAs + FIT if date provided
        sources = LISTING_OTAS.copy()
        if args.date:
            sources.extend(FIT_OTAS)

    # Filter by type
    if args.type == "fit":
        sources = [s for s in sources if s in FIT_OTAS]
    elif args.type == "group":
        sources = [s for s in sources if s in LISTING_OTAS]

    print(f"\nSources: {', '.join(sources)}")
    print(f"Destination: {args.dest}")
    if args.date:
        print(f"Date: {args.date} ({args.days} days)")
    print(f"Output: {args.output_dir}/")

    # Create output directory
    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    # Run scrapers
    results = []

    if args.parallel:
        # Parallel execution
        print("\n⚡ Running in parallel mode...")
        tasks = [
            scrape_single_ota(source, args.dest, args.date or "", args.days, output_dir)
            for source in sources
        ]
        results = await asyncio.gather(*tasks, return_exceptions=True)
        # Convert exceptions to error results
        for i, r in enumerate(results):
            if isinstance(r, Exception):
                results[i] = {
                    "source_id": sources[i],
                    "status": "error",
                    "count": 0,
                    "error": str(r),
                }
    else:
        # Sequential execution (default - gentler on OTAs)
        for source in sources:
            result = await scrape_single_ota(
                source, args.dest, args.date or "", args.days, output_dir
            )
            results.append(result)

    # Summary
    elapsed = (datetime.now() - start_time).total_seconds()

    print("\n" + "=" * 60)
    print("📊 Results Summary")
    print("=" * 60)

    total_packages = 0
    for r in results:
        if r["status"] == "ok":
            print(f"  ✅ {r['source_id']}: {r['count']} packages")
            total_packages += r["count"]
        elif r["status"] == "empty":
            print(f"  ⚠️ {r['source_id']}: No packages found")
        elif r["status"] == "skipped":
            print(f"  ⏭️ {r['source_id']}: {r['error']}")
        else:
            print(f"  ❌ {r['source_id']}: {r['error']}")

    print(f"\nTotal: {total_packages} packages from {len(results)} sources")
    print(f"Time: {elapsed:.1f}s")

    # Save batch summary
    summary = {
        "timestamp": datetime.now().isoformat(),
        "destination": args.dest,
        "date": args.date,
        "days": args.days,
        "elapsed_seconds": elapsed,
        "total_packages": total_packages,
        "results": results,
    }
    summary_file = output_dir / f"batch-{args.dest}-{datetime.now().strftime('%Y%m%d-%H%M%S')}.json"
    with open(summary_file, "w") as f:
        json.dump(summary, f, indent=2, ensure_ascii=False)
    print(f"\nSummary saved: {summary_file}")

    return results


def main():
    parser = argparse.ArgumentParser(description="Batch scrape multiple OTAs")
    parser.add_argument("--dest", required=True, help="Destination (tokyo, osaka, kansai, etc.)")
    parser.add_argument("--sources", help="Comma-separated OTA list (default: all)")
    parser.add_argument("--date", help="Departure date YYYY-MM-DD (required for FIT)")
    parser.add_argument("--days", type=int, default=5, help="Trip duration (default: 5)")
    parser.add_argument("--type", choices=["fit", "group", "all"], default="all", help="Package type")
    parser.add_argument("--output-dir", default="scrapes", help="Output directory")
    parser.add_argument("--parallel", action="store_true", help="Run in parallel")
    args = parser.parse_args()

    asyncio.run(run_batch(args))


if __name__ == "__main__":
    main()
