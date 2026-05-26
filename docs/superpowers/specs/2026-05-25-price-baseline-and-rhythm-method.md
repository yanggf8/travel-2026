# Optimal Travel Plan — Decision Methodology

**Status:** design — not yet implemented
**Author:** Yang
**Date:** 2026-05-25

## Framing

Two layers:

- **Outline (the search space):** Stage 0 triangle research — date × destination × flight, designed in `2026-05-22-new-planning-flow.md` and `2026-05-22-stage0-triangle-research-design.md`. It enumerates *what's possible*.
- **Methodology (the decision discipline):** this spec — *how* to pick the optimal plan from that search space.

The triangle gives candidates. The methodology decides which candidate is actually worth booking. Neither layer replaces the other; they compose.

## The goal

**Find the optimal travel plan** — not just the cheapest flight, not just the most convenient package, but the combination of date + destination + booking method that delivers the best trip for the least money.

Stage 0's default ranking (`flight_total_twd ASC`) answers one narrow question: which (date, destination) pair has the cheapest flight? It doesn't answer:

- Is this flight cheap enough to be worth assembling our own trip, vs just buying a tour group?
- Of all the candidates Stage 0 found, which ones sit at a *natural* price minimum (a sweet spot), vs which just happen to be one rung above the holiday peak?

The methodology below adds the two missing judgments.

## Two pillars of the methodology

1. **A baseline ceiling.** Without an absolute reference, "this flight is TWD 12,000" means nothing. With a reference ("the cheapest tour group for the same trip is TWD 40,000"), the flight price becomes a discount: 70% off the bundled alternative. Discounts are decidable; raw prices aren't.

2. **The rhythm around a holiday.** When the target window straddles a Taiwan public holiday, prices follow a predictable shape — peak on the holiday weekend, two sweet spots on either side. The cheapest candidate isn't always the right candidate; the cheapest *sweet-spot* candidate is. The sweet spots are structural (caused by consumer behavior around leave/weekends), not random.

---

## Method

### Baseline — the price ceiling

> **The price ceiling for the whole trip is the cheapest comparable tour group.**

A tour group (團體/跟團) bundles flight + hotel + transfers + some meals + a fixed itinerary at one price. It's the worst-case "I gave up shopping" option. Any FIT package, any flight+hotel split-booking, and any pure-flight + own-hotel combination **must come in cheaper than the tour group** — otherwise the tour group wins on convenience and we should book it instead.

So the baseline is:

```
baseline_ceiling_twd = MIN(tour_group_price_twd)
                      across all agencies (BestTour, Lion, Lifetour, Settour, Travel4U)
                      for comparable dates and destination
```

Every other option is then evaluated as a **discount vs. baseline**:

| Option | Compute |
|--------|---------|
| FIT package (機加酒, 自由行) | `(baseline_ceiling - fit_price) / baseline_ceiling` |
| Flight + own hotel | `(baseline_ceiling - (flight + hotel)) / baseline_ceiling` |
| Pure flight only | `(baseline_ceiling - hotel_estimate - flight) / baseline_ceiling` |

If any option's discount is **≤ 0**, the tour group wins. If discount > 0 and option meets other constraints (we don't want the tour group's hotel, fixed itinerary, etc.), pick the highest-discount option that meets constraints.

**This is a comparison method, not a research method.** It needs prices from sources Stage 0 doesn't currently scrape — specifically tour groups. Stage 0 candidates feed *into* this comparison; they don't replace it.

### Rhythm — the price shape around a Taiwan holiday

This is a refinement of how to *interpret* a Stage 0 candidate list, not a change to how Stage 0 produces it. Stage 0 already scrapes a window of dates; rhythm is what we look for in the resulting price-by-date table.

Around a Taiwan public holiday (端午節, 中秋節, 春節, 國慶, etc.), prices follow this shape:

```
price
 │                    ╱ holiday peak ╲
 │                  ╱                 ╲
 │              ╱╱                     ╲╲
 │   ╱─sweet──╱                          ╲──sweet─╲
 │  ╱  spot  ╱  ramp-up                    ramp-down  ╲
 │ ╱  (advance)  (3-5 days before)         (3-7 days after)  ╲
 │─────────────────────────────────────────────────────────────→ depart date
       T-7    T-4   T-2  T-0   T+1   T+3    T+5    T+7
                            (holiday)
```

Two sweet spots exist:

- **Advance sweet spot** — departures *before* the holiday weekend. People prefer to start their holiday on day T-0, so demand drops as you push earlier. But the drop is **not always clean** — usually there's a sharp cliff at T-3 or T-4 (the point where you'd have to take additional leave to make it worth going). The advance side is "noisy" because consumers stop advancing once leave cost outweighs price savings.

- **Delay sweet spot** — departures *after* the holiday peak. Prices drop faster and more reliably here because: (a) the holiday weekend is over, no consumer wants to start their trip with one weekday already burned; (b) people who can't get their preferred date almost always advance, not delay — they're protecting weekend coverage.

