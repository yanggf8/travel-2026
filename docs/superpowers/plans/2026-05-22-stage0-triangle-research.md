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
Expected: PASS — 1 test.

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

Append to `tests/integration/stage0-service.regression.test.ts` (inside the file, after the existing `describe` block):

```typescript
import {
  insertCandidate,
  upsertScrapeAttempt,
  rankRun,
  getCandidates,
  setRunStatus,
  adoptCandidate,
} from '../../src/services/stage0-service';

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
});
```

> Note: `getScrapeAttempts` is used above — it is added in Step 3.

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

export interface ScrapeAttempt {
  run_id: string;
  dest_code: string;
  nights: number;
  status: string;
  candidate_count: number | null;
  error: string | null;
  attempted_at: string | null;
}

export async function upsertScrapeAttempt(input: ScrapeAttemptInput): Promise<void> {
  const client = new TursoPipelineClient();
  // INSERT OR REPLACE — PK is (run_id, dest_code, nights).
  await client.execute(
    `INSERT OR REPLACE INTO stage0_scrape_attempts
      (run_id, dest_code, nights, status, candidate_count, error, attempted_at)
     VALUES (${sqlText(input.runId)}, ${sqlText(input.destCode)}, ${sqlInt(input.nights)},
       ${sqlText(input.status)}, ${sqlInt(input.candidateCount ?? null)},
       ${sqlText(input.error ?? null)}, ${sqlText(nowIso())});`
  );
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

Then add `getScrapeAttempts` to the import list at the top of the test file's second `describe` import block (it is already used in the test from Step 1 — make sure the import line includes it):

```typescript
import {
  insertCandidate,
  upsertScrapeAttempt,
  rankRun,
  getCandidates,
  setRunStatus,
  adoptCandidate,
  getScrapeAttempts,
} from '../../src/services/stage0-service';
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `npm test -- stage0-service`
Expected: PASS — 4 tests (1 from Task 2 + 3 new).

- [ ] **Step 5: Run typecheck**

Run: `npm run typecheck`
Expected: `✅ Typecheck passed`.

- [ ] **Step 6: Commit**

```bash
git add src/services/stage0-service.ts tests/integration/stage0-service.regression.test.ts
git commit -m "feat: add Stage 0 candidates, scrape-attempts, ranking, adopt"
```

---

## Task 4: CLI — add `--run` to the arg parser

**Files:**
- Modify: `src/cli/shared/args.ts:3-14`

- [ ] **Step 1: Add `--run` to `OPTIONS_WITH_VALUES`**

In `src/cli/shared/args.ts`, in the `OPTIONS_WITH_VALUES` set, add `'--run'` to the last line:

```typescript
  '--activities-zh-json', '--travel-date', '--travel-start', '--travel-end',
  '--run',
]);
```

- [ ] **Step 2: Run typecheck**

Run: `npm run typecheck`
Expected: `✅ Typecheck passed`.

- [ ] **Step 3: Commit**

```bash
git add src/cli/shared/args.ts
git commit -m "feat: register --run as a value-bearing CLI option"
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

Run: `npm run db:exec -- "DELETE FROM stage0_research_durations WHERE run_id='<run_id>'; DELETE FROM stage0_research_destinations WHERE run_id='<run_id>'; DELETE FROM stage0_research_runs WHERE run_id='<run_id>';"`
Expected: exits 0.

- [ ] **Step 6: Commit**

```bash
git add src/cli/commands/stage0.ts src/cli/travel-update.ts
git commit -m "feat: add stage0-init, stage0-compare, stage0-adopt CLI commands"
```

---

## Task 6: Python aggregator — `scripts/stage0_research.py`

**Files:**
- Create: `scripts/stage0_research.py`

The aggregator reads a run from Turso, scrapes each (destination, duration) pair via `scrape_date_range.py` into a temp file, parses results into candidates, computes leave-days, and ranks.

- [ ] **Step 1: Confirm the scrape-result JSON shape**

There is **no** Python Turso helper in this repo (only TypeScript `turso-pipeline.ts` / `turso-exec.ts`), so the aggregator talks to Turso's HTTP pipeline directly for reads (the `turso_query()` function in Step 2) and never writes to Turso — all writes go through `stage0-import` (Task 7) via a transient JSON handoff.

Read `scripts/scrape_date_range.py` to confirm its output JSON shape, which Step 2's parser depends on: `{ scraped_at, params, results: [...] }`. Each result row has `depart_date`, `return_date`, `depart_day`, `return_day`, `combined_cheapest_twd`, and `outbound`/`inbound` objects each containing a `flights` array (whose entries have `airline`, `depart`, `arrive`, `duration`, `nonstop`, `total_usd`).

- [ ] **Step 2: Create the aggregator script**

Create `scripts/stage0_research.py`. It reads the run via the Turso HTTP pipeline (`TURSO_URL` + `TURSO_TOKEN` from `.env`) and hands all DB writes to the `stage0-import` CLI command (Task 7) via a transient JSON file — the aggregator itself performs no Turso writes.

