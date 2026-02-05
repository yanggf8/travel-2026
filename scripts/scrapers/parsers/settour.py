"""
Settour Parser (tour.settour.com.tw)

Extracts flight info, hotel, pricing, dates, itinerary, and inclusions
from Settour (東南旅遊) package pages.
"""

from __future__ import annotations

import re

from ..base import BaseScraper, scroll_page
from ..schema import (
    ScrapeResult, FlightInfo, FlightSegment, HotelInfo,
    PriceInfo, DatesInfo, ItineraryDay,
)


class SettourParser(BaseScraper):
    source_id = "settour"

    async def prepare_page(self, page, url: str) -> None:
        """Click Settour tabs to load details."""
        await scroll_page(page)

        print("Settour detected: clicking tabs to load details...")
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

    def parse_raw_text(self, raw_text: str, url: str = "", **kwargs) -> ScrapeResult:
        """Parse Settour page text into structured data."""
        result = ScrapeResult(source_id=self.source_id, url=url)

        result.flight = _parse_flights(raw_text)
        result.hotel = _parse_hotel(raw_text)
        result.price = _parse_price(raw_text)
        result.dates = _parse_dates(raw_text)
        result.itinerary = _parse_itinerary(raw_text)
        result.inclusions = _parse_inclusions(raw_text)

        return result


# ---------------------------------------------------------------------------
# Pure parsing functions
# ---------------------------------------------------------------------------

def _parse_flights(raw_text: str) -> FlightInfo:
    """Parse Settour flight details from raw page text."""
    flight_info = FlightInfo()
    lines = raw_text.split("\n")

    for i, line in enumerate(lines):
        line = line.strip()

        if line == "去程" and i + 8 < len(lines):
            flight_info.outbound = _parse_flight_block(lines, i)
        elif line == "回程" and i + 8 < len(lines):
            flight_info.return_ = _parse_flight_block(lines, i)
            break

    # Fallback: scan for flight number patterns
    if not flight_info.outbound.is_populated:
        for i, line in enumerate(lines):
            line = line.strip()
            flight_match = re.search(r"([A-Z]{2})\s*(\d{2,4})", line)
            if flight_match and re.search(
                r"TPE|NRT|HND|KIX|OSA|NGO",
                "\n".join(lines[max(0, i - 5) : i + 5]),
            ):
                nearby = "\n".join(lines[max(0, i - 5) : i + 5])
                airports = re.findall(
                    r"(TPE|NRT|HND|KIX|OSA|NGO|CTS|FUK|OKA)", nearby
                )
                times = re.findall(r"(\d{2}:\d{2})", nearby)
                if airports and times:
                    target = (
                        flight_info.outbound
                        if not flight_info.outbound.is_populated
                        else flight_info.return_
                    )
                    target.flight_number = (
                        flight_match.group(1) + flight_match.group(2)
                    )
                    target.departure_code = airports[0] if airports else ""
                    target.arrival_code = airports[1] if len(airports) > 1 else ""
                    target.departure_time = times[0] if times else ""
                    target.arrival_time = times[1] if len(times) > 1 else ""
                    if (
                        flight_info.outbound.is_populated
                        and flight_info.return_.is_populated
                    ):
                        break

    return flight_info


def _parse_flight_block(lines: list[str], start: int) -> FlightSegment:
    """Parse a single flight block (去程 or 回程) from Settour text."""
    segment = FlightSegment()
    nearby = lines[start : min(start + 12, len(lines))]
    text = "\n".join(nearby)

    # Date
    date_match = re.search(r"(\d{4})[/-](\d{1,2})[/-](\d{1,2})", text)
    if date_match:
        segment.date = (
            f"{date_match.group(1)}/{date_match.group(2).zfill(2)}"
            f"/{date_match.group(3).zfill(2)}"
        )

    # Flight number
    flight_match = re.search(r"([A-Z]{2})\s*(\d{2,4})", text)
    if flight_match:
        segment.flight_number = flight_match.group(1) + flight_match.group(2)

    # Airports
    airports = re.findall(r"(TPE|NRT|HND|KIX|OSA|NGO|CTS|FUK|OKA)", text)
    if len(airports) >= 2:
        segment.departure_code = airports[0]
        segment.arrival_code = airports[1]

    # Times
    times = re.findall(r"(\d{2}:\d{2})", text)
    if len(times) >= 2:
        segment.departure_time = times[0]
        segment.arrival_time = times[1]

    # Airline name
    airline_match = re.search(
        r"(中華航空|長榮航空|星宇航空|台灣虎航|樂桃航空|酷航|捷星|亞洲航空)", text
    )
    if airline_match:
        segment.airline = airline_match.group(1)

    return segment


