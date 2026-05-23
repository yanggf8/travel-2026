import { TursoPipelineClient } from './turso-pipeline';

/**
 * Split a SQL blob on statement-terminating semicolons. Single-quoted string
 * literals are respected (`'` toggles in/out of a literal; the SQL convention
 * `''` for an escaped quote is handled naturally by the toggle). Comments and
 * other quoting forms (`"`, `` ` ``, `--`, `/* *​/`) are intentionally NOT
 * supported — db:exec is for short ad-hoc SQL, not arbitrary scripts.
 */
function splitStatements(sql: string): string[] {
  const out: string[] = [];
  let buf = '';
  let inStr = false;
  for (let i = 0; i < sql.length; i++) {
    const c = sql[i];
    if (c === "'") {
      inStr = !inStr;
      buf += c;
      continue;
    }
    if (c === ';' && !inStr) {
      const stmt = buf.trim();
      if (stmt) out.push(stmt + ';');
      buf = '';
      continue;
    }
    buf += c;
  }
  const tail = buf.trim();
  if (tail) out.push(tail);
  return out;
}

function printResult(label: string | null, result: any): void {
  if (result?.affected_row_count !== undefined && result.affected_row_count > 0) {
    const prefix = label ? `${label}: ` : '';
    console.log(`${prefix}${result.affected_row_count} row(s) affected`);
  }
  if (result?.rows && result.rows.length > 0) {
    const cols = result.cols!.map((c: any) => c.name);
    for (const row of result.rows) {
      const obj: Record<string, unknown> = {};
      cols.forEach((name: string, i: number) => {
        obj[name] = (row as any)[i]?.value ?? null;
      });
      console.log(JSON.stringify(obj));
    }
  }
}

async function main() {
  const sql = process.argv.slice(2).join(' ');
  if (!sql) {
    console.error('Usage: npx tsx scripts/turso-exec.ts <SQL>');
    console.error('  npm run db:exec -- "SELECT * FROM plan_metadata"');
    console.error('  npm run db:exec -- "DELETE FROM a WHERE x=1; DELETE FROM b WHERE x=1;"');
    process.exit(1);
  }

  const stmts = splitStatements(sql);
  if (stmts.length === 0) {
    console.error('Error: no SQL statements parsed from input');
    process.exit(1);
  }

  const client = new TursoPipelineClient();

  // Single statement → keep the historical one-line output (no [1/1] noise).
  if (stmts.length === 1) {
    const res = await client.execute(stmts[0]);
    const r0: any = res.results?.[0];
    // Pipeline reports per-statement errors as `{ type: 'error', error: {...} }`
    // at the top level of `results[i]` (NOT under `.response.error`).
    if (r0?.type === 'error' || r0?.error) {
      console.error('Error:', JSON.stringify(r0.error ?? r0));
      process.exit(1);
    }
    printResult(null, r0?.response?.result);
    return;
  }

  // Multi-statement → run statements one by one. executeBatch() throws on
  // the first per-statement error and swallows the rest, which is exactly
  // the failure mode that silently dropped Stage 0 cleanup DELETEs. By
  // catching per statement we can report partial success and still exit
  // nonzero if anything failed.
  let anyFailed = false;
  for (let i = 0; i < stmts.length; i++) {
    const label = `[${i + 1}/${stmts.length}]`;
    try {
      const res = await client.execute(stmts[i]);
      const r0: any = res.results?.[0];
      // See note above: error shape is `results[i].error`, not `.response.error`.
      if (r0?.type === 'error' || r0?.error) {
        anyFailed = true;
        console.error(`${label} Error: ${JSON.stringify(r0.error ?? r0)}`);
        continue;
      }
      const result = r0?.response?.result;
      const affected = result?.affected_row_count;
      const rowCount = result?.rows?.length ?? 0;
      if (affected !== undefined && affected > 0) {
        console.log(`${label} ${affected} row(s) affected`);
      } else if (rowCount > 0) {
        console.log(`${label} ${rowCount} row(s) returned`);
      } else {
        console.log(`${label} ok`);
      }
      if (rowCount > 0) printResult(null, result);
    } catch (e: any) {
      anyFailed = true;
      console.error(`${label} Error: ${e?.message ?? String(e)}`);
    }
  }
  if (anyFailed) process.exit(1);
}

main().catch((e) => {
  console.error(e.message);
  process.exit(1);
});
