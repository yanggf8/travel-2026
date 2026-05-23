/**
 * Stage 0 Service — all DB reads/writes for the triangle-research domain.
 *
 * Stage 0 tables are unscoped (keyed by run_id, not plan_id) — research
 * exists before any plan exists. Runs are IMMUTABLE: research inputs are
 * written once at creation and never edited; changing an input means a new
 * run. See docs/superpowers/specs/2026-05-22-stage0-triangle-research-design.md
 */

import path from 'node:path';
import { sqlText, sqlInt, sqlReal, rowsToObjects, rowsToObjectsAt } from '../state/sql-helpers';

// Scripts live outside src/ (rootDir), so use the same dynamic require pattern
// as the existing Turso services to avoid TS6059 errors.
function getProjectRoot(): string {
  return path.resolve(__dirname, '..', '..');
}

function requirePipeline(): { TursoPipelineClient: new (opts?: any) => any } {
  return require(path.join(getProjectRoot(), 'scripts', 'turso-pipeline.ts'));
}

function newClient(): any {
  const { TursoPipelineClient } = requirePipeline();
  return new TursoPipelineClient();
}

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
  const client = newClient();
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
  const client = newClient();
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
  const client = newClient();
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
  const client = newClient();
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
  const client = newClient();
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
  const client = newClient();
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
  const client = newClient();
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
  const client = newClient();
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
  const client = newClient();
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
  const client = newClient();
  await client.execute(
    `UPDATE stage0_research_runs SET status = ${sqlText(status)},
       updated_at = ${sqlText(nowIso())} WHERE run_id = ${sqlText(runId)};`
  );
}

export async function adoptCandidate(candidateId: string, planId: string): Promise<void> {
  const client = newClient();
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
