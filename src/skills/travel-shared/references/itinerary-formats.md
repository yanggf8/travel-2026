# Itinerary Formats Reference

Two itinerary formats are supported. The **session-based format** is preferred for new itineraries.

## Format 1: Session-Based (Preferred)

Used by: Tokyo, recommended for all new plans

```json
{
  "day_number": 1,
  "date": "2026-02-13",
  "day_type": "arrival",
  "theme": "Arrival + Rest",
  "morning": {
    "focus": "Departure from TPE",
    "activities": ["TPE Airport", "TR874 TPE 13:55 → NRT 18:00"],
    "meals": ["Lunch before flight"],
    "transit_notes": "NRT arrival 18:00"
  },
  "afternoon": {
    "focus": "Flight & Arrival",
    "activities": [...],
    "meals": [...],
    "transit_notes": "..."
  },
  "evening": {
    "focus": "Hotel Check-in",
    "activities": [...],
    "meals": [...],
    "transit_notes": "..."
  },
  "weather": { ... }
}
```

### Session Keys
- `morning`: Activities before 12:00
- `afternoon`: Activities 12:00-17:00
- `evening`: Activities after 17:00

### Day Types
| Type | Description |
|------|-------------|
| `arrival` | Flight lands, transit to hotel |
| `departure` | Checkout, transit to airport |
| `full` | Full sightseeing day |
| `free` | Package free day |
| `guided` | Package guided day |

## Format 2: Schedule-Based (Legacy)

Used by: Kyoto (legacy), some quick drafts

```json
{
  "day": 1,
  "date": "2026-02-24",
  "title": "Arrival + Fushimi Inari",
  "theme": "Arrival & iconic torii gates",
  "schedule": [
    {
      "time": "09:00",
      "activity": "Depart TPE (Thai Lion Air)",
      "location": "Taoyuan Airport"
    },
    {
      "time": "12:30",
      "activity": "Arrive KIX",
      "location": "Kansai Airport"
    },
    {
      "time": "13:00-14:30",
      "activity": "JR Haruka to Kyoto Station",
      "transport": "JR Haruka (¥1,800)",
      "duration": "75 min"
    }
  ],
  "transport_cost": "¥1,950"
}
```

### Schedule Item Keys
- `time`: Start time or range (e.g., "09:00" or "13:00-14:30")
- `activity`: Activity description
- `location`: Optional location name
- `transport`: Optional transport method
- `duration`: Optional duration string
- `notes`: Optional notes

## Dashboard Compatibility

The trip dashboard (`workers/trip-dashboard/`) supports **both formats**:

| Format | Field Mapping |
|--------|---------------|
| Session-based | `day_number`, `day_type`, `morning/afternoon/evening` |
| Schedule-based | `day` → `day_number`, infers `day_type` from title, converts `schedule[]` to sessions |

### Day Type Inference (Schedule Format)
- Title contains "arrival" or "抵達" → `arrival`
- Title contains "departure" or "回程" → `departure`
- Schedule contains "Arrive KIX/NRT" → `arrival`
- Schedule contains "Depart K/N" → `departure`
- Otherwise → `full`

## Migration Guide

To convert schedule-based to session-based:

1. Replace `day` with `day_number`
2. Add explicit `day_type`
3. Group schedule items by time:
   - `< 12:00` → `morning`
   - `12:00-17:00` → `afternoon`
   - `> 17:00` → `evening`
4. Extract meals from activities
5. Extract transport notes

## Validation

```bash
npm run travel -- validate-itinerary [--format session|schedule]
```

Checks:
- Required fields present for format type
- Day numbers sequential
- Dates match date anchor
- No overlapping times
