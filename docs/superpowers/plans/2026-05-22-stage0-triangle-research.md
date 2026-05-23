# Stage 0 Triangle Research — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the missing Stage 0 "triangle research" capability — a DB-backed research-session domain that ranks flight candidates across destinations and durations *before* any plan exists.

**Architecture:** Six new unscoped Turso tables keyed by `run_id` (not `plan_id`). A Python aggregator wraps the existing `scrape_date_range.py` over the cross product of destinations × durations. A TypeScript service module owns all Stage 0 DB reads/writes. Three CLI commands (`stage0-init`, `stage0-run` is the Python script, `stage0-compare`, `stage0-adopt`) plus a `/stage0-research` orchestration skill expose the workflow. P1–P5 skills are untouched.

**Tech Stack:** TypeScript (CLI commands, service layer, tests via Vitest), Python (aggregator script, consistent with existing scrapers), Turso (`TursoPipelineClient` raw-SQL pipeline).

**Spec:** `docs/superpowers/specs/2026-05-22-stage0-triangle-research-design.md`

---

## Key Codebase Conventions (read before starting)

- **Turso access:** `new TursoPipelineClient()` from `scripts/turso-pipeline.ts`. `.execute(sql)` runs one statement; `.executeBatch(sqls)` runs a read batch (`response.results[i]`); `.executeMany(sqls)` runs writes in chunks.
- **No parameter binding.** `TursoPipelineClient.execute()` takes a raw SQL string. **Always** build SQL values with the helpers in `src/state/sql-helpers.ts`: `sqlText()` (quotes + escapes strings — required for any user text), `sqlInt()`, `sqlReal()`. Read rows back with `rowsToObjects(response)` or `rowsToObjectsAt(response, i)`.
- **CLI command pattern:** each command is a `CommandHandler` object (`src/cli/shared/types.ts`) registered via `registerCommand()` and imported in `src/cli/travel-update.ts`. Set `requiresState: false` for pre-plan commands so they skip plan resolution.
- **Migrations:** `scripts/turso-migrate.ts` — append `CREATE TABLE IF NOT EXISTS` blocks; idempotent. Mirror DDL into `scripts/schema.sql` (read-only reference).
- **Tests:** integration-only, `tests/integration/*.test.ts`, Vitest, real DB, no mocks. Pattern: `describe`/`it` with seed → act → assert → teardown.
- **`run_id` format:** `stage0-YYYYMMDD-HHMMSS` (e.g., `stage0-20260522-143000`).

---

## File Structure

**Create:**
- `src/services/stage0-service.ts` — all Stage 0 DB reads/writes + ranking; ~6 functions, one responsibility (Stage 0 persistence).
- `src/cli/commands/stage0.ts` — the `stage0-init`, `stage0-compare`, `stage0-adopt` command handlers.
- `scripts/stage0_research.py` — the aggregator (wraps `scrape_date_range.py`).
- `src/skills/stage0-research/SKILL.md` — orchestration skill.
- `tests/integration/stage0-service.regression.test.ts` — ranking + adopt + scrape-attempt tests.

**Modify:**
- `scripts/turso-migrate.ts` — add 6 `CREATE TABLE IF NOT EXISTS` blocks.
- `scripts/schema.sql` — mirror the 6 tables.
- `src/cli/shared/args.ts` — add `--run` to `OPTIONS_WITH_VALUES`.
- `src/cli/travel-update.ts` — add `import './commands/stage0';`.
- `docs/plans/2026-05-22-new-planning-flow.md` — update Stage 0 Skill Mapping row to name `/stage0-research`.
- `CLAUDE.md` — add `/stage0-research` to the skills table; add Stage 0 tables to the Turso DB section.

---

## Task 1: Database migration — six Stage 0 tables

**Files:**
- Modify: `scripts/turso-migrate.ts`
- Modify: `scripts/schema.sql`

- [ ] **Step 1: Add the six table-creation blocks to the migration**

In `scripts/turso-migrate.ts`, inside the `main()` `try` block, after the last existing `CREATE TABLE` block, add:

```typescript
    // ── Stage 0 — Triangle Research (unscoped: keyed by run_id, not plan_id) ──
    console.log('Creating Stage 0 research tables...');
    await client.executeMany([
      `CREATE TABLE IF NOT EXISTS stage0_research_runs (
        run_id TEXT PRIMARY KEY,
        origin_code TEXT NOT NULL,
        pax INTEGER NOT NULL,
        window_start TEXT NOT NULL,
        window_end TEXT NOT NULL,
        currency TEXT NOT NULL,
        exchange_rate_usd_twd REAL NOT NULL,
        status TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
      );`,
      `CREATE TABLE IF NOT EXISTS stage0_research_destinations (
        run_id TEXT NOT NULL,
        dest_code TEXT NOT NULL,
        dest_label TEXT NOT NULL,
        sort_order INTEGER NOT NULL,
        PRIMARY KEY (run_id, dest_code)
      );`,
      `CREATE TABLE IF NOT EXISTS stage0_research_durations (
        run_id TEXT NOT NULL,
        nights INTEGER NOT NULL,
        duration_days INTEGER NOT NULL,
        PRIMARY KEY (run_id, nights)
      );`,
      `CREATE TABLE IF NOT EXISTS stage0_candidates (
        candidate_id TEXT PRIMARY KEY,
        run_id TEXT NOT NULL,
        dest_code TEXT NOT NULL,
        depart_date TEXT NOT NULL,
        return_date TEXT NOT NULL,
        nights INTEGER NOT NULL,
        flight_total_twd INTEGER,
        leave_days INTEGER,
        rank INTEGER,
        verdict TEXT,
        adopted_plan_id TEXT
      );`,
      `CREATE TABLE IF NOT EXISTS stage0_candidate_flights (
        candidate_id TEXT NOT NULL,
        direction TEXT NOT NULL,
        airline TEXT,
        depart_time TEXT,
        arrive_time TEXT,
        duration TEXT,
        nonstop INTEGER,
        price_total_twd INTEGER,
        PRIMARY KEY (candidate_id, direction)
      );`,
      `CREATE TABLE IF NOT EXISTS stage0_scrape_attempts (
        run_id TEXT NOT NULL,
        dest_code TEXT NOT NULL,
        nights INTEGER NOT NULL,
        status TEXT NOT NULL,
        candidate_count INTEGER,
        error TEXT,
        attempted_at TEXT,
        PRIMARY KEY (run_id, dest_code, nights)
      );`,
    ]);
    await client.execute('CREATE INDEX IF NOT EXISTS idx_s0_cand_run ON stage0_candidates(run_id, rank);');
    console.log('✅ Stage 0 research tables ready.');
```

- [ ] **Step 2: Run the migration**

Run: `npm run db:migrate:turso`
Expected: output includes `✅ Stage 0 research tables ready.` and exits 0.

- [ ] **Step 3: Verify tables exist**

Run: `npm run db:exec -- "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'stage0_%' ORDER BY name;"`
Expected: lists all six — `stage0_candidate_flights`, `stage0_candidates`, `stage0_research_destinations`, `stage0_research_durations`, `stage0_research_runs`, `stage0_scrape_attempts`.

- [ ] **Step 4: Verify idempotency**

Run: `npm run db:migrate:turso`
Expected: re-runs cleanly, exits 0, no error (the `IF NOT EXISTS` clauses make it a no-op).

- [ ] **Step 5: Mirror DDL into schema.sql**

Append the same six `CREATE TABLE` statements (plain SQL, no TypeScript wrapper) and the index to `scripts/schema.sql`, under a `-- ── Stage 0 — Triangle Research ──` comment header, matching the file's existing style.

- [ ] **Step 6: Commit**

```bash
git add scripts/turso-migrate.ts scripts/schema.sql
git commit -m "feat: add Stage 0 research tables migration"
```

---

## Task 2: Stage 0 service — types and run creation

**Files:**
- Create: `src/services/stage0-service.ts`
- Test: `tests/integration/stage0-service.regression.test.ts`

- [ ] **Step 1: Write the failing test for run creation**

Create `tests/integration/stage0-service.regression.test.ts`:

