---
name: stage0-research
description: Pre-lock "triangle research" — explore departure date, destination, and flight price together before any plan is committed. Owns Stage 0 of the research-first planning flow.
version: 1.0.0
requires_skills: [travel-shared, scrape-ota]
requires_processes: []
provides_processes: []
---

# /stage0-research

Orchestration skill for **Stage 0 — Triangle Research** of the adopted
research-first planning flow (`docs/plans/2026-05-22-new-planning-flow.md`).

It explores the three interdependent variables — departure date, destination,
flight price — *together*, before any of them is locked. It does **not**
replace `/p3-flights`: that skill requires P1/P2 to already exist, so it cannot
run pre-lock. `/stage0-research` owns the pre-lock phase and can seed the
initial P1/P2 rows once the user picks a candidate.

## When to use

- User describes a trip in loose terms — a date window and 1–3 candidate
  destinations — and has not committed to dates or a destination.
- Triggers: "find me the cheapest week to go to Japan in June", "should I do
  Osaka or Tokyo, depends on flight price", "what dates are cheapest".

Do **not** use this once dates and destination are already locked — go
straight to `/p3-flights` or `/p3p4-packages`.

## Data model

Stage 0 data lives in six unscoped Turso tables (keyed by `run_id`, not
`plan_id`): `stage0_research_runs`, `stage0_research_destinations`,
`stage0_research_durations`, `stage0_candidates`, `stage0_candidate_flights`,
`stage0_scrape_attempts`. See
`docs/superpowers/specs/2026-05-22-stage0-triangle-research-design.md`.

**Runs are immutable.** A `run_id` fixes origin, window, pax, exchange rate,
destinations, and durations. Changing any of those = a new run.

## Workflow

1. **Gather inputs** from the user (ask, do not guess):
   - Origin airport (default TPE)
   - Travel window — earliest and latest acceptable departure date
   - 1–3 candidate destinations (airport code + label)
   - Trip lengths to consider, in nights (e.g., 6 and 7)
   - Passenger count (default 2)

2. **Create the run:**
   ```bash
   npm run travel -- stage0-init --origin TPE \
     --start 2026-06-18 --end 2026-06-20 \
     --dest KIX:"Osaka/Kyoto (KIX)" --dest NRT:"Tokyo (NRT)" \
     --nights 6 --nights 7 --pax 2 --rate 32
   ```
   Note the `run_id` it prints.

3. **Run the aggregator** (scrapes destination × duration, imports + ranks):
   ```bash
   python scripts/stage0_research.py --run <run_id>
   ```

4. **Show the ranking:**
   ```bash
   npm run travel -- stage0-compare --run <run_id>
   ```
   Present the ranked table. Candidates sort by flight price; leave-days is a
   shown column and a tie-breaker only.

5. **Iterate.** If the user wants different destinations, a shifted window, or
   other durations, that is a **new run** (runs are immutable) — go back to
   step 2 with the new inputs. The previous run's candidates stay intact and
   comparable.

6. **Hand off on lock.** When the user picks a candidate:
   ```bash
   npm run travel -- stage0-adopt <candidate_id> <new_plan_id> \
     --create-plan --dest <destination_slug>
   ```
   This creates the minimal normalized plan rows, sets P1 dates from the
   candidate's depart/return dates, sets P2 destination from `--dest`, and
   links `adopted_plan_id` on the Stage 0 candidate. Use an existing
   `destination_config` slug such as `osaka_kyoto_2026`.

   If the plan already exists, use the legacy link-only form:
   ```bash
   npm run travel -- stage0-adopt <candidate_id> <existing_plan_id>
   ```

   After a new-plan handoff, continue with Stage 1:
   ```bash
   npm run travel -- scaffold-itinerary --plan-id <new_plan_id> --dest <destination_slug>
   ```

## Notes

- If a (destination, duration) scrape fails, the aggregator records it in
  `stage0_scrape_attempts` and continues. Re-running the aggregator on the
  same run retries only failed/pending attempts.
- The adopted planning flow is Stage 0 through Stage 4. Existing `/p1-*`
  through `/p5-*` skills remain implementation tools after Stage 0 locks a
  candidate.
