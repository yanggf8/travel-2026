/**
 * Fix Osaka-Kyoto plan in Turso DB.
 *
 * Fixes:
 *  1. Flight leg schema: rename from/to -> departure_airport_code/arrival_airport_code,
 *     departure/arrival -> departure_time/arrival_time
 *  2. Provenance schema: rename source -> source_id, add offers_found
 *  3. P5 day schema: add day_number, day_type, convert old schedule format to
 *     morning/afternoon/evening DaySession, fix status from 'draft' to 'planned'
 *  4. Record LionTravel FIT booking: price 23,348/person (46,696 total), order 2026-1311130
 *  5. Set P3_4 to booked, populate P3 transport + P4 accommodation from chosen offer
 *  6. Record Haruka round-trip airport transfer
 *
 * Usage: npx ts-node scripts/fix-osaka-plan-db.ts
 */
import path from 'node:path';

const tursoService = require(path.join(process.cwd(), 'src/services/turso-service'));
const { readPlanFromDb, writePlanToDb } = tursoService;

const PLAN_ID = 'osaka-kyoto-2026';
const DEST_KEY = 'osaka_kyoto_2026';
const NOW = new Date().toISOString();

// ---------------------------------------------------------------------------
// Fix helpers
// ---------------------------------------------------------------------------

/** Fix a single flight leg: rename from/to, departure/arrival */
function fixFlightLeg(leg: Record<string, unknown>): Record<string, unknown> {
  if (!leg) return leg;
  const fixed: Record<string, unknown> = { ...leg };

  // Rename from -> departure_airport_code
  if ('from' in fixed && !('departure_airport_code' in fixed)) {
    fixed.departure_airport_code = fixed.from;
    delete fixed.from;
  }
  // Rename to -> arrival_airport_code
  if ('to' in fixed && !('arrival_airport_code' in fixed)) {
    fixed.arrival_airport_code = fixed.to;
    delete fixed.to;
  }
  // Rename departure -> departure_time
  if ('departure' in fixed && !('departure_time' in fixed)) {
    fixed.departure_time = fixed.departure;
    delete fixed.departure;
  }
  // Rename arrival -> arrival_time
  if ('arrival' in fixed && !('arrival_time' in fixed)) {
    fixed.arrival_time = fixed.arrival;
    delete fixed.arrival;
  }
  // Rename flight_no -> flight_number
  if ('flight_no' in fixed && !('flight_number' in fixed)) {
    fixed.flight_number = fixed.flight_no;
    delete fixed.flight_no;
  }

  return fixed;
}

/** Fix all flight objects in an offer */
function fixOfferFlights(offer: Record<string, unknown>): void {
  const flight = offer.flight as Record<string, unknown> | undefined;
  if (!flight) return;
  if (flight.outbound) {
    flight.outbound = fixFlightLeg(flight.outbound as Record<string, unknown>);
  }
  if (flight.return) {
    flight.return = fixFlightLeg(flight.return as Record<string, unknown>);
  }
}

/** Fix provenance entries: rename source -> source_id, add offers_found */
function fixProvenance(prov: Record<string, unknown>[]): Record<string, unknown>[] {
  return prov.map(p => {
    const fixed: Record<string, unknown> = { ...p };
    if ('source' in fixed && !('source_id' in fixed)) {
      fixed.source_id = fixed.source;
      delete fixed.source;
    }
    if (!('offers_found' in fixed)) {
      fixed.offers_found = 1;
    }
    return fixed;
  });
}

/** Determine day_type based on index and total days */
function getDayType(dayIndex: number, totalDays: number): 'arrival' | 'full' | 'departure' {
  if (dayIndex === 0) return 'arrival';
  if (dayIndex === totalDays - 1) return 'departure';
  return 'full';
}