```typescript
import { describe, expect, it, afterAll } from 'vitest';
import {
  createResearchRun,
  getResearchRun,
  getScrapeAttempts,
  deleteResearchRun,
  type CreateRunInput,
} from '../../src/services/stage0-service';

const TEST_RUN_IDS: string[] = [];

function uniqueRunId(): string {
  const id = `stage0-test-${Date.now()}-${Math.floor(Math.random() * 10000)}`;
  TEST_RUN_IDS.push(id);
  return id;
}

afterAll(async () => {
  for (const id of TEST_RUN_IDS) {
    await deleteResearchRun(id);
  }
});

describe('Stage 0 service — run creation', () => {
  it('creates a run with destinations and durations, reads it back', async () => {
    const runId = uniqueRunId();
    const input: CreateRunInput = {
      runId,
      originCode: 'TPE',
      pax: 2,
      windowStart: '2026-06-18',
      windowEnd: '2026-06-20',
      exchangeRateUsdTwd: 32.0,
      destinations: [
        { destCode: 'KIX', destLabel: 'Osaka/Kyoto (KIX)' },
        { destCode: 'NRT', destLabel: 'Tokyo (NRT)' },
      ],
      durations: [{ nights: 6 }, { nights: 7 }],
    };
    await createResearchRun(input);

    const run = await getResearchRun(runId);
    expect(run).not.toBeNull();
    expect(run!.origin_code).toBe('TPE');
    expect(run!.currency).toBe('TWD');
    expect(run!.status).toBe('started');
    expect(run!.destinations).toHaveLength(2);
    expect(run!.durations).toHaveLength(2);
    // duration_days = nights + 1
    expect(run!.durations.find((d) => d.nights === 6)!.duration_days).toBe(7);
  });

  it('seeds a pending scrape-attempt row per destination x duration', async () => {
    const runId = uniqueRunId();
    await createResearchRun({
      runId, originCode: 'TPE', pax: 2,
      windowStart: '2026-06-18', windowEnd: '2026-06-20', exchangeRateUsdTwd: 32,
      destinations: [
        { destCode: 'KIX', destLabel: 'Osaka (KIX)' },
        { destCode: 'NRT', destLabel: 'Tokyo (NRT)' },
      ],
      durations: [{ nights: 6 }, { nights: 7 }],
    });
    const attempts = await getScrapeAttempts(runId);
    // 2 destinations x 2 durations = 4 pending rows
    expect(attempts).toHaveLength(4);
    expect(attempts.every((a) => a.status === 'pending')).toBe(true);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npm test -- stage0-service`
Expected: FAIL — `Cannot find module '../../src/services/stage0-service'`.

- [ ] **Step 3: Create the service with types and run creation**

Create `src/services/stage0-service.ts`:

```typescript
/**
 * Stage 0 Service — all DB reads/writes for the triangle-research domain.
 *
 * Stage 0 tables are unscoped (keyed by run_id, not plan_id) — research
 * exists before any plan exists. Runs are IMMUTABLE: research inputs are
 * written once at creation and never edited; changing an input means a new
 * run. See docs/superpowers/specs/2026-05-22-stage0-triangle-research-design.md
 */

import { TursoPipelineClient } from '../../scripts/turso-pipeline';
import { sqlText, sqlInt, sqlReal, rowsToObjects, rowsToObjectsAt } from '../state/sql-helpers';

// ── types ────────────────────────────────────────────────────────────

export interface CreateRunInput {
  runId: string;
  originCode: string;
  pax: number;
  windowStart: string;
  windowEnd: string;
  exchangeRateUsdTwd: number;
  destinations: Array<{ destCode: string; destLabel: string }>;
  durations: Array<{ nights: number }>;
}

export interface ResearchRun {
  run_id: string;
  origin_code: string;
  pax: number;
  window_start: string;
  window_end: string;
  currency: string;
  exchange_rate_usd_twd: number;
  status: string;
  created_at: string;
  updated_at: string;
  destinations: Array<{ dest_code: string; dest_label: string; sort_order: number }>;
  durations: Array<{ nights: number; duration_days: number }>;
}

function nowIso(): string {
  return new Date().toISOString();
}

// ── run creation ─────────────────────────────────────────────────────

export async function createResearchRun(input: CreateRunInput): Promise<void> {
  const client = new TursoPipelineClient();
  const ts = nowIso();
  const stmts: string[] = [];

  stmts.push(
    `INSERT INTO stage0_research_runs
      (run_id, origin_code, pax, window_start, window_end, currency,
       exchange_rate_usd_twd, status, created_at, updated_at)
     VALUES (${sqlText(input.runId)}, ${sqlText(input.originCode)}, ${sqlInt(input.pax)},
       ${sqlText(input.windowStart)}, ${sqlText(input.windowEnd)}, ${sqlText('TWD')},
       ${sqlReal(input.exchangeRateUsdTwd)}, ${sqlText('started')}, ${sqlText(ts)}, ${sqlText(ts)});`
  );

  input.destinations.forEach((d, i) => {
    stmts.push(
      `INSERT INTO stage0_research_destinations (run_id, dest_code, dest_label, sort_order)
       VALUES (${sqlText(input.runId)}, ${sqlText(d.destCode)}, ${sqlText(d.destLabel)}, ${sqlInt(i)});`
    );
  });

  for (const dur of input.durations) {
    stmts.push(
      `INSERT INTO stage0_research_durations (run_id, nights, duration_days)
       VALUES (${sqlText(input.runId)}, ${sqlInt(dur.nights)}, ${sqlInt(dur.nights + 1)});`
    );
  }

  // Seed one 'pending' scrape-attempt row per destination x duration. This
  // makes the full work matrix visible in the DB before scraping starts, so
  // a mid-run crash still shows what was attempted, and the aggregator can
  // skip already-'ok' pairs on a re-run.
  for (const d of input.destinations) {
    for (const dur of input.durations) {
      stmts.push(
        `INSERT INTO stage0_scrape_attempts
          (run_id, dest_code, nights, status, candidate_count, error, attempted_at)
         VALUES (${sqlText(input.runId)}, ${sqlText(d.destCode)}, ${sqlInt(dur.nights)},
           ${sqlText('pending')}, NULL, NULL, NULL);`
      );
    }
  }

  await client.executeMany(stmts);
}

// ── run read ─────────────────────────────────────────────────────────

export async function getResearchRun(runId: string): Promise<ResearchRun | null> {
  const client = new TursoPipelineClient();
  const res = await client.executeBatch([
    `SELECT * FROM stage0_research_runs WHERE run_id = ${sqlText(runId)};`,
    `SELECT * FROM stage0_research_destinations WHERE run_id = ${sqlText(runId)} ORDER BY sort_order;`,
    `SELECT * FROM stage0_research_durations WHERE run_id = ${sqlText(runId)} ORDER BY nights;`,
  ]);
  const runRows = rowsToObjectsAt(res, 0);
  if (runRows.length === 0) return null;
  const run = runRows[0];
  return {
    run_id: run.run_id,
    origin_code: run.origin_code,
    pax: Number(run.pax),
    window_start: run.window_start,
    window_end: run.window_end,
    currency: run.currency,
    exchange_rate_usd_twd: Number(run.exchange_rate_usd_twd),
    status: run.status,
    created_at: run.created_at,
    updated_at: run.updated_at,
    destinations: rowsToObjectsAt(res, 1).map((d) => ({
      dest_code: d.dest_code,
      dest_label: d.dest_label,
      sort_order: Number(d.sort_order),
    })),
    durations: rowsToObjectsAt(res, 2).map((d) => ({
      nights: Number(d.nights),
      duration_days: Number(d.duration_days),
    })),
  };
}

// ── scrape attempts (read) ───────────────────────────────────────────
// Attempt rows are seeded as 'pending' by createResearchRun above; the
// aggregator/import flow updates them to 'ok'/'failed'. Write helper
// (upsertScrapeAttempt) is added in Task 3.

export interface ScrapeAttempt {
  run_id: string;
  dest_code: string;
  nights: number;
  status: string;
  candidate_count: number | null;
  error: string | null;
  attempted_at: string | null;
}

export async function getScrapeAttempts(runId: string): Promise<ScrapeAttempt[]> {
  const client = new TursoPipelineClient();
  const res = await client.execute(
    `SELECT * FROM stage0_scrape_attempts WHERE run_id = ${sqlText(runId)}
     ORDER BY dest_code, nights;`
  );
  return rowsToObjects(res).map((a) => ({
    run_id: a.run_id,
    dest_code: a.dest_code,
    nights: Number(a.nights),
    status: a.status,
    candidate_count: a.candidate_count == null ? null : Number(a.candidate_count),
    error: a.error ?? null,
    attempted_at: a.attempted_at ?? null,
  }));
}

// ── teardown helper (used by tests) ──────────────────────────────────

export async function deleteResearchRun(runId: string): Promise<void> {
  const client = new TursoPipelineClient();
  const r = sqlText(runId);
  await client.executeMany([
    `DELETE FROM stage0_candidate_flights WHERE candidate_id IN
      (SELECT candidate_id FROM stage0_candidates WHERE run_id = ${r});`,
    `DELETE FROM stage0_candidates WHERE run_id = ${r};`,
    `DELETE FROM stage0_scrape_attempts WHERE run_id = ${r};`,
    `DELETE FROM stage0_research_durations WHERE run_id = ${r};`,
    `DELETE FROM stage0_research_destinations WHERE run_id = ${r};`,
    `DELETE FROM stage0_research_runs WHERE run_id = ${r};`,
  ]);
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `npm test -- stage0-service`
Expected: PASS — 2 tests (run creation + pending scrape-attempt seeding).

- [ ] **Step 5: Commit**

```bash
git add src/services/stage0-service.ts tests/integration/stage0-service.regression.test.ts
git commit -m "feat: add Stage 0 service with run creation"
```

---

## Task 3: Stage 0 service — candidates, scrape attempts, ranking

**Files:**
- Modify: `src/services/stage0-service.ts`
- Modify: `tests/integration/stage0-service.regression.test.ts`

- [ ] **Step 1: Write the failing test for candidates + ranking**

First, extend the **existing import block at the top of the file** (from Task 2) — add the
new names to it rather than writing a second `import` from the same module (a duplicate
import is a TS error):

```typescript
import { describe, expect, it, afterAll } from 'vitest';
import {
  createResearchRun,
  getResearchRun,
  getScrapeAttempts,
  deleteResearchRun,
  insertCandidate,
  upsertScrapeAttempt,
  rankRun,
  getCandidates,
  setRunStatus,
  adoptCandidate,
  deleteCandidatesForPair,
  type CreateRunInput,
} from '../../src/services/stage0-service';
```

Then append the new `describe` block to the end of the file:

```typescript
describe('Stage 0 service — candidates and ranking', () => {
  it('ranks candidates by price, then leave-days, then depart-date', async () => {
    const runId = uniqueRunId();
    await createResearchRun({
      runId, originCode: 'TPE', pax: 2,
      windowStart: '2026-06-18', windowEnd: '2026-06-20', exchangeRateUsdTwd: 32,
      destinations: [{ destCode: 'KIX', destLabel: 'Osaka (KIX)' }],
      durations: [{ nights: 6 }],
    });

    // Two candidates same price → leave-days breaks the tie.
    await insertCandidate({
      candidateId: `${runId}-KIX-2026-06-19-6n`, runId, destCode: 'KIX',
      departDate: '2026-06-19', returnDate: '2026-06-25', nights: 6,
      flightTotalTwd: 18000, leaveDays: 4, verdict: null, flights: [],
    });
    await insertCandidate({
      candidateId: `${runId}-KIX-2026-06-18-6n`, runId, destCode: 'KIX',
      departDate: '2026-06-18', returnDate: '2026-06-24', nights: 6,
      flightTotalTwd: 18000, leaveDays: 3, verdict: null, flights: [],
    });
    // Cheaper candidate must rank first regardless of leave-days.
    await insertCandidate({
      candidateId: `${runId}-KIX-2026-06-20-6n`, runId, destCode: 'KIX',
      departDate: '2026-06-20', returnDate: '2026-06-26', nights: 6,
      flightTotalTwd: 15000, leaveDays: 5, verdict: null, flights: [],
    });

    await rankRun(runId);
    const cands = await getCandidates(runId);

    expect(cands.map((c) => c.candidate_id)).toEqual([
      `${runId}-KIX-2026-06-20-6n`, // cheapest
      `${runId}-KIX-2026-06-18-6n`, // tie 18000, fewer leave days
      `${runId}-KIX-2026-06-19-6n`, // tie 18000, more leave days
    ]);
    expect(cands.map((c) => c.rank)).toEqual([1, 2, 3]);
  });

  it('records scrape attempts and reads them back', async () => {
    const runId = uniqueRunId();
    await createResearchRun({
      runId, originCode: 'TPE', pax: 2,
      windowStart: '2026-06-18', windowEnd: '2026-06-20', exchangeRateUsdTwd: 32,
      destinations: [{ destCode: 'KIX', destLabel: 'Osaka (KIX)' }],
      durations: [{ nights: 6 }],
    });
    await upsertScrapeAttempt({ runId, destCode: 'KIX', nights: 6, status: 'pending' });
    await upsertScrapeAttempt({ runId, destCode: 'KIX', nights: 6, status: 'failed', error: 'timeout' });

    const run = await getResearchRun(runId);
    expect(run).not.toBeNull();
    const attempts = await getScrapeAttempts(runId);
    expect(attempts).toHaveLength(1);
    expect(attempts[0].status).toBe('failed');
    expect(attempts[0].error).toBe('timeout');
  });

  it('adopts a candidate — sets adopted_plan_id and run status', async () => {
    const runId = uniqueRunId();
    await createResearchRun({
      runId, originCode: 'TPE', pax: 2,
      windowStart: '2026-06-18', windowEnd: '2026-06-20', exchangeRateUsdTwd: 32,
      destinations: [{ destCode: 'KIX', destLabel: 'Osaka (KIX)' }],
      durations: [{ nights: 6 }],
    });
    const candidateId = `${runId}-KIX-2026-06-18-6n`;
    await insertCandidate({
      candidateId, runId, destCode: 'KIX',
      departDate: '2026-06-18', returnDate: '2026-06-24', nights: 6,
      flightTotalTwd: 18000, leaveDays: 3, verdict: null, flights: [],
    });
    await adoptCandidate(candidateId, 'osaka-2026');

    const cands = await getCandidates(runId);
    expect(cands[0].adopted_plan_id).toBe('osaka-2026');
    const run = await getResearchRun(runId);
    expect(run!.status).toBe('adopted');
  });

  it('deleteCandidatesForPair clears prior candidates for one pair', async () => {
    const runId = uniqueRunId();
    await createResearchRun({
      runId, originCode: 'TPE', pax: 2,
      windowStart: '2026-06-18', windowEnd: '2026-06-20', exchangeRateUsdTwd: 32,
      destinations: [{ destCode: 'KIX', destLabel: 'Osaka (KIX)' }],
      durations: [{ nights: 6 }],
    });
    await insertCandidate({
      candidateId: `${runId}-KIX-2026-06-18-6n`, runId, destCode: 'KIX',
      departDate: '2026-06-18', returnDate: '2026-06-24', nights: 6,
      flightTotalTwd: 18000, leaveDays: 3, verdict: null,
      flights: [{
        direction: 'outbound', airline: 'SL', departTime: '09:00',
        arriveTime: '12:30', duration: '2h30m', nonstop: true, priceTotalTwd: 9000,
      }],
    });
    expect(await getCandidates(runId)).toHaveLength(1);

    // A re-scrape of the same pair that yields zero candidates must still
    // clear the prior row — the delete is keyed on the pair, not the payload.
    await deleteCandidatesForPair(runId, 'KIX', 6);
    expect(await getCandidates(runId)).toHaveLength(0);
  });
});
```

> Note: `getScrapeAttempts` is already defined in Task 2's service file; Task 3 only adds the
> write helper `upsertScrapeAttempt` and `deleteCandidatesForPair`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `npm test -- stage0-service`
Expected: FAIL — `insertCandidate`/`rankRun`/etc. are not exported.

- [ ] **Step 3: Add candidates, scrape-attempts, ranking, adopt to the service**

Append to `src/services/stage0-service.ts`:

```typescript
// ── candidates ───────────────────────────────────────────────────────

