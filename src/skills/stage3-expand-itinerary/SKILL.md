---
name: stage3-expand-itinerary
description: Expand the rough itinerary into a booking-aware detailed daily plan — cascade routes via derive-routes, agent-author AI-recommended (labeled) meals/depth, and drive completeness with the content-depth signal. Owns Stage 3 of the adopted research-first planning flow.
version: 1.3.0
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

5. **Derive route segments (deterministic cascade — run this FIRST)**

   `scaffold`/`populate` write activities only. `derive-routes` CASCADES the
   route skeleton from them — one `ai_recommended` transit leg between each pair
   of consecutive same-day activities (using their POI stations + destination
   transit metadata). Run it once after populate; it's idempotent and re-runnable
   after any activity edit:
   ```bash
   ./bin/travel derive-routes --dest <destination_slug>          # all days
   ./bin/travel derive-routes --day <N> --dest <destination_slug> # re-run one day after edits
   ```
   It never clobbers a `confirmed` route (a day you've hand-finalized is skipped),
   and its legs are labeled `ai_recommended` — refine or confirm them like any
   other suggestion. Only hand-author routes (`set-route-segments-bulk
   --recommended`) where derive can't (missing stations, a walk-chain it skipped).

6. **Enrich the remaining depth (agent-first, LABELED)**

   After derive-routes, the gaps that DON'T cascade are MEALS (no reference data —
   agent research) and any thin activities. **Do not leave them empty for the user
   to hand-fill.** This is agent-first work (like OTA extraction): the agent
   researches and authors the depth, then persists it **labeled as AI-recommended
   (unconfirmed)** with the `--recommended` flag. Real-but-labeled is honest — a
   transparent suggestion to confirm, never a fabricated fact or a claimed booking.

   **Meals — aim for per-day completeness** (this is what a real trip has): a
   lunch (noon) AND a dinner (evening) on every full day; a dinner on arrival day
   and a lunch on departure day when the timing fits. Real restaurants only (see
   the restaurant-pick rules: nearest to the site, Google-rating-trusted,
   authentic; always a main + a backup; never phone-only).
   ```bash
   ./bin/travel set-meals <day> <session> --meal "<label>｜map:<real place + area>" --recommended --dest <destination_slug>
   # Extra want/nice-to-have activities where the skeleton is too thin.
   ./bin/travel add-activity <day> <session> "<title>" --recommended --dest <destination_slug>
   # Hand-author a route only where derive-routes couldn't (clean stop names; notes in the notes field):
   ./bin/travel set-route-segments-bulk <day> --seg "<from>|<to>|<mode>|<min>||<notes>" --recommended --dest <destination_slug>
   ```

   Rules for authored content:
   - **Real data only.** Confirm a real place name + area before writing a
     `map:` pin; never invent a Japanese place name; gloss kana with Chinese for
     the user. If unconfirmed, label the pin generically rather than guessing.
   - **`--recommended` is mandatory** on every meal/route/activity the AGENT
     authored. User-provided facts (a booked restaurant, a confirmed transfer) are
     entered WITHOUT `--recommended` (they are `confirmed`). (Typos fail loud —
     the write commands reject unknown flags — so a mistyped `--recommended` won't
     silently write `confirmed`.)
   - **Set the session ZH in the SAME pass.** Adding the FIRST content (meal /
     route / activity) to a previously-empty session makes it *non-empty*, which
     means the Stage-4 `validate publish` gate will BLOCK until that session also
     has `focus_zh` (and `transit_notes_zh` if it has transit). So every session
     you enrich, also run
     `set-tod-zh <day> <session> --zh "<focus_zh>" [--transit-zh "<transit_zh>"]`
     right here — do NOT defer it to Stage 4, or the publish gate fails a full
     stage later with no breadcrumb back to the enrichment.
   - The dashboard renders these with a `🤖 AI-recommended (unconfirmed)` badge;
     `validate publish` reports the count as INFO (never a blocker); the user flips
     the ones they accept with `confirm-recommendations` (step 8).

