---
name: stage3-expand-itinerary
description: Expand the rough itinerary into a booking-aware detailed daily plan. Owns Stage 3 of the adopted research-first planning flow.
version: 1.0.0
requires_skills: [travel-shared, p5-itinerary]
requires_processes: [process_1_date_anchor, process_2_destination, process_3_transportation, process_4_accommodation]
provides_processes: [process_5_daily_itinerary]
---

# /stage3-expand-itinerary

Orchestration skill for **Stage 3 — Expand Itinerary** of the adopted
research-first planning flow (`docs/plans/2026-05-22-new-planning-flow.md`).

Stage 3 starts after transport and lodging are selected or booked. Its job is
to turn the Stage 1 rough draft into a detailed, validated day-by-day itinerary
that respects real flight times, hotel location, package guided/free days,
fixed-time activities, meals, and transit.

Use `/p5-itinerary` for the lower-level itinerary planning rules and CLI
commands. This skill owns the stage-level sequencing and validation gates.

## When to use

- User says "plan the days", "expand itinerary", "fill the itinerary",
  "rebalance day 3", "add a must-see", or "validate the schedule".
- P1/P2 are confirmed and Stage 2 has selected transportation and lodging, or
  the user explicitly wants a provisional detailed pass before final booking.

Do **not** use this for the first coarse draft before shopping; use
`/stage1-itinerary-draft` instead.

## Workflow

1. **Verify prerequisites**
   ```bash
   ./bin/travel status --full --plan-id <plan_id>
   ./bin/travel transport
   ```
   Confirm:
   - Dates and destination are confirmed.
   - Flight times or package transport assumptions are known.
   - Hotel/base location is known enough to plan daily transit.
   - Existing scaffold days are present, or create them with
     `scaffold-itinerary`.

2. **Refresh or create day skeletons**
   ```bash
   ./bin/travel scaffold-itinerary --plan-id <plan_id> --dest <destination_slug>
   ```
   Use `--force` only when the user explicitly approves replacing existing
   days. If Stage 1 already scaffolded days, continue from the existing draft.

3. **Assign clusters and must-do activities**
   ```bash
   ./bin/travel populate-itinerary --goals "<cluster1,cluster2>" --pace balanced --dest <destination_slug>
   ```
   Scheduling order:
   - Fixed-time bookings.
   - Must-do activities.
   - Guided/package portions.
   - Want/nice-to-have activities.
   - Meals and transit buffers.

4. **Set timing, themes, and transit**
   ```bash
   ./bin/travel set-activity-time <day> <session> "<activity>" --start HH:MM --end HH:MM --fixed true
   ./bin/travel set-day-theme <day> "<theme>" --zh "<zh_title>" --dest <destination_slug>
   ./bin/travel set-tod-zh <day> <session> --zh "<focus_zh>" --transit-zh "<transit_zh>"
   ```
   Default meals are lunch and dinner only. Add breakfast only when hotel or
   package terms include it, or when the user asks for it.

5. **Validate and rebalance**
   ```bash
   ./bin/travel validate-itinerary --dest <destination_slug> --severity warning
   ```
   Fix:
   - Time conflicts.
   - Overpacked days.
   - Cross-city inefficiency.
   - Missing transit buffers.
   - Booking deadlines.
   - Meal gaps that matter for the user.

6. **Confirm readiness for Stage 4**

   Move to Stage 4 only when the itinerary is detailed enough to publish:
   - Arrival/departure logistics are represented.
   - Each full day has a coherent area focus.
   - Fixed activities and booking requirements are explicit.
   - Validation has no unresolved errors.

## Output

End with:
- A concise day-by-day detailed itinerary summary.
- Validation result and any remaining warnings.
- Bookings or activities still needing confirmation.
- One next action: revise Stage 3, update bookings, fetch weather, or proceed
  to Stage 4 (`/stage4-publish-dashboard`).

## Notes

- Keep all itinerary content in DB-backed CLI/state paths. Do not hardcode
  dashboard content.
- Prefer session-based itinerary format for new plans.
- If a package includes guided days, preserve those fixed portions and plan only
  free or semi-free time around them.
