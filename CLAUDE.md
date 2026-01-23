# Yokohama Travel Project

## Trip Details
- **Dates**: February 11-15, 2026 (20260211 - 20260215)
- **Destination**: Yokohama, Japan

## Project Goals

### Primary Objectives
1. **Status Check Program** - Create a program to check and report travel project status
2. **Process Program as Agent Tool** - Build process automation that can be used as an agent tool to assist in completing travel planning tasks
3. **Claude Skill Conversion** - Eventually convert this project into a reusable Claude skill for travel planning

### Travel Planning Processes

#### Status & Milestone System
**Readiness definition**: "Can proceed to next dependent process"

| Status | Meaning |
|--------|---------|
| `pending` | Not started |
| `researched` | Options gathered, ready for selection |
| `selected` | Choice made, ready for booking |
| `booked` | Reservation confirmed |
| `confirmed` | Verified and finalized |

#### Process 1: Date Anchor
- **Set out date**: The departure date
- **Duration**: Number of travel days
- **Return date**: Calculated from set out date + duration
- Milestones: `pending` → `confirmed`
- Ready when: all 3 date fields filled

#### Process 2: Destination
- **Origin**: City and country of departure (e.g., Taipei, Taiwan)
- **Destination country**: Japan
- **Cities**: List of cities to visit with role and attractions
  - `name`: City name (e.g., Yokohama)
  - `role`: primary | day_trip
  - `nights`: Number of nights staying
  - `attractions[]`: Places to visit
- **Shopping**: Standalone shopping goals (cross-city)
  - `type`: Category (e.g., second_hand_luxury, general)
  - `stores[]`: Specific stores to visit
- Milestones: `pending` → `confirmed`
- Ready when: destination_country + at least one city filled

#### Process 3: Transportation
- **3.1 Flight**: Airline, flight number, departure/arrival times
- **3.2 Home → Airport**: Route, transport method, duration, cost
- **3.3 Airport → Hotel**: Route, transport method, duration, cost
- Milestones: `pending` → `researched` → `selected` → `booked`
- Ready when: flight outbound has airline + airports + datetime

#### Process 4: Accommodation
- **4.1 Location Zone**: Select area/district for hotel
- **4.2 Hotel Selection**: Exact hotel or candidate list with comparison
- Criteria: Price, location, amenities, reviews
- Milestones:
  - Zone: `pending` → `researched` → `selected`
  - Hotel: `pending` → `researched` → `selected` → `booked`
- Ready for zone: selected_area filled
- Ready for hotel: selected_hotel filled + booking confirmation

#### Process 5: Daily Itinerary
For each day:
- **Morning session** (before lunch): Attractions, activities, timing
- **Afternoon session** (after lunch): Attractions, activities, timing
- **Evening session** (optional): Dinner, night activities
- Milestones per day: `pending` → `researched` → `selected` → `confirmed`
- Ready when: morning has ≥1 activity AND afternoon has ≥1 activity

### Process Dependencies
```
[1. Date Anchor] ──┬──→ [3. Transportation] ──→ [4.1 Location Zone]
                   │                                    ↓
[2. Destination] ──┘                           [4.2 Hotel Selection]
                                                        ↓
                                               [5. Daily Itinerary]
```

### Development Phases

#### Phase 1: Status Check Program
- Check completion status of all 5 processes
- Report missing information
- Calculate overall readiness percentage

#### Phase 2: Process Program (Agent Tool)
- Execute each process step with agent assistance
- Research and gather options
- Provide structured outputs for decision making

#### Phase 3: Claude Skill
- Package into reusable travel planning skill
- Templated workflows for any destination

## Architecture

### System Design
```
Base Info Questionnaire → fills P1 + P2 directly
         ↓
    Skill (search) → Tool (validate) → JSON (store)
         ↓
    State Manager (event-driven state tracking)
```

### Components

| Component | Purpose |
|-----------|---------|
| **Questionnaire** | Collect inputs per process |
| **Skills** | `/p3-flights`, `/p4-hotels`, `/p5-itinerary` |
| **Tools** | Validate, merge, rank candidates (TypeScript) |
| **State Manager** | Event-driven state tracking |
| **JSON** | Single source of truth + readiness rules |

### State Machine Model
- Events trigger state changes
- Valid transitions defined in `data/state.json`
- States: `pending` → `researching` → `researched` → `selecting` → `selected` → `booking` → `booked` → `confirmed`

### Process-Skill Mapping

| Process | Questionnaire | Skill | Tool |
|---------|--------------|-------|------|
| P1 Date + P2 Dest | `base_info` | (none - direct fill) | - |
| P3 Transportation | `p3_transport` | `/p3-flights` | `transportation.ts` |
| P4 Accommodation | `p4_hotel` | `/p4-hotels` | `accommodation.ts` |
| P5 Itinerary | `p5_itinerary` | `/p5-itinerary` | `itinerary.ts` |

## Project Structure
```
/
├── CLAUDE.md              # Project configuration and goals
├── src/
│   ├── status/            # Status check program
│   │   ├── status-check.ts
│   │   └── rule-evaluator.ts
│   ├── process/           # Process automation tools
│   │   ├── types.ts
│   │   ├── plan-updater.ts
│   │   ├── transportation.ts
│   │   ├── accommodation.ts
│   │   └── itinerary.ts
│   ├── questionnaire/     # (planned) Input collection
│   └── skills/            # (planned) Skill definitions
├── data/
│   ├── travel-plan.json   # Trip data + readiness rules
│   └── state.json         # Event-driven state tracking
└── docs/                  # Documentation
```
