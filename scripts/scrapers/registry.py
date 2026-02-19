"""
OTA Scraper Registry

Maps URLs and source IDs to the appropriate parser class.
All mappings are data-driven from data/ota-sources.json.
"""

from __future__ import annotations

import importlib
import inspect
import re
from typing import Optional
from urllib.parse import urlparse

from .base import BaseScraper, load_ota_config


# Lazy-loaded caches
_url_patterns: list[tuple[str, str]] | None = None
_parser_cache: dict[str, BaseScraper] = {}


def _build_url_patterns() -> list[tuple[str, str]]:
    """Build URL patterns from ota-sources.json base_url fields."""
    global _url_patterns
    if _url_patterns is not None:
        return _url_patterns

    config = load_ota_config()
    patterns: list[tuple[str, str]] = []
    for source_id, source in config.items():
        base_url = source.get("base_url", "")
        if not base_url:
            continue
        # Extract domain from base_url
        parsed = urlparse(base_url)
        domain = parsed.netloc or parsed.path
        # Remove www. prefix for broader matching
        domain = domain.removeprefix("www.")
        patterns.append((re.escape(domain), source_id))

    _url_patterns = patterns
    return _url_patterns


def detect_ota(url: str) -> Optional[str]:
    """
    Detect OTA source_id from a URL.

    Returns source_id string or None if URL doesn't match any known OTA.
    """
    patterns = _build_url_patterns()
    for pattern, source_id in patterns:
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


def _get_parser_module_name(source_id: str) -> str:
    """Get the parser module name for a source_id.

    Convention: module name == source_id.
    Exception: sources with a 'parser_module' field in config override this.
    """
    config = load_ota_config()
    source = config.get(source_id, {})
    return source.get("parser_module", source_id)


def _create_parser(source_id: str) -> BaseScraper:
    """Create a parser instance by source_id using dynamic import."""
    module_name = _get_parser_module_name(source_id)

    try:
        module = importlib.import_module(f".parsers.{module_name}", package="scripts.scrapers")
    except ModuleNotFoundError:
        available = get_available_parsers()
        raise ValueError(
            f"No parser module found for source_id '{source_id}' "
            f"(tried 'scripts.scrapers.parsers.{module_name}'). "
            f"Available: {', '.join(available)}"
        )

    # Find the BaseScraper subclass in the module
    for _, obj in inspect.getmembers(module, inspect.isclass):
        if issubclass(obj, BaseScraper) and obj is not BaseScraper:
            return obj()

    raise ValueError(
        f"No BaseScraper subclass found in 'scripts.scrapers.parsers.{module_name}' "
        f"for source_id '{source_id}'"
    )


def get_available_parsers() -> list[str]:
    """Return list of all available parser source IDs (supported with scraper_script)."""
    config = load_ota_config()
    return [
        source_id
        for source_id, source in config.items()
        if source.get("supported") and source.get("scraper_script")
    ]
