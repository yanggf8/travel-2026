# Japan Travel Planning Flow — New Design

> **Date:** 2026-05-22
> **Status:** Adopted. Shaping Stage is implemented, `shaping-adopt --create-plan --dest` closes the pre-lock to plan handoff, and CLAUDE.md's Skill Decision Tree routes new planning work through this staged flow.
> **Purpose:** Replace P1→P2→P3→P4→P5 linear model with a research-first, iterative approach where dates/destinations/flights evolve together.

---

## Core Philosophy

The three core variables — **departure date**, **destination**, and **flight price** — are deeply interdependent. Cheap flights on certain dates can change the preferred destination; a destination shift opens different date windows. We should not commit to any one variable until candidates for all three are on the table.

The process is **research-first, booking-last**.

---

## Shaping Stage — Triangle Research (Research Phase)

**Goal:** Find the best combination of departure date range, duration, and destination candidate by checking flight prices across multiple dimensions.

**Input:**
- Origin: TPE (Taipei)
- General travel window (e.g., "June 18-25", "around Golden Week", "late June")
- Possible destinations (can be 1-3 candidates: Tokyo, Osaka, Kyoto, etc.)
- Duration range (e.g., 4-6 nights)

**Output:**
- A list of candidate options, each with:
  - Destination + airport (e.g., KIX for Osaka/Kyoto, NRT/HND for Tokyo)
  - Departure date + day-of-week
  - Return date + day-of-week
  - Flight price estimate (per person)
  - "Good fit" verdict: why this option is strong

**How it works — Dynamic Research Loop:**

```
START: user gives departure date range + possible destinations

For each destination candidate:
  For each date window in range (±3 days around target):
    Scrape flight prices for that date + 5-night / 6-night / 7-night durations
    Normalize and rank results

Present: sorted candidate list (best value first)

User picks a candidate OR asks to explore another date/destination
→ Loop back: adjust dates or destination, re-scrape
→ Stop when user says "this date + destination works"
```

**Key principle:** Dates and destination are not locked until flight candidates look good. The user can say "try June 20 instead" or "instead of Osaka try Kyoto" at any point.

**Default scope:** Explore every departure date in the user's stated window for each supplied destination and duration. Keep the first run to 1-3 destinations and 1-3 durations unless the user explicitly asks for a wider sweep; if the cartesian product becomes large, split it into multiple immutable runs so rankings remain explainable.

**Unit convention:** This doc speaks in **nights** (a 6-night trip). The scraper's `--duration` flag wants **trip days** = nights + 1 (depart and return days both counted). So a 6-night trip is `--duration 7`; a 5-night trip is `--duration 6`. Filenames below use the nights count (`6n`) to match what `view:prices --nights` expects.

**Tools:**
```bash
# Create an immutable run over destination × duration.
./bin/travel shaping-init --origin TPE \
  --start 2026-06-18 --end 2026-06-20 \
  --dest KIX:"Osaka/Kyoto (KIX)" --dest NRT:"Tokyo (NRT)" \
  --nights 6 --nights 7 --pax 2 --rate 32

# Capture offers via gwebcdb (WSLg), agent-extract, then import into the shaping run:
#   cd ~/b/gwebcdb && ./scripts/start-chrome-cdp-wslg.sh            # CDP on :9222
#   → python bridge/navigate.py "<url>"                            # + form_fill/combo_select for SPAs; settle ~25s
#   → python bridge/ota_capture.py --source <id>                   # → capture_id; UNREDACTED → captures
#   → AGENT reads captures.raw_text, extracts offers, emits TSV
#   → ./bin/travel ota write-offers <job_id> --capture <capture_id> --claim-token <tok> --tsv <path>
#   → ./bin/travel shaping-import --run <run_id> --file <handoff.json>

# Compare top candidates.
./bin/travel shaping-compare --run <run_id>
```

**Aggregation is DB-backed:** Shaping Stage stores immutable runs in unscoped `shaping_*` tables and ranks all imported candidates across destinations and durations.

**Exit condition:** User says "let's lock this date and destination." Adopt the candidate into a new plan and move to Stage 1:

```bash
./bin/travel shaping-adopt <candidate_id> <new_plan_id> --create-plan --dest <destination_slug>
```

This seeds the minimal normalized plan rows, sets P1 dates from the candidate's depart/return dates, sets P2 destination from `--dest`, and links `shaping_candidates.adopted_plan_id`.

---

