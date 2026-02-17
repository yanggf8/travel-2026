/**
 * Migrate ZH content from zh-content.ts → Turso DB.
 * One-time, idempotent. Already executed — zh-content.ts has been deleted.
 * To re-run, restore zh-content.ts from git first.
 */
import { TursoPipelineClient } from './turso-pipeline';
import {
  ZH_DAYS, ZH_TRANSIT, ZH_HOTELS, ZH_DAY_LANDMARKS,
  ZH_KYOTO_DAYS, ZH_KYOTO_TRANSIT, ZH_KYOTO_HOTELS, ZH_KYOTO_DAY_LANDMARKS,
  ZH_DAY_ROUTES, ZH_KYOTO_DAY_ROUTES, HOME_ADDRESS, KYOTO_HOME_ADDRESS,
} from '../workers/trip-dashboard/src/zh-content';

const esc = (s: string | null | undefined): string =>
  s == null ? 'NULL' : `'${String(s).replace(/'/g, "''")}'`;

async function main() {
  const client = new TursoPipelineClient();
  const stmts: string[] = [];

  const configs: Array<{
    planId: string; dest: string; days: Record<number, any>;
    hotels: Record<string, string>; transit: any;
    landmarks: Record<number, string[]>; routes: Record<number, any[]>;
    homeAddress: string;
  }> = [
    {
      planId: 'tokyo-2026', dest: 'tokyo_2026', days: ZH_DAYS,
      hotels: ZH_HOTELS, transit: ZH_TRANSIT,
      landmarks: ZH_DAY_LANDMARKS, routes: ZH_DAY_ROUTES,
      homeAddress: HOME_ADDRESS,
    },
    {
      planId: 'kyoto-2026', dest: 'kyoto_2026', days: ZH_KYOTO_DAYS,
      hotels: ZH_KYOTO_HOTELS, transit: ZH_KYOTO_TRANSIT,
      landmarks: ZH_KYOTO_DAY_LANDMARKS, routes: ZH_KYOTO_DAY_ROUTES,
      homeAddress: KYOTO_HOME_ADDRESS,
    },
  ];

  for (const cfg of configs) {
    const { planId, dest, days, hotels, transit, landmarks, routes, homeAddress } = cfg;
    console.log(`\n=== ${planId} / ${dest} ===`);

    // home_address on plan_destinations
    stmts.push(`UPDATE plan_destinations SET home_address = ${esc(homeAddress)} WHERE plan_id = ${esc(planId)} AND slug = ${esc(dest)}`);

    // ZH day themes
    for (const [dayNum, zh] of Object.entries(days)) {
      stmts.push(`UPDATE itinerary_days SET theme_zh = ${esc(zh.theme)} WHERE plan_id = ${esc(planId)} AND destination = ${esc(dest)} AND day_number = ${dayNum}`);

      for (const session of ['morning', 'afternoon', 'evening'] as const) {
        const s = zh[session];
        if (!s) continue;
        stmts.push(`UPDATE itinerary_sessions SET
          focus_zh = ${esc(s.focus)},
          transit_notes_zh = ${esc(s.transit_notes)},
          meals_zh_json = ${esc(JSON.stringify(s.meals))},
          activities_zh_json = ${esc(JSON.stringify(s.activities))}
          WHERE plan_id = ${esc(planId)} AND destination = ${esc(dest)} AND day_number = ${dayNum} AND session_type = ${esc(session)}`);
      }
    }

    // Hotel name_zh
    for (const [enName, zhName] of Object.entries(hotels)) {
      stmts.push(`UPDATE hotels SET name_zh = ${esc(zhName)} WHERE plan_id = ${esc(planId)} AND destination = ${esc(dest)} AND name = ${esc(enName)}`);
    }

    // Transit summary ZH
    stmts.push(`UPDATE itinerary_metadata SET transit_summary_zh = ${esc(JSON.stringify(transit))} WHERE plan_id = ${esc(planId)} AND destination = ${esc(dest)}`);

    // Landmarks
    stmts.push(`DELETE FROM day_landmarks WHERE plan_id = ${esc(planId)} AND destination = ${esc(dest)}`);
    for (const [dayNum, lms] of Object.entries(landmarks)) {
      for (let i = 0; i < lms.length; i++) {
        stmts.push(`INSERT INTO day_landmarks (plan_id, destination, day_number, sort_order, landmark) VALUES (${esc(planId)}, ${esc(dest)}, ${dayNum}, ${i}, ${esc(lms[i])})`);
      }
    }

    // Route segments
    stmts.push(`DELETE FROM day_route_segments WHERE plan_id = ${esc(planId)} AND destination = ${esc(dest)}`);
    for (const [dayNum, segs] of Object.entries(routes)) {
      for (let i = 0; i < segs.length; i++) {
        const s = segs[i];
        stmts.push(`INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode) VALUES (${esc(planId)}, ${esc(dest)}, ${dayNum}, ${i}, ${esc(s.from)}, ${esc(s.to)}, ${esc(s.mode)})`);
      }
    }
  }

  console.log(`\nExecuting ${stmts.length} statements...`);
  await client.executeMany(stmts);
  console.log('✅ ZH content migration complete.');
}

main().catch(console.error);
