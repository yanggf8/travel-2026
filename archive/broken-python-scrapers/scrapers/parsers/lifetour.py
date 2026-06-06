"""
Lifetour Parser (tour.lifetour.com.tw)

Extracts flight info, hotel, pricing, dates, itinerary, and inclusions
from Lifetour package pages.
"""

from __future__ import annotations

import re

from ..base import BaseScraper
from ..schema import (
    ScrapeResult, FlightInfo, FlightSegment, HotelInfo,
    PriceInfo, DatesInfo, ItineraryDay,
)


class LifetourParser(BaseScraper):
    source_id = "lifetour"

    def parse_raw_text(self, raw_text: str, url: str = "", **kwargs) -> ScrapeResult:
        """Parse Lifetour page text into structured data."""
        result = ScrapeResult(source_id=self.source_id, url=url)

        result.flight = _parse_flights(raw_text)
        result.hotel = _parse_hotel(raw_text)
        result.price = _parse_price(raw_text)
        result.dates = _parse_dates(raw_text)
        result.itinerary = _parse_itinerary(raw_text)
        result.inclusions = _parse_inclusions(raw_text)
        result.package_type = _classify_package_type(raw_text, url)

        return result


# ---------------------------------------------------------------------------
# Pure parsing functions
# ---------------------------------------------------------------------------

def _parse_flights(raw_text: str) -> FlightInfo:
    """Parse Lifetour flight details from raw page text."""
    flight_info = FlightInfo()
    lines = raw_text.split("\n")

    for i, line in enumerate(lines):
        line = line.strip()

        # Look for flight number pattern like "亞洲航空D7378"
        flight_match = re.search(
            r"(亞洲航空|華航|長榮|星宇|虎航|樂桃|酷航|捷星)([A-Z]{1,2}\d{2,4})", line
        )
        if flight_match and i >= 5:
            airline = flight_match.group(1)
            flight_num = flight_match.group(2)

            # Check previous lines for departure/arrival info
            prev_lines = lines[max(0, i - 8) : i + 1]
            prev_text = "\n".join(prev_lines)
            times = re.findall(
                r"(\d{2}/\d{2})\([一二三四五六日]\)\s*(\d{1,2}:\d{2})", prev_text
            )
            airports = re.findall(
                r"(TPE|NRT|HND|KIX|OSA|NGO|CTS|FUK|OKA)", prev_text
            )

            if len(times) >= 2 and len(airports) >= 2:
                segment = FlightSegment(
                    date=times[0][0],
                    departure_time=times[0][1],
                    arrival_time=times[1][1],
                    airline=airline,
                    flight_number=flight_num,
                    departure_code=airports[0],
                    arrival_code=airports[1],
                )

                if not flight_info.outbound.is_populated:
                    flight_info.outbound = segment
                elif not flight_info.return_.is_populated:
                    flight_info.return_ = segment

    return flight_info


def _parse_hotel(raw_text: str) -> HotelInfo:
    """Parse Lifetour hotel details from raw page text."""
    hotel = HotelInfo()
    lines = raw_text.split("\n")
    in_hotel_section = False

    for line in lines:
        line = line.strip()

        if line == "住宿":
            in_hotel_section = True
            continue

        if in_hotel_section and line:
            # Extract hotel names - usually contains "或" (or) separator
            if "或" in line and (
                "酒店" in line
                or "飯店" in line
                or "Hotel" in line
                or "Inn" in line
                or "GRAND" in line
            ):
                names = re.split(r"或\s*", line)
                for name in names:
                    name = name.strip()
                    clean_name = re.sub(r"\s*\([^)]*\)", "", name).strip()
                    if clean_name and len(clean_name) > 2 and "同級" not in clean_name:
                        hotel.names.append(clean_name)

                # Extract room type
                room_match = re.search(
                    r"(SEMI DOUBLE|TWN|TWIN|DBL|DOUBLE|單人房|雙人房)",
                    line,
                    re.IGNORECASE,
                )
                if room_match:
                    hotel.room_type = room_match.group(1)

                # Extract bed width
                bed_match = re.search(r"床寬(\d+)CM", line, re.IGNORECASE)
                if bed_match:
                    hotel.bed_width_cm = int(bed_match.group(1))

                break

            if line in ("餐食", "收合景點", "Day"):
                in_hotel_section = False

    if hotel.names:
        hotel.name = hotel.names[0]

    return hotel


