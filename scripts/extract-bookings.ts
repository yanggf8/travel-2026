/**
 * Booking Extractor — extract bookings from nested travel-plan.json into flat rows.
 *
 * Reads deeply nested JSON paths:
 *   - Package: destinations.{slug}.process_3_4_packages
 *   - Transfer: destinations.{slug}.process_3_transportation.airport_transfers
 *   - Activity: destinations.{slug}.process_5_daily_itinerary.days[*]
 *
 * Produces flat BookingRow[] suitable for Turso bookings_current upsert.
 */

import fs from 'node:fs';
import path from 'node:path';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface BookingRow {
  booking_key: string;
  trip_id: string;
  destination: string;
  category: 'package' | 'transfer' | 'activity';
  subtype: string | null;
  title: string;
  status: 'pending' | 'planned' | 'booked' | 'confirmed' | 'waitlist' | 'skipped' | 'cancelled';
  reference: string | null;
  book_by: string | null;
  booked_at: string | null;
  source_id: string | null;
  offer_id: string | null;
  selected_date: string | null;
  price_amount: number | null;
  price_currency: string;
  origin_path: string;
  payload_json: string | null;
}

export interface ExtractionResult {
  bookings: BookingRow[];
  warnings: string[];
}

// ---------------------------------------------------------------------------
// Key builders
// ---------------------------------------------------------------------------

export function buildBookingKey(
  tripId: string,
  dest: string,
  category: string,
  ...ids: string[]
): string {
  return [tripId, dest, category, ...ids].join(':');
}

// ---------------------------------------------------------------------------
// Status mapping
// ---------------------------------------------------------------------------

type BookingStatus = BookingRow['status'];

function mapPackageStatus(status: string | undefined): BookingStatus {
  switch (status) {
    case 'booked': return 'booked';
    case 'confirmed': return 'confirmed';
    case 'selected':
    case 'populated':
    case 'researched':
    case 'researching':
      return 'pending';
    default: return 'pending';
  }
}

function mapTransferStatus(status: string | undefined): BookingStatus {
  switch (status) {
    case 'booked': return 'booked';
    case 'confirmed': return 'confirmed';
    case 'planned': return 'planned';
    default: return 'pending';
  }
}

function mapActivityBookingStatus(status: string | undefined): BookingStatus {
  switch (status) {
    case 'booked': return 'booked';
    case 'confirmed': return 'confirmed';
    case 'pending': return 'pending';
    case 'waitlist': return 'waitlist';
    case 'not_required': return 'skipped';
    default: return 'pending';
  }
}

// ---------------------------------------------------------------------------
// SQL helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Extractors
// ---------------------------------------------------------------------------

export function extractPackageBookings(
  plan: Record<string, unknown>,
  tripId: string,
  dest: string
): BookingRow[] {
  const rows: BookingRow[] = [];
  const destObj = (plan.destinations as Record<string, Record<string, unknown>>)?.[dest];
  if (!destObj) return rows;

  const p34 = destObj.process_3_4_packages as Record<string, unknown> | undefined;
  if (!p34) return rows;

  const status = p34.status as string | undefined;
  const selectedOfferId = p34.selected_offer_id as string | null | undefined;
  const chosenOffer = p34.chosen_offer as Record<string, unknown> | undefined;
  const results = p34.results as Record<string, unknown> | undefined;
  const fullOffer = results?.chosen_offer as Record<string, unknown> | undefined;

  // Only emit a row if there's a selected offer
  if (!selectedOfferId) return rows;

  const selectedDate = chosenOffer?.selected_date as string | null | undefined;
  const selectedAt = chosenOffer?.selected_at as string | null | undefined;

  // Price from the full offer
  const pricePerPerson = fullOffer?.price_per_person as number | null | undefined;
  const priceCurrency = (fullOffer?.currency as string) || 'TWD';
  const sourceId = fullOffer?.source_id as string | null | undefined;
  const hotelObj = fullOffer?.hotel as Record<string, unknown> | undefined;
  const flightObj = fullOffer?.flight as Record<string, unknown> | undefined;

  const title = [
    sourceId || 'package',
    selectedOfferId,
    hotelObj?.name || '',
  ].filter(Boolean).join(' - ');

  const payload: Record<string, unknown> = {};
  if (hotelObj) payload.hotel = hotelObj;
  if (flightObj) payload.flight = flightObj;
  if (fullOffer?.includes) payload.includes = fullOffer.includes;

  rows.push({
    booking_key: buildBookingKey(tripId, dest, 'package', selectedOfferId, selectedDate || 'no-date'),
    trip_id: tripId,
    destination: dest,
    category: 'package',
    subtype: (fullOffer?.type as string) || 'package',
    title,
    status: mapPackageStatus(status),
    reference: null,
    book_by: null,
    booked_at: selectedAt || null,
    source_id: sourceId || null,
    offer_id: selectedOfferId,
    selected_date: selectedDate || null,
    price_amount: pricePerPerson != null ? Math.trunc(pricePerPerson) : null,
    price_currency: priceCurrency,
    origin_path: `destinations.${dest}.process_3_4_packages`,
    payload_json: Object.keys(payload).length > 0 ? JSON.stringify(payload) : null,
  });

  return rows;
}

