"""
Travel4U Parser (travel4u.com.tw / 山富旅遊)

Extracts flight info, hotel, pricing, dates, itinerary from Travel4U package pages.

URL Patterns:
- Listing: https://www.travel4u.com.tw/group/area/{area_code}/japan/
- Product: https://www.travel4u.com.tw/group/product/{product_code}/

Area Codes:
- 39: 北海道 (Hokkaido) - 高雄出發
- 40: 大阪/關西/四國 (Kansai)
- 41: 東京/關東/東北 (Tokyo/Kanto)
- 42: 九州 (Kyushu)
- 43: 沖繩 (Okinawa)
- 49: 名古屋/中部 (Nagoya/Chubu)
"""

from __future__ import annotations

import re
from typing import Optional

from ..base import BaseScraper
from ..schema import (
    ScrapeResult, FlightInfo, FlightSegment, HotelInfo,
    PriceInfo, DatesInfo, ItineraryDay,
)


class Travel4UParser(BaseScraper):
    source_id = "travel4u"

    def parse_raw_text(self, raw_text: str, url: str = "", **kwargs) -> ScrapeResult:
        """Parse Travel4U page text into structured data."""
        result = ScrapeResult(source_id=self.source_id, url=url)

        # Extract product code from URL
        code_match = re.search(r"/product/([A-Z0-9]+)", url)
        if code_match:
            result.product_code = code_match.group(1)

        result.flight = _parse_flights(raw_text)
        result.hotel = _parse_hotel(raw_text)
        result.price = _parse_price(raw_text)
        result.dates = _parse_dates(raw_text)
        result.itinerary = _parse_itinerary(raw_text)
        result.inclusions = _parse_inclusions(raw_text)
        result.package_type = _classify_package_type(raw_text, url)

        # Extract title
        result.title = _extract_title(raw_text)

        return result


# ---------------------------------------------------------------------------
# Area code mapping
# ---------------------------------------------------------------------------

AREA_CODES = {
    "hokkaido": "39",
    "kansai": "40",
    "osaka": "40",
    "tokyo": "41",
    "kanto": "41",
    "kyushu": "42",
    "okinawa": "43",
    "nagoya": "49",
    "chubu": "49",
}

DEPARTURE_CODES = {
    "taipei": "tpe",
    "taichung": "txg",
    "kaohsiung": "khh",
}


def build_listing_url(destination: str, departure: str = "taipei") -> str:
    """Build Travel4U listing URL for a destination."""
    area_code = AREA_CODES.get(destination.lower(), "40")
    return f"https://www.travel4u.com.tw/group/area/{area_code}/japan/"


# ---------------------------------------------------------------------------
# Pure parsing functions
# ---------------------------------------------------------------------------

def _extract_title(raw_text: str) -> str:
    """Extract package title from raw text."""
    lines = raw_text.split("\n")
    for i, line in enumerate(lines):
        line = line.strip()
        # Look for title pattern: "XXX５日－..." or similar
        if re.match(r".+[５６４７]\s*日.*[－\-]", line):
            return line[:200]
        # Or lines with day count
        if "日－" in line and len(line) > 20:
            return line[:200]
    return ""


