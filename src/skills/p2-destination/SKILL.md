---
name: p2-destination
description: Set up destination details including cities, nights allocation, and attraction priorities for the active destination.
---

# /p2-destination

## Shared references

Read these first unless the request is extremely narrow:

- `../travel-shared/references/io-contracts.md`
- `../travel-shared/references/state-manager.md`
- `../travel-shared/references/cascade-triggers.md`
- `../travel-shared/references/destinations/{destination}.json` - destination reference file

## Agent-First Defaults

- For multi-city trips, propose a night allocation based on attraction count and travel logistics.
- Use `StateManager` for status transitions; avoid direct JSON edits.
- Always confirm primary airport matches the package search requirements.

## Write Path

- `destinations.{active_destination}.process_2_destination`

## Workflow

1. **Read destination reference** (`src/skills/travel-shared/references/destinations/{slug}.json`)
2. **Collect city preferences** from user (or infer from attractions list)
3. **Determine night allocation**:
   - Count attractions per city
   - Account for inter-city travel time
   - Suggest split (e.g., "2 nights Osaka + 2 nights Kyoto")
4. **Set primary airport** based on package availability (KIX for Kansai)
5. **Write cities array** with slug, display_name, role, nights, attractions
6. **Mark process as `confirmed`**

## City Role Types

| Role | Description | Example |
|------|-------------|---------|
| `primary` | Main base, overnight stays | Osaka (3 nights) |
| `day_trip` | Visit but return to base | Nara (1 day from Osaka) |
| `mixed` | Split nights between cities | Kyoto (2 nights) |

## Multi-City Patterns

### Osaka + Kyoto (15min shinkansen)
```
Option A: 3 nights Osaka + 1 night Kyoto
  - Osaka: Namba, Dotonbori food, Osaka Castle
  - Kyoto: Fushimi Inari, Arashiyama, Gion

Option B: 2 nights Osaka + 2 nights Kyoto (recommended)
  - More time in Kyoto for temples
  - Easy day-trip from Kyoto to Nara

Option C: All nights in Osaka, day-trip to Kyoto
  - Simpler hotel logistics
  - 45min each way by JR
```

### Recommended defaults for 5-day trip:
- **3 nights Osaka + 1 night Kyoto** (most flexible)
- **2 nights Osaka + 2 nights Kyoto** (balanced)

## Output format

```json
{
  "status": "confirmed",
  "origin_city": "Taipei",
  "origin_country": "Taiwan",
  "destination_country": "Japan",
  "region": "Kansai",
  "primary_airport": "KIX",
  "cities": [
    {
      "slug": "osaka",
      "display_name": "Osaka",
      "role": "primary",
      "nights": 3,
      "attractions": ["Dotonbori", "Osaka Castle", "Kuromon Market"]
    },
    {
      "slug": "kyoto",
      "display_name": "Kyoto",
      "role": "primary",
      "nights": 1,
      "attractions": ["Fushimi Inari", "Kinkaku-ji", "Arashiyama"]
    }
  ]
}
```

## Status transitions

- `pending` → `confirmed` (once cities/nights are set)

## Cascade trigger

Setting `process_2_destination` to `confirmed`:
- Resets `process_3_*`, `process_4_*`, `process_5_*` for current destination
- Scope: current_destination only