export function extractTransferBookings(
  plan: Record<string, unknown>,
  tripId: string,
  dest: string
): BookingRow[] {
  const rows: BookingRow[] = [];
  const destObj = (plan.destinations as Record<string, Record<string, unknown>>)?.[dest];
  if (!destObj) return rows;

  const p3 = destObj.process_3_transportation as Record<string, unknown> | undefined;
  if (!p3) return rows;

  const transfers = p3.airport_transfers as Record<string, unknown> | undefined;
  if (!transfers) return rows;

  for (const direction of ['arrival', 'departure'] as const) {
    const segment = transfers[direction] as Record<string, unknown> | undefined;
    if (!segment) continue;

    const segStatus = segment.status as string | undefined;
    const selected = segment.selected as Record<string, unknown> | undefined;
    if (!selected) continue;

    const selectedId = (selected.id as string) || direction;
    const title = (selected.title as string) || `Airport transfer (${direction})`;
    const route = selected.route as string | undefined;
    const priceYen = selected.price_yen as number | undefined;
    const schedule = selected.schedule as string | undefined;

    const payload: Record<string, unknown> = { ...selected };
    if (route) payload.route = route;
    if (schedule) payload.schedule = schedule;

    rows.push({
      booking_key: buildBookingKey(tripId, dest, 'transfer', direction, selectedId),
      trip_id: tripId,
      destination: dest,
      category: 'transfer',
      subtype: direction,
      title: `${title} (${direction})`,
      status: mapTransferStatus(segStatus),
      reference: null,
      book_by: null,
      booked_at: null,
      source_id: null,
      offer_id: null,
      selected_date: null,
      price_amount: priceYen != null ? Math.trunc(priceYen) : null,
      price_currency: 'JPY',
      origin_path: `destinations.${dest}.process_3_transportation.airport_transfers.${direction}`,
      payload_json: JSON.stringify(payload),
    });
  }

  return rows;
}

export function extractActivityBookings(
  plan: Record<string, unknown>,
  tripId: string,
  dest: string
): BookingRow[] {
  const rows: BookingRow[] = [];
  const destObj = (plan.destinations as Record<string, Record<string, unknown>>)?.[dest];
  if (!destObj) return rows;

  const p5 = destObj.process_5_daily_itinerary as Record<string, unknown> | undefined;
  if (!p5) return rows;

  const days = p5.days as Array<Record<string, unknown>> | undefined;
  if (!days) return rows;

  const sessions = ['morning', 'afternoon', 'evening'] as const;

  for (const day of days) {
    const dayNumber = day.day_number as number;

    for (const session of sessions) {
      const sessionObj = day[session] as Record<string, unknown> | undefined;
      if (!sessionObj) continue;

      const activities = sessionObj.activities as Array<string | Record<string, unknown>> | undefined;
      if (!activities) continue;

      for (const activity of activities) {
        // Only extract structured activities with booking info
        if (typeof activity === 'string') continue;

        const bookingStatus = activity.booking_status as string | undefined;
        const bookingRequired = activity.booking_required as boolean | undefined;

        // Skip activities that don't need booking tracking
        if (!bookingStatus && !bookingRequired) continue;

        const activityId = (activity.id as string) || `day${dayNumber}_${session}`;
        const title = (activity.title as string) || 'Unknown activity';
        const bookingRef = activity.booking_ref as string | undefined;
        const bookBy = activity.book_by as string | undefined;
        const costEstimate = activity.cost_estimate as number | undefined;

        rows.push({
          booking_key: buildBookingKey(tripId, dest, 'activity', `${dayNumber}`, session, activityId),
          trip_id: tripId,
          destination: dest,
          category: 'activity',
          subtype: `day${dayNumber}_${session}`,
          title,
          status: mapActivityBookingStatus(bookingStatus),
          reference: bookingRef || null,
          book_by: bookBy || null,
          booked_at: null,
          source_id: null,
          offer_id: null,
          selected_date: null,
          price_amount: costEstimate != null ? Math.trunc(costEstimate) : null,
          price_currency: 'JPY',
          origin_path: `destinations.${dest}.process_5_daily_itinerary.days[${dayNumber - 1}].${session}`,
          payload_json: JSON.stringify({
            area: activity.area,
            booking_url: activity.booking_url,
            tags: activity.tags,
          }),
        });
      }
    }
  }

  return rows;
}

// ---------------------------------------------------------------------------
// Combined extractor
// ---------------------------------------------------------------------------

