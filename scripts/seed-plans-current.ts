/**
 * Seed plans_current table from existing JSON files.
 *
 * Reads data/travel-plan.json (→ "default") and data/trips/*/travel-plan.json
 * (→ "<trip-id>") and upserts them into Turso plans_current.
 *
 * Usage:
 *   npm run db:seed:plans
 */

import fs from 'node:fs';
import path from 'node:path';
import { TursoPipelineClient } from './turso-pipeline';

function sqlEscape(value: string): string {
  return value.replace(/'/g, "''");
}

function sqlText(value: string | null): string {
  if (value === null) return 'NULL';
  return `'${sqlEscape(value)}'`;
}

interface PlanEntry {
  planId: string;
  planPath: string;
  statePath: string;
}

function discoverPlans(): PlanEntry[] {
  const entries: PlanEntry[] = [];
  const root = process.cwd();

  // Default plan
  const defaultPlan = path.join(root, 'data', 'travel-plan.json');
  if (fs.existsSync(defaultPlan)) {
    entries.push({
      planId: 'default',
      planPath: defaultPlan,
      statePath: path.join(root, 'data', 'state.json'),
    });
  }

  // Trip-specific plans
  const tripsDir = path.join(root, 'data', 'trips');
  if (fs.existsSync(tripsDir)) {
    for (const dir of fs.readdirSync(tripsDir)) {
      const tripPlan = path.join(tripsDir, dir, 'travel-plan.json');
      if (fs.existsSync(tripPlan)) {
        entries.push({
          planId: dir,
          planPath: tripPlan,
          statePath: path.join(tripsDir, dir, 'state.json'),
        });
      }
    }
  }

  return entries;
}

async function main(): Promise<void> {
  const client = new TursoPipelineClient();
  const plans = discoverPlans();

  if (plans.length === 0) {
    console.log('No plan files found.');
    return;
  }

  console.log(`Found ${plans.length} plan(s) to seed:`);
  const sqlStatements: string[] = [];

  for (const entry of plans) {
    const planJson = fs.readFileSync(entry.planPath, 'utf-8');
    const plan = JSON.parse(planJson);
    const schemaVersion = (plan.schema_version as string) || 'unknown';

    const stateJson = fs.existsSync(entry.statePath)
      ? fs.readFileSync(entry.statePath, 'utf-8')
      : null;

    console.log(`  - ${entry.planId} (${entry.planPath}, schema ${schemaVersion})`);

    sqlStatements.push(
      `INSERT INTO plans_current (plan_id, schema_version, plan_json, state_json, updated_at)
VALUES (${sqlText(entry.planId)}, ${sqlText(schemaVersion)}, ${sqlText(planJson)}, ${sqlText(stateJson)}, datetime('now'))
ON CONFLICT(plan_id) DO UPDATE SET
  schema_version = ${sqlText(schemaVersion)},
  plan_json = ${sqlText(planJson)},
  state_json = ${sqlText(stateJson)},
  updated_at = datetime('now');`
    );
  }

  await client.executeMany(sqlStatements, 5);
  console.log(`✅ Seeded ${plans.length} plan(s) into plans_current.`);
}

main().catch((e) => {
  console.error(`Error: ${(e as Error).message}`);
  process.exit(1);
});