## Stage 1 — Itinerary Draft

**Goal:** With a proposed date + destination, build a rough itinerary to validate the choice. If timing or pacing feels wrong, go back to Shaping Stage to explore alternatives.

**Input:**
- Locked: departure date, return date, destination
- Unlocked: hotel, flight carrier (price matters more than carrier preference)

**What to draft:**
```bash
# If coming from Shaping Stage with `shaping-adopt --create-plan`, the plan,
# destination, and P1/P2 rows already exist.
./bin/travel plans
./bin/travel status --full --plan-id <plan-id>

# Orchestrate the rough draft with /stage1-itinerary-draft, which runs the
# scaffold command below and validates whether the plan should move to Stage 2.
./bin/travel scaffold-itinerary --plan-id <plan-id> --dest <destination-slug>
# Example dest slug: for Kansai use the slug from destination_config table
# (e.g., osaka_kyoto, kansai_2026, etc. — check with ./bin/travel status --full)
```

If Stage 1 starts without a Shaping Stage handoff, first ensure the plan and destination
exist through the normal `/new-destination`, `/p1-dates`, and `/p2-destination`
workflow.

Fill in:
- Day-by-day areas/clusters (e.g., Day 2: Dotonbori, Day 3: Arashiyama)
- Must-do activities (e.g., teamLab Borderless, Fushimi Inari)
- Rough time blocks (morning/afternoon/evening)

**Drafting default:** The agent drafts the first itinerary from known destination patterns, current plan constraints, and any must-do items the user already gave. Do not block Stage 1 waiting for a must-do list; ask targeted follow-up questions only when a missing preference changes routing, pacing, or booking strategy.

**Multi-city lodging topology:**
- **Split-stay**: Different hotel in each city (e.g., 3 nights Osaka + 2 nights Kyoto). Better for multi-city trips but more hotel logistics.
- **Day-trip**: Base in one city, travel to the other city by train. Simpler logistics, more travel time each day.
- **Single city**: One base, no inter-city travel needed. Simplest option.

Decide this before Stage 2 because it affects which packages are viable. Some packages only cover one city or one hotel base.

**Decision point:**
- If the itinerary is too packed or too loose, revise the draft or return to Shaping Stage for a different duration.
- If proposed flight times create arrival/departure-day conflicts, return to Shaping Stage or Stage 2 with narrower flight criteria.
- If a must-see requires a different day/date, adjust the draft or return to Shaping Stage.
- If the lodging topology does not fit the package/direct-booking strategy, revise the topology before Stage 2.

**Exit condition:** Itinerary draft, duration, flight timing assumptions, and lodging topology all look viable → move to Stage 2.

---

## Stage 2 — Shop / Record Transportation (has MODES)

**Goal:** Land the transport + accommodation for the locked date + destination.

**Stage 2 has three MODES (P4, 2026-07-02 — evidence: all 3 completed trips had pre-decided flights,
so a mandatory "compare direct vs package" stage did not fit reality):**
- **`shop`** — flexible / price-sensitive: compare direct flights vs packages (`/p3-flights`,
  `/p3p4-packages`, `/separate-bookings`). The full Path A/B flow below.
- **`ingest-known`** — flights/hotel ALREADY chosen/booked: record them (`set-flight`/`set-hotel`) and
  VALIDATE; no shopping. This is the common case — the known-flights fast-path, which is the DEFAULT
  planning path (Shaping is the optional side-tool for flexible/price-sensitive trips).
- **`defer`** — explicitly decline shopping for now; log the skip reason.

**Package/direct COMPARISON is OPTIONAL (only mode `shop`); transport/accommodation VALIDATION is
MANDATORY in every mode.** Record the chosen mode with `travel flow-decision shop mode --mode <m>`
(m ∈ `shop|ingest-known|defer`, matching `flow_decision.rs` MODES).

Use `/stage2-shop-transport` as the orchestration skill; it wraps `/p3-flights`, `/p3p4-packages`, and
`/separate-bookings`. The Path A/B flow below applies to mode `shop`.

> **Non-goals (hedge, 2026-07-02):** these are docs/routing changes only. There is **no trip-classification
> router** (the only routing concept is: default known-flights fast-path + Stage-2 purchase modes), **no lifecycle state machine**
> (extend the existing PAST/ACTIVE/UPCOMING logic when pre/in/post-trip is modeled), and **no
> Shaping-breadth algorithm** — deferred until at least one real flexible-flights trip exercises them.
> Evidence so far is n=3, all pre-decided-flight. See `docs/plans/2026-07-02-planning-flow-improvement.md`.

