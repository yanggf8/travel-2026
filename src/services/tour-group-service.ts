import path from 'node:path';
import { sqlText, sqlInt, rowsToObjects } from '../state/sql-helpers';

function getProjectRoot(): string {
  return path.resolve(__dirname, '..', '..');
}

function requirePipeline(): { TursoPipelineClient: new (opts?: any) => any } {
  return require(path.join(getProjectRoot(), 'scripts', 'turso-pipeline.ts'));
}

export function getTursoClient(): any {
  const { TursoPipelineClient } = requirePipeline();
  return new TursoPipelineClient();
}

export interface TourGroupOfferRow {
  run_id: string;
  offer_id: string;
  source_id: string;
  dest_region: string;
  depart_date: string;
  return_date: string;
  nights: number;
  price_per_person_twd: number;
  title: string;
  url: string;
  scraped_at: string;
  hotel_name?: string | null;
  hotel_star_rating?: number | null;
  meals_included_count?: number | null;
  departure_status?: string | null;
  seats_available?: number | null;
  min_group_size?: number | null;
  group_size_cap?: number | null;
  raw_json?: string | null;
  parse_warnings_json?: string | null;
}

export type ScrapeAttemptStatus = 'pending' | 'ok' | 'failed' | 'partial';

export interface ScrapeAttemptRow {
  run_id: string;
  source_id: string;
  dest_region: string;
  nights: number;
  status: ScrapeAttemptStatus;
  offer_count?: number | null;
  parsed_count?: number | null;
  skipped_count?: number | null;
  error?: string | null;
  attempted_at?: string | null;
}

const REQUIRED_OFFER_FIELDS: (keyof TourGroupOfferRow)[] = [
  'run_id', 'offer_id', 'source_id', 'dest_region',
  'depart_date', 'return_date', 'nights',
  'price_per_person_twd', 'title', 'url', 'scraped_at',
];

export function validateOfferRow(row: Partial<TourGroupOfferRow>): { ok: true } | { ok: false; missing: string[] } {
  const missing = REQUIRED_OFFER_FIELDS.filter(k => row[k] === undefined || row[k] === null || row[k] === '');
  if (missing.length > 0) return { ok: false, missing: missing.map(String) };
  return { ok: true };
}

export async function insertTourGroupOffers(rows: TourGroupOfferRow[]): Promise<void> {
  if (rows.length === 0) return;
  const client = getTursoClient();
  const stmts = rows.map(r => {
    return `INSERT OR REPLACE INTO stage0_tour_group_offers (
      run_id, offer_id, source_id, dest_region, depart_date, return_date, nights,
      price_per_person_twd, title, url, scraped_at,
      hotel_name, hotel_star_rating, meals_included_count, departure_status,
      seats_available, min_group_size, group_size_cap, raw_json, parse_warnings_json
    ) VALUES (
      ${sqlText(r.run_id)},
      ${sqlText(r.offer_id)},
      ${sqlText(r.source_id)},
      ${sqlText(r.dest_region)},
      ${sqlText(r.depart_date)},
      ${sqlText(r.return_date)},
      ${sqlInt(r.nights)},
      ${sqlInt(r.price_per_person_twd)},
      ${sqlText(r.title)},
      ${sqlText(r.url)},
      ${sqlText(r.scraped_at)},
      ${sqlText(r.hotel_name)},
      ${sqlInt(r.hotel_star_rating)},
      ${sqlInt(r.meals_included_count)},
      ${sqlText(r.departure_status)},
      ${sqlInt(r.seats_available)},
      ${sqlInt(r.min_group_size)},
      ${sqlInt(r.group_size_cap)},
      ${sqlText(r.raw_json)},
      ${sqlText(r.parse_warnings_json)}
    );`;
  });
  await client.executeMany(stmts);
}

