# Itinerary DAL Refactor Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make StateManager the sole DAL for itinerary operations — CLI commands never reference table names; add `noon` session type; rename tables to domain-appropriate names; fill missing CLI commands.

**Architecture:**
- Every itinerary mutation flows through `sm.dispatch()`, which calls `TursoRepository` fine-grained SQL (ADR-001).
- `SessionType` gains `noon`; table names become `days` (was `itinerary_days`) and `timesofday` (was `itinerary_sessions`).
- CLI renames `set-session-*` → `set-tod-*` (time-of-day); adds `delete-activity` command.

**Tech Stack:** TypeScript, LibSQL/Turso HTTP pipeline, Cloudflare Worker (SSR dashboard), tsx/ts-node CLI.

---

## Context & Key Files

| File | Role |
|------|------|
| `src/state/types.ts:142` | `SessionType` union — add `noon` here |
| `src/state/commands.ts` | All command types — already has `remove_activity`, `set_session_focus`, `set_session_zh_content` |
| `src/state/state-manager.ts:370` | `dispatch()` — already handles all itinerary commands |
| `src/state/plan-repository.ts` | 1216-line file; syncNormalizedTables() lives here — write SQL updated for new table names |
| `src/state/turso-repository.ts` | Read-path SQL queries — update table names |
| `src/cli/travel-update.ts` | CLI — add `delete-activity`; rename `set-session-*`→`set-tod-*`; add noon to valid session lists |
| `scripts/migrate-itinerary-tables.sql` | Existing migration DDL — update CHECK constraints + table names |
| `scripts/schema.sql` | Reference DDL — keep in sync |
| `workers/trip-dashboard/src/turso.ts:131` | Dashboard read queries — update table names + noon |
| `workers/trip-dashboard/src/render.ts` | Dashboard render — add noon slot rendering |

---

## Phase A — Add `noon` Session Type

### Task A1: Extend SessionType

**Files:**
- Modify: `src/state/types.ts:142`

**Step 1: Edit the type**

```typescript
// Before
export type SessionType = 'morning' | 'afternoon' | 'evening';

// After
export type SessionType = 'morning' | 'noon' | 'afternoon' | 'evening';
```

**Step 2: Run typecheck**

```bash
make check 2>&1 | head -60
```
Expected: errors in CLI and plan-repository (they hardcode the 3-session arrays — fix in next tasks).

**Step 3: Commit**

```bash
git add src/state/types.ts
git commit -m "feat: add noon to SessionType union"
```

---

### Task A2: Update CLI session validation arrays

**Files:**
- Modify: `src/cli/travel-update.ts`

**Step 1: Find all hardcoded session arrays**

```bash
grep -n "'morning'.*'afternoon'.*'evening'\|validSessions.*=\|sessionOrder.*=" src/cli/travel-update.ts
```

**Step 2: Replace all occurrences**

There are ~8 places that hardcode `['morning', 'afternoon', 'evening']`.
Replace each with `['morning', 'noon', 'afternoon', 'evening']`.

Also update `sessionOrder` map:
```typescript
// Before
const sessionOrder = { morning: 0, afternoon: 1, evening: 2 } as const;
// After
const sessionOrder = { morning: 0, noon: 1, afternoon: 2, evening: 3 } as const;
```

Update help text in the HELP string: all mentions of `morning | afternoon | evening` → `morning | noon | afternoon | evening`.

Update `getSessionOrderForDayType` (line 3751):
```typescript
function getSessionOrderForDayType(dayType: string): Array<SessionType> {
  if (dayType === 'arrival') return ['afternoon', 'evening'];
  if (dayType === 'departure') return ['morning', 'noon'];
  return ['morning', 'noon', 'afternoon', 'evening'];
}
```

**Step 3: Typecheck**

```bash
make check 2>&1 | grep 'noon\|session' | head -20
```

**Step 4: Commit**

```bash
git add src/cli/travel-update.ts
git commit -m "feat: add noon to CLI session validation and ordering"
```

---

### Task A3: Update plan-repository.ts for noon

**Files:**
- Modify: `src/state/plan-repository.ts`

**Step 1: Find session loops**

```bash
grep -n "morning.*afternoon.*evening\|'morning'\|'afternoon'\|'evening'" src/state/plan-repository.ts | head -30
```

