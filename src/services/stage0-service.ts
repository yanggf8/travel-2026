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