export async function upsertScrapeAttempt(row: ScrapeAttemptRow): Promise<void> {
  const client = getTursoClient();
  await client.execute(`INSERT INTO stage0_tour_group_scrape_attempts
      (run_id, source_id, dest_region, nights, status, offer_count, parsed_count, skipped_count, error, attempted_at)
      VALUES (
        ${sqlText(row.run_id)},
        ${sqlText(row.source_id)},
        ${sqlText(row.dest_region)},
        ${sqlInt(row.nights)},
        ${sqlText(row.status)},
        ${sqlInt(row.offer_count)},
        ${sqlInt(row.parsed_count)},
        ${sqlInt(row.skipped_count)},
        ${sqlText(row.error)},
        ${sqlText(row.attempted_at)}
      )
      ON CONFLICT(run_id, source_id, dest_region, nights) DO UPDATE SET
        status = excluded.status,
        offer_count = excluded.offer_count,
        parsed_count = excluded.parsed_count,
        skipped_count = excluded.skipped_count,
        error = excluded.error,
        attempted_at = excluded.attempted_at;`);
}

export async function findScrapeAttempt(
  run_id: string, source_id: string, dest_region: string, nights: number
): Promise<ScrapeAttemptRow | null> {
  const client = getTursoClient();
  const r = await client.execute(
    `SELECT * FROM stage0_tour_group_scrape_attempts
     WHERE run_id = ${sqlText(run_id)}
       AND source_id = ${sqlText(source_id)}
       AND dest_region = ${sqlText(dest_region)}
       AND nights = ${sqlInt(nights)};`
  );
  const rows = rowsToObjects(r);
  return (rows[0] as ScrapeAttemptRow) || null;
}

export async function listTourGroupOffers(filter: {
  run_id: string;
  source_id?: string;
  dest_region?: string;
  nights?: number;
  max_price?: number;
}): Promise<TourGroupOfferRow[]> {
  const client = getTursoClient();
  const where: string[] = [`run_id = ${sqlText(filter.run_id)}`];
  if (filter.source_id) { where.push(`source_id = ${sqlText(filter.source_id)}`); }
  if (filter.dest_region) { where.push(`dest_region = ${sqlText(filter.dest_region)}`); }
  if (filter.nights !== undefined) { where.push(`nights = ${sqlInt(filter.nights)}`); }
  if (filter.max_price !== undefined) { where.push(`price_per_person_twd <= ${sqlInt(filter.max_price)}`); }
  const r = await client.execute(
    `SELECT * FROM stage0_tour_group_offers
     WHERE ${where.join(' AND ')}
     ORDER BY price_per_person_twd ASC;`
  );
  return rowsToObjects(r) as TourGroupOfferRow[];
}

/**
 * Compute the audit set for a (run_id, dest_region, nights) combination.
 * Returns: raw cheapest + quality-floor cheapest + top-3 per source/depart_date.
 * Quality-floor default: hotel_star_rating >= 4. Rows with NULL star_rating
 * are excluded from the quality-floor pick but eligible for raw cheapest and
 * top-3-per-source/date.
 */
export async function findAuditSet(
  run_id: string,
  dest_region: string,
  nights: number,
  opts: { qualityFloorStarRating?: number } = {}
): Promise<TourGroupOfferRow[]> {
  const floor = opts.qualityFloorStarRating ?? 4;
  const all = await listTourGroupOffers({ run_id, dest_region, nights });
  if (all.length === 0) return [];

  const picked = new Map<string, TourGroupOfferRow>(); // offer_id -> row

  // raw cheapest
  const cheapest = all[0]; // already sorted ASC
  picked.set(cheapest.offer_id, cheapest);

  // quality-floor cheapest
  const qFloor = all.find(o => (o.hotel_star_rating ?? 0) >= floor);
  if (qFloor) picked.set(qFloor.offer_id, qFloor);

  // top-3 per (source_id, depart_date)
  const groups = new Map<string, TourGroupOfferRow[]>();
  for (const o of all) {
    const key = `${o.source_id}|${o.depart_date}`;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key)!.push(o);
  }
  for (const rows of groups.values()) {
    rows.sort((a, b) => a.price_per_person_twd - b.price_per_person_twd);
    for (const r of rows.slice(0, 3)) picked.set(r.offer_id, r);
  }

  return Array.from(picked.values()).sort((a, b) => a.price_per_person_twd - b.price_per_person_twd);
}
