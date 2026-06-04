import path from 'node:path';
import { describe, expect, it, afterAll } from 'vitest';
import {
  createResearchRun,
  getResearchRun,
  getResearchShaping,
  getScrapeAttempts,
  deleteResearchRun,
  insertCandidate,
  upsertScrapeAttempt,
  rankRun,
  getCandidates,
  setRunStatus,
  adoptCandidate,
  adoptCandidateToNewPlan,
  deleteCandidatesForPair,
  type CreateRunInput,
} from '../../src/services/shaping-service';
import { rowsToObjects, sqlText } from '../../src/state/sql-helpers';

const TEST_RUN_IDS: string[] = [];
const TEST_PLAN_IDS: string[] = [];

function newClient(): any {
  const { TursoPipelineClient } = require(path.join(process.cwd(), 'scripts', 'turso-pipeline.ts'));
  return new TursoPipelineClient();
}

function uniqueRunId(): string {
  const id = `shaping-test-${Date.now()}-${Math.floor(Math.random() * 10000)}`;
  TEST_RUN_IDS.push(id);
  return id;
}

function uniquePlanId(): string {
  const id = `shaping-test-plan-${Date.now()}-${Math.floor(Math.random() * 10000)}`;
  TEST_PLAN_IDS.push(id);
  return id;
}

async function deletePlanRows(planId: string): Promise<void> {
  const client = newClient();
  const p = sqlText(planId);
  await client.executeMany([
    `DELETE FROM event_log_process_events WHERE plan_id = ${p};`,
    `DELETE FROM event_log_dest_processes WHERE plan_id = ${p};`,
    `DELETE FROM event_log_destinations WHERE plan_id = ${p};`,
    `DELETE FROM event_log_state WHERE plan_id = ${p};`,
    `DELETE FROM process_statuses WHERE plan_id = ${p};`,
    `DELETE FROM date_anchors WHERE plan_id = ${p};`,
    `DELETE FROM destination_cities WHERE plan_id = ${p};`,
    `DELETE FROM destination_details WHERE plan_id = ${p};`,
    `DELETE FROM plan_destinations WHERE plan_id = ${p};`,
    `DELETE FROM plan_metadata WHERE plan_id = ${p};`,
    `DELETE FROM plans WHERE plan_id = ${p};`,
  ]);
}

afterAll(async () => {
  for (const id of TEST_PLAN_IDS) {
    await deletePlanRows(id);
  }
  for (const id of TEST_RUN_IDS) {
    await deleteResearchRun(id);
  }
});

