"""
OTA Scraper Registry

Maps URLs and source IDs to the appropriate parser class.
"""

from __future__ import annotations

import re
from typing import Optional

from .base import BaseScraper


# URL pattern → source_id mapping
_URL_PATTERNS: list[tuple[str, str]] = [
    (r"besttour\.com\.tw", "besttour"),
    (r"liontravel\.com", "liontravel"),
    (r"lifetour\.com\.tw", "lifetour"),
    (r"settour\.com\.tw", "settour"),
    (r"tigerairtw\.com", "tigerair"),
    (r"trip\.com", "trip"),
    (r"google\.com/travel/flights", "google_flights"),
    (r"agoda\.com", "agoda"),
    (r"eztravel\.com\.tw", "eztravel"),
    (r"booking\.com", "booking"),
]

# Lazy-loaded parser instances
_parser_cache: dict[str, BaseScraper] = {}


def detect_ota(url: str) -> Optional[str]:
    """
    Detect OTA source_id from a URL.

    Returns source_id string or None if URL doesn't match any known OTA.
    """
    for pattern, source_id in _URL_PATTERNS:
        if re.search(pattern, url):
            return source_id
    return None


def get_parser(source_id: str) -> BaseScraper:
    """
    Get a parser instance for the given source_id.

    Raises ValueError if no parser is registered for the source_id.
    """
    if source_id in _parser_cache:
        return _parser_cache[source_id]

    parser = _create_parser(source_id)
    _parser_cache[source_id] = parser
    return parser


def _create_parser(source_id: str) -> BaseScraper:
    """Create a parser instance by source_id (lazy import to avoid circular deps)."""
    if source_id == "besttour":
        from .parsers.besttour import BestTourParser
        return BestTourParser()
    elif source_id == "liontravel":
        from .parsers.liontravel import LionTravelParser
        return LionTravelParser()
    elif source_id == "lifetour":
        from .parsers.lifetour import LifetourParser
        return LifetourParser()
    elif source_id == "settour":
        from .parsers.settour import SettourParser
        return SettourParser()
    elif source_id == "tigerair":
        from .parsers.tigerair import TigerairParser
        return TigerairParser()
    elif source_id == "trip":
        from .parsers.trip_com import TripComParser
        return TripComParser()
    elif source_id == "google_flights":
        from .parsers.google_flights import GoogleFlightsParser
        return GoogleFlightsParser()
    elif source_id == "agoda":
        from .parsers.agoda import AgodaParser
        return AgodaParser()
    elif source_id == "eztravel":
        from .parsers.eztravel import EzTravelParser
        return EzTravelParser()
    else:
        raise ValueError(
            f"No parser registered for source_id '{source_id}'. "
            f"Available: besttour, liontravel, lifetour, settour, tigerair, trip, google_flights, agoda, eztravel"
        )


def get_available_parsers() -> list[str]:
    """Return list of all available parser source IDs."""
    return ["besttour", "liontravel", "lifetour", "settour", "tigerair", "trip", "google_flights", "agoda", "eztravel"]
