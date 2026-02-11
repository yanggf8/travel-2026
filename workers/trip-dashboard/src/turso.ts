export interface Env {
  TURSO_URL: string;
  TURSO_TOKEN: string;
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

function rowsToObjects(result: TursoResult): Record<string, string | null>[] {
  const colNames = result.cols.map((c) => c.name);
  return result.rows.map((row) => {
    const obj: Record<string, string | null> = {};
    colNames.forEach((name, i) => {
      obj[name] = row[i]?.value ?? null;
    });
    return obj;
  });
}

async function queryTurso(
  env: Env,
  sql: string
): Promise<Record<string, string | null>[]> {
  if (!env.TURSO_URL || !env.TURSO_TOKEN) {
    throw new Error('TURSO_URL and TURSO_TOKEN secrets are not configured. Run: wrangler secret put TURSO_URL && wrangler secret put TURSO_TOKEN');
  }
  const pipelineUrl = env.TURSO_URL.replace('libsql://', 'https://') + '/v2/pipeline';

  const body = {
    requests: [
      { type: 'execute', stmt: { sql } },
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
  const first = json.results?.[0];

  if (first?.response?.error) {
    throw new Error(`Turso query error: ${JSON.stringify(first.response.error)}`);
  }

  const result = first?.response?.result;
  if (!result) return [];

  return rowsToObjects(result);
}

/**
 * Multi-query pipeline: sends N SQL queries in a single HTTP request.
 * Returns an array of result arrays, one per query.
 */
async function queryTursoPipeline(
  env: Env,
  sqls: string[]
): Promise<Record<string, string | null>[][]> {
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
  plan_json: string;
  state_json: string | null;
  updated_at: string;
}

function toDestSlug(s: string): string {
  return s.replace(/-/g, '_');
}

export async function getPlan(env: Env, planId: string): Promise<PlanData | null> {
  const escaped = planId.replace(/'/g, "''");
  const destSlug = toDestSlug(escaped);
  const rows = await queryTurso(
    env,
    `SELECT plan_json, state_json, updated_at FROM plans
     WHERE plan_id = '${escaped}'
        OR json_extract(plan_json, '$.active_destination') = '${destSlug}'
     ORDER BY (plan_id = '${escaped}') DESC
     LIMIT 1`
  );

  if (rows.length === 0) return null;
  return {
    plan_json: rows[0].plan_json!,
    state_json: rows[0].state_json ?? null,
    updated_at: rows[0].updated_at!,
  };
}

/**
 * Dashboard data from normalized tables.
 *
 * Queries 8 tables in a single pipeline request, then overlays
 * itinerary data from tables onto the blob-loaded plan.
 * Render.ts receives the same PlanData shape — zero render changes.
 */
export async function getDashboardPlan(env: Env, planId: string): Promise<PlanData | null> {
  const escaped = planId.replace(/'/g, "''");
  const destSlug = toDestSlug(escaped);

  // Single pipeline: blob + 7 normalized table queries
  const results = await queryTursoPipeline(env, [
    // 0: blob (for non-normalized data: packages, transit_summary, display_name)
    `SELECT plan_json, state_json, updated_at FROM plans
     WHERE plan_id = '${escaped}'
        OR json_extract(plan_json, '$.active_destination') = '${destSlug}'
     ORDER BY (plan_id = '${escaped}') DESC
     LIMIT 1`,
    // 1: itinerary_days
    `SELECT * FROM itinerary_days WHERE plan_id = '${escaped}' ORDER BY destination, day_number`,
    // 2: itinerary_sessions
    `SELECT * FROM itinerary_sessions WHERE plan_id = '${escaped}' ORDER BY destination, day_number, session_type`,
    // 3: activities
    `SELECT * FROM activities WHERE plan_id = '${escaped}' ORDER BY destination, day_number, session_type, sort_order`,
    // 4: flights
    `SELECT * FROM flights WHERE plan_id = '${escaped}'`,
    // 5: hotels
    `SELECT * FROM hotels WHERE plan_id = '${escaped}'`,
    // 6: airport_transfers
    `SELECT * FROM airport_transfers WHERE plan_id = '${escaped}'`,
    // 7: process_statuses
    `SELECT * FROM process_statuses WHERE plan_id = '${escaped}'`,
  ]);

  const [blobRows, dayRows, sessionRows, activityRows, flightRows, hotelRows, transferRows, _statusRows] = results;

  if (blobRows.length === 0) return null;

  const planJson = blobRows[0].plan_json;
  if (!planJson) return null;

  // If no normalized data, return blob as-is
  if (dayRows.length === 0) {
    return {
      plan_json: planJson,
      state_json: blobRows[0].state_json ?? null,
      updated_at: blobRows[0].updated_at!,
    };
  }

  // Parse blob and overlay normalized itinerary
  const plan = JSON.parse(planJson);

  // Index sessions and activities
  const sKey = (dest: string, day: string, session: string) => `${dest}:${day}:${session}`;
  const sessionMap = new Map<string, Record<string, string | null>>();
  for (const row of sessionRows) {
    sessionMap.set(sKey(row.destination!, row.day_number!, row.session_type!), row);
  }

  const activityMap = new Map<string, Record<string, string | null>[]>();
  for (const row of activityRows) {
    const key = sKey(row.destination!, row.day_number!, row.session_type!);
    if (!activityMap.has(key)) activityMap.set(key, []);
    activityMap.get(key)!.push(row);
  }

  // Index flights, hotels, transfers by destination
  const flightMap = new Map<string, Record<string, string | null>>();
  for (const row of flightRows) flightMap.set(row.destination!, row);

  const hotelMap = new Map<string, Record<string, string | null>>();
  for (const row of hotelRows) hotelMap.set(row.destination!, row);

  const transferMap = new Map<string, Record<string, string | null>[]>();
  for (const row of transferRows) {
    const dest = row.destination!;
    if (!transferMap.has(dest)) transferMap.set(dest, []);
    transferMap.get(dest)!.push(row);
  }

  // Group days by destination
  const destDays = new Map<string, Record<string, string | null>[]>();
  for (const row of dayRows) {
    const dest = row.destination!;
    if (!destDays.has(dest)) destDays.set(dest, []);
    destDays.get(dest)!.push(row);
  }

  // Overlay normalized data onto plan
  for (const [dest, days] of destDays) {
    const destObj = plan.destinations?.[dest];
    if (!destObj) continue;

    // Reconstruct itinerary from tables
    if (!destObj.process_5_daily_itinerary) {
      destObj.process_5_daily_itinerary = {};
    }
    const p5 = destObj.process_5_daily_itinerary;

    const reconstructedDays: Record<string, unknown>[] = [];

    for (const dayRow of days) {
      const dayNumber = parseInt(dayRow.day_number!, 10);
      const day: Record<string, unknown> = {
        day_number: dayNumber,
        date: dayRow.date,
        theme: dayRow.theme,
        day_type: dayRow.day_type,
        status: dayRow.status || 'draft',
      };

      // Weather (use !== null to preserve valid zero values including 0)
      const hasWeather = dayRow.weather_label !== null || dayRow.temp_low_c !== null
        || dayRow.temp_high_c !== null || dayRow.precipitation_pct !== null || dayRow.weather_code !== null;
      if (hasWeather) {
        day.weather = {
          weather_label: dayRow.weather_label,
          temp_low_c: dayRow.temp_low_c !== null ? parseFloat(dayRow.temp_low_c) : undefined,
          temp_high_c: dayRow.temp_high_c !== null ? parseFloat(dayRow.temp_high_c) : undefined,
          precipitation_pct: dayRow.precipitation_pct !== null ? parseFloat(dayRow.precipitation_pct) : undefined,
          weather_code: dayRow.weather_code !== null ? parseInt(dayRow.weather_code, 10) : undefined,
          source_id: dayRow.weather_source_id,
          sourced_at: dayRow.weather_sourced_at,
        };
      }

      // Sessions
      for (const sessionType of ['morning', 'afternoon', 'evening']) {
        const key = sKey(dest, String(dayNumber), sessionType);
        const sRow = sessionMap.get(key);

        const session: Record<string, unknown> = {
          focus: sRow?.focus || null,
          transit_notes: sRow?.transit_notes || null,
          booking_notes: sRow?.booking_notes || null,
          activities: [],
        };

        if (sRow?.meals_json) {
          try { session.meals = JSON.parse(sRow.meals_json); } catch { /* ignore */ }
        }
        if (sRow?.time_range_start || sRow?.time_range_end) {
          session.time_range = { start: sRow?.time_range_start, end: sRow?.time_range_end };
        }

        // Activities
        const acts = activityMap.get(key) || [];
        const activityObjects: Record<string, unknown>[] = [];
        for (const a of acts) {
          activityObjects.push({
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
            tags: a.tags_json ? (() => { try { return JSON.parse(a.tags_json); } catch { return []; } })() : [],
            notes: a.notes || null,
            priority: a.priority || 'want',
          });
        }

        session.activities = activityObjects;
        day[sessionType] = session;
      }

      reconstructedDays.push(day);
    }

    p5.days = reconstructedDays;

    // Overlay flight from table
    const flightRow = flightMap.get(dest);
    if (flightRow && destObj.process_3_transportation) {
      const p3 = destObj.process_3_transportation;
      if (flightRow.populated_from) p3.populated_from = flightRow.populated_from;
      p3.flight = {
        airline: flightRow.airline,
        airline_code: flightRow.airline_code,
        outbound: flightRow.outbound_json ? JSON.parse(flightRow.outbound_json) : null,
        return: flightRow.return_json ? JSON.parse(flightRow.return_json) : null,
        booked_date: flightRow.booked_date,
      };
    }

    // Overlay hotel from table
    const hotelRow = hotelMap.get(dest);
    if (hotelRow && destObj.process_4_accommodation) {
      const p4 = destObj.process_4_accommodation;
      if (hotelRow.populated_from) p4.populated_from = hotelRow.populated_from;
      p4.hotel = {
        name: hotelRow.name,
        access: hotelRow.access_json ? JSON.parse(hotelRow.access_json) : null,
        check_in: hotelRow.check_in,
        notes: hotelRow.notes,
      };
    }

    // Overlay airport transfers from table
    const transfers = transferMap.get(dest);
    if (transfers && destObj.process_3_transportation) {
      const p3 = destObj.process_3_transportation;
      if (!p3.airport_transfers) p3.airport_transfers = {};
      for (const tr of transfers) {
        const dir = tr.direction!;
        p3.airport_transfers[dir] = {
          status: tr.status || 'planned',
          selected: tr.selected_json ? JSON.parse(tr.selected_json) : null,
          candidates: tr.candidates_json ? JSON.parse(tr.candidates_json) : [],
        };
      }
    }
  }

  return {
    plan_json: JSON.stringify(plan),
    state_json: blobRows[0].state_json ?? null,
    updated_at: blobRows[0].updated_at!,
  };
}

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

export interface PlanSummary {
  slug: string;
  display_name: string;
  updated_at: string;
}

export async function listPlans(env: Env): Promise<PlanSummary[]> {
  const rows = await queryTurso(
    env,
    `SELECT plan_id,
      json_extract(plan_json, '$.active_destination') as active_dest,
      json_extract(plan_json, '$.destinations.' ||
        json_extract(plan_json, '$.active_destination') || '.display_name') as display_name,
      updated_at FROM plans ORDER BY updated_at DESC`
  );
  return rows.map((r) => {
    const dest = r.active_dest || r.plan_id!;
    return {
      slug: dest.replace(/_/g, '-'),
      display_name: r.display_name || dest.replace(/_/g, ' '),
      updated_at: r.updated_at!,
    };
  });
}

export async function getBookings(
  env: Env,
  destination: string
): Promise<BookingRow[]> {
  const escaped = destination.replace(/'/g, "''");
  const rows = await queryTurso(
    env,
    `SELECT booking_key, trip_id, destination, category, title, status, reference, book_by, price_amount, price_currency, payload_json FROM bookings_current WHERE destination = '${escaped}'`
  );

  return rows as unknown as BookingRow[];
}
