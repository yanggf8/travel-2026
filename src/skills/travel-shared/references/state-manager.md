# State + Dirty Flags (Shared)

## Sources of truth

| File | Owner | Purpose |
|------|-------|---------|
| `data/travel-plan.json` (or `$TRAVEL_PLAN_PATH`) | Skills/StateManager | Trip data, status, dirty flags |
| `data/state.json` (or `$TRAVEL_STATE_PATH`) | StateManager | Event log, audit trail |
| `cascade_state.last_cascade_run` | Cascade Runner | Cascade ownership |

## StateManager API

Import: `import { StateManager, getStateManager } from '../state'`

Types:
- `ProcessId` — `'process_1_date_anchor' | 'process_2_destination' | 'process_3_4_packages' | 'process_3_transportation' | 'process_4_accommodation' | 'process_5_daily_itinerary'`
- `ProcessStatus` — includes `skipped` (see `src/state/types.ts`)

### Timestamps (atomic per session)

```typescript
sm.now()              // Returns session timestamp (same for all ops)
sm.freshTimestamp()   // Generate new ISO timestamp
sm.refreshTimestamp() // Start new session batch
```

### Process Status

Valid statuses: `pending` → `researching` → `researched` → `selecting` → `selected` → `booking` → `booked` → `confirmed` | `skipped` | `populated` (from package cascade)

```typescript
sm.setProcessStatus(destination, process: ProcessId, newStatus)  // Validates transition
sm.getProcessStatus(destination, process: ProcessId)             // Returns current status
sm.isValidTransition(from, to)                        // Check if allowed
```

### Dirty Flags

```typescript
sm.markDirty(destination, process: ProcessId)      // Mark for cascade re-evaluation
sm.clearDirty(destination, process: ProcessId)     // Clear after cascade processes
sm.isDirty(destination, process: ProcessId)        // Check if dirty
sm.getDirtyFlags()                      // Get all cascade_state
sm.markGlobalDirty('process_1_date_anchor')  // Global trigger
```

### Event Logging

```typescript
sm.emitEvent({ event: 'selected', destination, process, data: {...} })
sm.getEventLog()  // Returns TravelEvent[]
```

### File I/O

```typescript
sm.save()      // Writes travel-plan.json + state.json
sm.getPlan()   // Returns current plan (read-only)
```

### Itinerary Management

```typescript
sm.scaffoldItinerary(destination, days[], force?)  // Create day skeletons for P5
// force=true resets P5 to pending first, allowing re-scaffold
```

Day skeleton shape:
```typescript
{
  date: "2026-02-13",
  day_number: 1,
  day_type: "arrival" | "full" | "departure",
  status: "draft",
  morning: { focus, activities[], meals[], transit_notes, booking_notes },
  afternoon: { focus, activities[], meals[], transit_notes, booking_notes },
  evening: { focus, activities[], meals[], transit_notes, booking_notes },
}
```

### Activity CRUD

```typescript
// Set day/session themes (optional but recommended)
sm.setDayTheme(destination, dayNumber, "Shinjuku shopping day")
sm.setSessionFocus(destination, dayNumber, "morning", "Luxury shopping")

// Add activity to a session
const id = sm.addActivity(destination, dayNumber, session, {
  title: "KOMEHYO Shinjuku",
  area: "shinjuku",
  nearest_station: "Shinjuku",
  duration_min: 90,
  booking_required: false,
  tags: ["shopping", "luxury"],
  priority: "must",
});

// Update activity
sm.updateActivity(destination, dayNumber, session, activityId, {
  notes: "Focus on Chanel section, 3rd floor",
});

// Remove activity
sm.removeActivity(destination, dayNumber, session, activityId);
```

Activity schema (see `src/state/types.ts`):
```typescript
interface Activity {
  id: string;                    // auto-generated
  title: string;
  area: string;
  nearest_station: string | null;
  duration_min: number | null;
  booking_required: boolean;
  booking_url: string | null;
  cost_estimate: number | null;
  tags: string[];
  notes: string | null;
  priority: 'must' | 'want' | 'optional';
}
```

## Package select convention

When a package offer is selected:

1. Call `sm.selectOffer(offerId, date, populateCascade=true)`
   - Sets `process_3_4_packages.selected_offer_id = offerId`
   - Sets `process_3_4_packages.chosen_offer = {id, selected_date, selected_at}` (selection metadata)
   - Sets `process_3_4_packages.results.chosen_offer = <full offer object>` (for cascade / downstream)
   - Updates status to `selected`
   - If `populateCascade=true`, populates P3/P4 with status `populated`

## Itinerary scaffold convention

When creating day skeletons for P5:

1. Call `sm.scaffoldItinerary(destination, days)`
   - Writes `process_5_daily_itinerary.days = days`
   - Sets `scaffolded_at` timestamp
   - Updates status to `researching` (skeleton created, content pending)
   - Clears dirty flag

2. After populating activities, manually advance:
   - `sm.setProcessStatus(dest, 'process_5_daily_itinerary', 'researched')`

## Cascade Runner Ownership

The cascade runner (`src/cascade/runner.ts`) owns:
- Reading dirty flags to compute plan
- Clearing dirty flags for fired triggers
- Updating `cascade_state.last_cascade_run`

Cascade-runner-only helper:

```typescript
sm.markCascadeRun(timestamp?) // pass cascadePlan.computed_at for exact match
```

Skills should NOT call `markCascadeRun()` - that's cascade-runner-only.
