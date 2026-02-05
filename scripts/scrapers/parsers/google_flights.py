"""
Google Flights Parser (google.com/travel/flights)

Extracts flight search results from Google Flights using the natural-language
query URL format: ?q=Flights to DEST from ORIGIN on DATE through DATE

This avoids the need for form interaction — results render directly.
"""

from __future__ import annotations

import re
from urllib.parse import quote

from ..base import BaseScraper, scroll_page
from ..schema import (
    ScrapeResult, FlightInfo, FlightSegment, PriceInfo, DatesInfo,
)


# Airline name normalization
AIRLINE_NAMES = {
    "捷星日本航空": ("Jetstar Japan", "GK"),
    "捷星航空": ("Jetstar", "JQ"),
    "樂桃航空": ("Peach", "MM"),
    "亞洲航空 X": ("AirAsia X", "D7"),
    "亞洲航空": ("AirAsia", "AK"),
    "長榮航空": ("EVA Air", "BR"),
    "全日空航空": ("ANA", "NH"),
    "中華航空": ("China Airlines", "CI"),
    "日本航空": ("JAL", "JL"),
    "星宇航空": ("STARLUX", "JX"),
    "台灣虎航": ("Tigerair Taiwan", "IT"),
    "酷航": ("Scoot", "TR"),
    "泰越捷航空": ("VietJet", "VZ"),
    "泰國獅航": ("Thai Lion Air", "SL"),
    "國泰航空": ("Cathay Pacific", "CX"),
    "華信航空": ("Mandarin Airlines", "AE"),
}


def build_search_url(
    origin: str,
    dest: str,
    depart_date: str,
    return_date: str | None = None,
    pax: int = 1,
    currency: str = "TWD",
    lang: str = "zh-TW",
) -> str:
    """
    Build a Google Flights search URL using the natural-language query format.

    Args:
        origin: Origin airport IATA code (e.g., "TPE")
        dest: Destination airport IATA code (e.g., "KIX")
        depart_date: Departure date YYYY-MM-DD
        return_date: Return date YYYY-MM-DD (None for one-way)
        pax: Number of passengers
        currency: Currency code
        lang: Language code
    """
    if return_date:
        q = f"Flights to {dest} from {origin} on {depart_date} through {return_date}"
    else:
        q = f"Flights to {dest} from {origin} on {depart_date} one way"

    url = (
        f"https://www.google.com/travel/flights"
        f"?q={quote(q)}"
        f"&curr={currency}"
        f"&hl={lang}"
    )
    return url


class GoogleFlightsParser(BaseScraper):
    source_id = "google_flights"

    async def prepare_page(self, page, url: str) -> None:
        """Wait for Google Flights results to render."""
        # Wait for flight results to appear
        try:
            await page.wait_for_selector(
                "text=來回票價, text=單程票價, text=直達",
                timeout=15000,
            )
        except Exception:
            pass

        # Scroll to trigger any lazy-loaded content
        await scroll_page(page, steps=3, step_delay_ms=300, final_delay_ms=1000)

    def parse_raw_text(self, raw_text: str, url: str = "", **kwargs) -> ScrapeResult:
        """Parse Google Flights search results page text."""
        result = ScrapeResult(source_id=self.source_id, url=url)

        flights = parse_flight_results(raw_text)

        if flights:
            result.success = True

            # Set cheapest flight as primary
            cheapest = flights[0]  # Already sorted by price
            result.flight = FlightInfo(
                outbound=FlightSegment(
                    airline=cheapest.get("airline", ""),
                    airline_code=cheapest.get("airline_code", ""),
                    departure_time=cheapest.get("departure_time", ""),
                    arrival_time=cheapest.get("arrival_time", ""),
                    departure_code=cheapest.get("departure_code", ""),
                    arrival_code=cheapest.get("arrival_code", ""),
                ),
            )
            result.price = PriceInfo(
                per_person=cheapest.get("price"),
                currency=kwargs.get("currency", "TWD"),
            )

        # Store all flights in raw_data via extracted_elements
        if flights:
            result.extracted_elements["all_flights"] = [
                (
                    f"{f.get('airline', '?')} "
                    f"{f.get('departure_time', '?')}→{f.get('arrival_time', '?')} "
                    f"{f.get('duration', '?')} "
                    f"{'直達' if f.get('nonstop') else '轉機'} "
                    f"${f.get('price', '?')}"
                )
                for f in flights
            ]

        return result


