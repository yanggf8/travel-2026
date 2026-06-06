#!/usr/bin/env python3
"""
Tour-group listing scraper.

Scrapes tour-group (團體/跟團) listings from Taiwan travel agency sites for the
Shaping Stage baseline. Writes a JSON envelope matching the importer contract
documented in docs/superpowers/specs/2026-05-25-tour-group-scraper-design.md §3.3.

Usage:
    python scripts/scrape_tour_groups.py \\
        --source besttour --dest-region kansai --nights 5 \\
        --depart-start 2026-06-14 --depart-end 2026-06-28 \\
        --run-id shaping-20260525-... \\
        --output scrapes/besttour-kansai-5n.json

Live-site selectors drift. If a run returns 0 offers, open the listing URL
printed at the top of the run, inspect the HTML, and update the selectors
in the per-source function below.

Requirements:
    pip install playwright
    playwright install chromium
"""
from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import re
import sys
from datetime import datetime, timedelta
from pathlib import Path
from typing import Optional

try:
    from playwright.async_api import async_playwright, Page
except ImportError:
    print("Playwright not installed. Run: pip install playwright && playwright install chromium")
    sys.exit(1)


# ── URL templates (per-source listing pages) ─────────────────────────

BESTTOUR_REGION_PARAM = {
    "kansai": "japan_kansai",
    "kanto": "japan_tokyo",
    "tokyo": "japan_tokyo",
    "hokkaido": "japan_hokkaido",
    "kyushu": "japan_kyushu",
    "okinawa": "japan_okinawa",
    "tohoku": "japan_tohoku",
}

LIFETOUR_REGION_CODE = {
    "kansai": "0001-0003",
    "kanto": "0001-0001",
    "tokyo": "0001-0001",
    "hokkaido": "0001-0002",
    "kyushu": "0001-0004",
    "okinawa": "0001-0005",
}

SETTOUR_REGION_CODE = {
    "kansai": "JX_3",
    "kanto": "JX_1",
    "tokyo": "JX_1",
    "hokkaido": "JX_2",
    "kyushu": "JX_4",
    "okinawa": "JX_5",
}


def besttour_url(region: str) -> str:
    code = BESTTOUR_REGION_PARAM.get(region, "japan_kansai")
    return f"https://www.besttour.com.tw/e_web/activity?v={code}"


def lifetour_url(region: str) -> str:
    code = LIFETOUR_REGION_CODE.get(region)
    if not code:
        raise ValueError(f"No Lifetour region code for {region}")
    return f"https://tour.lifetour.com.tw/searchlist/tpe/{code}"


def settour_url(region: str) -> str:
    code = SETTOUR_REGION_CODE.get(region)
    if not code:
        raise ValueError(f"No Settour region code for {region}")
    return f"https://tour.settour.com.tw/search?destinationCode={code}"


# ── Common helpers ───────────────────────────────────────────────────

def parse_chinese_date(s: str) -> Optional[str]:
    """Parse common Chinese-locale date strings into YYYY-MM-DD."""
    if not s:
        return None
    m = re.search(r"(\d{4})[/\-年](\d{1,2})[/\-月](\d{1,2})", s)
    if m:
        y, mo, d = m.groups()
        return f"{y}-{int(mo):02d}-{int(d):02d}"
    m = re.search(r"(\d{1,2})[/\-月](\d{1,2})", s)
    if m:
        mo, d = m.groups()
        return f"{datetime.utcnow().year}-{int(mo):02d}-{int(d):02d}"
    return None


def parse_price_twd(s: str) -> Optional[int]:
    """Extract a TWD tour-group price from a card text block.

    Prefers the canonical BestTour/Lifetour/Settour "$N,NNN 元起" pattern,
    which is the per-person 'starting from' price. Falls back to the first
    plausible 4-6 digit number only if no '元起' anchor exists.
    """
    if not s:
        return None
    # Primary: '$41,800 元起' / '41800元起' — the explicit 'starting from' label.
    for pat in [
        r"\$?\s*([\d,]{4,9})\s*元\s*起",
        r"NT\$?\s*([\d,]{4,9})\s*元?\s*起",
        r"TWD\s*([\d,]{4,9})\s*元?\s*起",
    ]:
        m = re.search(pat, s)
        if m:
            try:
                v = int(m.group(1).replace(",", ""))
            except ValueError:
                continue
            if 5000 < v < 300000:
                return v
    # Fallback: first 4-6 digit number with a currency-ish neighbor.
    for pat in [
        r"\$\s*([\d,]{4,9})",
        r"NT\$\s*([\d,]{4,9})",
        r"TWD\s*([\d,]{4,9})",
    ]:
        m = re.search(pat, s)
        if m:
            try:
                v = int(m.group(1).replace(",", ""))
            except ValueError:
                continue
            if 5000 < v < 300000:
                return v
    return None


