import { TursoPipelineClient } from './turso-pipeline';

async function main() {
  const sql = process.argv.slice(2).join(' ');
  if (!sql) {
    console.error('Usage: npx tsx scripts/turso-exec.ts <SQL>');
    console.error('  npm run db:exec -- "SELECT * FROM plan_metadata"');
    console.error('  npm run db:exec -- "UPDATE timesofday SET focus_zh = \'新焦點\' WHERE ..."');
    process.exit(1);
  }
  const client = new TursoPipelineClient();
  const res = await client.execute(sql);
  const result = res.results?.[0]?.response?.result;
  if (res.results?.[0]?.response?.error) {
    console.error('Error:', JSON.stringify(res.results[0].response.error));
    process.exit(1);
  }
  if (result?.affected_row_count !== undefined && result.affected_row_count > 0) {
    console.log(`${result.affected_row_count} row(s) affected`);
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

main().catch((e) => {
  console.error(e.message);
  process.exit(1);
});
