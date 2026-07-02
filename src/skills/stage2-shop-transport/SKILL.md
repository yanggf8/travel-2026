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
topology. It records the chosen transport + lodging through the existing P3/P4
tools.

**Stage 2 has three MODES (P4, 2026-07-02) — `shop` | `ingest-known` | `defer`**
(matching `flow_decision.rs` MODES). Record the mode with
`travel flow-decision shop mode --mode <m>`:
- **`shop`** — flexible/price-sensitive: compare direct flight booking against
  flight+hotel packages and select the best booking path (the full workflow below).
- **`ingest-known`** — flights/hotel ALREADY chosen/booked: record them
  (`set-flight`/`set-hotel`) and validate; no shopping. (The common case — all 3
  completed trips were this.)
- **`defer`** — explicitly decline shopping for now; log the skip reason.

**Package/direct COMPARISON is OPTIONAL (mode `shop` only); transport/accommodation
VALIDATION is MANDATORY in every mode.** The package-vs-direct workflow below
applies to mode `shop`.

## When to use

- User says "find packages", "search OTA", "shop flights", "compare direct vs
  package", "book separately", "which offer should we take" (→ mode `shop`);
  "flights already booked" / "just record my flights" (→ mode `ingest-known`);
  "skip shopping for now" (→ mode `defer`).
- Dates and destination are locked, and the rough itinerary has enough shape to
  know whether a single-base package can work.

Do **not** use this while dates/destination are still flexible. If cheaper
flights on adjacent dates would change the trip, return to `/shaping-research`.

## Workflow

1. **Verify inputs**
   ```bash
   ./bin/travel status --full --plan-id <plan_id>
   ```
   Confirm:
   - P1 dates are confirmed.
   - P2 destination is confirmed.
   - Stage 1 has a rough day shape and lodging topology assumption.

1a. **Pick + RECORD the Stage 2 mode (F6 — do this before shopping).** Decide `shop` (flexible /
   price-sensitive → run steps 2–6 below), `ingest-known` (flights/hotel already chosen → skip to
   step 6, just record + validate), or `defer` (decline for now). Emit the routing record:
   ```bash
   ./bin/travel flow-decision shop mode --mode <shop|ingest-known|defer> [--reason <why>] --plan-id <plan_id>
   ```
   `--mode` is required and must be one of `flow_decision.rs` MODES. For `ingest-known`/`defer`, jump
   past the shopping steps — but transport/accommodation VALIDATION (step 6) is mandatory in every mode.

2. **Check package data freshness** *(mode `shop` only)*
   ```bash
   ./bin/travel check-freshness --source <source_id> --region <region>
   ```
   If data is stale or missing, use `/p3p4-packages` to capture and import. Capture
   live via the chromeport CDP driver (Python scrapers are decommissioned — see
   `/scrape-ota`), then import:
   ```bash
   ./bin/travel import-offers --dir scrapes --dest <destination_slug>
   ```

3. **Query package options**
   ```bash
   ./bin/travel query-offers --plan-id <plan_id> --dest <destination_slug>
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
   - Return to Shaping Stage if materially better prices require different dates.
   - Return to Stage 1 if the lodging topology must change before shopping can
     continue.

6. **Record the selected path**

   Package path:
   ```bash
   ./bin/travel select-offer <offer_id> <YYYY-MM-DD>
   ./bin/travel transport
   ./bin/travel query-bookings --category package
   ```

   Separate path:
   ```bash
   ./bin/travel set-flight outbound --dest <destination_slug> ...
   ./bin/travel set-flight return --dest <destination_slug> ...
   ./bin/travel set-hotel --dest <destination_slug> ...
   ./bin/travel transport
   ./bin/travel query-bookings
   ```

## Output

End with:
- A ranked comparison of viable package and direct-booking options.
- The recommended booking path and why.
- Any blocked assumptions, especially hotel topology or flight timing.
- One next action: select/book, return to Shaping Stage, return to Stage 1, or proceed
  to Stage 3 after booking is confirmed.

## Notes

- Always compare package vs direct on comparable hotel quality and location.
  A cheaper package is not viable if its hotel breaks the Stage 1 itinerary.
- Use existing `/p3-flights`, `/p3p4-packages`, and `/separate-bookings` skills
  for the lower-level work; this skill owns the Stage 2 decision.
- After transport and lodging are confirmed, move to Stage 3
  (`/stage3-expand-itinerary`) for detailed daily planning.