export function extractAllBookings(
  planPath: string,
  tripId?: string
): ExtractionResult {
  const warnings: string[] = [];

  if (!fs.existsSync(planPath)) {
    return { bookings: [], warnings: [`Plan file not found: ${planPath}`] };
  }

  let plan: Record<string, unknown>;
  try {
    plan = JSON.parse(fs.readFileSync(planPath, 'utf-8'));
  } catch (e) {
    return { bookings: [], warnings: [`Failed to parse plan: ${(e as Error).message}`] };
  }

  const effectiveTripId = tripId || inferTripId(planPath);
  const destinations = plan.destinations as Record<string, Record<string, unknown>> | undefined;
  if (!destinations) {
    return { bookings: [], warnings: ['No destinations found in plan'] };
  }

  const bookings: BookingRow[] = [];

  for (const dest of Object.keys(destinations)) {
    try {
      bookings.push(...extractPackageBookings(plan, effectiveTripId, dest));
    } catch (e) {
      warnings.push(`Package extraction failed for ${dest}: ${(e as Error).message}`);
    }

    try {
      bookings.push(...extractTransferBookings(plan, effectiveTripId, dest));
    } catch (e) {
      warnings.push(`Transfer extraction failed for ${dest}: ${(e as Error).message}`);
    }

    try {
      bookings.push(...extractActivityBookings(plan, effectiveTripId, dest));
    } catch (e) {
      warnings.push(`Activity extraction failed for ${dest}: ${(e as Error).message}`);
    }
  }

  return { bookings, warnings };
}

// ---------------------------------------------------------------------------
// SQL generators
// ---------------------------------------------------------------------------

export function toUpsertSql(row: BookingRow): string {
  return `INSERT INTO bookings_current (booking_key, trip_id, destination, category, subtype, title, status, reference, book_by, booked_at, source_id, offer_id, selected_date, price_amount, price_currency, origin_path, payload_json, updated_at) VALUES (${sqlText(row.booking_key)}, ${sqlText(row.trip_id)}, ${sqlText(row.destination)}, ${sqlText(row.category)}, ${sqlText(row.subtype)}, ${sqlText(row.title)}, ${sqlText(row.status)}, ${sqlText(row.reference)}, ${sqlText(row.book_by)}, ${sqlText(row.booked_at)}, ${sqlText(row.source_id)}, ${sqlText(row.offer_id)}, ${sqlText(row.selected_date)}, ${sqlInt(row.price_amount)}, ${sqlText(row.price_currency)}, ${sqlText(row.origin_path)}, ${sqlText(row.payload_json)}, datetime('now')) ON CONFLICT(booking_key) DO UPDATE SET status = ${sqlText(row.status)}, reference = ${sqlText(row.reference)}, book_by = ${sqlText(row.book_by)}, booked_at = ${sqlText(row.booked_at)}, price_amount = ${sqlInt(row.price_amount)}, payload_json = ${sqlText(row.payload_json)}, updated_at = datetime('now');`;
}

export function toEventSql(
  bookingKey: string,
  eventType: string,
  row: BookingRow
): string {
  return `INSERT INTO bookings_events (booking_key, event_type, new_status, reference, book_by, amount, currency, event_data, event_at) VALUES (${sqlText(bookingKey)}, ${sqlText(eventType)}, ${sqlText(row.status)}, ${sqlText(row.reference)}, ${sqlText(row.book_by)}, ${sqlInt(row.price_amount)}, ${sqlText(row.price_currency)}, ${sqlText(row.payload_json)}, datetime('now'));`;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function inferTripId(planPath: string): string {
  // Try to extract from path like data/trips/osaka-kyoto-2026/travel-plan.json
  const parts = planPath.split(path.sep);
  const tripsIdx = parts.indexOf('trips');
  if (tripsIdx !== -1 && tripsIdx + 1 < parts.length) {
    return parts[tripsIdx + 1];
  }
  // Default: project name from plan or fallback
  return 'japan-2026';
}

// ---------------------------------------------------------------------------
// CLI entry point (for standalone use)
// ---------------------------------------------------------------------------

if (require.main === module) {
  const args = process.argv.slice(2);
  const planPath = args[0] || 'data/travel-plan.json';
  const tripId = args.find(a => a.startsWith('--trip-id='))?.split('=')[1];
  const dryRun = args.includes('--dry-run');

  const { bookings, warnings } = extractAllBookings(planPath, tripId);

  if (warnings.length > 0) {
    console.warn('Warnings:');
    for (const w of warnings) console.warn(`  - ${w}`);
  }

  console.log(`\nExtracted ${bookings.length} bookings:`);
  for (const b of bookings) {
    console.log(`  [${b.category}] ${b.status.padEnd(9)} ${b.title}`);
  }

  if (dryRun) {
    console.log('\n(dry-run: no SQL generated)');
  } else {
    console.log('\nSQL statements:');
    for (const b of bookings) {
      console.log(toUpsertSql(b));
    }
  }
}
