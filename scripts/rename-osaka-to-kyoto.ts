#!/usr/bin/env node
/**
 * Rename plan: osaka-kyoto-2026 → kyoto-2026
 *
 * Updates:
 * - plans_current: plan_id + plan_json (active_destination, destination key, slug, display_name, cities)
 * - bookings_current: booking_key prefix, plan_id, destination
 * - bookings_events: booking_key prefix, plan_id
 * - plan_snapshots: plan_id
 * - events: plan_id
 * - state_json: destination references
 *
 * Usage: npx ts-node scripts/rename-osaka-to-kyoto.ts [--dry-run]
 */

import * as path from 'path';
import { TursoPipelineClient } from './turso-pipeline';

// Bridge scripts/ → src/ (different rootDir)
const tursoService = require(path.join(process.cwd(), 'src/services/turso-service'));
const { readPlanFromDb, writePlanToDb } = tursoService;

const OLD_PLAN_ID = 'osaka-kyoto-2026';
const NEW_PLAN_ID = 'kyoto-2026';
const OLD_DEST = 'osaka_kyoto_2026';
const NEW_DEST = 'kyoto_2026';

function sqlEscape(s: string): string {
  return s.replace(/'/g, "''");
}

function renamePlanJson(planJson: string): string {
  const plan = JSON.parse(planJson);

  // 1. active_destination
  if (plan.active_destination === OLD_DEST) {
    plan.active_destination = NEW_DEST;
  }

  // 2. project name
  if (plan.project) {
    plan.project = plan.project.replace(/Osaka\s*[+&,\s]\s*Kyoto/i, 'Kyoto');
  }

  // 3. destinations key
  if (plan.destinations?.[OLD_DEST]) {
    plan.destinations[NEW_DEST] = plan.destinations[OLD_DEST];
    delete plan.destinations[OLD_DEST];

    const dest = plan.destinations[NEW_DEST];
    dest.slug = NEW_DEST;
    dest.display_name = 'Kyoto';

    // Update cities: make Kyoto sole primary, remove Osaka
    if (dest.process_2_destination?.cities) {
      dest.process_2_destination.cities = dest.process_2_destination.cities.filter(
        (c: any) => c.slug !== 'osaka'
      );
      for (const city of dest.process_2_destination.cities) {
        if (city.slug === 'kyoto') city.role = 'primary';
      }
    }
  }

  // 4. cascade_state destination references
  if (plan.cascade_state?.destinations?.[OLD_DEST]) {
    plan.cascade_state.destinations[NEW_DEST] = plan.cascade_state.destinations[OLD_DEST];
    delete plan.cascade_state.destinations[OLD_DEST];
  }

  return JSON.stringify(plan);
}

function renameStateJson(stateJson: string): string {
  const state = JSON.parse(stateJson);

  if (state.destinations?.[OLD_DEST]) {
    state.destinations[NEW_DEST] = state.destinations[OLD_DEST];
    delete state.destinations[OLD_DEST];
  }

  if (typeof state.current_focus === 'string') {
    state.current_focus = state.current_focus.replace(OLD_DEST, NEW_DEST);
  }

  if (Array.isArray(state.event_log)) {
    for (const evt of state.event_log) {
      if (evt.destination === OLD_DEST) evt.destination = NEW_DEST;
    }
  }

  return JSON.stringify(state);
}

async function main() {
  const dryRun = process.argv.includes('--dry-run');
  const client = new TursoPipelineClient();

  console.log(`=== Rename Plan: ${OLD_PLAN_ID} → ${NEW_PLAN_ID} ===${dryRun ? ' [DRY RUN]' : ''}\n`);

  // 1. Read current plan via turso-service
  console.log('1. Reading plan from DB...');
  const row = await readPlanFromDb(OLD_PLAN_ID);
  if (!row) {
    console.error(`Plan "${OLD_PLAN_ID}" not found in DB.`);
    process.exit(1);
  }
  console.log(`   Found. Last updated: ${row.updated_at}`);

  // 2. Transform plan + state JSON
  console.log('\n2. Transforming plan JSON...');
  const newPlanJson = renamePlanJson(row.plan_json);
  const newStateJson = row.state_json ? renameStateJson(row.state_json) : null;

  // Verify transformation
  const newPlan = JSON.parse(newPlanJson);
  console.log(`   active_destination: ${newPlan.active_destination}`);
  console.log(`   destinations keys: ${Object.keys(newPlan.destinations).join(', ')}`);
  console.log(`   display_name: ${newPlan.destinations[NEW_DEST]?.display_name}`);
  console.log(`   project: ${newPlan.project}`);
  const cities = newPlan.destinations[NEW_DEST]?.process_2_destination?.cities;
  console.log(`   cities: ${cities?.map((c: any) => c.display_name).join(', ') || 'none'}`);

  if (dryRun) {
    console.log('\n[DRY RUN] No changes made.');
    return;
  }

  // 3. Write new plan via turso-service
  console.log('\n3. Writing new plan to DB...');
  const schemaVersion = newPlan.schema_version || '4.2.0';
  await writePlanToDb(NEW_PLAN_ID, newPlanJson, newStateJson, schemaVersion);
  console.log('   Written.');

  // 4. Delete old plan + update ancillary tables
  console.log('\n4. Updating ancillary tables...');
  const stmts: string[] = [
    // Delete old plan_id
    `DELETE FROM plans_current WHERE plan_id = '${sqlEscape(OLD_PLAN_ID)}'`,

    // bookings_current
    `UPDATE bookings_current SET booking_key = REPLACE(booking_key, '${sqlEscape(OLD_PLAN_ID)}:${sqlEscape(OLD_DEST)}', '${sqlEscape(NEW_PLAN_ID)}:${sqlEscape(NEW_DEST)}') WHERE booking_key LIKE '${sqlEscape(OLD_PLAN_ID)}:%'`,
    `UPDATE bookings_current SET plan_id = '${sqlEscape(NEW_PLAN_ID)}' WHERE plan_id = '${sqlEscape(OLD_PLAN_ID)}'`,
    `UPDATE bookings_current SET destination = '${sqlEscape(NEW_DEST)}' WHERE destination = '${sqlEscape(OLD_DEST)}'`,

    // bookings_events
    `UPDATE bookings_events SET booking_key = REPLACE(booking_key, '${sqlEscape(OLD_PLAN_ID)}:${sqlEscape(OLD_DEST)}', '${sqlEscape(NEW_PLAN_ID)}:${sqlEscape(NEW_DEST)}') WHERE booking_key LIKE '${sqlEscape(OLD_PLAN_ID)}:%'`,
    `UPDATE bookings_events SET plan_id = '${sqlEscape(NEW_PLAN_ID)}' WHERE plan_id = '${sqlEscape(OLD_PLAN_ID)}'`,

    // plan_snapshots
    `UPDATE plan_snapshots SET plan_id = '${sqlEscape(NEW_PLAN_ID)}' WHERE plan_id = '${sqlEscape(OLD_PLAN_ID)}'`,

    // events
    `UPDATE events SET plan_id = '${sqlEscape(NEW_PLAN_ID)}' WHERE plan_id = '${sqlEscape(OLD_PLAN_ID)}'`,
  ];

  for (const s of stmts) {
    console.log(`   ${s.slice(0, 80)}...`);
  }
  await client.executeMany(stmts);
  console.log('   Done.');

  // 5. Verify
  console.log('\n5. Verifying...');
  const verifyRow = await readPlanFromDb(NEW_PLAN_ID);
  if (!verifyRow) {
    console.error('   FAIL: New plan not found!');
    process.exit(1);
  }
  const verifyPlan = JSON.parse(verifyRow.plan_json);
  const checks = [
    { label: 'active_destination', ok: verifyPlan.active_destination === NEW_DEST, val: verifyPlan.active_destination },
    { label: 'destination key', ok: NEW_DEST in verifyPlan.destinations, val: Object.keys(verifyPlan.destinations).join(',') },
    { label: 'display_name', ok: verifyPlan.destinations[NEW_DEST]?.display_name === 'Kyoto', val: verifyPlan.destinations[NEW_DEST]?.display_name },
    { label: 'slug', ok: verifyPlan.destinations[NEW_DEST]?.slug === NEW_DEST, val: verifyPlan.destinations[NEW_DEST]?.slug },
  ];

  // Verify old plan is gone
  const oldRow = await readPlanFromDb(OLD_PLAN_ID);
  checks.push({ label: 'old plan deleted', ok: oldRow === null, val: oldRow ? 'still exists' : 'deleted' });

  let passed = 0;
  for (const c of checks) {
    const icon = c.ok ? 'PASS' : 'FAIL';
    console.log(`   [${icon}] ${c.label}: ${c.val}`);
    if (c.ok) passed++;
  }

  console.log(`\n=== Results: ${passed}/${checks.length} passed ===`);
  if (passed < checks.length) process.exit(1);
  console.log('\nRename complete!');
}

main().catch((err) => {
  console.error('Script failed:', err);
  process.exit(1);
});
