/**
 * Shaping Stage Service — all DB reads/writes for the triangle-research domain.
 *
 * Shaping Stage tables are unscoped (keyed by run_id, not plan_id) — research
 * exists before any plan exists. Runs are IMMUTABLE: research inputs are
 * written once at creation and never edited; changing an input means a new
 * run. See docs/superpowers/specs/2026-05-22-shaping-triangle-research-design.md
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
  shaping?: Array<{
    aspect: string;
    role: string;           // 'hard_constraint' | 'soft_preference' | 'search_directive' | ...
    kind: string;
    value_text?: string | null;
    value_date?: string | null;
    value_integer?: number | null;
    notes?: string | null;
  }>;
}

export interface ResearchShaping {
  aspect: string;
  role: string;
  kind: string;
  value_text: string | null;
  value_date: string | null;
  value_integer: number | null;
  notes: string | null;
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
  shaping: ResearchShaping[];
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
    `INSERT INTO shaping_research_runs
      (run_id, origin_code, pax, window_start, window_end, currency,
       exchange_rate_usd_twd, status, created_at, updated_at)
     VALUES (${sqlText(input.runId)}, ${sqlText(input.originCode)}, ${sqlInt(input.pax)},
       ${sqlText(input.windowStart)}, ${sqlText(input.windowEnd)}, ${sqlText('TWD')},
       ${sqlReal(input.exchangeRateUsdTwd)}, ${sqlText('started')}, ${sqlText(ts)}, ${sqlText(ts)});`
  );

  input.destinations.forEach((d, i) => {
    stmts.push(
      `INSERT INTO shaping_research_destinations (run_id, dest_code, dest_label, sort_order)
       VALUES (${sqlText(input.runId)}, ${sqlText(d.destCode)}, ${sqlText(d.destLabel)}, ${sqlInt(i)});`
    );
  });

  for (const dur of input.durations) {
    stmts.push(
      `INSERT INTO shaping_research_durations (run_id, nights, duration_days)
       VALUES (${sqlText(input.runId)}, ${sqlInt(dur.nights)}, ${sqlInt(dur.nights + 1)});`
    );
  }

  // Research shaping (normalized, no JSON). Includes hard constraints, soft preferences,
  // search directives, etc. "hard_constraint" is one role among others.
  if (input.shaping && input.shaping.length > 0) {
    for (const s of input.shaping) {
      stmts.push(
        `INSERT INTO shaping_rules
           (run_id, aspect, role, kind, value_text, value_date, value_integer, notes, created_at)
         VALUES (${sqlText(input.runId)}, ${sqlText(s.aspect)}, ${sqlText(s.role)}, ${sqlText(s.kind)},
           ${sqlText(s.value_text ?? null)}, ${sqlText(s.value_date ?? null)},
           ${sqlInt(s.value_integer ?? null)}, ${sqlText(s.notes ?? null)}, ${sqlText(ts)});`
      );
    }
  }

  // Seed one 'pending' scrape-attempt row per destination x duration. This
  // makes the full work matrix visible in the DB before scraping starts, so
  // a mid-run crash still shows what was attempted, and the aggregator can
  // skip already-'ok' pairs on a re-run.
  for (const d of input.destinations) {
    for (const dur of input.durations) {
      stmts.push(
        `INSERT INTO shaping_scrape_attempts
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
    `SELECT * FROM shaping_research_runs WHERE run_id = ${sqlText(runId)};`,
    `SELECT * FROM shaping_research_destinations WHERE run_id = ${sqlText(runId)} ORDER BY sort_order;`,
    `SELECT * FROM shaping_research_durations WHERE run_id = ${sqlText(runId)} ORDER BY nights;`,
    `SELECT * FROM shaping_rules WHERE run_id = ${sqlText(runId)} ORDER BY aspect, role, kind;`,
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
    shaping: rowsToObjectsAt(res, 3).map((s) => ({
      aspect: s.aspect,
      role: s.role,
      kind: s.kind,
      value_text: s.value_text ?? null,
      value_date: s.value_date ?? null,
      value_integer: s.value_integer == null ? null : Number(s.value_integer),
      notes: s.notes ?? null,
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

export async function getResearchShaping(runId: string): Promise<ResearchShaping[]> {
  const client = newClient();
  const res = await client.execute(
    `SELECT * FROM shaping_rules WHERE run_id = ${sqlText(runId)}
     ORDER BY aspect, role, kind;`
  );
  return rowsToObjects(res).map((s) => ({
    aspect: s.aspect,
    role: s.role,
    kind: s.kind,
    value_text: s.value_text ?? null,
    value_date: s.value_date ?? null,
    value_integer: s.value_integer == null ? null : Number(s.value_integer),
    notes: s.notes ?? null,
  }));
}

export async function getScrapeAttempts(runId: string): Promise<ScrapeAttempt[]> {
  const client = newClient();
  const res = await client.execute(
    `SELECT * FROM shaping_scrape_attempts WHERE run_id = ${sqlText(runId)}
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
    `DELETE FROM shaping_candidate_flights WHERE candidate_id IN
      (SELECT candidate_id FROM shaping_candidates WHERE run_id = ${r});`,
    `DELETE FROM shaping_candidates WHERE run_id = ${r};`,
    `DELETE FROM shaping_scrape_attempts WHERE run_id = ${r};`,
    `DELETE FROM shaping_research_durations WHERE run_id = ${r};`,
    `DELETE FROM shaping_research_destinations WHERE run_id = ${r};`,
    `DELETE FROM shaping_rules WHERE run_id = ${r};`,
    `DELETE FROM shaping_research_runs WHERE run_id = ${r};`,
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

export interface AdoptToNewPlanInput {
  candidateId: string;
  planId: string;
  destinationSlug: string;
  schemaVersion?: string;
}

export async function insertCandidate(input: InsertCandidateInput): Promise<void> {
  const client = newClient();
  const stmts: string[] = [];
  stmts.push(
    `INSERT INTO shaping_candidates
      (candidate_id, run_id, dest_code, depart_date, return_date, nights,
       flight_total_twd, leave_days, rank, verdict, adopted_plan_id)
     VALUES (${sqlText(input.candidateId)}, ${sqlText(input.runId)}, ${sqlText(input.destCode)},
       ${sqlText(input.departDate)}, ${sqlText(input.returnDate)}, ${sqlInt(input.nights)},
       ${sqlInt(input.flightTotalTwd)}, ${sqlInt(input.leaveDays)}, NULL,
       ${sqlText(input.verdict)}, NULL);`
  );
  for (const f of input.flights) {
    stmts.push(
      `INSERT INTO shaping_candidate_flights
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
    `SELECT * FROM shaping_candidates WHERE run_id = ${sqlText(runId)}
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
     FROM shaping_candidates WHERE run_id = ${sqlText(runId)};`
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
      `UPDATE shaping_candidates SET rank = ${sqlInt(i + 1)}
       WHERE candidate_id = ${sqlText(r.candidate_id)};`
  );
  stmts.push(
    `UPDATE shaping_research_runs SET status = ${sqlText('ranked')},
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
    `INSERT OR REPLACE INTO shaping_scrape_attempts
      (run_id, dest_code, nights, status, candidate_count, error, attempted_at)
     VALUES (${sqlText(input.runId)}, ${sqlText(input.destCode)}, ${sqlInt(input.nights)},
       ${sqlText(input.status)}, ${sqlInt(input.candidateCount ?? null)},
       ${sqlText(input.error ?? null)}, ${sqlText(nowIso())});`
  );
}

// ── candidate replacement (idempotent re-import for one pair) ─────────
// Deletes any existing candidates + their flights for one (run, dest,
// nights) pair so a re-import of that pair does not collide on the
// candidate_id PK. Used by shaping-import before inserting.

export async function deleteCandidatesForPair(
  runId: string, destCode: string, nights: number
): Promise<void> {
  const client = newClient();
  const where =
    `run_id = ${sqlText(runId)} AND dest_code = ${sqlText(destCode)} ` +
    `AND nights = ${sqlInt(nights)}`;
  await client.executeMany([
    `DELETE FROM shaping_candidate_flights WHERE candidate_id IN
      (SELECT candidate_id FROM shaping_candidates WHERE ${where});`,
    `DELETE FROM shaping_candidates WHERE ${where};`,
  ]);
}

// ── status + adopt ───────────────────────────────────────────────────

export async function setRunStatus(runId: string, status: string): Promise<void> {
  const client = newClient();
  await client.execute(
    `UPDATE shaping_research_runs SET status = ${sqlText(status)},
       updated_at = ${sqlText(nowIso())} WHERE run_id = ${sqlText(runId)};`
  );
}

export async function adoptCandidate(candidateId: string, planId: string): Promise<void> {
  const client = newClient();
  // Find the run, set the pointer, mark the run adopted — one batch.
  const res = await client.execute(
    `SELECT run_id FROM shaping_candidates WHERE candidate_id = ${sqlText(candidateId)};`
  );
  const rows = rowsToObjects(res);
  if (rows.length === 0) {
    throw new Error(`Shaping Stage candidate not found: ${candidateId}`);
  }
  const runId = rows[0].run_id as string;
  await client.executeMany([
    `UPDATE shaping_candidates SET adopted_plan_id = ${sqlText(planId)}
      WHERE candidate_id = ${sqlText(candidateId)};`,
    `UPDATE shaping_research_runs SET status = ${sqlText('adopted')},
      updated_at = ${sqlText(nowIso())} WHERE run_id = ${sqlText(runId)};`,
  ]);
}

export async function adoptCandidateToNewPlan(input: AdoptToNewPlanInput): Promise<void> {
  const client = newClient();
  const schemaVersion = input.schemaVersion ?? '4.2.0';
  const ts = nowIso();

  const res = await client.executeBatch([
    `SELECT c.*, r.origin_code
     FROM shaping_candidates c
     JOIN shaping_research_runs r ON r.run_id = c.run_id
     WHERE c.candidate_id = ${sqlText(input.candidateId)};`,
    `SELECT plan_id FROM plans WHERE plan_id = ${sqlText(input.planId)}
     UNION
     SELECT plan_id FROM plan_metadata WHERE plan_id = ${sqlText(input.planId)};`,
    `SELECT * FROM destination_config WHERE slug = ${sqlText(input.destinationSlug)};`,
    `SELECT * FROM shaping_rules WHERE run_id = (SELECT run_id FROM shaping_candidates WHERE candidate_id = ${sqlText(input.candidateId)}) ORDER BY aspect, role, kind;`,
    `SELECT airport FROM destination_airports WHERE slug = ${sqlText(input.destinationSlug)} ORDER BY sort_order;`,
  ]);
  const candidateRows = rowsToObjectsAt(res, 0);
  if (candidateRows.length === 0) {
    throw new Error(`Shaping Stage candidate not found: ${input.candidateId}`);
  }
  const existingPlanRows = rowsToObjectsAt(res, 1);
  if (existingPlanRows.length > 0) {
    throw new Error(`Plan already exists: ${input.planId}`);
  }
  const destRows = rowsToObjectsAt(res, 2);
  if (destRows.length === 0) {
    throw new Error(`Destination config not found: ${input.destinationSlug}`);
  }

  const candidate = candidateRows[0];
  const destConfig = destRows[0];
  const runId = candidate.run_id as string;
  const shapingRows = rowsToObjectsAt(res, 3); // shaping from the research run
  const startDate = candidate.depart_date as string;
  const endDate = candidate.return_date as string;
  const nights = Number(candidate.nights);
  const days = nights + 1;
  const destinationDisplayName = (destConfig.display_name as string | null) ?? input.destinationSlug;
  const region = (destConfig.ref_id as string | null) ?? input.destinationSlug;
  const originCode = (candidate.origin_code as string | null) ?? null;
  const primaryAirport = candidate.dest_code as string;
  const configuredAirports = rowsToObjectsAt(res, 4)
    .map((r) => String(r.airport).toUpperCase());
  if (configuredAirports.length > 0 && !configuredAirports.includes(primaryAirport.toUpperCase())) {
    throw new Error(
      `Candidate destination ${primaryAirport} does not match destination ${input.destinationSlug} ` +
      `(configured airports: ${configuredAirports.join(', ')})`
    );
  }

  await client.executeMany([
    `INSERT INTO plans (plan_id, schema_version, updated_at)
     VALUES (${sqlText(input.planId)}, ${sqlText(schemaVersion)}, datetime('now'));`,
    `INSERT INTO plan_metadata (plan_id, schema_version, active_destination, updated_at)
     VALUES (${sqlText(input.planId)}, ${sqlText(schemaVersion)}, ${sqlText(input.destinationSlug)}, datetime('now'));`,
    `INSERT INTO plan_destinations (plan_id, slug, display_name, status, created_at, updated_at)
     VALUES (${sqlText(input.planId)}, ${sqlText(input.destinationSlug)}, ${sqlText(destinationDisplayName)},
       ${sqlText('active')}, ${sqlText(ts)}, ${sqlText(ts)});`,
    `INSERT INTO destination_details (plan_id, destination, origin_city, region, primary_airport, updated_at)
     VALUES (${sqlText(input.planId)}, ${sqlText(input.destinationSlug)}, ${sqlText(originCode)},
       ${sqlText(region)}, ${sqlText(primaryAirport)}, datetime('now'));`,
    `INSERT INTO destination_cities (plan_id, destination, city_slug, display_name, role, nights, updated_at)
     VALUES (${sqlText(input.planId)}, ${sqlText(input.destinationSlug)}, ${sqlText(input.destinationSlug)},
       ${sqlText(destinationDisplayName)}, ${sqlText('primary')}, ${sqlInt(nights)}, datetime('now'));`,
    `INSERT INTO date_anchors (plan_id, destination, start_date, end_date, days, updated_at)
     VALUES (${sqlText(input.planId)}, ${sqlText(input.destinationSlug)}, ${sqlText(startDate)},
       ${sqlText(endDate)}, ${sqlInt(days)}, datetime('now'));`,
    `INSERT INTO process_statuses (plan_id, destination, process_id, status, updated_at)
     VALUES (${sqlText(input.planId)}, ${sqlText(input.destinationSlug)}, ${sqlText('process_1_date_anchor')},
       ${sqlText('confirmed')}, datetime('now'));`,
    `INSERT INTO process_statuses (plan_id, destination, process_id, status, updated_at)
     VALUES (${sqlText(input.planId)}, ${sqlText(input.destinationSlug)}, ${sqlText('process_2_destination')},
       ${sqlText('confirmed')}, datetime('now'));`,
    `INSERT INTO process_statuses (plan_id, destination, process_id, status, updated_at)
     VALUES (${sqlText(input.planId)}, ${sqlText(input.destinationSlug)}, ${sqlText('process_3_transportation')},
       ${sqlText('pending')}, datetime('now'));`,
    `INSERT INTO process_statuses (plan_id, destination, process_id, status, updated_at)
     VALUES (${sqlText(input.planId)}, ${sqlText(input.destinationSlug)}, ${sqlText('process_3_4_packages')},
       ${sqlText('pending')}, datetime('now'));`,
    `INSERT INTO process_statuses (plan_id, destination, process_id, status, updated_at)
     VALUES (${sqlText(input.planId)}, ${sqlText(input.destinationSlug)}, ${sqlText('process_4_accommodation')},
       ${sqlText('pending')}, datetime('now'));`,
    `INSERT INTO process_statuses (plan_id, destination, process_id, status, updated_at)
     VALUES (${sqlText(input.planId)}, ${sqlText(input.destinationSlug)}, ${sqlText('process_5_daily_itinerary')},
       ${sqlText('pending')}, datetime('now'));`,
    `INSERT INTO event_log_state (plan_id, session, project, version, current_focus, active_destination)
     VALUES (${sqlText(input.planId)}, ${sqlText(ts.slice(0, 10))}, ${sqlText('japan-travel')},
       ${sqlText('3.0')}, ${sqlText('')}, ${sqlText(input.destinationSlug)});`,
    `INSERT INTO event_log_destinations (plan_id, destination, status)
     VALUES (${sqlText(input.planId)}, ${sqlText(input.destinationSlug)}, ${sqlText('active')});`,
    // shaping_candidate_adopted event → plan_events (timeline) + plan_event_data KV
    // (no JSON column). The nested shaping_summary is flattened to a text value.
    `INSERT INTO plan_events (plan_id, scope, destination, process_id, sort_order, event, event_at)
     VALUES (${sqlText(input.planId)}, ${sqlText('timeline')}, ${sqlText('')}, ${sqlText('')}, 0,
       ${sqlText('shaping_candidate_adopted')}, ${sqlText(ts)});`,
    ...([
      ['candidate_id', input.candidateId],
      ['run_id', runId],
      ['depart_date', startDate],
      ['return_date', endDate],
      ['dest_code', primaryAirport],
      ['shaping_count', String(shapingRows.length)],
      ['shaping_summary', shapingRows.map((s: any) =>
        `${s.aspect}/${s.role}/${s.kind}=${s.value_date || s.value_text || s.value_integer}`
      ).join('; ')],
    ] as Array<[string, string]>).map(([k, v]) =>
      `INSERT INTO plan_event_data (plan_id, scope, destination, process_id, sort_order, key, value)
       VALUES (${sqlText(input.planId)}, ${sqlText('timeline')}, ${sqlText('')}, ${sqlText('')}, 0, ${sqlText(k)}, ${sqlText(v)});`
    ),
    `UPDATE shaping_candidates SET adopted_plan_id = ${sqlText(input.planId)}
      WHERE candidate_id = ${sqlText(input.candidateId)};`,
    `UPDATE shaping_research_runs SET status = ${sqlText('adopted')},
      updated_at = ${sqlText(ts)} WHERE run_id = ${sqlText(runId)};`,
  ]);

  // Bridge tour-group baseline offers into the freshly created plan.
  // Region is derived from destination_config.ref_id (the canonical region
  // vocabulary already used by compare-offers --region <name>). Non-fatal:
  // if no tour-group offers were scraped for this run/region/nights, the
  // bridge no-ops and we move on.
  try {
    const { bridgeAuditSet } = require('./tour-group-bridge');
    const audit = await bridgeAuditSet({
      run_id: runId,
      plan_id: input.planId,
      destination: input.destinationSlug,
      dest_region: region,
      nights,
    });
    if (audit.length > 0) {
      console.error(`ℹ️  Bridged ${audit.length} tour-group baseline offers into ${input.planId}.`);
    }
  } catch (err: any) {
    console.error(`⚠️  Tour-group bridge skipped: ${err?.message || err}`);
  }
}
