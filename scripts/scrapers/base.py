"""
Base Scraper

Shared browser helpers, retry logic, and abstract base class for OTA parsers.
"""

from __future__ import annotations

import asyncio
import re
from abc import ABC, abstractmethod
from datetime import datetime
from typing import Any, Optional
from urllib.parse import urljoin

from .schema import ScrapeResult


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


async def extract_package_links(page, base_url: str) -> list[dict]:
    """
    Extract package detail links from listing pages.

    Supports BestTour, LionTravel, Lifetour, Settour.
    """
    links = []

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
        """
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

        return result
