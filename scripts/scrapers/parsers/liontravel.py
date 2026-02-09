"""
Lion Travel Parser (liontravel.com / vacation.liontravel.com)

Extracts package search results and detail page pricing from Lion Travel.
Supports both listing search and individual product detail pages.
"""

from __future__ import annotations

import re
from datetime import datetime, timedelta

from ..base import BaseScraper
from ..schema import ScrapeResult, PriceInfo, DatesInfo, FlightInfo, FlightSegment, HotelInfo


class LionTravelParser(BaseScraper):
    source_id = "liontravel"

    async def prepare_page(self, page, url: str) -> None:
        """Wait for Lion Travel's dynamic content to load."""
        print("Waiting for Lion Travel search results to load...")
        await page.wait_for_timeout(8000)

        # Try to wait for product cards
        try:
            await page.wait_for_selector(
                ".product-card, .search-result-item, [class*='product']",
                timeout=15000,
            )
        except Exception:
            print("No product cards found with standard selectors, continuing...")

    def parse_raw_text(self, raw_text: str, url: str = "", **kwargs) -> ScrapeResult:
        """Parse Lion Travel page text into structured data."""
        result = ScrapeResult(source_id=self.source_id, url=url)

        result.price = _parse_search_prices(raw_text)
        result.dates = _parse_dates_from_url(url)
        result.flight = _parse_search_flight(raw_text)
        result.hotel = _parse_search_hotel(raw_text)
        result.package_type = "fit"  # LionTravel vacation.liontravel.com is FIT only

        return result

    async def scrape_search(
        self,
        page,
        departure_date: str = "2026-02-11",
        return_date: str = "2026-02-15",
        destination: str = "JP_TYO_6",
        adults: int = 2,
    ) -> ScrapeResult:
        """
        Scrape Lion Travel search results for specific dates.

        This is the high-level method that builds the URL and scrapes.
        """
        from ..base import navigate_with_retry, safe_extract_text

        from_date = departure_date.replace("-", "")
        to_date = return_date.replace("-", "")

        dep = datetime.strptime(departure_date, "%Y-%m-%d")
        ret = datetime.strptime(return_date, "%Y-%m-%d")
        days = (ret - dep).days + 1

        url = (
            f"https://vacation.liontravel.com/search"
            f"?Destination={destination}"
            f"&FromDate={from_date}&ToDate={to_date}"
            f"&Days={days}&roomlist={adults}-0-0"
        )

        result = ScrapeResult(
            source_id=self.source_id,
            url=url,
            scraped_at=datetime.now().isoformat(),
        )
        result.dates = DatesInfo(
            duration_days=days,
            departure_date=departure_date,
            return_date=return_date,
        )

        # Navigate
        success = await navigate_with_retry(page, url)
        if not success:
            result.success = False
            result.errors.append(f"Failed to navigate to {url}")
            return result

        # Prepare page
        await self.prepare_page(page, url)

        # Extract text
        raw_text = await safe_extract_text(page)
        result.raw_text = raw_text

        try:
            result.title = await page.title()
        except Exception:
            pass

        # Extract packages from DOM
        result.package_links = await _extract_packages_from_dom(page)

        # Parse prices from text
        result.price = _parse_search_prices(raw_text)

        # Extract flight and hotel info from raw text
        result.flight = _parse_search_flight(raw_text)
        result.hotel = _parse_search_hotel(raw_text)

        return result

    async def scrape_detail(
        self,
        page,
        product_id: str,
        departure_date: str = "2026-02-11",
        days: int = 5,
        adults: int = 2,
    ) -> ScrapeResult:
        """Scrape a specific product detail page."""
        from ..base import navigate_with_retry, safe_extract_text

        from_date = departure_date.replace("-", "")
        url = (
            f"https://vacation.liontravel.com/detail/{product_id}"
            f"?FromDate={from_date}&Days={days}&roomlist={adults}-0-0"
        )

        result = ScrapeResult(
            source_id=self.source_id,
            url=url,
            scraped_at=datetime.now().isoformat(),
        )

        success = await navigate_with_retry(page, url)
        if not success:
            result.success = False
            result.errors.append(f"Failed to navigate to {url}")
            return result

        await page.wait_for_timeout(5000)

        raw_text = await safe_extract_text(page)
        result.raw_text = raw_text

        try:
            result.title = await page.title()
        except Exception:
            pass

        result.price = _parse_detail_prices(raw_text)

        return result


