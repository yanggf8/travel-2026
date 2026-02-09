/**
 * Turso Service — thin wrapper around TursoPipelineClient + import parsing.
 *
 * Provides:
 *   importOffersFromFiles  — bulk import scrape JSON → Turso offers table
 *   queryOffers            — filtered reads from Turso
 *   checkFreshness         — staleness check for source/region
 *   syncBooking            — upsert booking decision to Turso bookings table
 *
 * Turso is required infrastructure. If TURSO_TOKEN is missing the service
 * throws with a clear error — no silent skipping.
 */

import fs from 'node:fs';
import path from 'node:path';

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

export interface BookingRecord {
  destination: string;
  offer_id: string;
  selected_date: string;
  price_per_person: number;
  price_total: number;
  status: 'selected' | 'booked' | 'confirmed';
  source_id: string;
  hotel_name?: string;
  airline?: string;
  flight_out?: string;
  flight_return?: string;
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
// Booking sync
// ---------------------------------------------------------------------------

export async function syncBooking(booking: BookingRecord): Promise<void> {
  const client = getClient();

  const sql = `INSERT INTO bookings (destination, offer_id, selected_date, price_per_person, price_total, currency, status, source_id, hotel_name, airline, flight_out, flight_return, selected_at, updated_at) VALUES (${sqlText(booking.destination)}, ${sqlText(booking.offer_id)}, ${sqlText(booking.selected_date)}, ${sqlInt(booking.price_per_person)}, ${sqlInt(booking.price_total)}, 'TWD', ${sqlText(booking.status)}, ${sqlText(booking.source_id)}, ${sqlText(booking.hotel_name)}, ${sqlText(booking.airline)}, ${sqlText(booking.flight_out)}, ${sqlText(booking.flight_return)}, datetime('now'), datetime('now')) ON CONFLICT(destination, offer_id) DO UPDATE SET status = ${sqlText(booking.status)}, price_per_person = ${sqlInt(booking.price_per_person)}, price_total = ${sqlInt(booking.price_total)}, updated_at = datetime('now');`;

  await client.execute(sql);
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
// Booking Sync (plan JSON → DB)
// ---------------------------------------------------------------------------

export async function syncBookingsFromPlan(
  planPath: string,
  opts?: { tripId?: string; dryRun?: boolean }
): Promise<{ synced: number; warnings: string[] }> {
  const extractor = requireExtractor();
  const { bookings, warnings } = extractor.extractAllBookings(
    planPath,
    opts?.tripId
  );

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

export async function createPlanSnapshot(
  planPath: string,
  statePath: string,
  tripId: string
): Promise<{ snapshot_id: string; trip_id: string }> {
  const planJson = fs.readFileSync(planPath, 'utf-8');
  const stateJson = fs.existsSync(statePath)
    ? fs.readFileSync(statePath, 'utf-8')
    : null;

  const plan = JSON.parse(planJson);
  const schemaVersion = (plan.schema_version as string) || 'unknown';
  const snapshotId = `${tripId}_${new Date().toISOString().replace(/[:.]/g, '-')}`;

  const client = getClient();
  const sql = `INSERT INTO plan_snapshots (snapshot_id, trip_id, schema_version, plan_json, state_json, created_at) VALUES (${sqlText(snapshotId)}, ${sqlText(tripId)}, ${sqlText(schemaVersion)}, ${sqlText(planJson)}, ${sqlText(stateJson)}, datetime('now'));`;

  await client.execute(sql);

  return { snapshot_id: snapshotId, trip_id: tripId };
}

// ---------------------------------------------------------------------------
// Booking Integrity Check
// ---------------------------------------------------------------------------

export async function checkBookingIntegrity(
  planPath: string,
  tripId?: string
): Promise<{ matches: number; mismatches: string[]; dbOnly: string[]; planOnly: string[] }> {
  const extractor = requireExtractor();
  const { bookings: planBookings } = extractor.extractAllBookings(planPath, tripId);

  const dbBookings = await queryBookings({
    tripId: tripId || undefined,
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
