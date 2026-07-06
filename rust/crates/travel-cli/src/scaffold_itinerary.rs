// `travel scaffold-itinerary [--dest slug] [--force]` — port of
// src/cli/commands/scaffold-itinerary.ts + StateManager.scaffoldItinerary.
//
// Builds the P5 day skeleton from the destination's confirmed date anchor:
// one `days` row per date (arrival / full / departure) plus its four
// `timesofday` session rows (morning/noon/afternoon/evening). Arrival/
// departure transit notes are derived from the P3 flight legs.
//
// Mirrors the TS path:
//   1. Read date anchor (date_anchors.start_date/end_date) — fail loud if absent.
//   2. Refuse to overwrite an existing itinerary unless --force.
//   3. Read outbound/return flight legs for arrival/departure transit notes.
//   4. Replace days + sessions (delete stale, insert fresh skeleton).
//   5. process_5 status → 'researching', clear its dirty flag.
//   6. Emit `itinerary_scaffolded` plan_event, bump plans.version, audit row.

use libsql::Connection;

#[derive(Default, Debug)]
struct ParsedArgs {
    dest: Option<String>,
    force: bool,
}

struct DayPlan {
    date: String,
    day_number: i64,
    day_type: &'static str,
    morning_transit: Option<String>,
    evening_transit: Option<String>,
}

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_args(args)?;

    let conn = crate::db::connect_write().await?;
    let destination = read_destination(&conn, &plan_id, &parsed.dest).await?;

    // 1. Confirmed dates from the normalized date anchor.
    let (start, end) = read_date_anchor(&conn, &plan_id, &destination).await?;

    // 2. Refuse overwrite unless --force.
    let existing = count_existing_days(&conn, &plan_id, &destination).await?;
    if existing > 0 && !parsed.force {
        return Err("Itinerary already has days. Use --force to overwrite.".to_string());
    }

    // 3. Flight legs → arrival/departure transit notes.
    let arrival_note = read_arrival_note(&conn, &plan_id, &destination).await?;
    let departure_note = read_departure_note(&conn, &plan_id, &destination).await?;

    // Build the day plan from the inclusive date range.
    let dates = enumerate_dates(&start, &end)?;
    if dates.is_empty() {
        return Err(format!(
            "date anchor range produced no days (start={start}, end={end})"
        ));
    }
    let last_idx = dates.len() - 1;
    let mut days: Vec<DayPlan> = Vec::with_capacity(dates.len());
    for (i, date) in dates.into_iter().enumerate() {
        let is_first = i == 0;
        let is_last = i == last_idx;
        let day_type = if is_first {
            "arrival"
        } else if is_last {
            "departure"
        } else {
            "full"
        };
        days.push(DayPlan {
            date,
            day_number: (i as i64) + 1,
            day_type,
            morning_transit: if is_first { arrival_note.clone() } else { None },
            evening_transit: if is_last { departure_note.clone() } else { None },
        });
    }

    println!("\n📅 Scaffolding itinerary for {destination}:");
    println!("   Dates: {start} → {end}");
    println!("   Days: {}", days.len());
    if let Some(n) = &arrival_note {
        println!("   Arrival: {n}");
    }
    if let Some(n) = &departure_note {
        println!("   Departure: {n}");
    }
    println!();
    for d in &days {
        let icon = match d.day_type {
            "arrival" => "✈️",
            "departure" => "🛫",
            _ => "📍",
        };
        println!("   {icon} Day {}: {} ({})", d.day_number, d.date, d.day_type);
    }

    // 4. Replace day skeleton (delete stale child + day rows, insert fresh).
    write_skeleton(&conn, &plan_id, &destination, &days).await?;

    // 5. process_5 → researching; clear its dirty flag.
    set_process_status(
        &conn,
        &plan_id,
        &destination,
        "process_5_daily_itinerary",
        "researching",
    )
    .await?;
    clear_dirty(&conn, &plan_id, &destination, "process_5_daily_itinerary").await?;

    // 6. Emit event + version bump + audit.
    let day_types: Vec<String> = days.iter().map(|d| d.day_type.to_string()).collect();
    let kv: Vec<(&str, String)> = vec![
        ("days_count", days.len().to_string()),
        ("day_types", day_types.join(", ")),
    ];
    execute_event(
        &conn,
        &plan_id,
        &destination,
        "itinerary_scaffolded",
        "scaffold-itinerary",
        &format!("{destination} {} days", days.len()),
        &kv,
    )
    .await?;

    println!("\n✅ Itinerary scaffolded");
    println!("\nNext action: Review day structure, then populate activities with /p5-itinerary");
    Ok(())
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut p = ParsedArgs::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dest" => {
                p.dest = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --dest".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--force" => {
                p.force = true;
                i += 1;
            }
            "--plan-id" => {
                // consumed by the top-level plan resolver; skip flag + value
                i += 2;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(p)
}