# ---------------------------------------------------------------------------
# Pure parsing functions
# ---------------------------------------------------------------------------

def _parse_search_prices(raw_text: str) -> PriceInfo:
    """Extract prices from Lion Travel search results text."""
    price = PriceInfo()

    price_pattern = r"TWD\s*([\d,]+)"
    matches = re.findall(price_pattern, raw_text)
    if matches:
        prices = sorted(set(int(p.replace(",", "")) for p in matches))
        # Filter out very small amounts (likely fees/taxes)
        prices = [p for p in prices if p > 10000]
        if prices:
            price.per_person = prices[0]
            price.currency = "TWD"

    return price


def _parse_detail_prices(raw_text: str) -> PriceInfo:
    """Extract pricing from Lion Travel detail page."""
    price = PriceInfo()

    # Total price
    total_match = re.search(r"總金額[^\d]*TWD\s*([\d,]+)", raw_text)
    if total_match:
        price.total = int(total_match.group(1).replace(",", ""))

    # Per person
    pp_match = re.search(r"TWD\s*([\d,]+)\s*人/起", raw_text)
    if pp_match:
        price.per_person = int(pp_match.group(1).replace(",", ""))

    if price.per_person is None and price.total is None:
        # Fallback: find any TWD amount
        all_prices = re.findall(r"TWD\s*([\d,]+)", raw_text)
        prices = sorted(
            set(int(p.replace(",", "")) for p in all_prices if int(p.replace(",", "")) > 10000)
        )
        if prices:
            price.per_person = prices[0]

    price.currency = "TWD"
    return price


def _parse_dates_from_url(url: str) -> DatesInfo:
    """Extract date info from Lion Travel search URL parameters."""
    dates = DatesInfo()

    from_match = re.search(r"FromDate=(\d{8})", url)
    to_match = re.search(r"ToDate=(\d{8})", url)
    days_match = re.search(r"Days=(\d+)", url)

    if from_match:
        d = from_match.group(1)
        dates.departure_date = f"{d[:4]}-{d[4:6]}-{d[6:8]}"
    if to_match:
        d = to_match.group(1)
        dates.return_date = f"{d[:4]}-{d[4:6]}-{d[6:8]}"
    if days_match:
        dates.duration_days = int(days_match.group(1))

    return dates


def _parse_search_flight(raw_text: str) -> FlightInfo:
    """Extract flight info from Lion Travel search page text.

    Typical patterns in raw text:
      泰國獅子航空SL396 09:00→12:30
      酷航TR874 13:55→18:00
      華航CI152 08:00→12:05
    """
    flight = FlightInfo()

    # Known airline name → IATA code mapping (Chinese → code)
    airline_map = {
        "泰國獅子航空": "SL",
        "酷航": "TR",
        "樂桃航空": "MM",
        "華航": "CI",
        "長榮": "BR",
        "星宇": "JX",
        "虎航": "IT",
        "台灣虎航": "IT",
        "亞洲航空": "AK",
        "捷星": "3K",
        "國泰航空": "CX",
        "日本航空": "JL",
        "全日空": "NH",
        "泰越捷": "VZ",
    }

    # Build airline alternation pattern
    airline_names = "|".join(re.escape(name) for name in airline_map)

    # Pattern: airline_name + flight_number + times
    # e.g. "泰國獅子航空SL396 09:00→12:30" or "酷航TR874 13:55-18:00"
    flight_pattern = (
        rf"({airline_names})\s*([A-Z]{{2}}\d{{2,4}})\s*"
        rf"(\d{{1,2}}:\d{{2}})\s*[→\->～~]+\s*(\d{{1,2}}:\d{{2}})"
    )

    matches = re.findall(flight_pattern, raw_text)

    if matches:
        # First match = outbound, second = return
        airline_zh, flight_num, dep_time, arr_time = matches[0]
        airline_code = airline_map.get(airline_zh, "")
        flight.outbound = FlightSegment(
            airline=airline_zh,
            airline_code=airline_code,
            flight_number=flight_num,
            departure_time=dep_time,
            arrival_time=arr_time,
        )

        if len(matches) >= 2:
            airline_zh2, flight_num2, dep_time2, arr_time2 = matches[1]
            airline_code2 = airline_map.get(airline_zh2, "")
            flight.return_ = FlightSegment(
                airline=airline_zh2,
                airline_code=airline_code2,
                flight_number=flight_num2,
                departure_time=dep_time2,
                arrival_time=arr_time2,
            )

    return flight


