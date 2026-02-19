"""
OTA Parser Modules

Each module provides a parser class that extracts structured data
from a specific OTA's page text.
"""

from .besttour import BestTourParser
from .lifetour import LifetourParser
from .settour import SettourParser
from .liontravel import LionTravelParser
from .tigerair import TigerairParser
from .trip_com import TripComParser
from .google_flights import GoogleFlightsParser
from .agoda import AgodaParser
from .travel4u import Travel4UParser
from .eztravel import EzTravelParser

__all__ = [
    "BestTourParser",
    "LifetourParser",
    "SettourParser",
    "LionTravelParser",
    "TigerairParser",
    "TripComParser",
    "GoogleFlightsParser",
    "AgodaParser",
    "Travel4UParser",
    "EzTravelParser",
]
