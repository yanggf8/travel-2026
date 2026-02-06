/**
 * Query Turso for offers matching trip criteria.
 * 
 * Usage:
 *   npm run db:query:turso -- --destination osaka_2026 --start 2026-02-24 --end 2026-02-28
 *   npm run db:query:turso -- --region kansai --sources besttour,liontravel --max 10
 *   npm run db:query:turso -- --fresh-hours 24
 */

import { TursoPipelineClient } from './turso-pipeline';

function usage(): string {
  return [
    'Query Turso for offers matching trip criteria.',
    '',
    'Usage:',
    '  npm run db:query:turso -- --destination osaka_2026 --start 2026-02-24 --end 2026-02-28',
    '  npm run db:query:turso -- --region kansai --sources besttour,liontravel',
    '  npm run db:query:turso -- --fresh-hours 24 --max 20',
    '',
    'Options:',
    '  --destination <slug>     Filter by destination (e.g. osaka_2026, tokyo_2026)',
    '  --region <name>          Filter by region (e.g. kansai, tokyo)',
    '  --start <YYYY-MM-DD>     Filter departure_date >= start',
    '  --end <YYYY-MM-DD>       Filter departure_date <= end',
    '  --sources <csv>          Filter by source_id (comma-separated: besttour,liontravel)',
    '  --type <type>            Filter by type (package, flight, hotel)',
    '  --max-price <int>        Filter price_per_person <= max',
    '  --fresh-hours <int>      Only offers scraped within N hours',
    '  --max <int>              Limit results (default: 50)',
    '  --include-undated        When filtering by date, also include offers without departure_date',
    '  --json                   Output as JSON array',
    '  --sql                    Show generated SQL (for debugging)',
    '',
    'Output columns:',
    '  id, source_id, type, name, price_per_person, departure_date, return_date, airline, hotel_name, age_hours',
    '',
  ].join('\n');
}

function optionValue(argv: string[], name: string): string | undefined {
  const i = argv.indexOf(name);
  if (i === -1) return undefined;
  return argv[i + 1];
}

function hasFlag(argv: string[], name: string): boolean {
  return argv.includes(name);
}

