"""
BestTour Parser (besttour.com.tw)

Extracts flight info from 交通方式 tab, hotel details, calendar pricing,
and inclusions from BestTour package pages.
"""

from __future__ import annotations

import re
from typing import Optional, Tuple

from ..base import BaseScraper, scroll_page
from ..schema import (
    ScrapeResult, FlightInfo, FlightSegment, HotelInfo,
    PriceInfo, DatePricing, DatesInfo,
)


class BestTourParser(BaseScraper):
    source_id = "besttour"

    async def prepare_page(self, page, url: str) -> None:
        """Click 交通方式 tab and scroll to load content."""
        await scroll_page(page)

        print("BestTour detected: clicking 交通方式 tab...")
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

    def parse_raw_text(self, raw_text: str, url: str = "", **kwargs) -> ScrapeResult:
        """Parse BestTour page text into structured data."""
        result = ScrapeResult(source_id=self.source_id, url=url)

        result.flight = _parse_flights(raw_text)
        result.hotel = _parse_hotel(raw_text)
        result.inclusions = _parse_inclusions(raw_text)

        ym = _infer_year_month_from_flight_date(
            result.flight.outbound.date
        )
        result.date_pricing = _parse_date_pricing(raw_text, year_month=ym)
        
        # Package type classification
        result.package_type = _classify_package_type(raw_text, url)

        return result


# ---------------------------------------------------------------------------
# Pure parsing functions
# ---------------------------------------------------------------------------

def _parse_flights(raw_text: str) -> FlightInfo:
    """Parse 交通方式 section for flight details."""
    flight_info = FlightInfo()
    lines = raw_text.split("\n")

    for i, line in enumerate(lines):
        line = line.strip()

        if line == "去程" and i + 8 < len(lines):
            flight_info.outbound = _parse_flight_block(lines, i)

        elif line == "回程" and i + 8 < len(lines):
            flight_info.return_ = _parse_flight_block(lines, i)
            break  # Found both, done

    return flight_info


def _parse_flight_block(lines: list[str], start: int) -> FlightSegment:
    """
    Parse a flight block starting at '去程' or '回程'.

    Expected layout:
      [start]   去程/回程
      [start+1] date
      [start+2] flight_number
      [start+3] airline
      [start+4] departure_airport(CODE)
      [start+5] departure_time
      [start+6] →
      [start+7] arrival_airport(CODE)
      [start+8] arrival_time
    """
    try:
        segment = FlightSegment(
            date=lines[start + 1].strip() if start + 1 < len(lines) else "",
            flight_number=lines[start + 2].strip() if start + 2 < len(lines) else "",
            airline=lines[start + 3].strip() if start + 3 < len(lines) else "",
            departure_airport=lines[start + 4].strip() if start + 4 < len(lines) else "",
            departure_time=lines[start + 5].strip() if start + 5 < len(lines) else "",
            arrival_airport=lines[start + 7].strip() if start + 7 < len(lines) else "",
            arrival_time=lines[start + 8].strip() if start + 8 < len(lines) else "",
        )
    except IndexError as e:
        import warnings
        warnings.warn(f"BestTour flight block parse error at line {start}: {e}")
        return FlightSegment()

    # Extract airport codes from e.g. "桃園(TPE)"
    dep_match = re.search(r"\(([A-Z]{3})\)", segment.departure_airport)
    arr_match = re.search(r"\(([A-Z]{3})\)", segment.arrival_airport)
    if dep_match:
        segment.departure_code = dep_match.group(1)
    if arr_match:
        segment.arrival_code = arr_match.group(1)

    return segment


def _infer_year_month_from_flight_date(date_str: str) -> Optional[Tuple[int, int]]:
    """Infer (year, month) from BestTour flight date strings like '2026/02/13(五)'."""
    if not date_str:
        return None
    m = re.search(r"(\d{4})[/-](\d{1,2})[/-](\d{1,2})", date_str)
    if not m:
        return None
    return int(m.group(1)), int(m.group(2))


