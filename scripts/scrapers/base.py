"""
Base Scraper

Shared browser helpers, retry logic, and abstract base class for OTA parsers.
"""

from __future__ import annotations

import asyncio
import json
import re
from abc import ABC, abstractmethod
from datetime import datetime
from pathlib import Path
from typing import Any, Optional
from urllib.parse import urljoin

from .schema import ScrapeResult


# ---------------------------------------------------------------------------
# OTA Config Loader
# ---------------------------------------------------------------------------

_ota_config_cache: dict | None = None


def load_ota_config() -> dict:
    """Load OTA configuration from ota-sources.json."""
    global _ota_config_cache
    if _ota_config_cache is not None:
        return _ota_config_cache

    config_path = Path(__file__).parent.parent.parent / "data" / "ota-sources.json"
    if config_path.exists():
        with open(config_path, encoding="utf-8") as f:
            _ota_config_cache = json.load(f).get("sources", {})
    else:
        _ota_config_cache = {}
    return _ota_config_cache


def get_listing_selectors(source_id: str) -> dict | None:
    """Get listing selectors for an OTA from config."""
    config = load_ota_config()
    source = config.get(source_id, {})
    return source.get("listing_selectors")


# ---------------------------------------------------------------------------
# Baggage Extraction (P6)
# ---------------------------------------------------------------------------

BAGGAGE_PATTERNS = {
    "included": [
        r"託運行李\s*(\d+)\s*公斤",
        r"含\s*(\d+)\s*kg\s*行李",
        r"checked baggage.*?(\d+)\s*kg",
        r"免費託運.*?(\d+)\s*kg",
        r"行李\s*(\d+)\s*kg",
    ],
    "not_included": [
        r"不含行李",
        r"無免費託運",
        r"行李.*另購",
        r"baggage not included",
        r"手提行李\s*\d+\s*kg\s*$",
    ],
}


def extract_baggage_info(raw_text: str) -> dict:
    """Extract baggage info from raw text. Returns {"included": bool|None, "kg": int|None}."""
    text = raw_text.lower() if raw_text else ""
    for pattern in BAGGAGE_PATTERNS["included"]:
        m = re.search(pattern, raw_text, re.IGNORECASE)
        if m:
            return {"included": True, "kg": int(m.group(1))}
    for pattern in BAGGAGE_PATTERNS["not_included"]:
        if re.search(pattern, raw_text, re.IGNORECASE):
            return {"included": False, "kg": None}
    return {"included": None, "kg": None}


# ---------------------------------------------------------------------------
# Hotel Area Detection (P7)
# ---------------------------------------------------------------------------

_hotel_areas_cache: dict | None = None


def _load_hotel_areas() -> dict:
    global _hotel_areas_cache
    if _hotel_areas_cache is not None:
        return _hotel_areas_cache
    path = Path(__file__).parent.parent.parent / "data" / "hotel-areas.json"
    if path.exists():
        with open(path, encoding="utf-8") as f:
            _hotel_areas_cache = json.load(f)
    else:
        _hotel_areas_cache = {}
    return _hotel_areas_cache


def detect_hotel_area(hotel_name: str, region: str) -> str:
    """Detect hotel area type from name. Returns 'central', 'airport', 'suburb', etc."""
    areas = _load_hotel_areas().get(region, {})
    for area_type, keywords in areas.items():
        for kw in keywords:
            if kw in hotel_name:
                return area_type
    return "unknown"


def _infer_region(url: str) -> str:
    """Best-effort region inference from URL keywords."""
    u = url.lower()
    for region in ("kansai", "osaka", "kyoto", "tokyo", "nagoya"):
        if region in u:
            return "kansai" if region in ("osaka", "kyoto") else region
    return ""


# ---------------------------------------------------------------------------
# Browser helpers
# ---------------------------------------------------------------------------

async def create_browser(playwright, headless: bool = True, viewport: dict | None = None):
    """Create a browser + context with standard settings."""
    browser = await playwright.chromium.launch(headless=headless)
    context = await browser.new_context(
        viewport=viewport or {"width": 1920, "height": 1080},
        user_agent=(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) "
            "AppleWebKit/537.36 (KHTML, like Gecko) "
            "Chrome/120.0.0.0 Safari/537.36"
        ),
    )
    page = await context.new_page()
    return browser, context, page


async def navigate_with_retry(
    page,
    url: str,
    max_retries: int = 3,
    backoff_base: float = 2.0,
    timeout: int = 60000,
) -> bool:
    """
    Navigate to a URL with exponential backoff retry.

    Tries networkidle first, falls back to domcontentloaded, then retries.
    Returns True if navigation succeeded, False if all retries exhausted.
    """
    strategies = ["networkidle", "domcontentloaded"]

    for attempt in range(max_retries):
        for strategy in strategies:
            try:
                await page.goto(url, wait_until=strategy, timeout=timeout)
                return True
            except Exception as e:
                if strategy == "networkidle":
                    # Expected — try domcontentloaded next
                    continue
                elif attempt < max_retries - 1:
                    wait_time = backoff_base ** (attempt + 1)
                    print(f"  Retry {attempt + 1}/{max_retries} after {wait_time:.0f}s: {e}")
                    await asyncio.sleep(wait_time)
                    break  # Break inner loop, retry outer loop
                else:
                    print(f"  All {max_retries} retries failed for {url}: {e}")
                    return False

    return False