def parse_nights_from_title(title: str) -> Optional[int]:
    """Extract trip length in nights from titles like '關西五日' or '5日4夜'."""
    if not title:
        return None
    # Chinese numerals first: 三/四/五/六/七/八/九/十 (3-10)
    cn_map = {"三": 3, "四": 4, "五": 5, "六": 6, "七": 7, "八": 8, "九": 9, "十": 10}
    m = re.search(r"([三四五六七八九十])\s*(?:日|天)", title)
    if m:
        return cn_map[m.group(1)] - 1
    # Arabic numerals: '5日', '5天', '5days'
    m = re.search(r"(\d+)\s*(?:日|天|days?)", title)
    if m:
        return int(m.group(1)) - 1
    return None


def detect_product_kind(title: str) -> str:
    """Classify an agency listing as 跟團 (group_tour) or 機加酒/自由行 (fit).

    Listing pages mix both products. Only the title carries a reliable
    signal: 機加酒 and 自由行 always mean FIT. Anything else defaults to
    group_tour — that's the safe assumption since these listing URLs are
    positioned as tour-group inventory.
    """
    if not title:
        return "group_tour"
    if "機加酒" in title or "自由行" in title:
        return "fit"
    return "group_tour"


def detect_departure_status(card_text: str) -> str:
    """Map agency status labels to canonical values."""
    if "額滿" in card_text or "售完" in card_text or "已滿" in card_text:
        return "sold_out"
    if "保證出發" in card_text or "成團" in card_text:
        return "guaranteed"
    if "可候補" in card_text or "候補" in card_text:
        return "waitlist"
    return "available"


def make_offer_id(source_id: str, region: str, depart_date: str, nights: int, url: str) -> str:
    short_hash = hashlib.md5(f"{url}|{depart_date}".encode()).hexdigest()[:6]
    return f"{source_id}-{region.upper()}-{depart_date.replace('-', '')}-{nights}n-{short_hash}"


async def safe_text(el) -> str:
    try:
        t = await el.inner_text()
        return (t or "").strip()
    except Exception:
        return ""


# ── Per-source listing scrapers ──────────────────────────────────────

