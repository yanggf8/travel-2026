# Deprecated Code

These files use a flat schema incompatible with the current nested destination schema (v4.2.0+).

**Do not use.** Kept for reference only.

| File | Replaced By |
|------|-------------|
| `types.ts` | `src/state/types.ts` |
| `itinerary.ts` | `src/cli/travel-update.ts` (populate-itinerary) |
| `transportation.ts` | StateManager.setAirportTransfer |
| `accommodation.ts` | StateManager + select-offer |
| `plan-updater.ts` | StateManager |

To delete: `rm -rf src/_deprecated`
