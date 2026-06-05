/**
 * Turso Service — thin wrapper around TursoPipelineClient + import parsing.
 *
 * Provides:
 *   importOffersFromFiles  — bulk import scrape JSON → Turso offers table
 *   queryOffers            — filtered reads from Turso
 *   checkFreshness         — staleness check for source/region
 *   syncEventsToDb         — idempotent event sync via SHA1 external_id
 *   syncBookingsFromPlanJson — extract + upsert bookings from plan object
 *
 * Turso is required infrastructure. If TURSO_TOKEN is missing the service
 * throws with a clear error — no silent skipping.
 */

import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import type { HolidayCalendar, HolidayEntry, MakeupWorkday } from '../utils/leave-calculator';

// Scripts live outside src/ (rootDir), so we use dynamic require resolved
// from the project root to avoid TS6059 errors.
function getProjectRoot(): string {
  return path.resolve(__dirname, '..', '..');
}

function requirePipeline(): { TursoPipelineClient: new (opts?: any) => any } {
  return require(path.join(getProjectRoot(), 'scripts', 'turso-pipeline'));
}

function requireExtractor(): {
  extractAllBookings: (planPath: string, tripId?: string) => { bookings: any[]; warnings: string[] };
  extractPackageBookings: (plan: Record<string, unknown>, tripId: string, dest: string) => any[];
  extractTransferBookings: (plan: Record<string, unknown>, tripId: string, dest: string) => any[];
  extractActivityBookings: (plan: Record<string, unknown>, tripId: string, dest: string) => any[];
  toUpsertSql: (row: any) => string;
  toEventSql: (bookingKey: string, eventType: string, row: any) => string;
  BookingRow: any;
} {
  return require(path.join(getProjectRoot(), 'scripts', 'extract-bookings'));
}