def parse_flight_results(raw_text: str) -> list[dict]:
    """
    Parse flight options from Google Flights results text.

    Google Flights text format (zh-TW):
        凌晨2:30
         –
        清晨6:00
        捷星日本航空
        2 小時 30 分鐘
        TPE–KIX
        直達
        146 公斤 CO2e
        平均排放量
        $12,528
        來回票價
    """
    flights: list[dict] = []
    lines = [l.strip() for l in raw_text.split("\n") if l.strip()]

    i = 0
    while i < len(lines):
        line = lines[i]

        # Look for time pattern as departure time (with optional prefix like 凌晨/清晨/上午/下午/晚上)
        dep_match = re.match(
            r"^(?:凌晨|清晨|上午|中午|下午|晚上)?(\d{1,2}:\d{2})$", line
        )
        if dep_match and i + 8 < len(lines):
            dep_time = dep_match.group(1)

            # Next meaningful line should be " – " separator, then arrival time
            # Skip the separator
            j = i + 1
            while j < min(i + 3, len(lines)) and lines[j] in ("–", "-", "—"):
                j += 1

            arr_match = re.match(
                r"^(?:凌晨|清晨|上午|中午|下午|晚上)?(\d{1,2}:\d{2})(?:\+\d)?$",
                lines[j] if j < len(lines) else "",
            )
            if arr_match:
                arr_time = arr_match.group(1)
                j += 1

                # Next: airline name(s)
                airline_raw = lines[j] if j < len(lines) else ""
                j += 1

                # Next: duration (e.g., "2 小時 30 分鐘")
                duration = lines[j] if j < len(lines) else ""
                j += 1

                # Next: route (e.g., "TPE–KIX")
                route = lines[j] if j < len(lines) else ""
                j += 1

                # Extract airport codes from route
                route_match = re.match(r"([A-Z]{3})[–\-]([A-Z]{3})", route)
                dep_code = route_match.group(1) if route_match else ""
                arr_code = route_match.group(2) if route_match else ""

                # Next: nonstop indicator
                nonstop = lines[j] == "直達" if j < len(lines) else False
                j += 1

                # Look for price in next few lines
                price = None
                for k in range(j, min(j + 6, len(lines))):
                    price_match = re.match(r"^\$?([\d,]+)$", lines[k])
                    if price_match:
                        price = int(price_match.group(1).replace(",", ""))
                        break

                # Normalize airline name
                airline_name = airline_raw
                airline_code = ""
                for zh_name, (en_name, code) in AIRLINE_NAMES.items():
                    if zh_name in airline_raw:
                        airline_name = airline_raw
                        airline_code = code
                        break

                # Parse duration into minutes
                dur_match = re.search(r"(\d+)\s*小時\s*(\d+)?\s*分", duration)
                duration_mins = None
                if dur_match:
                    duration_mins = int(dur_match.group(1)) * 60 + int(
                        dur_match.group(2) or 0
                    )

                flight = {
                    "airline": airline_name,
                    "airline_code": airline_code,
                    "departure_time": dep_time,
                    "arrival_time": arr_time,
                    "departure_code": dep_code,
                    "arrival_code": arr_code,
                    "duration": duration,
                    "duration_minutes": duration_mins,
                    "nonstop": nonstop,
                    "price": price,
                }
                flights.append(flight)

                i = j
                continue

        i += 1

    # Sort by price (None prices last)
    flights.sort(key=lambda f: f.get("price") or float("inf"))

    return flights
