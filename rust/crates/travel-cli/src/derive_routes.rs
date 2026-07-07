//! `travel derive-routes` — derive ai_recommended transit route segments from
//! consecutive same-day activity stations. Skips days with confirmed routes;
//! idempotent re-run when derived legs already match.

use libsql::Connection;
use std::collections::BTreeMap;
use travel_db::repo::route_segments::{self, SegmentWrite};

use crate::cascade::common::{
    insert_event, insert_kv_rows, next_dest_process_sort_order, next_timeline_sort_order,
    now_db_datetime, now_rfc3339, read_version, record_operation, resolve_active_destination,
};

#[derive(Default, Debug)]
struct Opts {
    day: Option<i64>,
    dest: Option<String>,
}

#[derive(Debug, Clone)]
struct ActivityRow {
    day_number: i64,
    station: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RouteLeg {
    from_place: String,
    to_place: String,
    mode: String,
    duration_min: Option<i64>,
    notes: Option<String>,
    start_time: Option<String>,
}

impl RouteLeg {
    fn to_write(&self) -> SegmentWrite {
        SegmentWrite {
            from_place: self.from_place.clone(),
            to_place: self.to_place.clone(),
            mode: self.mode.clone(),
            duration_min: self.duration_min,
            notes: self.notes.clone(),
            start_time: self.start_time.clone(),
        }
    }
}

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let opts = parse(args)?;
    let conn = crate::db::connect_write().await?;
    let destination =
        resolve_active_destination(&conn, &plan_id, opts.dest.as_deref()).await?;

    let activities = load_activities(&conn, &plan_id, &destination, opts.day).await?;
    let mut by_day: BTreeMap<i64, Vec<ActivityRow>> = BTreeMap::new();
    for row in activities {
        by_day.entry(row.day_number).or_default().push(row);
    }

    let mut days_scanned = 0i64;
    let mut days_written = 0i64;
    let mut inserted_total = 0i64;
    let mut deleted_total = 0i64;
    let mut skipped_confirmed = 0i64;

    for (day, acts) in &by_day {
        days_scanned += 1;

        let confirmed = count_confirmed(&conn, &plan_id, &destination, *day).await?;
        if confirmed > 0 {
            println!("Day {day}: skipped - confirmed route segment(s) exist");
            skipped_confirmed += 1;
            continue;
        }

        let desired = derive_legs(&conn, &destination, acts, *day).await?;
        let existing = load_existing_ai(&conn, &plan_id, &destination, *day).await?;

        if desired == existing {
            if !desired.is_empty() {
                println!(
                    "Day {day}: unchanged - existing ai_recommended route segment(s) already match"
                );
            }
            continue;
        }

        let deleted = route_segments::delete_ai_recommended_for_day(
            &conn,
            &plan_id,
            &destination,
            *day,
        )
        .await? as i64;
        for (i, leg) in desired.iter().enumerate() {
            route_segments::insert_segment(
                &conn,
                &plan_id,
                &destination,
                *day,
                i as i64,
                &leg.to_write(),
                "ai_recommended",
                &format!("[{i}]"),
            )
            .await?;
        }
        let now_db = now_db_datetime();
        route_segments::touch_day(&conn, &plan_id, &destination, *day, &now_db).await?;

        let inserted = desired.len() as i64;
        if deleted > 0 && inserted > 0 {
            println!(
                "Day {day}: replaced {deleted} stale ai_recommended route segment(s) with {inserted} derived segment(s)"
            );
        } else if inserted > 0 {
            println!("Day {day}: inserted {inserted} ai_recommended route segment(s)");
        } else if deleted > 0 {
            println!("Day {day}: cleared {deleted} stale ai_recommended route segment(s)");
        }

        inserted_total += inserted;
        deleted_total += deleted;
        days_written += 1;
    }

