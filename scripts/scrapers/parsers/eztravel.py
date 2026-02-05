"""
ezTravel Parser (flight.eztravel.com.tw)

Extracts flight search results from ezTravel flight search pages.
"""

from __future__ import annotations

import re

from ..base import BaseScraper
from ..schema import ScrapeResult, FlightInfo, FlightSegment, PriceInfo


class EzTravelParser(BaseScraper):
    source_id = "eztravel"

    def parse_raw_text(self, raw_text: str, url: str = "", **kwargs) -> ScrapeResult:
        """Parse ezTravel flight search page text into structured data."""
        result = ScrapeResult(source_id=self.source_id, url=url)

        # Extract flight results
        flights = _parse_flight_results(raw_text)
        
        if flights:
            # Use cheapest flight as primary result
            cheapest = min(flights, key=lambda f: f.get("price", float("inf")))
            result.price = PriceInfo(
                per_person=cheapest.get("price"),
                currency="TWD",
            )
            
            # Store all flights in extracted_elements for reference
            result.extracted_elements["all_flights"] = [
                f"{f.get('airline', 'Unknown')} {f.get('departure_time', '')}→{f.get('arrival_time', '')} TWD {f.get('price', 'N/A')}"
                for f in flights[:10]
            ]

        return result


def _parse_flight_results(raw_text: str) -> list[dict]:
    """
    Parse flight search results from ezTravel page.
    
    Expected pattern (repeated):
      HH:MM
      XhYmin
      直飛 or 轉機
      HH:MM
      TWD X,XXX or NT$ X,XXX
      Airline Name
    """
    flights = []
    lines = raw_text.split("\n")
    
    i = 0
    while i < len(lines):
        line = lines[i].strip()
        
        # Look for time pattern (departure time)
        time_match = re.match(r"^(\d{1,2}):(\d{2})$", line)
        if time_match and i + 5 < len(lines):
            departure_time = line
            
            # Next line: duration (e.g., "3h5min")
            duration_line = lines[i + 1].strip()
            duration_match = re.match(r"(\d+)h(\d+)min", duration_line)
            
            # Next line: nonstop flag
            nonstop_line = lines[i + 2].strip()
            nonstop = "直飛" in nonstop_line
            
            # Next line: arrival time
            arrival_time = lines[i + 3].strip()
            
            # Look ahead for price (within next 5 lines)
            price = None
            airline = None
            for j in range(i + 4, min(i + 10, len(lines))):
                price_match = re.search(r"(?:TWD|NT\$)\s*([\d,]+)", lines[j])
                if price_match:
                    price = int(price_match.group(1).replace(",", ""))
                    # Airline is usually near price
                    if j + 1 < len(lines):
                        airline_candidate = lines[j + 1].strip()
                        if airline_candidate and len(airline_candidate) < 50:
                            airline = airline_candidate
                    break
            
            if departure_time and arrival_time:
                flight = {
                    "departure_time": departure_time,
                    "arrival_time": arrival_time,
                    "duration_minutes": (int(duration_match.group(1)) * 60 + int(duration_match.group(2))) if duration_match else None,
                    "nonstop": nonstop,
                    "price": price,
                    "airline": airline or "Unknown",
                }
                flights.append(flight)
        
        i += 1
    
    return flights