async fn read_date_anchor(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<(String, String), String> {
    let mut rows = conn
        .query(
            "SELECT start_date, end_date FROM date_anchors \
             WHERE plan_id = ?1 AND destination = ?2",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("date_anchors query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("date_anchors row read failed: {e}"))?
    {
        let start: String = row.get(0).map_err(|e| format!("start_date read failed: {e}"))?;
        let end: String = row.get(1).map_err(|e| format!("end_date read failed: {e}"))?;
        if start.is_empty() || end.is_empty() {
            return Err(
                "No confirmed dates found in date anchor. Set dates first: set-dates <start> <end>"
                    .to_string(),
            );
        }
        return Ok((start, end));
    }
    Err(
        "No confirmed dates found in date anchor. Set dates first: set-dates <start> <end>"
            .to_string(),
    )
}

async fn count_existing_days(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM days WHERE plan_id = ?1 AND destination = ?2",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("days count query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("days count row read failed: {e}"))?
    {
        return Ok(row.get(0).unwrap_or(0));
    }
    Ok(0)
}

// Arrival note from outbound leg_order 0: "Arrive <code|airport> <time> → hotel".
async fn read_arrival_note(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT arrival_code, arrival_airport, arrival_time FROM flight_legs \
             WHERE plan_id = ?1 AND destination = ?2 AND direction = 'outbound' \
             ORDER BY leg_order DESC LIMIT 1",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("flight_legs outbound query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("flight_legs outbound row read failed: {e}"))?
    {
        let code: Option<String> = opt_text(&row, 0);
        let airport: Option<String> = opt_text(&row, 1);
        let time: Option<String> = opt_text(&row, 2);
        if let Some(t) = time {
            let place = code.or(airport).unwrap_or_else(|| "airport".to_string());
            return Ok(Some(format!("Arrive {place} {t} → hotel")));
        }
    }
    Ok(None)
}

// Departure note from return leg_order 0:
// "Hotel → <code|airport> for <time> flight".
async fn read_departure_note(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT departure_code, departure_airport, departure_time FROM flight_legs \
             WHERE plan_id = ?1 AND destination = ?2 AND direction = 'return' \
             ORDER BY leg_order ASC LIMIT 1",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("flight_legs return query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("flight_legs return row read failed: {e}"))?
    {
        let code: Option<String> = opt_text(&row, 0);
        let airport: Option<String> = opt_text(&row, 1);
        let time: Option<String> = opt_text(&row, 2);
        if let Some(t) = time {
            let place = code.or(airport).unwrap_or_else(|| "airport".to_string());
            return Ok(Some(format!("Hotel → {place} for {t} flight")));
        }
    }
    Ok(None)
}

