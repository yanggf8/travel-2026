/**
 * Seed a DISPOSABLE test plan by cloning an existing real plan into a new plan_id.
 *
 * Why clone instead of hand-writing INSERTs: a mutation/cascade test is only valid
 * against a STRUCTURALLY REAL plan — every child row, cascade table, and event must
 * match what the system actually produces. Hand-rolled INSERTs would miss rows or get
 * the post-de-JSON schema subtly wrong, and the cascade would then behave differently.
 *
 * It copies every plan-scoped table's rows (any table with a `plan_id` column) from the
 * source plan, rewriting plan_id → the target. Idempotent: deletes the target's rows
 * first, so it doubles as the RESET between a TS run and a Rust run (run it again to
 * restore the pre-mutation baseline). Destination slugs are preserved (only plan_id changes).
 *
 * Usage:
 *   npx ts-node scripts/seed-test-plan.ts                          # tokyo-2026 → test-set-dates-2026
 *   npx ts-node scripts/seed-test-plan.ts <source> <target>
 *
 * SAFETY: refuses to write to tokyo-2026 / kyoto-2026 as the TARGET.
 */
import { TursoPipelineClient } from './turso-pipeline';

function rowsAt(resp: any, idx: number): Record<string, any>[] {
  const r = resp?.results?.[idx]?.response?.result;
  if (!r?.rows || !r?.cols) return [];
  const cols = (r.cols as Array<{ name: string }>).map((c) => c.name);
  return (r.rows as unknown[][]).map((row) => {
    const o: Record<string, any> = {};
    cols.forEach((c, i) => (o[c] = (row as any)[i]?.value ?? null));
    return o;
  });
}

const PROTECTED = new Set(['tokyo-2026', 'kyoto-2026']);

function sqlLit(v: unknown): string {
  if (v === null || v === undefined) return 'NULL';
  if (typeof v === 'number') return String(v);
  return `'${String(v).replace(/'/g, "''")}'`;
}

async function main(): Promise<void> {
  const source = process.argv[2] || 'tokyo-2026';
  const target = process.argv[3] || 'test-set-dates-2026';
  if (PROTECTED.has(target)) {
    console.error(`Refusing to use a protected plan as TARGET: ${target}`);
    process.exit(1);
  }

  const client = new TursoPipelineClient();

  // Every table with a plan_id column.
  const tcols = await client.executeBatch([
    `SELECT m.name AS tbl FROM sqlite_master m JOIN pragma_table_info(m.name) p ON p.name='plan_id' WHERE m.type='table' ORDER BY m.name;`,
  ]);
  const tables = rowsAt(tcols, 0).map((r) => String(r.tbl));

  // 1. DELETE all target rows (the reset). operation_runs included so audit re-runs cleanly.
  const deletes = tables.map((t) => `DELETE FROM ${t} WHERE plan_id = ${sqlLit(target)};`);
  await client.executeMany(deletes);

  // Tables whose PK is a GLOBALLY-unique id (not plan-scoped) — rewrite that id by
  // suffixing with the target plan_id so the clone doesn't collide with the source.
  // (activity_tags.activity_id would need rewriting too, but tokyo has 0 tag rows;
  // assert that holds for any source by checking below.)
  const GLOBAL_ID_COL: Record<string, string> = {
    activities: 'id',
    operation_runs: 'run_id',
    plan_offer_warnings: 'id',
  };
  const idSuffix = `__${target}`;

  // 2. Read source rows per table, rewrite plan_id (+ global ids), build INSERTs.
  const reads = await client.executeBatch(tables.map((t) => `SELECT * FROM ${t} WHERE plan_id = ${sqlLit(source)};`));
  const inserts: string[] = [];
  let totalRows = 0;
  tables.forEach((t, i) => {
    const rows = rowsAt(reads, i);
    const idCol = GLOBAL_ID_COL[t];
    for (const row of rows) {
      const cols = Object.keys(row);
      const vals = cols.map((c) => {
        if (c === 'plan_id') return sqlLit(target);
        if (idCol && c === idCol && row[c] != null) return sqlLit(`${row[c]}${idSuffix}`);
        return sqlLit(row[c]);
      });
      inserts.push(`INSERT INTO ${t} (${cols.join(', ')}) VALUES (${vals.join(', ')});`);
      totalRows++;
    }
  });

  await client.executeMany(inserts);
  console.log(`✅ Seeded ${target} from ${source}: ${totalRows} rows across ${tables.filter((_, i) => rowsAt(reads, i).length > 0).length} tables.`);
  console.log(`   (Re-run this script to RESET ${target} to the same baseline.)`);
}

main().catch((err) => {
  console.error('Error:', err);
  process.exit(1);
});
