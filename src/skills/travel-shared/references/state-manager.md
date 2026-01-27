# State + Dirty Flags (Shared)

## Sources of truth

| File | Owner | Purpose |
|------|-------|---------|
| `data/travel-plan.json` | Skills/StateManager | Trip data, status, dirty flags |
| `data/state.json` | StateManager | Event log, audit trail |
| `cascade_state.last_cascade_run` | Cascade Runner | Cascade ownership |

## StateManager API

Import: `import { StateManager, getStateManager } from '../state'`

### Timestamps (atomic per session)

```typescript
sm.now()              // Returns session timestamp (same for all ops)
sm.freshTimestamp()   // Generate new ISO timestamp
sm.refreshTimestamp() // Start new session batch
```

### Process Status

Valid statuses: `pending` → `researching` → `researched` → `selecting` → `selected` → `booking` → `booked` → `confirmed` | `skipped`

```typescript
sm.setProcessStatus(destination, process, newStatus)  // Validates transition
sm.getProcessStatus(destination, process)             // Returns current status
sm.isValidTransition(from, to)                        // Check if allowed
```

### Dirty Flags

```typescript
sm.markDirty(destination, process)      // Mark for cascade re-evaluation
sm.clearDirty(destination, process)     // Clear after cascade processes
sm.isDirty(destination, process)        // Check if dirty
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

## Package select convention

When a package offer is selected:

1. Set `destinations.{slug}.process_3_4_packages.selected_offer_id`
2. Set `destinations.{slug}.process_3_4_packages.results.chosen_offer`
3. Mark dirty: `sm.markDirty(slug, 'process_3_4_packages')`
4. Run cascade to populate P3/P4

## Cascade Runner Ownership

The cascade runner (`src/cascade/runner.ts`) owns:
- Reading dirty flags to compute plan
- Clearing dirty flags for fired triggers
- Updating `cascade_state.last_cascade_run`

Skills should NOT call `markCascadeRun()` - that's cascade-runner-only.

