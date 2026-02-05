"""
Tigerair Taiwan Parser (booking.tigerairtw.com)

Form-based flight scraper — fills the booking form interactively
since Tigerair's SPA doesn't support URL-parameterized search.
"""

from __future__ import annotations

import re
from datetime import datetime

from ..base import BaseScraper
from ..schema import ScrapeResult, FlightInfo, FlightSegment, PriceInfo


# Tigerair route map — known destinations from TPE
TIGERAIR_ROUTES = {
    "TPE": {
        "NRT": "東京(成田)",
        "KIX": "大阪(關西)",
        "NGO": "名古屋(中部)",
        "OKA": "沖繩(那霸)",
        "CTS": "札幌(新千歲)",
        "FUK": "福岡",
        "ICN": "首爾(仁川)",
        "MFM": "澳門",
        "BKK": "曼谷(廊曼)",
    },
}


class TigerairParser(BaseScraper):
    source_id = "tigerair"

    def parse_raw_text(self, raw_text: str, url: str = "", **kwargs) -> ScrapeResult:
        """Parse Tigerair results page text."""
        result = ScrapeResult(source_id=self.source_id, url=url)
        flights = parse_tigerair_flights(raw_text)

        # Set cheapest flight as the primary flight info
        if flights:
            cheapest = min(
                (f for f in flights if f.get("price")),
                key=lambda f: f["price"],
                default=None,
            )
            if cheapest:
                result.flight = FlightInfo(
                    outbound=FlightSegment(
                        flight_number=cheapest.get("flight_number", ""),
                        departure_time=cheapest.get("departure_time", ""),
                        arrival_time=cheapest.get("arrival_time", ""),
                        airline="台灣虎航",
                        airline_code="IT",
                    ),
                )
                result.price = PriceInfo(
                    per_person=cheapest.get("price"),
                    currency="TWD",
                )

        return result


# ---------------------------------------------------------------------------
# Form interaction helpers
# ---------------------------------------------------------------------------

async def fill_search_form(page, origin: str, dest: str, date: str,
                           return_date: str | None = None, pax: int = 2,
                           lang: str = "zh-TW", debug: bool = False) -> bool:
    """Fill Tigerair booking search form and submit."""
    booking_url = f"https://booking.tigerairtw.com/{lang}/index"
    print(f"Navigating to: {booking_url}")

    try:
        await page.goto(booking_url, wait_until="networkidle", timeout=60000)
    except Exception:
        print("Networkidle timeout, trying domcontentloaded...")
        await page.goto(booking_url, wait_until="domcontentloaded", timeout=60000)

    await page.wait_for_timeout(3000)

    if debug:
        await page.screenshot(path="/tmp/tigerair-01-loaded.png")

    # Trip type
    trip_type = "roundTrip" if return_date else "oneWay"
    print(f"Setting trip type: {trip_type}")
    trip_selectors = [
        f"input[value='{trip_type}']",
        f"label:has-text('{trip_type}')",
        f"[data-trip-type='{trip_type}']",
        f"button:has-text('{'來回' if return_date else '單程'}')",
        f"label:has-text('{'來回' if return_date else '單程'}')",
        f"div:has-text('{'來回' if return_date else '單程'}'):not(:has(div))",
    ]
    for sel in trip_selectors:
        try:
            el = await page.query_selector(sel)
            if el:
                await el.click()
                print(f"  Clicked trip type: {sel}")
                await page.wait_for_timeout(500)
                break
        except Exception:
            continue

    # Origin
    print(f"Setting origin: {origin}")
    await _try_set_airport(page, "origin", origin)

    # Destination
    print(f"Setting destination: {dest}")
    await _try_set_airport(page, "destination", dest)

    # Departure date
    print(f"Setting departure date: {date}")
    await _try_set_date(page, "departure", date)

    # Return date
    if return_date:
        print(f"Setting return date: {return_date}")
        await _try_set_date(page, "return", return_date)

    # Passengers
    if pax != 1:
        print(f"Setting passengers: {pax}")
        await _try_set_passengers(page, pax)

    if debug:
        await page.screenshot(path="/tmp/tigerair-02-filled.png")

    # Submit
    print("Submitting search...")
    search_selectors = [
        "button[type='submit']",
        "button:has-text('搜尋')",
        "button:has-text('Search')",
        "button:has-text('查詢')",
        "[class*='search'] button",
        "[class*='submit']",
    ]
    for sel in search_selectors:
        try:
            el = await page.query_selector(sel)
            if el and await el.is_visible():
                await el.click()
                print(f"  Clicked search: {sel}")
                break
        except Exception:
            continue

    return True