def _parse_hotel(raw_text: str) -> HotelInfo:
    """Parse Settour hotel details from raw page text."""
    hotel = HotelInfo()
    lines = raw_text.split("\n")
    in_hotel_section = False

    for line in lines:
        line = line.strip()

        if line in ("飯店安排", "住宿安排", "住宿"):
            in_hotel_section = True
            continue

        if in_hotel_section and line:
            if line in ("每日行程", "航班資訊", "出發日期", "費用說明", "注意事項"):
                break

            if re.search(
                r"(Hotel|飯店|酒店|旅館|Inn|Resort|HOTEL)", line, re.IGNORECASE
            ):
                names = re.split(r"或\s*同級|或\s*", line)
                for name in names:
                    name = re.sub(r"\s*\([^)]*\)", "", name).strip()
                    name = re.sub(r"(或同級|同級)", "", name).strip()
                    if name and len(name) > 2:
                        hotel.names.append(name)

    if hotel.names:
        hotel.name = hotel.names[0]

    # Extract area
    for line in raw_text.split("\n"):
        m = re.search(r"(地區|區域)[:：]\s*(.+)$", line)
        if m:
            hotel.area = m.group(2).strip()
            break

    return hotel


def _parse_price(raw_text: str) -> PriceInfo:
    """Parse Settour price details from raw page text."""
    price = PriceInfo()

    # Pattern: 售價 NT$XX,XXX
    price_matches = re.findall(
        r"(?:售價|團費|價格)\s*(?:NT)?\$\s*([\d,]+)", raw_text
    )
    if price_matches:
        prices = [int(p.replace(",", "")) for p in price_matches]
        prices = [p for p in prices if p > 10000]
        if prices:
            price.per_person = min(prices)
            price.currency = "TWD"

    # Fallback: any NT$ amount > 15000
    if price.per_person is None:
        all_prices = re.findall(r"NT?\$\s*([\d,]+)", raw_text)
        prices = sorted(
            set(
                int(p.replace(",", ""))
                for p in all_prices
                if int(p.replace(",", "")) > 15000
            )
        )
        if prices:
            price.per_person = prices[0]
            price.currency = "TWD"

    # Deposit
    deposit_match = re.search(r"訂金\s*(?:NT)?\$\s*([\d,]+)", raw_text)
    if deposit_match:
        price.deposit = int(deposit_match.group(1).replace(",", ""))

    return price


def _parse_dates(raw_text: str) -> DatesInfo:
    """Parse Settour travel dates from raw page text."""
    dates = DatesInfo()

    duration_match = re.search(r"(\d+)\s*天\s*(\d+)\s*夜", raw_text)
    if duration_match:
        dates.duration_days = int(duration_match.group(1))
        dates.duration_nights = int(duration_match.group(2))

    depart_match = re.search(
        r"出發日期\s*[:：]?\s*(\d{4})[/-](\d{1,2})[/-](\d{1,2})", raw_text
    )
    if depart_match:
        dates.year = int(depart_match.group(1))
        dates.departure_month = int(depart_match.group(2))
        dates.departure_day = int(depart_match.group(3))

    return dates


def _parse_itinerary(raw_text: str) -> list[ItineraryDay]:
    """Parse daily itinerary from Settour page text."""
    itinerary: list[ItineraryDay] = []
    lines = raw_text.split("\n")

    current_day = None
    current_content: list[str] = []

    for line in lines:
        line = line.strip()

        day_match = re.match(r"^(?:Day|DAY|第)\s*(\d+)\s*(?:天)?$", line)
        if day_match:
            if current_day is not None:
                _append_day(itinerary, current_day, current_content)
            current_day = int(day_match.group(1))
            current_content = []
            continue

        if current_day is not None:
            if line.startswith("注意事項") or line.startswith("出團備註"):
                break
            current_content.append(line)

    if current_day is not None and current_content:
        _append_day(itinerary, current_day, current_content)

    return itinerary


def _append_day(
    itinerary: list[ItineraryDay], day_num: int, content_lines: list[str]
):
    content_text = " ".join(content_lines)
    itinerary.append(
        ItineraryDay(
            day=day_num,
            content=content_text[:500],
            is_free=any(
                kw in content_text for kw in ["自由活動", "全日自由", "自由前往"]
            ),
            is_guided=any(
                kw in content_text
                for kw in ["奈良", "京都", "嵐山", "伏見", "清水寺"]
            ),
        )
    )


def _parse_inclusions(raw_text: str) -> list[str]:
    """Parse inclusions from Settour text."""
    inclusions = []
    text = raw_text.replace(" ", "")

    if "含團險" in text or "旅行業責任保險" in text:
        inclusions.append("travel_insurance")
    if "含機場稅" in text or "含國內外機場稅" in text or "兩地機場稅" in text:
        inclusions.append("airport_tax")
    if "早餐" in text and (
        "飯店內用" in text or "含早餐" in text or "飯店早餐" in text
    ):
        inclusions.append("breakfast")

    return inclusions
