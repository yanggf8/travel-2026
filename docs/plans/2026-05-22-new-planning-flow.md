# Japan Travel Planning Flow — New Design

> **Date:** 2026-05-22
> **Purpose:** Replace P1→P2→P3→P4→P5 linear model with a research-first, iterative approach where dates/destinations/flights evolve together.

---

## Core Philosophy

The three core variables — **departure date**, **destination**, and **flight price** — are deeply interdependent. Cheap flights on certain dates can change the preferred destination; a destination shift opens different date windows. We should not commit to any one variable until candidates for all three are on the table.

The process is **research-first, booking-last**.

---

## Stage 0 — Triangle Research (Research Phase)

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

**Tools:**
```bash
# NOTE: --dest accepts ONE airport code; --duration is a single integer (trip days, not nights)
# Run separately per destination and duration. Results compared manually or via view:prices.

# Kansai (Osaka/Kyoto): 6-night trip = depart June 18, return June 24
python scripts/scrape_date_range.py \
  --depart-start 2026-06-18 --depart-end 2026-06-20 \
  --origin tpe --dest kix \
  --duration 7 --pax 2 \
  -o scrapes/june-kix-6n.json

# Tokyo (Narita): same dates for comparison
python scripts/scrape_date_range.py \
  --depart-start 2026-06-18 --depart-end 2026-06-20 \
  --origin tpe --dest nrt \
  --duration 7 --pax 2 \
  -o scrapes/june-nrt-6n.json

# Also try 7-night (depart June 18, return June 25) for the same destinations
python scripts/scrape_date_range.py \
  --depart-start 2026-06-18 --depart-end 2026-06-20 \
  --origin tpe --dest kix \
  --duration 8 --pax 2 \
  -o scrapes/june-kix-7n.json

# Compare top results side-by-side
npm run view:prices -- --flights scrapes/june-kix-6n.json --nights 6
npm run view:prices -- --flights scrapes/june-nrt-6n.json --nights 6
npm run view:prices -- --flights scrapes/june-kix-7n.json --nights 7
```

**Duration note:** `--duration` is trip days (depart → return, inclusive). A 6-night trip is `--duration 7` (e.g., June 18–24 = 7 days). A 5-night trip is `--duration 6` (June 18–23 = 6 days).

**Manual aggregation required:** The script handles one destination + one duration per run. For the research loop, run it multiple times and compare results manually using `npm run view:prices -- --flights <file> --nights N`. An aggregator wrapper script could automate this in the future.

**Exit condition:** User says "let's lock this date and destination" → move to Stage 1.

---

## Stage 1 — Itinerary Draft

**Goal:** With a proposed date + destination, build a rough itinerary to validate the choice. If timing or pacing feels wrong, go back to Stage 0 to explore alternatives.

**Input:**
- Locked: departure date, return date, destination
- Unlocked: hotel, flight carrier (price matters more than carrier preference)

**What to draft:**
```bash
# First confirm the plan and destination already exist.
# set-dates mutates an existing plan; it does not create a missing plan_id.
npm run travel -- plans
npm run view:status -- --plan-id <plan-id>

# If the plan_id or destination slug is missing, run the /new-destination workflow
# and seed/migrate the DB before using the normal planning commands.

# Set dates on the existing plan
npm run travel -- set-dates 2026-06-18 2026-06-25 --plan-id <plan-id>

# Scaffold the itinerary
npm run travel -- scaffold-itinerary --plan-id <plan-id> --dest <destination-slug>
# Example dest slug: for Kansai use the slug from destination_config table
# (e.g., osaka_kyoto, kansai_2026, etc. — check with npm run view:status)
```

Fill in:
- Day-by-day areas/clusters (e.g., Day 2: Dotonbori, Day 3: Arashiyama)
- Must-do activities (e.g., teamLab Borderless, Fushimi Inari)
- Rough time blocks (morning/afternoon/evening)

**Multi-city lodging topology:**
- **Split-stay**: Different hotel in each city (e.g., 3 nights Osaka + 2 nights Kyoto). Better for multi-city trips but more hotel logistics.
- **Day-trip**: Base in one city, travel to the other city by train. Simpler logistics, more travel time each day.
- **Single city**: One base, no inter-city travel needed. Simplest option.

Decide this before Stage 2 because it affects which packages are viable. Some packages only cover one city or one hotel base.

**Decision point:**
- If the itinerary is too packed or too loose, revise the draft or return to Stage 0 for a different duration.
- If proposed flight times create arrival/departure-day conflicts, return to Stage 0 or Stage 2 with narrower flight criteria.
- If a must-see requires a different day/date, adjust the draft or return to Stage 0.
- If the lodging topology does not fit the package/direct-booking strategy, revise the topology before Stage 2.

**Exit condition:** Itinerary draft, duration, flight timing assumptions, and lodging topology all look viable → move to Stage 2.

---

## Stage 2 — Shop Flight (Book Transportation)

**Goal:** Find the best flight option for the locked date + destination. Can be bought directly or as part of a package.

**Two paths:**

### Path A: Direct Flight Purchase
```bash
# Search specific dates — June 18 to June 25 = 7 nights, so duration = 8 days
python scripts/scrape_date_range.py \
  --depart-start 2026-06-18 --depart-end 2026-06-18 \
  --origin tpe --dest kix --duration 8 --pax 2 \
  -o scrapes/june18-outbound.json

# Compare with package prices (Path B must run first to populate DB)
npm run travel -- query-offers --region kansai --start 2026-06-18 --end 2026-06-25 --max-price 30000 --json
```

**Note:** Stage 0 only researches flights. To compare direct vs package, run Path B below first to scrape and import package data, then return to Path A results.