async def extract_flight_results(page, debug: bool = False) -> dict:
    """Wait for results and extract flight data."""
    print("Waiting for search results...")

    try:
        await page.wait_for_url("**/flight/select**", timeout=30000)
        print("  URL changed to flight select page")
    except Exception:
        print("  URL did not change, checking for results on current page...")

    await page.wait_for_timeout(5000)

    if debug:
        await page.screenshot(path="/tmp/tigerair-03-results.png")

    raw_text = await page.evaluate("() => document.body.innerText")
    flights = parse_tigerair_flights(raw_text)

    return {"raw_text": raw_text, "flights": flights}


# ---------------------------------------------------------------------------
# Pure parsing functions
# ---------------------------------------------------------------------------

def parse_tigerair_flights(raw_text: str) -> list[dict]:
    """Parse flight options from Tigerair results page text."""
    flights = []
    lines = raw_text.split("\n")
    current_flight: dict = {}

    for i, line in enumerate(lines):
        line = line.strip()
        if not line:
            continue

        # Flight number (IT + 3-4 digits)
        flight_match = re.search(r"\b(IT\s*\d{3,4})\b", line)
        if flight_match:
            if current_flight.get("flight_number"):
                flights.append(current_flight)
                current_flight = {}
            current_flight["flight_number"] = flight_match.group(1).replace(" ", "")

        # Time pattern
        time_match = re.findall(r"(\d{1,2}:\d{2})", line)
        if time_match and current_flight.get("flight_number"):
            if len(time_match) >= 2 and "departure_time" not in current_flight:
                current_flight["departure_time"] = time_match[0]
                current_flight["arrival_time"] = time_match[1]
            elif len(time_match) == 1 and "departure_time" not in current_flight:
                current_flight["departure_time"] = time_match[0]
            elif len(time_match) == 1 and "arrival_time" not in current_flight:
                current_flight["arrival_time"] = time_match[0]

        # Price
        price_match = re.search(r"(?:TWD|NT\$?)\s*([\d,]+)", line)
        if price_match and current_flight.get("flight_number"):
            price = int(price_match.group(1).replace(",", ""))
            if price > 500:
                if "price" not in current_flight or price < current_flight["price"]:
                    current_flight["price"] = price

        # Duration
        dur_match = re.search(r"(\d+)\s*[hH小時]\s*(\d+)?\s*[mM分]?", line)
        if dur_match and current_flight.get("flight_number"):
            hours = int(dur_match.group(1))
            mins = int(dur_match.group(2) or 0)
            current_flight["duration_minutes"] = hours * 60 + mins

    if current_flight.get("flight_number"):
        flights.append(current_flight)

    return flights


# ---------------------------------------------------------------------------
# Airport / date / passenger helpers
# ---------------------------------------------------------------------------

async def _try_set_airport(page, field: str, code: str) -> bool:
    """Try to set airport in the search form."""
    field_selectors = [
        f"[data-field='{field}']",
        f"[id*='{field}']",
        f"[name*='{field}']",
        f"[class*='{field}']",
        f"input[placeholder*='{'出發' if field == 'origin' else '到達'}']",
        f"input[placeholder*='{'From' if field == 'origin' else 'To'}']",
        f"div:has-text('{'出發地' if field == 'origin' else '目的地'}'):not(:has(div))",
    ]

    for sel in field_selectors:
        try:
            el = await page.query_selector(sel)
            if el:
                await el.click()
                await page.wait_for_timeout(800)
                code_selectors = [
                    f"text={code}",
                    f"[data-code='{code}']",
                    f"li:has-text('{code}')",
                    f"option[value='{code}']",
                    f"div:has-text('{code}'):not(:has(div:has-text('{code}')))",
                ]
                for code_sel in code_selectors:
                    try:
                        code_el = await page.query_selector(code_sel)
                        if code_el and await code_el.is_visible():
                            await code_el.click()
                            print(f"  Selected {field}: {code} via {sel} → {code_sel}")
                            await page.wait_for_timeout(500)
                            return True
                    except Exception:
                        continue
        except Exception:
            continue

    # Fallback: type into input
    input_selectors = [
        f"input[id*='{field}']",
        f"input[name*='{field}']",
        f"input[class*='{field}']",
    ]
    for sel in input_selectors:
        try:
            el = await page.query_selector(sel)
            if el:
                await el.fill("")
                await el.type(code, delay=100)
                await page.wait_for_timeout(1000)
                await page.keyboard.press("Enter")
                print(f"  Typed {field}: {code} via {sel}")
                return True
        except Exception:
            continue

    return False


