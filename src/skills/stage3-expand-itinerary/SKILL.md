---
name: stage3-expand-itinerary
description: Expand the rough itinerary into a booking-aware detailed daily plan, agent-authoring AI-recommended (labeled) depth. Owns Stage 3 of the adopted research-first planning flow.
version: 1.1.0
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

5. **Enrich with AI-recommended depth (agent-first, LABELED)**

   `scaffold`/`populate` produce a structural skeleton only — no meals, no
   route-segments, thin activities. **Do not leave that depth empty for the user
   to hand-fill.** This is agent-first work (like OTA extraction): the agent
   researches and authors the depth, then persists it **labeled as AI-recommended
   (unconfirmed)** with the `--recommended` flag. Real-but-labeled is honest — it
   is a transparent suggestion to confirm, not a fabricated fact and never a
   claimed booking.

   ```bash
   # Meals — real restaurants (see the restaurant-pick rules: nearest to the site,
   # Google-rating-trusted, authentic; always a main + a backup; never phone-only).
   ./bin/travel set-meals <day> <session> --meal "<label>｜map:<real place + area>" --recommended --dest <destination_slug>
   # Transit — a whole day's real place-chain (walk→monorail→shuttle→taxi; NO public bus).
   # Keep stop names CLEAN (no （…）notes / clock times inside a stop — those go in the notes field);
   # each --seg is "from|to|mode[|duration[|start_time[|notes]]]".
   ./bin/travel set-route-segments-bulk <day> \
     --seg "<from>|<to>|<mode>|<min>||<notes>" \
     --seg "<from>|<to>|<mode>|<min>||<notes>" --recommended --dest <destination_slug>
   # (or a single leg: set-route-segment <day> <sort_order> <from> <to> <mode> [--duration N] [--notes ".."] --recommended)
   # Extra want/nice-to-have activities the skeleton is too thin for.
   ./bin/travel add-activity <day> <session> "<title>" --recommended --dest <destination_slug>
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
     the ones they accept with `confirm-recommendations` (step 7).

6. **Validate and rebalance**
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

7. **Surface AI-recommended items for confirmation**

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

8. **Confirm readiness for Stage 4**

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