fn opt_text(row: &libsql::Row, idx: i32) -> Option<String> {
    match row.get_value(idx).ok()? {
        libsql::Value::Text(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

// Delete the existing day skeleton (+ session/activity child rows) then
// insert the fresh one. Mirrors syncNormalizedTables' delete-then-insert
// for the itinerary tables, scoped to this destination. Domain writes live in
// `repo::itinerary::replace_skeleton`; the single `now` is captured here and
// used for ALL day + timesofday inserts.
async fn write_skeleton(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    days: &[DayPlan],
) -> Result<(), String> {
    let now = now_db_datetime();
    let skeleton: Vec<travel_db::repo::itinerary::SkeletonDay> = days
        .iter()
        .map(|d| travel_db::repo::itinerary::SkeletonDay {
            day_number: d.day_number,
            date: d.date.clone(),
            day_type: d.day_type.to_string(),
            morning_transit: d.morning_transit.clone(),
            evening_transit: d.evening_transit.clone(),
        })
        .collect();
    travel_db::repo::itinerary::replace_skeleton(conn, plan_id, destination, &skeleton, &now).await
}

// Enumerate inclusive YYYY-MM-DD dates from start to end.
fn enumerate_dates(start: &str, end: &str) -> Result<Vec<String>, String> {
    let s = parse_ymd(start)?;
    let e = parse_ymd(end)?;
    let start_n = ymd_to_days(s.0, s.1, s.2);
    let end_n = ymd_to_days(e.0, e.1, e.2);
    if end_n < start_n {
        return Err(format!("date anchor end ({end}) precedes start ({start})"));
    }
    let mut out = Vec::new();
    let mut n = start_n;
    while n <= end_n {
        let (y, m, d) = days_to_ymd(n);
        out.push(format!("{y:04}-{m:02}-{d:02}"));
        n += 1;
    }
    Ok(out)
}

fn parse_ymd(s: &str) -> Result<(i64, u32, u32), String> {
    // Accept "YYYY-MM-DD" or "YYYY-MM-DDT..." (ISO with time) — take date part.
    let date_part = s.split('T').next().unwrap_or(s);
    let parts: Vec<&str> = date_part.split('-').collect();
    if parts.len() != 3 {
        return Err(format!("invalid date in anchor: {s}"));
    }
    let y: i64 = parts[0].parse().map_err(|_| format!("invalid year in {s}"))?;
    let m: u32 = parts[1].parse().map_err(|_| format!("invalid month in {s}"))?;
    let d: u32 = parts[2].parse().map_err(|_| format!("invalid day in {s}"))?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(format!("out-of-range date in anchor: {s}"));
    }
    Ok((y, m, d))
}

// Days since civil epoch (Howard Hinnant's algorithm).
fn ymd_to_days(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let mp = if m > 2 { m as i64 - 3 } else { m as i64 + 9 };
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_to_ymd(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

async fn set_process_status(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    process_id: &str,
    status: &str,
) -> Result<(), String> {
    travel_db::repo::itinerary::upsert_process_status_replace(
        conn,
        plan_id,
        destination,
        process_id,
        status,
        &now_db_datetime(),
    )
    .await
}

async fn clear_dirty(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    process_id: &str,
) -> Result<(), String> {
    travel_db::repo::itinerary::clear_dirty(
        conn,
        plan_id,
        destination,
        process_id,
        &now_db_datetime(),
    )
    .await
}

// ─────────────────────────────────────────────────────────────────
// Event / version / audit helpers (mirror set_day_theme.rs)
// ─────────────────────────────────────────────────────────────────

async fn read_destination(
    conn: &Connection,
    plan_id: &str,
    dest_opt: &Option<String>,
) -> Result<String, String> {
    crate::cascade::common::resolve_active_destination(conn, plan_id, dest_opt.as_deref()).await
}

async fn execute_event(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    event_name: &str,
    command_type: &str,
    command_summary: &str,
    kv: &[(&str, String)],
) -> Result<(), String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    let dest_process_so =
        next_dest_process_sort_order(conn, plan_id, destination, "process_5_daily_itinerary")
            .await?;
    let timeline_so = next_timeline_sort_order(conn, plan_id).await?;

    insert_event(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_5_daily_itinerary",
        dest_process_so,
        event_name,
        &now_iso,
    )
    .await?;
    insert_kv(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_5_daily_itinerary",
        dest_process_so,
        kv,
    )
    .await?;
    insert_event(
        conn,
        plan_id,
        "timeline",
        "",
        "process_5_daily_itinerary",
        timeline_so,
        event_name,
        &now_iso,
    )
    .await?;
    insert_kv(
        conn,
        plan_id,
        "timeline",
        "",
        "process_5_daily_itinerary",
        timeline_so,
        kv,
    )
    .await?;

    crate::cascade::common::record_operation(
        conn,
        plan_id,
        command_type,
        command_summary,
        version_before,
        version_after,
        &now_db,
    )
    .await?;
    Ok(())
}

async fn read_version(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT version FROM plans WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plans query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plans row read failed: {e}"))?
    {
        let v: i64 = row
            .get(0)
            .map_err(|e| format!("version col read failed: {e}"))?;
        return Ok(v);
    }
    Err(format!("plans row missing for plan_id={plan_id}"))
}

async fn next_dest_process_sort_order(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'dest_process' \
               AND destination = ?2 AND process_id = ?3",
            libsql::params![plan_id.to_string(), dest.to_string(), process_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_events MAX query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_events MAX row read failed: {e}"))?
    {
        let m: i64 = row
            .get(0)
            .map_err(|e| format!("plan_events MAX col read failed: {e}"))?;
        return Ok(m + 1);
    }
    Ok(0)
}

async fn next_timeline_sort_order(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'timeline'",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_events MAX(timeline) query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_events MAX(timeline) row read failed: {e}"))?
    {
        let m: i64 = row
            .get(0)
            .map_err(|e| format!("plan_events MAX(timeline) col read failed: {e}"))?;
        return Ok(m + 1);
    }
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
async fn insert_event(
    conn: &Connection,
    plan_id: &str,
    scope: &str,
    destination: &str,
    process_id: &str,
    sort_order: i64,
    event: &str,
    event_at: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_events \
         WHERE plan_id = ?1 AND scope = ?2 AND destination = ?3 \
           AND process_id = ?4 AND sort_order = ?5",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order],
    )
    .await
    .map_err(|e| format!("plan_events DELETE failed: {e}"))?;
    conn.execute(
        "INSERT INTO plan_events \
            (plan_id, scope, destination, process_id, sort_order, \
             event, event_at, from_state, to_state) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order, event.to_string(), event_at.to_string()],
    )
    .await
    .map_err(|e| format!("plan_events INSERT failed: {e}"))?;
    Ok(())
}

async fn insert_kv(
    conn: &Connection,
    plan_id: &str,
    scope: &str,
    destination: &str,
    process_id: &str,
    sort_order: i64,
    kv: &[(&str, String)],
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_event_data \
         WHERE plan_id = ?1 AND scope = ?2 AND destination = ?3 \
           AND process_id = ?4 AND sort_order = ?5",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order],
    )
    .await
    .map_err(|e| format!("plan_event_data DELETE failed: {e}"))?;
    for (k, v) in kv {
        conn.execute(
            "INSERT INTO plan_event_data \
                (plan_id, scope, destination, process_id, sort_order, key, value) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order, k.to_string(), v.clone()],
        )
        .await
        .map_err(|e| format!("plan_event_data INSERT failed: {e}"))?;
    }
    Ok(())
}

fn now_rfc3339() -> String {
    let (year, month, day, hour, min, sec, ms) = now_civil();
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{ms:03}Z")
}

fn now_db_datetime() -> String {
    let (year, month, day, hour, min, sec, _) = now_civil();
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}")
}