```python
#!/usr/bin/env python3
"""
Stage 0 aggregator — scrapes flight candidates across destination x duration
for one immutable research run, then hands results to the TS CLI for import.

Reads the run from Turso (read-only HTTP pipeline). For each (destination,
duration) pair it invokes scrape_date_range.py into a temp file, parses the
results, and accumulates candidate dicts. The accumulated candidates +
scrape-attempt outcomes are written to a temp JSON file and handed to
`npm run travel -- stage0-import` which performs all DB writes + ranking.

The temp files are transient implementation detail — not durable state.

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


def load_env():
    env_path = os.path.join(PROJECT_ROOT, ".env")
    if not os.path.exists(env_path):
        return
    with open(env_path, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            k, v = line.split("=", 1)
            os.environ.setdefault(k.strip(), v.strip())


def turso_query(sql):
    """Run a read-only SQL query against Turso, return list of row-dicts."""
    import urllib.request
    load_env()
    url = os.environ["TURSO_URL"].rstrip("/")
    token = os.environ["TURSO_TOKEN"]
    # Turso HTTP pipeline endpoint
    endpoint = url.replace("libsql://", "https://") + "/v2/pipeline"
    body = json.dumps({
        "requests": [
            {"type": "execute", "stmt": {"sql": sql}},
            {"type": "close"},
        ]
    }).encode("utf-8")
    req = urllib.request.Request(
        endpoint, data=body,
        headers={"Authorization": f"Bearer {token}",
                 "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req) as resp:
        data = json.loads(resp.read().decode("utf-8"))
    result = data["results"][0]
    if result.get("type") != "ok":
        raise RuntimeError(f"Turso query failed: {result}")
    res = result["response"]["result"]
    cols = [c["name"] for c in res["cols"]]
    rows = []
    for r in res["rows"]:
        rows.append({cols[i]: cell.get("value") for i, cell in enumerate(r)})
    return rows


def load_run(run_id):
    runs = turso_query(
        f"SELECT * FROM stage0_research_runs WHERE run_id = '{run_id}';")
    if not runs:
        print(f"Error: research run not found: {run_id}", file=sys.stderr)
        sys.exit(1)
    run = runs[0]
    run["destinations"] = turso_query(
        f"SELECT * FROM stage0_research_destinations "
        f"WHERE run_id = '{run_id}' ORDER BY sort_order;")
    run["durations"] = turso_query(
        f"SELECT * FROM stage0_research_durations "
        f"WHERE run_id = '{run_id}' ORDER BY nights;")
    return run


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

    all_candidates = []
    attempts = []
    for dest in run["destinations"]:
        for dur in run["durations"]:
            nights = int(dur["nights"])
            duration_days = int(dur["duration_days"])
            label = f"{dest['dest_code']} {nights}n"
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

## Task 7: CLI — `stage0-import` (aggregator handoff → DB)

**Files:**
- Modify: `src/cli/commands/stage0.ts`
- Modify: `src/cli/shared/args.ts` (add `--file` if absent)

`stage0-import` consumes the aggregator's JSON handoff: it inserts candidates (computing leave-days via the TS leave calculator), records scrape attempts, ranks the run, and sets final status. This keeps all DB writes and the leave calculation in TypeScript.

- [ ] **Step 1: Confirm `--file` is a known option**

Run: `grep -n "'--file'" src/cli/shared/args.ts`
Expected: if it prints a match, skip Step 2. If no output, do Step 2.

- [ ] **Step 2: Add `--file` to `OPTIONS_WITH_VALUES` (only if Step 1 found nothing)**

In `src/cli/shared/args.ts`, add `'--file'` next to `'--run'`:

```typescript
  '--run', '--file',
]);
```

- [ ] **Step 3: Add the `stage0-import` command**

In `src/cli/commands/stage0.ts`, add this handler before the `registerCommand(...)` calls:

```typescript
// ── stage0-import ────────────────────────────────────────────────────
// Consumes the Python aggregator's JSON handoff: insert candidates (with
// leave-days computed here via the TS calculator), record scrape attempts,
// rank, set final run status.

