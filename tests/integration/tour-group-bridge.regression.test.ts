import { describe, it, expect, beforeEach } from 'vitest';
import { rowsToObjects, sqlText } from '../../src/state/sql-helpers';
import { bridgeAuditSet } from '../../src/services/tour-group-bridge';
import { insertTourGroupOffers, getTursoClient } from '../../src/services/tour-group-service';

const RUN = 'test-run-bridge-001';
const PLAN = 'test-plan-bridge-001';
const DEST = 'osaka_2026';

async function clear() {
  const c = getTursoClient();
  await c.execute(`DELETE FROM stage0_tour_group_offers WHERE run_id = ${sqlText(RUN)}`);
  await c.execute(`DELETE FROM plan_offer_group_meta WHERE plan_id = ${sqlText(PLAN)}`);
  await c.execute(`DELETE FROM plan_offers WHERE plan_id = ${sqlText(PLAN)}`);
}

function offer(over: any) {
  return {
    run_id: RUN, source_id: 'besttour', dest_region: 'kansai',
    depart_date: '2026-06-20', return_date: '2026-06-25', nights: 5,
    title: 'test', url: 'http://test/', scraped_at: '2026-05-25T00:00:00Z',
    ...over,
  };
}

describe('tour-group adopt-time bridge', () => {
  beforeEach(clear);

  it('copies audit set: raw cheapest + quality-floor + top-3 per source/date', async () => {
    await insertTourGroupOffers([
      offer({ offer_id: 'a1', price_per_person_twd: 28000, hotel_star_rating: 3 }), // raw cheapest + top-3 besttour
      offer({ offer_id: 'a2', price_per_person_twd: 35000, hotel_star_rating: 4 }), // quality-floor cheapest + top-3 besttour
      offer({ offer_id: 'a3', price_per_person_twd: 30000, hotel_star_rating: 3 }), // top-3 besttour
      offer({ offer_id: 'a4', price_per_person_twd: 42000, hotel_star_rating: 5 }), // not in any set
      offer({ offer_id: 'b1', source_id: 'lifetour', price_per_person_twd: 31000, hotel_star_rating: 3 }), // top-3 lifetour
    ]);

    await bridgeAuditSet({
      run_id: RUN, plan_id: PLAN, destination: DEST,
      dest_region: 'kansai', nights: 5,
    });

    const c = getTursoClient();
    const inserted = rowsToObjects(await c.execute(
      `SELECT id, package_subtype, price_per_person FROM plan_offers WHERE plan_id = ${sqlText(PLAN)} ORDER BY price_per_person`
    ));
    const ids = inserted.map((r: any) => r.id);
    expect(ids).toContain('a1'); // raw cheapest
    expect(ids).toContain('a2'); // quality-floor
    expect(ids).toContain('a3'); // top-3 besttour
    expect(ids).toContain('b1'); // top-3 lifetour
    expect(ids).not.toContain('a4'); // 4th cheapest besttour, no other reason to include
    for (const row of inserted) {
      expect(row.package_subtype).toBe('group_tour');
    }

    const meta = rowsToObjects(await c.execute(
      `SELECT source_offer_id, source_offer_run_id FROM plan_offer_group_meta WHERE plan_id = ${sqlText(PLAN)}`
    ));
    expect(meta.map((m: any) => m.source_offer_id).sort()).toEqual(['a1', 'a2', 'a3', 'b1'].sort());
    for (const m of meta) {
      expect(m.source_offer_run_id).toBe(RUN);
    }
  });

  it('handles empty quality-floor gracefully (no 4-star rows)', async () => {
    await insertTourGroupOffers([
      offer({ offer_id: 'c1', price_per_person_twd: 28000, hotel_star_rating: 3 }),
      offer({ offer_id: 'c2', price_per_person_twd: 30000, hotel_star_rating: 3 }),
    ]);
    await bridgeAuditSet({
      run_id: RUN, plan_id: PLAN, destination: DEST,
      dest_region: 'kansai', nights: 5,
    });
    const c = getTursoClient();
    const ids = rowsToObjects(await c.execute(
      `SELECT id FROM plan_offers WHERE plan_id = ${sqlText(PLAN)}`
    )).map((r: any) => r.id).sort();
    expect(ids).toEqual(['c1', 'c2']);
  });

  it('returns empty array when no tour-group offers match', async () => {
    const result = await bridgeAuditSet({
      run_id: RUN, plan_id: PLAN, destination: DEST,
      dest_region: 'kansai', nights: 5,
    });
    expect(result).toEqual([]);
    const c = getTursoClient();
    const count = rowsToObjects(await c.execute(
      `SELECT COUNT(*) AS n FROM plan_offers WHERE plan_id = ${sqlText(PLAN)}`
    ));
    expect(Number((count[0] as any).n)).toBe(0);
  });
});