**Two paths:**

### Path A: Direct Flight Purchase
```bash
# Direct-flight prices come from the same gwebcdb capture path as packages (agent-first extraction).
# Capture a flight source (e.g. google_flights) for the locked dates, agent-extract → TSV → write-offers:
#   cd ~/b/gwebcdb && ./scripts/start-chrome-cdp-wslg.sh
#   → python bridge/navigate.py "<google-flights-url>"  → python bridge/ota_capture.py --source google_flights
#   → AGENT reads captures.raw_text, emits TSV → ./bin/travel ota write-offers <job_id> --capture <capture_id> --claim-token <tok> --tsv <path>

# Compare with package prices (Path B must run first to populate DB)
./bin/travel query-offers --region kansai --start 2026-06-18 --end 2026-06-25 --max-price 30000 --json
```

**Note:** Shaping Stage only researches flights. To compare direct vs package, run Path B below first to scrape and import package data, then return to Path A results.

### Path B: Package (Flight + Hotel)
```bash
# IMPORTANT: Run this BEFORE comparing direct vs package — Shaping Stage only has flight data
# Search packages for the locked dates via gwebcdb (WSLg); extraction is agent-first (no in-CLI parser):
#   cd ~/b/gwebcdb && ./scripts/start-chrome-cdp-wslg.sh
#   → python bridge/navigate.py "<ota-url>"  (+ form_fill/combo_select/form_click for SPA searches; settle ~25s)
#   → python bridge/ota_capture.py --source <id>            # → capture_id; UNREDACTED → captures
#   → AGENT reads captures.raw_text, extracts offers, emits TSV
#   → ./bin/travel ota write-offers <job_id> --capture <capture_id> --claim-token <tok> --tsv <path>
# See src/skills/scrape-ota/SKILL.md and CLAUDE.md → URL Routing

# Import and query
./bin/travel import-offers --dir scrapes --dest <slug>
./bin/travel query-offers --plan-id <id> --dest <slug> --max-price 30000 --json
```

**Decision:** Direct flight only OR package?

- If package has good hotel + good flight → select package (P3+P4 done together)
- If direct flight is cheaper and user prefers choice of hotel → book flight separately → move to Stage 3
- If package total is within 10% of direct flight plus a comparable hotel, prefer the package only when the hotel location, room type, cancellation terms, and lodging topology are acceptable. Otherwise prefer separate booking.

**If flight is cheaper on slightly different dates →** Go back to Shaping Stage with new dates, re-run triangle research.

---

## Stage 3 — Expand Itinerary (Detailed Planning)

**Trigger:** Transport + lodging are selected/recorded — booked, or provisionally
chosen (a detailed pass before final ticketing is allowed).

**Goal:** Fill in the full itinerary — attractions, transit between areas, meal arrangements (no breakfast), timing per session.

Use `/stage3-expand-itinerary` as the orchestration skill for this stage — **it is
the authoritative step list** (this section is the high-level overview). It wraps
`/p5-itinerary` and the itinerary CLI commands, and owns detailed validation
against real transport and lodging, the agent-first **AI-recommended enrichment**
step (see Fill order below), and the confirm loop.

**What to do:**
```bash
# Set the confirmed flight details (both legs)
./bin/travel set-flight outbound --dest <slug> \
  --flight SL396 --airline "Thai Lion Air" --airline-code SL \
  --from TPE --dep 09:00 --to KIX --arr 12:30 --date 2026-06-18
./bin/travel set-flight return --dest <slug> \
  --flight SL397 --airline "Thai Lion Air" --airline-code SL \
  --from KIX --dep 13:30 --to TPE --arr 15:40 --date 2026-06-25

# Validate the itinerary fits with actual flight times (--severity error|warning|info)
./bin/travel validate-itinerary --dest <slug> --severity warning

# For each day, set activities with timing (--fixed REQUIRES a value: true|false)
./bin/travel set-activity-time <day> <session> "<activity>" --start HH:MM --end HH:MM --fixed true

# Set day themes
./bin/travel set-day-theme <day> "Dotonbori Food Walk" --zh "道頓堀美食"

# Set session focus and transit
./bin/travel set-tod-zh <day> <session> \
  --zh "午餐：美國村特色料理" \
  --transit-zh "地下鐵御堂筋線 難波→心齋橋 5分鐘"
```