async def scroll_page(page, steps: int = 5, step_delay_ms: int = 500, final_delay_ms: int = 2000):
    """Scroll page in steps to trigger lazy loading."""
    # Initial scroll to bottom
    await page.evaluate("window.scrollTo(0, document.body.scrollHeight)")
    await page.wait_for_timeout(final_delay_ms)

    # Scroll in incremental steps
    for i in range(steps):
        await page.evaluate(f"window.scrollTo(0, {(i + 1) * 1000})")
        await page.wait_for_timeout(step_delay_ms)

    # Final scroll to bottom
    await page.evaluate("window.scrollTo(0, document.body.scrollHeight)")
    await page.wait_for_timeout(final_delay_ms)


async def safe_extract_text(page) -> str:
    """Extract visible text from page with error handling."""
    try:
        return await page.evaluate("() => document.body.innerText")
    except Exception as e:
        print(f"  Warning: Could not extract page text: {e}")
        return ""


async def extract_generic_elements(page) -> dict[str, list[str]]:
    """Extract elements matching common travel-site CSS selectors."""
    selectors_to_try = [
        (".price", "price_element"),
        (".itinerary", "itinerary_element"),
        (".flight-info", "flight_element"),
        (".hotel-info", "hotel_element"),
        ("[class*='price']", "price_class"),
        ("[class*='flight']", "flight_class"),
        ("[class*='hotel']", "hotel_class"),
        ("table", "tables"),
        (".content", "content"),
        ("main", "main"),
        ("#content", "content_id"),
    ]

    extracted = {}
    for selector, name in selectors_to_try:
        try:
            elements = await page.query_selector_all(selector)
            if elements:
                texts = []
                for el in elements[:5]:
                    text = await el.inner_text()
                    if text.strip():
                        texts.append(text.strip())
                if texts:
                    extracted[name] = texts
        except Exception:
            pass

    return extracted


async def _extract_container_based(page, selectors: dict, base_url: str) -> list[dict]:
    """Extract packages using container-based selectors from config."""
    links = []
    container_sel = selectors.get("container", ".product-item")
    title_sel = selectors.get("title", "h3, h4, .title")
    price_sel = selectors.get("price", ".price")
    code_regex = selectors.get("code_regex", r"([A-Z0-9]+)")
    url_template = selectors.get("url_template", "")

    try:
        items = await page.query_selector_all(container_sel)
        for item in items:
            try:
                # Get title
                title_el = await item.query_selector(title_sel)
                title = ""
                if title_el:
                    title = await title_el.inner_text()
                    title = title.strip()[:100]

                # Get price
                price_el = await item.query_selector(price_sel)
                price_text = ""
                if price_el:
                    price_text = await price_el.inner_text()

                # Get product code from container HTML
                item_html = await item.inner_html()
                code_match = re.search(code_regex, item_html)
                code = code_match.group(1) if code_match else ""

                if code and url_template:
                    # Construct product URL from template
                    product_url = url_template.replace("{code}", code)
                    links.append({
                        "url": product_url,
                        "code": code,
                        "title": f"{title} {price_text}".strip(),
                    })
            except Exception:
                continue
    except Exception as e:
        print(f"  Error extracting with container selectors: {e}")

    return links


async def extract_package_links(page, base_url: str) -> list[dict]:
    """
    Extract package detail links from listing pages.

    Uses config from ota-sources.json when available, falls back to hardcoded selectors.
    Supports BestTour, LionTravel, Lifetour, Settour.
    """
    links = []

    # Try to get selectors from config
    source_id = None
    for ota_id in ["besttour", "lifetour", "settour", "liontravel"]:
        if ota_id in base_url or f"{ota_id}.com" in base_url:
            source_id = ota_id
            break

    if source_id:
        selectors = get_listing_selectors(source_id)
        if selectors and selectors.get("method") == "container":
            return await _extract_container_based(page, selectors, base_url)

    # Fallback: Special handling for Settour - uses .product-item containers
    if "settour.com.tw" in base_url:
        selectors = {
            "container": ".product-item",
            "title": ".product-title, h3, h4, .title",
            "price": ".ori-price-offer, .price",
            "code_regex": r"slider-flightInfo_([A-Z0-9]+)",
            "url_template": "https://tour.settour.com.tw/product/{code}",
        }
        return await _extract_container_based(page, selectors, base_url)

    # Standard anchor-based extraction for other OTAs
    try:
        anchors = await page.query_selector_all("a[href]")
        seen: set[str] = set()

        for anchor in anchors[:100]:
            try:
                href = await anchor.get_attribute("href")
                if not href:
                    continue

                text = await anchor.inner_text()
                text = text.strip()[:100] if text else ""
                full_url = urljoin(base_url, href)

                if full_url in seen:
                    continue

                link_entry = _match_package_link(base_url, href, full_url, text)
                if link_entry:
                    seen.add(full_url)
                    links.append(link_entry)

            except Exception:
                continue

    except Exception as e:
        print(f"  Error extracting package links: {e}")

    return links


