---
name: p5-itinerary
description: Agent-first daily itinerary planning for the active destination, writing day-by-day plans into process_5_daily_itinerary.
version: 1.0.0
requires_skills: [travel-shared]
requires_processes: [process_1_date_anchor, process_2_destination, process_3_transportation, process_4_accommodation]
provides_processes: [process_5_daily_itinerary]
---

# /p5-itinerary

## Shared references

Read these first unless the request is extremely narrow:

- `../travel-shared/references/io-contracts.md`
- `../travel-shared/references/state-manager.md`
- `../travel-shared/references/cascade-triggers.md`
- `../travel-shared/references/itinerary-formats.md` — **Session vs Schedule formats, dashboard compatibility**
- `../travel-shared/references/destinations/{destination}.json` — POI, areas, clusters

## Agent-First Defaults

- Draft a complete first-pass itinerary (arrival/full/departure day shapes) before asking for optional preferences.
- Use `StateManager` for status transitions and event logging; avoid direct JSON edits.
- Always end with one clear next action (review day X, lock must-dos, or confirm pacing).
- Respect fixed-time activities as hard constraints; schedule flex activities around them.

## Write Path

- `destinations.{active_destination}.process_5_daily_itinerary`

## Prerequisites

Before running this skill, ensure:
- P1: Dates confirmed (`process_1_date_anchor.status = confirmed`)
- P2: Destination confirmed with cities/nights allocation
- P3: Transportation known (flight times determine arrival/departure windows)
- P4: Accommodation known (hotel location determines daily base)

## Day Types

| Type | Description | Available time |
|------|-------------|---------------|
| `arrival` | Flight lands, transit to hotel | Evening only (typically) |
| `full` | Full sightseeing day | Morning + Afternoon + Evening |
| `departure` | Checkout, last activities, transit to airport | Morning + early Afternoon |
| `free` | Package free day (no guided tour) | All day, self-planned |
| `guided` | Package guided day (tour bus) | Fixed by tour operator |

## Session Structure

Each day has 3 sessions: `morning`, `afternoon`, `evening`.

> **Format Note**: Use session-based format (documented here) for new itineraries. A legacy schedule-based format exists but is deprecated. See `../travel-shared/references/itinerary-formats.md` for details and migration guide.

```json
{
  "focus": "Area or theme name",
  "time_range": { "start": "09:00", "end": "12:00" },
  "activities": [],
  "meals": [],
  "transit_notes": "How to get there from previous session",
  "booking_notes": "What needs booking"
}
```

## Activity Schema

Activities can be strings (simple) or objects (rich):

```json
{
  "id": "activity_xxx",
  "title": "teamLab Borderless",
  "area": "Azabudai Hills",
  "nearest_station": "Kamiyacho",
  "duration_min": 150,
  "start_time": "10:00",
  "end_time": "12:30",
  "is_fixed_time": true,
  "booking_required": true,
  "booking_status": "pending",
  "booking_url": "https://...",
  "book_by": "2026-02-10",
  "cost_estimate": 3800,
  "tags": ["art", "immersive"],
  "priority": "must"
}
```

### Priority Levels

| Priority | Meaning | Scheduling rule |
|----------|---------|-----------------|
| `must` | Non-negotiable, plan day around it | Schedule first |
| `want` | Strong preference, drop only if conflict | Schedule second |
| `nice` | Optional filler, drop if tight | Schedule last |

## Workflow

### Step 1: Create Day Skeletons

```bash
npm run travel -- scaffold-itinerary --dest <destination>
```

Generates empty days based on date anchor:
- Day 1 = arrival (based on flight landing time)
- Days 2..N-1 = full (or free/guided if package)
- Day N = departure (based on flight departure time)

### Step 2: Cluster Assignment

Map user goals to destination clusters, then assign clusters to days.

**Algorithm:**
1. List requested clusters from destination reference
2. Sort by `duration_min` descending (longest first)
3. Assign to days using area proximity:
   - Group clusters in same area on same day
   - Avoid cross-city travel mid-day
4. Respect day type constraints (no clusters on arrival/departure days)

```bash
npm run travel -- populate-itinerary --goals "cluster1,cluster2" --pace balanced
```

### Step 3: Time Constraint Solving

Schedule activities within sessions respecting constraints:

**Hard constraints (never violate):**
- `is_fixed_time` activities at their specified times
- Flight departure/arrival times
- Hotel check-in/checkout times
- Business hours of attractions

