export interface Env {
  TURSO_URL: string;
  TURSO_TOKEN: string;
  GOOGLE_MAPS_KEY?: string;
  ADMIN_TOKEN?: string;
}

interface TursoCell {
  type: string;
  value?: string;
}

interface TursoResult {
  cols: Array<{ name: string; decltype?: string }>;
  rows: TursoCell[][];
}

interface TursoPipelineResponse {
  results: Array<{
    response: {
      type: string;
      result?: TursoResult;
      error?: unknown;
    };
  }>;
}

type Row = Record<string, string | null>;

function rowsToObjects(result: TursoResult): Row[] {
  const colNames = result.cols.map((c) => c.name);
  return result.rows.map((row) => {
    const obj: Row = {};
    colNames.forEach((name, i) => {
      obj[name] = row[i]?.value ?? null;
    });
    return obj;
  });
}

/**
 * Multi-query pipeline: sends N SQL queries in a single HTTP request.
 * Returns an array of result arrays, one per query.
 */
async function queryTursoPipeline(
  env: Env,
  sqls: string[]
): Promise<Row[][]> {
  if (!env.TURSO_URL || !env.TURSO_TOKEN) {
    throw new Error('TURSO_URL and TURSO_TOKEN secrets are not configured.');
  }
  const pipelineUrl = env.TURSO_URL.replace('libsql://', 'https://') + '/v2/pipeline';

  const body = {
    requests: [
      ...sqls.map((sql) => ({ type: 'execute', stmt: { sql } })),
      { type: 'close' },
    ],
  };

  const res = await fetch(pipelineUrl, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${env.TURSO_TOKEN}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });

  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(`Turso HTTP ${res.status}: ${text || res.statusText}`);
  }

  const json = (await res.json()) as TursoPipelineResponse;

  return sqls.map((_sql, i) => {
    const entry = json.results?.[i];
    if (entry?.response?.error) {
      throw new Error(`Turso pipeline query ${i} error: ${JSON.stringify(entry.response.error)}`);
    }
    const result = entry?.response?.result;
    if (!result) return [];
    return rowsToObjects(result);
  });
}

export interface PlanData {
  plan: Record<string, unknown>;
  updated_at: string;
}

function toDestSlug(s: string): string {
  return s.replace(/-/g, '_');
}

function toPlanId(s: string): string {
  return s.replace(/_/g, '-');
}

function tryParseJson(s: string | null): unknown {
  if (!s) return null;
  try { return JSON.parse(s); } catch { return null; }
}

export async function getPlan(env: Env, planId: string): Promise<PlanData | null> {
  return getDashboardPlan(env, planId);
}

// ============================================================================
// getDashboardPlan — 100% normalized tables, zero blob
// ============================================================================