function sqlEscapeText(value: string): string {
  return value.replace(/'/g, "''");
}

function sqlText(value: string): string {
  return `'${sqlEscapeText(value)}'`;
}

function unwrapTursoCell(value: unknown): unknown {
  if (value && typeof value === 'object') {
    const v = value as Record<string, unknown>;
    if ('value' in v) return v.value;
  }
  return value;
}

function cellAt(row: unknown, idx: number): unknown {
  if (Array.isArray(row)) return unwrapTursoCell(row[idx]);
  return undefined;
}

function cellKey(row: unknown, key: string): unknown {
  if (row && typeof row === 'object' && !Array.isArray(row)) {
    const obj = row as Record<string, unknown>;
    return unwrapTursoCell(obj[key]);
  }
  return undefined;
}

function asString(value: unknown): string | null {
  const v = unwrapTursoCell(value);
  if (v === null || v === undefined) return null;
  if (typeof v === 'string') return v;
  if (typeof v === 'number' || typeof v === 'boolean') return String(v);
  return JSON.stringify(v);
}

function asNumber(value: unknown): number | null {
  const v = unwrapTursoCell(value);
  if (typeof v === 'number' && Number.isFinite(v)) return v;
  if (typeof v === 'string' && v.trim() && Number.isFinite(Number(v))) return Number(v);
  return null;
}

function formatAge(scrapedAt: string | null): string {
  if (!scrapedAt) return '?';
  const scraped = new Date(scrapedAt).getTime();
  const now = Date.now();
  const hours = Math.round((now - scraped) / (1000 * 60 * 60));
  if (hours < 1) return '<1h';
  if (hours < 24) return `${hours}h`;
  const days = Math.round(hours / 24);
  return `${days}d`;
}

function truncate(s: string | null, max: number): string {
  if (!s) return '';
  if (s.length <= max) return s;
  return s.slice(0, max - 1) + '…';
}

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  if (argv.includes('--help') || argv.includes('-h')) {
    console.log(usage());
    process.exit(0);
  }

  const destination = optionValue(argv, '--destination');
  const region = optionValue(argv, '--region');
  const start = optionValue(argv, '--start');
  const end = optionValue(argv, '--end');
  const sourcesCsv = optionValue(argv, '--sources');
  const type = optionValue(argv, '--type');
  const maxPriceStr = optionValue(argv, '--max-price');
  const freshHoursStr = optionValue(argv, '--fresh-hours');
  const maxStr = optionValue(argv, '--max');
  const jsonOutput = hasFlag(argv, '--json');
  const showSql = hasFlag(argv, '--sql');
  const includeUndated = hasFlag(argv, '--include-undated');

  const maxPrice = maxPriceStr ? parseInt(maxPriceStr, 10) : null;
  const freshHours = freshHoursStr ? parseInt(freshHoursStr, 10) : null;
  const limit = maxStr ? parseInt(maxStr, 10) : 50;

  if (start && !/^\d{4}-\d{2}-\d{2}$/.test(start)) {
    console.error(`Invalid --start date format: ${start} (expected YYYY-MM-DD)`);
    process.exit(1);
  }
  if (end && !/^\d{4}-\d{2}-\d{2}$/.test(end)) {
    console.error(`Invalid --end date format: ${end} (expected YYYY-MM-DD)`);
    process.exit(1);
  }
  if (start && end && start > end) {
    console.error(`Invalid date range: --start ${start} is after --end ${end}`);
    process.exit(1);
  }
  if (type && !['package', 'flight', 'hotel'].includes(type)) {
    console.error(`Invalid --type: ${type} (expected package|flight|hotel)`);
    process.exit(1);
  }
  if (maxPrice !== null && (Number.isNaN(maxPrice) || maxPrice < 0)) {
    console.error(`Invalid --max-price: ${maxPriceStr}`);
    process.exit(1);
  }
  if (freshHours !== null && (Number.isNaN(freshHours) || freshHours < 0)) {
    console.error(`Invalid --fresh-hours: ${freshHoursStr}`);
    process.exit(1);
  }
  if (Number.isNaN(limit) || limit <= 0) {
    console.error(`Invalid --max: ${maxStr}`);
    process.exit(1);
  }

  // Build WHERE clauses
  const conditions: string[] = [];

  if (destination) {
    conditions.push(`destination = ${sqlText(destination)}`);
  }
  if (region) {
    conditions.push(`region = ${sqlText(region)}`);
  }
  if (start || end) {
    if (!includeUndated) conditions.push('departure_date IS NOT NULL');
    if (start) {
      conditions.push(includeUndated ? `(departure_date IS NULL OR departure_date >= ${sqlText(start)})` : `departure_date >= ${sqlText(start)}`);
    }
    if (end) {
      conditions.push(includeUndated ? `(departure_date IS NULL OR departure_date <= ${sqlText(end)})` : `departure_date <= ${sqlText(end)}`);
    }
  }
  if (sourcesCsv) {
    const sources = sourcesCsv
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean);
    if (sources.length > 0) {
      conditions.push(`source_id IN (${sources.map(sqlText).join(',')})`);
    }
  }
  if (type) {
    conditions.push(`type = ${sqlText(type)}`);
  }
  if (maxPrice !== null && !isNaN(maxPrice)) {
    conditions.push(`price_per_person <= ${maxPrice}`);
  }
  if (freshHours !== null && !isNaN(freshHours)) {
    // Robust for ISO-8601: compare via julianday()
    conditions.push(`scraped_at IS NOT NULL`);
    conditions.push(`julianday(scraped_at) >= (julianday('now') - (${freshHours} / 24.0))`);
  }

  const whereClause = conditions.length > 0 ? `WHERE ${conditions.join(' AND ')}` : '';
  
  const sql = `
    SELECT 
      id,
      source_id,
      type,
      name,
      price_per_person,
      currency,
      departure_date,
      return_date,
      airline,
      hotel_name,
      scraped_at,
      (julianday('now') - julianday(scraped_at)) * 24.0 AS age_hours
    FROM offers
    ${whereClause}
    ORDER BY 
      CASE WHEN price_per_person IS NULL THEN 1 ELSE 0 END,
      price_per_person ASC,
      scraped_at DESC
    LIMIT ${limit}
  `.trim().replace(/\s+/g, ' ');

  if (showSql) {
    console.log('SQL:', sql);
    console.log('');
  }

  const client = new TursoPipelineClient();
  client.loadEnv();

  const result = await client.execute(sql);
  const rows = result?.results?.[0]?.response?.result?.rows as unknown[] | undefined;

  if (!rows || rows.length === 0) {
    console.log('No offers found matching criteria.');
    if (!jsonOutput) {
      console.log('');
      console.log('Filters applied:');
      if (destination) console.log(`  destination: ${destination}`);
      if (region) console.log(`  region: ${region}`);
      if (start || end) console.log(`  dates: ${start || '*'} to ${end || '*'}`);
      if (sourcesCsv) console.log(`  sources: ${sourcesCsv}`);
      if (maxPrice) console.log(`  max_price: ${maxPrice}`);
      if (freshHours) console.log(`  fresh_hours: ${freshHours}`);
    }
    process.exit(0);
  }

  if (jsonOutput) {
    const offers = rows.map((row) => ({
      id: asString(cellKey(row, 'id') ?? cellAt(row, 0)),
      source_id: asString(cellKey(row, 'source_id') ?? cellAt(row, 1)),
      type: asString(cellKey(row, 'type') ?? cellAt(row, 2)),
      name: asString(cellKey(row, 'name') ?? cellAt(row, 3)),
      price_per_person: asNumber(cellKey(row, 'price_per_person') ?? cellAt(row, 4)),
      currency: asString(cellKey(row, 'currency') ?? cellAt(row, 5)),
      departure_date: asString(cellKey(row, 'departure_date') ?? cellAt(row, 6)),
      return_date: asString(cellKey(row, 'return_date') ?? cellAt(row, 7)),
      airline: asString(cellKey(row, 'airline') ?? cellAt(row, 8)),
      hotel_name: asString(cellKey(row, 'hotel_name') ?? cellAt(row, 9)),
      scraped_at: asString(cellKey(row, 'scraped_at') ?? cellAt(row, 10)),
      age_hours: asNumber(cellKey(row, 'age_hours') ?? cellAt(row, 11)),
    }));
    console.log(JSON.stringify(offers, null, 2));
    process.exit(0);
  }

  // Table output
  console.log(`Found ${rows.length} offer(s):\n`);
  console.log(
    'SOURCE'.padEnd(12) +
    'TYPE'.padEnd(9) +
    'PRICE'.padEnd(10) +
    'DATE'.padEnd(23) +
    'AGE'.padEnd(6) +
    'NAME'
  );
  console.log('-'.repeat(80));

  for (const row of rows) {
    const sourceId = String(asString(cellKey(row, 'source_id') ?? cellAt(row, 1)) ?? '');
    const offerType = String(asString(cellKey(row, 'type') ?? cellAt(row, 2)) ?? '');
    const price = asNumber(cellKey(row, 'price_per_person') ?? cellAt(row, 4));
    const currency = asString(cellKey(row, 'currency') ?? cellAt(row, 5)) ?? 'TWD';
    const departureDate = asString(cellKey(row, 'departure_date') ?? cellAt(row, 6));
    const returnDate = asString(cellKey(row, 'return_date') ?? cellAt(row, 7));
    const name = asString(cellKey(row, 'name') ?? cellAt(row, 3));
    const scrapedAt = asString(cellKey(row, 'scraped_at') ?? cellAt(row, 10));

    const priceStr = price !== null ? `${currency} ${price}` : '—';
    const dateStr = departureDate ? `${departureDate}${returnDate ? `→${returnDate}` : ''}` : '—';
    const ageStr = formatAge(scrapedAt);
    const nameStr = truncate(String(name ?? ''), 40);

    console.log(
      sourceId.padEnd(12) +
      offerType.padEnd(9) +
      priceStr.padEnd(10) +
      dateStr.padEnd(23) +
      ageStr.padEnd(6) +
      nameStr
    );
  }

  console.log('');
  console.log(`Showing ${rows.length} of max ${limit} results.`);
}

main().catch((e) => {
  console.error(`Error: ${(e as Error).message}`);
  process.exit(1);
});