def _parse_price(raw_text: str) -> PriceInfo:
    """Parse Lifetour price details from raw page text."""
    price = PriceInfo()

    # Look for price pattern: NT$XX,XXX or $XX,XXX
    price_matches = re.findall(r"NT?\$\s*([\d,]+)\s*元?", raw_text)
    if price_matches:
        prices = [int(p.replace(",", "")) for p in price_matches]
        prices = [p for p in prices if p > 15000]  # Filter out deposits
        if prices:
            price.per_person = min(prices)
            price.currency = "TWD"

    # Look for deposit
    deposit_match = re.search(r"訂金\s*NT?\$\s*([\d,]+)", raw_text)
    if deposit_match:
        price.deposit = int(deposit_match.group(1).replace(",", ""))

    # Look for availability
    avail_match = re.search(r"可售\s*(\d+)\s*人", raw_text)
    if avail_match:
        price.seats_available = int(avail_match.group(1))

    min_match = re.search(r"成行\s*(\d+)\s*人", raw_text)
    if min_match:
        price.min_travelers = int(min_match.group(1))

    return price


def _parse_dates(raw_text: str) -> DatesInfo:
    """Parse Lifetour travel dates from raw page text."""
    dates = DatesInfo()

    # Duration: 5天4夜
    duration_match = re.search(r"(\d+)\s*天\s*(\d+)\s*夜", raw_text)
    if duration_match:
        dates.duration_days = int(duration_match.group(1))
        dates.duration_nights = int(duration_match.group(2))

    # Year from calendar section
    year_match = re.search(r"(\d{4})\s*年\s*(\d{1,2})\s*月", raw_text)
    if year_match:
        dates.year = int(year_match.group(1))

    # Departure date: 出發日期 02月27日 or 2/27
    depart_match = re.search(r"出發日期\s*(\d{1,2})月(\d{1,2})日", raw_text)
    if depart_match:
        dates.departure_month = int(depart_match.group(1))
        dates.departure_day = int(depart_match.group(2))
        
        # Build ISO date if we have year
        if dates.year:
            dates.departure_date = f"{dates.year:04d}-{dates.departure_month:02d}-{dates.departure_day:02d}"

    return dates


def _classify_package_type(raw_text: str, url: str) -> str:
    """Classify Lifetour package type."""
    # Semi-FIT indicators (伴自由 = semi-guided with free time) - treat as FIT
    if any(kw in raw_text for kw in ["伴自由", "半自由", "半自助"]):
        return "fit"  # Treat semi-guided as FIT for filtering purposes
    
    # FIT indicators
    if any(kw in raw_text for kw in ["自由行", "機加酒", "自由配"]):
        return "fit"
    
    # Group tour indicators
    if any(kw in raw_text for kw in ["團體", "跟團", "領隊", "導遊", "迷你小團"]):
        return "group"
    
    return "unknown"


def _parse_itinerary(raw_text: str) -> list[ItineraryDay]:
    """Parse daily itinerary from raw page text."""
    itinerary = []
    lines = raw_text.split("\n")

    current_day = None
    current_content: list[str] = []

    for line in lines:
        line = line.strip()

        day_match = re.match(r"^Day\s*(\d+)$", line)
        if day_match:
            if current_day is not None:
                _append_day(itinerary, current_day, current_content)

            current_day = int(day_match.group(1))
            current_content = []
            continue

        if current_day is not None:
            if line.startswith("出團備註") or line.startswith("看完整資訊"):
                break
            current_content.append(line)

    # Don't forget last day
    if current_day is not None and current_content:
        _append_day(itinerary, current_day, current_content)

    return itinerary


def _append_day(itinerary: list[ItineraryDay], day_num: int, content_lines: list[str]):
    """Build an ItineraryDay and append it."""
    content_text = " ".join(content_lines)
    itinerary.append(
        ItineraryDay(
            day=day_num,
            content=content_text[:500],
            is_free=any(kw in content_text for kw in ["自由活動", "全日自由"]),
            is_guided=any(kw in content_text for kw in ["奈良", "京都", "嵐山", "伏見"]),
        )
    )


def _parse_inclusions(raw_text: str) -> list[str]:
    """Parse inclusions from Lifetour text."""
    inclusions = []
    text = raw_text.replace(" ", "")

    if "含團險" in text:
        inclusions.append("travel_insurance")
    if "含國內外機場稅" in text or "含機場稅" in text:
        inclusions.append("airport_tax")
    if "早餐" in text and ("飯店內用" in text or "含早餐" in text):
        inclusions.append("breakfast")

    return inclusions