def _classify_package_type(raw_text: str, url: str) -> str:
    """Classify BestTour package type based on content."""
    # FIT indicators
    if any(kw in raw_text for kw in ["機加酒", "自由行", "機+酒"]):
        return "fit"
    
    # Group tour indicators
    if any(kw in raw_text for kw in ["團體", "跟團", "領隊", "導遊"]):
        return "group"
    
    # Flight only
    if "flight" in url or "機票" in raw_text:
        return "flight"
    
    # Hotel only
    if "hotel" in url or ("飯店" in raw_text and "機" not in raw_text):
        return "hotel"
    
    return "unknown"


def _parse_hotel(raw_text: str) -> HotelInfo:
    """Parse hotel section from raw page text (heuristic)."""
    lines = [l.strip() for l in raw_text.split("\n")]
    hotel = HotelInfo()

    # Heuristic: find a '住宿' section and take the next meaningful line as name
    for i, line in enumerate(lines):
        if line in ("住宿", "飯店", "旅館", "酒店") and i + 1 < len(lines):
            for j in range(i + 1, min(i + 25, len(lines))):
                candidate = lines[j].strip()
                if not candidate:
                    continue
                if candidate in ("交通方式", "行程內容", "出發日期", "費用說明"):
                    break
                if re.match(r"^(地區|區域|地址|電話|入住|退房)[:：]", candidate):
                    continue
                if len(candidate) >= 4:
                    hotel.name = candidate
                    break
            if hotel.name:
                break

    # Area label extraction
    for line in lines:
        m = re.search(r"(地區|區域)[:：]\s*(.+)$", line)
        if m:
            hotel.area = m.group(2).strip()
            break

    # Access: collect transit lines with minutes
    access = []
    for line in lines:
        if (
            re.search(r"(JR|地鐵|捷運|單軌|Monorail|Yurikamome|ゆりかもめ)", line, re.IGNORECASE)
            and re.search(r"(\d+)\s*(分|分鐘|min)", line, re.IGNORECASE)
        ):
            access.append(line.strip())
    hotel.access = list(dict.fromkeys(access))[:8]

    return hotel


def _parse_date_pricing(
    raw_text: str, year_month: Optional[Tuple[int, int]] = None
) -> dict[str, DatePricing]:
    """Parse calendar pricing from raw page text."""

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

    pricing: dict[str, DatePricing] = {}
    lines = [l.strip() for l in raw_text.split("\n") if l.strip()]

    # Prefer full-date matches
    full_date_re = re.compile(
        r"(\d{4})[/-](\d{1,2})[/-](\d{1,2}).{0,20}?"
        r"(可售|滿團|候補|額滿|已滿|停售|關團).{0,30}?([0-9]{4,6})"
    )
    for line in lines:
        m = full_date_re.search(line)
        if not m:
            continue
        y, mo, d = int(m.group(1)), int(m.group(2)), int(m.group(3))
        label = m.group(4)
        price = int(m.group(5))
        seats_match = re.search(r"可售[:：]?\s*(\d+)", line)
        seats = int(seats_match.group(1)) if seats_match else None
        pricing[to_iso(y, mo, d)] = DatePricing(
            date=to_iso(y, mo, d),
            price=price,
            availability=map_availability(label),
            seats_remaining=seats,
        )

    if pricing:
        return pricing

    # Fallback: day-of-month calendar lines if year/month is known
    if not year_month:
        return pricing
    y, mo = year_month
    day_line_re = re.compile(
        r"^(\d{1,2})\s*(可售|滿團|候補|額滿|已滿|停售|關團).{0,40}?([0-9]{4,6})"
    )
    for line in lines:
        m = day_line_re.match(line)
        if not m:
            continue
        d = int(m.group(1))
        label = m.group(2)
        price = int(m.group(3))
        seats_match = re.search(r"可售[:：]?\s*(\d+)", line)
        seats = int(seats_match.group(1)) if seats_match else None
        pricing[to_iso(y, mo, d)] = DatePricing(
            date=to_iso(y, mo, d),
            price=price,
            availability=map_availability(label),
            seats_remaining=seats,
        )

    return pricing


def _parse_inclusions(raw_text: str) -> list[str]:
    """Extract inclusions like breakfast from BestTour text."""
    inclusions = []
    text = raw_text.replace(" ", "")
    if "早餐" in text and (
        "含早餐" in text
        or "包含早餐" in text
        or "附早餐" in text
        or "輕食早餐" in text
        or "簡易早餐" in text
    ):
        inclusions.append("light_breakfast")
    return inclusions