const stage0ImportCommand: CommandHandler = {
  names: ['stage0-import'],
  description: 'Import Stage 0 aggregator results from a handoff JSON file.',
  usage: 'stage0-import --run <run_id> --file <path>',
  requiresState: false,
  async execute(ctx: CliContext): Promise<void> {
    const fs = require('fs');
    const {
      insertCandidate, upsertScrapeAttempt, rankRun, setRunStatus, getResearchRun,
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

    if (candidates.length === 0) {
      await setRunStatus(runId, 'failed');
      console.log(`⚠️  No candidates imported for ${runId} — run marked failed.`);
      return;
    }
    await rankRun(runId);
    console.log(`✅ Imported ${candidates.length} candidates for ${runId}, ranked.`);
    console.log(`   View: npm run travel -- stage0-compare --run ${runId}`);
  },
};
```

Then add it to the registration block:

```typescript
registerCommand(stage0InitCommand);
registerCommand(stage0CompareCommand);
registerCommand(stage0AdoptCommand);
registerCommand(stage0ImportCommand);
```

- [ ] **Step 4: Run typecheck**

Run: `npm run typecheck`
Expected: `✅ Typecheck passed`.

- [ ] **Step 5: Smoke-test stage0-import with a hand-built handoff file**

Create a run, then a handoff file, then import:
```bash
npm run travel -- stage0-init --origin TPE --start 2026-06-18 --end 2026-06-18 \
  --dest KIX:"Osaka (KIX)" --nights 6
# note the run id, then:
echo '{"candidates":[{"candidateId":"RUNID-KIX-2026-06-18-6n","runId":"RUNID","destCode":"KIX","departDate":"2026-06-18","returnDate":"2026-06-24","nights":6,"flightTotalTwd":18000,"flights":[]}],"attempts":[{"runId":"RUNID","destCode":"KIX","nights":6,"status":"ok","candidateCount":1,"error":null}]}' > /tmp/s0-handoff.json
# replace RUNID in the file with the real run id, then:
npm run travel -- stage0-import --run <run_id> --file /tmp/s0-handoff.json
npm run travel -- stage0-compare --run <run_id>
```
Expected: import prints `✅ Imported 1 candidates ... ranked`; compare shows one ranked row with a computed leave-days value.

- [ ] **Step 6: Clean up the smoke-test run**

Run: `npm run db:exec -- "DELETE FROM stage0_candidate_flights WHERE candidate_id IN (SELECT candidate_id FROM stage0_candidates WHERE run_id='<run_id>'); DELETE FROM stage0_candidates WHERE run_id='<run_id>'; DELETE FROM stage0_scrape_attempts WHERE run_id='<run_id>'; DELETE FROM stage0_research_durations WHERE run_id='<run_id>'; DELETE FROM stage0_research_destinations WHERE run_id='<run_id>'; DELETE FROM stage0_research_runs WHERE run_id='<run_id>';"`
Expected: exits 0.

- [ ] **Step 7: Commit**

```bash
git add src/cli/commands/stage0.ts src/cli/shared/args.ts
git commit -m "feat: add stage0-import command for aggregator handoff"
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

Orchestration skill for **Stage 0 — Triangle Research** of the proposed
research-first planning flow (`docs/plans/2026-05-22-new-planning-flow.md`).

It explores the three interdependent variables — departure date, destination,
flight price — *together*, before any of them is locked. It does **not**
replace `/p3-flights`: that skill requires P1/P2 to already exist, so it cannot
run pre-lock. `/stage0-research` owns the pre-lock phase and hands off to
`/p1-dates` + `/p2-destination` once the user picks a candidate.

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
   npm run travel -- stage0-adopt <candidate_id> <plan_id>
   ```
   Then invoke `/p1-dates` (set the candidate's depart/return dates) and
   `/p2-destination` (set the destination). The normal P1→P5 flow takes over.

## Notes

- If a (destination, duration) scrape fails, the aggregator records it in
  `stage0_scrape_attempts` and continues. Re-running the aggregator on the
  same run retries only failed/pending attempts.
- The proposed planning flow is still **Proposed** — this skill is the Stage 0
  capability, but P1→P5 and CLAUDE.md's Skill Decision Tree remain the
  operative flow.
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
Expected: all tests pass, including the 4 `stage0-service` tests.

- [ ] **Step 2: Run typecheck**

Run: `npm run typecheck`
Expected: `✅ Typecheck passed`.

- [ ] **Step 3: Run the doctor**

Run: `npm run doctor`
Expected: full health check passes (0 errors).

- [ ] **Step 4: Verify `stage0-init` appears in CLI help**

Run: `npm run travel -- help`
Expected: help text lists `stage0-init`, `stage0-compare`, `stage0-adopt`, `stage0-import`.

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

**Mechanism decision resolved:** the spec left "is run-creation a `stage0-init` command or skill-direct" open. This plan resolves it: `stage0-init` is a CLI command (Task 5), and a `stage0-import` command (Task 7) owns the aggregator→DB handoff so all DB writes + the leave-days calculation stay in TypeScript. The Python aggregator never writes to Turso directly — it only reads, then hands off a transient JSON file.

**Type consistency:** `CreateRunInput`, `ResearchRun`, `Candidate`, `InsertCandidateInput`, `CandidateFlight`, `ScrapeAttempt`, `ScrapeAttemptInput` defined in Task 2/3 and used consistently by Tasks 5 and 7. Function names (`createResearchRun`, `getResearchRun`, `insertCandidate`, `getCandidates`, `rankRun`, `upsertScrapeAttempt`, `getScrapeAttempts`, `setRunStatus`, `adoptCandidate`, `deleteResearchRun`) are stable across all tasks.
