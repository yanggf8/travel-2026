# Skill: /p3-flights

Search and compare flight options for travel planning (standalone, not package).

## Overview

| Field | Value |
|-------|-------|
| **Skill ID** | `p3_flights` |
| **Version** | `1.1.0` |
| **Schema Compatibility** | `travel-plan.json ^4.2.0` |
| **Write Path** | `destinations.{active_destination}.process_3_transportation.flight` |
| **Use Case** | When booking flights separately from hotels |

## Trigger
- User asks to find/search flights separately
- Process 3 (Transportation) is current focus
- After P3 questionnaire is completed
- **Note**: For package deals (flight+hotel), use `/p3p4-packages` instead

## Input Contract

```typescript
interface P3FlightsInput {
  // Required
  active_destination: string;  // e.g., "tokyo_2026"
  
  origin: string;              // e.g., "Taipei"
  origin_airports: string[];   // e.g., ["TPE", "TSA"]
  
  destination: string;         // e.g., "Tokyo"
  destination_airports: string[];  // e.g., ["NRT", "HND"]
  
  dates: {
    outbound: string;          // ISO-8601 date
    return: string;            // ISO-8601 date
    flexible: boolean;         // search ±3 days if true
  };
  
  passengers: number;          // default: 2
  
  preferences: {
    budget: "budget" | "balanced" | "convenience";
    outbound_time: "morning" | "afternoon" | "evening" | "any";
    return_time: "morning" | "afternoon" | "evening" | "any";
    airlines: string[];        // preferred airlines (empty = any)
    avoid_red_eye: boolean;    // no 00:00-05:00 departures
  };
}
```

## Example Input (Tokyo 2026)

```json
{
  "active_destination": "tokyo_2026",
  "origin": "Taipei",
  "origin_airports": ["TPE", "TSA"],
  "destination": "Tokyo",
  "destination_airports": ["NRT", "HND"],
  "dates": {
    "outbound": "2026-02-21",
    "return": "2026-02-25",
    "flexible": true
  },
  "passengers": 2,
  "preferences": {
    "budget": "balanced",
    "outbound_time": "any",
    "return_time": "evening",
    "airlines": [],
    "avoid_red_eye": true
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

## Output Contract

```typescript
interface P3FlightsOutput {
  candidates: FlightCandidate[];
  chosen_flight: FlightCandidate | null;
  provenance: Array<{
    source: string;
    searched_at: string;
    results_count: number;
  }>;
  warnings: string[];
}

interface FlightCandidate {
  id: string;              // format: flight_{airline}_{route}_{date}
  rank: number;
  route: string;           // e.g., "TPE-NRT"
  outbound: FlightLeg;
  return: FlightLeg;
  price_per_person: number;
  total_price: number;
  currency: string;
  pros: string[];
  cons: string[];
  source: string;
}
```

## Output Destination

Write to: `destinations.{active_destination}.process_3_transportation.flight`
- `candidates`: sorted flight options
- `chosen_flight`: user-selected flight (null until selection)
- Update `data/state.json` with event log

## State Transitions

| From | Event | To |
|------|-------|-----|
| `pending` | search started | `researching` |
| `researching` | candidates found | `researched` |
| `researched` | user selects | `selected` |
| `selected` | user changes | `researched` |

## Airport Reference (Taiwan ↔ Japan)

| Code | Airport | City | Notes |
|------|---------|------|-------|
| **TPE** | Taoyuan | Taipei | Main international hub |
| **TSA** | Songshan | Taipei | Domestic + regional, city center |
| **NRT** | Narita | Tokyo | More LCC options, 60-90min to city |
| **HND** | Haneda | Tokyo | Closer to city, premium carriers |

## Route Recommendations (Tokyo trips)

| Priority | Route | Pros | Cons |
|----------|-------|------|------|
| 1 | TSA-HND | City-to-city, fastest ground transfer | Limited schedules, higher price |
| 2 | TPE-HND | Good balance, close to Tokyo | Fewer budget options |
| 3 | TPE-NRT | Most LCC options, cheapest | Long ground transfer (90min) |

## Notes

- Always search ALL airport combinations (TPE/TSA × NRT/HND)
- Include both budget (LCC: Peach, Tigerair, Scoot) and full-service (EVA, JAL, ANA)
- Note airport transfer implications in pros/cons
- HND is preferred for Tokyo trips (30min to most areas)
- NRT is acceptable if price difference > TWD 3,000/person