export async function getDashboardPlan(env: Env, planId: string): Promise<PlanData | null> {
  const escaped = planId.replace(/'/g, "''");
  const destSlug = toDestSlug(escaped);

  const results = await queryTursoPipeline(env, [
    /*  0 */ `SELECT * FROM plan_metadata WHERE plan_id = '${escaped}'`,
    /*  1 */ `SELECT * FROM plan_destinations WHERE plan_id = '${escaped}'`,
    /*  2 */ `SELECT * FROM date_anchors WHERE plan_id = '${escaped}'`,
    /*  3 */ `SELECT * FROM process_statuses WHERE plan_id = '${escaped}'`,
    /*  4 */ `SELECT * FROM plan_offers WHERE plan_id = '${escaped}'`,
    /*  5 */ `SELECT * FROM plan_offer_date_pricing WHERE plan_id = '${escaped}'`,
    /*  6 */ `SELECT * FROM plan_offer_selection WHERE plan_id = '${escaped}'`,
    /*  7 */ `SELECT * FROM flight_legs WHERE plan_id = '${escaped}' ORDER BY destination, direction, leg_order`,
    /*  8 */ `SELECT * FROM hotels WHERE plan_id = '${escaped}'`,
    /*  9 */ `SELECT * FROM hotel_access_lines WHERE plan_id = '${escaped}' ORDER BY destination, sort_order`,
    /* 10 */ `SELECT * FROM airport_transfers WHERE plan_id = '${escaped}'`,
    /* 11 */ `SELECT * FROM days WHERE plan_id = '${escaped}' ORDER BY destination, day_number`,
    /* 12 */ `SELECT * FROM timesofday WHERE plan_id = '${escaped}' ORDER BY destination, day_number, session_type`,
    /* 13 */ `SELECT * FROM activities WHERE plan_id = '${escaped}' ORDER BY destination, day_number, session_type, sort_order`,
    /* 14 */ `SELECT * FROM itinerary_metadata WHERE plan_id = '${escaped}'`,
    /* 15 */ `SELECT * FROM day_route_segments WHERE plan_id = '${escaped}' ORDER BY destination, day_number, sort_order`,
    /* 16 */ `SELECT * FROM day_landmarks WHERE plan_id = '${escaped}' ORDER BY destination, day_number, sort_order`,
    /* 17 */ `SELECT slug, currency FROM destination_config`,
    /* 18 */ `SELECT * FROM session_meals WHERE plan_id = '${escaped}' ORDER BY destination, day_number, session_type, sort_order`,
    /* 19 */ `SELECT * FROM session_activities_zh WHERE plan_id = '${escaped}' ORDER BY destination, day_number, session_type, sort_order`,
    /* 20 */ `SELECT at.activity_id, at.tag FROM activity_tags at INNER JOIN activities a ON at.activity_id = a.id WHERE a.plan_id = '${escaped}'`,
  ]);

  const [
    metaRows, destRows, dateRows, statusRows,
    offerRows, offerDatePricingRows, offerSelRows,
    flightLegRows, hotelRows, accessLineRows, transferRows,
    dayRows, sessionRows, activityRows, itinMetaRows,
    routeSegRows, landmarkRows, destConfigRows,
    mealRows, activitiesZhRows, tagRows,
  ] = results;

  if (metaRows.length === 0) return null;

  const meta = metaRows[0];
  const activeDest = meta.active_destination || destSlug;

  // --- Index rows by destination ---

  const destDisplayNames = new Map<string, string>();
  for (const r of destRows) destDisplayNames.set(r.slug!, r.display_name || r.slug!);

  const currencyBySlug = new Map<string, string>();
  for (const r of destConfigRows) if (r.slug && r.currency) currencyBySlug.set(r.slug, r.currency);

  const dateAnchorMap = new Map<string, Row>();
  for (const r of dateRows) dateAnchorMap.set(r.destination!, r);

  // Process statuses indexed by dest:process_id
  const statusMap = new Map<string, Row>();
  for (const r of statusRows) statusMap.set(`${r.destination}:${r.process_id}`, r);

  // Offers indexed by dest
  const offersByDest = new Map<string, Row[]>();
  for (const r of offerRows) {
    const d = r.destination!;
    if (!offersByDest.has(d)) offersByDest.set(d, []);
    offersByDest.get(d)!.push(r);
  }

  // Date pricing indexed by offer_id
  const datePricingByOffer = new Map<string, Row[]>();
  for (const r of offerDatePricingRows) {
    const oid = r.offer_id!;
    if (!datePricingByOffer.has(oid)) datePricingByOffer.set(oid, []);
    datePricingByOffer.get(oid)!.push(r);
  }

  // Offer selection by dest
  const offerSelByDest = new Map<string, Row>();
  for (const r of offerSelRows) offerSelByDest.set(r.destination!, r);

  // Flight legs by dest:direction
  const flightLegMap = new Map<string, Row[]>();
  const flightDests = new Set<string>();
  for (const r of flightLegRows) {
    const key = `${r.destination!}:${r.direction!}`;
    if (!flightLegMap.has(key)) flightLegMap.set(key, []);
    flightLegMap.get(key)!.push(r);
    flightDests.add(r.destination!);
  }

  // Hotels by dest
  const hotelMap = new Map<string, Row>();
  for (const r of hotelRows) hotelMap.set(r.destination!, r);

  // Access lines by dest
  const accessByDest = new Map<string, string[]>();
  for (const r of accessLineRows) {
    const d = r.destination!;
    if (!accessByDest.has(d)) accessByDest.set(d, []);
    accessByDest.get(d)!.push(r.line!);
  }

  // Transfers by dest
  const transferByDest = new Map<string, Row[]>();
  for (const r of transferRows) {
    const d = r.destination!;
    if (!transferByDest.has(d)) transferByDest.set(d, []);
    transferByDest.get(d)!.push(r);
  }

  // Days by dest
  const daysByDest = new Map<string, Row[]>();
  for (const r of dayRows) {
    const d = r.destination!;
    if (!daysByDest.has(d)) daysByDest.set(d, []);
    daysByDest.get(d)!.push(r);
  }

  // Sessions by dest:day:session
  const sKey = (dest: string, day: string, session: string) => `${dest}:${day}:${session}`;
  const sessionMap = new Map<string, Row>();
  for (const r of sessionRows) sessionMap.set(sKey(r.destination!, r.day_number!, r.session_type!), r);

  // Activities by dest:day:session
  const activityMap = new Map<string, Row[]>();
  for (const r of activityRows) {
    const key = sKey(r.destination!, r.day_number!, r.session_type!);
    if (!activityMap.has(key)) activityMap.set(key, []);
    activityMap.get(key)!.push(r);
  }

  // Normalized list child rows (no JSON columns).
  const mealMap = new Map<string, string[]>();
  for (const r of mealRows) {
    const key = sKey(r.destination!, r.day_number!, r.session_type!);
    if (!mealMap.has(key)) mealMap.set(key, []);
    mealMap.get(key)!.push(r.meal!);
  }
  const activitiesZhMap = new Map<string, string[]>();
  for (const r of activitiesZhRows) {
    const key = sKey(r.destination!, r.day_number!, r.session_type!);
    if (!activitiesZhMap.has(key)) activitiesZhMap.set(key, []);
    activitiesZhMap.get(key)!.push(r.activity!);
  }
  const tagMap = new Map<string, string[]>();
  for (const r of tagRows) {
    if (!tagMap.has(r.activity_id!)) tagMap.set(r.activity_id!, []);
    tagMap.get(r.activity_id!)!.push(r.tag!);
  }

  // Itinerary metadata by dest
  const itinMetaMap = new Map<string, Row>();
  for (const r of itinMetaRows) itinMetaMap.set(r.destination!, r);

  // Route segments by dest:day
  const routesByDestDay = new Map<string, Row[]>();
  for (const r of routeSegRows) {
    const key = `${r.destination!}:${r.day_number!}`;
    if (!routesByDestDay.has(key)) routesByDestDay.set(key, []);
    routesByDestDay.get(key)!.push(r);
  }

  // Landmarks by dest:day
  const landmarksByDestDay = new Map<string, string[]>();
  for (const r of landmarkRows) {
    const key = `${r.destination!}:${r.day_number!}`;
    if (!landmarksByDestDay.has(key)) landmarksByDestDay.set(key, []);
    landmarksByDestDay.get(key)!.push(r.landmark!);
  }

  // --- Collect all destination slugs ---
  const allDests = new Set<string>();
  for (const r of destRows) allDests.add(r.slug!);
  // Ensure active dest is included even if plan_destinations is sparse
  allDests.add(activeDest);

  // --- Assemble plan ---
  const destinations: Record<string, unknown> = {};

  for (const dest of allDests) {
    const dateAnchor = dateAnchorMap.get(dest);
    const p34Status = statusMap.get(`${dest}:process_3_4_packages`)?.status;
    const p4Status = statusMap.get(`${dest}:process_4_accommodation`)?.status;

    // Offers
    const offers = (offersByDest.get(dest) || []).map((o) => {
      const dp: Record<string, { price: number; availability?: string; seats_remaining?: number }> = {};
      for (const pr of datePricingByOffer.get(o.id!) || []) {
        dp[pr.date!] = {
          price: pr.price ? parseInt(pr.price, 10) : 0,
          availability: pr.availability || undefined,
          seats_remaining: pr.seats_remaining ? parseInt(pr.seats_remaining, 10) : undefined,
        };
      }
      return {
        id: o.id,
        source_id: o.source_id,
        product_code: o.product_code,
        price_per_person: o.price_per_person ? parseInt(o.price_per_person, 10) : null,
        date_pricing: Object.keys(dp).length > 0 ? dp : undefined,
      };
    });

    const sel = offerSelByDest.get(dest);
    const chosenOffer = sel ? { id: sel.selected_offer_id, selected_date: sel.selected_date } : undefined;

    // Flight
    const buildLeg = (rows: Row[] | undefined) => {
      if (!rows || rows.length === 0) return null;
      const r = rows[0];
      return {
        flight_number: r.flight_number,
        departure_airport_code: r.departure_code,
        departure_terminal: r.departure_terminal,
        departure_time: r.departure_time,
        arrival_airport_code: r.arrival_code,
        arrival_terminal: r.arrival_terminal,
        arrival_time: r.arrival_time,
        date: r.flight_date,
      };
    };
    const outboundLegs = flightLegMap.get(`${dest}:outbound`);
    const returnLegs = flightLegMap.get(`${dest}:return`);
    const firstLeg = outboundLegs?.[0] ?? returnLegs?.[0];

    // Hotel
    const hotelRow = hotelMap.get(dest);
    const access = accessByDest.get(dest) || (hotelRow?.access_json ? tryParseJson(hotelRow.access_json) as string[] : null);

    // Airport transfers
    const transfers = transferByDest.get(dest) || [];
    const airportTransfers: Record<string, unknown> = {};
    for (const tr of transfers) {
      airportTransfers[tr.direction!] = {
        status: tr.status || 'planned',
        selected: tryParseJson(tr.selected_json),
        candidates: tryParseJson(tr.candidates_json) || [],
      };
    }

    // Itinerary days
    const days = daysByDest.get(dest) || [];
    const reconstructedDays: Record<string, unknown>[] = [];
    for (const dayRow of days) {
      const dayNumber = parseInt(dayRow.day_number!, 10);
      const day: Record<string, unknown> = {
        day_number: dayNumber,
        date: dayRow.date,
        theme: dayRow.theme,
        theme_zh: dayRow.theme_zh || null,
        day_type: dayRow.day_type,
        status: dayRow.status || 'draft',
      };

      const hasWeather = dayRow.weather_label !== null || dayRow.temp_low_c !== null
        || dayRow.temp_high_c !== null || dayRow.precipitation_pct !== null || dayRow.weather_code !== null;
      if (hasWeather) {
        day.weather = {
          weather_label: dayRow.weather_label,
          temp_low_c: dayRow.temp_low_c !== null ? parseFloat(dayRow.temp_low_c!) : undefined,
          temp_high_c: dayRow.temp_high_c !== null ? parseFloat(dayRow.temp_high_c!) : undefined,
          feels_like_low_c: dayRow.feels_like_low_c != null ? parseFloat(dayRow.feels_like_low_c!) : undefined,
          feels_like_high_c: dayRow.feels_like_high_c != null ? parseFloat(dayRow.feels_like_high_c!) : undefined,
          precipitation_pct: dayRow.precipitation_pct !== null ? parseFloat(dayRow.precipitation_pct!) : undefined,
          weather_code: dayRow.weather_code !== null ? parseInt(dayRow.weather_code!, 10) : undefined,
          source_id: dayRow.weather_source_id,
          sourced_at: dayRow.weather_sourced_at,
        };
      }

      for (const sessionType of ['morning', 'noon', 'afternoon', 'evening']) {
        const key = sKey(dest, String(dayNumber), sessionType);
        const sRow = sessionMap.get(key);

        const session: Record<string, unknown> = {
          focus: sRow?.focus || null,
          transit_notes: sRow?.transit_notes || null,
          booking_notes: sRow?.booking_notes || null,
          activities: [],
          focus_zh: sRow?.focus_zh || null,
          transit_notes_zh: sRow?.transit_notes_zh || null,
          meals_zh: null, // meals_zh_json removed (Batch C de-JSON)
          activities_zh: activitiesZhMap.has(key) ? activitiesZhMap.get(key)! : null,
        };

        const sessionMeals = mealMap.get(key);
        if (sessionMeals && sessionMeals.length > 0) {
          session.meals = sessionMeals;
        }
        if (sRow?.time_range_start || sRow?.time_range_end) {
          session.time_range = { start: sRow?.time_range_start, end: sRow?.time_range_end };
        }

        const acts = activityMap.get(key) || [];
        session.activities = acts.map((a) => ({
          id: a.id,
          title: a.title,
          area: a.area || '',
          nearest_station: a.nearest_station || null,
          duration_min: a.duration_min ? parseInt(a.duration_min, 10) : null,
          booking_required: a.booking_required === '1',
          booking_url: a.booking_url || null,
          booking_status: a.booking_status || undefined,
          booking_ref: a.booking_ref || undefined,
          book_by: a.book_by || undefined,
          start_time: a.start_time || undefined,
          end_time: a.end_time || undefined,
          is_fixed_time: a.is_fixed_time === '1',
          cost_estimate: a.cost_estimate ? parseInt(a.cost_estimate, 10) : null,
          tags: tagMap.get(a.id!) || [],
          notes: a.notes || null,
          priority: a.priority || 'want',
        }));

        day[sessionType] = session;
      }

      // Attach route segments and landmarks to day
      const dayKey = `${dest}:${dayNumber}`;
      const dayRoutes = routesByDestDay.get(dayKey);
      if (dayRoutes && dayRoutes.length > 0) {
        day.route_segments = dayRoutes.map(r => ({ from: r.from_place, to: r.to_place, mode: r.mode, duration_min: r.duration_min ? parseInt(r.duration_min, 10) : null, notes: r.notes || null, start_time: r.start_time || null }));
      }
      const dayLandmarks = landmarksByDestDay.get(dayKey);
      if (dayLandmarks && dayLandmarks.length > 0) {
        day.landmarks = dayLandmarks;
      }

      reconstructedDays.push(day);
    }

    // Transit summary from itinerary_metadata
    const itinMeta = itinMetaMap.get(dest);
    const transitSummary = itinMeta?.transit_summary ? tryParseJson(itinMeta.transit_summary) : undefined;
    const transitSummaryZh = itinMeta?.transit_summary_zh ? tryParseJson(itinMeta.transit_summary_zh) : undefined;

    // Find home_address from destRows
    const destRow = destRows.find(r => r.slug === dest);

    destinations[dest] = {
      display_name: destDisplayNames.get(dest) || dest.replace(/_/g, ' '),
      currency: currencyBySlug.get(dest) || null,
      home_address: destRow?.home_address || null,
      process_1_date_anchor: dateAnchor ? {
        confirmed_dates: { start: dateAnchor.start_date, end: dateAnchor.end_date },
        days: dateAnchor.days ? parseInt(dateAnchor.days, 10) : null,
      } : undefined,
      process_3_4_packages: {
        status: p34Status || undefined,
        chosen_offer: chosenOffer,
        results: { offers },
      },
      process_3_transportation: {
        flight: flightDests.has(dest) ? {
          airline: firstLeg?.airline ?? null,
          airline_code: firstLeg?.airline_code ?? null,
          outbound: buildLeg(outboundLegs),
          return: buildLeg(returnLegs),
          booked_date: firstLeg?.booked_date ?? null,
        } : undefined,
        airport_transfers: Object.keys(airportTransfers).length > 0 ? airportTransfers : undefined,
      },
      process_4_accommodation: {
        status: p4Status || undefined,
        hotel: hotelRow ? {
          name: hotelRow.name,
          name_zh: hotelRow.name_zh || null,
          access: access,
          check_in: hotelRow.check_in,
          notes: hotelRow.notes,
        } : undefined,
      },
      process_5_daily_itinerary: {
        days: reconstructedDays,
        transit_summary: transitSummary,
        transit_summary_zh: transitSummaryZh,
      },
    };
  }

  const plan = {
    active_destination: activeDest,
    schema_version: meta.schema_version || '4.2.0',
    destinations,
  };

  return {
    plan: plan as Record<string, unknown>,
    updated_at: meta.updated_at || new Date().toISOString(),
  };
}

// ============================================================================
// listPlans — from plan_metadata + plan_destinations (no blob)
// ============================================================================

export interface PlanSummary {
  slug: string;
  display_name: string;
  updated_at: string;
  start_date: string | null;
  end_date: string | null;
  days: number | null;
}

export async function listPlans(env: Env): Promise<PlanSummary[]> {
  const results = await queryTursoPipeline(env, [
    `SELECT m.plan_id, m.active_destination, d.display_name, m.updated_at,
            da.start_date, da.end_date, da.days
     FROM plan_metadata m
     LEFT JOIN plan_destinations d ON m.plan_id = d.plan_id AND m.active_destination = d.slug
     LEFT JOIN date_anchors da ON m.plan_id = da.plan_id AND m.active_destination = da.destination
     ORDER BY da.start_date ASC, m.updated_at DESC`,
  ]);
  const rows = results[0];
  return rows.map((r) => {
    const dest = r.active_destination || r.plan_id!;
    return {
      slug: toPlanId(dest),
      display_name: r.display_name || dest.replace(/_/g, ' '),
      updated_at: r.updated_at!,
      start_date: r.start_date || null,
      end_date: r.end_date || null,
      days: r.days ? parseInt(r.days, 10) : null,
    };
  });
}

// ============================================================================
// Bookings
// ============================================================================

export interface BookingRow {
  booking_key: string;
  trip_id: string;
  destination: string;
  category: string;
  title: string;
  status: string;
  reference: string | null;
  book_by: string | null;
  price_amount: string | null;
  price_currency: string | null;
  payload_json: string | null;
}

export async function getBookings(
  env: Env,
  destination: string
): Promise<BookingRow[]> {
  const escaped = destination.replace(/'/g, "''");
  const results = await queryTursoPipeline(env, [
    `SELECT booking_key, trip_id, destination, category, title, status, reference, book_by, price_amount, price_currency, payload_json FROM bookings_current WHERE destination = '${escaped}'`,
  ]);
  return results[0] as unknown as BookingRow[];
}

// ============================================================================
// Write functions for inline editor
// ============================================================================

const DAY_EDITABLE_FIELDS = new Set(['theme', 'theme_zh']);
// Scalar columns updated directly on timesofday.
const SESSION_SCALAR_FIELDS = new Set([
  'focus', 'focus_zh',
  'transit_notes_zh', 'transit_notes',
]);
// List fields backed by child tables (no JSON columns): client sends a JSON
// array string; we replace the child rows.
const SESSION_LIST_FIELDS: Record<string, { table: string; col: string }> = {
  activities_zh: { table: 'session_activities_zh', col: 'activity' },
  meals: { table: 'session_meals', col: 'meal' },
};

interface TursoArg {
  type: string;
  value: string;
}

async function executeTursoWrite(
  env: Env,
  sql: string,
  args: TursoArg[]
): Promise<void> {
  if (!env.TURSO_URL || !env.TURSO_TOKEN) {
    throw new Error('TURSO_URL and TURSO_TOKEN secrets are not configured.');
  }
  const pipelineUrl = env.TURSO_URL.replace('libsql://', 'https://') + '/v2/pipeline';

  const body = {
    requests: [
      { type: 'execute', stmt: { sql, args } },
      { type: 'close' },
    ],
  };

  const res = await fetch(pipelineUrl, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${env.TURSO_TOKEN}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });

  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(`Turso write HTTP ${res.status}: ${text || res.statusText}`);
  }

  const json = (await res.json()) as TursoPipelineResponse;
  const entry = json.results?.[0];
  if (entry?.response?.error) {
    throw new Error(`Turso write error: ${JSON.stringify(entry.response.error)}`);
  }
}

