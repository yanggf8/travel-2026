/**
 * Fetch Taiwan public holidays from DGPA (行政院人事行政總處) and insert into Turso.
 *
 * DGPA publishes the official 中華民國政府行政機關辦公日曆表 as CSV. This is the
 * authoritative source for Taiwan holidays. Format:
 *
 *   西元日期,星期,是否放假,備註
 *   20260619,五,2,端午節
 *
 * 是否放假:
 *   0 = workday (no holiday)
 *   1 = makeup workday (補班日, otherwise weekend but office is open)
 *   2 = holiday or weekend off
 *
 * Each row recorded with source_url so the data is verifiable and refreshable.
 * Replaces deprecated data/holidays/taiwan-2026.json (hand-curated, missing entries).
 *
 * Usage:
 *   npx ts-node scripts/fetch-taiwan-holidays.ts <csv-url>
 *
 * The CSV URL is the one shown on data.gov.tw dataset 14718. You can paste any
 * year's DGPA CSV (114年/2025, 115年/2026, 116年/2027, ...). The script reads the
 * 西元日期 (Gregorian date) to know which year to insert.
 *
 * Example for 115年 (2026):
 *   npx ts-node scripts/fetch-taiwan-holidays.ts \
 *     "https://www.dgpa.gov.tw/FileConversion?filename=dgpa/files/202506/a52331bd-a189-466b-b0f0-cae3062bbf74.csv&nfix=&name=115年中華民國政府行政機關辦公日曆表.csv"
 */

import { TursoPipelineClient } from './turso-pipeline';
import { tursoText, tursoInt } from './turso-pipeline';

const COUNTRY = 'taiwan';
const SOURCE_LABEL = 'DGPA 行政院人事行政總處 中華民國政府行政機關辦公日曆表';

interface CsvRow {
  date: string;          // YYYY-MM-DD
  day_of_week: string;   // 一/二/三/四/五/六/日
  is_holiday: number;    // 0 / 1 / 2
  name: string | null;   // 備註 (holiday name, '' becomes NULL)
}

function parseCsv(text: string): CsvRow[] {
  // Strip BOM if present
  if (text.charCodeAt(0) === 0xFEFF) text = text.slice(1);
  const lines = text.split(/\r?\n/).filter(l => l.trim().length > 0);
  if (lines.length === 0) return [];
  // Skip header
  const header = lines[0];
  if (!header.includes('西元日期')) {
    throw new Error(`Unexpected CSV header (expected '西元日期,星期,是否放假,備註'): ${header}`);
  }
  const rows: CsvRow[] = [];
  for (let i = 1; i < lines.length; i++) {
    const cols = lines[i].split(',');
    if (cols.length < 3) continue;
    const raw_date = cols[0].trim();
    if (!/^\d{8}$/.test(raw_date)) continue;
    const date = `${raw_date.slice(0, 4)}-${raw_date.slice(4, 6)}-${raw_date.slice(6, 8)}`;
    const day_of_week = cols[1].trim();
    const is_holiday = parseInt(cols[2].trim(), 10);
    if (Number.isNaN(is_holiday)) continue;
    const name = (cols[3] || '').trim() || null;
    rows.push({ date, day_of_week, is_holiday, name });
  }
  return rows;
}

async function main(): Promise<void> {
  const url = process.argv[2];
  if (!url) {
    console.error('Usage: npx ts-node scripts/fetch-taiwan-holidays.ts <csv-url>');
    console.error('Get the URL from: https://data.gov.tw/dataset/14718');
    process.exit(1);
  }

  console.log(`Fetching: ${url}`);
  const resp = await fetch(url);
  if (!resp.ok) {
    console.error(`HTTP ${resp.status} ${resp.statusText}`);
    process.exit(1);
  }
  const text = await resp.text();
  console.log(`Got ${text.length} bytes`);

  const rows = parseCsv(text);
  if (rows.length === 0) {
    console.error('No rows parsed from CSV — check format.');
    process.exit(1);
  }
  const firstYear = rows[0].date.slice(0, 4);
  const lastYear = rows[rows.length - 1].date.slice(0, 4);
  console.log(`Parsed ${rows.length} day rows, covering ${firstYear}..${lastYear}`);

  // Count what we have
  const totalDays = rows.length;
  const holidayCount = rows.filter(r => r.is_holiday === 2 && r.name).length;
  const weekendCount = rows.filter(r => r.is_holiday === 2 && !r.name).length;
  const makeupCount = rows.filter(r => r.is_holiday === 1).length;
  const workdayCount = rows.filter(r => r.is_holiday === 0).length;
  console.log(`  ${holidayCount} named holidays`);
  console.log(`  ${weekendCount} weekend days off`);
  console.log(`  ${makeupCount} makeup workdays (補班)`);
  console.log(`  ${workdayCount} regular workdays`);

  // Prepare INSERT batch (idempotent via INSERT OR REPLACE)
  const fetched_at = new Date().toISOString();
  const stmts = rows.map(r => ({
    sql: `INSERT OR REPLACE INTO holidays
      (country, year, date, name, day_of_week, is_holiday, source_url, source_label, fetched_at, confidence)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?);`,
    args: [
      tursoText(COUNTRY),
      tursoInt(parseInt(r.date.slice(0, 4), 10)),
      tursoText(r.date),
      tursoText(r.name),
      tursoText(r.day_of_week),
      tursoInt(r.is_holiday),
      tursoText(url),
      tursoText(SOURCE_LABEL),
      tursoText(fetched_at),
      tursoText('authoritative'),
    ],
  }));

  const client = new TursoPipelineClient();
  console.log(`\nWriting ${stmts.length} rows to Turso...`);
  // executeMany handles batching; libSQL HTTP has a per-request limit so chunk to be safe
  const CHUNK = 200;
  for (let i = 0; i < stmts.length; i += CHUNK) {
    const chunk = stmts.slice(i, i + CHUNK);
    await client.executeManyParams(chunk);
    console.log(`  ${Math.min(i + CHUNK, stmts.length)}/${stmts.length}...`);
  }
  console.log(`✅ ${stmts.length} holidays/days written.`);

  // Spot-check: print named holidays we just imported
  console.log('\nNamed holidays imported:');
  for (const r of rows.filter(r => r.is_holiday === 2 && r.name)) {
    console.log(`  ${r.date} (${r.day_of_week}) — ${r.name}`);
  }
}

main().catch((err) => {
  console.error('Error:', err);
  process.exit(1);
});
