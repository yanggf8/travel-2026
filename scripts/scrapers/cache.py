"""
Scraper Result Cache

File-based cache with TTL for scrape results.
Prevents redundant scrapes and rate limit issues.
"""

from __future__ import annotations

import hashlib
import json
import os
from datetime import datetime, timedelta
from pathlib import Path
from typing import Optional

from .schema import ScrapeResult


class ScrapeCache:
    """File-based cache for scrape results with TTL."""
    
    def __init__(self, cache_dir: str = "scrapes/cache", default_ttl_hours: int = 24):
        self.cache_dir = Path(cache_dir)
        self.cache_dir.mkdir(parents=True, exist_ok=True)
        self.default_ttl = timedelta(hours=default_ttl_hours)
    
    def _cache_key(self, source_id: str, url: str, **kwargs) -> str:
        """Generate cache key from source_id + url + params."""
        key_parts = [source_id, url]
        if kwargs:
            # Sort kwargs for consistent hashing
            key_parts.extend(f"{k}={v}" for k, v in sorted(kwargs.items()))
        key_str = "|".join(key_parts)
        return hashlib.sha256(key_str.encode()).hexdigest()[:16]
    
    def _cache_path(self, cache_key: str) -> Path:
        """Get cache file path for a cache key."""
        return self.cache_dir / f"{cache_key}.json"
    
    def get(self, source_id: str, url: str, **kwargs) -> Optional[ScrapeResult]:
        """
        Get cached result if exists and not expired.
        
        Returns None if cache miss or expired.
        """
        cache_key = self._cache_key(source_id, url, **kwargs)
        cache_path = self._cache_path(cache_key)
        
        if not cache_path.exists():
            return None
        
        try:
            with open(cache_path, encoding="utf-8") as f:
                data = json.load(f)
            
            # Check expiry
            scraped_at = data.get("scraped_at", "")
            if scraped_at:
                scraped_time = datetime.fromisoformat(scraped_at.replace("Z", "+00:00"))
                if datetime.now() - scraped_time > self.default_ttl:
                    # Expired
                    return None
            
            # Deserialize
            result = ScrapeResult.from_dict(data)
            result.warnings.append(f"Loaded from cache (age: {self._age_str(scraped_at)})")
            return result
            
        except Exception as e:
            print(f"  Cache read error: {e}")
            return None
    
    def set(self, result: ScrapeResult, **kwargs):
        """
        Cache a scrape result.
        
        kwargs: Additional cache key parameters (e.g., date, pax)
        """
        cache_key = self._cache_key(result.source_id, result.url, **kwargs)
        cache_path = self._cache_path(cache_key)
        
        try:
            # Use to_dict() to preserve source_id, errors, warnings
            data = result.to_dict()
            with open(cache_path, "w", encoding="utf-8") as f:
                json.dump(data, f, ensure_ascii=False, indent=2)
        except Exception as e:
            print(f"  Cache write error: {e}")
    
    def invalidate(self, source_id: str, url: str, **kwargs):
        """Invalidate (delete) a cached result."""
        cache_key = self._cache_key(source_id, url, **kwargs)
        cache_path = self._cache_path(cache_key)
        if cache_path.exists():
            cache_path.unlink()
    
    def clear(self, source_id: Optional[str] = None):
        """Clear all cache or cache for a specific source."""
        if source_id:
            # Clear only files matching source_id (requires metadata)
            # For now, just clear all
            pass
        
        for cache_file in self.cache_dir.glob("*.json"):
            cache_file.unlink()
    
    def _age_str(self, scraped_at: str) -> str:
        """Human-readable age string."""
        try:
            scraped_time = datetime.fromisoformat(scraped_at.replace("Z", "+00:00"))
            age = datetime.now() - scraped_time
            hours = age.total_seconds() / 3600
            if hours < 1:
                return f"{int(age.total_seconds() / 60)}m"
            elif hours < 24:
                return f"{int(hours)}h"
            else:
                return f"{int(hours / 24)}d"
        except Exception:
            return "unknown"


# Global cache instance
_cache = ScrapeCache()


def get_cache() -> ScrapeCache:
    """Get the global cache instance."""
    return _cache