async def scrape_besttour(page: Page, region: str, target_nights: int,
                          depart_start: str, depart_end: str,
                          run_id: str, scraped_at: str) -> list[dict]:
    """Scrape BestTour 喜鴻 tour-group listings.

    BestTour pages render each tour as a card with ONE title anchor (whose
    depth-2 container holds title + days + 'starting from' price) plus N
    date anchors (one per displayed departure date). Both types of anchor
    have hrefs of the form:

        https://www.besttour.com.tw/itinerary/OSA05BR260612KM
                                              │  │ │  │
                                              │  │ │  └─ depart-date YYMMDD
                                              │  │ └──── airline code (BR/AK/CI/JX/...)
                                              │  └────── trip length in DAYS, zero-padded
                                              └───────── destination (OSA/TYO/UKB/...)

    Title anchors give the canonical URL (often a non-June default date);
    date anchors give the real departures. We do two passes:

    1. Collect all title anchors (depth-2 container text matches '元起') and
       index their (dest, days, airline) -> {title, price}.
    2. For each date anchor matching target_days and the depart-date window,
       look up the indexed price by (dest, days, airline) prefix.

    A date anchor without a matching title-anchor entry is skipped.
    """
    target_days = target_nights + 1
    offers: list[dict] = []
    seen_ids: set[str] = set()

    anchors = await page.query_selector_all("a[href*='/itinerary/']")
    print(f"  besttour: {len(anchors)} itinerary anchors on page")

    url_re = re.compile(r"/itinerary/([A-Z]{3})(\d{2})([A-Z0-9]{2,4})(\d{6})")

    # Pass 1: harvest title anchors (those whose depth-2 container has '元起').
    tour_index: dict[tuple, dict] = {}
    for anchor in anchors:
        try:
            href = await anchor.get_attribute("href") or ""
            m = url_re.search(href)
            if not m:
                continue
            dest_code = m.group(1)
            days = int(m.group(2))
            airline = m.group(3)
            if days != target_days:
                continue

            container_text = await anchor.evaluate(
                "el => { let p = el; for (let i=0;i<2;i++) { if (!p.parentElement) break; p = p.parentElement; } return p.innerText || ''; }"
            )
            if "元起" not in container_text:
                continue  # date anchor or other; only title anchors have the price label

            price = parse_price_twd(container_text)
            if not price:
                continue
            title_m = re.search(r"【([^】]+)】[^\n]*", container_text)
            title = title_m.group(0).strip() if title_m else f"BestTour {dest_code} {days}日"
            departure_status = detect_departure_status(container_text)

            key = (dest_code, days, airline)
            # If multiple title anchors share the same (dest, days, airline),
            # keep the cheapest. This is defensive — BestTour usually has one
            # title per tour, but date anchors can also accidentally match if
            # the listing layout shifts.
            existing = tour_index.get(key)
            if existing is None or price < existing["price"]:
                tour_index[key] = {
                    "price": price,
                    "title": title,
                    "departure_status": departure_status,
                    "product_kind": detect_product_kind(title),
                }
        except Exception:
            continue

    # Pass 2: enumerate date anchors and emit one offer per (tour, depart_date).
    for anchor in anchors:
        try:
            href = await anchor.get_attribute("href") or ""
            m = url_re.search(href)
            if not m:
                continue
            dest_code, days, airline, ymd = (
                m.group(1), int(m.group(2)), m.group(3), m.group(4)
            )
            if days != target_days:
                continue

            try:
                depart_dt = datetime.strptime(ymd, "%y%m%d")
            except ValueError:
                continue
            depart_date = depart_dt.strftime("%Y-%m-%d")
            if not (depart_start <= depart_date <= depart_end):
                continue

            tour = tour_index.get((dest_code, days, airline))
            if not tour:
                # Date anchor with no matching title; rare. Skip — no price.
                continue

            return_date = (depart_dt + timedelta(days=target_nights)).strftime("%Y-%m-%d")
            offer_id = make_offer_id("besttour", region, depart_date, target_nights, href)
            if offer_id in seen_ids:
                continue
            seen_ids.add(offer_id)

            offers.append({
                "run_id": run_id,
                "offer_id": offer_id,
                "source_id": "besttour",
                "dest_region": region,
                "depart_date": depart_date,
                "return_date": return_date,
                "nights": target_nights,
                "price_per_person_twd": tour["price"],
                "title": tour["title"],
                "url": href,
                "scraped_at": scraped_at,
                "hotel_name": None,
                "hotel_star_rating": None,
                "meals_included_count": None,
                "departure_status": tour["departure_status"],
                "seats_available": None,
                "min_group_size": None,
                "group_size_cap": None,
                "product_kind": tour["product_kind"],
            })
        except Exception:
            continue

    return offers


async def scrape_lifetour(page: Page, region: str, target_nights: int,
                          depart_start: str, depart_end: str,
                          run_id: str, scraped_at: str) -> list[dict]:
    return await _scrape_generic_cards(
        page, "lifetour", region, target_nights, depart_start, depart_end,
        run_id, scraped_at,
        url_prefix="https://tour.lifetour.com.tw",
        card_selectors=[".product-card", ".listing-item", ".tour-item",
                        "[class*='product']", "li.item"],
    )


async def scrape_settour(page: Page, region: str, target_nights: int,
                         depart_start: str, depart_end: str,
                         run_id: str, scraped_at: str) -> list[dict]:
    return await _scrape_generic_cards(
        page, "settour", region, target_nights, depart_start, depart_end,
        run_id, scraped_at,
        url_prefix="https://tour.settour.com.tw",
        card_selectors=[".product", ".product-card", ".tour-item",
                        "[class*='product']", "li.item"],
    )