### Path B: Package (Flight + Hotel)
```bash
# IMPORTANT: Run this BEFORE comparing direct vs package — Stage 0 only has flight data
# Search packages for the locked dates
npm run scraper:batch -- --dest kansai --date 2026-06-18 --type fit

# Import and query
npm run travel -- import-offers --dir scrapes --dest <slug>
npm run travel -- query-offers --plan-id <id> --dest <slug> --max-price 30000 --json
```

**Decision:** Direct flight only OR package?

- If package has good hotel + good flight → select package (P3+P4 done together)
- If direct flight is cheaper and user prefers choice of hotel → book flight separately → move to Stage 3

**If flight is cheaper on slightly different dates →** Go back to Stage 0 with new dates, re-run triangle research.

---

## Stage 3 — Expand Itinerary (Detailed Planning)

**Trigger:** Flight is confirmed (booked).

**Goal:** Fill in the full itinerary — attractions, transit between areas, meal arrangements (no breakfast), timing per session.

**What to do:**
```bash
# Set the confirmed flight details
npm run travel -- set-flight outbound --dest <slug> --flight <num> ...

# Validate the itinerary fits with actual flight times
npm run travel -- validate-itinerary --severity warning

# For each day, set activities with timing
npm run travel -- set-activity-time <day> <session> "<activity>" --start HH:MM --end HH:MM --fixed

# Set day themes
npm run travel -- set-day-theme <day> "Dotonbori Food Walk" --zh "道頓堀美食"

# Set session focus and transit
npm run travel -- set-tod-zh <day> <session> \
  --zh "午餐：美國村特色料理" \
  --transit-zh "地下鐵御堂筋線 難波→心齋橋 5分鐘"
```

**Fill order:**
1. Lock must-do activities (fixed time, priority = must)
2. Map out transit between areas
3. Fill in meals (lunch + dinner; no breakfast by default)
4. Add nice-to-have activities in remaining slots
5. Check pacing (relaxed/balanced/packed)

---

## Stage 4 — Publish to Dashboard

**Goal:** Make the plan visible on the web dashboard for reference and sharing.

**What to populate:**
```bash
# ZH content for all sessions (bilingual display)
# Bulk populate pattern from scripts/set-kyoto-zh-sessions-v2.ts

# Set all day themes in ZH
npm run travel -- set-day-theme 1 arrival --zh "大阪抵達" --dest <slug>
npm run travel -- set-day-theme 2 full --zh "道頓堀+美國村" --dest <slug>
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

---

## Stage 5 — Iteration Loop (Ongoing)

After any stage, user may want to iterate:

| Trigger | Action | Go to |
|---------|--------|-------|
| "Flight price changed — is June 19 better?" | Re-scrape flights | Stage 0 |
| "I want to add Kyoto as a second destination" | Update dates/destination | Stage 0 → Stage 1 |
| "Hotel in package looks bad" | Unselect offer, shop separately | Stage 2 |
| "Day 3 is too packed" | Re-balance itinerary | Stage 3 |
| "Add a must-see in Kyoto" | Insert activity, check transit | Stage 3 |
| "New attractions info online" | Update via CLI, redeploy | Stage 4 |

---

## Process Summary

```
┌─────────────────────────────────────────────────────────┐
│  STAGE 0: Triangle Research                             │
│  Departure dates + destination + flight price           │
│  Research loop until candidate locked                   │
└────────────────┬────────────────────────────────────────┘
                 │ user locks date + destination
                 ▼
┌─────────────────────────────────────────────────────────┐
│  STAGE 1: Itinerary Draft                               │
│  Rough day-by-day plan → validate timing fit            │
│  If bad timing → back to Stage 0                        │
└────────────────┬────────────────────────────────────────┘
                 │ itinerary fits
                 ▼
┌─────────────────────────────────────────────────────────┐
│  STAGE 2: Shop Flight                                   │
│  Direct booking OR package (flight + hotel)            │
│  If different dates cheaper → back to Stage 0          │
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

## Comparison: Old vs New

| Area | Original Flow | New Proposed Flow |
|------|---------------|-------------------|
| Overall model | Linear P1 → P2 → P3 → P4 → P5 | Iterative research-first loop |
| First decision | Lock dates first | Explore dates, destination, and flight price together |
| Destination choice | Chosen after dates are set | Compared alongside flight/date options |
| Flight prices | Checked after date/destination decisions | Used early as a primary decision signal |
| Date flexibility | Low; going back is possible but awkward | Expected; dates can change during research |
| Stage 0 equivalent | No dedicated triangle research stage | New Stage 0 researches date + destination + flight price |
| Itinerary timing | Built late after transport/accommodation | Rough itinerary drafted before booking |
| Package vs separate booking | P3 transport then P4 accommodation are separate sequential steps | Stage 2 chooses direct flight vs flight+hotel package |
| Multi-city lodging | Usually handled later during itinerary planning | Decided before package/direct-booking choice |
| Dashboard | Published near the end | Can be published any time useful |
| User iteration | More like restarting or backtracking | Explicit iteration loop after every stage |
| Main benefit | Simple, predictable process | Better optimization across price, dates, and destination |
| Main tradeoff | Can lock bad assumptions too early | Requires more upfront comparison and manual aggregation |

---

## Questions / Decisions Needed

1. **Stage 0 scrape scope**: How many date windows to explore per destination? (e.g., ±2 days around target = 5 dates × 3 durations = 15 combos per destination)
2. **Stage 1 — who drafts the itinerary?** Agent-only or user provides must-do list first?
3. **Stage 2 — package preference**: If package is within 10% of direct flight + separate hotel, which wins?
4. **Stage 3 — no breakfast default**: Confirm this is the right rule? (Some hotels include breakfast)
5. **Stage 4 — auto-deploy**: Should dashboard auto-update when itinerary changes, or only on explicit deploy?
