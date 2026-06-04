# Okinawa 2026 Stage 0 Research — Baseline Checkpoint

**Run ID**: `stage0-20260525-093508`  
**Date saved**: 2026-05-27  
**Last updated**: 2026-05-27 (Liko preference change)  
**Focus**: Pre-lock triangle research (dates × destinations × FIT baseline pricing)  
**Key Goal**: Establish price rhythm / ceiling using real competitor FIT offers (especially good hotels)

## Shaping Rules (hard + soft constraints captured)

**Initial (before 2026-05-27):**
- date:hard_constraint:return_no_later_than: 2026-06-27
- date:hard_constraint:exclude_depart: 2026-06-28 (Liko 馬偕)

**Updated 2026-05-27 (Liko response):**
- Liko prefers later dates → previous hard cap of 6/27 is relaxed
- date:observed_signal:liko_prefers_late: true
- date:soft_preference:explore_after_28: 2026-06-28+
- lodging:soft_preference:preferred_hotel_area: 中央那霸 Yui-rail 可步行 / 水之都那霸飯店優先 (飯店要好，優先有泳池的知名飯店)
- channel:soft_preference:preferred_sources: liontravel, besttour, travel4u, lifetour

**2026-06-04 live re-check**:
- 6/12 Aqua Citta-style leads are now rejected as too early for this trip.
- LionTravel phone confirmation: the 6/21–6/24 Aqua Citta / 水之都 offer at 14,499 is no longer being sold.
- Funtime 6/21-only page still shows the LionTravel Aqua Citta anchor, but it is stale relative to the phone confirmation and should not be treated as bookable.
- Funtime no longer exposes a Travel4U / 山富 6/21 Aqua Citta link. The visible 6/21 water-city match is LionTravel only.
- The old Travel4U 6/21–6/24 @ 16,199 entry remains a May 27 historical reference, not a live-confirmed option.

**2026-06-04 Liko shaping update**:
- Liko can accept a 2026-06-13 departure if the package quality is good.
- This reopens 6/13 as an actionable date, but not generic early-June leads: prioritize good central Naha / Yui-rail walkable hotels.
- BestTour 逸之彩 6/13 is now a serious watchlist candidate; both 4-day and 5-day variants exist.

**Shaping is multi-role and evolves** (all stored in stage0_research_shaping):
- Early entries: date:hard_constraint:return_no_later_than + exclude_depart (original Liko schedule)
- Later entries (after Liko response): observed_signal:liko_prefers_late + soft_preference:explore_after_28

We do not treat only the initial hard date rules as the sole filter. The soft/observed shaping added on 2026-05-27 now actively guides the research toward later dates. "Alignment" with shaping considers the full set.

## Current High-Value FIT Baseline — Pre-28 Window (kept for reference)

Strongest specific-hotel options captured so far (return ≤ 2026-06-27):

- LionTravel 水之都那霸飯店 (Aqua Citta Naha)
  - 6/14–17 @ 14,499
  - 6/21–24 @ 14,499 (with flight details) — **stale; phone-confirmed no longer sold on 2026-06-04**

- BestTour WBF水之都那霸酒店
  - 6/12–15 @ 16,888 — rejected as too early

- Travel4U 水之都那霸飯店
  - 6/21–24 @ 16,199 — historical May 27 capture; not live-confirmed on 2026-06-04

- Lifetour Mercure Okinawa Naha
  - 6/14 @ 18,900

Many Funtime "自選市區" at ~11.9k–13k as price floor.

## Current Watchlist — 2026-06-04 funtime live page

After LionTravel phone-confirmed the 6/21 Aqua Citta option is no longer sold, watchlist focus shifted to similarly decent central Naha options that still align with the 6/21 reference date.

- BestTour / 喜鴻: 逸之彩溫泉度假飯店
  - 6/13–6/16 (3n / 4 days) @ 19,888 TWD/person, available seats: 2
  - URL: https://www.besttour.com.tw/itinerary/OKA04MM260613AT?fc=ft
  - DB offer_id: besttour-okinawa-20260613-3n-mpyzyua0
  - Flights: MM922 TPE 09:35 → OKA 12:20; MM929 OKA 16:45 → TPE 17:20
  - Hotel: twin room 20㎡, 120cm × 2 beds, 3 nights; Makishi Station ~1 min walk, Kokusai-dori ~6 min walk
  - Notes: Peach / 樂桃, 20kg checked baggage, outdoor pool + natural hot spring
  - Booking caveat: BestTour page says order requires customer-service confirmation in ~1–3 business days; availability is not final until confirmed.