async def _scrape_generic_cards(page, source_id, region, target_nights,
                                 depart_start, depart_end, run_id, scraped_at,
                                 url_prefix, card_selectors) -> list[dict]:
    """Generic listing scraper for Lifetour / Settour (same card shape)."""
    offers: list[dict] = []
    cards = []
    for sel in card_selectors:
        cards = await page.query_selector_all(sel)
        if cards:
            print(f"  {source_id}: using selector {sel!r} ({len(cards)} cards)")
            break
    if not cards:
        print(f"  {source_id}: no listing cards found; site selectors may have changed")
        return offers

    for card in cards:
        try:
            title_el = await card.query_selector("a, h3, .title, .product-name, .product-title")
            if not title_el:
                continue
            title = await safe_text(title_el)
            if not title:
                continue
            url = await title_el.get_attribute("href") or ""
            if url and not url.startswith("http"):
                url = url_prefix + url

            price = None
            for sel in [".price-num", ".price", "[class*='price']", "[class*='Price']"]:
                el = await card.query_selector(sel)
                if el:
                    price = parse_price_twd(await safe_text(el))
                    if price:
                        break
            if not price:
                price = parse_price_twd(await safe_text(card))
            if not price:
                continue

            date_text = ""
            for sel in [".depart-date", ".date", "[class*='depart']", "[class*='date']"]:
                el = await card.query_selector(sel)
                if el:
                    date_text = await safe_text(el)
                    if date_text:
                        break
            depart_date = parse_chinese_date(date_text)
            if not depart_date or not (depart_start <= depart_date <= depart_end):
                continue

            n = parse_nights_from_title(title)
            if n is None or n != target_nights:
                continue

            return_date = (datetime.strptime(depart_date, "%Y-%m-%d")
                           + timedelta(days=target_nights)).strftime("%Y-%m-%d")

            departure_status = detect_departure_status(await safe_text(card))

            offers.append({
                "run_id": run_id,
                "offer_id": make_offer_id(source_id, region, depart_date, target_nights, url),
                "source_id": source_id,
                "dest_region": region,
                "depart_date": depart_date,
                "return_date": return_date,
                "nights": target_nights,
                "price_per_person_twd": price,
                "title": title,
                "url": url,
                "scraped_at": scraped_at,
                "hotel_name": None,
                "hotel_star_rating": None,
                "meals_included_count": None,
                "departure_status": departure_status,
                "seats_available": None,
                "min_group_size": None,
                "group_size_cap": None,
                "product_kind": detect_product_kind(title),
            })
        except Exception:
            continue
    return offers


# ── Dispatch + envelope ──────────────────────────────────────────────

SOURCE_HANDLERS = {
    "besttour": (besttour_url, scrape_besttour),
    "lifetour": (lifetour_url, scrape_lifetour),
    "settour": (settour_url, scrape_settour),
}


async def run_scrape(args) -> dict:
    if args.source not in SOURCE_HANDLERS:
        raise ValueError(f"Unknown source: {args.source}. Supported: {list(SOURCE_HANDLERS)}")
    url_builder, scrape_fn = SOURCE_HANDLERS[args.source]
    listing_url = url_builder(args.dest_region)
    scraped_at = datetime.utcnow().isoformat() + "Z"

    print(f"Listing URL: {listing_url}")

    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        ctx = await browser.new_context()
        page = await ctx.new_page()
        try:
            await page.goto(listing_url, wait_until="domcontentloaded", timeout=60000)
            await page.wait_for_timeout(3000)
        except Exception as e:
            print(f"  navigation failed: {e}")
            await browser.close()
            return _envelope(args, [], scraped_at)

        # Scroll to trigger lazy content.
        for _ in range(5):
            await page.evaluate("window.scrollTo(0, document.body.scrollHeight)")
            await page.wait_for_timeout(1500)

        offers = await scrape_fn(
            page, args.dest_region, args.nights,
            args.depart_start, args.depart_end,
            args.run_id, scraped_at,
        )
        await browser.close()

    return _envelope(args, offers, scraped_at)


def _envelope(args, offers: list[dict], scraped_at: str) -> dict:
    return {
        "run_id": args.run_id,
        "scraped_at": scraped_at,
        "source_id": args.source,
        "dest_region": args.dest_region,
        "nights": args.nights,
        "tour_group_offers": offers,
    }


def main():
    p = argparse.ArgumentParser(description="Tour-group listing scraper for Shaping Stage baseline.")
    p.add_argument("--source", required=True, choices=list(SOURCE_HANDLERS),
                   help="Agency source ID (besttour, lifetour, settour).")
    p.add_argument("--dest-region", required=True,
                   help="Region label (kansai, kanto, kyushu, hokkaido, okinawa).")
    p.add_argument("--nights", type=int, required=True,
                   help="Trip length in nights (filter — exact match against title).")
    p.add_argument("--depart-start", required=True, help="YYYY-MM-DD")
    p.add_argument("--depart-end", required=True, help="YYYY-MM-DD")
    p.add_argument("--run-id", required=True, help="Shaping Stage run ID (must match a pending attempt row).")
    p.add_argument("--output", required=True, help="Output JSON path.")
    args = p.parse_args()

    envelope = asyncio.run(run_scrape(args))
    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(envelope, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"✅ Wrote {len(envelope['tour_group_offers'])} offers to {out}")


if __name__ == "__main__":
    main()