def _parse_flights(raw_text: str) -> FlightInfo:
    """Parse Travel4U flight details from raw page text."""
    flight_info = FlightInfo()

    # Map Chinese airports to codes
    airport_map = {
        "高雄小港機場": "KHH",
        "桃園機場": "TPE",
        "台中機場": "TXG",
        "千歲": "CTS",
        "成田": "NRT",
        "羽田": "HND",
        "關西": "KIX",
        "那霸": "OKA",
        "福岡": "FUK",
    }

    # Map Chinese airlines to codes
    airline_map = {
        "泰國獅航": "SL",
        "華航": "CI",
        "長榮": "BR",
        "星宇": "JX",
        "虎航": "IT",
        "樂桃": "MM",
        "酷航": "TR",
        "捷星": "3K",
        "國泰": "CX",
        "全日空": "NH",
        "日航": "JL",
        "亞洲航空": "AK",
    }

    lines = raw_text.split("\n")
    
    # State for current segment being parsed
    current_day = None
    current_airline = ""
    current_airline_code = ""
    current_flight_num = ""
    in_departure = False
    in_arrival = False
    departure_code = ""
    departure_time = ""
    arrival_code = ""
    arrival_time = ""

    for line in lines:
        line = line.strip()
        
        # Detect Day marker
        day_match = re.match(r"Day\s*(\d+)", line)
        if day_match:
            current_day = int(day_match.group(1))
            # Reset segment state for new day
            current_airline = ""
            current_airline_code = ""
            current_flight_num = ""
            in_departure = False
            in_arrival = False
            departure_code = ""
            departure_time = ""
            arrival_code = ""
            arrival_time = ""
            continue
        
        # Detect airline (exact match)
        if line in airline_map:
            current_airline = line
            current_airline_code = airline_map[line]
            continue
        
        # Detect flight number (exact match for pattern like SL392)
        if re.match(r"^[A-Z]{1,2}\d{2,4}$", line):
            current_flight_num = line
            continue
        
        # Detect Departure/Arrival markers
        if line == "Departure":
            in_departure = True
            in_arrival = False
            continue
        if line == "Arrival":
            in_arrival = True
            in_departure = False
            continue
        
        # Extract airport (when in Departure or Arrival mode)
        if in_departure or in_arrival:
            for airport_cn, code in airport_map.items():
                if airport_cn in line:
                    if in_departure:
                        departure_code = code
                    elif in_arrival:
                        arrival_code = code
                    break
        
        # Extract time (when in Departure or Arrival mode)
        if re.match(r"^\d{1,2}:\d{2}$", line):
            if in_departure and not departure_time:
                departure_time = line
            elif in_arrival and not arrival_time:
                arrival_time = line
        
        # Check if we have a complete segment
        if (current_flight_num and departure_code and arrival_code and
                departure_time and arrival_time and current_day):
            segment = FlightSegment(
                airline=current_airline,
                airline_code=current_airline_code,
                flight_number=current_flight_num,
                departure_code=departure_code,
                departure_time=departure_time,
                arrival_code=arrival_code,
                arrival_time=arrival_time,
            )
            
            # Day 1 = outbound, higher day = return
            if current_day == 1 and not flight_info.outbound.is_populated:
                flight_info.outbound = segment
            elif current_day > 1 and not flight_info.return_.is_populated:
                flight_info.return_ = segment
            
            # Reset segment state (keep day)
            current_airline = ""
            current_airline_code = ""
            current_flight_num = ""
            in_departure = False
            in_arrival = False
            departure_code = ""
            departure_time = ""
            arrival_code = ""
            arrival_time = ""

    return flight_info


def _parse_hotel(raw_text: str) -> HotelInfo:
    """Parse Travel4U hotel details from raw page text."""
    hotel = HotelInfo()

    # Look for hotel patterns
    hotel_patterns = [
        r"([\w\s]+(?:酒店|飯店|Hotel|Inn|Resort|HOTEL)[\w\s]*)",
        r"住宿[：:]\s*([\w\s]+)",
    ]

    for pattern in hotel_patterns:
        matches = re.findall(pattern, raw_text, re.IGNORECASE)
        for match in matches:
            name = match.strip()
            if len(name) > 3 and "同級" not in name:
                hotel.names.append(name)

    if hotel.names:
        hotel.name = hotel.names[0]

    return hotel


def _parse_price(raw_text: str) -> PriceInfo:
    """Parse Travel4U price details from raw page text."""
    price = PriceInfo()

    # Look for price patterns: NT$ XX,XXX or $XX,XXX
    # Travel4U shows prices like "售價: NT$ 43,900"
    price_match = re.search(r"售價[：:]\s*NT?\$?\s*([\d,]+)", raw_text)
    if price_match:
        price.per_person = int(price_match.group(1).replace(",", ""))
        price.currency = "TWD"

    # Also look for calendar prices
    cal_prices = re.findall(r"(\d{2}/\d{2})\s*\n?\s*([\d,]+)\s*起?", raw_text)
    if cal_prices:
        prices = [int(p[1].replace(",", "")) for p in cal_prices if int(p[1].replace(",", "")) > 10000]
        if prices and not price.per_person:
            price.per_person = min(prices)
            price.currency = "TWD"

    # Look for availability
    avail_match = re.search(r"可售名額[：:]\s*(\d+)", raw_text)
    if avail_match:
        price.seats_available = int(avail_match.group(1))

    return price


