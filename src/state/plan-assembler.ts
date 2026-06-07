/**
 * Plan Assembly — reconstruct TravelPlanMinimal + EventLogState from normalized table rows.
 * Used by TursoRepository.create() and dashboard.
 */
import type { TravelPlanMinimal, EventLogState, CascadeState, TravelEvent } from './types';

type R = Record<string, any>;

function num(v: any): number | null { return v != null ? Number(v) : null; }
function bool(v: any): boolean { return v === 1 || v === '1' || v === true; }

/** Strip null values from a DB row — Zod optional() accepts undefined but not null. */
function coerceRow<T extends Record<string, unknown>>(row: T): T {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(row)) out[k] = v === null ? undefined : v;
  return out as T;
}

/** Index rows by a key field */
function groupBy(rows: R[], key: string): Map<string, R[]> {
  const m = new Map<string, R[]>();
  for (const r of rows) { const k = r[key]; if (!m.has(k)) m.set(k, []); m.get(k)!.push(r); }
  return m;
}

function compositeKey(...parts: (string | number)[]): string { return parts.join(':'); }

export function assemblePlan(
  planId: string,
  metaRows: R[], destRows: R[], detailRows: R[], cityRows: R[],
  dateRows: R[], statusRows: R[], dirtyRows: R[],
  offerRows: R[], offerFlightRows: R[], offerHotelRows: R[],
  offerDatePricingRows: R[], offerSelRows: R[],
  budgetRows: R[], triggerRows: R[], contractRows: R[],
  precedenceRows: R[], cascadeGlobalRows: R[], rootP1Rows: R[],
  dayRows: R[], sessionRows: R[], activityRows: R[],
  flightLegRows: R[], hotelRows: R[], transferRows: R[],
  locZoneRows: R[], transportExtraRows: R[], itinMetaRows: R[],
  transferCandRows: R[], accessLineRows: R[], mealRows: R[], tagRows: R[],
  bestValueRows: R[], routeSegmentRows: R[] = [], activitiesZhRows: R[] = [],
  offerIncludesRows: R[] = [], offerHotelAccessRows: R[] = [],
  locZoneCandRows: R[] = [], transportExtraCandRows: R[] = [],
  triggerResetRows: R[] = [], triggerPopulateMapRows: R[] = [],
  schemaNodeRows: R[] = [], precedenceEntryRows: R[] = [], flexDateRows: R[] = [],
  transitKeyLineRows: R[] = [],
): TravelPlanMinimal {
  const meta = metaRows[0];

  // --- Root-level ---
  const plan: any = {
    schema_version: meta.schema_version || '5.0.0',
    active_destination: meta.active_destination || '',
    cascade_state: { last_cascade_run: '', global: { process_1_date_anchor: { dirty: false, last_changed: null } }, destinations: {} } as CascadeState,
    destinations: {},
  };

  // budget
  if (budgetRows.length > 0) {
    const b = budgetRows[0];
    plan.budget = { total_cap: num(b.total_cap), flight_cap: num(b.flight_cap), accommodation_cap: num(b.accommodation_cap), daily_cap: num(b.daily_cap), pax: num(b.pax) ?? 2, currency: b.currency || 'TWD' };
  }

  // schema_contract — process_nodes from child rows (no JSON column)
  if (contractRows.length > 0) {
    const sc = contractRows[0];
    const nodes = (schemaNodeRows || []).map((r: R) => r.node);
    plan.schema_contract = { id_convention: sc.id_convention, currency: sc.currency, process_nodes: nodes };
  }

  // process_precedence — reconstruct keyed object from entry rows (no JSON column)
  if (precedenceEntryRows && precedenceEntryRows.length > 0) {
    const prec: Record<string, unknown> = {};
    for (const e of precedenceEntryRows) {
      prec[e.name] = {
        ...(e.primary_process != null ? { primary: e.primary_process } : {}),
        ...(e.mode != null ? { mode: e.mode } : {}),
        ...(e.fallback_text ? { fallback: String(e.fallback_text).split('; ') } : {}),
        ...(e.rules_text ? { rules: String(e.rules_text).split('; ') } : {}),
      };
    }
    plan.process_precedence = prec;
  }

  // cascade_rules — reset/condition/populate_map from child rows + scalar cols
  if (triggerRows.length > 0) {
    const resetByTrigger = groupBy(triggerResetRows, 'trigger_id');
    const populateByTrigger = groupBy(triggerPopulateMapRows, 'trigger_id');
    plan.cascade_rules = {
      triggers: triggerRows.map(t => {
        const condition = t.condition_field != null
          ? { field: t.condition_field, ...(t.condition_changed != null ? { changed: num(t.condition_changed) === 1 } : {}) }
          : null;
        const pmRows = populateByTrigger.get(t.trigger_id) || [];
        const populate_map = pmRows.length > 0
          ? Object.fromEntries(pmRows.map((r: R) => [r.source_path, r.target_path]))
          : null;
        return {
          id: t.trigger_id, trigger: t.event,
          reset: (resetByTrigger.get(t.trigger_id) || []).map((r: R) => r.target),
          scope: t.scope, condition,
          action: t.action, populate_map, set_source: t.set_source,
        };
      }),
    };
  }

  // root P1
  if (rootP1Rows.length > 0) {
    const p1 = rootP1Rows[0];
    // flexibility from scalar cols + flex-date child rows (no JSON column)
    let flexibility: Record<string, unknown> | null = null;
    if (p1.flex_date_flexible != null || p1.flex_reason != null || (flexDateRows && flexDateRows.length > 0)) {
      const preferred = (flexDateRows || []).filter((r: R) => r.kind === 'preferred').map((r: R) => r.date);
      const avoid = (flexDateRows || []).filter((r: R) => r.kind === 'avoid').map((r: R) => r.date);
      flexibility = {
        ...(p1.flex_date_flexible != null ? { date_flexible: num(p1.flex_date_flexible) === 1 } : {}),
        preferred_dates: preferred,
        avoid_dates: avoid,
        ...(p1.flex_reason != null ? { reason: p1.flex_reason } : {}),
      };
    }
    plan.process_1_date_anchor = {
      status: p1.status, set_out_date: p1.set_out_date,
      duration_days: num(p1.duration_days), return_date: p1.return_date,
      flexibility,
    };
  }

  // cascade_global_state
  if (cascadeGlobalRows.length > 0) {
    const cg = cascadeGlobalRows[0];
    plan.cascade_state.last_cascade_run = cg.last_cascade_run || '';
    plan.cascade_state.global = {
      process_1_date_anchor: { dirty: bool(cg.p1_dirty), last_changed: null },
      active_destination_last: cg.active_dest_last,
    };
  }

  // --- Index helpers ---
  const detailMap = new Map(detailRows.map(r => [r.destination, r]));
  const citiesByDest = groupBy(cityRows, 'destination');
  const datesByDest = new Map(dateRows.map(r => [r.destination, r]));
  const statusByDest = groupBy(statusRows, 'destination');
  const dirtyByDest = groupBy(dirtyRows, 'destination');
  const offersByDest = groupBy(offerRows, 'destination');
  const offerFlightsByOffer = groupBy(offerFlightRows, 'offer_id');
  const offerHotelsByOffer = new Map(offerHotelRows.map(r => [r.offer_id, r]));
  const offerDPByOffer = groupBy(offerDatePricingRows, 'offer_id');
  const offerSelByDest = new Map(offerSelRows.map(r => [r.destination, r]));
  const flightsByDest = groupBy(flightLegRows, 'destination');
  const hotelByDest = new Map(hotelRows.map(r => [r.destination, r]));
  const transferByDest = groupBy(transferRows, 'destination');
  const locZoneByDest = new Map(locZoneRows.map(r => [r.destination, r]));
  const transportExtraByDest = new Map(transportExtraRows.map(r => [r.destination, r]));
  const itinMetaByDest = new Map(itinMetaRows.map(r => [r.destination, r]));
  const candsByDest = groupBy(transferCandRows, 'destination');
  const accessByDest = groupBy(accessLineRows, 'destination');
  // Batch D child-row maps (no JSON columns).
  const offerIncludesByOffer = groupBy(offerIncludesRows, 'offer_id');
  const offerHotelAccessByOffer = groupBy(offerHotelAccessRows, 'offer_id');
  const locZoneCandsByDest = groupBy(locZoneCandRows, 'destination');
  const transportExtraCandsByDest = groupBy(transportExtraCandRows, 'destination');
  // transit key lines by destination:lang (no JSON column)
  const transitKeyLinesByDestLang = new Map<string, string[]>();
  for (const r of transitKeyLineRows) {
    const k = `${r.destination}:${r.lang}`;
    if (!transitKeyLinesByDestLang.has(k)) transitKeyLinesByDestLang.set(k, []);
    transitKeyLinesByDestLang.get(k)!.push(r.line);
  }
  const daysByDest = groupBy(dayRows, 'destination');
  const sKey = (dest: string, day: any, sess: string) => compositeKey(dest, day, sess);
  const sessionMap = new Map<string, R>();
  for (const r of sessionRows) sessionMap.set(sKey(r.destination, r.day_number, r.session_type), r);
  const activityMap = new Map<string, R[]>();
  for (const r of activityRows) { const k = sKey(r.destination, r.day_number, r.session_type); if (!activityMap.has(k)) activityMap.set(k, []); activityMap.get(k)!.push(r); }
  const mealMap = new Map<string, R[]>();
  for (const r of mealRows) { const k = sKey(r.destination, r.day_number, r.session_type); if (!mealMap.has(k)) mealMap.set(k, []); mealMap.get(k)!.push(r); }
  const tagMap = new Map<string, string[]>();
  for (const r of tagRows) { if (!tagMap.has(r.activity_id)) tagMap.set(r.activity_id, []); tagMap.get(r.activity_id)!.push(r.tag); }
  const activitiesZhMap = new Map<string, string[]>();
  for (const r of activitiesZhRows) { const k = sKey(r.destination, r.day_number, r.session_type); if (!activitiesZhMap.has(k)) activitiesZhMap.set(k, []); activitiesZhMap.get(k)!.push(r.activity); }
  const bestValueByOffer = new Map(bestValueRows.map(r => [r.offer_id, r]));
  const routeSegsByDest = groupBy(routeSegmentRows, 'destination');

  // --- Per-destination ---
  for (const dr of destRows) {
    const slug = dr.slug;
    const dest: any = { slug, display_name: dr.display_name, status: dr.status, created_at: dr.created_at, updated_at: dr.updated_at };

    // P1 date anchor
    const da = datesByDest.get(slug);
    if (da) {
      dest.process_1_date_anchor = { status: 'confirmed', confirmed_dates: { start: da.start_date, end: da.end_date }, days: num(da.days) };
    }

    // P2 destination
    const detail = detailMap.get(slug);
    const cities = citiesByDest.get(slug) || [];
    const statuses = statusByDest.get(slug) || [];
    const p2Status = statuses.find(s => s.process_id === 'process_2_destination')?.status || 'pending';
    if (detail) {
      dest.process_2_destination = {
        status: p2Status, origin_city: detail.origin_city, region: detail.region, primary_airport: detail.primary_airport,
        cities: cities.map(c => ({ slug: c.city_slug, display_name: c.display_name ?? c.city_slug, role: c.role, nights: num(c.nights) })),
      };
    }

    // P3_4 packages
    const offers = offersByDest.get(slug) || [];
    const sel = offerSelByDest.get(slug);
    const p34Status = statuses.find(s => s.process_id === 'process_3_4_packages')?.status || 'pending';
    dest.process_3_4_packages = {
      status: p34Status,
      selected_offer_id: sel?.selected_offer_id || null,
      results: {
        offers: offers.map(o => {
          const flights = offerFlightsByOffer.get(o.id) || [];
          const hotel = offerHotelsByOffer.get(o.id);
          const dp = offerDPByOffer.get(o.id) || [];
          const bv = bestValueByOffer.get(o.id);
          const offer: any = {
            id: o.id, source_id: o.source_id, type: o.type, title: o.title,
            product_code: o.product_code, url: o.url, scraped_at: o.scraped_at,
            duration_days: num(o.duration_days), currency: o.currency,
            price_per_person: num(o.price_per_person), price_total: num(o.price_total),
            availability: o.availability, seats_remaining: num(o.seats_remaining),
            includes: (offerIncludesByOffer.get(o.id) || []).map((r: R) => r.item),
          };
          if (flights.length > 0) {
            offer.flight = { airline: flights[0].airline, airline_code: flights[0].airline_code, outbound: null, return: null };
            for (const f of flights) {
              offer.flight[f.direction] = { flight_number: f.flight_number, departure_airport_code: f.departure_code, departure_time: f.departure_time, arrival_airport_code: f.arrival_code, arrival_time: f.arrival_time };
            }
          }
          if (hotel) {
            offer.hotel = { name: hotel.name, ...(hotel.slug != null ? { slug: hotel.slug } : {}), area: hotel.area, star_rating: num(hotel.star_rating), access: (offerHotelAccessByOffer.get(o.id) || []).map((r: R) => r.line) };
          }
          if (dp.length > 0) {
            offer.date_pricing = {};
            for (const d of dp) offer.date_pricing[d.date] = { price: num(d.price), availability: d.availability, seats_remaining: num(d.seats_remaining) };
          }
          if (bv) offer.best_value = { date: bv.best_date, price: num(bv.best_price) };
          return offer;
        }),
        selection: sel ? { offer_id: sel.selected_offer_id, date: sel.selected_date, selected_at: sel.selected_at } : undefined,
      },
    };

    // P3 transportation
    const p3Status = statuses.find(s => s.process_id === 'process_3_transportation')?.status || 'pending';
    const txExtra = transportExtraByDest.get(slug);
    const p3: any = { status: p3Status, source: txExtra?.source || null, populated_from: txExtra?.populated_from || null };

    // Flight legs
    const legs = flightsByDest.get(slug) || [];
    if (legs.length > 0) {
      const first = legs[0];
      p3.flight = { airline: first.airline, airline_code: first.airline_code, booked_date: first.booked_date, outbound: null, return: null };
      for (const l of legs) {
        p3.flight[l.direction] = {
          flight_number: l.flight_number, departure_airport_code: l.departure_code,
          departure_terminal: l.departure_terminal, departure_time: l.departure_time,
          arrival_airport_code: l.arrival_code, arrival_terminal: l.arrival_terminal,
          arrival_time: l.arrival_time, date: l.flight_date,
        };
      }
    }

    // Airport transfers
    const transfers = transferByDest.get(slug) || [];
    const cands = candsByDest.get(slug) || [];
    if (transfers.length > 0) {
      p3.airport_transfers = {};
      for (const tr of transfers) {
        const dir = tr.direction;
        const dirCands = cands.filter(c => c.direction === dir);
        const selected = (tr.selected_title || tr.selected_id) ? {
          id: tr.selected_id ?? null,
          title: tr.selected_title, route: tr.selected_route,
          duration_min: num(tr.selected_duration_min), price_yen: num(tr.selected_price_yen),
          schedule: tr.selected_schedule, booking_url: tr.selected_booking_url, notes: tr.selected_notes,
        } : null;
        p3.airport_transfers[dir] = {
          status: tr.status || 'planned', selected,
          candidates: dirCands.map(c => ({
            id: c.candidate_id, title: c.title, route: c.route,
            duration_min: num(c.duration_min), price_yen: num(c.price_yen),
            schedule: c.schedule, booking_url: c.booking_url, notes: c.notes,
          })),
        };
      }
    }

    // home_to_airport / airport_to_hotel (legacy transport extras) — from child rows
    const teCands = transportExtraCandsByDest.get(slug) || [];
    const buildExtra = (direction: string, status: string | null) => {
      const dirCands = teCands.filter((c: R) => c.direction === direction);
      if (!status && dirCands.length === 0) return undefined;
      return {
        status: status || 'planned',
        candidates: dirCands.map((c: R) => ({
          id: c.candidate_id, method: c.method, route: c.route,
          departure_time: c.departure_time, arrival_time: c.arrival_time,
          duration_min: num(c.duration_min), cost_jpy: num(c.cost_jpy), transfers: num(c.transfers),
          ...(c.extra_text ? { notes: c.extra_text } : {}),
        })),
      };
    };
    const h2a = buildExtra('home_to_airport', txExtra?.home_to_airport_status ?? null);
    const a2h = buildExtra('airport_to_hotel', txExtra?.airport_to_hotel_status ?? null);
    if (h2a) p3.home_to_airport = h2a;
    if (a2h) p3.airport_to_hotel = a2h;

    dest.process_3_transportation = p3;

    // P4 accommodation
    const p4Status = statuses.find(s => s.process_id === 'process_4_accommodation')?.status || 'pending';
    const hotelRow = hotelByDest.get(slug);
    const lz = locZoneByDest.get(slug);
    const p4: any = { status: p4Status, source: null, populated_from: hotelRow?.populated_from || null };
    if (lz) {
      const lzCands = (locZoneCandsByDest.get(slug) || []).map((c: R) => ({
        slug: c.slug, display_name: c.display_name,
        ...(c.pros_text ? { pros: String(c.pros_text).split('; ') } : {}),
        ...(c.cons_text ? { cons: String(c.cons_text).split('; ') } : {}),
      }));
      p4.location_zone = { status: lz.selected_area ? 'selected' : 'pending', selected_area: lz.selected_area, candidates: lzCands };
    }
    if (hotelRow) {
      const hotel = coerceRow(hotelRow);
      const accessLines = accessByDest.get(slug) || [];
      p4.hotel = {
        name: hotel.name, access: accessLines.map((a: R) => a.line),
        check_in: hotel.check_in, notes: hotel.notes,
      };
    }
    dest.process_4_accommodation = p4;

    // P5 itinerary
    const p5Status = statuses.find(s => s.process_id === 'process_5_daily_itinerary')?.status || 'pending';
    const itinMetaRaw = itinMetaByDest.get(slug);
    const itinMeta = itinMetaRaw ? coerceRow(itinMetaRaw) : undefined;
    // transit_summary {hotel_station, key_lines[]} from scalar cols + child rows (no JSON)
    const buildTransit = (lang: 'en' | 'zh', hotelStation: unknown) => {
      const keyLines = transitKeyLinesByDestLang.get(`${slug}:${lang}`) || [];
      if (!hotelStation && keyLines.length === 0) return undefined;
      return { ...(hotelStation ? { hotel_station: hotelStation } : {}), key_lines: keyLines };
    };
    const p5: any = {
      status: p5Status, scaffolded_at: itinMeta?.scaffolded_at, populated_at: itinMeta?.populated_at,
      transit_summary: buildTransit('en', itinMeta?.transit_hotel_station),
      transit_summary_zh: buildTransit('zh', itinMeta?.transit_hotel_station_zh),
    };
    const days = daysByDest.get(slug) || [];
    p5.days = days.map(rawDayRow => {
      const dayRow = coerceRow(rawDayRow);
      const day: any = { day_number: num(dayRow.day_number), date: dayRow.date, theme: dayRow.theme, theme_zh: dayRow.theme_zh ?? null, day_type: dayRow.day_type, status: dayRow.status || 'draft' };
      if (dayRow.weather_label || dayRow.temp_low_c != null) {
        day.weather = {
          weather_label: dayRow.weather_label, temp_low_c: num(dayRow.temp_low_c), temp_high_c: num(dayRow.temp_high_c),
          feels_like_low_c: num(dayRow.feels_like_low_c), feels_like_high_c: num(dayRow.feels_like_high_c),
          precipitation_pct: num(dayRow.precipitation_pct), weather_code: num(dayRow.weather_code),
          source_id: dayRow.weather_source_id, sourced_at: dayRow.weather_sourced_at,
        };
      }
      for (const st of ['morning', 'noon', 'afternoon', 'evening']) {
        const sk = sKey(slug, dayRow.day_number, st);
        const sRow = sessionMap.get(sk);
        const meals = mealMap.get(sk) || [];
        const acts = activityMap.get(sk) || [];
        day[st] = {
          focus: sRow?.focus || null, focus_zh: sRow?.focus_zh || null,
          transit_notes: sRow?.transit_notes || null, transit_notes_zh: sRow?.transit_notes_zh || null,
          booking_notes: sRow?.booking_notes || null,
          activities_zh: activitiesZhMap.has(sk) ? activitiesZhMap.get(sk)! : null,
          meals_zh: null, // meals_zh_json removed (Batch C): was dead non-JSON transit text
          meals: meals.map(m => m.meal),
          time_range: (sRow?.time_range_start || sRow?.time_range_end) ? { start: sRow?.time_range_start, end: sRow?.time_range_end } : undefined,
          activities: acts.map(a => ({
            id: a.id, title: a.title, area: a.area || '', nearest_station: a.nearest_station || null,
            duration_min: num(a.duration_min), booking_required: bool(a.booking_required),
            booking_url: a.booking_url || null, booking_status: a.booking_status || undefined,
            booking_ref: a.booking_ref || undefined, book_by: a.book_by || undefined,
            start_time: a.start_time || undefined, end_time: a.end_time || undefined,
            is_fixed_time: bool(a.is_fixed_time), cost_estimate: num(a.cost_estimate),
            tags: tagMap.get(a.id) || [],
            notes: a.notes || null, priority: a.priority || 'want',
          })),
        };
      }
      const routeSegs = (routeSegsByDest.get(slug) || [])
        .filter((r: R) => String(r.day_number) === String(dayRow.day_number))
        .sort((a: R, b: R) => num(a.sort_order)! - num(b.sort_order)!);
      if (routeSegs.length > 0) {
        day.route_segments = routeSegs.map((r: R) => ({
          sort_order: num(r.sort_order),
          from_place: r.from_place,
          to_place: r.to_place,
          mode: r.mode,
          duration_min: num(r.duration_min) ?? null,
          notes: r.notes ?? null,
          start_time: r.start_time ?? null,
        }));
      }
      return day;
    });
    dest.process_5_daily_itinerary = p5;

    // Cascade dirty flags
    const flags = dirtyByDest.get(slug) || [];
    if (flags.length > 0) {
      plan.cascade_state.destinations[slug] = {};
      for (const f of flags) {
        plan.cascade_state.destinations[slug][f.process_id] = { dirty: bool(f.dirty), last_changed: f.last_changed };
      }
    }

    plan.destinations[slug] = dest;
  }

  return plan as TravelPlanMinimal;
}

