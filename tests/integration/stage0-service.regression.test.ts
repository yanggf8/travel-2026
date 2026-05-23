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