def _parse_dates(raw_text: str) -> DatesInfo:
    """Parse Travel4U travel dates from raw page text."""
    dates = DatesInfo()

    # Duration: 5天4夜 or ５日
    duration_match = re.search(r"([５６４７\d]+)\s*[天日]", raw_text)
    if duration_match:
        day_str = duration_match.group(1)
        # Convert full-width to half-width
        day_str = day_str.replace("５", "5").replace("６", "6").replace("４", "4").replace("７", "7")
        dates.duration_days = int(day_str)
        dates.duration_nights = dates.duration_days - 1

    # Look for departure dates: 2026/02/23 or 02/23
    date_matches = re.findall(r"(\d{4})/(\d{2})/(\d{2})", raw_text)
    if date_matches:
        dates.year = int(date_matches[0][0])
        dates.departure_month = int(date_matches[0][1])
        dates.departure_day = int(date_matches[0][2])
        dates.departure_date = f"{dates.year:04d}-{dates.departure_month:02d}-{dates.departure_day:02d}"

    return dates


def _parse_itinerary(raw_text: str) -> list[ItineraryDay]:
    """Parse daily itinerary from raw page text."""
    itinerary = []

    # Split by Day markers
    day_sections = re.split(r"\bDay\s*(\d+)\b", raw_text)

    i = 1
    while i < len(day_sections) - 1:
        day_num = int(day_sections[i])
        content = day_sections[i + 1]

        # Truncate at next section marker
        content = re.split(r"(?:額外費用|注意事項|行程特色|參考航班)", content)[0]
        content = content[:1000]

        is_free = any(kw in content for kw in ["自由活動", "全日自由", "自由行程"])

        itinerary.append(
            ItineraryDay(
                day=day_num,
                content=content.strip()[:500],
                is_free=is_free,
            )
        )
        i += 2

    return itinerary


def _parse_inclusions(raw_text: str) -> list[str]:
    """Parse inclusions from Travel4U text."""
    inclusions = []
    text = raw_text.replace(" ", "")

    if "含團險" in text or "旅遊責任險" in text:
        inclusions.append("travel_insurance")
    if "含機場稅" in text or "含稅" in text:
        inclusions.append("airport_tax")
    if "早餐" in text and ("飯店" in text or "含早" in text):
        inclusions.append("breakfast")

    return inclusions


def _classify_package_type(raw_text: str, url: str) -> str:
    """Classify Travel4U package type."""
    # Check URL first - /group/ indicates group tour
    if "/group/" in url:
        return "group"
    
    # FIT URL patterns
    if "/fit/" in url or "/free/" in url:
        return "fit"
    
    # Content-based classification (less reliable due to nav menu text)
    # Group tour indicators (stronger signal)
    group_indicators = ["團體旅遊", "跟團旅遊", "領隊全程", "導遊服務", "參團人數"]
    if any(kw in raw_text for kw in group_indicators):
        return "group"
    
    # FIT indicators (check in context, not nav menu)
    # These need to appear in context, not just in navigation
    fit_pattern = re.search(r"(自由行|機加酒|自由配).{0,20}(套餐|行程|方案)", raw_text)
    if fit_pattern:
        return "fit"
    
    # Default: Travel4U is primarily group tours
    return "group"


# ---------------------------------------------------------------------------
# Listing page parsing
# ---------------------------------------------------------------------------

def parse_listing_item(item_text: str, item_url: str) -> Optional[dict]:
    """Parse a single listing item."""
    result = {
        "url": item_url,
        "source_id": "travel4u",
    }

    # Extract product code
    code_match = re.search(r"/product/([A-Z0-9]+)", item_url)
    if code_match:
        result["product_code"] = code_match.group(1)

    # Extract title (first substantial line)
    lines = [l.strip() for l in item_text.split("\n") if l.strip()]
    for line in lines:
        if len(line) > 10 and ("日" in line or "天" in line):
            result["title"] = line[:200]
            break

    # Extract price
    price_match = re.search(r"\$\s*([\d,]+)\s*起?", item_text)
    if price_match:
        result["price"] = int(price_match.group(1).replace(",", ""))

    # Extract dates
    date_matches = re.findall(r"(\d{2}/\d{2})", item_text)
    if date_matches:
        result["departure_dates"] = date_matches[:5]

    # Extract duration
    duration_match = re.search(r"(\d+)\s*天", item_text)
    if duration_match:
        result["duration_days"] = int(duration_match.group(1))

    return result if "title" in result or "product_code" in result else None