    if days_written > 0 {
        let version_before = read_version(&conn, &plan_id).await?;
        let version_after = version_before + 1;
        let now_iso = now_rfc3339();
        let now_db = now_db_datetime();

        let mut kv: Vec<(&str, String)> = vec![
            ("days_written", days_written.to_string()),
            ("inserted", inserted_total.to_string()),
            ("deleted", deleted_total.to_string()),
        ];
        if let Some(day) = opts.day {
            kv.push(("day_filter", day.to_string()));
        }

        let dest_process_so = next_dest_process_sort_order(
            &conn,
            &plan_id,
            &destination,
            "process_5_daily_itinerary",
        )
        .await?;
        let timeline_so = next_timeline_sort_order(&conn, &plan_id).await?;

        insert_event(
            &conn,
            &plan_id,
            "dest_process",
            &destination,
            "process_5_daily_itinerary",
            dest_process_so,
            "route_segments_bulk_updated",
            &now_iso,
            None,
            None,
        )
        .await?;
        insert_kv_rows(
            &conn,
            &plan_id,
            "dest_process",
            &destination,
            "process_5_daily_itinerary",
            dest_process_so,
            &kv,
        )
        .await?;

        insert_event(
            &conn,
            &plan_id,
            "timeline",
            "",
            "process_5_daily_itinerary",
            timeline_so,
            "route_segments_bulk_updated",
            &now_iso,
            None,
            None,
        )
        .await?;
        insert_kv_rows(
            &conn,
            &plan_id,
            "timeline",
            "",
            "process_5_daily_itinerary",
            timeline_so,
            &kv,
        )
        .await?;

        let summary = format!(
            "{days_written} day(s), {inserted_total} inserted, {deleted_total} deleted"
        );
        record_operation(
            &conn,
            &plan_id,
            "derive-routes",
            &summary,
            version_before,
            version_after,
            &now_db,
        )
        .await?;
    }

    println!(
        "Totals: days_scanned={days_scanned} days_written={days_written} inserted={inserted_total} deleted={deleted_total} skipped_confirmed={skipped_confirmed}"
    );
    if days_written > 0 {
        println!(
            "✅ Derived ai_recommended route segments ({inserted_total} inserted, {deleted_total} deleted across {days_written} day(s))"
        );
    } else {
        println!("✅ No route segment changes needed");
    }
    Ok(())
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut opts = Opts::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--day" => {
                let raw = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --day".to_string())?;
                let day: i64 = raw
                    .parse()
                    .map_err(|_| "--day must be a positive integer".to_string())?;
                if day < 1 {
                    return Err("--day must be a positive integer".to_string());
                }
                opts.day = Some(day);
                i += 2;
            }
            "--dest" => {
                opts.dest = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --dest".to_string())?
                        .clone(),
                );
                i += 2;
            }
            f if crate::plan_resolver::is_resolver_flag(f) => {
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            other => {
                return Err(format!("unexpected positional argument: {other}"));
            }
        }
    }
    Ok(opts)
}

async fn load_activities(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day_filter: Option<i64>,
) -> Result<Vec<ActivityRow>, String> {
    let day_param = day_filter.unwrap_or(-1);
    let mut rows = conn
        .query(
            "SELECT d.day_number, a.id, a.title, a.session_type, a.sort_order, \
             COALESCE(NULLIF(TRIM(a.nearest_station),''), NULLIF(TRIM(p.nearest_station),'')) AS station, \
             COALESCE(NULLIF(TRIM(a.area),''), NULLIF(TRIM(p.area),'')) AS area \
             FROM days d \
             JOIN activities a ON a.plan_id = d.plan_id AND a.destination = d.destination AND a.day_number = d.day_number \
             LEFT JOIN destination_pois p ON p.slug = a.destination AND p.poi_id = a.poi_id \
             WHERE d.plan_id = ?1 AND d.destination = ?2 AND (?3 < 0 OR d.day_number = ?3) \
             ORDER BY d.day_number, \
               CASE a.session_type WHEN 'morning' THEN 0 WHEN 'noon' THEN 1 WHEN 'afternoon' THEN 2 WHEN 'evening' THEN 3 ELSE 9 END, \
               a.sort_order, a.id",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day_param
            ],
        )
        .await
        .map_err(|e| format!("activities query failed: {e}"))?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let station: Option<String> = row.get(5).map_err(|e| e.to_string())?;
        out.push(ActivityRow {
            day_number: row.get(0).map_err(|e| e.to_string())?,
            station: station.filter(|s| !s.trim().is_empty()),
        });
    }
    Ok(out)
}