**Step 2: Replace all 3-session arrays with 4-session arrays**

Any `['morning', 'afternoon', 'evening']` → `['morning', 'noon', 'afternoon', 'evening']`.
Any CHECK constraint strings for session_type in INSERT SQL → add `'noon'`.

**Step 3: Typecheck**

```bash
make check 2>&1 | head -30
```
Expected: clean.

**Step 4: Commit**

```bash
git add src/state/plan-repository.ts
git commit -m "feat: add noon to plan-repository session handling"
```

---

### Task A4: DB migration — add noon to CHECK constraints

LibSQL doesn't support `ALTER COLUMN`. The migration must recreate the two tables that have `session_type` CHECK constraints: `itinerary_sessions` and `activities`.

**Files:**
- Create: `scripts/migrate-add-noon.ts`

**Step 1: Write migration script**

```typescript
#!/usr/bin/env tsx
import * as fs from 'fs';
const envFile = '/home/yanggf/b/travel-2026/.env';
for (const line of fs.readFileSync(envFile, 'utf8').split('\n')) {
  const m = line.match(/^([^#=]+)=(.*)$/);
  if (m) process.env[m[1].trim()] = m[2].trim();
}
const TURSO_URL = process.env.TURSO_URL!.replace('libsql://', 'https://');
const TURSO_TOKEN = process.env.TURSO_TOKEN!;

const requests = [
  // 1. Rename old tables (LibSQL supports ALTER TABLE RENAME TO)
  { type: 'execute', stmt: { sql: 'ALTER TABLE itinerary_sessions RENAME TO itinerary_sessions_old' } },
  { type: 'execute', stmt: { sql: 'ALTER TABLE activities RENAME TO activities_old' } },
  // 2. Create new itinerary_sessions with noon in CHECK
  { type: 'execute', stmt: { sql: `CREATE TABLE itinerary_sessions (
    plan_id TEXT NOT NULL,
    destination TEXT NOT NULL,
    day_number INTEGER NOT NULL,
    session_type TEXT NOT NULL CHECK(session_type IN ('morning','noon','afternoon','evening')),
    focus TEXT,
    transit_notes TEXT,
    booking_notes TEXT,
    meals_json TEXT,
    time_range_start TEXT,
    time_range_end TEXT,
    focus_zh TEXT,
    transit_notes_zh TEXT,
    meals_zh_json TEXT,
    activities_zh_json TEXT,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (plan_id, destination, day_number, session_type)
  )` } },
  // 3. Copy sessions data
  { type: 'execute', stmt: { sql: 'INSERT INTO itinerary_sessions SELECT * FROM itinerary_sessions_old' } },
  // 4. Create new activities with noon in CHECK
  { type: 'execute', stmt: { sql: `CREATE TABLE activities (
    id TEXT PRIMARY KEY,
    plan_id TEXT NOT NULL,
    destination TEXT NOT NULL,
    day_number INTEGER NOT NULL,
    session_type TEXT NOT NULL CHECK(session_type IN ('morning','noon','afternoon','evening')),
    sort_order INTEGER NOT NULL DEFAULT 0,
    title TEXT NOT NULL,
    area TEXT,
    nearest_station TEXT,
    duration_min INTEGER,
    booking_required INTEGER NOT NULL DEFAULT 0,
    booking_url TEXT,
    booking_status TEXT CHECK(booking_status IN ('not_required','pending','booked','waitlist')),
    booking_ref TEXT,
    book_by TEXT,
    start_time TEXT,
    end_time TEXT,
    is_fixed_time INTEGER NOT NULL DEFAULT 0,
    cost_estimate INTEGER,
    tags_json TEXT,
    notes TEXT,
    priority TEXT NOT NULL DEFAULT 'want' CHECK(priority IN ('must','want','optional')),
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
  )` } },
  // 5. Copy activities data
  { type: 'execute', stmt: { sql: 'INSERT INTO activities SELECT * FROM activities_old' } },
  // 6. Drop old tables
  { type: 'execute', stmt: { sql: 'DROP TABLE activities_old' } },
  { type: 'execute', stmt: { sql: 'DROP TABLE itinerary_sessions_old' } },
  // 7. Recreate indexes
  { type: 'execute', stmt: { sql: 'CREATE INDEX IF NOT EXISTS idx_activities_session ON activities(plan_id, destination, day_number, session_type, sort_order)' } },
  { type: 'execute', stmt: { sql: 'CREATE INDEX IF NOT EXISTS idx_activities_booking ON activities(plan_id, booking_status)' } },
  { type: 'close' },
];

async function run() {
  const res = await fetch(`${TURSO_URL}/v2/pipeline`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${TURSO_TOKEN}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ requests }),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}: ${await res.text()}`);
  const json = await res.json() as any;
  const labels = [
    'rename sessions→old', 'rename activities→old',
    'create itinerary_sessions', 'copy sessions',
    'create activities', 'copy activities',
    'drop activities_old', 'drop sessions_old',
    'idx_activities_session', 'idx_activities_booking',
  ];
  json.results?.slice(0, -1).forEach((r: any, i: number) => {
    const error = r?.response?.error;
    if (error) console.error(`❌ [${labels[i]}]: ${JSON.stringify(error)}`);
    else console.log(`✅ [${labels[i]}]`);
  });
}
run().catch(e => { console.error(e); process.exit(1); });
```

**Step 2: Run migration**

```bash
npx tsx scripts/migrate-add-noon.ts
```
Expected: 10 ✅ lines.

**Step 3: Verify**

```bash
npx tsx scripts/turso-exec.ts "SELECT session_type, count(*) FROM itinerary_sessions GROUP BY session_type"
```
Expected: rows for morning/afternoon/evening (no noon rows yet — that's fine).

**Step 4: Commit**

```bash
git add scripts/migrate-add-noon.ts
git commit -m "feat: DB migration — add noon to session_type CHECK constraint"
```

---

### Task A5: Dashboard — add noon to query and render

**Files:**
- Modify: `workers/trip-dashboard/src/turso.ts:358`
- Modify: `workers/trip-dashboard/src/render.ts`

**Step 1: Update turso.ts session loop**

```typescript
// Before (line ~358)
for (const sessionType of ['morning', 'afternoon', 'evening']) {
// After
for (const sessionType of ['morning', 'noon', 'afternoon', 'evening']) {
```

Also update `updateSessionField` validator (line ~631):
```typescript
// Before
if (!['morning', 'afternoon', 'evening'].includes(sessionType)) {
// After
if (!['morning', 'noon', 'afternoon', 'evening'].includes(sessionType)) {
```

**Step 2: Update render.ts — add noon slot**

Find the session rendering loop in `render.ts`. The pattern renders session cards in order. After the existing morning/afternoon/evening handling, noon should render between morning and afternoon.

Search for where session cards are rendered:
```bash
grep -n "morning\|afternoon\|evening\|session" workers/trip-dashboard/src/render.ts | head -40
```

Add noon to the rendered session order wherever morning/afternoon/evening are listed.

**Step 3: Local test**

```bash
cd workers/trip-dashboard
unset CLOUDFLARE_API_TOKEN && npx wrangler dev
```
Open http://localhost:8787/?plan=kyoto-2026 — should render without errors (noon slot appears empty, which is correct since no noon data yet).

**Step 4: Commit + deploy**

```bash
git add workers/trip-dashboard/src/turso.ts workers/trip-dashboard/src/render.ts
git commit -m "feat: dashboard — add noon session slot"
cd workers/trip-dashboard && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy
```

---

## Phase B — Add Missing CLI Commands

### Task B1: Add `delete-activity` CLI command

`remove_activity` command already exists in `commands.ts` and `state-manager.ts`. Only the CLI case is missing.

**Files:**
- Modify: `src/cli/travel-update.ts`

**Step 1: Find where `set-activity-booking` case is (reference pattern)**

```bash
grep -n "case 'set-activity-booking'" src/cli/travel-update.ts
```

**Step 2: Add `delete-activity` case** (after or near `remove-activity` alias)

```typescript
case 'delete-activity':
case 'remove-activity': {
  // delete-activity <day> <session> <activity_id_or_title> [--plan-id <id>]
  const dayNum = parseInt(args[1], 10);
  const sessionArg = args[2];
  const activityArg = args[3];
  if (!dayNum || !sessionArg || !activityArg) {
    console.error('Usage: delete-activity <day> <session> <activity_id_or_title>');
    console.error('Example: delete-activity 2 morning "teamLab Borderless"');
    process.exit(1);
  }
  const validSessions = ['morning', 'noon', 'afternoon', 'evening'];
  if (!validSessions.includes(sessionArg)) {
    console.error(`Error: session must be one of: ${validSessions.join(' | ')}`);
    process.exit(1);
  }
  // Find activity ID by title substring
  const plan = sm.getPlan();
  const dest = planOpts.dest || sm.getActiveDestination();
  const destData = plan.destinations[dest];
  const dayData = destData?.process_5_daily_itinerary?.days?.find(
    (d: any) => d.day_number === dayNum
  );
  const session = dayData?.[sessionArg];
  const activities: any[] = session?.activities ?? [];
  const match = activities.find(
    (a: any) => a.id === activityArg || a.title.toLowerCase().includes(activityArg.toLowerCase())
  );
  if (!match) {
    console.error(`Activity not found: "${activityArg}" in Day ${dayNum} ${sessionArg}`);
    console.error('Activities in this session:');
    activities.forEach((a: any) => console.error(`  [${a.id}] ${a.title}`));
    process.exit(1);
  }
  await sm.dispatch({
    type: 'remove_activity',
    destination: dest,
    dayNumber: dayNum,
    session: sessionArg as SessionType,
    activityId: match.id,
  });
  await sm.saveWithTracking('delete-activity', `D${dayNum}/${sessionArg}/${match.title}`);
  console.log(`✅ Deleted activity: "${match.title}" (D${dayNum} ${sessionArg})`);
  break;
}
```

**Step 3: Add to HELP string**

```
  delete-activity <day> <session> <activity>
    Remove an activity from a session. activity: ID or title substring.
    Example: delete-activity 2 morning "teamLab Borderless"
```

**Step 4: Typecheck**

```bash
make check 2>&1 | head -20
```
Expected: clean.

**Step 5: Test**

```bash
# First view current activities
./bin/travel itinerary --dest kyoto_2026
# Find a test activity and verify delete works (use --dry-run if available, otherwise verify via view after)
```

**Step 6: Commit**

```bash
git add src/cli/travel-update.ts
git commit -m "feat: add delete-activity CLI command"
```

---

### Task B2: Add `set-session-focus` CLI command (EN focus only)

Currently `set-session-zh` covers ZH content. EN focus can only be set via the full session scaffold or a direct DB write. Add a simple CLI command.

**Files:**
- Modify: `src/cli/travel-update.ts`

**Step 1: Add case**

```typescript
case 'set-tod-focus':
case 'set-session-focus': {
  // set-tod-focus <day> <session> "<focus_text>" [--plan-id <id>]
  const dayNum = parseInt(args[1], 10);
  const sessionArg = args[2];
  const focusText = args[3];
  if (!dayNum || !sessionArg) {
    console.error('Usage: set-tod-focus <day> <session> "<focus_text>"');
    console.error('Example: set-tod-focus 2 morning "北野天滿宮 → 金閣寺"');
    process.exit(1);
  }
  const dest = planOpts.dest || sm.getActiveDestination();
  await sm.dispatch({
    type: 'set_session_focus',
    destination: dest,
    dayNumber: dayNum,
    session: sessionArg as SessionType,
    focus: focusText ?? null,
  });
  await sm.saveWithTracking('set-tod-focus', `D${dayNum}/${sessionArg}`);
  console.log(`✅ Focus set for D${dayNum} ${sessionArg}: "${focusText}"`);
  break;
}
```

**Step 2: Add to HELP string**

```
  set-tod-focus <day> <session> "<focus_text>" [--plan-id <id>]
    Set EN session focus summary (shown as subtitle under ZH focus).
    Example: set-tod-focus 2 morning "Kitano Tenmangu → Kinkaku-ji"
```

**Step 3: Typecheck + commit**

```bash
make check 2>&1 | head -10
git add src/cli/travel-update.ts
git commit -m "feat: add set-tod-focus CLI command"
```

---

### Task B3: Rename `set-session-zh` → `set-tod-zh` (with alias)

**Files:**
- Modify: `src/cli/travel-update.ts`

**Step 1: Find the case**

```bash
grep -n "case 'set-session-zh'" src/cli/travel-update.ts
```

**Step 2: Add alias**

```typescript
// Before
case 'set-session-zh': {
// After
case 'set-tod-zh':
case 'set-session-zh': {
```

Same for `set-session-time-range` → `set-tod-time-range`:
```typescript
case 'set-tod-time-range':
case 'set-session-time-range': {
```

**Step 3: Update HELP** — add `set-tod-zh` / `set-tod-time-range` as primary, keep `set-session-*` as deprecated alias note.

**Step 4: Typecheck + commit**

```bash
make check 2>&1 | head -10
git add src/cli/travel-update.ts
git commit -m "feat: rename set-session-* CLI commands to set-tod-* (with backward-compat aliases)"
```

---

## Phase C — DB Table Rename

### Task C1: Write table rename migration

LibSQL supports `ALTER TABLE RENAME TO` for simple renames. Existing foreign keys in `activities` will need to be aware — but since LibSQL doesn't enforce FK by default, this is safe.

**Files:**
- Create: `scripts/migrate-rename-itinerary-tables.ts`

**Step 1: Write script**

```typescript
#!/usr/bin/env tsx
import * as fs from 'fs';
const envFile = '/home/yanggf/b/travel-2026/.env';
for (const line of fs.readFileSync(envFile, 'utf8').split('\n')) {
  const m = line.match(/^([^#=]+)=(.*)$/);
  if (m) process.env[m[1].trim()] = m[2].trim();
}
const TURSO_URL = process.env.TURSO_URL!.replace('libsql://', 'https://');
const TURSO_TOKEN = process.env.TURSO_TOKEN!;

const requests = [
  { type: 'execute', stmt: { sql: 'ALTER TABLE itinerary_days RENAME TO days' } },
  { type: 'execute', stmt: { sql: 'ALTER TABLE itinerary_sessions RENAME TO timesofday' } },
  { type: 'close' },
];

async function run() {
  const res = await fetch(`${TURSO_URL}/v2/pipeline`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${TURSO_TOKEN}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ requests }),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}: ${await res.text()}`);
  const json = await res.json() as any;
  ['days rename', 'timesofday rename'].forEach((label, i) => {
    const error = json.results?.[i]?.response?.error;
    if (error) console.error(`❌ ${label}: ${JSON.stringify(error)}`);
    else console.log(`✅ ${label}`);
  });
}
run().catch(e => { console.error(e); process.exit(1); });
```

**⚠️ IMPORTANT: Do NOT run this until all code changes in Task C2 are ready in the same commit.**

**Step 2: Commit script (not yet run)**

```bash
git add scripts/migrate-rename-itinerary-tables.ts
git commit -m "feat: add table rename migration script (not yet run)"
```

---

### Task C2: Update all code references to renamed tables

Update all SQL strings that reference `itinerary_days` or `itinerary_sessions`.

**Files:**
- Modify: `src/state/turso-repository.ts`
- Modify: `src/state/plan-repository.ts`
- Modify: `workers/trip-dashboard/src/turso.ts`
- Modify: `scripts/schema.sql`
- Modify: `scripts/migrate-itinerary-tables.sql`

**Step 1: Find all occurrences**

```bash
grep -rn "itinerary_days\|itinerary_sessions" src/ scripts/ workers/ --include="*.ts" --include="*.sql"
```

**Step 2: Replace in each file**

For each file: `itinerary_days` → `days`, `itinerary_sessions` → `timesofday`.

In `src/state/plan-repository.ts`:
- UPDATE/INSERT/DELETE SQL: replace table names
- Any CREATE TABLE IF NOT EXISTS statements: update names

In `src/state/turso-repository.ts`:
- SELECT statements: update table names

In `workers/trip-dashboard/src/turso.ts`:
- Lines 131-132: `FROM itinerary_days` → `FROM days`, `FROM itinerary_sessions` → `FROM timesofday`
- `updateDayField` (line ~610): `UPDATE itinerary_days SET` → `UPDATE days SET`
- `updateSessionField` (line ~634): `UPDATE itinerary_sessions SET` → `UPDATE timesofday SET`

In `workers/trip-dashboard/src/index.ts`:
- EditRequest body validation (line 55-61): references `'itinerary_days' | 'itinerary_sessions'` — update to `'days' | 'timesofday'`

In `scripts/schema.sql` and `scripts/migrate-itinerary-tables.sql`:
- Update CREATE TABLE statements

**Step 3: Typecheck**

```bash
make check 2>&1 | head -30
```
Expected: clean.

**Step 4: Commit ALL changes**

```bash
git add src/state/turso-repository.ts src/state/plan-repository.ts
git add workers/trip-dashboard/src/turso.ts workers/trip-dashboard/src/index.ts workers/trip-dashboard/src/render.ts
git add scripts/schema.sql scripts/migrate-itinerary-tables.sql
git commit -m "feat: update all code references — itinerary_days→days, itinerary_sessions→timesofday"
```

---

### Task C3: Run table rename migration + verify + deploy

**Step 1: Run migration**

```bash
npx tsx scripts/migrate-rename-itinerary-tables.ts
```
Expected:
```
✅ days rename
✅ timesofday rename
```

**Step 2: Verify data intact**

```bash
npx tsx scripts/turso-exec.ts "SELECT count(*) FROM days WHERE plan_id='kyoto-2026'"
npx tsx scripts/turso-exec.ts "SELECT count(*) FROM timesofday WHERE plan_id='kyoto-2026'"
```
Expected: same row counts as before migration (5 days, 15 sessions for kyoto-2026).

**Step 3: Test CLI**

```bash
./bin/travel itinerary --plan-id kyoto-2026
```
Expected: same itinerary output as before.

**Step 4: Deploy dashboard**

```bash
cd workers/trip-dashboard && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy
```

**Step 5: Verify live dashboard**

Open https://trip-dashboard.yanggf.workers.dev/?plan=kyoto-2026 — content should be identical to before.

---

## Phase D — Update Docs

### Task D1: Update CLAUDE.md

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Update table name references**

In the Turso DB tables section of CLAUDE.md:
- `itinerary_days (+ weather + theme_zh)` → `days (+ weather + theme_zh)`
- `itinerary_sessions (+ focus_zh, ...)` → `timesofday (+ focus_zh, ...)`

In CLI Quick Reference:
- Add `delete-activity` to mutations section
- Add `set-tod-zh` as primary (with `set-session-zh` as alias note)
- Add `noon` to session type mentions

In Kyoto Itinerary table: ensure it's up to date (already done in previous sessions).

**Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md — renamed tables, new CLI commands, noon session type"
```

---

### Task D2: Update skill SKILL.md files

**Files:**
- Modify: `src/skills/p5-itinerary/SKILL.md`
- Modify: `src/skills/travel-shared/SKILL.md` (if it lists session types)

**Step 1: Find mentions**

```bash
grep -rn "morning.*afternoon.*evening\|itinerary_days\|itinerary_sessions" src/skills/ --include="*.md"
```

**Step 2: Update** — `morning | afternoon | evening` → `morning | noon | afternoon | evening`, table renames.

**Step 3: Commit**

```bash
git add src/skills/
git commit -m "docs: update skill SKILL.md files — noon session type, renamed tables"
```

---

## Execution Order

Run phases in order: **A → B → C → D**

Within Phase C, **Task C2 must be committed before C3 is run** — the code and DB must be in sync. If the code is deployed with `itinerary_days` still referencing the old table name while the DB has been renamed, dashboard breaks. The commit order is:
1. C1 (script, not run)
2. C2 (code updates, committed)
3. Run `npx tsx scripts/migrate-rename-itinerary-tables.ts`
4. Test + deploy (C3)

**If migration fails mid-way:** Turso HTTP pipeline executes statements sequentially. If `days rename` fails (table already renamed), check current table names with:
```bash
npx tsx scripts/turso-exec.ts "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"
```

---

## Rollback Plan

If dashboard breaks after C3:
1. Immediately rollback code deploy: `git revert HEAD && cd workers/trip-dashboard && npx wrangler deploy`
2. Rename tables back: run `ALTER TABLE days RENAME TO itinerary_days; ALTER TABLE timesofday RENAME TO itinerary_sessions;` via turso-exec

The table rename is the only irreversible step — everything else is code-only and can be reverted via git.