def _match_package_link(base_url: str, href: str, full_url: str, text: str) -> dict | None:
    """Check if a link matches known OTA package URL patterns."""
    if "besttour.com.tw" in base_url:
        if "/itinerary/" in href:
            code_match = re.search(r"/itinerary/([A-Z0-9]+)", href)
            return {
                "url": full_url,
                "code": code_match.group(1) if code_match else "",
                "title": text,
            }

    elif "liontravel.com" in base_url:
        if "/product/" in href or "/detail/" in href:
            code_match = re.search(r"/(?:product|detail)/(\d+)", href)
            return {
                "url": full_url,
                "code": code_match.group(1) if code_match else "",
                "title": text,
            }

    elif "lifetour.com.tw" in base_url:
        if "/detail" in href:
            return {"url": full_url, "code": "", "title": text}

    elif "settour.com.tw" in base_url:
        if "/product/" in href:
            code_match = re.search(r"/product/([A-Z0-9]+)", href, re.IGNORECASE)
            return {
                "url": full_url,
                "code": code_match.group(1) if code_match else "",
                "title": text,
            }

    return None


# ---------------------------------------------------------------------------
# Abstract base parser
# ---------------------------------------------------------------------------

class BaseScraper(ABC):
    """
    Abstract base class for OTA parsers.

    Subclasses must implement:
    - source_id: OTA identifier (e.g., "besttour")
    - parse_raw_text(): Pure parsing from page text (testable without browser)
    - prepare_page(): OTA-specific page preparation (clicking tabs, etc.)
    """

    source_id: str = ""

    @abstractmethod
    def parse_raw_text(self, raw_text: str, url: str = "", **kwargs) -> ScrapeResult:
        """
        Parse structured data from raw page text.

        This is the pure-parsing method that can be tested without a browser.
        """
        ...

    async def prepare_page(self, page, url: str) -> None:
        """
        OTA-specific page preparation after navigation.

        Override to click tabs, dismiss popups, etc.
        Default: scroll page to trigger lazy loading.
        """
        await scroll_page(page)

    async def scrape(self, page, url: str, **kwargs) -> ScrapeResult:
        """
        Full scrape: navigate, prepare, extract, parse.

        Uses navigate_with_retry for reliable page loading.
        Supports caching via use_cache kwarg.
        """
        from .cache import get_cache
        
        use_cache = kwargs.pop("use_cache", True)
        cache = get_cache()
        
        # Try cache first
        if use_cache:
            cached = cache.get(self.source_id, url, **kwargs)
            if cached:
                return cached
        
        result = ScrapeResult(
            source_id=self.source_id,
            url=url,
            scraped_at=datetime.now().isoformat(),
        )

        # Navigate
        success = await navigate_with_retry(page, url)
        if not success:
            result.success = False
            result.errors.append(f"Failed to navigate to {url} after retries")
            return result

        # Wait for initial content
        await page.wait_for_timeout(3000)

        # OTA-specific preparation (tabs, scrolling, etc.)
        await self.prepare_page(page, url)

        # Extract page title
        try:
            result.title = await page.title()
        except Exception:
            pass

        # Extract raw text
        result.raw_text = await safe_extract_text(page)

        # Extract generic CSS elements
        result.extracted_elements = await extract_generic_elements(page)

        # Extract package links
        result.package_links = await extract_package_links(page, url)

        # Run OTA-specific parsing
        parsed = self.parse_raw_text(result.raw_text, url=url, **kwargs)

        # Merge parsed data into result
        result.flight = parsed.flight
        result.hotel = parsed.hotel
        result.price = parsed.price
        result.dates = parsed.dates
        result.inclusions = parsed.inclusions
        result.date_pricing = parsed.date_pricing
        result.itinerary = parsed.itinerary

        # Auto-extract baggage info if not already set
        if result.baggage_included is None and result.raw_text:
            bag = extract_baggage_info(result.raw_text)
            result.baggage_included = bag["included"]
            result.baggage_kg = bag["kg"]

        # Auto-detect hotel area type if hotel is populated
        if result.hotel.is_populated and not result.hotel.area_type:
            region = _infer_region(url)
            if region:
                result.hotel.area_type = detect_hotel_area(
                    result.hotel.name or (result.hotel.names[0] if result.hotel.names else ""),
                    region,
                )
        
        # Cache the result
        if use_cache:
            cache.set(result, **kwargs)

        return result
