#!/usr/bin/env python3
"""
Filter Packages - Filter scraped packages by type, date, and price

Reads scraped package data and filters by criteria.

Usage:
    python scripts/filter_packages.py <input.json> [options]
    python scripts/filter_packages.py data/*.json --type fit --date 2026-02-24 --max-price 25000

Options:
    --type fit|group|flight|hotel    Filter by package type
    --date YYYY-MM-DD                Filter by departure date
    --min-price N                    Minimum price (TWD)
    --max-price N                    Maximum price (TWD)
    --source SOURCE_ID               Filter by OTA source
    -o, --output FILE                Output filtered results to JSON
    --refresh-stale                  Show refresh command for stale data

Examples:
    # Find FIT packages under 25k
    python scripts/filter_packages.py data/*-scrape.json --type fit --max-price 25000
    
    # Find packages departing Feb 24
    python scripts/filter_packages.py data/*-scrape.json --date 2026-02-24
    
    # Combined filters
    python scripts/filter_packages.py data/*-scrape.json --type fit --date 2026-02-24 --max-price 25000
"""

import argparse
import json
import sys
from datetime import datetime, timedelta
from pathlib import Path


def _classify_package_type_from_title(title: str) -> str:
    """Lightweight package type classification from title keywords."""
    title_lower = title.lower()

    # Group indicators (check first to avoid false FIT positives)
    group_keywords = ["團體", "跟團", "精緻團", "品質團", "領隊", "導遊"]
    if any(kw in title_lower for kw in group_keywords):
        return "group"

    # Phrases common in group tours that mention free time
    group_free_time_phrases = ["自由活動", "自由時間", "自由選購", "自由行程"]
    if any(kw in title_lower for kw in group_free_time_phrases):
        return "group"

    # FIT indicators
    fit_keywords = ["自由行", "機加酒", "自助", "半自由", "伴自由", "自由配", "fit"]
    if any(kw in title_lower for kw in fit_keywords):
        return "fit"

    return "unknown"


def load_scrape_result(file_path: str) -> dict | list[dict]:
    """Load a scrape result JSON file or listing file."""
    with open(file_path, encoding="utf-8") as f:
        data = json.load(f)
    
    # Check if it's a listing file format
    if isinstance(data, dict) and "listings" in data:
        # Propagate top-level scraped_at to each listing entry
        scraped_at = data.get("scraped_at", "")
        listings = data["listings"]
        for listing in listings:
            if "scraped_at" not in listing:
                listing["scraped_at"] = scraped_at
            # Add lightweight package_type classification if missing
            if "package_type" not in listing or not listing["package_type"]:
                listing["package_type"] = _classify_package_type_from_title(listing.get("title", ""))
        return listings
    
    # Regular scrape result
    return [data] if isinstance(data, dict) else data


def get_cache_age_hours(scraped_at: str) -> float:
    """Calculate cache age in hours."""
    try:
        scraped_time = datetime.fromisoformat(scraped_at.replace("Z", "+00:00"))
        age = datetime.now() - scraped_time
        return age.total_seconds() / 3600
    except Exception:
        return 0


def is_stale(scraped_at: str, threshold_hours: int = 24) -> bool:
    """Check if data is stale (older than threshold)."""
    return get_cache_age_hours(scraped_at) > threshold_hours


def format_age(hours: float) -> str:
    """Format age as human-readable string."""
    if hours < 1:
        return f"{int(hours * 60)}m"
    elif hours < 24:
        return f"{int(hours)}h"
    else:
        return f"{int(hours / 24)}d"


def extract_package_data(data: dict) -> dict:
    """Extract package data from scrape result."""
    extracted = data.get("extracted", {})
    
    # Get package type (new field, may be at top level or missing)
    package_type = data.get("package_type", "unknown")
    
    # Get departure date - try multiple sources
    dates = extracted.get("dates", {})
    departure_date = dates.get("departure_date", "")
    
    # Fallback: check date_pricing for BestTour-style data
    if not departure_date:
        date_pricing = extracted.get("date_pricing", {})
        if date_pricing:
            # Get earliest date from date_pricing
            sorted_dates = sorted(date_pricing.keys())
            if sorted_dates:
                departure_date = sorted_dates[0]
    
    # Get price
    price_data = extracted.get("price", {})
    price = price_data.get("per_person")
    currency = price_data.get("currency", "TWD")
    
    # Get source
    source_id = data.get("source_id", data.get("source", "unknown"))
    
    return {
        "url": data.get("url", ""),
        "title": data.get("title", ""),
        "source_id": source_id,
        "package_type": package_type,
        "departure_date": departure_date,
        "price": price,
        "currency": currency,
        "scraped_at": data.get("scraped_at", ""),
        "raw_data": data,
    }


def matches_filters(pkg: dict, args: argparse.Namespace) -> bool:
    """Check if package matches all filters."""
    
    # Type filter
    if args.type and pkg["package_type"] != args.type:
        return False
    
    # Date filter
    if args.date and pkg["departure_date"] != args.date:
        return False
    
    # Price filters
    if pkg["price"]:
        if args.min_price and pkg["price"] < args.min_price:
            return False
        if args.max_price and pkg["price"] > args.max_price:
            return False
    elif args.min_price or args.max_price:
        # No price data but price filter specified
        return False
    
    # Source filter
    if args.source and pkg["source_id"] != args.source:
        return False
    
    return True


