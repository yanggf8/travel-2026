# DB-First Pattern (Critical)

## Rule: Never Edit JSON Directly

**All mutations must go through StateManager or CLI commands.**

### Why This Matters

Direct JSON edits bypass:
1. **Event logging** — No audit trail of what changed
2. **Dirty flags** — Cascade system doesn't know to re-evaluate
3. **Status validation** — Invalid state transitions allowed
4. **Timestamp tracking** — No record of when changes occurred

### The Right Way

```typescript
// WRONG — Direct JSON edit
plan.destinations.tokyo_2026.process_3_transportation.status = 'selected';

// RIGHT — Via StateManager
const sm = await StateManager.create();
sm.setProcessStatus('tokyo_2026', 'process_3_transportation', 'selected');
await sm.saveWithTracking('select-transport', 'tokyo_2026');
```

```bash
# RIGHT — Via CLI
npm run travel -- mark-booked --dest tokyo_2026
npm run travel -- set-activity-booking 2 morning "teamLab" booked --ref "TLB-12345"
```

### Common Scenarios

| Task | Wrong | Right |
|------|-------|-------|
| Change dates | Edit JSON directly | `npm run travel -- set-dates <start> <end>` |
| Select package | Edit `selected_offer_id` | `npm run travel -- select-offer <id> <date>` |
| Mark booked | Edit `status: "booked"` | `npm run travel -- mark-booked` |
| Add activity | Push to `activities[]` | `sm.addActivity(dest, day, session, {...})` |
| Update booking | Edit `booking_status` | `npm run travel -- set-activity-booking ...` |
| Swap days | Rewrite day arrays | `npm run travel -- swap-days <A> <B>` |

### Read-Only Operations

Reads are fine — StateManager or direct JSON parse:
```typescript
const sm = await StateManager.create();
const plan = sm.getPlan();
console.log(plan.destinations.tokyo_2026.process_1_date_anchor.start_date);
```

### Emergency Override

If you must edit data directly (system broken, migration, etc.):
1. Document why in commit message
2. Manually emit event: `sm.emitEvent({event: 'manual_edit', ...})`
3. Mark affected processes dirty: `sm.markDirty(dest, process)`
4. Run cascade: `npx ts-node src/cli/cascade.ts --apply`

### Quick Reference

**Before any mutation, ask:**
- Is there a CLI command? Use it
- Is there a StateManager method? Use it
- Neither exists? Create the command/method first

**See also:**
- `references/state-manager.md` — Full StateManager API
- `src/contracts/skill-contracts.ts` — All CLI commands
