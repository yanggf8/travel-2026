#!/usr/bin/env python3
"""
Generic OTA Package Scraper

Scrape travel package details from various OTA websites using Playwright.
Supports JavaScript-rendered pages and extracts raw text + structured elements.

Supported OTAs:
- BestTour (besttour.com.tw) - Full calendar pricing
- Lion Travel (liontravel.com) - Package search results
- Lifetour (tour.lifetour.com.tw) - Package with itinerary
- Settour (tour.settour.com.tw) - Package with itinerary
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
from typing import Optional, Tuple

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

        # Settour specific: click tabs to load details
        if "settour.com.tw" in url:
            print("Settour detected: clicking tabs to load details...")
            try:
                tab_selectors = [
                    "text=航班資訊",
                    "text=飯店安排",
                    "text=每日行程",
                    "text=出發日期",
                ]
                for selector in tab_selectors:
                    try:
                        tab = await page.query_selector(selector)
                        if tab:
                            await tab.click()
                            print(f"  Clicked tab: {selector}")
                            await page.wait_for_timeout(1500)
                    except Exception:
                        continue
            except Exception as e:
                print(f"  Could not click Settour tabs: {e}")

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
                "inclusions": [],
                "date_pricing": {}
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

        # Extract package links from listing pages
        package_links = await extract_package_links(page, url)
        if package_links:
            result["package_links"] = package_links

        # BestTour specific parsing
        if "besttour.com.tw" in url:
            flights = parse_besttour_flights(result["raw_text"])
            result["extracted"]["flight"] = flights
            result["extracted"]["hotel"] = parse_besttour_hotel(result["raw_text"])
            ym = infer_year_month_from_besttour_flight_date(
                (flights.get("outbound") or {}).get("date")
            )
            result["extracted"]["date_pricing"] = parse_besttour_date_pricing(
                result["raw_text"], year_month=ym
            )
            result["extracted"]["inclusions"] = parse_besttour_inclusions(result["raw_text"])

        # Settour specific parsing
        if "settour.com.tw" in url:
            result["extracted"]["flight"] = parse_settour_flights(result["raw_text"])
            result["extracted"]["hotel"] = parse_settour_hotel(result["raw_text"])
            result["extracted"]["price"] = parse_settour_price(result["raw_text"])
            result["extracted"]["dates"] = parse_settour_dates(result["raw_text"])
            result["extracted"]["itinerary"] = parse_settour_itinerary(result["raw_text"])
            result["extracted"]["inclusions"] = parse_settour_inclusions(result["raw_text"])

        # Lifetour specific parsing
        if "lifetour.com.tw" in url:
            result["extracted"]["flight"] = parse_lifetour_flights(result["raw_text"])
            result["extracted"]["hotel"] = parse_lifetour_hotel(result["raw_text"])
            result["extracted"]["price"] = parse_lifetour_price(result["raw_text"])
            result["extracted"]["dates"] = parse_lifetour_dates(result["raw_text"])
            result["extracted"]["itinerary"] = parse_lifetour_itinerary(result["raw_text"])
            result["extracted"]["inclusions"] = parse_lifetour_inclusions(result["raw_text"])

        await browser.close()

        return result


async def extract_package_links(page, base_url: str) -> list:
    """Extract package detail links from listing pages.

    Supports:
    - BestTour: /itinerary/CODE links
    - LionTravel: vacation.liontravel.com/product/* links
    - Lifetour: tour.lifetour.com.tw/detail* links
    - Settour: tour.settour.com.tw/product/* links
    """
    import re
    from urllib.parse import urljoin

    links = []

    try:
        # Get all anchor elements
        anchors = await page.query_selector_all('a[href]')
        seen = set()

        for anchor in anchors[:100]:  # Limit to first 100 links
            try:
                href = await anchor.get_attribute('href')
                if not href:
                    continue

                # Get link text for context
                text = await anchor.inner_text()
                text = text.strip()[:100] if text else ''

                # Resolve relative URLs
                full_url = urljoin(base_url, href)

                # Skip if already seen
                if full_url in seen:
                    continue

                # BestTour package links
                if 'besttour.com.tw' in base_url:
                    if '/itinerary/' in href and href not in seen:
                        seen.add(full_url)
                        # Extract product code
                        code_match = re.search(r'/itinerary/([A-Z0-9]+)', href)
                        code = code_match.group(1) if code_match else ''
                        links.append({
                            'url': full_url,
                            'code': code,
                            'title': text,
                        })

                # LionTravel package links
                elif 'liontravel.com' in base_url:
                    if '/product/' in href or '/detail/' in href:
                        seen.add(full_url)
                        code_match = re.search(r'/(?:product|detail)/(\d+)', href)
                        code = code_match.group(1) if code_match else ''
                        links.append({
                            'url': full_url,
                            'code': code,
                            'title': text,
                        })

                # Lifetour package links
                elif 'lifetour.com.tw' in base_url:
                    if '/detail' in href:
                        seen.add(full_url)
                        links.append({
                            'url': full_url,
                            'code': '',
                            'title': text,
                        })

                # Settour package links
                elif 'settour.com.tw' in base_url:
                    if '/product/' in href:
                        seen.add(full_url)
                        code_match = re.search(r'/product/([A-Z0-9]+)', href, re.IGNORECASE)
                        code = code_match.group(1) if code_match else ''
                        links.append({
                            'url': full_url,
                            'code': code,
                            'title': text,
                        })

            except Exception:
                continue

    except Exception as e:
        print(f"  Error extracting package links: {e}")

    return links


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


def infer_year_month_from_besttour_flight_date(date_str: Optional[str]) -> Optional[Tuple[int, int]]:
    """Infer (year, month) from BestTour flight date strings like '2026/02/13(五)'."""
    if not date_str:
        return None
    import re
    m = re.search(r'(\d{4})[/-](\d{1,2})[/-](\d{1,2})', date_str)
    if not m:
        return None
    return int(m.group(1)), int(m.group(2))


def parse_besttour_inclusions(raw_text: str) -> list:
    """Extract inclusions like breakfast from BestTour text."""
    inclusions = []
    text = raw_text.replace(" ", "")
    if "早餐" in text and ("含早餐" in text or "包含早餐" in text or "附早餐" in text or "輕食早餐" in text or "簡易早餐" in text):
        inclusions.append("light_breakfast")
    return inclusions


def parse_besttour_hotel(raw_text: str) -> dict:
    """Parse BestTour hotel section from raw page text (heuristic)."""
    import re

    lines = [l.strip() for l in raw_text.split("\n")]
    hotel = {"name": None, "area": None, "access": []}

    # Heuristic 1: find a '住宿' section and take the next meaningful line as name.
    for i, line in enumerate(lines):
        if line in ("住宿", "飯店", "旅館", "酒店") and i + 1 < len(lines):
            for j in range(i + 1, min(i + 25, len(lines))):
                candidate = lines[j].strip()
                if not candidate:
                    continue
                if candidate in ("交通方式", "行程內容", "出發日期", "費用說明"):
                    break
                # Avoid label-like lines
                if re.match(r"^(地區|區域|地址|電話|入住|退房)[:：]", candidate):
                    continue
                if len(candidate) >= 4:
                    hotel["name"] = candidate
                    break
            if hotel["name"]:
                break

    # Area label extraction if present
    for line in lines:
        m = re.search(r"(地區|區域)[:：]\s*(.+)$", line)
        if m:
            hotel["area"] = m.group(2).strip()
            break

    # Access: collect typical transit lines with minutes
    access = []
    for line in lines:
        if re.search(r"(JR|地鐵|捷運|單軌|Monorail|Yurikamome|ゆりかもめ)", line, re.IGNORECASE) and re.search(r"(\d+)\s*(分|分鐘|min)", line, re.IGNORECASE):
            access.append(line.strip())
    hotel["access"] = list(dict.fromkeys(access))[:8]

    return hotel


def parse_besttour_date_pricing(raw_text: str, year_month: Optional[Tuple[int, int]] = None) -> dict:
    """Parse BestTour calendar pricing from raw page text (heuristic)."""
    import re

    def to_iso(y: int, m: int, d: int) -> str:
        return f"{y:04d}-{m:02d}-{d:02d}"

    def map_availability(label: str) -> str:
        if label in ("可售", "可報名", "可預訂"):
            return "available"
        if label in ("滿團", "額滿", "已滿", "停售", "滿員"):
            return "sold_out"
        if label in ("候補", "關團", "有限"):
            return "limited"
        return "limited"

    pricing: dict = {}
    lines = [l.strip() for l in raw_text.split("\n") if l.strip()]

    # Prefer full-date matches if present.
    full_date_re = re.compile(r"(\d{4})[/-](\d{1,2})[/-](\d{1,2}).{0,20}?(可售|滿團|候補|額滿|已滿|停售|關團).{0,30}?([0-9]{4,6})")
    for line in lines:
        m = full_date_re.search(line)
        if not m:
            continue
        y, mo, d = int(m.group(1)), int(m.group(2)), int(m.group(3))
        label = m.group(4)
        price = int(m.group(5))
        seats_match = re.search(r"可售[:：]?\s*(\d+)", line)
        seats = int(seats_match.group(1)) if seats_match else None
        pricing[to_iso(y, mo, d)] = {
            "price": price,
            "availability": map_availability(label),
            "seats_remaining": seats,
        }

    if pricing:
        return pricing

    # Fallback: day-of-month calendar lines if year/month is known.
    if not year_month:
        return pricing
    y, mo = year_month
    day_line_re = re.compile(r"^(\d{1,2})\s*(可售|滿團|候補|額滿|已滿|停售|關團).{0,40}?([0-9]{4,6})")
    for line in lines:
        m = day_line_re.match(line)
        if not m:
            continue
        d = int(m.group(1))
        label = m.group(2)
        price = int(m.group(3))
        seats_match = re.search(r"可售[:：]?\s*(\d+)", line)
        seats = int(seats_match.group(1)) if seats_match else None
        pricing[to_iso(y, mo, d)] = {
            "price": price,
            "availability": map_availability(label),
            "seats_remaining": seats,
        }

    return pricing


def parse_lifetour_flights(raw_text: str) -> dict:
    """Parse Lifetour flight details from raw page text."""
    import re

    flight_info = {"outbound": {}, "return": {}}
    lines = raw_text.split('\n')

    # Pattern: MM/DD(Day) HH:MM followed by airport info
    # Example: 02/27(五) 15:40 → 台北市 TPE ... → 02/27(五) 19:20 大阪 OSA
    for i, line in enumerate(lines):
        line = line.strip()

        # Look for flight number pattern like "亞洲航空D7378"
        flight_match = re.search(r'(亞洲航空|華航|長榮|星宇|虎航|樂桃|酷航|捷星)([A-Z]{1,2}\d{2,4})', line)
        if flight_match and i >= 5:
            # Found a flight, look backwards for time/route info
            airline = flight_match.group(1)
            flight_num = flight_match.group(2)

            # Check previous lines for departure/arrival info
            prev_lines = lines[max(0, i-8):i+1]
            times = re.findall(r'(\d{2}/\d{2})\([一二三四五六日]\)\s*(\d{1,2}:\d{2})', '\n'.join(prev_lines))
            airports = re.findall(r'(TPE|NRT|HND|KIX|OSA|NGO|CTS|FUK|OKA)', '\n'.join(prev_lines))

            if len(times) >= 2 and len(airports) >= 2:
                if not flight_info["outbound"].get("flight_number"):
                    flight_info["outbound"] = {
                        "date": times[0][0],
                        "departure_time": times[0][1],
                        "arrival_time": times[1][1],
                        "airline": airline,
                        "flight_number": flight_num,
                        "departure_code": airports[0],
                        "arrival_code": airports[1],
                    }
                elif not flight_info["return"].get("flight_number"):
                    flight_info["return"] = {
                        "date": times[0][0],
                        "departure_time": times[0][1],
                        "arrival_time": times[1][1],
                        "airline": airline,
                        "flight_number": flight_num,
                        "departure_code": airports[0],
                        "arrival_code": airports[1],
                    }

    return flight_info


def parse_lifetour_hotel(raw_text: str) -> dict:
    """Parse Lifetour hotel details from raw page text."""
    import re

    hotel = {"names": [], "room_type": None, "area": None}

    # Look for hotel names after "住宿" section
    lines = raw_text.split('\n')
    in_hotel_section = False

    for line in lines:
        line = line.strip()

        if line == "住宿":
            in_hotel_section = True
            continue

        if in_hotel_section and line:
            # Extract hotel names - usually contains "或" (or) separator
            if "或" in line and ("酒店" in line or "飯店" in line or "Hotel" in line or "Inn" in line or "GRAND" in line):
                # Split by "或" and clean up
                names = re.split(r'或\s*', line)
                for name in names:
                    name = name.strip()
                    # Remove room type info in parentheses for name extraction
                    clean_name = re.sub(r'\s*\([^)]*\)', '', name).strip()
                    if clean_name and len(clean_name) > 2 and "同級" not in clean_name:
                        hotel["names"].append(clean_name)

                # Extract room type
                room_match = re.search(r'(SEMI DOUBLE|TWN|TWIN|DBL|DOUBLE|單人房|雙人房)', line, re.IGNORECASE)
                if room_match:
                    hotel["room_type"] = room_match.group(1)

                # Extract bed width
                bed_match = re.search(r'床寬(\d+)CM', line, re.IGNORECASE)
                if bed_match:
                    hotel["bed_width_cm"] = int(bed_match.group(1))

                break

            if line in ("餐食", "收合景點", "Day"):
                in_hotel_section = False

    return hotel


def parse_lifetour_price(raw_text: str) -> dict:
    """Parse Lifetour price details from raw page text."""
    import re

    price = {}

    # Look for price pattern: NT$XX,XXX or $XX,XXX
    price_matches = re.findall(r'NT?\$\s*([\d,]+)\s*元?', raw_text)
    if price_matches:
        # Filter out deposit (usually 10,000)
        prices = [int(p.replace(',', '')) for p in price_matches]
        prices = [p for p in prices if p > 15000]  # Filter out deposits
        if prices:
            price["per_person"] = min(prices)
            price["currency"] = "TWD"

    # Look for deposit
    deposit_match = re.search(r'訂金\s*NT?\$\s*([\d,]+)', raw_text)
    if deposit_match:
        price["deposit"] = int(deposit_match.group(1).replace(',', ''))

    # Look for availability
    avail_match = re.search(r'可售\s*(\d+)\s*人', raw_text)
    if avail_match:
        price["seats_available"] = int(avail_match.group(1))

    min_match = re.search(r'成行\s*(\d+)\s*人', raw_text)
    if min_match:
        price["min_travelers"] = int(min_match.group(1))

    return price


def parse_lifetour_dates(raw_text: str) -> dict:
    """Parse Lifetour travel dates from raw page text."""
    import re

    dates = {}

    # Look for duration: 5天4夜
    duration_match = re.search(r'(\d+)\s*天\s*(\d+)\s*夜', raw_text)
    if duration_match:
        dates["duration_days"] = int(duration_match.group(1))
        dates["duration_nights"] = int(duration_match.group(2))

    # Look for departure date: 出發日期 02月27日 or 2/27
    depart_match = re.search(r'出發日期\s*(\d{1,2})月(\d{1,2})日', raw_text)
    if depart_match:
        dates["departure_month"] = int(depart_match.group(1))
        dates["departure_day"] = int(depart_match.group(2))

    # Look for year in calendar section
    year_match = re.search(r'(\d{4})\s*年\s*(\d{1,2})\s*月', raw_text)
    if year_match:
        dates["year"] = int(year_match.group(1))

    return dates


def parse_lifetour_itinerary(raw_text: str) -> list:
    """Parse Lifetour daily itinerary from raw page text."""
    import re

    itinerary = []
    lines = raw_text.split('\n')

    current_day = None
    current_content = []

    for line in lines:
        line = line.strip()

        # Match day headers: Day 1, Day 2, etc.
        day_match = re.match(r'^Day\s*(\d+)$', line)
        if day_match:
            # Save previous day if exists
            if current_day is not None:
                content_text = ' '.join(current_content)
                itinerary.append({
                    "day": current_day,
                    "content": content_text[:500],  # First 500 chars
                    "is_free": any(kw in content_text for kw in ["自由活動", "全日自由"]),
                    "is_guided": any(kw in content_text for kw in ["奈良", "京都", "嵐山", "伏見"]),
                })

            current_day = int(day_match.group(1))
            current_content = []
            continue

        if current_day is not None:
            # Stop at next Day header or end markers
            if line.startswith("出團備註") or line.startswith("看完整資訊"):
                break
            current_content.append(line)

    # Don't forget last day
    if current_day is not None and current_content:
        content_text = ' '.join(current_content)
        itinerary.append({
            "day": current_day,
            "content": content_text[:500],
            "is_free": any(kw in content_text for kw in ["自由活動", "全日自由"]),
            "is_guided": any(kw in content_text for kw in ["奈良", "京都", "嵐山", "伏見"]),
        })

    return itinerary


def parse_lifetour_inclusions(raw_text: str) -> list:
    """Parse Lifetour inclusions from raw page text."""
    inclusions = []

    text = raw_text.replace(" ", "")

    if "含團險" in text:
        inclusions.append("travel_insurance")
    if "含國內外機場稅" in text or "含機場稅" in text:
        inclusions.append("airport_tax")
    if "早餐" in text and ("飯店內用" in text or "含早餐" in text):
        inclusions.append("breakfast")

    return inclusions


def parse_settour_flights(raw_text: str) -> dict:
    """Parse Settour flight details from raw page text."""
    import re

    flight_info = {"outbound": {}, "return": {}}
    lines = raw_text.split('\n')

    for i, line in enumerate(lines):
        line = line.strip()

        # Match 去程 / 回程 markers
        if line == '去程' and i + 8 < len(lines):
            flight_info["outbound"] = _parse_settour_flight_block(lines, i)
        elif line == '回程' and i + 8 < len(lines):
            flight_info["return"] = _parse_settour_flight_block(lines, i)
            break

    # Fallback: scan for flight number patterns
    if not flight_info["outbound"]:
        for i, line in enumerate(lines):
            line = line.strip()
            flight_match = re.search(r'([A-Z]{2})\s*(\d{2,4})', line)
            if flight_match and re.search(r'TPE|NRT|HND|KIX|OSA|NGO', '\n'.join(lines[max(0,i-5):i+5])):
                airports = re.findall(r'(TPE|NRT|HND|KIX|OSA|NGO|CTS|FUK|OKA)', '\n'.join(lines[max(0,i-5):i+5]))
                times = re.findall(r'(\d{2}:\d{2})', '\n'.join(lines[max(0,i-5):i+5]))
                if airports and times:
                    target = flight_info["outbound"] if not flight_info["outbound"] else flight_info["return"]
                    target.update({
                        "flight_number": flight_match.group(1) + flight_match.group(2),
                        "departure_code": airports[0] if airports else None,
                        "arrival_code": airports[1] if len(airports) > 1 else None,
                        "departure_time": times[0] if times else None,
                        "arrival_time": times[1] if len(times) > 1 else None,
                    })
                    if flight_info["outbound"] and flight_info["return"]:
                        break

    return flight_info


def _parse_settour_flight_block(lines: list, start: int) -> dict:
    """Parse a single flight block (去程 or 回程) from Settour text."""
    import re

    block = {}
    nearby = lines[start:min(start + 12, len(lines))]
    text = '\n'.join(nearby)

    # Extract date
    date_match = re.search(r'(\d{4})[/-](\d{1,2})[/-](\d{1,2})', text)
    if date_match:
        block["date"] = f"{date_match.group(1)}/{date_match.group(2).zfill(2)}/{date_match.group(3).zfill(2)}"

    # Extract flight number
    flight_match = re.search(r'([A-Z]{2})\s*(\d{2,4})', text)
    if flight_match:
        block["flight_number"] = flight_match.group(1) + flight_match.group(2)

    # Extract airports
    airports = re.findall(r'(TPE|NRT|HND|KIX|OSA|NGO|CTS|FUK|OKA)', text)
    if len(airports) >= 2:
        block["departure_code"] = airports[0]
        block["arrival_code"] = airports[1]

    # Extract times
    times = re.findall(r'(\d{2}:\d{2})', text)
    if len(times) >= 2:
        block["departure_time"] = times[0]
        block["arrival_time"] = times[1]

    # Extract airline name
    airline_match = re.search(r'(中華航空|長榮航空|星宇航空|台灣虎航|樂桃航空|酷航|捷星|亞洲航空)', text)
    if airline_match:
        block["airline"] = airline_match.group(1)

    return block


def parse_settour_hotel(raw_text: str) -> dict:
    """Parse Settour hotel details from raw page text."""
    import re

    hotel = {"names": [], "area": None, "access": []}
    lines = raw_text.split('\n')
    in_hotel_section = False

    for line in lines:
        line = line.strip()

        if line in ("飯店安排", "住宿安排", "住宿"):
            in_hotel_section = True
            continue

        if in_hotel_section and line:
            # Stop at next section
            if line in ("每日行程", "航班資訊", "出發日期", "費用說明", "注意事項"):
                break

            # Look for hotel names (often contain Hotel, 飯店, 酒店)
            if re.search(r'(Hotel|飯店|酒店|旅館|Inn|Resort|HOTEL)', line, re.IGNORECASE):
                names = re.split(r'或\s*同級|或\s*', line)
                for name in names:
                    name = re.sub(r'\s*\([^)]*\)', '', name).strip()
                    name = re.sub(r'(或同級|同級)', '', name).strip()
                    if name and len(name) > 2:
                        hotel["names"].append(name)

    # Extract area
    for line in lines:
        m = re.search(r'(地區|區域)[:：]\s*(.+)$', line)
        if m:
            hotel["area"] = m.group(2).strip()
            break

    return hotel


def parse_settour_price(raw_text: str) -> dict:
    """Parse Settour price details from raw page text."""
    import re

    price = {}

    # Pattern: 售價 NT$XX,XXX or $XX,XXX
    price_matches = re.findall(r'(?:售價|團費|價格)\s*(?:NT)?\$\s*([\d,]+)', raw_text)
    if price_matches:
        prices = [int(p.replace(',', '')) for p in price_matches]
        prices = [p for p in prices if p > 10000]
        if prices:
            price["per_person"] = min(prices)
            price["currency"] = "TWD"

    # Fallback: any NT$ amount > 15000
    if not price.get("per_person"):
        all_prices = re.findall(r'NT?\$\s*([\d,]+)', raw_text)
        prices = sorted(set(int(p.replace(',', '')) for p in all_prices if int(p.replace(',', '')) > 15000))
        if prices:
            price["per_person"] = prices[0]
            price["currency"] = "TWD"

    # Deposit
    deposit_match = re.search(r'訂金\s*(?:NT)?\$\s*([\d,]+)', raw_text)
    if deposit_match:
        price["deposit"] = int(deposit_match.group(1).replace(',', ''))

    return price


def parse_settour_dates(raw_text: str) -> dict:
    """Parse Settour travel dates from raw page text."""
    import re

    dates = {}

    duration_match = re.search(r'(\d+)\s*天\s*(\d+)\s*夜', raw_text)
    if duration_match:
        dates["duration_days"] = int(duration_match.group(1))
        dates["duration_nights"] = int(duration_match.group(2))

    depart_match = re.search(r'出發日期\s*[:：]?\s*(\d{4})[/-](\d{1,2})[/-](\d{1,2})', raw_text)
    if depart_match:
        dates["year"] = int(depart_match.group(1))
        dates["departure_month"] = int(depart_match.group(2))
        dates["departure_day"] = int(depart_match.group(3))

    return dates


def parse_settour_itinerary(raw_text: str) -> list:
    """Parse Settour daily itinerary from raw page text."""
    import re

    itinerary = []
    lines = raw_text.split('\n')

    current_day = None
    current_content = []

    for line in lines:
        line = line.strip()

        # Match day headers: Day 1, DAY1, 第1天, etc.
        day_match = re.match(r'^(?:Day|DAY|第)\s*(\d+)\s*(?:天)?$', line)
        if day_match:
            if current_day is not None:
                content_text = ' '.join(current_content)
                itinerary.append({
                    "day": current_day,
                    "content": content_text[:500],
                    "is_free": any(kw in content_text for kw in ["自由活動", "全日自由", "自由前往"]),
                    "is_guided": any(kw in content_text for kw in ["奈良", "京都", "嵐山", "伏見", "清水寺"]),
                })
            current_day = int(day_match.group(1))
            current_content = []
            continue

        if current_day is not None:
            if line.startswith("注意事項") or line.startswith("出團備註"):
                break
            current_content.append(line)

    if current_day is not None and current_content:
        content_text = ' '.join(current_content)
        itinerary.append({
            "day": current_day,
            "content": content_text[:500],
            "is_free": any(kw in content_text for kw in ["自由活動", "全日自由", "自由前往"]),
            "is_guided": any(kw in content_text for kw in ["奈良", "京都", "嵐山", "伏見", "清水寺"]),
        })

    return itinerary


def parse_settour_inclusions(raw_text: str) -> list:
    """Parse Settour inclusions from raw page text."""
    inclusions = []
    text = raw_text.replace(" ", "")

    if "含團險" in text or "旅行業責任保險" in text:
        inclusions.append("travel_insurance")
    if "含機場稅" in text or "含國內外機場稅" in text or "兩地機場稅" in text:
        inclusions.append("airport_tax")
    if "早餐" in text and ("飯店內用" in text or "含早餐" in text or "飯店早餐" in text):
        inclusions.append("breakfast")

    return inclusions


def save_result(result: dict, output_path: str):
    """Save the scraped result to a JSON file."""
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)
    print(f"Saved to: {output_path}")


async def main():
    quiet = "--quiet" in sys.argv
    argv = [a for a in sys.argv[1:] if a != "--quiet"]

    url = argv[0] if len(argv) > 0 else "https://www.besttour.com.tw/itinerary/TYO05MM260211AM"
    output = argv[1] if len(argv) > 1 else "data/package-scrape-result.json"

    result = await scrape_package(url)

    if not quiet:
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