async fn count_confirmed(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM day_route_segments \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND source = 'confirmed'",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("confirmed count query failed: {e}"))?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        return row.get(0).map_err(|e| e.to_string());
    }
    Ok(0)
}

async fn derive_legs(
    conn: &Connection,
    destination: &str,
    acts: &[ActivityRow],
    day: i64,
) -> Result<Vec<RouteLeg>, String> {
    let mut desired = Vec::new();
    for pair in acts.windows(2) {
        let Some(from_station) = pair[0].station.as_deref() else {
            continue;
        };
        let Some(to_station) = pair[1].station.as_deref() else {
            continue;
        };
        if norm_station(from_station) == norm_station(to_station) {
            continue;
        }
        let from_place = from_station.to_string();
        let to_place = to_station.to_string();
        let mode = "transit".to_string();
        if let Err(reason) = crate::checks::guard_segment(&from_place, &to_place, &mode) {
            println!("Day {day}: skipped leg {from_place}->{to_place} ({reason})");
            continue;
        }
        let (duration_min, notes) =
            lookup_transit_metadata(conn, destination, from_station, to_station).await?;
        desired.push(RouteLeg {
            from_place,
            to_place,
            mode,
            duration_min,
            notes,
            start_time: None,
        });
    }
    Ok(desired)
}

async fn load_existing_ai(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<Vec<RouteLeg>, String> {
    let mut rows = conn
        .query(
            "SELECT from_place, to_place, mode, duration_min, notes, start_time \
             FROM day_route_segments \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND source = 'ai_recommended' \
             ORDER BY sort_order",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("existing ai_recommended query failed: {e}"))?;

    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let notes: Option<String> = row.get(4).map_err(|e| e.to_string())?;
        out.push(RouteLeg {
            from_place: row.get(0).map_err(|e| e.to_string())?,
            to_place: row.get(1).map_err(|e| e.to_string())?,
            mode: row.get(2).map_err(|e| e.to_string())?,
            duration_min: row.get(3).map_err(|e| e.to_string())?,
            notes: notes.filter(|n| !n.trim().is_empty()),
            start_time: row.get(5).map_err(|e| e.to_string())?,
        });
    }
    Ok(out)
}

use crate::transit_key::norm_station;

async fn lookup_transit_metadata(
    conn: &Connection,
    slug: &str,
    from: &str,
    to: &str,
) -> Result<(Option<i64>, Option<String>), String> {
    // Shared key math (transit_key) — the SAME normalization add-transit writes,
    // so a pair persisted via `add-transit` is always found here.
    let candidates = crate::transit_key::lookup_candidates(from, to);
    let mut rows = conn
        .query(
            "SELECT minutes, line, LOWER(pair_key) FROM destination_transit \
             WHERE slug = ?1 AND LOWER(pair_key) IN (?2, ?3, ?4, ?5)",
            libsql::params![
                slug.to_string(),
                candidates[0].clone(),
                candidates[1].clone(),
                candidates[2].clone(),
                candidates[3].clone(),
            ],
        )
        .await
        .map_err(|e| format!("destination_transit lookup failed: {e}"))?;

    let mut best: Option<(usize, Option<i64>, Option<String>)> = None;
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let pk: String = row.get(2).map_err(|e| e.to_string())?;
        let Some(idx) = candidates.iter().position(|k| k == &pk) else {
            continue;
        };
        if best.as_ref().is_none_or(|(best_idx, _, _)| idx < *best_idx) {
            let minutes: Option<i64> = row.get(0).map_err(|e| e.to_string())?;
            let line: Option<String> = row.get(1).map_err(|e| e.to_string())?;
            let notes = line.filter(|l| !l.trim().is_empty());
            best = Some((idx, minutes, notes));
        }
    }
    if let Some((_, minutes, notes)) = best {
        return Ok((minutes, notes));
    }
    Ok((None, None))
}