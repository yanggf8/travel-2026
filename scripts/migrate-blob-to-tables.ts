/**
 * Phase 2: Migrate plan_json + state_json blobs → normalized tables.
 * Idempotent (DELETE + INSERT per plan). Run after turso-migrate.ts.
 */
import { TursoPipelineClient } from './turso-pipeline';

const esc = (s: string | null | undefined): string =>
  s == null ? 'NULL' : `'${String(s).replace(/'/g, "''")}'`;
const escInt = (v: number | null | undefined): string =>
  v == null ? 'NULL' : String(Math.round(v));
const escJson = (v: unknown): string =>
  v == null ? 'NULL' : esc(JSON.stringify(v));

async function main() {
  const client = new TursoPipelineClient();

  // Load all plans
  const plansResp = await client.execute('SELECT plan_id, plan_json, state_json FROM plans');
  const result = plansResp?.results?.[0]?.response?.result as any;
  const cols = (result?.cols ?? []).map((c: any) => c.name);
  const rawRows = result?.rows ?? [];

  for (const raw of rawRows) {
    const row: Record<string, string | null> = {};
    cols.forEach((name: string, i: number) => { row[name] = (raw as any)[i]?.value ?? null; });

    const planId = row.plan_id!;
    const plan = JSON.parse(row.plan_json!);
    const state = row.state_json ? JSON.parse(row.state_json) : null;

    console.log(`\n=== Migrating plan: ${planId} ===`);
    const stmts: string[] = [];

    // Delete existing rows for this plan in all new tables
    const newTables = [
      'plan_destinations', 'destination_details', 'destination_cities',
      'plan_offers', 'plan_offer_flights', 'plan_offer_hotels',
      'plan_offer_date_pricing', 'plan_offer_best_value', 'plan_offer_selection',
      'plan_offer_provenance', 'plan_offer_warnings',
      'plan_budget', 'cascade_triggers', 'plan_schema_contract',
      'plan_process_precedence', 'cascade_global_state', 'plan_root_date_anchor',
      'itinerary_metadata', 'accommodation_location_zone', 'transportation_extras',
      'event_log_state', 'event_log_global_processes', 'event_log_destinations',
      'event_log_dest_processes', 'event_log_process_events',
      'airport_transfer_candidates', 'hotel_access_lines', 'session_meals', 'activity_tags',
    ];
    for (const t of newTables) {
      stmts.push(`DELETE FROM ${t} WHERE plan_id = ${esc(planId)}`);
    }
    // activity_tags uses activity_id, delete by prefix
    stmts.push(`DELETE FROM activity_tags WHERE activity_id LIKE ${esc(planId + '%')} OR activity_id LIKE 'activity_%'`);

    // --- Root-level data ---

    // budget
    if (plan.budget) {
      const b = plan.budget;
      stmts.push(`INSERT INTO plan_budget (plan_id, total_cap, flight_cap, accommodation_cap, daily_cap, pax, currency)
        VALUES (${esc(planId)}, ${escInt(b.total_cap)}, ${escInt(b.flight_cap)}, ${escInt(b.accommodation_cap)}, ${escInt(b.daily_cap)}, ${escInt(b.pax ?? 1)}, ${esc(b.currency ?? 'TWD')})`);
    }

    // schema_contract
    if (plan.schema_contract) {
      const sc = plan.schema_contract;
      stmts.push(`INSERT INTO plan_schema_contract (plan_id, id_convention, currency, process_nodes_json)
        VALUES (${esc(planId)}, ${esc(sc.id_convention)}, ${esc(sc.currency ?? 'TWD')}, ${escJson(sc.process_nodes)})`);
    }

    // process_precedence
    if (plan.process_precedence) {
      stmts.push(`INSERT INTO plan_process_precedence (plan_id, precedence_json)
        VALUES (${esc(planId)}, ${escJson(plan.process_precedence)})`);
    }

    // cascade_triggers
    if (plan.cascade_rules?.triggers) {
      for (const t of plan.cascade_rules.triggers) {
        stmts.push(`INSERT INTO cascade_triggers (plan_id, trigger_id, event, reset_json, scope, condition_json, action, populate_map_json, set_source)
          VALUES (${esc(planId)}, ${esc(t.id)}, ${esc(t.trigger)}, ${escJson(t.reset)}, ${esc(t.scope)}, ${escJson(t.condition)}, ${esc(t.action)}, ${escJson(t.populate_map)}, ${esc(t.set_source)})`);
      }
    }

    // root P1
    if (plan.process_1_date_anchor) {
      const p1 = plan.process_1_date_anchor;
      stmts.push(`INSERT INTO plan_root_date_anchor (plan_id, status, set_out_date, duration_days, return_date, flexibility_json)
        VALUES (${esc(planId)}, ${esc(p1.status)}, ${esc(p1.set_out_date)}, ${escInt(p1.duration_days)}, ${esc(p1.return_date)}, ${escJson(p1.flexibility)})`);
    }

    // cascade_global_state
    if (plan.cascade_state) {
      const cs = plan.cascade_state;
      stmts.push(`INSERT INTO cascade_global_state (plan_id, last_cascade_run, active_dest_last, p1_dirty)
        VALUES (${esc(planId)}, ${esc(cs.last_cascade_run)}, ${esc(cs.global?.active_destination_last)}, ${cs.global?.process_1_date_anchor?.dirty ? 1 : 0})`);
    }

    // --- Per-destination data ---
    for (const [destSlug, destObj] of Object.entries(plan.destinations || {})) {
      const dest = destObj as Record<string, any>;

      // plan_destinations
      stmts.push(`INSERT INTO plan_destinations (plan_id, slug, display_name, status, created_at, updated_at)
        VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(dest.display_name ?? destSlug)}, ${esc(dest.status ?? 'draft')}, ${esc(dest.created_at)}, ${esc(dest.updated_at)})`);

      // destination_details (P2)
      const p2 = dest.process_2_destination;
      if (p2) {
        stmts.push(`INSERT INTO destination_details (plan_id, destination, origin_city, region, primary_airport)
          VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(p2.origin_city)}, ${esc(p2.region)}, ${esc(p2.primary_airport)})`);

        // destination_cities
        if (p2.cities) {
          for (const city of p2.cities) {
            stmts.push(`INSERT INTO destination_cities (plan_id, destination, city_slug, role, nights)
              VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(city.slug)}, ${esc(city.role)}, ${escInt(city.nights)})`);
          }
        }
      }

      // plan_offers (P3_4)
      const p34 = dest.process_3_4_packages;
      const offers = p34?.results?.offers || p34?.offers || [];
      for (const o of offers) {
        stmts.push(`INSERT INTO plan_offers (plan_id, destination, id, source_id, type, title, price_per_person, currency, availability, url, scraped_at, product_code, duration_days, price_total, seats_remaining, includes_json)
          VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(o.id)}, ${esc(o.source_id)}, ${esc(o.type)}, ${esc(o.title)}, ${escInt(o.price_per_person)}, ${esc(o.currency ?? 'TWD')}, ${esc(o.availability)}, ${esc(o.url)}, ${esc(o.scraped_at)}, ${esc(o.product_code)}, ${escInt(o.duration_days)}, ${escInt(o.price_total)}, ${escInt(o.seats_remaining)}, ${escJson(o.includes)})`);

        // offer flights
        if (o.flight) {
          for (const dir of ['outbound', 'return'] as const) {
            const leg = o.flight[dir];
            if (!leg) continue;
            stmts.push(`INSERT INTO plan_offer_flights (plan_id, destination, offer_id, direction, flight_number, airline, airline_code, departure_code, departure_time, arrival_code, arrival_time)
              VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(o.id)}, ${esc(dir)}, ${esc(leg.flight_number)}, ${esc(o.flight.airline)}, ${esc(o.flight.airline_code)}, ${esc(leg.departure_airport_code)}, ${esc(leg.departure_time)}, ${esc(leg.arrival_airport_code)}, ${esc(leg.arrival_time)})`);
          }
        }

        // offer hotel
        if (o.hotel) {
          stmts.push(`INSERT INTO plan_offer_hotels (plan_id, destination, offer_id, name, slug, area, star_rating, access_json)
            VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(o.id)}, ${esc(o.hotel.name)}, ${esc(o.hotel.slug)}, ${esc(o.hotel.area)}, ${escInt(o.hotel.star_rating)}, ${escJson(o.hotel.access)})`);
        }

        // date_pricing
        if (o.date_pricing) {
          for (const [date, dp] of Object.entries(o.date_pricing)) {
            const d = dp as any;
            stmts.push(`INSERT INTO plan_offer_date_pricing (plan_id, destination, offer_id, date, price, availability, seats_remaining)
              VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(o.id)}, ${esc(date)}, ${escInt(d.price)}, ${esc(d.availability)}, ${escInt(d.seats_remaining)})`);
          }
        }

        // best_value (compute from date_pricing)
        if (o.date_pricing) {
          let bestDate: string | null = null, bestPrice = Infinity;
          for (const [date, dp] of Object.entries(o.date_pricing)) {
            const d = dp as any;
            if (d.availability !== 'sold_out' && d.price < bestPrice) {
              bestPrice = d.price; bestDate = date;
            }
          }
          if (bestDate) {
            stmts.push(`INSERT INTO plan_offer_best_value (plan_id, destination, offer_id, best_date, best_price)
              VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(o.id)}, ${esc(bestDate)}, ${escInt(bestPrice)})`);
          }
        }
      }

      // offer selection
      if (p34?.selected_offer_id || p34?.results?.selection) {
        const sel = p34.results?.selection || {};
        stmts.push(`INSERT INTO plan_offer_selection (plan_id, destination, selected_offer_id, selected_date, selected_at)
          VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(p34.selected_offer_id || sel.offer_id)}, ${esc(sel.date)}, ${esc(sel.selected_at || p34.updated_at)})`);
      }

      // provenance
      if (p34?.provenance) {
        for (const p of p34.provenance) {
          stmts.push(`INSERT INTO plan_offer_provenance (plan_id, destination, source_id, scraped_at, file_path)
            VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(p.source_id)}, ${esc(p.scraped_at)}, ${esc(p.file_path)})`);
        }
      }

      // itinerary_metadata
      const p5 = dest.process_5_daily_itinerary;
      if (p5) {
        stmts.push(`INSERT INTO itinerary_metadata (plan_id, destination, scaffolded_at, populated_at, transit_summary)
          VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(p5.scaffolded_at)}, ${esc(p5.populated_at)}, ${esc(p5.transit_summary)})`);
      }

      // accommodation_location_zone
      const p4 = dest.process_4_accommodation;
      if (p4?.location_zone) {
        const lz = p4.location_zone;
        stmts.push(`INSERT INTO accommodation_location_zone (plan_id, destination, selected_area, source, candidates_json)
          VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(lz.selected_area)}, ${esc(lz.source)}, ${escJson(lz.candidates)})`);
      }

      // transportation_extras
      const p3 = dest.process_3_transportation;
      if (p3) {
        stmts.push(`INSERT INTO transportation_extras (plan_id, destination, source, populated_from, home_to_airport_json, airport_to_hotel_json)
          VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(p3.source)}, ${esc(p3.populated_from)}, ${escJson(p3.home_to_airport)}, ${escJson(p3.airport_to_hotel)})`);
      }

      // airport_transfer_candidates (normalize from airport_transfers table selected_json/candidates_json)
      if (p3?.airport_transfers) {
        for (const dir of ['arrival', 'departure'] as const) {
          const seg = p3.airport_transfers[dir];
          if (!seg) continue;
          // Write selected_* scalars to airport_transfers
          if (seg.selected) {
            const s = seg.selected;
            stmts.push(`UPDATE airport_transfers SET
              selected_title = ${esc(s.title)}, selected_route = ${esc(s.route)},
              selected_duration_min = ${escInt(s.duration_min)}, selected_price_yen = ${escInt(s.price_yen)},
              selected_schedule = ${esc(s.schedule)}, selected_booking_url = ${esc(s.booking_url)},
              selected_notes = ${esc(s.notes)}
              WHERE plan_id = ${esc(planId)} AND destination = ${esc(destSlug)} AND direction = ${esc(dir)}`);
          }
          // Candidates → airport_transfer_candidates
          const candidates = seg.candidates || [];
          for (let i = 0; i < candidates.length; i++) {
            const c = candidates[i];
            stmts.push(`INSERT INTO airport_transfer_candidates (plan_id, destination, direction, candidate_id, title, route, duration_min, price_yen, schedule, booking_url, notes, sort_order)
              VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(dir)}, ${esc(c.id)}, ${esc(c.title || c.method)}, ${esc(c.route)}, ${escInt(c.duration_min)}, ${escInt(c.price_yen ?? c.cost_jpy)}, ${esc(c.schedule)}, ${esc(c.booking_url)}, ${esc(c.notes)}, ${escInt(i)})`);
          }
        }
      }

      // hotel_access_lines
      if (p4?.hotel?.access && Array.isArray(p4.hotel.access)) {
        for (let i = 0; i < p4.hotel.access.length; i++) {
          stmts.push(`INSERT INTO hotel_access_lines (plan_id, destination, sort_order, line)
            VALUES (${esc(planId)}, ${esc(destSlug)}, ${escInt(i)}, ${esc(p4.hotel.access[i])})`);
        }
      }

      // session_meals + activity_tags (from itinerary days)
      if (p5?.days) {
        for (const day of p5.days) {
          for (const sessionType of ['morning', 'afternoon', 'evening']) {
            const session = day[sessionType];
            if (!session) continue;
            // meals
            if (session.meals && Array.isArray(session.meals)) {
              for (let i = 0; i < session.meals.length; i++) {
                stmts.push(`INSERT INTO session_meals (plan_id, destination, day_number, session_type, sort_order, meal)
                  VALUES (${esc(planId)}, ${esc(destSlug)}, ${escInt(day.day_number)}, ${esc(sessionType)}, ${escInt(i)}, ${esc(session.meals[i])})`);
              }
            }
            // activity tags
            if (session.activities) {
              for (const act of session.activities) {
                if (typeof act === 'object' && act.id && act.tags && Array.isArray(act.tags)) {
                  for (const tag of act.tags) {
                    stmts.push(`INSERT OR IGNORE INTO activity_tags (activity_id, tag)
                      VALUES (${esc(act.id)}, ${esc(tag)})`);
                  }
                }
              }
            }
          }
        }
      }
    }

    // --- Event log (state_json) ---
    if (state) {
      stmts.push(`INSERT INTO event_log_state (plan_id, session, project, version, current_focus, active_destination, next_actions_json)
        VALUES (${esc(planId)}, ${esc(state.session)}, ${esc(state.project)}, ${esc(state.version)}, ${esc(state.current_focus)}, ${esc(state.active_destination)}, ${escJson(state.next_actions)})`);

      // global_processes
      for (const [pid, pobj] of Object.entries(state.global_processes || {})) {
        const p = pobj as any;
        stmts.push(`INSERT INTO event_log_global_processes (plan_id, process_id, status, events_json)
          VALUES (${esc(planId)}, ${esc(pid)}, ${esc(p.state)}, ${escJson(p.events)})`);
      }

      // destinations
      for (const [destSlug, dobj] of Object.entries(state.destinations || {})) {
        const d = dobj as any;
        stmts.push(`INSERT INTO event_log_destinations (plan_id, destination, status)
          VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(d.status)})`);

        for (const [pid, pobj] of Object.entries(d.processes || {})) {
          const p = pobj as any;
          stmts.push(`INSERT INTO event_log_dest_processes (plan_id, destination, process_id, status, events_json)
            VALUES (${esc(planId)}, ${esc(destSlug)}, ${esc(pid)}, ${esc(p.state)}, ${escJson(p.events)})`);
        }
      }

      // event_log (flat events)
      if (state.event_log) {
        for (const evt of state.event_log) {
          stmts.push(`INSERT INTO event_log_process_events (plan_id, destination, process_id, event_type, event_data, event_at)
            VALUES (${esc(planId)}, ${esc(evt.destination)}, ${esc(evt.process ?? 'global')}, ${esc(evt.event)}, ${escJson(evt.data)}, ${esc(evt.at)})`);
        }
      }
    }

    // Execute in transaction
    if (stmts.length > 0) {
      stmts.unshift('BEGIN');
      stmts.push('COMMIT');
      try {
        await client.executeMany(stmts);
        console.log(`✅ Migrated ${planId}: ${stmts.length - 2} statements`);
      } catch (e: any) {
        console.error(`❌ Failed to migrate ${planId}: ${e.message}`);
        try { await client.execute('ROLLBACK'); } catch {}
      }
    }
  }

  console.log('\n✅ Migration complete.');
}

main().catch(console.error);
