import { describe, it, expect, beforeEach } from 'vitest';
import { spawnSync } from 'child_process';
import * as path from 'path';
import { getTursoClient } from '../../src/services/tour-group-service';
import { rowsToObjects } from '../../src/state/sql-helpers';

const FIXTURE_DIR = path.resolve(__dirname, '../fixtures/tour-group');
const CLI = ['ts-node', 'src/cli/travel-update.ts'];

function runCli(args: string[]): { stdout: string; stderr: string; code: number } {
  const r = spawnSync('npx', [...CLI, ...args], { encoding: 'utf-8' });
  return { stdout: r.stdout, stderr: r.stderr, code: r.status ?? -1 };
}

async function clearRun(run_id: string) {
  const c = getTursoClient();
  await c.execute(`DELETE FROM stage0_tour_group_offers WHERE run_id = ${sqlText(run_id)}`);
  await c.execute(`DELETE FROM stage0_tour_group_scrape_attempts WHERE run_id = ${sqlText(run_id)}`);
}

function sqlText(v: string): string {
  return `'${v.replace(/'/g, "''")}'`;
}

async function seedPendingAttempt(run_id: string, source_id: string, dest_region: string, nights: number) {
  const c = getTursoClient();
  await c.execute(`INSERT OR REPLACE INTO stage0_tour_group_scrape_attempts
    (run_id, source_id, dest_region, nights, status) VALUES (${sqlText(run_id)}, ${sqlText(source_id)}, ${sqlText(dest_region)}, ${nights}, 'pending')`);
}

describe('import-tour-group-offers', () => {
  beforeEach(async () => {
    await clearRun('test-run-tg-001');
    await clearRun('test-run-tg-002');
    await clearRun('test-run-tg-003');
  });

  it('happy path: all rows imported, attempt status=ok', async () => {
    await seedPendingAttempt('test-run-tg-001', 'besttour', 'kansai', 5);
    const { code, stderr } = runCli([
      'import-tour-group-offers',
      '--run', 'test-run-tg-001',
      '--file', path.join(FIXTURE_DIR, 'besttour-kansai-5n-ok.json'),
    ]);
    expect(code, stderr).toBe(0);

    const c = getTursoClient();
    const offers = rowsToObjects(await c.execute(
      `SELECT * FROM stage0_tour_group_offers WHERE run_id = 'test-run-tg-001' ORDER BY price_per_person_twd`
    ));
    expect(offers).toHaveLength(2);
    expect(Number(offers[0].price_per_person_twd)).toBe(32500);
    expect(Number(offers[1].hotel_star_rating)).toBe(4);

    const att = rowsToObjects(await c.execute(
      `SELECT * FROM stage0_tour_group_scrape_attempts WHERE run_id = 'test-run-tg-001'`
    ))[0];
    expect(att.status).toBe('ok');
    expect(Number(att.parsed_count)).toBe(2);
    expect(Number(att.skipped_count)).toBe(0);
  });

  it('partial: one row skipped for missing required field, attempt status=partial', async () => {
    await seedPendingAttempt('test-run-tg-002', 'besttour', 'kansai', 5);
    const { code } = runCli([
      'import-tour-group-offers',
      '--run', 'test-run-tg-002',
      '--file', path.join(FIXTURE_DIR, 'besttour-kansai-5n-partial.json'),
    ]);
    expect(code).toBe(0);

    const c = getTursoClient();
    const offers = rowsToObjects(await c.execute(
      `SELECT * FROM stage0_tour_group_offers WHERE run_id = 'test-run-tg-002'`
    ));
    expect(offers).toHaveLength(1);
    const att = rowsToObjects(await c.execute(
      `SELECT * FROM stage0_tour_group_scrape_attempts WHERE run_id = 'test-run-tg-002'`
    ))[0];
    expect(att.status).toBe('partial');
    expect(Number(att.parsed_count)).toBe(1);
    expect(Number(att.skipped_count)).toBe(1);
  });

  it('rejects: attempt-identity mismatch — top-level source_id has no pending attempt', async () => {
    // Note: we seed 'besttour' but the file declares 'lifetour' at top level
    await seedPendingAttempt('test-run-tg-003', 'besttour', 'kansai', 5);
    const { code, stderr } = runCli([
      'import-tour-group-offers',
      '--run', 'test-run-tg-003',
      '--file', path.join(FIXTURE_DIR, 'besttour-kansai-5n-mismatch.json'),
    ]);
    expect(code).not.toBe(0);
    expect(stderr).toMatch(/no pending attempt|attempt not found/i);

    const c = getTursoClient();
    const offers = rowsToObjects(await c.execute(
      `SELECT COUNT(*) AS n FROM stage0_tour_group_offers WHERE run_id = 'test-run-tg-003'`
    ));
    expect(Number((offers[0] as any).n)).toBe(0);
  });
});