/** Convert old schedule-based day to new DaySession format */
function convertDay(day: Record<string, unknown>, dayIndex: number, totalDays: number): Record<string, unknown> {
  const dayNumber = (day.day as number) || (dayIndex + 1);
  const dayType = getDayType(dayIndex, totalDays);

  // Parse old schedule entries into morning/afternoon/evening
  const schedule = (day.schedule as Array<Record<string, unknown>>) || [];

  const morningActivities: string[] = [];
  const afternoonActivities: string[] = [];
  const eveningActivities: string[] = [];
  const morningMeals: string[] = [];
  const afternoonMeals: string[] = [];
  const eveningMeals: string[] = [];
  let morningTransit: string | null = null;
  let afternoonTransit: string | null = null;
  let eveningTransit: string | null = null;

  for (const entry of schedule) {
    const timeStr = (entry.time as string) || '';
    const activity = (entry.activity as string) || '';
    const transport = entry.transport as string | undefined;
    const location = entry.location as string | undefined;
    const notes = entry.notes as string | undefined;

    // Parse hour from time string
    const hourMatch = timeStr.match(/^(\d{1,2})/);
    const hour = hourMatch ? parseInt(hourMatch[1], 10) : 12;

    // Build activity description
    let desc = activity;
    if (location) desc += ` @ ${location}`;
    if (notes) desc += ` (${notes})`;

    // Check if this is a meal
    const isMeal = /breakfast|lunch|dinner|食|meal/i.test(activity);

    // Classify by hour
    if (hour < 12) {
      if (isMeal) morningMeals.push(desc);
      else morningActivities.push(desc);
      if (transport && !morningTransit) morningTransit = transport;
    } else if (hour < 17) {
      if (isMeal) afternoonMeals.push(desc);
      else afternoonActivities.push(desc);
      if (transport && !afternoonTransit) afternoonTransit = transport;
    } else {
      if (isMeal) eveningMeals.push(desc);
      else eveningActivities.push(desc);
      if (transport && !eveningTransit) eveningTransit = transport;
    }
  }

  return {
    date: day.date as string,
    day_number: dayNumber,
    day_type: dayType,
    status: 'planned',
    theme: (day.theme as string) || (day.title as string) || null,
    morning: {
      focus: (day.title as string) || null,
      activities: morningActivities,
      meals: morningMeals,
      transit_notes: morningTransit,
      booking_notes: null,
    },
    afternoon: {
      focus: null,
      activities: afternoonActivities,
      meals: afternoonMeals,
      transit_notes: afternoonTransit,
      booking_notes: null,
    },
    evening: {
      focus: null,
      activities: eveningActivities,
      meals: eveningMeals,
      transit_notes: eveningTransit,
      booking_notes: null,
    },
  };
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  console.log('=== Fix Osaka-Kyoto Plan in DB ===\n');

  // 1. Read from DB
  console.log('1. Reading plan from DB...');
  const row = await readPlanFromDb(PLAN_ID);
  if (!row) {
    console.error(`Plan "${PLAN_ID}" not found in DB.`);
    process.exit(1);
  }
  console.log(`   Found. Last updated: ${row.updated_at}`);

  const plan = JSON.parse(row.plan_json);
  const state = row.state_json ? JSON.parse(row.state_json) : null;
  const dest = plan.destinations?.[DEST_KEY];
  if (!dest) {
    console.error(`Destination "${DEST_KEY}" not found in plan.`);
    process.exit(1);
  }

  // 2. Fix flight schemas in all offers
  console.log('\n2. Fixing flight schemas...');
  const p34 = dest.process_3_4_packages;
  const offers = p34?.results?.offers || [];
  let flightsFixed = 0;
  for (const offer of offers) {
    if (offer.flight) {
      const ob = offer.flight.outbound;
      const rt = offer.flight.return;
      const hadFrom = ob && 'from' in ob;
      fixOfferFlights(offer);
      if (hadFrom) flightsFixed++;
    }
  }
  console.log(`   Fixed ${flightsFixed} offer flight objects.`);

  // 3. Fix provenance
  console.log('\n3. Fixing provenance...');
  if (p34?.results?.provenance) {
    const before = p34.results.provenance[0];
    p34.results.provenance = fixProvenance(p34.results.provenance);
    console.log(`   Fixed ${p34.results.provenance.length} provenance entries.`);
    console.log(`   Before: ${JSON.stringify(before)}`);
    console.log(`   After:  ${JSON.stringify(p34.results.provenance[0])}`);
  }

  // 4. Fix P5 days
  console.log('\n4. Fixing P5 itinerary days...');
  const p5 = dest.process_5_daily_itinerary;
  if (p5?.days) {
    const totalDays = p5.days.length;
    const oldDay0 = p5.days[0];
    p5.days = p5.days.map((d: Record<string, unknown>, i: number) => convertDay(d, i, totalDays));
    console.log(`   Converted ${totalDays} days.`);
    console.log(`   Day[0] before keys: ${Object.keys(oldDay0).join(', ')}`);
    console.log(`   Day[0] after keys:  ${Object.keys(p5.days[0]).join(', ')}`);
  }

  // 5. Fix P5 status (draft -> researched; 'planned' is only valid for day-level, not process-level)
  console.log('\n5. Fixing P5 status...');
  if (p5) {
    const oldStatus = p5.status;
    p5.status = 'researched';
    p5.updated_at = NOW;
    console.log(`   P5 status: ${oldStatus} -> ${p5.status}`);
  }

  // 6. Update chosen offer price and record booking
  console.log('\n6. Recording LionTravel FIT booking...');
  const chosenOffer = offers.find((o: Record<string, unknown>) => o.id === 'liontravel_190620015');
  if (chosenOffer) {
    const oldPrice = chosenOffer.price_per_person;
    chosenOffer.price_per_person = 23348;
    chosenOffer.price_total = 46696;
    chosenOffer.last_price_check = NOW;
    console.log(`   Price updated: ${oldPrice} -> ${chosenOffer.price_per_person} TWD/person`);
    console.log(`   Total: ${chosenOffer.price_total} TWD (2 pax)`);
  }

  // Set P3_4 to booked with chosen_offer reference
  p34.status = 'booked';
  p34.updated_at = NOW;
  p34.chosen_offer = {
    id: 'liontravel_190620015',
    selected_date: '2026-02-24',
    selected_at: NOW,
  };

  // Add booking reference to results
  if (!p34.results.chosen_offer) {
    p34.results.chosen_offer = { ...chosenOffer };
  }
  p34.booking_reference = '2026-1311130';
  p34.booked_at = NOW;
  console.log(`   P3_4 status: booked`);
  console.log(`   Booking reference: 2026-1311130`);

  // 7. Populate P3 transportation from chosen offer
  console.log('\n7. Populating P3 transportation...');
  const p3 = dest.process_3_transportation;
  const chosenFlight = chosenOffer?.flight;
  if (p3 && chosenFlight) {
    p3.status = 'booked';
    p3.updated_at = NOW;
    p3.source = 'liontravel';
    p3.populated_from = 'liontravel_190620015';
    p3.flight = {
      airline: chosenFlight.airline || 'Thai Lion Air',
      airline_code: chosenFlight.airline_code || null,
      outbound: { ...chosenFlight.outbound },
      return: { ...chosenFlight.return },
      booked_date: NOW.split('T')[0],
      populated_at: NOW,
    };

    // Record Haruka airport transfer
    p3.airport_transfers = {
      arrival: {
        status: 'booked',
        selected: {
          id: 'haruka_kix_kyoto',
          title: 'JR Haruka Express',
          route: 'KIX -> Kyoto Station',
          duration_min: 75,
          price_yen: 450,
          schedule: 'Every 30 min',
          booking_url: null,
          notes: 'Included in LionTravel FIT package (TWD 450/person/trip)',
          tags: ['train', 'airport_express', 'included'],
        },
        candidates: [],
      },
      departure: {
        status: 'booked',
        selected: {
          id: 'haruka_kyoto_kix',
          title: 'JR Haruka Express',
          route: 'Kyoto Station -> KIX',
          duration_min: 75,
          price_yen: 450,
          schedule: 'Every 30 min',
          booking_url: null,
          notes: 'Included in LionTravel FIT package (TWD 450/person/trip)',
          tags: ['train', 'airport_express', 'included'],
        },
        candidates: [],
      },
    };
    console.log('   P3 status: booked');
    console.log('   Flight: Thai Lion Air TPE<->KIX');
    console.log('   Airport transfer: JR Haruka Express (included in package)');
  }

  // 8. Populate P4 accommodation from chosen offer
  console.log('\n8. Populating P4 accommodation...');
  const p4 = dest.process_4_accommodation;
  const chosenHotel = chosenOffer?.hotel;
  if (p4 && chosenHotel) {
    p4.status = 'booked';
    p4.updated_at = NOW;
    p4.source = 'liontravel';
    p4.populated_from = 'liontravel_190620015';
    p4.hotel = {
      name: chosenHotel.name || 'APA Hotel Kyoto Ekimae',
      slug: 'apa-kyoto-ekimae',
      area: chosenHotel.area || 'Kyoto Station',
      area_type: chosenHotel.area_type || 'central',
      star_rating: null,
      access: ['JR Kyoto Station 3 min walk'],
      check_in: '15:00',
      populated_at: NOW,
    };
    p4.location_zone = {
      status: 'confirmed',
      selected_area: 'Kyoto Station',
      candidates: [],
    };
    console.log('   P4 status: booked');
    console.log(`   Hotel: ${p4.hotel.name}`);
  }

  // 9. Fix destination P1 date anchor to match global
  console.log('\n9. Syncing destination P1 date anchor...');
  if (dest.process_1_date_anchor) {
    dest.process_1_date_anchor.status = 'confirmed';
    dest.process_1_date_anchor.updated_at = NOW;
    dest.process_1_date_anchor.confirmed_dates = {
      start: '2026-02-24',
      end: '2026-02-28',
    };
    console.log('   P1 status: confirmed (Feb 24-28)');
  }

  // 10. Update state_json
  console.log('\n10. Updating state_json...');
  if (state) {
    state.current_focus = `${DEST_KEY}.process_5_daily_itinerary`;
    state.next_actions = [
      'finalize_p5_itinerary_details',
      'add_restaurant_reservations',
      'confirm_kimono_experience_booking',
    ];

    // Update destination process states
    if (state.destinations?.[DEST_KEY]?.processes) {
      const procs = state.destinations[DEST_KEY].processes;
      if (procs.process_1_date_anchor) procs.process_1_date_anchor.state = 'confirmed';
      if (procs.process_3_4_packages) procs.process_3_4_packages.state = 'booked';
      if (procs.process_3_transportation) procs.process_3_transportation.state = 'booked';
      if (procs.process_4_accommodation) procs.process_4_accommodation.state = 'booked';
      if (procs.process_5_daily_itinerary) procs.process_5_daily_itinerary.state = 'researched';
    }

    // Add booking event
    state.event_log.push({
      event: 'package_booked',
      at: NOW,
      destination: DEST_KEY,
      process: 'process_3_4_packages',
      data: {
        offer_id: 'liontravel_190620015',
        booking_reference: '2026-1311130',
        price_per_person: 23348,
        total: 46696,
        currency: 'TWD',
        hotel: 'APA Hotel Kyoto Ekimae',
        airline: 'Thai Lion Air',
      },
    });
    console.log('   State updated with booking event.');
  }

  // 11. Write back to DB
  console.log('\n11. Writing fixed plan to DB...');
  const planJson = JSON.stringify(plan);
  const stateJson = state ? JSON.stringify(state) : null;
  await writePlanToDb(PLAN_ID, planJson, stateJson, plan.schema_version || '4.2.0');
  console.log('   Written successfully.');

  // 12. Verify
  console.log('\n12. Verifying fix...');
  const verifyRow = await readPlanFromDb(PLAN_ID);
  if (!verifyRow) {
    console.error('   VERIFY FAILED: Plan not found after write!');
    process.exit(1);
  }
  const verifyPlan = JSON.parse(verifyRow.plan_json);
  const vDest = verifyPlan.destinations?.[DEST_KEY];

  const checks: Array<{ label: string; ok: boolean; detail: string }> = [];

  // Check flight schema
  const vOffer = vDest?.process_3_4_packages?.results?.offers?.[0];
  const vOb = vOffer?.flight?.outbound;
  checks.push({
    label: 'Flight outbound has departure_airport_code',
    ok: vOb?.departure_airport_code === 'TPE',
    detail: `departure_airport_code=${vOb?.departure_airport_code}`,
  });
  checks.push({
    label: 'Flight outbound has arrival_airport_code',
    ok: vOb?.arrival_airport_code === 'KIX',
    detail: `arrival_airport_code=${vOb?.arrival_airport_code}`,
  });
  checks.push({
    label: 'Flight outbound has departure_time',
    ok: typeof vOb?.departure_time === 'string',
    detail: `departure_time=${vOb?.departure_time}`,
  });

  // Check provenance
  const vProv = vDest?.process_3_4_packages?.results?.provenance?.[0];
  checks.push({
    label: 'Provenance has source_id',
    ok: typeof vProv?.source_id === 'string',
    detail: `source_id=${vProv?.source_id}`,
  });
  checks.push({
    label: 'Provenance has offers_found',
    ok: typeof vProv?.offers_found === 'number',
    detail: `offers_found=${vProv?.offers_found}`,
  });

  // Check P5 days
  const vDay0 = vDest?.process_5_daily_itinerary?.days?.[0];
  checks.push({
    label: 'Day[0] has day_number',
    ok: vDay0?.day_number === 1,
    detail: `day_number=${vDay0?.day_number}`,
  });
  checks.push({
    label: 'Day[0] has day_type',
    ok: vDay0?.day_type === 'arrival',
    detail: `day_type=${vDay0?.day_type}`,
  });
  checks.push({
    label: 'Day[0] has morning session',
    ok: typeof vDay0?.morning === 'object' && vDay0?.morning !== null,
    detail: `morning=${typeof vDay0?.morning}`,
  });
  checks.push({
    label: 'P5 status is researched',
    ok: vDest?.process_5_daily_itinerary?.status === 'researched',
    detail: `status=${vDest?.process_5_daily_itinerary?.status}`,
  });

  // Check booking
  checks.push({
    label: 'P3_4 status is booked',
    ok: vDest?.process_3_4_packages?.status === 'booked',
    detail: `status=${vDest?.process_3_4_packages?.status}`,
  });
  checks.push({
    label: 'Booking reference is 2026-1311130',
    ok: vDest?.process_3_4_packages?.booking_reference === '2026-1311130',
    detail: `ref=${vDest?.process_3_4_packages?.booking_reference}`,
  });
  checks.push({
    label: 'Price updated to 23348',
    ok: vOffer?.price_per_person === 23348,
    detail: `price=${vOffer?.price_per_person}`,
  });

  // Check P3 transport
  checks.push({
    label: 'P3 status is booked',
    ok: vDest?.process_3_transportation?.status === 'booked',
    detail: `status=${vDest?.process_3_transportation?.status}`,
  });
  checks.push({
    label: 'P3 has flight data',
    ok: typeof vDest?.process_3_transportation?.flight === 'object',
    detail: `flight=${typeof vDest?.process_3_transportation?.flight}`,
  });
  checks.push({
    label: 'P3 has airport_transfers',
    ok: typeof vDest?.process_3_transportation?.airport_transfers === 'object',
    detail: `transfers=${typeof vDest?.process_3_transportation?.airport_transfers}`,
  });

  // Check P4 accommodation
  checks.push({
    label: 'P4 status is booked',
    ok: vDest?.process_4_accommodation?.status === 'booked',
    detail: `status=${vDest?.process_4_accommodation?.status}`,
  });
  checks.push({
    label: 'P4 has hotel data',
    ok: vDest?.process_4_accommodation?.hotel?.name?.includes('APA'),
    detail: `hotel=${vDest?.process_4_accommodation?.hotel?.name}`,
  });

  // Print results
  console.log('');
  let passed = 0;
  let failed = 0;
  for (const c of checks) {
    const icon = c.ok ? 'PASS' : 'FAIL';
    console.log(`   [${icon}] ${c.label} (${c.detail})`);
    if (c.ok) passed++;
    else failed++;
  }

  console.log(`\n=== Results: ${passed}/${checks.length} passed, ${failed} failed ===`);

  if (failed > 0) {
    process.exit(1);
  }

  console.log('\nDone! Plan fixed and verified in DB.');
}

main().catch((err) => {
  console.error('Script failed:', err);
  process.exit(1);
});
