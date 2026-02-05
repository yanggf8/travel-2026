#!/usr/bin/env python3
"""
Scrape Lion Travel packages with specific date selection.
Uses Playwright to interact with the booking calendar.
"""

import asyncio
import json
import sys
import re
from datetime import datetime, timedelta

try:
    from playwright.async_api import async_playwright
except ImportError:
    print("Playwright not installed. Run: pip install playwright && playwright install chromium")
    sys.exit(1)


async def scrape_liontravel_packages(
    departure_date: str = "2026-02-11",
    return_date: str = "2026-02-15",
    destination: str = "JP_TYO_6",
    adults: int = 2
) -> dict:
    """
    Scrape Lion Travel vacation packages with specific dates.
    
    Args:
        departure_date: Departure date in YYYY-MM-DD format
        return_date: Return date in YYYY-MM-DD format
        destination: Lion Travel destination code (JP_TYO_6 = Tokyo)
        adults: Number of adult passengers
    """
    
    # Format dates for URL
    from_date = departure_date.replace("-", "")
    to_date = return_date.replace("-", "")
    
    # Calculate days
    dep = datetime.strptime(departure_date, "%Y-%m-%d")
    ret = datetime.strptime(return_date, "%Y-%m-%d")
    days = (ret - dep).days + 1
    
    base_url = f"https://vacation.liontravel.com/search"
    params = f"?Destination={destination}&FromDate={from_date}&ToDate={to_date}&Days={days}&roomlist={adults}-0-0"
    url = base_url + params
    
    result = {
        "url": url,
        "scraped_at": datetime.now().isoformat(),
        "search_params": {
            "departure_date": departure_date,
            "return_date": return_date,
            "destination": destination,
            "days": days,
            "adults": adults
        },
        "packages": [],
        "raw_prices": [],
        "errors": []
    }
    
    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        context = await browser.new_context(
            viewport={"width": 1920, "height": 1080},
            user_agent="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"
        )
        page = await context.new_page()
        
        print(f"Fetching: {url}")
        
        try:
            # Navigate with longer timeout
            await page.goto(url, wait_until="domcontentloaded", timeout=90000)
            
            # Wait for the page to load dynamic content
            print("Waiting for search results to load...")
            await page.wait_for_timeout(8000)
            
            # Try to wait for product cards
            try:
                await page.wait_for_selector(".product-card, .search-result-item, [class*='product']", timeout=15000)
            except:
                print("No product cards found with standard selectors, continuing...")
            
            # Get page content for analysis
            content = await page.content()
            text = await page.evaluate("() => document.body.innerText")
            
            # Extract all TWD prices
            price_pattern = r'TWD\s*([\d,]+)'
            prices = re.findall(price_pattern, text)
            result["raw_prices"] = list(set(prices))[:20]  # Dedupe, limit to 20
            
            # Try to extract package information
            # Look for product cards or list items
            package_selectors = [
                ".product-card",
                ".search-result-item", 
                "[class*='product-item']",
                ".vacation-item",
                "article",
                ".card"
            ]
            
            for selector in package_selectors:
                elements = await page.query_selector_all(selector)
                if elements and len(elements) > 0:
                    print(f"Found {len(elements)} elements with selector: {selector}")
                    for i, el in enumerate(elements[:10]):  # Limit to 10
                        try:
                            item_text = await el.inner_text()
                            if "TWD" in item_text or "自由行" in item_text:  # currency match in scraped DOM text
                                # Extract price from this item
                                item_prices = re.findall(price_pattern, item_text)
                                
                                # Try to get title/name
                                title = ""
                                title_el = await el.query_selector("h2, h3, .title, [class*='title'], a")
                                if title_el:
                                    title = await title_el.inner_text()
                                
                                # Try to get link
                                link = ""
                                link_el = await el.query_selector("a[href]")
                                if link_el:
                                    link = await link_el.get_attribute("href")
                                
                                result["packages"].append({
                                    "index": i,
                                    "title": title.strip()[:100] if title else "",
                                    "prices_found": item_prices[:3],
                                    "link": link,
                                    "text_preview": item_text[:300]
                                })
                        except Exception as e:
                            result["errors"].append(f"Error extracting package {i}: {str(e)}")
                    break  # Found working selector, stop
            
            # Also try to find specific price elements
            price_elements = await page.query_selector_all("[class*='price'], .amount, .TWD")
            if price_elements:
                print(f"Found {len(price_elements)} price elements")
                for i, el in enumerate(price_elements[:10]):
                    try:
                        price_text = await el.inner_text()
                        if re.search(r'\d', price_text):
                            result["raw_prices"].append(f"element_{i}: {price_text.strip()}")
                    except:
                        pass
            
            # Get the full text for reference
            result["page_text_sample"] = text[:3000]
            
            # Screenshot for debugging
            await page.screenshot(path="data/liontravel-search-screenshot.png")
            print("Screenshot saved to data/liontravel-search-screenshot.png")
            
        except Exception as e:
            result["errors"].append(f"Page load error: {str(e)}")
            print(f"Error: {e}")
        
        await browser.close()
    
    return result


