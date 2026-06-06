"""
Trip.com Parser (trip.com)

Extracts flight prices from Trip.com search results.
Supports one-way flight scraping and date-range comparison.
"""

from __future__ import annotations

import asyncio
import re
from datetime import datetime, timedelta

from ..base import BaseScraper
from ..schema import ScrapeResult, FlightInfo, FlightSegment, PriceInfo


# City name mapping for Trip.com URL
CITY_NAMES = {
    "tpe": "taipei",
    "kix": "osaka",
    "nrt": "tokyo",
    "hnd": "tokyo",
    "knh": "kinmen",
    "cts": "sapporo",
    "oka": "okinawa",
    "fuk": "fukuoka",
    "ngo": "nagoya",
}


class TripComParser(BaseScraper):
    source_id = "trip"

    def parse_raw_text(self, raw_text: str, url: str = "", **kwargs) -> ScrapeResult:
        """Parse Trip.com flight results page text."""
        result = ScrapeResult(source_id=self.source_id, url=url)
        pax = kwargs.get("pax", 2)

        flights = parse_nonstop_flights(raw_text, pax)

        if flights:
            cheapest = min(flights, key=lambda f: f["total_usd"])
            result.flight = FlightInfo(
                outbound=FlightSegment(
                    airline=cheapest.get("airline", ""),
                    departure_time=cheapest.get("depart", ""),
                    arrival_time=cheapest.get("arrive", ""),
                ),
            )
            result.price = PriceInfo(
                per_person=cheapest.get("price_per_person_usd"),
                total=cheapest.get("total_usd"),
                currency="USD",
            )

        return result


# ---------------------------------------------------------------------------
# URL builders
# ---------------------------------------------------------------------------

def build_oneway_url(
    origin: str, dest: str, date: str, pax: int
) -> str:
    """Build Trip.com one-way flight search URL."""
    origin_name = CITY_NAMES.get(origin.lower(), origin.lower())
    dest_name = CITY_NAMES.get(dest.lower(), dest.lower())
    o = origin.lower()
    d = dest.lower()
    return (
        f"https://www.trip.com/flights/{origin_name}-to-{dest_name}/"
        f"tickets-{o}-{d}?dcity={o}&acity={d}"
        f"&ddate={date}&flighttype=ow&class=y&quantity={pax}"
    )


# ---------------------------------------------------------------------------
# Pure parsing functions
# ---------------------------------------------------------------------------

def parse_nonstop_flights(raw_text: str, pax: int = 2) -> list[dict]:
    """Extract nonstop flight options from Trip.com results text."""
    flights = []
    lines = raw_text.split("\n")

    i = 0
    while i < len(lines):
        line = lines[i].strip()

        if re.match(r"^\d{1,2}:\d{2}$", line):
            # Departure time found — look back for airline
            airline = ""
            for j in range(max(0, i - 3), i):
                candidate = lines[j].strip()
                if candidate and not re.match(
                    r"^(Carry-on|Included|Checked|<\d|CO2)", candidate
                ):
                    airline = candidate

            depart_time = line

            if i + 4 < len(lines):
                duration = lines[i + 2].strip() if i + 2 < len(lines) else ""
                nonstop = "Nonstop" in (
                    lines[i + 3].strip() if i + 3 < len(lines) else ""
                )
                arrive_time = lines[i + 4].strip() if i + 4 < len(lines) else ""

                # Find price
                price = None
                for k in range(i + 4, min(i + 10, len(lines))):
                    price_match = re.match(
                        r"US\$(\d[\d,]*)", lines[k].strip()
                    )
                    if price_match:
                        price = int(price_match.group(1).replace(",", ""))
                        break

                # Total price
                total_price = None
                for k in range(i + 4, min(i + 12, len(lines))):
                    total_match = re.match(
                        r"Total US\$(\d[\d,]*)", lines[k].strip()
                    )
                    if total_match:
                        total_price = int(
                            total_match.group(1).replace(",", "")
                        )
                        break

                if price and nonstop:
                    flights.append({
                        "airline": airline,
                        "depart": depart_time,
                        "arrive": arrive_time,
                        "duration": duration,
                        "nonstop": True,
                        "price_per_person_usd": price,
                        "total_usd": total_price or price * pax,
                    })
        i += 1

    return flights


# ---------------------------------------------------------------------------
# Date range helpers
# ---------------------------------------------------------------------------

def date_range(start: str, end: str) -> list[str]:
    """Generate list of dates from start to end inclusive."""
    s = datetime.strptime(start, "%Y-%m-%d")
    e = datetime.strptime(end, "%Y-%m-%d")
    dates = []
    current = s
    while current <= e:
        dates.append(current.strftime("%Y-%m-%d"))
        current += timedelta(days=1)
    return dates


def add_days(date_str: str, days: int) -> str:
    """Add N days to a date string."""
    d = datetime.strptime(date_str, "%Y-%m-%d")
    return (d + timedelta(days=days)).strftime("%Y-%m-%d")


def day_of_week(date_str: str) -> str:
    """Get day of week abbreviation for a date."""
    d = datetime.strptime(date_str, "%Y-%m-%d")
    return ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][d.weekday()]