export interface CandidateFlight {
  direction: 'outbound' | 'return';
  airline: string | null;
  departTime: string | null;
  arriveTime: string | null;
  duration: string | null;
  nonstop: boolean | null;
  priceTotalTwd: number | null;
}

export interface InsertCandidateInput {
  candidateId: string;
  runId: string;
  destCode: string;
  departDate: string;
  returnDate: string;
  nights: number;
  flightTotalTwd: number | null;
  leaveDays: number | null;
  verdict: string | null;
  flights: CandidateFlight[];
}

export interface Candidate {
  candidate_id: string;
  run_id: string;
  dest_code: string;
  depart_date: string;
  return_date: string;
  nights: number;
  flight_total_twd: number | null;
  leave_days: number | null;
  rank: number | null;
  verdict: string | null;
  adopted_plan_id: string | null;
}

export async function insertCandidate(input: InsertCandidateInput): Promise<void> {
  const client = new TursoPipelineClient();
  const stmts: string[] = [];
  stmts.push(
    `INSERT INTO stage0_candidates
      (candidate_id, run_id, dest_code, depart_date, return_date, nights,
       flight_total_twd, leave_days, rank, verdict, adopted_plan_id)
     VALUES (${sqlText(input.candidateId)}, ${sqlText(input.runId)}, ${sqlText(input.destCode)},
       ${sqlText(input.departDate)}, ${sqlText(input.returnDate)}, ${sqlInt(input.nights)},
       ${sqlInt(input.flightTotalTwd)}, ${sqlInt(input.leaveDays)}, NULL,
       ${sqlText(input.verdict)}, NULL);`
  );
  for (const f of input.flights) {
    stmts.push(
      `INSERT INTO stage0_candidate_flights
        (candidate_id, direction, airline, depart_time, arrive_time, duration, nonstop, price_total_twd)
       VALUES (${sqlText(input.candidateId)}, ${sqlText(f.direction)}, ${sqlText(f.airline)},
         ${sqlText(f.departTime)}, ${sqlText(f.arriveTime)}, ${sqlText(f.duration)},
         ${sqlInt(f.nonstop == null ? null : f.nonstop ? 1 : 0)}, ${sqlInt(f.priceTotalTwd)});`
    );
  }
  await client.executeMany(stmts);
}

