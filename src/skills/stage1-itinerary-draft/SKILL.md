---
name: stage1-itinerary-draft
description: Draft and validate the first rough itinerary after dates and destination are locked. Owns Stage 1 of the adopted research-first planning flow.
version: 1.0.0
requires_skills: [travel-shared, p1-dates, p2-destination]
requires_processes: [process_1_date_anchor, process_2_destination]
provides_processes: []
---

# /stage1-itinerary-draft

Orchestration skill for **Stage 1 — Itinerary Draft** of the adopted
research-first planning flow (`docs/plans/2026-05-22-new-planning-flow.md`).

Stage 1 starts after the user has locked dates and destination, usually through
`stage0-adopt --create-plan --dest`. Its job is to create a rough day-by-day
shape, validate that the duration and lodging topology make sense, and decide
whether to proceed to Stage 2 or return to Stage 0 with revised constraints.

This is not the detailed itinerary pass. Detailed activities, exact timings,
booking status, and full transit notes remain Stage 3
(`/stage3-expand-itinerary`) after transport and lodging are known.

## When to use

- User says "draft the trip", "rough itinerary", "does this date/destination
  work", or "continue after locking the Stage 0 candidate".
- A plan exists with confirmed P1 dates and P2 destination, whether created by
  `stage0-adopt --create-plan` or by the older `/p1-dates` + `/p2-destination`
  path.

Do **not** use this before dates and destination are locked. If the user is
still comparing dates, destinations, or flight prices, use `/stage0-research`.

## Workflow

1. **Verify plan state**
   ```bash
   npm run view:status -- --plan-id <plan_id>
   ```
   Confirm:
   - P1 dates are confirmed.
   - P2 destination is confirmed.
   - `destination_config` slug matches the active destination.

   If the plan came from Stage 0, these rows should already exist. If not, run
   `/p1-dates` and `/p2-destination` before continuing.

2. **Create the day skeleton**
   ```bash
   npm run travel -- scaffold-itinerary --plan-id <plan_id> --dest <destination_slug>
   ```
   Use `--force` only when the user explicitly wants to replace an existing
   rough draft.

3. **Draft the rough shape**

   Create a concise day-by-day proposal:
   - Arrival day: airport, hotel/base area, light evening only.
   - Full days: one primary area or city cluster per day.
   - Departure day: checkout, light activity only if flight time allows.
   - Meals: lunch and dinner only by default; breakfast only if the selected
     hotel/package includes it or the user asks for it.

4. **Decide lodging topology before Stage 2**

   For multi-city trips, recommend one of:
   - `split-stay`: better pacing, more hotel logistics.
   - `day-trip`: simpler hotel logistics, more repeated transit.
   - `single-city`: one base, no inter-city lodging decision.

   This matters before package shopping because many packages assume one hotel
   base.

5. **Validate viability**

   Check whether the draft works with:
   - Trip duration.
   - Arrival/departure day assumptions.
   - Must-do activities already known.
   - Package/direct-booking strategy.
   - Cross-city transit load.

   If timing, duration, or destination choice looks wrong, return to Stage 0
   with a narrower research request. If the rough plan is viable, proceed to
   Stage 2 (`/stage2-shop-transport`).

## Output

End with:
- A day-by-day rough itinerary.
- Recommended lodging topology.
- Any assumptions that could affect booking.
- One next action: proceed to Stage 2, revise Stage 1, or return to Stage 0.

## Notes

- Keep this draft intentionally coarse. Do not overfit exact attraction timing
  before flights and accommodation are selected.
- Use existing P1/P2 skills for revisions; Stage 1 is the decision wrapper, not
  a replacement for those lower-level write paths.
- Existing `/p1-*` through `/p5-*` skill names remain compatibility
  implementation tools inside the adopted Stage 0 through Stage 4 flow.