**Yang's heuristic:** the **delay sweet spot is usually cheaper and more reliable** than the advance sweet spot, because of the asymmetric consumer preference (people advance, don't delay, when forced to move).

To use rhythm:
1. Run Stage 0 with a window that includes **at least T-7 to T+7** around the relevant holiday (Stage 0 already supports arbitrary windows; this is a usage convention, not a new feature).
2. Read the candidate list as a price-by-date series, not just a ranked top-N.
3. Identify the local minima on both sides of the holiday peak — those are the two sweet spots.
4. Apply the baseline check to each sweet-spot candidate; the cheapest sweet-spot candidate that also beats the baseline is the winner.

---

## Where the methodology applies in the stage flow

```
Stage 0 — Triangle research          ← outline: enumerate candidates
   produces: candidate list (date × dest × flight price)

   ↓ apply methodology ↓

[Baseline + Rhythm decision]         ← this spec: pick the optimal candidate
   adds:    tour group ceiling (the baseline)
            rhythm reading of the candidate window
            discount-vs-baseline computation
   produces: chosen candidate = optimal under the methodology

   ↓ lock the choice ↓

stage0-adopt --create-plan           ← exists, unchanged

Stage 1 — Itinerary draft            ← exists, unchanged
Stage 2 — Shop transport             ← exists; this is where the methodology
                                        re-applies: at booking time, compare
                                        the tour-group baseline against the
                                        actual FIT package or split-booking
                                        prices for the locked date+dest
Stage 3, 4 — unchanged
```

The methodology applies **twice**:
- **Before lock** (between Stage 0 and `stage0-adopt`): to choose the optimal date+destination from Stage 0's candidate list.
- **At booking** (inside Stage 2): to choose the optimal booking method (tour group vs FIT package vs split-booking) for the locked date+destination.

Same method, applied at two decision points. The triangle finds candidates; the methodology gates both the lock-in and the booking.

---

## What needs to exist for the comparison to work

Things this method needs that don't exist today:

| Need | Today's state |
|------|---------------|
| Tour group prices for the candidate window | **Not scraped at all.** Zero tour-group scrapers exist. Currently only manual lookup. |
| A field to record `baseline_ceiling_twd` per Stage 0 run | Not in `stage0_research_runs` schema. |
| A field on candidates for `discount_vs_baseline_pct` | Not in `stage0_candidates` schema. |
| A view that shows price by date (rhythm) rather than ranked by price | `stage0-compare` ranks by `flight_total_twd ASC, leave_days ASC, depart_date ASC` — doesn't preserve date order. |
| Calendar awareness so the system can highlight holiday peaks | `data/holidays/taiwan-2026.json` exists, but `stage0-compare` doesn't read it. |

None of these are blockers for using the method **manually** on the June trip. They become important if we want the system to surface "best decidable candidate" automatically rather than requiring me/Yang to eyeball.

---

## Agency scraping difficulty (lessons from the BestTour build)

Empirically, Taiwan travel agency price discovery splits into two tiers:

**Tier 1 — server-rendered listings (automatable):**
- **BestTour 喜鴻** — `/e_web/activity?v=japan_<region>` renders cards server-side with title, day-count, and price embedded in card text. Tour-group URLs encode destination/days/depart-date in the path (`/itinerary/OSA05BR260612KM`). Cheap to scrape: one HTTP load, parse anchors, done. **This is the only Tier 1 agency we've found.**

**Tier 2 — client-side search flows (automation projects):**
- **Settour 東南** — landing pages embed `cmsPrice` URL parameters as **teasers** (1-night-base), but real per-night prices require driving the search UI (open hidden panel, fill dates, click Search, wait for async results).
- **LionTravel 雄獅** — FIT site (`vacation.liontravel.com`) is structured around a 1-night base with `+N/night` extensions; needs both per-night-rate capture and arithmetic to derive the full-trip price.
- **EzTravel 易遊網** — `packages.eztravel.com.tw/roundtrip-TPE-<dest>` ignores URL date parameters; needs date-picker UI automation.
- **ezfly 易飛旅遊** — not yet probed; likely similar.
- **Lifetour 五福** — has Okinawa FIT but search-flow not yet probed.

Each Tier 2 agency is roughly half a day of Playwright automation work to scrape reliably. For a single trip, that investment doesn't pay off.

**Decision rule:** For the immediate trip, automate Tier 1 only (BestTour); use **manual browser lookups** for Tier 2 agencies, recorded into the same `stage0_tour_group_offers` table with provenance fields (`raw_json` JSON tagging `source: manual_browser_lookup`, `confidence: low|medium|high`, `observed_by: yang`, `observed_at: ISO timestamp`). Re-evaluate building Tier 2 scrapers only when we have multiple trips to compare and the same pattern emerges.

**Confidence tiers for manual entries:**
- **high** — landing-page or product page shows a *bookable* price for our exact `(depart_date, nights)`. Rare.
- **medium** — listed price seen on a search-results page or landing-card; not confirmed at checkout.
- **low** — teaser price (e.g. Settour `cmsPrice` in URL, base price before extensions, "starting from" copy).

