---
name: shaping-research
description: The Shaping Stage — pre-lock "triangle research" that explores departure date, destination, and flight/FIT price together AND records the trip's shaping (hard constraints + soft preferences) before any plan is committed. First stage of the research-first planning flow.
version: 2.0.0
requires_skills: [travel-shared, scrape-ota]
requires_processes: []
provides_processes: []
---

# /shaping-research — the Shaping Stage

Orchestration skill for the **Shaping Stage** (formerly "Stage 0") of the
research-first planning flow (`docs/plans/2026-05-22-new-planning-flow.md`).

The Shaping Stage does two inseparable jobs **before** dates/destination lock:

1. **Shape** — capture the trip's *constraints and preferences* (the "shaping"):
   hard date caps, excluded options, lodging/channel preferences, observed
   signals. This is the decision base everything downstream is judged against.
2. **Research** — explore the interdependent triangle (departure date ×
   destination × flight/FIT price) *against that shaping*.

It does **not** replace `/p3-flights` (which needs P1/P2 to already exist and so
can't run pre-lock). The Shaping Stage owns the pre-lock phase and seeds the
initial P1/P2 rows once the user picks a candidate.

## When to use

- User describes a trip in loose terms — a date window and 1–3 candidate
  destinations — and has not committed to dates or a destination.
- User states or changes a **constraint** ("must be back before X", "hotel must
  be near Y", "exclude source Z", "this booking failed, find another").
- Triggers: "find me the cheapest week to go to Japan in June", "Osaka or Tokyo
  depending on price", "what dates are cheapest", "look for FIT for <trip>".

Do **not** use once dates and destination are already locked — go straight to
`/p3-flights` or `/p3p4-packages`.

## Data model

Shaping Stage data lives in unscoped Turso tables (keyed by `run_id`, not
`plan_id`): `shaping_research_runs`, `shaping_research_destinations`,
`shaping_research_durations`, `shaping_rules` (the shaping constraints/prefs),
`shaping_candidates`, `shaping_candidate_flights`, `shaping_scrape_attempts`,
`shaping_tour_group_offers`, `shaping_tour_group_scrape_attempts`. Research
artifacts and the chosen offer live in `shaping_research_artifacts` /
`shaping_selected_offers`. See
`docs/superpowers/specs/2026-05-22-stage0-shaping.md`.

**Runs are immutable.** A `run_id` fixes origin, window, pax, exchange rate,
destinations, durations, **and shaping**. Changing any of those = a new run.
Because they're immutable, getting the shaping right *before* you create the run
matters — a wrong base produces an orphan run and wasted research.

## Workflow

### Step 1 — LOAD prior shaping from the DB (ALWAYS, before anything else)

Never start a Shaping Stage from a blank slate. A prior run for this trip may
already hold hard constraints, preferences, and observed signals you must build
on (or that have since changed). Query first:

```bash
# List existing runs, then inspect the most relevant one's shaping
./bin/travel db exec "SELECT run_id, origin_code, window_start, window_end FROM shaping_research_runs ORDER BY run_id DESC;"
./bin/travel shaping-compare --run <prior_run_id>
```

Reconcile old vs. new out loud: which rules **carry forward**, which are
**superseded** by what the user just said. Record the supersession as an
`observed_signal` so the lineage is explicit.

### Step 2 — RECORD the shaping (REQUIRED — this is not optional)

Shaping is the *point* of this stage. Capture every hard constraint and soft
preference as `--shaping ASPECT:ROLE:KIND:VALUE[:NOTES]` (repeatable):

- **ROLE**: `hard_constraint` | `soft_preference` | `search_directive` |
  `observed_signal` | `hypothesis`
- **ASPECT**: `date` | `channel` | `mobility` | `lodging` | `budget` |
  `activity` | `general`

If the user gave no explicit constraints, ask for at least the binding ones
(date caps, budget, must-haves) — do not create a run with zero shaping.

### Step 3 — Create the run (carrying the shaping)

```bash
./bin/travel shaping-init --origin TPE \
  --start 2026-06-18 --end 2026-06-20 \
  --dest KIX:"Osaka (KIX)" --dest NRT:"Tokyo (NRT)" \
  --nights 6 --nights 7 --pax 2 --rate 32 \
  --shaping date:hard_constraint:return_no_later_than:2026-06-24:Must be back before 6/25 \
  --shaping lodging:soft_preference:preferred_hotel_area:中央那霸:near Yui-rail \
  --shaping general:observed_signal:supersedes_run:<prior_run_id>:reconciliation note
```
Note the `run_id` it prints. `shaping-init` warns if you pass **no** `--shaping`
— that warning means you skipped Step 2; go back and record the constraints.

**Then record the routing choice on the plan (F6 instrumentation).** Entering the Shaping Stage is the
flexible-research path (the optional Shaping side-tool, vs the default known-flights fast-path); emit it so the plan history shows WHY
this trip went through Shaping rather than the known-flights fast-path:
```bash
./bin/travel flow-decision shaping enter --reason flexible --plan-id <plan-id>
```
(Only once a plan exists — for a pre-plan run this is recorded at adopt time / when the plan is created.
`flow-decision` vocab is fixed by `flow_decision.rs`: stage `shaping`, decision `enter`.)

### Step 4 — Run the aggregator (flights) and/or gather FIT offers

```bash
# Flight candidates: capture via gwebcdb on WSLg (e.g. --source google_flights), agent-extract TSV,
# write offers, then import into the run:
#   cd ~/b/gwebcdb && ./scripts/start-chrome-cdp-wslg.sh && python bridge/navigate.py "<flights-url>"
#   → python bridge/ota_capture.py --source google_flights   # → capture_id
#   → AGENT reads captures.raw_text, emits TSV → ./bin/travel ota write-offers <job_id> --capture <capture_id> --claim-token <tok> --tsv <path>
#   → ./bin/travel shaping-import --run <run_id> --file <handoff.json>
# FIT/tour-group offers: capture via gwebcdb → import-tour-group-offers --run <run_id> --file ...
```

### Step 5 — Show the ranking against the shaping

```bash
./bin/travel shaping-compare --run <run_id>             # candidates + shaping recap
./bin/travel shaping-baseline --run <run_id>            # FIT-vs-group methodology view
```
Present the ranked table; judge every option against the HARD constraints.


**If the run has package / FIT offers (gathered in Step 4), score them before locking.**
`shaping-purchase-matrix` is read-only: it scores every option (the flight candidate + each package)
against THIS run's shaping rules — hard constraints are GATES (a violation ⇒ DISQUALIFIED), soft
preferences are NUDGES (score adjustments). Skip this if the run is flight-only (no offers to score).
```bash
./bin/travel shaping-purchase-matrix --run <run_id>                    # full matrix (disqualified shown last)
./bin/travel shaping-purchase-matrix --run <run_id> --qualified-only   # hide disqualified options
```
Use it to pick the candidate/offer to hand off in Step 7 — it makes the "which option best fits the
rules" call explicit instead of eyeballing the ranking.

### Step 6 — Iterate (new run on any input change)

Different destinations, shifted window, other durations, or **changed shaping**
= a **new run** (runs are immutable). Re-do Steps 1–3 with the new inputs; prior
candidates stay intact and comparable.

### Step 7 — Hand off on lock

```bash
./bin/travel shaping-adopt <candidate_id> <new_plan_id> \
  --create-plan --dest <destination_slug>
```
Creates minimal normalized plan rows, sets P1 dates from the candidate's
depart/return, sets P2 destination from `--dest`, links `adopted_plan_id`.
If the plan already exists, use the link-only form:
`./bin/travel shaping-adopt <candidate_id> <existing_plan_id>`.

After a new-plan handoff, continue with `/stage1-itinerary-draft`:
```bash
./bin/travel scaffold-itinerary --plan-id <new_plan_id> --dest <destination_slug>
```

## Notes

- **The #1 failure mode is skipping Steps 1–2** (jumping straight to research or
  to `shaping-init` with no `--shaping`). If you catch yourself reaching for the
  aggregator before the shaping is loaded and recorded, stop and do Steps 1–2.
- If a (destination, duration) scrape fails, the aggregator records it in
  `shaping_scrape_attempts` and continues. Re-running retries only failed ones.
- The adopted planning flow is Shaping Stage → Stage 4. Existing `/p1-*` through
  `/p5-*` skills remain implementation tools after a candidate locks.