async def scrape_package_detail(product_id: str, departure_date: str = "2026-02-11", days: int = 5, adults: int = 2) -> dict:
    """
    Scrape a specific package detail page and try to select the date.
    """
    from_date = departure_date.replace("-", "")
    url = f"https://vacation.liontravel.com/detail/{product_id}?FromDate={from_date}&Days={days}&roomlist={adults}-0-0"
    
    result = {
        "url": url,
        "product_id": product_id,
        "scraped_at": datetime.now().isoformat(),
        "departure_date": departure_date,
        "days": days,
        "pricing": {},
        "flight_options": [],
        "hotel_options": [],
        "errors": []
    }
    
    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        context = await browser.new_context(
            viewport={"width": 1920, "height": 1080},
            user_agent="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"
        )
        page = await context.new_page()
        
        print(f"Fetching detail page: {url}")
        
        try:
            await page.goto(url, wait_until="domcontentloaded", timeout=90000)
            await page.wait_for_timeout(5000)
            
            # Try to find and click on date picker to select correct date
            date_picker_selectors = [
                "[class*='calendar']",
                "[class*='date-picker']",
                ".date-select",
                "input[type='date']",
                "[class*='datepicker']"
            ]
            
            for selector in date_picker_selectors:
                picker = await page.query_selector(selector)
                if picker:
                    print(f"Found date picker: {selector}")
                    try:
                        await picker.click()
                        await page.wait_for_timeout(1000)
                        
                        # Try to find the target date in calendar
                        # Feb 11 = day 11 in February
                        day_num = int(departure_date.split("-")[2])
                        month_num = int(departure_date.split("-")[1])
                        
                        # Look for the day cell
                        day_cell = await page.query_selector(f"[data-date='{departure_date}'], td:has-text('{day_num}')")
                        if day_cell:
                            await day_cell.click()
                            await page.wait_for_timeout(2000)
                            print(f"Clicked on date: {departure_date}")
                    except Exception as e:
                        result["errors"].append(f"Date picker interaction failed: {str(e)}")
                    break
            
            # Extract current displayed pricing
            text = await page.evaluate("() => document.body.innerText")
            
            # Look for total price
            total_pattern = r'總金額[^\d]*TWD\s*([\d,]+)'
            per_person_pattern = r'TWD\s*([\d,]+)\s*人/起'
            
            total_match = re.search(total_pattern, text)
            per_person_match = re.search(per_person_pattern, text)
            
            if total_match:
                result["pricing"]["total"] = total_match.group(1)
            if per_person_match:
                result["pricing"]["per_person"] = per_person_match.group(1)
            
            # Extract all prices
            all_prices = re.findall(r'TWD\s*([\d,]+)', text)
            result["pricing"]["all_prices_found"] = list(set(all_prices))[:15]
            
            # Look for displayed date range
            date_range_pattern = r'(\d{4}/\d{2}/\d{2})[^\d]+(\d{4}/\d{2}/\d{2})'
            date_match = re.search(date_range_pattern, text)
            if date_match:
                result["pricing"]["displayed_dates"] = {
                    "from": date_match.group(1),
                    "to": date_match.group(2)
                }
            
            # Extract flight info
            flight_pattern = r'([\w\s]+航空|Peach|虎航|樂桃|星宇|長榮|華航|酷航).*?(\d{2}:\d{2})'
            flights = re.findall(flight_pattern, text)
            result["flight_options"] = [{"airline": f[0], "time": f[1]} for f in flights[:6]]
            
            result["page_text_sample"] = text[:2500]
            
            # Screenshot
            await page.screenshot(path=f"data/liontravel-detail-{product_id}-screenshot.png")
            
        except Exception as e:
            result["errors"].append(f"Error: {str(e)}")
        
        await browser.close()
    
    return result


def save_result(result: dict, output_path: str):
    """Save the scraped result to a JSON file."""
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)
    print(f"Saved to: {output_path}")


async def main():
    # Default: search Feb 11-15 Tokyo packages
    mode = sys.argv[1] if len(sys.argv) > 1 else "search"
    
    if mode == "search":
        # Search mode
        dep_date = sys.argv[2] if len(sys.argv) > 2 else "2026-02-11"
        ret_date = sys.argv[3] if len(sys.argv) > 3 else "2026-02-15"
        output = sys.argv[4] if len(sys.argv) > 4 else f"data/liontravel-search-{dep_date}.json"
        
        print(f"Searching Lion Travel packages: {dep_date} to {ret_date}")
        result = await scrape_liontravel_packages(dep_date, ret_date)
        
    elif mode == "detail":
        # Detail mode - scrape specific product
        product_id = sys.argv[2] if len(sys.argv) > 2 else "170525001"
        dep_date = sys.argv[3] if len(sys.argv) > 3 else "2026-02-11"
        days = int(sys.argv[4]) if len(sys.argv) > 4 else 5
        output = sys.argv[5] if len(sys.argv) > 5 else f"data/liontravel-detail-{product_id}-{dep_date}.json"
        
        print(f"Fetching Lion Travel product detail: {product_id} for {dep_date} ({days} days)")
        result = await scrape_package_detail(product_id, dep_date, days)
    
    else:
        print(f"Unknown mode: {mode}")
        print("Usage:")
        print("  python scrape_liontravel_dated.py search [dep_date] [ret_date] [output]")
        print("  python scrape_liontravel_dated.py detail [product_id] [dep_date] [days] [output]")
        sys.exit(1)
    
    # Print summary
    print("\n" + "="*60)
    print("SCRAPE RESULTS")
    print("="*60)
    
    if "packages" in result and result["packages"]:
        print(f"\nFound {len(result['packages'])} packages:")
        for pkg in result["packages"][:5]:
            print(f"  - {pkg.get('title', 'No title')[:50]}: {pkg.get('prices_found', [])}")
    
    if "raw_prices" in result:
        print(f"\nPrices found: {result['raw_prices'][:10]}")
    
    if "pricing" in result:
        print(f"\nPricing: {result['pricing']}")
    
    if result.get("errors"):
        print(f"\nErrors: {result['errors']}")
    
    save_result(result, output)


if __name__ == "__main__":
    asyncio.run(main())
