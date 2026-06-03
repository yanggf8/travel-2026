"""
Scraper Result Cache

Local scrape-result caching is disabled. Raw scraper output may land in
scrapes/ only for import into Turso; runtime caching belongs in Turso if it is
needed again.
"""

from __future__ import annotations

import hashlib
from datetime import timedelta
from typing import Optional

from .schema import ScrapeResult


class ScrapeCache:
    """No-op cache preserving the previous scraper interface."""

    def __init__(self, cache_dir: str = "turso", default_ttl_hours: int = 24):
        self.cache_dir = cache_dir
        self.default_ttl = timedelta(hours=default_ttl_hours)

    def _cache_key(self, source_id: str, url: str, **kwargs) -> str:
        key_parts = [source_id, url]
        if kwargs:
            key_parts.extend(f"{k}={v}" for k, v in sorted(kwargs.items()))
        return hashlib.sha256("|".join(key_parts).encode()).hexdigest()[:16]

    def get(self, source_id: str, url: str, **kwargs) -> Optional[ScrapeResult]:
        return None

    def set(self, result: ScrapeResult, **kwargs) -> None:
        return None

    def invalidate(self, source_id: str, url: str, **kwargs) -> None:
        return None

    def clear(self, source_id: Optional[str] = None) -> None:
        return None


_cache = ScrapeCache()


def get_cache() -> ScrapeCache:
    return _cache
