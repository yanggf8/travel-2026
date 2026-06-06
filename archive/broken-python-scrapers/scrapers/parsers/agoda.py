"""
Agoda Parser (agoda.com)

Extracts hotel details and pricing from direct Agoda hotel page URLs.

Note: Agoda search pages return empty results for far-future dates and
sometimes error out. Direct hotel URLs work reliably and return full
pricing, reviews, and amenity information.
"""

from __future__ import annotations

import re

from ..base import BaseScraper, scroll_page
from ..schema import (
    ScrapeResult, HotelInfo, PriceInfo, DatesInfo,
)


def build_hotel_url(
    hotel_slug: str,
    city_slug: str,
    country: str = "jp",
    check_in: str = "",
    nights: int = 4,
    adults: int = 2,
    rooms: int = 1,
    currency: str = "TWD",
) -> str:
    """
    Build an Agoda direct hotel URL.

    Args:
        hotel_slug: Hotel URL slug (e.g., "cross-hotel-osaka")
        city_slug: City slug (e.g., "osaka")
        country: Country code (e.g., "jp")
        check_in: Check-in date YYYY-MM-DD
        nights: Number of nights
        adults: Number of adults
        rooms: Number of rooms
        currency: Currency code
    """
    url = f"https://www.agoda.com/{hotel_slug}/hotel/{city_slug}-{country}.html"
    params = []
    if check_in:
        params.append(f"checkIn={check_in}")
    params.append(f"los={nights}")
    params.append(f"adults={adults}")
    params.append(f"rooms={rooms}")
    params.append(f"currency={currency}")
    return url + "?" + "&".join(params)


def build_search_url(
    city_id: int,
    check_in: str,
    check_out: str,
    adults: int = 2,
    rooms: int = 1,
    currency: str = "TWD",
) -> str:
    """
    Build an Agoda search URL.

    Known city IDs:
        Osaka: 14811
        Tokyo: 5765
        Kyoto: 5814
        Nagoya: 17285

    Note: Search may return 0 results for dates far in the future.
    """
    return (
        f"https://www.agoda.com/search"
        f"?city={city_id}"
        f"&checkIn={check_in}"
        f"&checkOut={check_out}"
        f"&rooms={rooms}"
        f"&adults={adults}"
        f"&currency={currency}"
    )


# Known Agoda city IDs
CITY_IDS = {
    "osaka": 14811,
    "tokyo": 5765,
    "kyoto": 5814,
    "nagoya": 17285,
    "sapporo": 10570,
    "fukuoka": 5788,
    "okinawa": 17074,
}


class AgodaParser(BaseScraper):
    source_id = "agoda"

    async def prepare_page(self, page, url: str) -> None:
        """Wait for Agoda SPA to render hotel content."""
        # Agoda is a heavy SPA — wait longer
        await page.wait_for_timeout(10000)

        # Scroll to trigger lazy content
        await scroll_page(page, steps=5, step_delay_ms=500, final_delay_ms=2000)

    def parse_raw_text(self, raw_text: str, url: str = "", **kwargs) -> ScrapeResult:
        """Parse Agoda hotel page text."""
        result = ScrapeResult(source_id=self.source_id, url=url)

        result.hotel = _parse_hotel(raw_text)
        result.price = _parse_price(raw_text)
        result.dates = _parse_dates(raw_text, url)

        return result


# ---------------------------------------------------------------------------
# Pure parsing functions
# ---------------------------------------------------------------------------

def _parse_hotel(raw_text: str) -> HotelInfo:
    """Parse hotel info from Agoda page text."""
    hotel = HotelInfo()

    lines = [l.strip() for l in raw_text.split("\n") if l.strip()]

    # Look for hotel name in parentheses (English name) — most reliable
    for line in lines:
        name_match = re.search(
            r"\(([A-Za-z\s&'-]+(?:Hotel|Inn|Resort|Hostel|House|Suites?)[A-Za-z\s&'-]*)\)",
            line,
        )
        if name_match:
            en_name = name_match.group(1).strip()
            hotel.names.append(en_name)
            # The Chinese name is typically on the same line before the parentheses
            zh_match = re.match(r"^(.+?)\s*\(", line)
            if zh_match:
                zh_name = zh_match.group(1).strip()
                hotel.name = zh_name
                hotel.names.insert(0, zh_name)
            else:
                hotel.name = en_name
            break

    # Look for star rating
    for line in lines:
        star_match = re.search(r"獲得(\d)顆星", line)
        if star_match:
            hotel.star_rating = int(star_match.group(1))
            break

    # If no English name found, try Chinese hotel name pattern
    if not hotel.name:
        for i, line in enumerate(lines):
            if re.search(r"(飯店|酒店|旅館|民宿|青旅)", line) and len(line) > 4:
                # Skip navigation/section headers
                if line not in ("簡介", "旅遊好去處", "設施與服務", "住宿評鑑", "地點", "政策"):
                    hotel.name = line
                    break

    # Extract address / area
    for line in lines:
        if re.search(r"(心齋橋|難波|梅田|日本橋|天王寺|新宿|池袋|澀谷|淺草|銀座|上野)", line):
            # This line likely contains area info
            hotel.area = line[:100]
            break

    # Extract amenities
    amenity_keywords = [
        "免費Wi-Fi", "WiFi", "游泳池", "健身房", "停車場", "機場接駁",
        "早餐", "餐廳", "溫泉", "大浴場", "洗衣", "行李寄存",
    ]
    for kw in amenity_keywords:
        if kw in raw_text:
            hotel.amenities.append(kw)

    return hotel


def _parse_price(raw_text: str) -> PriceInfo:
    """Parse price info from Agoda page text."""
    price = PriceInfo(currency="TWD")

    # Look for price patterns: NT$ X,XXX or TWD X,XXX
    price_matches = re.findall(r"NT\$\s*([\d,]+)", raw_text)
    if not price_matches:
        price_matches = re.findall(r"TWD\s*([\d,]+)", raw_text)

    if price_matches:
        prices = sorted(set(int(p.replace(",", "")) for p in price_matches))
        # Filter out very small amounts (ratings, review counts, etc.)
        room_prices = [p for p in prices if p > 500]
        if room_prices:
            price.per_person = room_prices[0]  # Cheapest room per night

    return price


def _parse_dates(raw_text: str, url: str = "") -> DatesInfo:
    """Parse date info from Agoda page text and URL."""
    dates = DatesInfo()

    # Extract from URL params
    checkin_match = re.search(r"checkIn=(\d{4}-\d{2}-\d{2})", url)
    if checkin_match:
        dates.departure_date = checkin_match.group(1)

    checkout_match = re.search(r"checkOut=(\d{4}-\d{2}-\d{2})", url)
    if checkout_match:
        dates.return_date = checkout_match.group(1)

    los_match = re.search(r"los=(\d+)", url)
    if los_match:
        dates.duration_nights = int(los_match.group(1))

    # Extract from text
    duration_match = re.search(r"(\d+)\s*晚", raw_text)
    if duration_match and not dates.duration_nights:
        dates.duration_nights = int(duration_match.group(1))

    return dates
