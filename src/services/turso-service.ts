/**
 * Turso Service — thin wrapper around TursoPipelineClient + import parsing.
 *
 * Provides:
 *   importOffersFromFiles  — bulk import scrape JSON → Turso offers table
 *   queryOffers            — filtered reads from Turso
 *   checkFreshness         — staleness check for source/region
 *   writePlanToDb          — upsert plan+state to plans (DB-primary)
 *   readPlanFromDb         — read plan+state from plans
 *   syncEventsToDb         — idempotent event sync via SHA1 external_id
 *   syncBookingsFromPlan   — extract + upsert bookings from plan JSON
 *
 * Turso is required infrastructure. If TURSO_TOKEN is missing the service
 * throws with a clear error — no silent skipping.
 */

import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';

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
  const cols = extractColumns(response);
  const rows = extractRows(response);
  return rows.map((row: any[]) => {
    const obj: Record<string, any> = {};
    cols.forEach((col: string, i: number) => {
      const cell = row[i];
      obj[col] = cell?.value ?? null;
    });
    return obj;
  });
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
  },
): Promise<FreshnessResult> {
  const client = getClient();
  const maxAge = opts?.maxAgeHours ?? 24;

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

  // Get newest scraped_at and count
  const sql = `SELECT COUNT(*) as cnt, MAX(scraped_at) as newest FROM offers WHERE ${where};`;
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
 * Write plan + state JSON to Turso plans (upsert).
 * This is the blocking DB write used by StateManager.save().
 *
 * When stateJson is null, the existing state_json in DB is preserved
 * (COALESCE prevents cascade-only writes from erasing state).
 */
export async function writePlanToDb(
  planId: string,
  planJson: string,
  stateJson: string | null,
  schemaVersion: string
): Promise<void> {
  const client = getClient();
  const sql = `INSERT INTO plans (plan_id, schema_version, plan_json, state_json, updated_at)
VALUES (${sqlText(planId)}, ${sqlText(schemaVersion)}, ${sqlText(planJson)}, ${sqlText(stateJson)}, datetime('now'))
ON CONFLICT(plan_id) DO UPDATE SET
  schema_version = ${sqlText(schemaVersion)},
  plan_json = ${sqlText(planJson)},
  state_json = COALESCE(excluded.state_json, plans.state_json),
  updated_at = datetime('now');`;
  await client.execute(sql);
}

/**
 * Read plan + state from Turso plans.
 * Returns null if no row found for planId.
 * Includes version counter for audit trail (defaults to 0 for pre-migration rows).
 */
export async function readPlanFromDb(
  planId: string
): Promise<{ plan_json: string; state_json: string | null; updated_at: string; version: number } | null> {
  const client = getClient();
  const sql = `SELECT plan_json, state_json, updated_at, COALESCE(version, 0) as version FROM plans WHERE plan_id = ${sqlText(planId)} LIMIT 1;`;
  const response = await client.execute(sql);
  const rows = rowsToObjects(response);
  if (rows.length === 0) return null;
  return {
    plan_json: rows[0].plan_json as string,
    state_json: rows[0].state_json as string | null,
    updated_at: rows[0].updated_at as string,
    version: typeof rows[0].version === 'number' ? rows[0].version : parseInt(rows[0].version || '0', 10),
  };
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

function inferTripIdFromPlanPath(planPath: string): string {
  const normalized = planPath.replace(/\\/g, '/');
  const m = normalized.match(/\/?data\/trips\/([^/]+)\//);
  if (m) return m[1];
  return 'japan-2026';
}

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

/**
 * @deprecated Use syncBookingsFromPlanJson instead. This path-based API
 * will be removed in a future version.
 */
export async function syncBookingsFromPlan(
  planPath: string,
  opts?: { tripId?: string; dryRun?: boolean }
): Promise<{ synced: number; warnings: string[] }> {
  const extractor = requireExtractor();
  const planId = derivePlanId(planPath);

  const dbRow = await readPlanFromDb(planId);
  if (!dbRow) {
    return {
      synced: 0,
      warnings: [`Plan not found in DB for plan_id="${planId}". Run 'npm run db:seed:plans' first.`],
    };
  }

  let plan: Record<string, unknown>;
  try {
    plan = JSON.parse(dbRow.plan_json) as Record<string, unknown>;
  } catch (e) {
    return {
      synced: 0,
      warnings: [`Failed to parse plan_json in DB for plan_id="${planId}": ${(e as Error).message}`],
    };
  }

  const effectiveTripId = opts?.tripId || inferTripIdFromPlanPath(planPath);
  const { bookings, warnings } = extractBookingsFromPlanObject(plan, effectiveTripId, extractor);

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
// Plan Snapshot
// ---------------------------------------------------------------------------

/**
 * Create a snapshot of plan+state by planId (DB-native).
 * This is the preferred API - does not require file paths.
 * 
 * @param planId - The plan ID in plans
 * @param tripId - The trip ID for the snapshot
 */
export async function createPlanSnapshotByPlanId(
  planId: string,
  tripId: string
): Promise<{ snapshot_id: string; trip_id: string }> {
  const dbRow = await readPlanFromDb(planId);
  if (!dbRow) {
    throw new Error(`Plan "${planId}" not found in DB. Run 'npm run db:seed:plans' first.`);
  }

  const planJson = dbRow.plan_json;
  const stateJson = dbRow.state_json;
  const plan = JSON.parse(planJson);
  const schemaVersion = (plan.schema_version as string) || 'unknown';
  const snapshotId = `${tripId}_${new Date().toISOString().replace(/[:.]/g, '-')}`;

  const client = getClient();
  const sql = `INSERT INTO plan_snapshots (snapshot_id, trip_id, schema_version, plan_json, state_json, created_at) VALUES (${sqlText(snapshotId)}, ${sqlText(tripId)}, ${sqlText(schemaVersion)}, ${sqlText(planJson)}, ${sqlText(stateJson)}, datetime('now'));`;

  await client.execute(sql);

  return { snapshot_id: snapshotId, trip_id: tripId };
}

/**
 * @deprecated Use createPlanSnapshotByPlanId instead. This path-based API
 * will be removed in a future version.
 */
export async function createPlanSnapshot(
  planPath: string,
  _statePath: string,
  tripId: string
): Promise<{ snapshot_id: string; trip_id: string }> {
  const planId = derivePlanId(planPath);
  return createPlanSnapshotByPlanId(planId, tripId);
}

// ---------------------------------------------------------------------------
// Booking Integrity Check
// ---------------------------------------------------------------------------

export async function checkBookingIntegrity(
  planPath: string,
  tripId?: string
): Promise<{ matches: number; mismatches: string[]; dbOnly: string[]; planOnly: string[] }> {
  const extractor = requireExtractor();
  const planId = derivePlanId(planPath);
  const dbRow = await readPlanFromDb(planId);
  if (!dbRow) {
    throw new Error(`Plan "${planId}" not found in DB. Run 'npm run db:seed:plans' first.`);
  }

  let plan: Record<string, unknown>;
  try {
    plan = JSON.parse(dbRow.plan_json) as Record<string, unknown>;
  } catch (e) {
    throw new Error(`Failed to parse plan_json in DB for "${planId}": ${(e as Error).message}`);
  }

  const effectiveTripId = tripId || inferTripIdFromPlanPath(planPath);
  const { bookings: planBookings, warnings } = extractBookingsFromPlanObject(plan, effectiveTripId, extractor);

  const dbBookings = await queryBookings({
    tripId: effectiveTripId || undefined,
  });

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