Manual rows should always carry a confidence tag in `raw_json`. The methodology layer can then weight or filter when computing the ceiling.

### BestTour listing-page reliability — scraper has a title/price corruption bug (Okinawa June 2026 lesson)

A real failure case discovered during the June trip, and the lesson is broader than expected:

**The bug:** The BestTour two-pass scraper (`scripts/scrape_tour_groups.py`) indexes title anchors by `(dest_code, days, airline)` and resolves the cheapest title per key. When multiple distinct products share the same `(dest, days, airline)` bucket, the scraper collapses them — the **cheapest-title-wins tie-breaker attaches one product's title and price to OTHER products' date anchors** in the same bucket.

Concrete example for product URL `OKA04FD260626X`:
- **Scraper output**: title `【機加酒．沖繩自由行４日】那霸露櫻GRANTIA、亞洲航空、行李20公斤`, price `TWD 12,999`
- **Real product page**: title `【微旅沖繩４日】玉泉洞琉球大鼓秀、海葡萄採摘、美麗海水族館、萬座毛、保證入住美國村飯店乙晚`, price `TWD 29,900`

These are **completely different tours** — different itineraries, different price tiers — but they share the same `OKA04FD` URL prefix and got merged.

**Two methodology corrections from this:**

1. **The BestTour automation is less reliable than initially claimed.** Listing-page text prices agree with product-page prices for SOME rows but not all. Specifically, when multiple distinct products share a `(dest, days, airline)` key, the title and price for date anchors in that bucket may be **mismatched to a different product**. Confidence on any individual BestTour row drops to **medium** unless the row is the only product in its `(dest, days, airline)` bucket.

2. **`機加酒/自由行` in BestTour titles is unreliable for `product_kind` classification.** A title with `自由行` may describe a fixed-departure tour with self-guided activities at the destination, not true FIT. Title keyword check passes too easily. Real FIT signal: ability to pick any depart date, any flight, any return — product page must offer date/flight selection rather than a fixed-departure list.

**Action item for the scraper (deferred — out of scope for this spec):** Index BestTour title anchors by the full URL (including suffix code like `260626X`), not by `(dest, days, airline)`. Or: skip the title-pass deduplication entirely and scrape each anchor independently. Without this fix, all BestTour group-tour data should be treated as medium-confidence, with spot checks against the actual product page before any booking decision.

**Updated decision rule for this trip:** Use BestTour listing data as a **starting lead** for which products exist in the window. Always cross-check the specific product URL on BestTour's site before relying on its title or price. Especially before any booking decision, verify the **product page** shows the title/price our DB recorded.

---

## What to do for the June 2026 trip (manual workflow)

This is what we should actually do for the immediate trip — no new code yet, just executing the method by hand:

1. **Tour group baseline pass (manual, ~30 min):**
   - For KIX, SDJ, FUK in the June 14–28 window, look up tour groups on the 5 agency sites.
   - Record the cheapest tour group price per dest as `baseline_ceiling_twd_<dest>`.
   - Cheapest tour group, not best — we want the ceiling.

2. **Run Stage 0 as designed:**
   - The aggregator already covers the date × destination × flight triangle.
   - Window June 14–28 already includes T-7 to T+7 around 端午節 (Sat June 20).

3. **Read the Stage 0 output for rhythm, not just rank:**
   - Look at price by departure date, not just the top-5.
   - Identify the advance sweet spot (likely June 14–17) and delay sweet spot (likely June 23–28).
   - Note the day-over-day deltas.

4. **Compute discount manually for the top sweet-spot candidates:**
   - For each sweet-spot candidate, add a hotel estimate (~TWD 3000–6000/night depending on dest and tier).
   - `discount = (baseline_ceiling - (flight + hotel*nights)) / baseline_ceiling`
   - Pick the highest discount that also fits constraints (hotel quality, schedule).

5. **Adopt that candidate into a plan** via `stage0-adopt --create-plan`.

This whole loop is throwaway for one trip, but each step surfaces what the system *would* need to be doing automatically — that informs what we build next.

---

## Open questions before any implementation

1. **What defines "comparable" for the tour group baseline?** Same dest, same nights, same season — but tour groups vary by hotel quality (3-star vs 4-star), included meals, escort. Do we take the cheapest tour group regardless of quality, or the cheapest one above some floor (e.g. 4-star)?

2. **Rhythm detection — algorithmic or visual?** A simple "local minima on either side of the holiday peak" works for most cases but breaks with noisy data (e.g. one airline's flash sale on T-2). Do we want moving-average smoothing, or just present the daily prices and eyeball?

3. **Which Taiwan holidays are in scope?** 春節, 二二八, 清明, 端午, 中秋, 國慶, plus single-day holidays creating long weekends. If the rhythm method is general, the system should know all of them. Calendar data exists in `data/holidays/taiwan-2026.json`.

4. **When (if ever) do we build the tour-group scrapers?** Without them, the baseline is permanently manual. The first scraper is the hardest; once one agency works, the others should follow the same pattern. Worth doing eventually, but not for one trip.
