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

**Note**: "keep 21" — the 6/21 LionTravel 水之都 at 14,499 remains a strong reference point.

**Shaping is multi-role and evolves** (all stored in stage0_research_shaping):
- Early entries: date:hard_constraint:return_no_later_than + exclude_depart (original Liko schedule)
- Later entries (after Liko response): observed_signal:liko_prefers_late + soft_preference:explore_after_28

We do not treat only the initial hard date rules as the sole filter. The soft/observed shaping added on 2026-05-27 now actively guides the research toward later dates. "Alignment" with shaping considers the full set.

## Current High-Value FIT Baseline — Pre-28 Window (kept for reference)

Strongest specific-hotel options captured so far (return ≤ 2026-06-27):

- LionTravel 水之都那霸飯店 (Aqua Citta Naha)
  - 6/14–17 @ 14,499
  - 6/21–24 @ 14,499 (with flight details) ← **keep 21** per current preference

- BestTour WBF水之都那霸酒店
  - 6/12–15 @ 16,888 (earliest strong window)

- Travel4U 水之都那霸飯店
  - 6/21–24 @ 16,199

- Lifetour Mercure Okinawa Naha
  - 6/14 @ 18,900

Many Funtime "自選市區" at ~11.9k–13k as price floor.

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
