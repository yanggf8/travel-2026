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