**Soft constraints (prefer but flexible):**
- Pace preference (relaxed: 2 activities/day, balanced: 3-4, packed: 5+)
- Meal times (lunch 11:30-13:00, dinner 17:30-19:30)
- Transit buffer between areas (30 min default)

**Scheduling order:**
1. Place fixed-time activities first
2. Place `must` priority activities
3. Fill remaining slots with `want` then `nice`
4. Add transit notes between sessions
5. Add meal slots

### Step 4: Area Efficiency Check

Validate that each day minimizes unnecessary travel:

| Pattern | Status | Action |
|---------|--------|--------|
| All activities in same area | Good | Keep |
| 2 adjacent areas | Acceptable | Keep, add transit |
| 3+ scattered areas | Inefficient | Reorganize |
| Cross-city round trip | Bad | Swap days |

### Step 5: Validation

```bash
npm run travel -- validate-itinerary --severity warning
```

Checks for:
- **Time conflicts**: Overlapping fixed-time activities
- **Business hours**: Activities outside opening times
- **Booking deadlines**: Past-due `book_by` dates
- **Area efficiency**: Too many area changes per day
- **Pace violations**: Too many/few activities for pace setting
- **Missing meals**: Sessions without meal plan
- **Transit feasibility**: Enough time between areas

### Step 6: Review and Confirm

Present itinerary to user for approval. Status transitions:
- `pending` → `researched` (first draft created)
- `researched` → `selected` (user approves overall structure)
- `selected` → `confirmed` (all bookings made)

## Pacing Guide

| Pace | Activities/day | Buffer between | Session density |
|------|---------------|----------------|-----------------|
| `relaxed` | 2-3 | 45 min | 1 anchor per session |
| `balanced` | 3-4 | 30 min | 1-2 anchors per session |
| `packed` | 5-6 | 15 min | 2 anchors per session |

## Multi-City Patterns

### Base hotel + day trip
```
Day 1: Arrive Osaka, check-in
Day 2: [Free] Osaka (Dotonbori, Shinsekai)
Day 3: [Free] Day trip Kyoto (Fushimi Inari, Arashiyama)
Day 4: [Guided or Free] Nara + Kyoto
Day 5: Osaka shopping, depart
```

### Split stay
```
Day 1: Arrive Osaka
Day 2-3: Osaka activities
Day 3 evening: Move to Kyoto hotel
Day 4: Kyoto full day
Day 5: Kyoto → airport, depart
```

## Package Tour Integration

For package tours with guided + free days:

1. **Guided days**: Import itinerary from scraped package data (`extracted.itinerary`)
2. **Free days**: Plan using destination clusters and POIs
3. **Semi-free days**: Mark guided portions as `is_fixed_time`, plan around them

```
Lifetour example:
  Day 1: arrival (fixed)
  Day 2: FREE → use /p5-itinerary to plan
  Day 3: FREE → use /p5-itinerary to plan
  Day 4: GUIDED (Nara+Kyoto) → import from package, mark fixed
  Day 5: semi-guided (shopping+departure) → mark fixed portions
```

## CLI Commands

```bash
# Scaffold empty days
npm run travel -- scaffold-itinerary [--force]

# Populate with clusters
npm run travel -- populate-itinerary --goals "cluster1,cluster2" --pace balanced

# Reorder days
npm run travel -- swap-days <dayA> <dayB>  # Swap all activities between two days

# Set activity details
npm run travel -- set-activity-time <day> <session> "<activity>" --start HH:MM --end HH:MM [--fixed true]
npm run travel -- set-activity-booking <day> <session> "<activity>" <status> [--ref "..."] [--book-by YYYY-MM-DD]

# Set session boundaries
npm run travel -- set-session-time-range <day> <session> --start HH:MM --end HH:MM

# Validate
npm run travel -- validate-itinerary [--severity warning]
```

## Example Session

```
User: "Plan the free days for our Osaka trip"

Agent:
1. Read destination ref: osaka_kyoto.json → clusters, POIs, areas
2. Read package data: Day 2-3 are free, Day 4 guided
3. Check hotel location: Shinsaibashi area → Namba/Shinsaibashi base
4. Assign clusters:
   - Day 2: Dotonbori food + Shinsekai (same area, walking distance)
   - Day 3: USJ full day OR Umeda shopping (user choice)
5. Schedule with balanced pace:
   - Morning: 1 anchor activity
   - Afternoon: 1-2 activities in same area
   - Evening: Dinner + optional nightlife
6. Present draft → user confirms → mark researched
```
