# Skill: /p3-flights

## Purpose
Search and compare flight options for travel planning.

## Trigger
- User asks to find/search flights
- Process 3 (Transportation) is current focus
- After P3 questionnaire is completed

## Inputs (from questionnaire)
```json
{
  "origin": "Taipei",
  "origin_airports": ["TPE", "TSA"],
  "destination": "Yokohama",
  "destination_airports": ["NRT", "HND"],
  "dates": {
    "outbound": "2026-02-11",
    "return": "2026-02-15"
  },
  "passengers": 2,
  "preferences": {
    "budget": "balanced",
    "outbound_time": "any",
    "return_time": "evening",
    "airlines": []
  }
}
```

## Execution Steps

### 1. Map Airports
- Resolve city names to airport codes
- Known mappings:
  - Taipei: TPE (Taoyuan), TSA (Songshan)
  - Tokyo/Yokohama: NRT (Narita), HND (Haneda)

### 2. Enumerate Routes
Generate all origin-destination airport combinations:
- TPE → NRT
- TPE → HND
- TSA → NRT
- TSA → HND

### 3. Search Each Route
For each route, search for:
- Airlines operating the route
- Flight schedules
- Prices (per person and total)
- Flight duration

### 4. Filter Results
- Check day-of-week operation for travel dates
- Filter by time preferences
- Filter by airline preferences (if any)

### 5. Rank Candidates
Ranking factors by budget preference:

**Budget mode:**
1. Price (lowest first)
2. Total travel time
3. Convenience

**Balanced mode:**
1. Value score (price vs convenience)
2. Departure/arrival times
3. Airport convenience

**Convenience mode:**
1. Airport convenience (TSA-HND best)
2. Flight times
3. Airline quality
4. Price

### 6. Output Candidates
Return structured JSON array:
```json
{
  "candidates": [
    {
      "id": "flight_xxx",
      "rank": 1,
      "route": "TSA-HND",
      "outbound": {
        "airline": "EVA Air",
        "flight_number": "BR190",
        "departure": "07:15",
        "arrival": "11:25",
        "airport_codes": ["TSA", "HND"]
      },
      "return": {
        "airline": "EVA Air",
        "flight_number": "BR191",
        "departure": "18:30",
        "arrival": "21:00",
        "airport_codes": ["HND", "TSA"]
      },
      "price_per_person": 8700,
      "total_price": 17400,
      "currency": "TWD",
      "pros": ["City-to-city convenience", "Full service", "Good times"],
      "cons": ["Higher price than LCC"],
      "source": "web_search"
    }
  ]
}
```

## Data Sources
- Web search for flight schedules
- Airline websites
- Flight aggregators (Skyscanner, Google Flights, KAYAK)
- Other agents (Comet, etc.)

## Output Destination
- Merge into `data/travel-plan.json` at `process_3_transportation.flight.candidates`
- Update state.json with event

## State Transitions
- On start: `pending` → `researching`
- On candidates found: `researching` → `researched`
- When user selects: `researched` → `selected`

## Notes
- Always search ALL airport combinations
- Include both budget (LCC) and full-service options
- Note airport transfer implications in pros/cons
- TSA-HND is most convenient for Yokohama trips
- NRT has more budget options but longer ground transfer
