/**
 * Migrate Itinerary Data
 *
 * Reads each plan's JSON blob from plans_current and inserts into normalized tables:
 * - itinerary_days, itinerary_sessions, activities
 * - plan_metadata, date_anchors, process_statuses
 * - airport_transfers, flights, hotels
 *
 * Idempotent: uses INSERT OR REPLACE.
 * Run after: npm run db:migrate:turso (creates the tables)
 * Usage: npx ts-node scripts/migrate-itinerary-data.ts
 */

import { TursoPipelineClient } from './turso-pipeline';

function sqlText(v: string | null | undefined): string {
  if (v === null || v === undefined) return 'NULL';
  return `'${String(v).replace(/'/g, "''")}'`;
}

function sqlInt(v: number | null | undefined): string {
  if (v === null || v === undefined) return 'NULL';
  return String(Math.round(v));
}

function sqlReal(v: number | null | undefined): string {
  if (v === null || v === undefined) return 'NULL';
  return String(v);
}

function sqlBool(v: boolean | null | undefined): string {
  if (v === null || v === undefined) return '0';
  return v ? '1' : '0';
}

type PlanRow = {
  plan_id: string;
  plan_json: string;
  schema_version: string;
};

async function main() {
  const client = new TursoPipelineClient();
  console.log('Migrating itinerary data from plan blobs to normalized tables...\n');

  // 1. Read all plans
  const plansResponse = await client.execute(
    'SELECT plan_id, plan_json, schema_version FROM plans_current'
  );
  const plans = rowsToObjects(plansResponse) as PlanRow[];
  console.log(`Found ${plans.length} plan(s) to migrate.\n`);

  for (const row of plans) {
    const planId = row.plan_id;
    console.log(`--- Migrating plan: ${planId} ---`);

    let plan: Record<string, unknown>;
    try {
      plan = JSON.parse(row.plan_json);
    } catch (e: any) {
      console.warn(`  ⚠️ Invalid JSON for ${planId}, skipping: ${e.message}`);
      continue;
    }

    const statements: string[] = [];

    // plan_metadata
    statements.push(
      `INSERT OR REPLACE INTO plan_metadata (plan_id, schema_version, active_destination, updated_at)
       VALUES (${sqlText(planId)}, ${sqlText(row.schema_version)}, ${sqlText(plan.active_destination as string)}, datetime('now'))`
    );

    const destinations = plan.destinations as Record<string, Record<string, unknown>> | undefined;
    if (!destinations) {
      console.log('  No destinations found, skipping.');
      continue;
    }

    for (const [destSlug, dest] of Object.entries(destinations)) {
      console.log(`  Destination: ${destSlug}`);

      // date_anchors
      const p1 = dest.process_1_date_anchor as Record<string, unknown> | undefined;
      if (p1) {
        const dates = p1.confirmed_dates as { start: string; end: string } | undefined;
        if (dates) {
          statements.push(
            `INSERT OR REPLACE INTO date_anchors (plan_id, destination, start_date, end_date, days, updated_at)
             VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(dates.start)}, ${sqlText(dates.end)}, ${sqlInt(p1.days as number)}, datetime('now'))`
          );
        }
      }

      // process_statuses
      const processIds = [
        'process_1_date_anchor',
        'process_2_destination',
        'process_3_4_packages',
        'process_3_transportation',
        'process_4_accommodation',
        'process_5_daily_itinerary',
      ];
      for (const pid of processIds) {
        const proc = dest[pid] as Record<string, unknown> | undefined;
        if (proc?.status) {
          statements.push(
            `INSERT OR REPLACE INTO process_statuses (plan_id, destination, process_id, status, updated_at)
             VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(pid)}, ${sqlText(proc.status as string)}, datetime('now'))`
          );
        }
      }

      // airport_transfers
      const p3 = dest.process_3_transportation as Record<string, unknown> | undefined;
      if (p3?.airport_transfers && typeof p3.airport_transfers === 'object') {
        const transfers = p3.airport_transfers as Record<string, unknown>;
        for (const dir of ['arrival', 'departure'] as const) {
          const segment = transfers[dir] as Record<string, unknown> | undefined;
          if (segment) {
            statements.push(
              `INSERT OR REPLACE INTO airport_transfers (plan_id, destination, direction, status, selected_json, candidates_json, updated_at)
               VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(dir)}, ${sqlText((segment.status as string) || 'planned')}, ${sqlText(segment.selected ? JSON.stringify(segment.selected) : null)}, ${sqlText(segment.candidates ? JSON.stringify(segment.candidates) : null)}, datetime('now'))`
            );
          }
        }

        // flights
        if (p3.flight && typeof p3.flight === 'object') {
          const flight = p3.flight as Record<string, unknown>;
          statements.push(
            `INSERT OR REPLACE INTO flights (plan_id, destination, populated_from, airline, airline_code, outbound_json, return_json, booked_date, updated_at)
             VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(p3.populated_from as string)}, ${sqlText(flight.airline as string)}, ${sqlText(flight.airline_code as string)}, ${sqlText(flight.outbound ? JSON.stringify(flight.outbound) : null)}, ${sqlText(flight.return ? JSON.stringify(flight.return) : null)}, ${sqlText(flight.booked_date as string)}, datetime('now'))`
          );
        }
      }

      // hotels
      const p4 = dest.process_4_accommodation as Record<string, unknown> | undefined;
      if (p4?.hotel && typeof p4.hotel === 'object') {
        const hotel = p4.hotel as Record<string, unknown>;
        statements.push(
          `INSERT OR REPLACE INTO hotels (plan_id, destination, populated_from, name, access_json, check_in, notes, updated_at)
           VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(p4.populated_from as string)}, ${sqlText(hotel.name as string)}, ${sqlText(hotel.access ? JSON.stringify(hotel.access) : null)}, ${sqlText(hotel.check_in as string)}, ${sqlText(hotel.notes as string)}, datetime('now'))`
        );
      }

      // cascade_dirty_flags
      const cascadeState = plan.cascade_state as Record<string, unknown> | undefined;
      const destFlags = (cascadeState?.destinations as Record<string, Record<string, unknown>>)?.[destSlug];
      if (destFlags) {
        for (const [pid, flag] of Object.entries(destFlags)) {
          const f = flag as { dirty: boolean; last_changed: string | null };
          statements.push(
            `INSERT OR REPLACE INTO cascade_dirty_flags (plan_id, destination, process_id, dirty, last_changed)
             VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlText(pid)}, ${sqlBool(f.dirty)}, ${sqlText(f.last_changed)})`
          );
        }
      }

      // itinerary_days + sessions + activities
      const p5 = dest.process_5_daily_itinerary as Record<string, unknown> | undefined;
      const days = p5?.days as Array<Record<string, unknown>> | undefined;
      if (days && Array.isArray(days)) {
        console.log(`    Days: ${days.length}`);
        let totalActivities = 0;

        for (const day of days) {
          const dayNumber = day.day_number as number;
          const weather = day.weather as Record<string, unknown> | undefined;

          statements.push(
            `INSERT OR REPLACE INTO itinerary_days (plan_id, destination, day_number, date, theme, day_type, status, weather_label, temp_low_c, temp_high_c, precipitation_pct, weather_code, weather_source_id, weather_sourced_at, updated_at)
             VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlInt(dayNumber)}, ${sqlText(day.date as string)}, ${sqlText(day.theme as string)}, ${sqlText(day.day_type as string)}, ${sqlText((day.status as string) || 'draft')}, ${sqlText(weather?.weather_label as string)}, ${sqlReal(weather?.temp_low_c as number)}, ${sqlReal(weather?.temp_high_c as number)}, ${sqlReal(weather?.precipitation_pct as number)}, ${sqlInt(weather?.weather_code as number)}, ${sqlText(weather?.source_id as string)}, ${sqlText(weather?.sourced_at as string)}, datetime('now'))`
          );

          for (const sessionType of ['morning', 'afternoon', 'evening'] as const) {
            const session = day[sessionType] as Record<string, unknown> | undefined;
            if (!session) continue;

            const timeRange = session.time_range as { start: string; end: string } | undefined;
            const meals = session.meals as string[] | undefined;

            statements.push(
              `INSERT OR REPLACE INTO itinerary_sessions (plan_id, destination, day_number, session_type, focus, transit_notes, booking_notes, meals_json, time_range_start, time_range_end, updated_at)
               VALUES (${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlInt(dayNumber)}, ${sqlText(sessionType)}, ${sqlText(session.focus as string)}, ${sqlText(session.transit_notes as string)}, ${sqlText(session.booking_notes as string)}, ${sqlText(meals ? JSON.stringify(meals) : null)}, ${sqlText(timeRange?.start)}, ${sqlText(timeRange?.end)}, datetime('now'))`
            );

            const activities = session.activities as Array<string | Record<string, unknown>> | undefined;
            if (activities) {
              for (let i = 0; i < activities.length; i++) {
                const act = activities[i];
                totalActivities++;

                if (typeof act === 'string') {
                  // Legacy string activity
                  const actId = `migrated_${planId}_${destSlug}_d${dayNumber}_${sessionType}_${i}`;
                  statements.push(
                    `INSERT OR REPLACE INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title, priority, updated_at)
                     VALUES (${sqlText(actId)}, ${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlInt(dayNumber)}, ${sqlText(sessionType)}, ${sqlInt(i)}, ${sqlText(act)}, 'want', datetime('now'))`
                  );
                } else {
                  const actId = (act.id as string) || `migrated_${planId}_${destSlug}_d${dayNumber}_${sessionType}_${i}`;
                  statements.push(
                    `INSERT OR REPLACE INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title, area, nearest_station, duration_min, booking_required, booking_url, booking_status, booking_ref, book_by, start_time, end_time, is_fixed_time, cost_estimate, tags_json, notes, priority, updated_at)
                     VALUES (${sqlText(actId)}, ${sqlText(planId)}, ${sqlText(destSlug)}, ${sqlInt(dayNumber)}, ${sqlText(sessionType)}, ${sqlInt(i)}, ${sqlText(act.title as string)}, ${sqlText(act.area as string)}, ${sqlText(act.nearest_station as string)}, ${sqlInt(act.duration_min as number)}, ${sqlBool(act.booking_required as boolean)}, ${sqlText(act.booking_url as string)}, ${sqlText(act.booking_status as string)}, ${sqlText(act.booking_ref as string)}, ${sqlText(act.book_by as string)}, ${sqlText(act.start_time as string)}, ${sqlText(act.end_time as string)}, ${sqlBool(act.is_fixed_time as boolean)}, ${sqlInt(act.cost_estimate as number)}, ${sqlText(act.tags ? JSON.stringify(act.tags) : null)}, ${sqlText(act.notes as string)}, ${sqlText((act.priority as string) || 'want')}, datetime('now'))`
                  );
                }
              }
            }
          }
        }
        console.log(`    Activities: ${totalActivities}`);
      }
    }

    // Execute in batch
    if (statements.length > 0) {
      console.log(`  Executing ${statements.length} statements...`);
      await client.executeMany(statements, 20);
      console.log(`  ✅ Done.`);
    } else {
      console.log('  No data to migrate.');
    }
  }

  console.log('\n✅ Migration complete.');
}

function rowsToObjects(response: any): Record<string, unknown>[] {
  const result = response?.results?.[0]?.response?.result;
  if (!result?.rows || !result?.cols) return [];

  const cols = result.cols.map((c: any) => c.name);
  return result.rows.map((row: any[]) => {
    const obj: Record<string, unknown> = {};
    for (let i = 0; i < cols.length; i++) {
      const cell = row[i];
      obj[cols[i]] = cell?.value ?? cell ?? null;
    }
    return obj;
  });
}

main().catch(console.error);
