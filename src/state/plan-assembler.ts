/**
 * Plan Assembly — reconstruct TravelPlanMinimal + EventLogState from normalized table rows.
 * Used by TursoRepository.create() and dashboard.
 */
import type { TravelPlanMinimal, EventLogState, CascadeState } from './types';

type R = Record<string, any>;

function num(v: any): number | null { return v != null ? Number(v) : null; }
function bool(v: any): boolean { return v === 1 || v === '1' || v === true; }
function tryJson(v: any): any { if (!v) return null; try { return JSON.parse(v); } catch { return null; } }

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
  bestValueRows: R[],
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

  // schema_contract
  if (contractRows.length > 0) {
    const sc = contractRows[0];
    plan.schema_contract = { id_convention: sc.id_convention, currency: sc.currency, process_nodes: tryJson(sc.process_nodes_json) || [] };
  }

  // process_precedence
  if (precedenceRows.length > 0) {
    plan.process_precedence = tryJson(precedenceRows[0].precedence_json);
  }

  // cascade_rules
  if (triggerRows.length > 0) {
    plan.cascade_rules = {
      triggers: triggerRows.map(t => ({
        id: t.trigger_id, trigger: t.event, reset: tryJson(t.reset_json) || [],
        scope: t.scope, condition: tryJson(t.condition_json),
        action: t.action, populate_map: tryJson(t.populate_map_json), set_source: t.set_source,
      })),
    };
  }

  // root P1
  if (rootP1Rows.length > 0) {
    const p1 = rootP1Rows[0];
    plan.process_1_date_anchor = {
      status: p1.status, set_out_date: p1.set_out_date,
      duration_days: num(p1.duration_days), return_date: p1.return_date,
      flexibility: tryJson(p1.flexibility_json),
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
  const bestValueByOffer = new Map(bestValueRows.map(r => [r.offer_id, r]));

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
            includes: tryJson(o.includes_json) || [],
          };
          if (flights.length > 0) {
            offer.flight = { airline: flights[0].airline, airline_code: flights[0].airline_code, outbound: null, return: null };
            for (const f of flights) {
              offer.flight[f.direction] = { flight_number: f.flight_number, departure_airport_code: f.departure_code, departure_time: f.departure_time, arrival_airport_code: f.arrival_code, arrival_time: f.arrival_time };
            }
          }
          if (hotel) {
            offer.hotel = { name: hotel.name, ...(hotel.slug != null ? { slug: hotel.slug } : {}), area: hotel.area, star_rating: num(hotel.star_rating), access: tryJson(hotel.access_json) || [] };
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
        const selected = tr.selected_title ? {
          id: tr.selected_json ? tryJson(tr.selected_json)?.id : null,
          title: tr.selected_title, route: tr.selected_route,
          duration_min: num(tr.selected_duration_min), price_yen: num(tr.selected_price_yen),
          schedule: tr.selected_schedule, booking_url: tr.selected_booking_url, notes: tr.selected_notes,
        } : (tr.selected_json ? tryJson(tr.selected_json) : null);
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

    // home_to_airport / airport_to_hotel (legacy transport extras)
    if (txExtra?.home_to_airport_json) p3.home_to_airport = tryJson(txExtra.home_to_airport_json);
    if (txExtra?.airport_to_hotel_json) p3.airport_to_hotel = tryJson(txExtra.airport_to_hotel_json);

    dest.process_3_transportation = p3;

    // P4 accommodation
    const p4Status = statuses.find(s => s.process_id === 'process_4_accommodation')?.status || 'pending';
    const hotelRow = hotelByDest.get(slug);
    const lz = locZoneByDest.get(slug);
    const p4: any = { status: p4Status, source: null, populated_from: hotelRow?.populated_from || null };
    if (lz) {
      p4.location_zone = { status: lz.selected_area ? 'selected' : 'pending', selected_area: lz.selected_area, candidates: tryJson(lz.candidates_json) || [] };
    }
    if (hotelRow) {
      const hotel = coerceRow(hotelRow);
      const accessLines = accessByDest.get(slug) || [];
      p4.hotel = {
        name: hotel.name, access: accessLines.length > 0 ? accessLines.map((a: R) => a.line) : (hotel.access_json ? tryJson(hotel.access_json) : []),
        check_in: hotel.check_in, notes: hotel.notes,
      };
    }
    dest.process_4_accommodation = p4;

    // P5 itinerary
    const p5Status = statuses.find(s => s.process_id === 'process_5_daily_itinerary')?.status || 'pending';
    const itinMetaRaw = itinMetaByDest.get(slug);
    const itinMeta = itinMetaRaw ? coerceRow(itinMetaRaw) : undefined;
    const p5: any = { status: p5Status, scaffolded_at: itinMeta?.scaffolded_at, populated_at: itinMeta?.populated_at, transit_summary: itinMeta?.transit_summary };
    const days = daysByDest.get(slug) || [];
    p5.days = days.map(rawDayRow => {
      const dayRow = coerceRow(rawDayRow);
      const day: any = { day_number: num(dayRow.day_number), date: dayRow.date, theme: dayRow.theme, day_type: dayRow.day_type, status: dayRow.status || 'draft' };
      if (dayRow.weather_label || dayRow.temp_low_c != null) {
        day.weather = {
          weather_label: dayRow.weather_label, temp_low_c: num(dayRow.temp_low_c), temp_high_c: num(dayRow.temp_high_c),
          feels_like_low_c: num(dayRow.feels_like_low_c), feels_like_high_c: num(dayRow.feels_like_high_c),
          precipitation_pct: num(dayRow.precipitation_pct), weather_code: num(dayRow.weather_code),
          source_id: dayRow.weather_source_id, sourced_at: dayRow.weather_sourced_at,
        };
      }
      for (const st of ['morning', 'afternoon', 'evening']) {
        const sk = sKey(slug, dayRow.day_number, st);
        const sRow = sessionMap.get(sk);
        const meals = mealMap.get(sk) || [];
        const acts = activityMap.get(sk) || [];
        day[st] = {
          focus: sRow?.focus || null, transit_notes: sRow?.transit_notes || null, booking_notes: sRow?.booking_notes || null,
          meals: meals.map(m => m.meal),
          time_range: (sRow?.time_range_start || sRow?.time_range_end) ? { start: sRow?.time_range_start, end: sRow?.time_range_end } : undefined,
          activities: acts.map(a => ({
            id: a.id, title: a.title, area: a.area || '', nearest_station: a.nearest_station || null,
            duration_min: num(a.duration_min), booking_required: bool(a.booking_required),
            booking_url: a.booking_url || null, booking_status: a.booking_status || undefined,
            booking_ref: a.booking_ref || undefined, book_by: a.book_by || undefined,
            start_time: a.start_time || undefined, end_time: a.end_time || undefined,
            is_fixed_time: bool(a.is_fixed_time), cost_estimate: num(a.cost_estimate),
            tags: tagMap.get(a.id) || (a.tags_json ? tryJson(a.tags_json) : []) || [],
            notes: a.notes || null, priority: a.priority || 'want',
          })),
        };
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
  stateRows: R[], globalRows: R[], destRows: R[], destProcRows: R[], evtRows: R[],
): EventLogState {
  if (stateRows.length === 0) {
    return { session: new Date().toISOString().split('T')[0], project: 'japan-travel', version: '3.0', active_destination: '', current_focus: '', event_log: [], global_processes: {}, destinations: {} };
  }
  const s = stateRows[0];
  const log: EventLogState = {
    session: s.session, project: s.project, version: s.version,
    active_destination: s.active_destination || '', current_focus: s.current_focus || '',
    next_actions: tryJson(s.next_actions_json) || [],
    event_log: evtRows.map(e => { const ev = coerceRow(e); return { event: ev.event_type, at: ev.event_at, destination: ev.destination, process: ev.process_id !== 'global' ? ev.process_id : undefined, data: tryJson(ev.event_data) }; }),
    global_processes: {},
    destinations: {},
  };
  for (const g of globalRows) {
    log.global_processes[g.process_id] = { state: g.status, events: tryJson(g.events_json) || [] };
  }
  const destProcByDest = new Map<string, R[]>();
  for (const r of destProcRows) { if (!destProcByDest.has(r.destination)) destProcByDest.set(r.destination, []); destProcByDest.get(r.destination)!.push(r); }
  for (const d of destRows) {
    const procs = destProcByDest.get(d.destination) || [];
    log.destinations[d.destination] = {
      status: d.status as 'active' | 'archived',
      processes: Object.fromEntries(procs.map(p => [p.process_id, { state: p.status, events: tryJson(p.events_json) || [] }])),
    };
  }
  return log;
}