async def _try_set_date(page, field: str, date_str: str) -> bool:
    """Try to set a date in the search form."""
    year, month, day = date_str.split("-")

    date_field_selectors = [
        f"[data-field='{field}-date']",
        f"[id*='{field}'][id*='date']",
        f"input[name*='{field}']",
        f"[class*='{field}'][class*='date']",
        f"div:has-text('{'出發日期' if field == 'departure' else '回程日期'}'):not(:has(div))",
    ]

    for sel in date_field_selectors:
        try:
            el = await page.query_selector(sel)
            if el:
                await el.click()
                await page.wait_for_timeout(800)

                target_month = f"{year}-{month}"
                await _navigate_calendar(page, target_month)

                day_int = int(day)
                day_selectors = [
                    f"[data-date='{date_str}']",
                    f"td[data-day='{day_int}']",
                    f"button:has-text('{day_int}'):not(:has-text('/'))",
                    f"[aria-label*='{date_str}']",
                ]
                for day_sel in day_selectors:
                    try:
                        day_el = await page.query_selector(day_sel)
                        if day_el and await day_el.is_visible():
                            await day_el.click()
                            print(f"  Selected {field} date: {date_str}")
                            await page.wait_for_timeout(500)
                            return True
                    except Exception:
                        continue
        except Exception:
            continue

    # Fallback: direct input
    for sel in [f"input[id*='{field}']", "input[name*='date']"]:
        try:
            el = await page.query_selector(sel)
            if el:
                await el.fill(date_str)
                print(f"  Filled {field} date input: {date_str}")
                return True
        except Exception:
            continue

    return False


async def _navigate_calendar(page, target_ym: str):
    """Navigate calendar to target year-month."""
    for _ in range(12):
        cal_header = await page.query_selector(
            "[class*='calendar'] [class*='header'], "
            "[class*='month-year'], [class*='title']"
        )
        if cal_header:
            text = await cal_header.inner_text()
            if target_ym.replace("-", "/") in text or target_ym in text:
                return

        next_selectors = [
            "button[class*='next']",
            "[class*='next-month']",
            "button:has-text('>')",
            "[aria-label='Next month']",
        ]
        clicked = False
        for sel in next_selectors:
            try:
                el = await page.query_selector(sel)
                if el and await el.is_visible():
                    await el.click()
                    await page.wait_for_timeout(300)
                    clicked = True
                    break
            except Exception:
                continue
        if not clicked:
            break


async def _try_set_passengers(page, pax: int) -> bool:
    """Try to set passenger count."""
    pax_selectors = [
        "[class*='passenger']",
        "[id*='passenger']",
        "[data-field='adults']",
        "div:has-text('乘客'):not(:has(div))",
        "div:has-text('Passengers'):not(:has(div))",
    ]

    for sel in pax_selectors:
        try:
            el = await page.query_selector(sel)
            if el:
                await el.click()
                await page.wait_for_timeout(500)
                plus_selectors = [
                    "button:has-text('+')",
                    "[class*='increase']",
                    "[class*='plus']",
                    "[aria-label='Add adult']",
                ]
                for _ in range(pax - 1):
                    for plus_sel in plus_selectors:
                        try:
                            plus_el = await page.query_selector(plus_sel)
                            if plus_el and await plus_el.is_visible():
                                await plus_el.click()
                                await page.wait_for_timeout(200)
                                break
                        except Exception:
                            continue
                print(f"  Set passengers: {pax}")
                return True
        except Exception:
            continue

    return False