function requireImporter(): {
  parseRawScrapeAsOfferRow: (fileName: string, data: Record<string, unknown>, dest: string | null, region: string | null) => any | null;
  parseScraperOfferAsOfferRow: (fileName: string, offer: Record<string, unknown>, dest: string | null, region: string | null) => any | null;
  toInsertSql: (row: any) => string;
  isWithinDateRange: (departureDate: string | null, start: string | null, end: string | null, includeUndated: boolean) => boolean;
} {
  return require(path.join(getProjectRoot(), 'scripts', 'import-offers-to-turso'));
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface TursoOfferQuery {
  planId?: string;
  destination?: string;
  region?: string;
  start?: string;
  end?: string;
  sources?: string[];
  type?: 'package' | 'flight' | 'hotel';
  maxPrice?: number;
  freshHours?: number;
  limit?: number;
}

export interface TursoOfferResult {
  id: string;
  source_id: string;
  type: string;
  name: string | null;
  price_per_person: number | null;
  currency: string | null;
  region: string | null;
  destination: string | null;
  departure_date: string | null;
  return_date: string | null;
  nights: number | null;
  availability: string | null;
  hotel_name: string | null;
  airline: string | null;
  scraped_at: string | null;
  source_file: string | null;
}

export interface TursoRawOfferResult extends TursoOfferResult {
  raw_data: string | null;
}

export interface TransportRouteData {
  time_min: number;
  cost_jpy: number;
  method: string;
}

export interface TransportRoutesData {
  [region: string]: {
    routes: Record<string, TransportRouteData>;
    hubs: Record<string, { type: string; area: string }>;
  };
}

export interface ImportResult {
  imported: number;
  skipped: number;
  filtered: number;
}

export interface FreshnessResult {
  hasFreshData: boolean;
  ageHours: number | null;
  offerCount: number;
  recommendation: 'skip' | 'rescrape' | 'no_data';
  region?: string;
}

const DB_MISSING_FIX =
  'Run npm run db:migrate:turso, then run the relevant Turso backfill/seed script before using this command.';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function getClient(): any {
  const { TursoPipelineClient } = requirePipeline();
  return new TursoPipelineClient();
}

function sqlEscape(value: string): string {
  return value.replace(/'/g, "''");
}

function sqlText(value: string | null | undefined): string {
  if (value == null) return 'NULL';
  return `'${sqlEscape(value)}'`;
}

function sqlInt(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return 'NULL';
  return `${Math.trunc(value)}`;
}

function extractRows(response: any): any[] {
  const result = response?.results?.[0]?.response?.result;
  if (!result?.rows) return [];
  return result.rows;
}

function extractColumns(response: any): string[] {
  const result = response?.results?.[0]?.response?.result;
  if (!result?.cols) return [];
  return result.cols.map((c: any) => c.name);
}

function rowsToObjects(response: any): Record<string, any>[] {
  return rowsToObjectsAt(response, 0);
}

function rowsToObjectsAt(response: any, idx: number): Record<string, any>[] {
  const cols = extractColumns(response);
  const result = response?.results?.[idx]?.response?.result;
  const atCols = result?.cols ? result.cols.map((c: any) => c.name) : cols;
  const rows = result?.rows || (idx === 0 ? extractRows(response) : []);
  return rows.map((row: any[]) => {
    const obj: Record<string, any> = {};
    atCols.forEach((col: string, i: number) => {
      const cell = row[i];
      obj[col] = cell?.value ?? null;
    });
    return obj;
  });
}

function normalizeCountry(countryOrMarket: string): string {
  const v = countryOrMarket.toLowerCase();
  if (v === 'tw') return 'taiwan';
  if (v === 'jp') return 'japan';
  return v;
}

function assertRows(rows: Record<string, any>[], label: string): void {
  if (rows.length === 0) {
    throw new Error(`Missing Turso data for ${label}. ${DB_MISSING_FIX}`);
  }
}

function toMonthDay(date: string): string {
  return date.slice(5);
}

export async function loadHolidayCalendarFromTurso(
  countryOrMarket: string,
  year: number,
): Promise<HolidayCalendar> {
  const country = normalizeCountry(countryOrMarket);
  const client = getClient();
  const response = await client.execute(
    `SELECT country, year, date, name, day_of_week, is_holiday, source_url, fetched_at, confidence
     FROM holidays
     WHERE country = '${sqlEscape(country)}'
       AND date >= '${Math.trunc(year)}-01-01'
       AND date <= '${Math.trunc(year)}-12-31'
     ORDER BY date;`
  );
  const rows = rowsToObjects(response);
  assertRows(rows, `holidays country=${country} year=${year}`);

  const holidays: Record<string, HolidayEntry> = {};
  const makeupWorkdays: Record<string, MakeupWorkday> = {};

  for (const row of rows) {
    const date = String(row.date);
    const monthDay = toMonthDay(date);
    const name = row.name ? String(row.name) : '';
    const flag = Number(row.is_holiday);
    if (flag === 2 && name) {
      holidays[monthDay] = { name, name_en: '', type: 'national' };
    } else if (flag === 1) {
      makeupWorkdays[monthDay] = {
        name: name || '補班日',
        name_en: 'Makeup workday',
        for_holiday: '',
      };
    }
  }

  return {
    country,
    year,
    description: `Turso holidays for ${country} ${year}`,
    holidays,
    makeup_workdays: makeupWorkdays,
  };
}

export async function loadHotelAreasFromTurso(): Promise<Record<string, Record<string, string[]>>> {
  const client = getClient();
  const response = await client.execute(
    `SELECT region, area_type, keywords_json FROM hotel_areas ORDER BY region, area_type;`
  );
  const rows = rowsToObjects(response);
  assertRows(rows, 'hotel_areas');

  const out: Record<string, Record<string, string[]>> = {};
  for (const row of rows) {
    const region = String(row.region);
    const areaType = String(row.area_type);
    if (!out[region]) out[region] = {};
    out[region][areaType] = row.keywords_json ? JSON.parse(String(row.keywords_json)) : [];
  }
  return out;
}

export async function loadTransportRoutesFromTurso(): Promise<TransportRoutesData> {
  const client = getClient();
  const response = await client.executeBatch([
    `SELECT region, route_key, time_min, cost_jpy, method FROM transport_routes ORDER BY region, route_key;`,
    `SELECT region, hub_id, hub_type, area FROM transport_hubs ORDER BY region, hub_id;`,
  ]);
  const routeRows = rowsToObjectsAt(response, 0);
  const hubRows = rowsToObjectsAt(response, 1);
  assertRows(routeRows, 'transport_routes');
  assertRows(hubRows, 'transport_hubs');

  const out: TransportRoutesData = {};
  for (const row of routeRows) {
    const region = String(row.region);
    if (!out[region]) out[region] = { routes: {}, hubs: {} };
    out[region].routes[String(row.route_key)] = {
      time_min: Number(row.time_min),
      cost_jpy: Number(row.cost_jpy),
      method: String(row.method || ''),
    };
  }
  for (const row of hubRows) {
    const region = String(row.region);
    if (!out[region]) out[region] = { routes: {}, hubs: {} };
    out[region].hubs[String(row.hub_id)] = {
      type: String(row.hub_type || ''),
      area: String(row.area || ''),
    };
  }
  return out;
}

export interface AirlineRef {
  code: string;
  type: 'LCC' | 'FSC';
  hand_baggage_kg: number;
  checked_bag_included: boolean;
  checked_bag_cost_twd?: number;
  checked_bag_kg?: number;
}

// Stable airline slug keys (match the original ota-knowledge.json keys and the
// nameMap in compare-true-cost.ts), keyed by IATA code.
const AIRLINE_SLUG_BY_CODE: Record<string, string> = {
  MM: 'peach',
  SL: 'thai_lion',
  IT: 'tigerair',
  GK: 'jetstar',
  VZ: 'thai_vietjet',
  BR: 'eva',
  CI: 'china_airlines',
  JX: 'starlux',
};

/**
 * Load airline baggage reference, keyed by stable slug (peach, eva, thai_lion, …)
 * to match the lookup logic in compare-true-cost. Replaces reading ota-knowledge.json.
 * Throws if the airlines table is empty (fail loud — no local-file fallback).
 */
export async function loadAirlinesFromTurso(): Promise<Record<string, AirlineRef>> {
  const client = getClient();
  const response = await client.execute(
    `SELECT code, name, type, hand_baggage_kg, checked_bag_included, checked_bag_kg, checked_bag_cost_twd
       FROM airlines ORDER BY code;`
  );
  const rows = rowsToObjects(response);
  assertRows(rows, 'airlines');

  const out: Record<string, AirlineRef> = {};
  for (const row of rows) {
    const code = String(row.code);
    const slug = AIRLINE_SLUG_BY_CODE[code] ?? String(row.name).toLowerCase().replace(/\s+/g, '_');
    out[slug] = {
      code: String(row.code),
      type: String(row.type) === 'FSC' ? 'FSC' : 'LCC',
      hand_baggage_kg: Number(row.hand_baggage_kg),
      checked_bag_included: Number(row.checked_bag_included) === 1,
      checked_bag_kg: row.checked_bag_kg === null ? undefined : Number(row.checked_bag_kg),
      checked_bag_cost_twd: row.checked_bag_cost_twd === null ? undefined : Number(row.checked_bag_cost_twd),
    };
  }
  return out;
}

export async function loadDestinationReferenceFromTurso(slug: string): Promise<Record<string, unknown>> {
  const client = getClient();
  const esc = sqlEscape(slug);
  const response = await client.executeBatch([
    `SELECT display_name, timezone, currency, primary_airports_json, tips_json
       FROM destination_config WHERE slug = '${esc}';`,
    `SELECT area_id, name, type, stations_json, vibe, best_for_json
       FROM destination_areas WHERE slug = '${esc}' ORDER BY area_id;`,
    `SELECT poi_id, title, area, nearest_station, duration_min, booking_required, booking_url,
            cost_estimate, tags_json, notes, hours, address
       FROM destination_pois WHERE slug = '${esc}' ORDER BY poi_id;`,
    `SELECT cluster_id, name, description, pois_json, duration_min, best_area
       FROM destination_clusters WHERE slug = '${esc}' ORDER BY cluster_id;`,
    `SELECT pair_key, kind, minutes, line, station_from, station_to
       FROM destination_transit WHERE slug = '${esc}' ORDER BY kind, pair_key;`,
  ]);

  const configRows = rowsToObjectsAt(response, 0);
  assertRows(configRows, `destination_config slug=${slug}`);
  const cfg = configRows[0];
  const areaRows = rowsToObjectsAt(response, 1);
  const poiRows = rowsToObjectsAt(response, 2);
  const clusterRows = rowsToObjectsAt(response, 3);
  const transitRows = rowsToObjectsAt(response, 4);

  const parseJson = (v: any, fallback: any) => {
    if (v === null || v === undefined) return fallback;
    try { return JSON.parse(String(v)); } catch { return fallback; }
  };

  const areas = areaRows.map((r) => ({
    id: String(r.area_id),
    name: r.name ?? null,
    type: r.type ?? null,
    stations: parseJson(r.stations_json, []),
    vibe: r.vibe ?? null,
    best_for: parseJson(r.best_for_json, []),
  }));

  const pois = poiRows.map((r) => ({
    id: String(r.poi_id),
    title: r.title ?? null,
    area: r.area ?? null,
    nearest_station: r.nearest_station ?? null,
    duration_min: r.duration_min === null ? null : Number(r.duration_min),
    booking_required: Number(r.booking_required) === 1,
    ...(r.booking_url ? { booking_url: String(r.booking_url) } : {}),
    cost_estimate: r.cost_estimate === null ? null : Number(r.cost_estimate),
    tags: parseJson(r.tags_json, []),
    ...(r.notes ? { notes: String(r.notes) } : {}),
    ...(r.hours ? { hours: String(r.hours) } : {}),
    ...(r.address ? { address: String(r.address) } : {}),
  }));

  // clusters as a keyed object (cluster_id → cluster), matching DestinationRefSchema.
  const clusters: Record<string, unknown> = {};
  for (const r of clusterRows) {
    clusters[String(r.cluster_id)] = {
      name: r.name ?? null,
      description: r.description ?? null,
      pois: parseJson(r.pois_json, []),
      duration_min: r.duration_min === null ? undefined : Number(r.duration_min),
      best_area: r.best_area ?? undefined,
    };
  }

  const transit_estimates: Record<string, unknown> = {};
  const inter_city_transit: Record<string, unknown> = {};
  for (const r of transitRows) {
    const entry: Record<string, unknown> = { minutes: Number(r.minutes), line: r.line ?? null };
    if (r.station_from) entry.station_from = String(r.station_from);
    if (r.station_to) entry.station_to = String(r.station_to);
    if (String(r.kind) === 'inter_city') inter_city_transit[String(r.pair_key)] = entry;
    else transit_estimates[String(r.pair_key)] = entry;
  }

  return {
    destination_id: slug,
    display_name: cfg.display_name ?? null,
    timezone: cfg.timezone ?? null,
    currency: cfg.currency ?? null,
    primary_airports: parseJson(cfg.primary_airports_json, []),
    areas,
    pois,
    clusters,
    transit_estimates,
    ...(Object.keys(inter_city_transit).length ? { inter_city_transit } : {}),
    tips: parseJson(cfg.tips_json, []),
  };
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

export async function importOffersFromFiles(
  files: string[],
  opts?: {
    destination?: string;
    region?: string;
    start?: string;
    end?: string;
    includeUndated?: boolean;
  },
): Promise<ImportResult> {
  const client = getClient();
  const importer = requireImporter();
  const destinationOverride = opts?.destination ?? null;
  const regionOverride = opts?.region ?? null;
  const startFilter = opts?.start ?? null;
  const endFilter = opts?.end ?? null;
  const includeUndated = opts?.includeUndated ?? true;

  let imported = 0;
  let skipped = 0;
  let filtered = 0;
  const sqlStatements: string[] = [];

  for (const filePath of files) {
    if (!fs.existsSync(filePath)) {
      skipped++;
      continue;
    }

    const fileName = path.basename(filePath);
    let raw: unknown;
    try {
      raw = JSON.parse(fs.readFileSync(filePath, 'utf-8')) as unknown;
    } catch {
      skipped++;
      continue;
    }

    const offers: any[] = [];

    if (raw && typeof raw === 'object' && !Array.isArray(raw)) {
      const obj = raw as Record<string, unknown>;
      const maybeOffers = obj.offers;
      if (Array.isArray(maybeOffers)) {
        for (const o of maybeOffers) {
          if (!o || typeof o !== 'object' || Array.isArray(o)) continue;
          const row = importer.parseScraperOfferAsOfferRow(
            fileName,
            o as Record<string, unknown>,
            destinationOverride,
            regionOverride,
          );
          if (row) offers.push(row);
        }
      } else {
        const row = importer.parseRawScrapeAsOfferRow(
          fileName,
          obj,
          destinationOverride,
          regionOverride,
        );
        if (row) offers.push(row);
      }
    }

    if (offers.length === 0) {
      skipped++;
      continue;
    }

    const filteredOffers = offers.filter((row: any) => {
      const inRange = importer.isWithinDateRange(
        row.departure_date,
        startFilter,
        endFilter,
        includeUndated,
      );
      if (!inRange) filtered++;
      return inRange;
    });

    for (const row of filteredOffers) {
      row.source_file = fileName;
      sqlStatements.push(importer.toInsertSql(row));
    }
    imported += filteredOffers.length;
  }

  if (sqlStatements.length > 0) {
    await client.executeMany(sqlStatements, 25);
  }

  return { imported, skipped, filtered };
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

export async function queryOffers(
  filters: TursoOfferQuery,
): Promise<TursoOfferResult[]> {
  const client = getClient();

  // When planId is provided, query plan_offers (normalized tables)
  if (filters.planId) {
    const conditions: string[] = [`po.plan_id = '${sqlEscape(filters.planId)}'`];

    if (filters.destination) {
      conditions.push(`po.destination = '${sqlEscape(filters.destination)}'`);
    }
    if (filters.sources && filters.sources.length > 0) {
      const escaped = filters.sources.map((s) => `'${sqlEscape(s)}'`).join(',');
      conditions.push(`po.source_id IN (${escaped})`);
    }
    if (filters.type) {
      conditions.push(`po.type = '${sqlEscape(filters.type)}'`);
    }
    if (filters.maxPrice != null) {
      conditions.push(`po.price_per_person <= ${sqlInt(filters.maxPrice)}`);
    }
    if (filters.start) {
      conditions.push(`(pob.best_date IS NULL OR pob.best_date >= '${sqlEscape(filters.start)}')`);
    }
    if (filters.end) {
      conditions.push(`(pob.best_date IS NULL OR pob.best_date <= '${sqlEscape(filters.end)}')`);
    }
    if (filters.freshHours != null) {
      conditions.push(`po.scraped_at >= datetime('now', '-${Math.trunc(filters.freshHours)} hours')`);
    }

    const where = `WHERE ${conditions.join(' AND ')}`;
    const limit = filters.limit ? `LIMIT ${Math.trunc(filters.limit)}` : 'LIMIT 100';

    const sql = `SELECT po.id, po.source_id, po.type, po.title AS name,
      po.price_per_person, po.currency, NULL AS region, po.destination,
      pob.best_date AS departure_date, NULL AS return_date, NULL AS nights,
      po.availability, poh.name AS hotel_name,
      pof.airline, po.scraped_at, po.url AS source_file
    FROM plan_offers po
    LEFT JOIN plan_offer_hotels poh ON poh.offer_id = po.id AND poh.plan_id = po.plan_id AND poh.destination = po.destination
    LEFT JOIN plan_offer_flights pof ON pof.offer_id = po.id AND pof.plan_id = po.plan_id AND pof.destination = po.destination AND pof.direction = 'outbound'
    LEFT JOIN plan_offer_best_value pob ON pob.offer_id = po.id AND pob.plan_id = po.plan_id AND pob.destination = po.destination
    ${where} ORDER BY po.scraped_at DESC, po.price_per_person ASC ${limit};`;

    const response = await client.execute(sql);
    return rowsToObjects(response) as TursoOfferResult[];
  }

  // Legacy path: query offers table
  const conditions: string[] = [];

  if (filters.destination) {
    conditions.push(`destination = '${sqlEscape(filters.destination)}'`);
  }
  if (filters.region) {
    conditions.push(`region = '${sqlEscape(filters.region)}'`);
  }
  if (filters.start) {
    conditions.push(`departure_date >= '${sqlEscape(filters.start)}'`);
  }
  if (filters.end) {
    conditions.push(`departure_date <= '${sqlEscape(filters.end)}'`);
  }
  if (filters.sources && filters.sources.length > 0) {
    const escaped = filters.sources.map((s) => `'${sqlEscape(s)}'`).join(',');
    conditions.push(`source_id IN (${escaped})`);
  }
  if (filters.type) {
    conditions.push(`type = '${sqlEscape(filters.type)}'`);
  }
  if (filters.maxPrice != null) {
    conditions.push(`price_per_person <= ${sqlInt(filters.maxPrice)}`);
  }
  if (filters.freshHours != null) {
    conditions.push(
      `scraped_at >= datetime('now', '-${Math.trunc(filters.freshHours)} hours')`,
    );
  }

  const where = conditions.length > 0 ? `WHERE ${conditions.join(' AND ')}` : '';
  const limit = filters.limit ? `LIMIT ${Math.trunc(filters.limit)}` : 'LIMIT 100';

  const sql = `SELECT id, source_id, type, name, price_per_person, currency, region, destination, departure_date, return_date, nights, availability, hotel_name, airline, scraped_at, source_file FROM offers ${where} ORDER BY scraped_at DESC, price_per_person ASC ${limit};`;

  const response = await client.execute(sql);
  return rowsToObjects(response) as TursoOfferResult[];
}

export async function queryRawOffers(
  filters: TursoOfferQuery,
): Promise<TursoRawOfferResult[]> {
  const client = getClient();
  const conditions: string[] = [];

  if (filters.destination) {
    conditions.push(`destination = '${sqlEscape(filters.destination)}'`);
  }
  if (filters.region) {
    conditions.push(`region = '${sqlEscape(filters.region)}'`);
  }
  if (filters.start) {
    conditions.push(`departure_date >= '${sqlEscape(filters.start)}'`);
  }
  if (filters.end) {
    conditions.push(`departure_date <= '${sqlEscape(filters.end)}'`);
  }
  if (filters.sources && filters.sources.length > 0) {
    const escaped = filters.sources.map((s) => `'${sqlEscape(s)}'`).join(',');
    conditions.push(`source_id IN (${escaped})`);
  }
  if (filters.type) {
    conditions.push(`type = '${sqlEscape(filters.type)}'`);
  }
  if (filters.maxPrice != null) {
    conditions.push(`price_per_person <= ${sqlInt(filters.maxPrice)}`);
  }

  const where = conditions.length > 0 ? `WHERE ${conditions.join(' AND ')}` : '';
  const limit = filters.limit ? `LIMIT ${Math.trunc(filters.limit)}` : 'LIMIT 500';
  const sql = `SELECT id, source_id, type, name, price_per_person, currency, region, destination, departure_date, return_date, nights, availability, hotel_name, airline, scraped_at, source_file, raw_data FROM offers ${where} ORDER BY scraped_at DESC, price_per_person ASC ${limit};`;

  const response = await client.execute(sql);
  const rows = rowsToObjects(response) as TursoRawOfferResult[];
  if (rows.length === 0) {
    throw new Error(`Missing Turso offers for filters ${JSON.stringify(filters)}. Import scraper output with npm run db:import:turso before running this command.`);
  }
  return rows;
}

// ---------------------------------------------------------------------------
// Freshness
// ---------------------------------------------------------------------------

export async function checkFreshness(
  sourceId: string,
  opts?: {
    region?: string;
    start?: string;
    end?: string;
    maxAgeHours?: number;
    planId?: string;
    destination?: string;
  },
): Promise<FreshnessResult> {
  const client = getClient();
  const maxAge = opts?.maxAgeHours ?? 24;

  let sql: string;

  if (opts?.planId && opts?.destination) {
    // Plan-based path: query plan_offer_provenance
    const conditions: string[] = [
      `plan_id = '${sqlEscape(opts.planId)}'`,
      `destination = '${sqlEscape(opts.destination)}'`,
      `source_id = '${sqlEscape(sourceId)}'`,
    ];
    const where = conditions.join(' AND ');
    sql = `SELECT MAX(scraped_at) as newest, SUM(COALESCE(offer_count, 0)) as cnt FROM plan_offer_provenance WHERE ${where};`;
  } else {
    // Legacy path: query offers table
    const conditions: string[] = [`source_id = '${sqlEscape(sourceId)}'`];
    if (opts?.region) {
      conditions.push(`region = '${sqlEscape(opts.region)}'`);
    }
    if (opts?.start) {
      conditions.push(`departure_date >= '${sqlEscape(opts.start)}'`);
    }
    if (opts?.end) {
      conditions.push(`departure_date <= '${sqlEscape(opts.end)}'`);
    }
    const where = conditions.join(' AND ');
    sql = `SELECT COUNT(*) as cnt, MAX(scraped_at) as newest FROM offers WHERE ${where};`;
  }

  const response = await client.execute(sql);
  const rows = rowsToObjects(response);
  const row = rows[0] || {};

  const count = typeof row.cnt === 'number' ? row.cnt : parseInt(row.cnt || '0', 10);
  const newest = row.newest as string | null;

  if (count === 0 || !newest) {
    return {
      hasFreshData: false,
      ageHours: null,
      offerCount: 0,
      recommendation: 'no_data',
      region: opts?.region,
    };
  }

  const newestMs = new Date(newest + (newest.includes('Z') ? '' : 'Z')).getTime();

  // Guard against malformed scraped_at producing NaN
  if (isNaN(newestMs)) {
    return {
      hasFreshData: false,
      ageHours: null,
      offerCount: count,
      recommendation: 'rescrape',
      region: opts?.region,
    };
  }

  const ageMs = Date.now() - newestMs;
  const ageHours = ageMs / (1000 * 60 * 60);

  const hasFreshData = ageHours <= maxAge;

  return {
    hasFreshData,
    ageHours,
    offerCount: count,
    recommendation: hasFreshData ? 'skip' : 'rescrape',
    region: opts?.region,
  };
}

// ---------------------------------------------------------------------------
// Formatting helper (for CLI table output)
// ---------------------------------------------------------------------------

export function printTursoOfferTable(results: TursoOfferResult[]): void {
  if (results.length === 0) {
    console.log('\nNo offers found in Turso.');
    return;
  }

  console.log(`\nTurso Offers (${results.length} results):`);
  console.log('─'.repeat(100));

  const header = [
    'Source'.padEnd(12),
    'Type'.padEnd(8),
    'Price'.padStart(8),
    'Hotel'.padEnd(25),
    'Airline'.padEnd(10),
    'Depart'.padEnd(12),
    'Scraped'.padEnd(20),
  ].join(' │ ');
  console.log(header);
  console.log('─'.repeat(100));

  for (const r of results) {
    const price = r.price_per_person != null ? `${r.price_per_person}` : '-';
    const hotel = (r.hotel_name || '-').slice(0, 25).padEnd(25);
    const airline = (r.airline || '-').padEnd(10);
    const depart = (r.departure_date || '-').padEnd(12);
    const scraped = r.scraped_at ? r.scraped_at.slice(0, 19) : '-';

    console.log(
      [
        (r.source_id || '-').padEnd(12),
        (r.type || '-').padEnd(8),
        price.padStart(8),
        hotel,
        airline,
        depart,
        scraped.padEnd(20),
      ].join(' │ '),
    );
  }

  console.log('─'.repeat(100));
}

// ---------------------------------------------------------------------------
// Plan DB-Primary (plans)
// ---------------------------------------------------------------------------

/**
 * Derive a plan ID from the file path.
 * data/trips/<id>/travel-plan.json → "<id>"
 * Other paths → "path:<sha1-prefix>" (hash of canonical absolute path)
 */
export function derivePlanId(planPath: string): string {
  const normalize = (p: string): string => p.replace(/\\/g, '/');
  const canonicalAbs = (p: string): string => {
    const resolved = path.resolve(p);
    try {
      return normalize(fs.realpathSync(resolved));
    } catch {
      return normalize(resolved);
    }
  };

  const canonicalPath = canonicalAbs(planPath);
  const relFromRoot = normalize(path.relative(canonicalAbs(process.cwd()), canonicalPath));

  const tripsMatch = relFromRoot.match(/^data\/trips\/([^/]+)\//);
  if (tripsMatch) return tripsMatch[1];

  const hash = crypto.createHash('sha1').update(canonicalPath).digest('hex').slice(0, 12);
  return `path:${hash}`;
}

/**
 * Sync events to Turso events table with SHA1-based idempotency.
 * Same pattern as turso-sync-events.ts — hash event content → external_id.
 */
export async function syncEventsToDb(
  events: Array<{
    at: string;
    event: string;
    destination?: string;
    process?: string;
    data?: unknown;
  }>
): Promise<{ synced: number; skipped: number }> {
  if (events.length === 0) return { synced: 0, skipped: 0 };

  const client = getClient();
  const sqlStatements: string[] = [];

  for (const ev of events) {
    const payload = JSON.stringify({
      at: ev.at,
      event: ev.event,
      destination: ev.destination,
      process: ev.process,
      data: ev.data,
    });
    const eid = crypto.createHash('sha1').update(payload).digest('hex');

    const cols = ['external_id', 'event_type', 'destination', 'process', 'data', 'created_at'];
    const values = [
      sqlText(eid),
      sqlText(ev.event),
      sqlText(ev.destination || null),
      sqlText(ev.process || null),
      sqlText(ev.data ? JSON.stringify(ev.data) : null),
      sqlText(ev.at),
    ];

    sqlStatements.push(
      `INSERT INTO events (${cols.join(',')}) VALUES (${values.join(',')}) ON CONFLICT(external_id) DO NOTHING;`
    );
  }

  await client.executeMany(sqlStatements, 50);
  return { synced: sqlStatements.length, skipped: 0 };
}

// ---------------------------------------------------------------------------
// Booking Current Types
// ---------------------------------------------------------------------------

export interface BookingCurrentRow {
  booking_key: string;
  trip_id: string;
  destination: string;
  category: 'package' | 'transfer' | 'activity';
  subtype: string | null;
  title: string;
  status: string;
  reference: string | null;
  book_by: string | null;
  booked_at: string | null;
  source_id: string | null;
  offer_id: string | null;
  selected_date: string | null;
  price_amount: number | null;
  price_currency: string | null;
  origin_path: string | null;
  payload_json: string | null;
  updated_at: string | null;
}

// ---------------------------------------------------------------------------
// Booking Sync (plan in DB → bookings_current)
// ---------------------------------------------------------------------------

function extractBookingsFromPlanObject(
  plan: Record<string, unknown>,
  tripId: string,
  extractor: {
    extractPackageBookings: (plan: Record<string, unknown>, tripId: string, dest: string) => any[];
    extractTransferBookings: (plan: Record<string, unknown>, tripId: string, dest: string) => any[];
    extractActivityBookings: (plan: Record<string, unknown>, tripId: string, dest: string) => any[];
  }
): { bookings: any[]; warnings: string[] } {
  const warnings: string[] = [];
  const destinations = plan.destinations as Record<string, Record<string, unknown>> | undefined;
  if (!destinations) {
    return { bookings: [], warnings: ['No destinations found in plan'] };
  }

  const bookings: any[] = [];
  for (const dest of Object.keys(destinations)) {
    try {
      bookings.push(...extractor.extractPackageBookings(plan, tripId, dest));
    } catch (e) {
      warnings.push(`Package extraction failed for ${dest}: ${(e as Error).message}`);
    }
    try {
      bookings.push(...extractor.extractTransferBookings(plan, tripId, dest));
    } catch (e) {
      warnings.push(`Transfer extraction failed for ${dest}: ${(e as Error).message}`);
    }
    try {
      bookings.push(...extractor.extractActivityBookings(plan, tripId, dest));
    } catch (e) {
      warnings.push(`Activity extraction failed for ${dest}: ${(e as Error).message}`);
    }
  }

  return { bookings, warnings };
}

/**
 * Sync bookings from in-memory plan JSON to bookings_current table.
 * This is the DB-native API that does not read from filesystem.
 * 
 * @param plan - The plan object (in-memory, already parsed)
 * @param tripId - The trip ID for booking keys
 * @param opts - Options: dryRun to skip actual DB writes
 */
export async function syncBookingsFromPlanJson(
  plan: Record<string, unknown>,
  tripId: string,
  opts?: { dryRun?: boolean }
): Promise<{ synced: number; warnings: string[] }> {
  const extractor = requireExtractor();
  const { bookings, warnings } = extractBookingsFromPlanObject(plan, tripId, extractor);

  if (bookings.length === 0) {
    return { synced: 0, warnings };
  }

  if (opts?.dryRun) {
    return { synced: bookings.length, warnings };
  }

  const client = getClient();

  // Collect trip IDs for DELETE scope
  const tripIds = [...new Set(bookings.map(b => b.trip_id))];

  // Fetch existing rows for diff-based event emission
  const existingMap = new Map<string, BookingCurrentRow>();
  for (const tid of tripIds) {
    const existing = await queryBookings({ tripId: tid });
    for (const row of existing) {
      existingMap.set(row.booking_key, row);
    }
  }

  const sqlStatements: string[] = [];

  // Transaction: DELETE stale rows, then upsert current bookings
  sqlStatements.push('BEGIN;');

  // Delete all rows for these trip IDs (prevents stale ghost rows)
  for (const tid of tripIds) {
    sqlStatements.push(`DELETE FROM bookings_current WHERE trip_id = '${sqlEscape(tid)}';`);
  }

  // Upsert current bookings + diff-based events
  for (const row of bookings) {
    sqlStatements.push(extractor.toUpsertSql(row));

    // Only emit event if something actually changed
    const prev = existingMap.get(row.booking_key);
    if (!prev) {
      sqlStatements.push(extractor.toEventSql(row.booking_key, 'created', row));
    } else if (
      prev.status !== row.status ||
      prev.reference !== row.reference ||
      prev.book_by !== row.book_by ||
      prev.price_amount !== row.price_amount ||
      prev.title !== row.title
    ) {
      sqlStatements.push(extractor.toEventSql(row.booking_key, 'updated', row));
    }
    // No event if nothing changed
  }

  sqlStatements.push('COMMIT;');

  await client.executeMany(sqlStatements, 50);

  return { synced: bookings.length, warnings };
}


// ---------------------------------------------------------------------------
// Query Bookings
// ---------------------------------------------------------------------------

export async function queryBookings(filters: {
  tripId?: string;
  destination?: string;
  category?: 'package' | 'transfer' | 'activity';
  status?: string;
  limit?: number;
}): Promise<BookingCurrentRow[]> {
  const client = getClient();

  const conditions: string[] = [];

  if (filters.tripId) {
    conditions.push(`trip_id = '${sqlEscape(filters.tripId)}'`);
  }
  if (filters.destination) {
    conditions.push(`destination = '${sqlEscape(filters.destination)}'`);
  }
  if (filters.category) {
    conditions.push(`category = '${sqlEscape(filters.category)}'`);
  }
  if (filters.status) {
    conditions.push(`status = '${sqlEscape(filters.status)}'`);
  }

  const where = conditions.length > 0 ? `WHERE ${conditions.join(' AND ')}` : '';
  const limit = filters.limit ? `LIMIT ${Math.trunc(filters.limit)}` : 'LIMIT 100';

  const sql = `SELECT booking_key, trip_id, destination, category, subtype, title, status, reference, book_by, booked_at, source_id, offer_id, selected_date, price_amount, price_currency, origin_path, payload_json, updated_at FROM bookings_current ${where} ORDER BY category, destination, updated_at DESC ${limit};`;

  const response = await client.execute(sql);
  return rowsToObjects(response) as BookingCurrentRow[];
}

// ---------------------------------------------------------------------------
// Booking Integrity Check
// ---------------------------------------------------------------------------

/**
 * Compare bookings derived from the in-memory plan against bookings_current in DB.
 * Caller is responsible for providing the plan object (e.g. from StateManager.getPlan()).
 */
export async function checkBookingIntegrity(
  plan: Record<string, unknown>,
  tripId: string,
): Promise<{ matches: number; mismatches: string[]; dbOnly: string[]; planOnly: string[] }> {
  const extractor = requireExtractor();

  const { bookings: planBookings, warnings } = extractBookingsFromPlanObject(plan, tripId, extractor);

  const dbBookings = await queryBookings({ tripId });

  const planKeys = new Map<string, any>();
  for (const b of planBookings) {
    planKeys.set(b.booking_key, b);
  }

  const dbKeys = new Map<string, BookingCurrentRow>();
  for (const b of dbBookings) {
    dbKeys.set(b.booking_key, b);
  }

  let matches = 0;
  const mismatches: string[] = [];
  const planOnly: string[] = [];
  const dbOnly: string[] = [];

  for (const [key, planRow] of planKeys) {
    const dbRow = dbKeys.get(key);
    if (!dbRow) {
      planOnly.push(`${key} (${planRow.category}: ${planRow.title})`);
    } else {
      // Compare key fields
      const diffs: string[] = [];
      if (dbRow.status !== planRow.status) diffs.push(`status: plan=${planRow.status} db=${dbRow.status}`);
      if ((dbRow.reference || null) !== (planRow.reference || null)) diffs.push(`reference: plan=${planRow.reference} db=${dbRow.reference}`);
      if ((dbRow.book_by || null) !== (planRow.book_by || null)) diffs.push(`book_by: plan=${planRow.book_by} db=${dbRow.book_by}`);
      if (dbRow.price_amount !== planRow.price_amount) diffs.push(`price: plan=${planRow.price_amount} db=${dbRow.price_amount}`);
      if (dbRow.title !== planRow.title) diffs.push(`title: plan="${planRow.title}" db="${dbRow.title}"`);

      if (diffs.length > 0) {
        mismatches.push(`${key}: ${diffs.join(', ')}`);
      } else {
        matches++;
      }
    }
  }

  for (const [key, dbRow] of dbKeys) {
    if (!planKeys.has(key)) {
      dbOnly.push(`${key} (${dbRow.category}: ${dbRow.title})`);
    }
  }

  for (const w of warnings) {
    mismatches.push(`extractor warning: ${w}`);
  }

  return { matches, mismatches, dbOnly, planOnly };
}

// ---------------------------------------------------------------------------
// Booking Table Formatter
// ---------------------------------------------------------------------------

export function printBookingsTable(results: BookingCurrentRow[]): void {
  if (results.length === 0) {
    console.log('\nNo bookings found.');
    return;
  }

  console.log(`\nBookings (${results.length} rows):`);
  console.log('─'.repeat(95));

  const header = [
    'Category'.padEnd(10),
    'Status'.padEnd(10),
    'Title'.padEnd(40),
    'Price'.padStart(8),
    'Book By'.padEnd(12),
    'Ref'.padEnd(10),
  ].join(' │ ');
  console.log(header);
  console.log('─'.repeat(95));

  for (const r of results) {
    const price = r.price_amount != null ? `${r.price_amount}` : '-';
    const bookBy = r.book_by || '-';
    const ref = (r.reference || '-').slice(0, 10);

    console.log(
      [
        (r.category || '-').padEnd(10),
        (r.status || '-').padEnd(10),
        (r.title || '-').slice(0, 40).padEnd(40),
        price.padStart(8),
        bookBy.padEnd(12),
        ref.padEnd(10),
      ].join(' │ '),
    );
  }

  console.log('─'.repeat(95));
}

// ---------------------------------------------------------------------------
// Normalized Table Sync (pipeline execution)
// ---------------------------------------------------------------------------

/**
 * Execute an array of SQL statements as a single pipeline request.
 * Used by syncNormalizedTables to keep BEGIN/COMMIT in one HTTP round-trip.
 */
export async function executePipelineTransaction(statements: string[]): Promise<void> {
  const client = getClient();
  await client.executeMany(statements, statements.length);
}

/**
 * Execute a single SQL statement and attempt rollback. Best-effort.
 */
export async function executePipelineRollback(): Promise<void> {
  const client = getClient();
  try { await client.execute('ROLLBACK'); } catch { /* best effort */ }
}

// ---------------------------------------------------------------------------
// Operation Run Queries
// ---------------------------------------------------------------------------

export interface OperationRun {
  run_id: string;
  plan_id: string;
  command_type: string;
  command_summary: string | null;
  status: string;
  version_before: number;
  version_after: number | null;
  started_at: string;
  completed_at: string | null;
  error_message: string | null;
}

/**
 * Get a single operation run. If runId is given, returns that run (scoped to planId).
 * Otherwise returns the most recent run for the plan.
 */
export async function getOperationRun(planId: string, runId?: string): Promise<OperationRun | null> {
  const client = getClient();
  let sql: string;
  if (runId) {
    sql = `SELECT * FROM operation_runs WHERE run_id = ${sqlText(runId)} AND plan_id = ${sqlText(planId)} LIMIT 1`;
  } else {
    sql = `SELECT * FROM operation_runs WHERE plan_id = ${sqlText(planId)} ORDER BY started_at DESC LIMIT 1`;
  }
  const resp = await client.execute(sql);
  const rows = rowsToObjects(resp);
  return rows.length > 0 ? rows[0] as unknown as OperationRun : null;
}

/**
 * List recent operation runs for a plan.
 */
export async function listOperationRuns(
  planId: string,
  opts?: { status?: string; limit?: number }
): Promise<OperationRun[]> {
  const client = getClient();
  const limit = Math.max(1, Math.min(100, opts?.limit ?? 20));
  const conditions: string[] = [`plan_id = ${sqlText(planId)}`];
  if (opts?.status) {
    conditions.push(`status = ${sqlText(opts.status)}`);
  }
  const sql = `SELECT run_id, command_type, command_summary, status, version_before, version_after, started_at, completed_at, error_message
    FROM operation_runs WHERE ${conditions.join(' AND ')}
    ORDER BY started_at DESC LIMIT ${limit}`;
  const resp = await client.execute(sql);
  return rowsToObjects(resp) as unknown as OperationRun[];
}

/**
 * Log an operation run start to operation_runs table.
 */
export async function logOperationStart(
  runId: string, planId: string, commandType: string, summary: string | null, version: number
): Promise<void> {
  const client = getClient();
  await client.execute(
    `INSERT INTO operation_runs (run_id, plan_id, command_type, command_summary, status, version_before, started_at)
     VALUES (${sqlText(runId)}, ${sqlText(planId)}, ${sqlText(commandType)}, ${sqlText(summary as string)}, 'started', ${version}, datetime('now'))`
  );
}

/**
 * Log an operation run completion.
 */
export async function logOperationComplete(runId: string, versionAfter: number): Promise<void> {
  const client = getClient();
  await client.execute(
    `UPDATE operation_runs SET status = 'completed', version_after = ${versionAfter}, completed_at = datetime('now') WHERE run_id = ${sqlText(runId)}`
  );
}

/**
 * Log an operation run failure.
 */
export async function logOperationFailed(runId: string, err: unknown): Promise<void> {
  const client = getClient();
  const msg = err instanceof Error ? err.message.substring(0, 500) : 'unknown';
  await client.execute(
    `UPDATE operation_runs SET status = 'failed', error_message = ${sqlText(msg)}, completed_at = datetime('now') WHERE run_id = ${sqlText(runId)}`
  );
}
