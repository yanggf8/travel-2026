---
name: stage2-shop-transport
description: Shop direct flights and flight+hotel packages, then decide the best booking path. Owns Stage 2 of the adopted research-first planning flow.
version: 1.0.0
requires_skills: [travel-shared, p3-flights, p3p4-packages, separate-bookings, scrape-ota]
requires_processes: [process_1_date_anchor, process_2_destination]
provides_processes: [process_3_transportation, process_3_4_packages, process_4_accommodation]
---

# /stage2-shop-transport

Orchestration skill for **Stage 2 — Shop Flight / Package** of the adopted
research-first planning flow (`docs/plans/2026-05-22-new-planning-flow.md`).

Stage 2 starts after Stage 1 has produced a viable rough itinerary and lodging
topology. Its job is to compare direct flight booking against flight+hotel
packages, select the best booking path, and record the chosen transport and
lodging state through the existing P3/P4 tools.

## When to use

- User says "find packages", "search OTA", "shop flights", "compare direct vs
  package", "book separately", or "which offer should we take".
- Dates and destination are locked, and the rough itinerary has enough shape to
  know whether a single-base package can work.

Do **not** use this while dates/destination are still flexible. If cheaper
flights on adjacent dates would change the trip, return to `/stage0-research`.

## Workflow

1. **Verify inputs**
   ```bash
   npm run view:status -- --plan-id <plan_id>
   ```
   Confirm:
   - P1 dates are confirmed.
   - P2 destination is confirmed.
   - Stage 1 has a rough day shape and lodging topology assumption.

2. **Check package data freshness**
   ```bash
   npm run travel -- check-freshness --source <source_id> --region <region>
   ```
   If data is stale or missing, use `/p3p4-packages` to scrape and import:
   ```bash
   npm run scraper:batch -- --dest <region> --date <YYYY-MM-DD> --type fit
   npm run travel -- import-offers --dir scrapes --dest <destination_slug>
   ```

3. **Query package options**
   ```bash
   npm run travel -- query-offers --plan-id <plan_id> --dest <destination_slug> --json
   ```
   Shortlist only offers whose flight times, hotel location, room type,
   cancellation terms, and lodging topology match the Stage 1 draft.

4. **Compare direct booking**

   Use `/p3-flights` for flight-only search and `/separate-bookings` for the
   package-vs-direct cost comparison. Compare total trip cost, not just airfare:
   - Direct flight total.
   - Comparable hotel total.
   - Package total.
   - Leave days.
   - Hotel/location quality.
   - Cancellation and booking constraints.

5. **Apply the Stage 2 decision rule**

   - Package wins if it has acceptable flight times and hotel terms, and the
     total is within 10% of direct flight plus comparable hotel.
   - Separate booking wins if the package hotel/location is weak, cancellation
     terms are poor, room type is wrong, or the user needs lodging flexibility.
   - Return to Stage 0 if materially better prices require different dates.
   - Return to Stage 1 if the lodging topology must change before shopping can
     continue.

6. **Record the selected path**

   Package path:
   ```bash
   npm run travel -- select-offer <offer_id> <YYYY-MM-DD>
   npm run view:transport
   npm run travel -- query-bookings --category package
   ```

   Separate path:
   ```bash
   npm run travel -- set-flight outbound --dest <destination_slug> ...
   npm run travel -- set-flight return --dest <destination_slug> ...
   npm run travel -- set-hotel --dest <destination_slug> ...
   npm run view:transport
   npm run travel -- query-bookings
   ```

## Output

End with:
- A ranked comparison of viable package and direct-booking options.
- The recommended booking path and why.
- Any blocked assumptions, especially hotel topology or flight timing.
- One next action: select/book, return to Stage 0, return to Stage 1, or proceed
  to Stage 3 after booking is confirmed.

## Notes

- Always compare package vs direct on comparable hotel quality and location.
  A cheaper package is not viable if its hotel breaks the Stage 1 itinerary.
- Use existing `/p3-flights`, `/p3p4-packages`, and `/separate-bookings` skills
  for the lower-level work; this skill owns the Stage 2 decision.
- After transport and lodging are confirmed, move to Stage 3
  (`/p5-itinerary`) for detailed daily planning.