// Execute several statements in one pipeline request (used for child-row replace).
async function executeTursoWriteBatch(
  env: Env,
  statements: Array<{ sql: string; args: TursoArg[] }>
): Promise<void> {
  if (!env.TURSO_URL || !env.TURSO_TOKEN) {
    throw new Error('TURSO_URL and TURSO_TOKEN secrets are not configured.');
  }
  const pipelineUrl = env.TURSO_URL.replace('libsql://', 'https://') + '/v2/pipeline';
  const body = {
    requests: [
      ...statements.map((s) => ({ type: 'execute', stmt: { sql: s.sql, args: s.args } })),
      { type: 'close' },
    ],
  };
  const res = await fetch(pipelineUrl, {
    method: 'POST',
    headers: { Authorization: `Bearer ${env.TURSO_TOKEN}`, 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(`Turso write HTTP ${res.status}: ${text || res.statusText}`);
  }
  const json = (await res.json()) as TursoPipelineResponse;
  const failed = json.results?.find((r) => r?.response?.error);
  if (failed?.response?.error) {
    throw new Error(`Turso write error: ${JSON.stringify(failed.response.error)}`);
  }
}

export async function updateDayField(
  env: Env,
  planId: string,
  dest: string,
  dayNumber: number,
  field: string,
  value: string
): Promise<void> {
  if (!DAY_EDITABLE_FIELDS.has(field)) {
    throw new Error(`Field "${field}" is not editable on days`);
  }
  const sql = `UPDATE days SET ${field} = ?1 WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4`;
  await executeTursoWrite(env, sql, [
    { type: 'text', value },
    { type: 'text', value: planId },
    { type: 'text', value: dest },
    { type: 'integer', value: String(dayNumber) },
  ]);
}

export async function updateSessionField(
  env: Env,
  planId: string,
  dest: string,
  dayNumber: number,
  sessionType: string,
  field: string,
  value: string
): Promise<void> {
  if (!['morning', 'noon', 'afternoon', 'evening'].includes(sessionType)) {
    throw new Error(`Invalid session type "${sessionType}"`);
  }

  // List fields → replace child rows (client sends a JSON array string).
  const listField = SESSION_LIST_FIELDS[field];
  if (listField) {
    let items: string[];
    try {
      const parsed = JSON.parse(value);
      items = Array.isArray(parsed) ? parsed.map((x) => String(x)) : [];
    } catch {
      throw new Error(`Field "${field}" expects a JSON array value`);
    }
    const keyArgs: TursoArg[] = [
      { type: 'text', value: planId },
      { type: 'text', value: dest },
      { type: 'integer', value: String(dayNumber) },
      { type: 'text', value: sessionType },
    ];
    const statements: Array<{ sql: string; args: TursoArg[] }> = [
      {
        sql: `DELETE FROM ${listField.table} WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4`,
        args: keyArgs,
      },
      ...items.map((item, i) => ({
        sql: `INSERT INTO ${listField.table} (plan_id, destination, day_number, session_type, sort_order, ${listField.col}) VALUES (?1, ?2, ?3, ?4, ?5, ?6)`,
        args: [...keyArgs, { type: 'integer', value: String(i) }, { type: 'text', value: item }] as TursoArg[],
      })),
    ];
    await executeTursoWriteBatch(env, statements);
    return;
  }

  if (!SESSION_SCALAR_FIELDS.has(field)) {
    throw new Error(`Field "${field}" is not editable on timesofday`);
  }
  const sql = `UPDATE timesofday SET ${field} = ?1 WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4 AND session_type = ?5`;
  await executeTursoWrite(env, sql, [
    { type: 'text', value },
    { type: 'text', value: planId },
    { type: 'text', value: dest },
    { type: 'integer', value: String(dayNumber) },
    { type: 'text', value: sessionType },
  ]);
}
