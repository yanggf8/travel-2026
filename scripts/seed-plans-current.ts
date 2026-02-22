/**
 * Seed plans table (plan_id + schema_version only — no blobs).
 *
 * All data lives in normalized tables. Use `npm run db:migrate:turso` to create tables.
 * This script ensures the `plans` row exists for each plan_id.
 *
 * Usage:
 *   npm run db:seed:plans
 */

import { TursoPipelineClient } from './turso-pipeline';

function sqlEscape(value: string): string {
  return value.replace(/'/g, "''");
}

function sqlText(value: string | null): string {
  if (value === null) return 'NULL';
  return `'${sqlEscape(value)}'`;
}

async function main(): Promise<void> {
  const client = new TursoPipelineClient();

  // Known plans — add new plan IDs here
  const plans = [
    { planId: 'tokyo-2026', schemaVersion: '4.2.0' },
    { planId: 'kyoto-2026', schemaVersion: '4.2.0' },
  ];

  console.log(`Seeding ${plans.length} plan(s) into plans table...`);
  const sqlStatements: string[] = [];

  for (const entry of plans) {
    console.log(`  - ${entry.planId} (schema ${entry.schemaVersion})`);
    sqlStatements.push(
      `INSERT INTO plans (plan_id, schema_version, updated_at)
VALUES (${sqlText(entry.planId)}, ${sqlText(entry.schemaVersion)}, datetime('now'))
ON CONFLICT(plan_id) DO UPDATE SET
  schema_version = ${sqlText(entry.schemaVersion)},
  updated_at = datetime('now');`
    );
  }

  await client.executeMany(sqlStatements, 5);
  console.log(`Done. Plan rows exist for: ${plans.map(p => p.planId).join(', ')}`);
}

main().catch((e) => {
  console.error(`Error: ${(e as Error).message}`);
  process.exit(1);
});
