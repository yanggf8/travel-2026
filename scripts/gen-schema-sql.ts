import { TursoPipelineClient } from './turso-pipeline';
import * as fs from 'fs';
function rowsAt(resp: any, idx: number): Record<string, any>[] {
  const r = resp?.results?.[idx]?.response?.result;
  if (!r?.rows || !r?.cols) return [];
  const cols = (r.cols as Array<{name:string}>).map(c => c.name);
  return (r.rows as unknown[][]).map(row => { const o: Record<string,any> = {}; cols.forEach((c,i)=>o[c]=(row as any)[i]?.value ?? null); return o; });
}
(async () => {
  const c = new TursoPipelineClient();
  // Tables + indexes, excluding sqlite internals + the dead 'flights' (already dropped),
  // ordered by name for stable diffs.
  const resp = await c.executeBatch([
    `SELECT name, sql FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND sql IS NOT NULL ORDER BY name;`,
    `SELECT name, tbl_name, sql FROM sqlite_master WHERE type='index' AND name NOT LIKE 'sqlite_%' AND sql IS NOT NULL ORDER BY tbl_name, name;`,
  ]);
  const tables = rowsAt(resp, 0);
  const indexes = rowsAt(resp, 1);

  const header = `-- =============================================================================
-- travel-2026 Database Schema (Turso / libSQL)
-- AUTO-GENERATED from the live DB via scripts/turso-migrate.ts's output.
-- Regenerate: npx ts-node scripts/gen-schema-sql.ts  (do NOT hand-edit)
-- Source of truth = scripts/turso-migrate.ts; this mirrors the applied schema.
-- Generated: ${new Date().toISOString().slice(0,10)}
-- Tables: ${tables.length} | Indexes: ${indexes.length}
-- =============================================================================
--
-- Naming conventions:
--   plan_id      : kebab-case (e.g. 'tokyo-2026')
--   destination  : snake_case (e.g. 'tokyo_2026')
--
-- NOTE (de-JSON program, 2026-06): no JSON-encoded values are stored in any
-- column. Former *_json columns were re-normalized into child tables / typed
-- columns / *_text fields. See memory no-json-in-rdb.
-- =============================================================================

`;
  const tableDDL = tables.map(t => `${String(t.sql).trim()};`).join('\n\n');
  const indexDDL = indexes.length
    ? `\n\n-- =====================\n-- Indexes\n-- =====================\n\n` + indexes.map(i => `${String(i.sql).trim()};`).join('\n')
    : '';
  fs.writeFileSync('scripts/schema.sql', header + tableDDL + indexDDL + '\n');
  console.log(`wrote scripts/schema.sql — ${tables.length} tables, ${indexes.length} indexes`);
})().catch(e => { console.error('FAIL:', e.message); process.exit(1); });
