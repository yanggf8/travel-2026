# Architecture Decisions

## ADR-001: StateManager must use fine-grained DB operations (no in-memory repo)

### Decision

**DB is the single source of truth. StateManager reads state from DB and writes changes back as targeted SQL — one command, one precise UPDATE or INSERT. No in-memory plan object. No coarse-grained flush.**

### Context

The current implementation loads the entire plan (38 queries) into a `PlanRepository` object, mutates it in memory, then flushes all rows back via `syncNormalizedTables()` on every `save()`. This causes:

- **Dual-path sync bugs**: adding a DB column requires updating `plan-assembler.ts` (read path) AND `plan-repository.ts` INSERT (write path) independently — compiler cannot enforce this
- **Coarse writes**: every `save()` rewrites entire tables even when only one field changed
- **False source of truth**: the DB reflects state only after `save()`, not after each command
- **Hidden data loss**: a column missing from `INSERT OR REPLACE` silently wipes its value on every save

### Correct Pattern

Each `StateManager` method:
1. Reads **only what it needs** to validate preconditions (targeted SELECT)
2. Writes **exactly what changed** (targeted parameterized UPDATE or INSERT)
3. No assembled plan object in memory
4. No `syncNormalizedTables()` flush

```typescript
// WRONG — current pattern
setSessionFocus(dest, day, session, focus, focus_zh) {
  this.repo.setSessionField(dest, day, session, 'focus', focus);   // mutates memory
  this.repo.setSessionField(dest, day, session, 'focus_zh', focus_zh);
  await sm.save();  // flushes entire itinerary_sessions table
}

// RIGHT — target pattern
async setSessionFocus(dest, day, session, focus, focus_zh?) {
  // validate
  const exists = await db.queryOne(
    'SELECT 1 FROM itinerary_sessions WHERE plan_id=? AND destination=? AND day_number=? AND session_type=?',
    [planId, dest, day, session]
  );
  if (!exists) throw new Error(`Session D${day}/${session} not found in ${dest}`);

  // write exactly what changed
  await db.execute(
    'UPDATE itinerary_sessions SET focus=?, focus_zh=?, updated_at=datetime("now") WHERE plan_id=? AND destination=? AND day_number=? AND session_type=?',
    [focus, focus_zh ?? null, planId, dest, day, session]
  );
}
```

Adding a new column in the future: update **one** UPDATE statement. No assembler. No INSERT. No silent data loss.

### What this removes

| Removed | Reason |
|---------|--------|
| `PlanRepository` | In-memory object; replaced by targeted SQL per command |
| `plan-assembler.ts` | Only needed to build the in-memory object; kept for dashboard `turso.ts` only |
| `syncNormalizedTables()` | Coarse flush; replaced by per-command writes |
| `TravelPlanMinimal` in-memory type | No object to assemble into |
| `repo.setField() / setSessionField()` | Replaced by SQL in each StateManager method |

### What stays

| Kept | Reason |
|------|--------|
| `StateManager` class and `dispatch()` | Same external API; commands unchanged |
| All 25 Command types | Interface to callers does not change |
| Cascade logic | Stays in StateManager; uses targeted reads instead of in-memory checks |
| `turso-service.ts` DAL | Becomes the DB client; refine into typed `DbClient` |
| Operation tracking (`operation_runs`) | Fine-grained write; already correct |
| Dashboard `turso.ts` | Reads in batch for rendering; independent of StateManager |

### DB client contract

All StateManager methods use a `DbClient` with two operations:

```typescript
interface DbClient {
  // read: one row or null
  queryOne<T>(sql: string, args: SqlArg[]): Promise<T | null>;
  // read: multiple rows
  queryMany<T>(sql: string, args: SqlArg[]): Promise<T[]>;
  // write: returns affected_row_count
  execute(sql: string, args: SqlArg[]): Promise<{ affected: number }>;
}

type SqlArg = { type: 'text' | 'integer' | 'real' | 'null'; value: string };
```

**Integer args must use string values**: `{ type: 'integer', value: '1' }` — Turso HTTP pipeline rejects numeric values.

### Cascade pattern under fine-grained writes

Cascades that previously inspected the in-memory object now read directly:

```typescript
// After changing process status, check if cascade is needed
const dirty = await db.queryMany(
  'SELECT process FROM cascade_dirty_flags WHERE plan_id=? AND destination=?',
  [planId, dest]
);
// then write cascade effects as targeted UPDATEs
```

### Migration path

The external contracts are already stable — `dispatch(Command)` on the CLI side, normalized Turso tables on the DB side, and the dashboard reads Turso directly without going through StateManager. Changing the internals has zero impact on existing data or any web page.

Implementation approach:
1. Define `DbClient` interface (queryOne / queryMany / execute)
2. Implement `TursoDbClient` backed by the existing Turso HTTP pipeline
3. Rewrite each `StateManager` method as targeted reads + writes against `DbClient`
4. Remove `PlanRepository`, `plan-assembler.ts`, `syncNormalizedTables()`

### Testing

No mocks. Tests run against a real DB (local SQLite via `libsql` embedded, or a dedicated Turso test database). Mocks diverge from the real schema and give false confidence — a passing mock test is no guarantee the SQL is correct.

Test pattern:
- Seed a test plan into the real DB
- Call `dispatch(command)`
- Assert the DB row directly (`SELECT` the updated column)
