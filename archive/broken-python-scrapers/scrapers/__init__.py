"""
OTA Scrapers Package

Modular scraper framework for travel OTA websites.
Each OTA has its own parser module in scrapers/parsers/.
"""

from .schema import ScrapeResult, FlightInfo, HotelInfo, PriceInfo, DatePricing
from .base import BaseScraper, navigate_with_retry, create_browser, scroll_page
from .registry import detect_ota, get_parser

__all__ = [
    "ScrapeResult",
    "FlightInfo",
    "HotelInfo",
    "PriceInfo",
    "DatePricing",
    "BaseScraper",
    "navigate_with_retry",
    "create_browser",
    "scroll_page",
    "detect_ota",
    "get_parser",
]