export async function getCandidates(runId: string): Promise<Candidate[]> {
  const client = new TursoPipelineClient();
  // rank NULLs sort last so an un-ranked run still returns rows deterministically.
  const res = await client.execute(
    `SELECT * FROM stage0_candidates WHERE run_id = ${sqlText(runId)}
     ORDER BY rank IS NULL, rank ASC, depart_date ASC;`
  );
  return rowsToObjects(res).map((c) => ({
    candidate_id: c.candidate_id,
    run_id: c.run_id,
    dest_code: c.dest_code,
    depart_date: c.depart_date,
    return_date: c.return_date,
    nights: Number(c.nights),
    flight_total_twd: c.flight_total_twd == null ? null : Number(c.flight_total_twd),
    leave_days: c.leave_days == null ? null : Number(c.leave_days),
    rank: c.rank == null ? null : Number(c.rank),
    verdict: c.verdict ?? null,
    adopted_plan_id: c.adopted_plan_id ?? null,
  }));
}

// ── ranking (spec §4: flight_total_twd ASC, leave_days ASC, depart_date ASC) ──

export async function rankRun(runId: string): Promise<void> {
  const client = new TursoPipelineClient();
  // Read candidates, sort in JS (deterministic, NULL price last), write rank back.
  const res = await client.execute(
    `SELECT candidate_id, flight_total_twd, leave_days, depart_date
     FROM stage0_candidates WHERE run_id = ${sqlText(runId)};`
  );
  const rows = rowsToObjects(res);
  rows.sort((a, b) => {
    const pa = a.flight_total_twd == null ? Number.MAX_SAFE_INTEGER : Number(a.flight_total_twd);
    const pb = b.flight_total_twd == null ? Number.MAX_SAFE_INTEGER : Number(b.flight_total_twd);
    if (pa !== pb) return pa - pb;
    const la = a.leave_days == null ? Number.MAX_SAFE_INTEGER : Number(a.leave_days);
    const lb = b.leave_days == null ? Number.MAX_SAFE_INTEGER : Number(b.leave_days);
    if (la !== lb) return la - lb;
    return String(a.depart_date).localeCompare(String(b.depart_date));
  });
  const stmts = rows.map(
    (r, i) =>
      `UPDATE stage0_candidates SET rank = ${sqlInt(i + 1)}
       WHERE candidate_id = ${sqlText(r.candidate_id)};`
  );
  stmts.push(
    `UPDATE stage0_research_runs SET status = ${sqlText('ranked')},
       updated_at = ${sqlText(nowIso())} WHERE run_id = ${sqlText(runId)};`
  );
  await client.executeMany(stmts);
}

// ── scrape attempts ──────────────────────────────────────────────────

export interface ScrapeAttemptInput {
  runId: string;
  destCode: string;
  nights: number;
  status: 'pending' | 'ok' | 'failed';
  candidateCount?: number | null;
  error?: string | null;
}

// `ScrapeAttempt` and `getScrapeAttempts` are already defined in Task 2.
// This task only adds the write helper.

export async function upsertScrapeAttempt(input: ScrapeAttemptInput): Promise<void> {
  const client = new TursoPipelineClient();
  // INSERT OR REPLACE — PK is (run_id, dest_code, nights). Pending rows are
  // seeded by createResearchRun; this overwrites them with ok/failed outcomes.
  await client.execute(
    `INSERT OR REPLACE INTO stage0_scrape_attempts
      (run_id, dest_code, nights, status, candidate_count, error, attempted_at)
     VALUES (${sqlText(input.runId)}, ${sqlText(input.destCode)}, ${sqlInt(input.nights)},
       ${sqlText(input.status)}, ${sqlInt(input.candidateCount ?? null)},
       ${sqlText(input.error ?? null)}, ${sqlText(nowIso())});`
  );
}

// ── candidate replacement (idempotent re-import for one pair) ─────────
// Deletes any existing candidates + their flights for one (run, dest,
// nights) pair so a re-import of that pair does not collide on the
// candidate_id PK. Used by stage0-import before inserting.

export async function deleteCandidatesForPair(
  runId: string, destCode: string, nights: number
): Promise<void> {
  const client = new TursoPipelineClient();
  const where =
    `run_id = ${sqlText(runId)} AND dest_code = ${sqlText(destCode)} ` +
    `AND nights = ${sqlInt(nights)}`;
  await client.executeMany([
    `DELETE FROM stage0_candidate_flights WHERE candidate_id IN
      (SELECT candidate_id FROM stage0_candidates WHERE ${where});`,
    `DELETE FROM stage0_candidates WHERE ${where};`,
  ]);
}

// ── status + adopt ───────────────────────────────────────────────────

export async function setRunStatus(runId: string, status: string): Promise<void> {
  const client = new TursoPipelineClient();
  await client.execute(
    `UPDATE stage0_research_runs SET status = ${sqlText(status)},
       updated_at = ${sqlText(nowIso())} WHERE run_id = ${sqlText(runId)};`
  );
}

export async function adoptCandidate(candidateId: string, planId: string): Promise<void> {
  const client = new TursoPipelineClient();
  // Find the run, set the pointer, mark the run adopted — one batch.
  const res = await client.execute(
    `SELECT run_id FROM stage0_candidates WHERE candidate_id = ${sqlText(candidateId)};`
  );
  const rows = rowsToObjects(res);
  if (rows.length === 0) {
    throw new Error(`Stage 0 candidate not found: ${candidateId}`);
  }
  const runId = rows[0].run_id as string;
  await client.executeMany([
    `UPDATE stage0_candidates SET adopted_plan_id = ${sqlText(planId)}
      WHERE candidate_id = ${sqlText(candidateId)};`,
    `UPDATE stage0_research_runs SET status = ${sqlText('adopted')},
      updated_at = ${sqlText(nowIso())} WHERE run_id = ${sqlText(runId)};`,
  ]);
}
```

(The test file's import block was already extended in Step 1 to include all of these names.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `npm test -- stage0-service`
Expected: PASS — 6 tests (2 from Task 2 + 4 new).

- [ ] **Step 5: Run typecheck**

Run: `npm run typecheck`
Expected: `✅ Typecheck passed`.

- [ ] **Step 6: Commit**

```bash
git add src/services/stage0-service.ts tests/integration/stage0-service.regression.test.ts
git commit -m "feat: add Stage 0 candidates, scrape-attempts, ranking, adopt"
```

---

## Task 4: CLI — register all new Stage 0 options in the arg parser

**Files:**
- Modify: `src/cli/shared/args.ts:3-14`

Every value-bearing option the Stage 0 commands use must be in `OPTIONS_WITH_VALUES`,
or the shared parser leaves the value in `cleanArgs`. The Stage 0 commands use:
`--run`, `--file`, `--origin`, `--rate`. (`--start`, `--end`, `--dest`, `--pax`,
`--limit`, `--nights` are already registered.)

- [ ] **Step 1: Add the four new options to `OPTIONS_WITH_VALUES`**

In `src/cli/shared/args.ts`, append to the `OPTIONS_WITH_VALUES` set:

```typescript
  '--activities-zh-json', '--travel-date', '--travel-start', '--travel-end',
  '--run', '--file', '--origin', '--rate',
]);
```

- [ ] **Step 2: Run typecheck**

Run: `npm run typecheck`
Expected: `✅ Typecheck passed`.

- [ ] **Step 3: Commit**

```bash
git add src/cli/shared/args.ts
git commit -m "feat: register Stage 0 CLI options (--run, --file, --origin, --rate)"
```

---

## Task 5: CLI — `stage0-init`, `stage0-compare`, `stage0-adopt` commands

**Files:**
- Create: `src/cli/commands/stage0.ts`
- Modify: `src/cli/travel-update.ts`

- [ ] **Step 1: Create the command module**

Create `src/cli/commands/stage0.ts`:

```typescript
import type { CommandHandler, CliContext } from '../shared/types';
import { registerCommand } from './registry';

// All three commands are pre-plan (requiresState: false) — Stage 0 runs
// before any plan exists, so they must skip plan resolution.