describe('Shaping Stage service — run creation', () => {
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

  it('creates a run with shaping rules (date + channel + mobility) and reads them back', async () => {
    const runId = uniqueRunId();
    await createResearchRun({
      runId,
      originCode: 'TPE',
      pax: 2,
      windowStart: '2026-06-12',
      windowEnd: '2026-06-25',
      exchangeRateUsdTwd: 32,
      destinations: [{ destCode: 'OKA', destLabel: 'Okinawa (OKA)' }],
      durations: [{ nights: 3 }],
      shaping: [
        { aspect: 'date', role: 'hard_constraint', kind: 'return_no_later_than', value_date: '2026-06-27' },
        { aspect: 'date', role: 'hard_constraint', kind: 'exclude_depart', value_date: '2026-06-28', notes: 'Liko 馬偕' },
        { aspect: 'channel', role: 'hard_constraint', kind: 'exclude_source', value_text: 'kkday' },
        { aspect: 'mobility', role: 'hard_constraint', kind: 'no_car', value_text: 'true' },
      ],
    });
    TEST_RUN_IDS.push(runId);

    const run = await getResearchRun(runId);
    expect(run).not.toBeNull();
    expect(run!.shaping).toHaveLength(4);
    expect(run!.shaping.some(s => s.kind === 'return_no_later_than')).toBe(true);

    const direct = await getResearchShaping(runId);
    expect(direct).toHaveLength(4);

    // Cleanup already handled by afterAll via TEST_RUN_IDS
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

describe('Shaping Stage service — candidates and ranking', () => {
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

  it('adopts a candidate into a newly seeded plan with locked P1/P2 rows', async () => {
    const runId = uniqueRunId();
    const planId = uniquePlanId();
    await createResearchRun({
      runId, originCode: 'TPE', pax: 2,
      windowStart: '2026-06-18', windowEnd: '2026-06-20', exchangeRateUsdTwd: 32,
      destinations: [{ destCode: 'KIX', destLabel: 'Osaka/Kyoto (KIX)' }],
      durations: [{ nights: 6 }],
    });
    const candidateId = `${runId}-KIX-2026-06-18-6n`;
    await insertCandidate({
      candidateId, runId, destCode: 'KIX',
      departDate: '2026-06-18', returnDate: '2026-06-24', nights: 6,
      flightTotalTwd: 18000, leaveDays: 3, verdict: null, flights: [],
    });

    await adoptCandidateToNewPlan({
      candidateId,
      planId,
      destinationSlug: 'osaka_kyoto_2026',
    });

    const client = newClient();
    const res = await client.executeBatch([
      `SELECT active_destination FROM plan_metadata WHERE plan_id = ${sqlText(planId)};`,
      `SELECT start_date, end_date, days FROM date_anchors WHERE plan_id = ${sqlText(planId)} AND destination = ${sqlText('osaka_kyoto_2026')};`,
      `SELECT process_id, status FROM process_statuses WHERE plan_id = ${sqlText(planId)} AND destination = ${sqlText('osaka_kyoto_2026')} ORDER BY process_id;`,
      `SELECT primary_airport FROM destination_details WHERE plan_id = ${sqlText(planId)} AND destination = ${sqlText('osaka_kyoto_2026')};`,
    ]);
    expect(rowsToObjects(res.results?.[0] ? { results: [res.results[0]] } : res)[0].active_destination).toBe('osaka_kyoto_2026');
    const dateRows = rowsToObjects(res.results?.[1] ? { results: [res.results[1]] } : res);
    expect(dateRows[0]).toMatchObject({ start_date: '2026-06-18', end_date: '2026-06-24' });
    expect(Number(dateRows[0].days)).toBe(7);
    const statuses = rowsToObjects(res.results?.[2] ? { results: [res.results[2]] } : res);
    expect(statuses).toEqual([
      { process_id: 'process_1_date_anchor', status: 'confirmed' },
      { process_id: 'process_2_destination', status: 'confirmed' },
      { process_id: 'process_3_4_packages', status: 'pending' },
      { process_id: 'process_3_transportation', status: 'pending' },
      { process_id: 'process_4_accommodation', status: 'pending' },
      { process_id: 'process_5_daily_itinerary', status: 'pending' },
    ]);
    const detailRows = rowsToObjects(res.results?.[3] ? { results: [res.results[3]] } : res);
    expect(detailRows[0].primary_airport).toBe('KIX');

    const cands = await getCandidates(runId);
    expect(cands[0].adopted_plan_id).toBe(planId);
    const run = await getResearchRun(runId);
    expect(run!.status).toBe('adopted');
  });

  it('rejects create-plan adoption when destination slug airports do not match the candidate', async () => {
    const runId = uniqueRunId();
    const planId = uniquePlanId();
    await createResearchRun({
      runId, originCode: 'TPE', pax: 2,
      windowStart: '2026-06-18', windowEnd: '2026-06-20', exchangeRateUsdTwd: 32,
      destinations: [{ destCode: 'KIX', destLabel: 'Osaka/Kyoto (KIX)' }],
      durations: [{ nights: 6 }],
    });
    const candidateId = `${runId}-KIX-2026-06-18-6n`;
    await insertCandidate({
      candidateId, runId, destCode: 'KIX',
      departDate: '2026-06-18', returnDate: '2026-06-24', nights: 6,
      flightTotalTwd: 18000, leaveDays: 3, verdict: null, flights: [],
    });

    await expect(adoptCandidateToNewPlan({
      candidateId,
      planId,
      destinationSlug: 'tokyo_2026',
    })).rejects.toThrow(/does not match destination tokyo_2026/);

    const client = newClient();
    const res = await client.execute(
      `SELECT COUNT(*) AS n FROM plan_metadata WHERE plan_id = ${sqlText(planId)};`
    );
    expect(Number(rowsToObjects(res)[0].n)).toBe(0);
  });
});
