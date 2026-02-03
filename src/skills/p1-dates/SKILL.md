---
name: p1-dates
description: Set trip dates including duration, flexibility preferences, and date anchor for the travel plan.
---

# /p1-dates

## Shared references

Read these first unless the request is extremely narrow:

- `../travel-shared/references/io-contracts.md`
- `../travel-shared/references/date-filters.md`
- `../travel-shared/references/state-manager.md`
- `../travel-shared/references/cascade-triggers.md`

## Agent-First Defaults

- Propose concrete date ranges with day-of-week awareness.
- Account for weekend premiums (Fri-Sun flights cost more).
- Consider seasonal factors (Cherry blossom, Golden Week, holidays).
- Always confirm duration in nights, not days.

## Write Path

- `process_1_date_anchor` (root level - shared across all destinations)

## Workflow

1. **Collect constraints**:
   - Desired duration (e.g., 5 days = 4 nights)
   - Date flexibility (exact vs ±2-3 days)
   - Avoid dates (holidays, events, price peaks)
   - Weekend preference (arrive Thu/Fri for weekend start?)

2. **Calculate date options**:
   - Generate 2-3 date range candidates
   - Include day-of-week for each date
   - Flag any price concerns (CNY, Golden Week, weekends)

3. **Confirm selection**:
   - User picks exact dates OR
   - Agent recommends based on constraints

4. **Write date anchor**:
   - Set `set_out_date`, `return_date`, `duration_days`
   - Mark `status` as `confirmed`
   - Record flexibility for future reference

## Date Flexibility Types

| Flexibility | Description | Use Case |
|-------------|-------------|----------|
| `exact` | Only specific dates work | Must visit on specific event day |
| `±2_days` | Flexible within 2 days | Budget-conscious, avoiding peaks |
| `±1_week` | Highly flexible | Off-peak season, looking for deals |

## Date Anchor Schema

```json
{
  "status": "confirmed",
  "set_out_date": "2026-02-25",
  "duration_days": 5,
  "return_date": "2026-03-01",
  "flexibility": {
    "date_flexible": false,
    "preferred_dates": ["2026-02-25"],
    "avoid_dates": ["2026-02-14", "2026-02-15", "2026-02-16"],
    "reason": "Avoid CNY pricing peak Feb 14-17"
  }
}
```

## Common Patterns

### Pattern: Weekend departure (4-5 day trip)
```
Start: Thursday or Friday
End: Monday or Tuesday
Example: Feb 26 (Thu) → Mar 2 (Mon)
```

### Pattern: Mid-week departure (budget)
```
Start: Tuesday or Wednesday
End: Sunday or Monday
Example: Feb 25 (Wed) → Mar 1 (Sun)
```

### Pattern: Flexible dates
```
Option A: Feb 25 (Wed) → Mar 1 (Sun) - 5 days
Option B: Feb 26 (Thu) → Mar 2 (Mon) - 5 days
```

## Duration Guidelines

| Trip Type | Recommended Nights |
|-----------|-------------------|
| Single city | 3-4 nights |
| Two cities | 4-5 nights |
| Multi-city (3+) | 7+ nights |

## Status transitions

- `pending` → `confirmed` (once dates are locked)

## Cascade trigger

Changing `process_1_date_anchor`:
- Resets `process_3_*`, `process_4_*`, `process_5_*` for **all** destinations
- Scope: global (affects Tokyo + Nagoya + Osaka/Kyoto)

## Weekday reference

| Day | Code | Notes |
|-----|------|-------|
| Monday | Mon | Lowest flight prices |
| Tuesday | Tue | Second lowest |
| Wednesday | Wed | Moderate |
| Thursday | Thu | Weekend pricing starts |
| Friday | Fri | Peak prices |
| Saturday | Sat | Peak prices |
| Sunday | Sun | Return pricing peaks |

## Example session

```
User: "5 days, can start Feb 25 or 26"

Agent: "Here are two 5-day options:

Option A: Feb 25 (Wed) → Mar 1 (Sun)
  - Depart Wed, return Sun (work days: Mon-Tue)
  - Cheaper flights (weekday departure)
  - 5 days, 4 nights

Option B: Feb 26 (Thu) → Mar 2 (Mon)
  - Depart Thu, return Mon (work day: Fri only)
  - Weekend departure = higher prices
  - 5 days, 4 nights

Which do you prefer?"
```

## CLI equivalent

```bash
npm run travel -- set-dates 2026-02-25 2026-03-01
```