7. **Validate + use the content-depth signal as the gap list**
   ```bash
   ./bin/travel validate-itinerary --dest <destination_slug> --severity warning
   ./bin/travel validate publish --plan-id <plan_id>    # content-depth WARN/INFO = your remaining-depth checklist
   ```
   `validate-itinerary` catches mechanics (time conflicts, overpacked days,
   cross-city inefficiency, missing transit buffers, booking deadlines).
   `validate publish` adds **content-depth** WARN/INFO — the concrete gap list to
   drive back into step 6: "day N may be thin: missing dinner meal (evening)" or
   "day N: X activities but 0 route segments". Keep filling (agent-first meals,
   re-run `derive-routes --day N` for routes) until the content-depth WARNs are
   gone or you've consciously accepted a compact day. These are WARN/INFO, never
   blockers — a labeled, still-thin draft is a valid pre-trip state.

   After the content-depth WARNs are addressed, run the depth ORACLE against a
   known-good reference:
   `./bin/travel compare content-depth --plan-id <plan_id> [--against okinawa-2026]`.
   Treat the `SHORT: <axes>` line as the enrichment worklist — enrich the named
   axes (agent-first meals on the SHORT days, re-run `derive-routes --day N` for
   routes), then re-compare. Repeat until `VERDICT: BETTER`.
   **This is a mid-loop oracle, NOT final acceptance.** The final gate is the
   deployed dashboard page reviewed side by side with the reference (Stage 4).

8. **Surface AI-recommended items for confirmation**

   **First, LIST what the agent authored and present it to the user** — don't just
   flip it silently. This works BEFORE the dashboard is deployed (Stage 4), so the
   user can review during Stage 3:
   ```bash
   ./bin/travel query-recommendations --plan-id <plan_id> --dest <destination_slug>
   ```
   It prints the AI-recommended meals/routes/activities grouped by kind, each with a
   `Day N session` scope hint. Render that list to the user and ask which to accept.

   Then flip ONLY the approved items from `ai_recommended` → `confirmed` (same
   filters as `query-recommendations`, so preview then confirm the same scope):
   ```bash
   ./bin/travel confirm-recommendations [--day N] [--session s] [--kind activity|meal|route] --plan-id <plan_id> --dest <destination_slug>
   ```
   Scope by `--day`/`--session`/`--kind`, or run bare to confirm everything the
   user approved. Leave un-confirmed items labeled — a labeled suggestion is a
   valid pre-trip state (`validate publish` will not block on it; it counts them as
   INFO, and the dashboard badges them 🤖 once deployed).

9. **Confirm readiness for Stage 4**

   Move to Stage 4 only when the itinerary is detailed enough to publish:
   - Arrival/departure logistics are represented.
   - Each full day has a coherent area focus.
   - Fixed activities and booking requirements are explicit.
   - Validation has no unresolved errors.
   - AI-recommended items are either user-confirmed or intentionally left labeled.

## Output

End with:
- A concise day-by-day detailed itinerary summary.
- Validation result and any remaining warnings.
- A count of AI-recommended (unconfirmed) items awaiting the user's confirmation.
- Bookings or activities still needing confirmation.
- One next action: revise Stage 3, confirm AI-recommended items, update bookings,
  fetch weather, or proceed to Stage 4 (`/stage4-publish-dashboard`).

## Notes

- Keep all itinerary content in DB-backed CLI/state paths. Do not hardcode
  dashboard content.
- Prefer session-based itinerary format for new plans.
- If a package includes guided days, preserve those fixed portions and plan only
  free or semi-free time around them.
- **Agent-first + labeled provenance.** The agent AUTHORS meals/transit/extra
  activities (real data) rather than dumping the hand-work on the user — but every
  agent-authored item carries `--recommended` (source `ai_recommended`) so it shows
  as a suggestion (`🤖` badge, `validate publish` INFO), never as a confirmed fact.
  User-supplied facts are entered without `--recommended`. Confirm accepted items
  with `confirm-recommendations`. No-cheat still holds: never fabricate a real-world
  place, never mark AI content confirmed/booked.