**Fill order:**
1. Lock must-do activities (fixed time, priority = must)
2. Map out transit between areas
3. Fill in meals (lunch + dinner; no breakfast by default unless the selected hotel/package includes it)
4. Add nice-to-have activities in remaining slots
5. **Enrich, agent-first + LABELED.** The agent authors the real depth (meals via
   `set-meals`, transit via `set-route-segments-bulk`, extra activities via
   `add-activity`) with the `--recommended` flag → `source='ai_recommended'`. Real
   data only; the dashboard badges it 🤖, `validate publish` counts it as INFO
   (never blocks). **Set the session ZH (`set-tod-zh`) in the same pass** as the
   first content you add to a previously-empty session — else the Stage-4 publish
   gate BLOCKs on missing `focus_zh`.
6. **Review + confirm.** `query-recommendations` lists the AI-recommended items
   (same filters as confirm) — present them to the user; then
   `confirm-recommendations` flips the approved scope `ai_recommended`→`confirmed`.
   Un-confirmed items stay labeled (a valid pre-trip state).
7. Check pacing (relaxed/balanced/packed) and run `validate publish` for readiness.

---

## Stage 4 — Publish to Dashboard

**Goal:** Make the plan visible on the web dashboard for reference and sharing.

Use `/stage4-publish-dashboard` as the orchestration skill for this stage. It
wraps `/deploy-dashboard` and `/weather-update`, and owns publish readiness,
explicit deployment, and post-deploy verification.

**What to populate:**
```bash
# ZH content for all sessions (bilingual display)
# Bulk populate pattern from scripts/set-kyoto-zh-sessions-v2.ts

# Set all day themes in ZH
./bin/travel set-day-theme 1 arrival --zh "大阪抵達" --dest <slug>
./bin/travel set-day-theme 2 full --zh "道頓堀+美國村" --dest <slug>
...

# Transit summary for the whole trip
# (stored in itinerary_metadata.transit_summary_zh)
```

**Deploy:**
```bash
cd workers/trip-dashboard
unset CLOUDFLARE_API_TOKEN && npx wrangler deploy
```

**This stage is not sequential — run it any time user wants to share or reference the plan.**

**Deploy default:** Deploy only on explicit user request or when a task specifically asks for publishing. Itinerary/database changes do not automatically deploy the dashboard.

---

## Stage 5 — Iteration Loop (Ongoing)

After any stage, user may want to iterate:

| Trigger | Action | Go to |
|---------|--------|-------|
| "Flight price changed — is June 19 better?" | Re-scrape flights | Shaping Stage |
| "I want to add Kyoto as a second destination" | Update dates/destination | Shaping Stage → Stage 1 |
| "Hotel in package looks bad" | Unselect offer, shop separately | Stage 2 |
| "Day 3 is too packed" | Re-balance itinerary | Stage 3 |
| "Add a must-see in Kyoto" | Insert activity, check transit | Stage 3 |
| "New attractions info online" | Update via CLI, redeploy | Stage 4 |

---

## Process Summary

```
┌─────────────────────────────────────────────────────────┐
│  OPTIONAL STAGE 0: Triangle Research                    │
│  Departure dates + destination + flight price           │
│  Research loop until candidate locked                   │
└────────────────┬────────────────────────────────────────┘
                 │ user locks date + destination
                 ▼
┌─────────────────────────────────────────────────────────┐
│  STAGE 1: Itinerary Draft                               │
│  Rough day-by-day plan → validate timing fit            │
│  If bad timing → back to Shaping Stage                        │
└────────────────┬────────────────────────────────────────┘
                 │ itinerary fits
                 ▼
┌─────────────────────────────────────────────────────────┐
│  STAGE 2: Shop / Record Transport                       │
│  ingest-known OR shop OR defer                          │
│  If different dates cheaper → back to Shaping Stage          │
└────────────────┬────────────────────────────────────────┘
                 │ flight confirmed
                 ▼
┌─────────────────────────────────────────────────────────┐
│  STAGE 3: Expand Itinerary                              │
│  Fill activities, transit, meals (no breakfast)        │
│  Validate with real flight times                        │
└────────────────┬────────────────────────────────────────┘
                 │ itinerary detailed
                 ▼
┌─────────────────────────────────────────────────────────┐
│  STAGE 4: Publish to Dashboard                          │
│  ZH content, deploy to CF Workers                      │
│  (Can run anytime — not strictly sequential)           │
└─────────────────────────────────────────────────────────┘
```