fn now_civil() -> (i32, u32, u32, u32, u32, u32, u32) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let ms = d.subsec_millis();
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400) as u32;
    let hour = sod / 3600;
    let min = (sod % 3600) / 60;
    let sec = sod % 60;
    let (y, m, d_) = days_to_ymd(days);
    (y as i32, m, d_, hour, min, sec, ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerate_inclusive_range() {
        let d = enumerate_dates("2026-02-13", "2026-02-17").unwrap();
        assert_eq!(d, vec![
            "2026-02-13", "2026-02-14", "2026-02-15", "2026-02-16", "2026-02-17"
        ]);
    }

    #[test]
    fn enumerate_single_day() {
        let d = enumerate_dates("2026-03-01", "2026-03-01").unwrap();
        assert_eq!(d, vec!["2026-03-01"]);
    }

    #[test]
    fn enumerate_month_boundary() {
        let d = enumerate_dates("2026-02-27", "2026-03-02").unwrap();
        assert_eq!(d, vec![
            "2026-02-27", "2026-02-28", "2026-03-01", "2026-03-02"
        ]);
    }

    #[test]
    fn enumerate_leap_day() {
        let d = enumerate_dates("2024-02-28", "2024-03-01").unwrap();
        assert_eq!(d, vec!["2024-02-28", "2024-02-29", "2024-03-01"]);
    }

    #[test]
    fn enumerate_iso_with_time() {
        let d = enumerate_dates("2026-02-13T00:00:00Z", "2026-02-14").unwrap();
        assert_eq!(d, vec!["2026-02-13", "2026-02-14"]);
    }

    #[test]
    fn enumerate_reversed_errors() {
        assert!(enumerate_dates("2026-02-17", "2026-02-13").is_err());
    }

    #[test]
    fn parse_args_force() {
        let p = parse_args(&["--force".to_string()]).unwrap();
        assert!(p.force);
        assert!(p.dest.is_none());
    }

    #[test]
    fn parse_args_dest() {
        let p = parse_args(&["--dest".to_string(), "kyoto_2026".to_string()]).unwrap();
        assert_eq!(p.dest.as_deref(), Some("kyoto_2026"));
    }
}