def main():
    parser = argparse.ArgumentParser(description="Filter scraped packages")
    parser.add_argument("files", nargs="+", help="Scrape result JSON files")
    parser.add_argument("--type", choices=["fit", "group", "flight", "hotel"], help="Package type")
    parser.add_argument("--date", help="Departure date YYYY-MM-DD")
    parser.add_argument("--min-price", type=int, help="Minimum price (TWD)")
    parser.add_argument("--max-price", type=int, help="Maximum price (TWD)")
    parser.add_argument("--source", help="OTA source ID")
    parser.add_argument("-o", "--output", help="Output JSON file")
    parser.add_argument("--refresh-stale", action="store_true", help="Show refresh commands for stale data")
    args = parser.parse_args()
    
    # Load all packages
    all_packages = []
    stale_files = []
    
    for file_path in args.files:
        try:
            results = load_scrape_result(file_path)
            
            # Handle both single result and list of results
            if not isinstance(results, list):
                results = [results]
            
            for data in results:
                # Skip if it's a listing entry (minimal format)
                if "source_id" in data and "url" in data and "title" in data:
                    # Listing format - convert to scrape result format
                    pkg_data = {
                        "source_id": data.get("source_id", "unknown"),
                        "package_type": data.get("package_type", "unknown"),
                        "url": data.get("url", ""),
                        "title": data.get("title", ""),
                        "scraped_at": data.get("scraped_at", ""),
                        "extracted": {
                            "dates": {"departure_date": data.get("date", "")},
                            "price": {"per_person": data.get("price"), "currency": data.get("currency", "TWD")},
                        }
                    }
                    data = pkg_data
                
                pkg = extract_package_data(data)
                pkg["file_path"] = file_path
                all_packages.append(pkg)
                
                # Check staleness
                if pkg["scraped_at"] and is_stale(pkg["scraped_at"]):
                    stale_files.append((file_path, pkg["scraped_at"]))
        except Exception as e:
            print(f"⚠️  Error loading {file_path}: {e}", file=sys.stderr)
    
    # Filter packages
    filtered = [pkg for pkg in all_packages if matches_filters(pkg, args)]
    
    # Sort by price (cheapest first)
    filtered.sort(key=lambda p: p["price"] if p["price"] else float("inf"))
    
    # Print results
    print(f"\n{'='*80}")
    print(f"Filtered {len(filtered)} packages from {len(all_packages)} total")
    print(f"{'='*80}\n")
    
    if args.type:
        print(f"Type: {args.type}")
    if args.date:
        print(f"Date: {args.date}")
    if args.min_price or args.max_price:
        price_range = f"{args.min_price or 0:,} - {args.max_price or '∞':,} TWD"
        print(f"Price: {price_range}")
    if args.source:
        print(f"Source: {args.source}")
    print()
    
    # Show results
    for i, pkg in enumerate(filtered[:20], 1):
        price_str = f"{pkg['currency']} {pkg['price']:,}" if pkg['price'] else "Price N/A"
        age = format_age(get_cache_age_hours(pkg['scraped_at']))
        type_badge = f"[{pkg['package_type']}]" if pkg['package_type'] != "unknown" else ""
        
        print(f"{i:2d}. {price_str:15s} {type_badge:8s} {pkg['title'][:50]}")
        if pkg['departure_date']:
            print(f"    📅 {pkg['departure_date']} | 🕐 {age} old | {pkg['source_id']}")
        else:
            print(f"    🕐 {age} old | {pkg['source_id']}")
        print()
    
    if len(filtered) > 20:
        print(f"... and {len(filtered) - 20} more results\n")
    
    # Freshness warnings
    if stale_files and args.refresh_stale:
        print(f"\n{'='*80}")
        print(f"⚠️  {len(stale_files)} files have stale data (>24h old)")
        print(f"{'='*80}\n")
        print("Refresh commands:\n")
        
        for file_path, scraped_at in stale_files[:10]:
            age = format_age(get_cache_age_hours(scraped_at))
            # Try to extract URL from file
            try:
                data = load_scrape_result(file_path)
                url = data.get("url", "")
                if url:
                    print(f"# {age} old")
                    print(f"python scripts/scrape_package.py '{url}' {file_path} --refresh\n")
            except Exception:
                pass
    
    # Save to file
    if args.output:
        output_data = {
            "filtered_at": datetime.now().isoformat(),
            "filters": {
                "type": args.type,
                "date": args.date,
                "min_price": args.min_price,
                "max_price": args.max_price,
                "source": args.source,
            },
            "total_input": len(all_packages),
            "total_filtered": len(filtered),
            "packages": [
                {
                    "url": p["url"],
                    "title": p["title"],
                    "source_id": p["source_id"],
                    "package_type": p["package_type"],
                    "departure_date": p["departure_date"],
                    "price": p["price"],
                    "currency": p["currency"],
                    "scraped_at": p["scraped_at"],
                }
                for p in filtered
            ],
        }
        
        with open(args.output, "w", encoding="utf-8") as f:
            json.dump(output_data, f, ensure_ascii=False, indent=2)
        
        print(f"Saved {len(filtered)} filtered packages to {args.output}")


if __name__ == "__main__":
    main()
