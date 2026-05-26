import { describe, it, expect, beforeEach } from 'vitest';
import { spawnSync } from 'child_process';
import { rowsToObjects, sqlText } from '../../src/state/sql-helpers';
import { insertTourGroupOffers, getTursoClient } from '../../src/services/tour-group-service';

const RUN = 'test-run-baseline-001';
const CLI = ['ts-node', 'src/cli/travel-update.ts'];

function runCli(cmdArgs: string[]): { stdout: string; stderr: string; code: number } {
  const r = spawnSync('npx', [...CLI, ...cmdArgs], { encoding: 'utf-8' });
  return { stdout: r.stdout, stderr: r.stderr, code: r.status ?? -1 };
}

async function clear() {
  const c = getTursoClient();
  await c.execute(`DELETE FROM stage0_tour_group_offers WHERE run_id = ${sqlText(RUN)}`);
  await c.execute(`DELETE FROM stage0_research_runs WHERE run_id = ${sqlText(RUN)}`);
}

async function seedRun() {
  const c = getTursoClient();
  // Minimal research run so the command can resolve it
  await c.execute(`INSERT INTO stage0_research_runs
    (run_id, origin_code, pax, window_start, window_end, currency, exchange_rate_usd_twd, status, created_at, updated_at)
    VALUES (${sqlText(RUN)}, 'TPE', 2, '2026-06-12', '2026-06-25', 'TWD', 32.0, 'open', '2026-05-27T00:00:00Z', '2026-05-27T00:00:00Z')`);
}

function offer(over: any) {
  return {
    run_id: RUN, source_id: 'besttour', dest_region: 'okinawa',
    depart_date: '2026-06-18', return_date: '2026-06-21', nights: 3,
    title: 'test', url: 'http://test/', scraped_at: '2026-05-27T00:00:00Z',
    ...over,
  };
}

describe('stage0-baseline CLI', () => {
  beforeEach(async () => {
    await clear();
    await seedRun();
  });

  it('errors when run_id is missing', () => {
    const { code, stderr } = runCli(['stage0-baseline']);
    expect(code).not.toBe(0);
    expect(stderr).toMatch(/requires --run|--run/i);
  });

  it('errors when run does not exist', () => {
    const { code, stderr } = runCli(['stage0-baseline', '--run', 'no-such-run']);
    expect(code).not.toBe(0);
    expect(stderr).toMatch(/not found/i);
  });

  it('shows empty result cleanly when no offers exist', () => {
    const { code, stdout } = runCli(['stage0-baseline', '--run', RUN]);
    expect(code, stdout).toBe(0);
    expect(stdout).toMatch(/no offers|no tour-group/i);
  });

  it('groups by product_kind and shows cheapest per (date, kind)', async () => {
    await insertTourGroupOffers([
      // group_tour rows
      offer({ offer_id: 'gt1', product_kind: 'group_tour', price_per_person_twd: 21999 }),
      offer({ offer_id: 'gt2', product_kind: 'group_tour', price_per_person_twd: 26900,
              depart_date: '2026-06-21', return_date: '2026-06-24' }),
      offer({ offer_id: 'gt3', product_kind: 'group_tour', price_per_person_twd: 25000 }),
      // fit rows
      offer({ offer_id: 'fit1', product_kind: 'fit', price_per_person_twd: 11900,
              raw_json: '{"confidence":"high","source":"funtime_aggregator"}' }),
      offer({ offer_id: 'fit2', product_kind: 'fit', price_per_person_twd: 12999,
              depart_date: '2026-06-21', return_date: '2026-06-24',
              raw_json: '{"confidence":"high","source":"funtime_aggregator"}' }),
    ]);

    const { code, stdout } = runCli(['stage0-baseline', '--run', RUN]);
    expect(code, stdout).toBe(0);
    expect(stdout).toMatch(/baseline/i);
    expect(stdout).toContain('11,900'); // cheapest FIT on 6/18 surfaces
    expect(stdout).toContain('21,999'); // cheapest group_tour on 6/18 surfaces
    expect(stdout).toContain('12,999'); // cheapest FIT on 6/21
    expect(stdout).toContain('26,900'); // cheapest group_tour on 6/21
    // The 25,000 group_tour on 6/18 should NOT be a per-date pillar (21,999 wins)
    // but still visible as part of the count (3 group_tour on 6/18)
    expect(stdout).toMatch(/okinawa/i);
  });

  it('computes savings when both fit and group_tour exist for the same date', async () => {
    await insertTourGroupOffers([
      offer({ offer_id: 'gt1', product_kind: 'group_tour', price_per_person_twd: 21999 }),
      offer({ offer_id: 'fit1', product_kind: 'fit', price_per_person_twd: 11900,
              raw_json: '{"confidence":"high"}' }),
    ]);
    const { stdout } = runCli(['stage0-baseline', '--run', RUN]);
    // Savings = 21999 - 11900 = 10099
    expect(stdout).toMatch(/10,?099|savings|cheaper/i);
  });

  it('filters by --dest-region', async () => {
    await insertTourGroupOffers([
      offer({ offer_id: 'oka1', dest_region: 'okinawa', product_kind: 'fit', price_per_person_twd: 11900 }),
      offer({ offer_id: 'kan1', dest_region: 'kansai', product_kind: 'fit', price_per_person_twd: 22888 }),
    ]);
    const { stdout } = runCli(['stage0-baseline', '--run', RUN, '--dest-region', 'okinawa']);
    expect(stdout).toContain('11,900');
    expect(stdout).not.toContain('22,888');
  });

  it('--json emits structured data', async () => {
    await insertTourGroupOffers([
      offer({ offer_id: 'gt1', product_kind: 'group_tour', price_per_person_twd: 21999 }),
      offer({ offer_id: 'fit1', product_kind: 'fit', price_per_person_twd: 11900,
              raw_json: '{"confidence":"high"}' }),
    ]);
    const { stdout } = runCli(['stage0-baseline', '--run', RUN, '--json']);
    const parsed = JSON.parse(stdout);
    expect(parsed.run_id).toBe(RUN);
    expect(Array.isArray(parsed.baseline_rows)).toBe(true);
    const row = parsed.baseline_rows.find((r: any) =>
      r.depart_date === '2026-06-18' && r.dest_region === 'okinawa'
    );
    expect(row).toBeTruthy();
    expect(Number(row.cheapest_group_tour_twd)).toBe(21999);
    expect(Number(row.cheapest_fit_twd)).toBe(11900);
    expect(Number(row.fit_savings_vs_group_tour_twd)).toBe(10099);
  });
});