---

## Skill Mapping — New Stages vs Existing P1–P5 Skills

The repo ships `/p1-dates`, `/p2-destination`, `/p3-flights`, `/p3p4-packages`, and `/p5-itinerary`. This flow does not delete them — it re-sequences them. Until the skills are renamed, use this mapping:

| Stage | Existing skill(s) reused | What changes |
|-------|--------------------------|--------------|
| Optional Shaping Stage — Triangle Research | `/shaping-research` (orchestration skill) + gwebcdb capture + agent TSV extraction + `ota write-offers` | `/shaping-research` owns optional pre-lock research — it has `requires_processes: []`, so it runs before dates/destination exist. `/p3-flights` still cannot be reused here (it requires P1/P2). |
| Stage 1 — Itinerary Draft | `/stage1-itinerary-draft`, `shaping-adopt --create-plan`, `/p1-dates`, `/p2-destination`, `scaffold-itinerary` | Shaping Stage handoff can seed the provisional P1/P2 lock **but not transport/accommodation** — an adopted plan lands "dates+dest done, flights empty" and still needs Stage 2 (`ingest-known`/`shop`); `/stage1-itinerary-draft` owns the rough itinerary and viability check; `/p1-dates` and `/p2-destination` still handle manual or later revisions. |
| Stage 2 — Shop Flight | `/stage2-shop-transport`, `/p3-flights`, `/p3p4-packages`, `/separate-bookings` | `/stage2-shop-transport` owns the package-vs-direct decision; lower-level P3/P4 skills remain the implementation tools. |
| Stage 3 — Expand Itinerary | `/stage3-expand-itinerary`, `/p5-itinerary` | `/stage3-expand-itinerary` owns booking-aware detail expansion and validation; `/p5-itinerary` remains the implementation tool. |
| Stage 4 — Publish | `/stage4-publish-dashboard`, `/deploy-dashboard`, `/weather-update` | `/stage4-publish-dashboard` owns explicit publish readiness and verification; `/deploy-dashboard` remains the implementation tool. |

**Naming decision:** Keep the existing `/p1-*` through `/p5-*` skill names as compatibility labels. The adopted mental model is Shaping Stage through Stage 4; the P-numbered skills are implementation tools reused inside those stages.

---

## Comparison: Old vs New

| Area | Original Flow | Adopted Flow |
|------|---------------|-------------------|
| Overall model | Linear P1 → P2 → P3 → P4 → P5 | Known-flights fast-path by default; optional research-first loop when flights/dates are loose |
| First decision | Lock dates first | Record known dates/flights when chosen; otherwise explore dates, destination, and flight price together |
| Destination choice | Chosen after dates are set | Compared alongside flight/date options |
| Flight prices | Checked after date/destination decisions | Used early as a primary decision signal |
| Date flexibility | Low; going back is possible but awkward | Expected; dates can change during research |
| Shaping Stage equivalent | No dedicated triangle research stage | Optional Shaping Stage researches date + destination + flight price |
| Itinerary timing | Built late after transport/accommodation | Rough itinerary drafted before booking |
| Package vs separate booking | P3 transport then P4 accommodation are separate sequential steps | Stage 2 chooses direct flight vs flight+hotel package |
| Multi-city lodging | Usually handled later during itinerary planning | Decided before package/direct-booking choice |
| Dashboard | Published near the end | Can be published any time useful |
| User iteration | More like restarting or backtracking | Explicit iteration loop after every stage |
| Main benefit | Simple, predictable process | Better optimization across price, dates, and destination |
| Main tradeoff | Can lock bad assumptions too early | Requires more upfront comparison and manual aggregation |

---

## Resolved Decisions

1. **Shaping Stage scrape scope:** use every departure date in the user's stated window across supplied destinations and durations; split large searches into multiple immutable runs rather than hiding a huge sweep in one run.
2. **Stage 1 drafting:** agent drafts first, using known destination patterns and any user-provided must-do items; ask only preference questions that materially change routing or booking.
3. **Stage 2 package preference:** package wins within a 10% total-price band only if hotel quality/location, flight times, room terms, and lodging topology are acceptable; otherwise separate booking wins.
4. **Stage 3 meals:** no breakfast by default unless the booked hotel/package includes it or the user asks to add it.
5. **Stage 4 deploy:** explicit deploy only; never auto-deploy from itinerary changes.
