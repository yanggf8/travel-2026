import { TursoPipelineClient } from './turso-pipeline';

function optionValue(argv: string[], name: string): string | undefined {
  const i = argv.indexOf(name);
  if (i === -1) return undefined;
  return argv[i + 1];
}

function unwrapTursoCell(value: unknown): unknown {
  if (value && typeof value === 'object') {
    const v = value as Record<string, unknown>;
    if ('value' in v) return v.value;
  }
  return value;
}

function formatScalar(value: unknown): string {
  const unwrapped = unwrapTursoCell(value);
  if (unwrapped === null || unwrapped === undefined) return '';
  if (typeof unwrapped === 'string' || typeof unwrapped === 'number' || typeof unwrapped === 'boolean') {
    return String(unwrapped);
  }
  return JSON.stringify(unwrapped);
}

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  const endpoint = optionValue(argv, '--endpoint');

  const client = new TursoPipelineClient({ ...(endpoint ? { endpoint } : {}) });
  client.loadEnv();

  const queries: Array<{ label: string; sql: string }> = [
    { label: 'offers_count', sql: 'SELECT COUNT(*) AS n FROM offers' },
    { label: 'events_count', sql: 'SELECT COUNT(*) AS n FROM events' },
    { label: 'destinations_count', sql: 'SELECT COUNT(*) AS n FROM destinations' },
    { label: 'bookings_count', sql: 'SELECT COUNT(*) AS n FROM bookings_current' },
    { label: 'booking_events_count', sql: 'SELECT COUNT(*) AS n FROM bookings_events' },
    { label: 'snapshots_count', sql: 'SELECT COUNT(*) AS n FROM plan_snapshots' },
    { label: 'offers_last_scraped_at', sql: 'SELECT MAX(scraped_at) AS v FROM offers' },
    { label: 'events_last_created_at', sql: 'SELECT MAX(created_at) AS v FROM events' },
  ];

  for (const q of queries) {
    try {
      const res = await client.execute(q.sql);
      const row = res.results?.[0]?.response?.result?.rows?.[0] as unknown;
      if (!row) {
        console.log(`${q.label}: (no rows)`);
        continue;
      }

      if (Array.isArray(row)) {
        console.log(`${q.label}: ${formatScalar(row[0])}`);
        continue;
      }

      if (row && typeof row === 'object') {
        const values = Object.values(row as Record<string, unknown>);
        console.log(`${q.label}: ${formatScalar(values[0])}`);
        continue;
      }

      console.log(`${q.label}: ${formatScalar(row)}`);
    } catch (e) {
      console.log(`${q.label}: (error) ${(e as Error).message}`);
    }
  }
}

main().catch((e) => {
  console.error(`Error: ${(e as Error).message}`);
  process.exit(1);
});
