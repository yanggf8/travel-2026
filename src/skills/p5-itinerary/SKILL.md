---
name: p5-itinerary
description: Agent-first daily itinerary planning for the active destination, writing day-by-day plans into process_5_daily_itinerary.
---

# /p5-itinerary

## Shared references

Read these first unless the request is extremely narrow:

- `../travel-shared/references/io-contracts.md`
- `../travel-shared/references/date-filters.md`
- `../travel-shared/references/state-manager.md`
- `../travel-shared/references/cascade-triggers.md`

## Agent-First Defaults

- Draft a complete first-pass itinerary (arrival/full/departure day shapes) before asking for optional preferences.
- Use `StateManager` for status transitions and event logging; avoid direct JSON edits.
- Always end with one clear next action (review day X, lock must-dos, or confirm pacing).

## Write Path

- `destinations.{active_destination}.process_5_daily_itinerary`

## Output shape (stub)

Write a minimal structure that can be refined later:

- `days[]`: `{ date, day_number, day_type, status, morning, afternoon, evening }`
- Each session: `{ focus, activities[], meals[], transit_notes, booking_notes }`

## Workflow (stub)

1. Read date anchor + destination constraints
2. Create day skeletons (arrival/full/departure)
3. Populate a balanced plan (1–2 anchors/day) with buffers
4. Mark `process_5_daily_itinerary` as `researched` → `selected` → `confirmed` as the user approves