def _parse_search_hotel(raw_text: str) -> HotelInfo:
    """Extract hotel info from Lion Travel search page text.

    Typical patterns:
      捷絲旅大阪心齋橋館  (Just Sleep Osaka Shinsaibashi)
      APA Hotel京都駅前    (APA Hotel Kyoto Ekimae)
      TAVINOS 濱松町       (TAVINOS Hamamatsucho)

    LionTravel search pages list hotel names near package cards.
    We look for known hotel brand patterns in the raw text.
    """
    hotel = HotelInfo()

    # Known hotel brand patterns (Chinese + English mixed)
    hotel_patterns = [
        # Japanese/Taiwanese hotel chains
        r"捷絲旅[^\s,，、\n]{2,15}",       # Just Sleep + location
        r"APA\s*Hotel[^\s,，、\n]{2,20}",   # APA Hotel + location
        r"APA\s*ホテル[^\s,，、\n]{2,20}",  # APA Hotel (JP) + location
        r"TAVINOS\s*[^\s,，、\n]{2,15}",    # TAVINOS + location
        r"東橫INN[^\s,，、\n]{2,20}",       # Toyoko Inn + location
        r"東横INN[^\s,，、\n]{2,20}",       # Toyoko Inn alt char
        r"Vessel\s*Hotel[^\s,，、\n]{2,20}",
        r"Dormy\s*Inn[^\s,，、\n]{2,20}",
        r"Super\s*Hotel[^\s,，、\n]{2,20}",
        r"スーパーホテル[^\s,，、\n]{2,20}",
        r"ホテルタビノス[^\s,，、\n]{2,15}",
        r"御宿野乃[^\s,，、\n]{2,15}",      # Onyado Nono
        r"リッチモンドホテル[^\s,，、\n]{2,20}",
        # Generic: "XX飯店" or "XXホテル" (Chinese/Japanese for "hotel")
        r"[A-Za-z\u4e00-\u9fff]{2,10}飯店",
        r"[A-Za-z\u4e00-\u9fff]{2,10}酒店",
    ]

    hotel_names = []
    for pattern in hotel_patterns:
        found = re.findall(pattern, raw_text)
        for name in found:
            name = name.strip()
            if name and name not in hotel_names and len(name) >= 4:
                hotel_names.append(name)

    if hotel_names:
        hotel.name = hotel_names[0]
        hotel.names = hotel_names

    return hotel


async def _extract_packages_from_dom(page) -> list[dict]:
    """Extract package cards from the DOM."""
    packages = []

    package_selectors = [
        ".product-card",
        ".search-result-item",
        "[class*='product-item']",
        ".vacation-item",
        "article",
        ".card",
    ]

    price_pattern = r"TWD\s*([\d,]+)"

    for selector in package_selectors:
        elements = await page.query_selector_all(selector)
        if not elements:
            continue

        print(f"Found {len(elements)} elements with selector: {selector}")
        for i, el in enumerate(elements[:10]):
            try:
                item_text = await el.inner_text()
                if "TWD" not in item_text and "自由行" not in item_text:
                    continue

                item_prices = re.findall(price_pattern, item_text)

                title = ""
                title_el = await el.query_selector(
                    "h2, h3, .title, [class*='title'], a"
                )
                if title_el:
                    title = await title_el.inner_text()

                link = ""
                link_el = await el.query_selector("a[href]")
                if link_el:
                    link = await link_el.get_attribute("href")

                packages.append({
                    "index": i,
                    "title": title.strip()[:100] if title else "",
                    "prices_found": item_prices[:3],
                    "link": link,
                    "text_preview": item_text[:300],
                })
            except Exception:
                continue
        break  # Found working selector

    return packages