- BestTour / 喜鴻: 逸之彩溫泉度假飯店
  - 6/13–6/17 (4n / 5 days) @ 20,800 TWD/person, available seats: 2
  - URL: https://www.besttour.com.tw/itinerary/OKA05MM260613EF?fc=ft
  - DB offer_id: besttour-okinawa-20260613-4n-mpyzyub0
  - Flights: MM924 TPE 14:50 → OKA 17:35; MM927 OKA 13:15 → TPE 13:50
  - Hotel: twin room 20㎡, 120cm × 2 beds, 4 nights; Makishi Station ~1 min walk, Kokusai-dori ~6 min walk
  - Notes: Peach / 樂桃, 20kg checked baggage, outdoor pool + natural hot spring
  - Booking caveat: BestTour page says order requires customer-service confirmation in ~1–3 business days; availability is not final until confirmed.

- BestTour / 喜鴻 via funtime: 逸之彩溫泉度假飯店
  - 6/21–6/24 (3n / 4 days) @ 18,988 TWD/person
  - URL: https://www.besttour.com.tw/itinerary/OKA04MM260621AT?fc=ft
  - DB offer_id: besttour-okinawa-20260621-3n-mpyxqrds
  - Flights: MM922 TPE 09:35 → OKA 12:20; MM929 OKA 16:45 → TPE 17:20
  - Hotel: twin room 20㎡, 120cm × 2 beds, 3 nights; Makishi Station ~1 min walk, Kokusai-dori ~6 min walk
  - Notes: Peach / 樂桃, 20kg checked baggage, outdoor pool + natural hot spring; funtime data-id `ID:BEST_23632_CT_OKINAWA`
  - Booking caveat: BestTour page says order requires customer-service confirmation in ~1–3 business days; availability is not final until confirmed.

- BestTour / 喜鴻 via funtime: 逸之彩溫泉度假飯店
  - 6/21–6/25 (4n / 5 days) @ 20,300 TWD/person
  - URL: https://www.besttour.com.tw/e_web/travel?v=OKA05MM260621EF&fc=ft
  - DB offer_id: besttour-okinawa-20260621-4n-mpyxqsm9
  - Flights: MM924 TPE 14:45 → OKA 17:30; MM927 OKA 13:15 → TPE 13:50
  - Hotel: twin room 20㎡, 120cm × 2 beds, 4 nights; Makishi Station ~1 min walk, Kokusai-dori ~6 min walk
  - Notes: Peach / 樂桃, 20kg checked baggage, outdoor pool + natural hot spring; funtime data-id `ID:BEST_23674_CT_OKINAWA`
  - Booking caveat: BestTour page says order requires customer-service confirmation in ~1–3 business days; availability is not final until confirmed.

## Post-28 June Exploration (new focus after Liko feedback 2026-05-27)

Liko prefers later dates (captured as observed_signal + soft_preference in DB).

We are now actively researching departures on/after 2026-06-28. These are evaluated against the **full current shaping** (including the later soft/observed entries), not only the initial hard date constraints.

Current captured good-hotel options in the post-28 window:
- KKday 水之都那霸飯店 (Aqua Citta Naha)
  - 6/29–7/3 (4n) @ 17,900
  - 6/30–7/4 (4n) @ 18,500

More data needed from BestTour / LionTravel / Lifetour / Travel4U for late June and early July with decent hotels (水之都 or similar quality).

## Files in this checkpoint

- `stage0-export-*.json` — Full research run + shaping (standard handoff format)
- `tour-group-offers-*.json` — All manually captured FIT/tour-group offers (the primary baseline data)
- `shaping-*.txt` — Explicit shaping rules

## Next after save (2026-05-27 update)

- Constraint relaxed per Liko's preference for later dates.
- New focus: explore post-28 June options while keeping the 6/21 水之都 as reference.
- Ready for new agency data (late June / early July searches).

Data is durably preserved in DB + this research checkpoint.