function newRunId(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `stage0-${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-` +
    `${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}

// ── stage0-init ──────────────────────────────────────────────────────
// Creates an immutable research run. Destinations: --dest CODE:LABEL (repeatable).
// Durations: --nights N (repeatable). Window: --start / --end.

const stage0InitCommand: CommandHandler = {
  names: ['stage0-init'],
  description: 'Create a Stage 0 research run (immutable inputs).',
  usage: 'stage0-init --origin TPE --start 2026-06-18 --end 2026-06-20 ' +
    '--dest KIX:"Osaka (KIX)" --dest NRT:"Tokyo (NRT)" --nights 6 --nights 7 ' +
    '[--pax 2] [--rate 32]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { createResearchRun } = require('../../services/stage0-service');
    const { args } = ctx;

    const origin = args.optionValue('--origin');
    const start = args.optionValue('--start');
    const end = args.optionValue('--end');
    const destOpts = args.optionValues('--dest');
    const nightsOpts = args.optionValues('--nights');
    const pax = parseInt(args.optionValue('--pax') || '2', 10);
    const rate = parseFloat(args.optionValue('--rate') || '32');

    if (!origin || !start || !end || destOpts.length === 0 || nightsOpts.length === 0) {
      console.error('Error: stage0-init requires --origin, --start, --end, ' +
        'at least one --dest CODE:LABEL, and at least one --nights N');
      process.exit(1);
    }

    const destinations = destOpts.map((d) => {
      const idx = d.indexOf(':');
      if (idx === -1) {
        console.error(`Error: --dest must be CODE:LABEL (got: ${d})`);
        process.exit(1);
      }
      return { destCode: d.slice(0, idx).toUpperCase(), destLabel: d.slice(idx + 1) };
    });
    const durations = nightsOpts.map((n) => ({ nights: parseInt(n, 10) }));

    const runId = newRunId();
    await createResearchRun({
      runId, originCode: origin.toUpperCase(), pax,
      windowStart: start, windowEnd: end, exchangeRateUsdTwd: rate,
      destinations, durations,
    });

    console.log(`\n✅ Stage 0 research run created: ${runId}`);
    console.log(`   Origin: ${origin.toUpperCase()}  Window: ${start} → ${end}  Pax: ${pax}`);
    console.log(`   Destinations: ${destinations.map((d) => d.destCode).join(', ')}`);
    console.log(`   Durations: ${durations.map((d) => d.nights + 'n').join(', ')}`);
    console.log(`\nNext: python scripts/stage0_research.py --run ${runId}`);
  },
};

// ── stage0-compare ───────────────────────────────────────────────────

const stage0CompareCommand: CommandHandler = {
  names: ['stage0-compare'],
  description: 'Show ranked Stage 0 candidates across destinations.',
  usage: 'stage0-compare --run <run_id> [--json] [--limit N]',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { getResearchRun, getCandidates } = require('../../services/stage0-service');
    const { args } = ctx;
    const runId = args.optionValue('--run');
    if (!runId) {
      console.error('Error: stage0-compare requires --run <run_id>');
      process.exit(1);
    }
    const run = await getResearchRun(runId);
    if (!run) {
      console.error(`Error: research run not found: ${runId}`);
      process.exit(1);
    }
    const limit = parseInt(args.optionValue('--limit') || '0', 10);
    let candidates = await getCandidates(runId);
    if (limit > 0) candidates = candidates.slice(0, limit);

    if (args.hasFlag('--json')) {
      console.log(JSON.stringify({ run, candidates }, null, 2));
      return;
    }

    console.log(`\nStage 0 Research — ${run.run_id}  (${run.origin_code}, ` +
      `${run.pax} pax, window ${run.window_start}..${run.window_end})`);
    console.log('');
    if (candidates.length === 0) {
      console.log('(no candidates — run the aggregator first)\n');
      return;
    }
    const header = [
      '#'.padEnd(3), 'Dest'.padEnd(5), 'Depart'.padEnd(12), 'Return'.padEnd(12),
      'Nights'.padEnd(7), 'Flight (party)'.padEnd(16), 'Leave'.padEnd(6), 'Verdict',
    ].join(' ');
    console.log(header);
    console.log('─'.repeat(header.length));
    for (const c of candidates) {
      const price = c.flight_total_twd == null
        ? 'n/a' : `${run.currency} ${c.flight_total_twd.toLocaleString()}`;
      console.log([
        String(c.rank ?? '-').padEnd(3),
        c.dest_code.padEnd(5),
        c.depart_date.padEnd(12),
        c.return_date.padEnd(12),
        `${c.nights}n`.padEnd(7),
        price.padEnd(16),
        String(c.leave_days ?? '-').padEnd(6),
        c.verdict ?? '',
      ].join(' '));
    }
    console.log('');
  },
};

// ── stage0-adopt ─────────────────────────────────────────────────────

const stage0AdoptCommand: CommandHandler = {
  names: ['stage0-adopt'],
  description: 'Record a Stage 0 candidate as adopted into a plan.',
  usage: 'stage0-adopt <candidate_id> <plan_id>',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { adoptCandidate } = require('../../services/stage0-service');
    const [, candidateId, planId] = ctx.args.cleanArgs;
    if (!candidateId || !planId) {
      console.error('Error: stage0-adopt requires <candidate_id> <plan_id>');
      process.exit(1);
    }
    await adoptCandidate(candidateId, planId);
    console.log(`✅ Candidate ${candidateId} adopted into plan ${planId}`);
    console.log('   Next: set the locked dates/destination via /p1-dates and /p2-destination');
  },
};

registerCommand(stage0InitCommand);
registerCommand(stage0CompareCommand);
registerCommand(stage0AdoptCommand);
```

- [ ] **Step 2: Register the module in the CLI entry**

In `src/cli/travel-update.ts`, after the line `import './commands/plans';`, add:

```typescript
import './commands/stage0';
```

- [ ] **Step 3: Run typecheck**

Run: `npm run typecheck`
Expected: `✅ Typecheck passed`.

- [ ] **Step 4: Smoke-test the commands end-to-end**

Run:
```bash
npm run travel -- stage0-init --origin TPE --start 2026-06-18 --end 2026-06-20 \
  --dest KIX:"Osaka (KIX)" --nights 6
```
Expected: prints `✅ Stage 0 research run created: stage0-...` with a run id.

Then, using that run id:
```bash
npm run travel -- stage0-compare --run <run_id>
```
Expected: prints the run header and `(no candidates — run the aggregator first)`.

- [ ] **Step 5: Clean up the smoke-test run**

`stage0-init` seeds `stage0_scrape_attempts` rows, so the cleanup must delete those too.
`db:exec` runs each `;`-separated statement individually and prints `[N/M]` per statement; it exits nonzero if any fails.

Run:
```bash
npm run db:exec -- "DELETE FROM stage0_scrape_attempts WHERE run_id='<run_id>'; DELETE FROM stage0_research_durations WHERE run_id='<run_id>'; DELETE FROM stage0_research_destinations WHERE run_id='<run_id>'; DELETE FROM stage0_research_runs WHERE run_id='<run_id>';"
```
Expected: 4 `[N/4]` lines, each `1 row(s) affected` (or `ok` if a table had no rows), exit 0.

Then **verify** the run is gone — do not trust exit code alone:
```bash
npm run db:exec -- "SELECT COUNT(*) AS n FROM stage0_research_runs WHERE run_id='<run_id>';"
```
Expected: `{"n":"0"}`.

- [ ] **Step 6: Add the Stage 0 commands to the static help text**

`src/cli/commands/help.ts` is a static `HELP` template string — new commands do not appear
automatically. In `src/cli/commands/help.ts`, inside the `HELP` string's `Commands:` section,
after the `run-status` / `run-list` block (the last command block), add:

```
  stage0-init --origin <IATA> --start <date> --end <date> --dest CODE:LABEL --nights N
    Create a Stage 0 triangle-research run (immutable inputs; pre-plan).
    Repeat --dest and --nights for multiple destinations/durations.
    Example: stage0-init --origin TPE --start 2026-06-18 --end 2026-06-20 --dest KIX:"Osaka (KIX)" --nights 6

  stage0-export --run <run_id> --json
    Export a Stage 0 run as JSON (consumed by the aggregator script).

  stage0-import --run <run_id> --file <path>
    Import Stage 0 aggregator results from a handoff JSON file.

  stage0-compare --run <run_id> [--json] [--limit N]
    Show ranked Stage 0 flight candidates across destinations.
    Example: stage0-compare --run stage0-20260522-143000

  stage0-adopt <candidate_id> <plan_id>
    Record a Stage 0 candidate as adopted into a plan.
```

- [ ] **Step 7: Run typecheck**

Run: `npm run typecheck`
Expected: `✅ Typecheck passed`.

- [ ] **Step 8: Commit**

```bash
git add src/cli/commands/stage0.ts src/cli/travel-update.ts src/cli/commands/help.ts
git commit -m "feat: add stage0-init, stage0-compare, stage0-adopt CLI commands"
```

---

## Task 6: Python aggregator — `scripts/stage0_research.py`

**Files:**
- Create: `scripts/stage0_research.py`

The aggregator reads a run from Turso, scrapes each (destination, duration) pair via `scrape_date_range.py` into a temp file, parses results into candidates, computes leave-days, and ranks.

- [ ] **Step 1: Confirm the scrape-result JSON shape**

The aggregator performs **no** Turso I/O directly — not even reads. It loads the run via the
`stage0-export` CLI command and writes via `stage0-import` (both in Task 7). This keeps all
SQL in TypeScript, where `sql-helpers.ts` escaping is enforced — no SQL string is ever built
from CLI args in Python.

> **Task ordering note:** the aggregator (Task 6) calls `stage0-export`/`stage0-import`, which
> are created in Task 7. Task 6's Step 3 smoke test only checks `--help` (no DB calls), so it
> is safe to build Task 6 before Task 7. The full end-to-end aggregator run is exercised in
> Task 10, after both exist. If executing strictly in order, this is fine; if a worker prefers
> to build Task 7 first, that is also valid.

Read `scripts/scrape_date_range.py` to confirm its output JSON shape, which Step 2's parser depends on: `{ scraped_at, params, results: [...] }`. Each result row has `depart_date`, `return_date`, `depart_day`, `return_day`, `combined_cheapest_twd`, and `outbound`/`inbound` objects each containing a `flights` array (whose entries have `airline`, `depart`, `arrive`, `duration`, `nonstop`, `total_usd`).

- [ ] **Step 2: Create the aggregator script**

Create `scripts/stage0_research.py`. The aggregator performs **no** Turso I/O of its own —
not even reads. It loads the run by shelling out to the `stage0-export` CLI command (Task 7,
built before this task in the dependency order — but written after; the smoke test in Step 3
only checks `--help`, and the end-to-end test in Task 10 runs after both exist). This keeps
**all** SQL in TypeScript, where `sql-helpers.ts` escaping is enforced — no raw SQL strings
are ever built from CLI args in Python.

For each (destination, duration) pair, it checks the pair's seeded scrape-attempt status:
pairs already `ok` are **skipped** (idempotent re-run); `pending`/`failed` pairs are scraped.
It invokes `scrape_date_range.py` into a temp file, parses the results, accumulates candidate
dicts, and hands everything to `stage0-import`.

```python
#!/usr/bin/env python3
"""
Stage 0 aggregator — scrapes flight candidates across destination x duration
for one immutable research run, then hands results to the TS CLI for import.

Performs NO Turso I/O directly: it loads the run via `stage0-export` and writes
via `stage0-import`. All SQL stays in TypeScript (sql-helpers.ts escaping).

For each (destination, duration) pair it checks the seeded scrape-attempt
status — 'ok' pairs are skipped (idempotent re-run), 'pending'/'failed' pairs
are scraped via scrape_date_range.py into a temp file. Results are handed to
`npm run travel -- stage0-import`, which performs all DB writes + ranking.

Temp files are transient implementation detail — not durable state.

Usage:
  python scripts/stage0_research.py --run <run_id>
"""
import argparse
import json
import os
import subprocess
import sys
import tempfile

THIS_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.dirname(THIS_DIR)


def load_run(run_id):
    """Load the run + destinations + durations + scrape attempts via the
    stage0-export CLI command (all SQL stays in TypeScript)."""
    proc = subprocess.run(
        ["npm", "run", "--silent", "travel", "--",
         "stage0-export", "--run", run_id, "--json"],
        check=True, cwd=PROJECT_ROOT, capture_output=True, text=True)
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError:
        print(f"Error: stage0-export did not return JSON for {run_id}",
              file=sys.stderr)
        print(proc.stdout, file=sys.stderr)
        sys.exit(1)


def scrape_pair(run, dest, duration_days):
    """Scrape one (destination, duration) pair via scrape_date_range.py.
    Returns the parsed results list, or raises on failure."""
    with tempfile.NamedTemporaryFile(
            mode="r", suffix=".json", delete=False) as tf:
        tmp_path = tf.name
    try:
        cmd = [
            sys.executable,
            os.path.join(THIS_DIR, "scrape_date_range.py"),
            "--depart-start", run["window_start"],
            "--depart-end", run["window_end"],
            "--origin", run["origin_code"].lower(),
            "--dest", dest["dest_code"].lower(),
            "--duration", str(duration_days),
            "--pax", str(run["pax"]),
            "--exchange-rate", str(run["exchange_rate_usd_twd"]),
            "--output", tmp_path,
        ]
        subprocess.run(cmd, check=True, cwd=PROJECT_ROOT)
        with open(tmp_path, "r", encoding="utf-8") as f:
            return json.load(f).get("results", [])
    finally:
        if os.path.exists(tmp_path):
            os.unlink(tmp_path)


def build_candidates(run, dest, nights, results):
    """Map scrape results -> candidate dicts for stage0-import."""
    candidates = []
    for r in results:
        depart = r.get("depart_date")
        return_date = r.get("return_date")
        if not depart or not return_date:
            continue
        total = r.get("combined_cheapest_twd")
        cand_id = f"{run['run_id']}-{dest['dest_code']}-{depart}-{nights}n"
        flights = []
        for direction, key in (("outbound", "outbound"), ("return", "inbound")):
            leg = r.get(key) or {}
            leg_flights = leg.get("flights") or []
            if not leg_flights:
                continue
            cheapest = min(leg_flights, key=lambda x: x.get("total_usd", 1e9))
            flights.append({
                "direction": direction,
                "airline": cheapest.get("airline"),
                "departTime": cheapest.get("depart"),
                "arriveTime": cheapest.get("arrive"),
                "duration": cheapest.get("duration"),
                "nonstop": cheapest.get("nonstop"),
                "priceTotalTwd": None,
            })
        candidates.append({
            "candidateId": cand_id,
            "runId": run["run_id"],
            "destCode": dest["dest_code"],
            "departDate": depart,
            "returnDate": return_date,
            "nights": nights,
            "flightTotalTwd": int(total) if total is not None else None,
            "leaveDays": None,  # computed by stage0-import (TS leave calculator)
            "verdict": None,
            "flights": flights,
        })
    return candidates


def main():
    parser = argparse.ArgumentParser(description="Stage 0 flight aggregator")
    parser.add_argument("--run", required=True, help="Stage 0 run_id")
    args = parser.parse_args()

    run = load_run(args.run)
    print(f"Stage 0 aggregator — run {run['run_id']} "
          f"({len(run['destinations'])} dest x {len(run['durations'])} duration)")

    # Build a {(dest_code, nights): status} map from the seeded attempt rows.
    attempt_status = {
        (a["dest_code"], int(a["nights"])): a["status"]
        for a in run.get("attempts", [])
    }

    all_candidates = []
    attempts = []
    for dest in run["destinations"]:
        for dur in run["durations"]:
            nights = int(dur["nights"])
            duration_days = int(dur["duration_days"])
            label = f"{dest['dest_code']} {nights}n"
            # Idempotent re-run: skip pairs already scraped successfully.
            if attempt_status.get((dest["dest_code"], nights)) == "ok":
                print(f"  skipping {label} (already ok)")
                continue
            try:
                print(f"  scraping {label} ...")
                results = scrape_pair(run, dest, duration_days)
                cands = build_candidates(run, dest, nights, results)
                all_candidates.extend(cands)
                attempts.append({
                    "runId": run["run_id"], "destCode": dest["dest_code"],
                    "nights": nights, "status": "ok",
                    "candidateCount": len(cands), "error": None,
                })
                print(f"    -> {len(cands)} candidates")
            except Exception as exc:  # noqa: BLE001 — continue other pairs
                print(f"    !! {label} failed: {exc}", file=sys.stderr)
                attempts.append({
                    "runId": run["run_id"], "destCode": dest["dest_code"],
                    "nights": nights, "status": "failed",
                    "candidateCount": None, "error": str(exc)[:500],
                })

    if not all_candidates and not attempts:
        print("All pairs already scraped — nothing to do.")
        print(f"View: npm run travel -- stage0-compare --run {run['run_id']}")
        return

    # Hand off to the TS CLI for all DB writes + leave-days + ranking.
    with tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", delete=False, encoding="utf-8") as tf:
        handoff_path = tf.name
        json.dump({"candidates": all_candidates, "attempts": attempts}, tf,
                  ensure_ascii=False)
    try:
        subprocess.run(
            ["npm", "run", "travel", "--", "stage0-import",
             "--run", run["run_id"], "--file", handoff_path],
            check=True, cwd=PROJECT_ROOT)
    finally:
        if os.path.exists(handoff_path):
            os.unlink(handoff_path)

    print(f"Done. View: npm run travel -- stage0-compare --run {run['run_id']}")


if __name__ == "__main__":
    main()
```

- [ ] **Step 3: Verify the script parses arguments**

Run: `python scripts/stage0_research.py --help`
Expected: prints usage with `--run` required, exits 0.

- [ ] **Step 4: Commit**

```bash
git add scripts/stage0_research.py
git commit -m "feat: add Stage 0 flight aggregator script"
```

---

## Task 7: CLI — `stage0-export` and `stage0-import` (aggregator handoff)

**Files:**
- Modify: `src/cli/commands/stage0.ts`

`stage0-export` emits a run (run + destinations + durations + scrape attempts) as JSON for
the Python aggregator to consume — so no SQL is built in Python. `stage0-import` consumes the
aggregator's JSON handoff: for each scraped pair it deletes any prior candidates for that
pair (idempotent re-run), inserts the new candidates with leave-days computed via the TS
calculator, records scrape attempts, ranks, and sets final status. All DB writes and the
leave calculation stay in TypeScript.

`--file` was already registered in Task 4 — no `args.ts` change needed here.

- [ ] **Step 1: Add the `stage0-export` command**

In `src/cli/commands/stage0.ts`, add this handler before the `registerCommand(...)` calls:

```typescript
// ── stage0-export ────────────────────────────────────────────────────
// Emits a run (run + destinations + durations + scrape attempts) as JSON.
// The Python aggregator consumes this instead of building SQL itself.

const stage0ExportCommand: CommandHandler = {
  names: ['stage0-export'],
  description: 'Export a Stage 0 research run as JSON (for the aggregator).',
  usage: 'stage0-export --run <run_id> --json',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const { getResearchRun, getScrapeAttempts } = require('../../services/stage0-service');
    const runId = ctx.args.optionValue('--run');
    if (!runId) {
      console.error('Error: stage0-export requires --run <run_id>');
      process.exit(1);
    }
    const run = await getResearchRun(runId);
    if (!run) {
      console.error(`Error: research run not found: ${runId}`);
      process.exit(1);
    }
    const attempts = await getScrapeAttempts(runId);
    // Single JSON object on stdout — nothing else, so Python can parse it.
    console.log(JSON.stringify({ ...run, attempts }));
  },
};
```

- [ ] **Step 2: Add the `stage0-import` command**

In `src/cli/commands/stage0.ts`, add this handler before the `registerCommand(...)` calls:

```typescript
// ── stage0-import ────────────────────────────────────────────────────
// Consumes the Python aggregator's JSON handoff. Idempotent per pair:
// for each scraped (dest, nights) pair it first deletes any prior
// candidates for that pair, so a re-import never collides on the
// candidate_id PK. Inserts candidates with leave-days computed via the TS
// calculator, records scrape attempts, ranks, sets final run status.

const stage0ImportCommand: CommandHandler = {
  names: ['stage0-import'],
  description: 'Import Stage 0 aggregator results from a handoff JSON file.',
  usage: 'stage0-import --run <run_id> --file <path>',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const fs = require('fs');
    const {
      insertCandidate, upsertScrapeAttempt, rankRun, setRunStatus,
      getResearchRun, getCandidates, deleteCandidatesForPair,
    } = require('../../services/stage0-service');
    const { calculateLeave } = require('../../utils/holiday-calculator');
    const { args } = ctx;

    const runId = args.optionValue('--run');
    const file = args.optionValue('--file');
    if (!runId || !file) {
      console.error('Error: stage0-import requires --run <run_id> and --file <path>');
      process.exit(1);
    }
    const run = await getResearchRun(runId);
    if (!run) {
      console.error(`Error: research run not found: ${runId}`);
      process.exit(1);
    }
    const payload = JSON.parse(fs.readFileSync(file, 'utf-8'));
    const candidates: any[] = payload.candidates || [];
    const attempts: any[] = payload.attempts || [];

    // Idempotent per pair: clear prior candidates for every pair THIS handoff
    // processed before inserting. The delete set is built from `attempts`,
    // not `candidates` — attempts are the authoritative list of pairs this
    // handoff scraped. A pair that scraped successfully but returned zero
    // flights still has an attempt row; building the set from `candidates`
    // would skip it and leave stale rows from a prior import.
    const pairs = new Set<string>();
    for (const a of attempts) pairs.add(`${a.destCode}|${a.nights}`);
    for (const key of pairs) {
      const [destCode, nightsStr] = key.split('|');
      await deleteCandidatesForPair(runId, destCode, parseInt(nightsStr, 10));
    }

    for (const a of attempts) {
      await upsertScrapeAttempt({
        runId, destCode: a.destCode, nights: a.nights,
        status: a.status, candidateCount: a.candidateCount, error: a.error,
      });
    }

    for (const c of candidates) {
      // Leave-days computed here — TS owns the holiday calendar.
      const leave = calculateLeave({
        startDate: c.departDate, endDate: c.returnDate, market: 'taiwan',
      });
      await insertCandidate({
        candidateId: c.candidateId, runId, destCode: c.destCode,
        departDate: c.departDate, returnDate: c.returnDate, nights: c.nights,
        flightTotalTwd: c.flightTotalTwd ?? null,
        leaveDays: leave.leaveDaysNeeded,
        verdict: c.verdict ?? null,
        flights: c.flights || [],
      });
    }

    // Rank if the run has any candidates at all (this import may have added
    // to candidates from an earlier partial run). Mark failed only when the
    // run is still completely empty.
    const allCandidates = await getCandidates(runId);
    if (allCandidates.length === 0) {
      await setRunStatus(runId, 'failed');
      console.log(`⚠️  No candidates for ${runId} — run marked failed.`);
      return;
    }
    await rankRun(runId);
    console.log(`✅ Imported ${candidates.length} candidates for ${runId} ` +
      `(${allCandidates.length} total), ranked.`);
    console.log(`   View: npm run travel -- stage0-compare --run ${runId}`);
  },
};
```

Then add all four to the registration block:

```typescript
registerCommand(stage0InitCommand);
registerCommand(stage0CompareCommand);
registerCommand(stage0AdoptCommand);
registerCommand(stage0ExportCommand);
registerCommand(stage0ImportCommand);
```

- [ ] **Step 3: Run typecheck**

Run: `npm run typecheck`
Expected: `✅ Typecheck passed`.

- [ ] **Step 4: Smoke-test export + import with a hand-built handoff file**

Create a run, verify `stage0-export`, then import. The run id is needed in three places —
capture it in a shell variable:
```bash
RID=$(npm run --silent travel -- stage0-init --origin TPE \
  --start 2026-06-18 --end 2026-06-18 --dest KIX:"Osaka (KIX)" --nights 6 \
  | grep -oE 'stage0-[0-9]+-[0-9]+')
echo "run id: $RID"

# stage0-export must emit one JSON line the aggregator can parse:
npm run --silent travel -- stage0-export --run "$RID" --json | head -c 200

# build a handoff file with the real run id substituted in:
printf '{"candidates":[{"candidateId":"%s-KIX-2026-06-18-6n","runId":"%s","destCode":"KIX","departDate":"2026-06-18","returnDate":"2026-06-24","nights":6,"flightTotalTwd":18000,"flights":[]}],"attempts":[{"runId":"%s","destCode":"KIX","nights":6,"status":"ok","candidateCount":1,"error":null}]}' "$RID" "$RID" "$RID" > /tmp/s0-handoff.json

npm run travel -- stage0-import --run "$RID" --file /tmp/s0-handoff.json
npm run travel -- stage0-compare --run "$RID"
```
Expected: `stage0-export` prints a JSON object starting `{"run_id":"stage0-...`; import prints `✅ Imported 1 candidates ... ranked`; compare shows one ranked row with a computed (non-null) leave-days value.

- [ ] **Step 5: Verify import idempotency**

Run the same import a second time:
```bash
npm run travel -- stage0-import --run "$RID" --file /tmp/s0-handoff.json
```
Expected: succeeds again (no PK collision) — prints `✅ Imported 1 candidates (1 total), ranked`. The per-pair delete made it idempotent.

- [ ] **Step 6: Clean up the smoke-test run**

`db:exec` runs each `;`-separated statement individually and exits nonzero if any fails — but **also verify the run is gone** rather than trusting the exit code alone.

Run:
```bash
npm run db:exec -- "DELETE FROM stage0_candidate_flights WHERE candidate_id IN (SELECT candidate_id FROM stage0_candidates WHERE run_id='$RID'); DELETE FROM stage0_candidates WHERE run_id='$RID'; DELETE FROM stage0_scrape_attempts WHERE run_id='$RID'; DELETE FROM stage0_research_durations WHERE run_id='$RID'; DELETE FROM stage0_research_destinations WHERE run_id='$RID'; DELETE FROM stage0_research_runs WHERE run_id='$RID';"
npm run db:exec -- "SELECT COUNT(*) AS n FROM stage0_research_runs WHERE run_id='$RID';"
```
Expected: 6 `[N/6]` lines from the DELETE chain, followed by `{"n":"0"}` from the verification SELECT.

- [ ] **Step 7: Commit**

```bash
git add src/cli/commands/stage0.ts
git commit -m "feat: add stage0-export and idempotent stage0-import commands"
```

---

## Task 8: The `/stage0-research` orchestration skill

**Files:**
- Create: `src/skills/stage0-research/SKILL.md`

- [ ] **Step 1: Read an existing skill for the house format**

Read `src/skills/p3-flights/SKILL.md` in full to match frontmatter keys, section headings, and tone.

- [ ] **Step 2: Create the skill**

Create `src/skills/stage0-research/SKILL.md`:

```markdown
---
name: stage0-research
description: Pre-lock "triangle research" — explore departure date, destination, and flight price together before any plan is committed. Owns Stage 0 of the research-first planning flow.
version: 1.0.0
requires_skills: [travel-shared, scrape-ota]
requires_processes: []
provides_processes: []
---

# /stage0-research

Orchestration skill for **Stage 0 — Triangle Research** of the adopted
research-first planning flow (`docs/plans/2026-05-22-new-planning-flow.md`).

It explores the three interdependent variables — departure date, destination,
flight price — *together*, before any of them is locked. It does **not**
replace `/p3-flights`: that skill requires P1/P2 to already exist, so it cannot
run pre-lock. `/stage0-research` owns the pre-lock phase and can seed the
initial P1/P2 rows once the user picks a candidate.

## When to use

- User describes a trip in loose terms — a date window and 1–3 candidate
  destinations — and has not committed to dates or a destination.
- Triggers: "find me the cheapest week to go to Japan in June", "should I do
  Osaka or Tokyo, depends on flight price", "what dates are cheapest".

Do **not** use this once dates and destination are already locked — go
straight to `/p3-flights` or `/p3p4-packages`.

## Data model

Stage 0 data lives in six unscoped Turso tables (keyed by `run_id`, not
`plan_id`): `stage0_research_runs`, `stage0_research_destinations`,
`stage0_research_durations`, `stage0_candidates`, `stage0_candidate_flights`,
`stage0_scrape_attempts`. See
`docs/superpowers/specs/2026-05-22-stage0-triangle-research-design.md`.

**Runs are immutable.** A `run_id` fixes origin, window, pax, exchange rate,
destinations, and durations. Changing any of those = a new run.

## Workflow

1. **Gather inputs** from the user (ask, do not guess):
   - Origin airport (default TPE)
   - Travel window — earliest and latest acceptable departure date
   - 1–3 candidate destinations (airport code + label)
   - Trip lengths to consider, in nights (e.g., 6 and 7)
   - Passenger count (default 2)

2. **Create the run:**
   ```bash
   npm run travel -- stage0-init --origin TPE \
     --start 2026-06-18 --end 2026-06-20 \
     --dest KIX:"Osaka/Kyoto (KIX)" --dest NRT:"Tokyo (NRT)" \
     --nights 6 --nights 7 --pax 2 --rate 32
   ```
   Note the `run_id` it prints.

3. **Run the aggregator** (scrapes destination × duration, imports + ranks):
   ```bash
   python scripts/stage0_research.py --run <run_id>
   ```

4. **Show the ranking:**
   ```bash
   npm run travel -- stage0-compare --run <run_id>
   ```
   Present the ranked table. Candidates sort by flight price; leave-days is a
   shown column and a tie-breaker only.

5. **Iterate.** If the user wants different destinations, a shifted window, or
   other durations, that is a **new run** (runs are immutable) — go back to
   step 2 with the new inputs. The previous run's candidates stay intact and
   comparable.

6. **Hand off on lock.** When the user picks a candidate:
   ```bash
   npm run travel -- stage0-adopt <candidate_id> <new_plan_id> \
     --create-plan --dest <destination_slug>
   ```
   This creates the minimal normalized plan rows, sets P1 dates from the
   candidate's depart/return dates, sets P2 destination from `--dest`, and
   links `adopted_plan_id` on the Stage 0 candidate. Use an existing
   `destination_config` slug such as `osaka_kyoto_2026`.

   If the plan already exists, use the legacy link-only form:
   ```bash
   npm run travel -- stage0-adopt <candidate_id> <existing_plan_id>
   ```

   After a new-plan handoff, continue with `/stage1-itinerary-draft`, whose
   first CLI step is:
   ```bash
   npm run travel -- scaffold-itinerary --plan-id <new_plan_id> --dest <destination_slug>
   ```

## Notes

- If a (destination, duration) scrape fails, the aggregator records it in
  `stage0_scrape_attempts` and continues. Re-running the aggregator on the
  same run retries only failed/pending attempts.
- The adopted planning flow is Stage 0 through Stage 4. Existing `/p1-*`
  through `/p5-*` skills remain implementation tools after Stage 0 locks a
  candidate.
```

- [ ] **Step 3: Commit**

```bash
git add src/skills/stage0-research/SKILL.md
git commit -m "feat: add /stage0-research orchestration skill"
```

---

## Task 9: Documentation — wire Stage 0 into project docs

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/plans/2026-05-22-new-planning-flow.md`

- [ ] **Step 1: Add the skill to CLAUDE.md's skills table**

In `CLAUDE.md`, in the `## Available Skills` table, add this row after the `/new-destination` row:

```markdown
| `/stage0-research` | `src/skills/stage0-research/SKILL.md` | Pre-lock triangle research (date/destination/flight) |
```

- [ ] **Step 2: Add the Stage 0 tables to CLAUDE.md's Turso DB section**

In `CLAUDE.md`, in the `## Turso DB` "Tables:" list, add a new bullet after the `Operation tracking` bullet:

```markdown
- **Stage 0 research** (unscoped, keyed by `run_id`): `stage0_research_runs`, `stage0_research_destinations`, `stage0_research_durations`, `stage0_candidates`, `stage0_candidate_flights`, `stage0_scrape_attempts` — pre-plan triangle-research domain (see `docs/superpowers/specs/2026-05-22-stage0-triangle-research-design.md`)
```

- [ ] **Step 3: Update the planning-flow doc's Stage 0 Skill Mapping row**

In `docs/plans/2026-05-22-new-planning-flow.md`, in the Skill Mapping table, replace the Stage 0 row:

```markdown
| Stage 0 — Triangle Research | `/stage0-research` (orchestration skill) + `scripts/stage0_research.py` | `/stage0-research` owns pre-lock research — it has `requires_processes: []`, so it runs before dates/destination exist. `/p3-flights` still cannot be reused here (it requires P1/P2). |
```

- [ ] **Step 4: Run the doc/data validation**

Run: `npm run validate:data`
Expected: `✅ Data validation passed` (0 errors).

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md docs/plans/2026-05-22-new-planning-flow.md
git commit -m "docs: wire Stage 0 research into CLAUDE.md and planning-flow doc"
```

---

## Task 10: Full verification

**Files:** none (verification only)

- [ ] **Step 1: Run the full test suite**

Run: `npm test`
Expected: all tests pass, including the 6 `stage0-service` tests.

- [ ] **Step 2: Run typecheck**

Run: `npm run typecheck`
Expected: `✅ Typecheck passed`.

- [ ] **Step 3: Run the doctor**

Run: `npm run doctor`
Expected: full health check passes (0 errors).

- [ ] **Step 4: Verify `stage0-init` appears in CLI help**

Run: `npm run travel -- help`
Expected: help text lists `stage0-init`, `stage0-export`, `stage0-import`, `stage0-compare`, `stage0-adopt`.

- [ ] **Step 5: Final commit (if any uncommitted changes remain)**

```bash
git status
# if clean, nothing to do; otherwise:
git add -A && git commit -m "chore: Stage 0 triangle research — final verification"
```

---

## Self-Review Notes

**Spec coverage:**
- §3.1–3.6 six tables → Task 1 ✓
- §4 ranking (price, leave-days, depart-date) → Task 3 `rankRun` + test ✓
- §5.1 aggregator (dest × duration, temp-file capture, exchange-rate, scrape-attempts) → Task 6 ✓
- §5.2 `stage0-compare` (`requiresState: false`, `--run`) → Task 5 + Task 4 ✓
- §5.3 `/stage0-research` skill → Task 8 ✓
- §6 adopt / handoff (`adopted_plan_id`) → Task 3 `adoptCandidate` + Task 5 `stage0-adopt` ✓
- §7 migration idempotency + schema.sql mirror → Task 1 ✓
- §8 doc hygiene → Task 9 ✓
- §9 tests (migration, ranking, compare, adopt, scrape-attempt) → Task 1 Step 4, Task 3 tests ✓

**Mechanism decision resolved:** the spec left "is run-creation a `stage0-init` command or skill-direct" open. This plan resolves it: `stage0-init` is a CLI command (Task 5) that also seeds one `pending` `stage0_scrape_attempts` row per destination × duration. The Python aggregator performs **no** Turso I/O at all — it reads the run via `stage0-export` and writes via `stage0-import` (Task 7), so every SQL statement stays in TypeScript under `sql-helpers.ts` escaping. `stage0-import` is idempotent per (dest, nights) pair via `deleteCandidatesForPair`, and the aggregator skips pairs already marked `ok` — so re-running a partially-failed run is safe and never collides on the `candidate_id` PK.

**Type consistency:** `CreateRunInput`, `ResearchRun`, `ScrapeAttempt` defined in Task 2; `Candidate`, `InsertCandidateInput`, `CandidateFlight`, `ScrapeAttemptInput` defined in Task 3; all used consistently by Tasks 5 and 7. Function names (`createResearchRun`, `getResearchRun`, `getScrapeAttempts`, `deleteResearchRun` — Task 2; `insertCandidate`, `getCandidates`, `rankRun`, `upsertScrapeAttempt`, `deleteCandidatesForPair`, `setRunStatus`, `adoptCandidate` — Task 3) are stable across all tasks. `getScrapeAttempts` is defined once (Task 2) and only consumed thereafter — Task 3 does not redefine it.

**Review fixes applied (post-review patch):**
- Retry idempotency — aggregator skips `ok` pairs; `stage0-import` deletes candidates per (dest, nights) pair before inserting (`deleteCandidatesForPair`).
- `pending` attempt rows seeded at `stage0-init`, so a mid-run crash leaves a visible work matrix.
- No raw SQL in Python — aggregator reads via `stage0-export`, writes via `stage0-import`; all SQL stays in TypeScript.
- `--origin`, `--rate`, `--file` registered in `OPTIONS_WITH_VALUES` alongside `--run` (Task 4).
- Static `help.ts` updated with all five Stage 0 commands (Task 5 Step 6).