export function assembleEventLog(
  stateRows: R[], globalRows: R[], destRows: R[], destProcRows: R[],
  planEventRows: R[], eventDataRows: R[] = [], nextActionRows: R[] = [],
): EventLogState {
  if (stateRows.length === 0) {
    return { session: new Date().toISOString().split('T')[0], project: 'japan-travel', version: '3.0', active_destination: '', current_focus: '', event_log: [], global_processes: {}, destinations: {} };
  }
  const s = stateRows[0];

  // plan_event_data KV → { natural-key → {key: value} } (rebuild open `data` object).
  // Key = scope:destination:process_id:sort_order (matches plan_events PK).
  const evKey = (r: R) => `${r.scope}:${r.destination ?? ''}:${r.process_id ?? ''}:${r.sort_order}`;
  const dataByEvent = new Map<string, Record<string, unknown> | string>();
  for (const r of eventDataRows) {
    const k = evKey(r);
    if (r.key === '_value') { dataByEvent.set(k, r.value); continue; }
    let bucket = dataByEvent.get(k);
    if (bucket == null || typeof bucket === 'string') { bucket = {}; dataByEvent.set(k, bucket); }
    (bucket as Record<string, unknown>)[r.key] = r.value;
  }
  const toEvent = (e: R): TravelEvent => {
    const data = dataByEvent.get(evKey(e));
    return {
      event: e.event, at: e.event_at,
      ...(e.destination != null ? { destination: e.destination } : {}),
      ...(e.process_id != null ? { process: e.process_id } : {}),
      ...(e.from_state != null ? { from: e.from_state } : {}),
      ...(e.to_state != null ? { to: e.to_state } : {}),
      ...(data != null ? { data } : {}),
    } as TravelEvent;
  };

  // Split plan_events by scope.
  const timeline: R[] = [];
  const globalEvByProc = new Map<string, R[]>();
  const destEvByKey = new Map<string, R[]>();
  for (const e of planEventRows) {
    if (e.scope === 'timeline') timeline.push(e);
    else if (e.scope === 'global_process') {
      if (!globalEvByProc.has(e.process_id)) globalEvByProc.set(e.process_id, []);
      globalEvByProc.get(e.process_id)!.push(e);
    } else if (e.scope === 'dest_process') {
      const k = `${e.destination}:${e.process_id}`;
      if (!destEvByKey.has(k)) destEvByKey.set(k, []);
      destEvByKey.get(k)!.push(e);
    }
  }

  const log: EventLogState = {
    session: s.session, project: s.project, version: s.version,
    active_destination: s.active_destination || '', current_focus: s.current_focus || '',
    next_actions: nextActionRows.map(r => r.action),
    event_log: timeline.map(toEvent),
    global_processes: {},
    destinations: {},
  };
  for (const g of globalRows) {
    log.global_processes[g.process_id] = { state: g.status, events: (globalEvByProc.get(g.process_id) || []).map(toEvent) };
  }
  const destProcByDest = new Map<string, R[]>();
  for (const r of destProcRows) { if (!destProcByDest.has(r.destination)) destProcByDest.set(r.destination, []); destProcByDest.get(r.destination)!.push(r); }
  for (const d of destRows) {
    const procs = destProcByDest.get(d.destination) || [];
    log.destinations[d.destination] = {
      status: d.status as 'active' | 'archived',
      processes: Object.fromEntries(procs.map(p => [p.process_id, { state: p.status, events: (destEvByKey.get(`${d.destination}:${p.process_id}`) || []).map(toEvent) }])),
    };
  }
  return log;
}
