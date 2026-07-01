// `travel set-flight <outbound|return> [opts...]` — port of
// src/cli/commands/set-flight.ts. NO CASCADE.
//
// Mirrors the TS path:
//   1. UPSERT flight_legs for the given <direction>: sets leg-level
//      fields (flight_number, departure_*, arrival_*, flight_date).
//      Upsert (not plain UPDATE) so a booking-first plan with no
//      offer-cascade skeleton row still persists — a plain UPDATE
//      silently no-ops on a missing leg row.
//      If the user provided airline / airlineCode / booked_date,
//      those are SHARED flight-level fields — apply to BOTH legs
//      (outbound + return) because the TS in-memory shape
//      p3.flight.airline/airline_code/booked_date is shared.
//   2. INSERT plan_events (1 dest_process + 1 timeline flight_leg_updated
//      on process_3_transportation) with KV payload containing every
//      field the user provided (TS spread `...input` includes the
//      direction the user passed).
//   3. INSERT operation_runs + UPDATE plans.version.
//
// Verified no-cascade: cascade_dirty_flags UNCHANGED.

use libsql::Connection;
use travel_db::repo::flight_legs;

#[derive(Default, Debug)]
struct FlightInput {
    flight_number: Option<String>,
    airline: Option<String>,
    airline_code: Option<String>,
    departure_code: Option<String>,
    departure_terminal: Option<String>,
    departure_time: Option<String>,
    arrival_code: Option<String>,
    arrival_terminal: Option<String>,
    arrival_time: Option<String>,
    date: Option<String>,
    booked_date: Option<String>,
}

pub async fn run(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    if args.is_empty() {
        eprintln!("Error: set-flight requires <outbound|return>");
        eprintln!("Example: set-flight outbound --dest kyoto_2026 --flight SL396 --airline \"Thai Lion Air\" --from TPE --dep 09:00 --to KIX --arr 12:30");
        std::process::exit(1);
    }
    let direction = args[0].clone();
    if direction != "outbound" && direction != "return" {
        eprintln!("Error: set-flight requires <outbound|return>");
        std::process::exit(1);
    }
    let input = parse_args(&args[1..])?;

    let conn = match crate::db::connect_write().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: failed to connect to Turso (write tier): {e}");
            std::process::exit(1);
        }
    };

    let destination = match read_destination(&conn, &plan_id, &args[1..]).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // Print the user-facing header (mirrors TS).
    println!("\n✈️  Setting flight leg:");
    println!("   Destination: {destination}");
    println!("   Direction: {direction}");
    if let Some(f) = &input.flight_number {
        println!("   Flight: {f}");
    }
    let airline_label = if let Some(ac) = &input.airline_code {
        format!("{} ({ac})", input.airline.as_deref().unwrap_or(""))
    } else {
        input.airline.clone().unwrap_or_default()
    };
    if input.airline.is_some() || input.airline_code.is_some() {
        println!("   Airline: {airline_label}");
    }
    if input.departure_code.is_some() || input.departure_time.is_some() {
        let from_term = input
            .departure_terminal
            .as_deref()
            .map(|t| format!(" T{t}"))
            .unwrap_or_default();
        let from_time = input.departure_time.as_deref().unwrap_or("");
        let from_code = input.departure_code.as_deref().unwrap_or("");
        println!("   From: {from_code}{from_term} {from_time}");
    }
    if input.arrival_code.is_some() || input.arrival_time.is_some() {
        let to_term = input
            .arrival_terminal
            .as_deref()
            .map(|t| format!(" T{t}"))
            .unwrap_or_default();
        let to_time = input.arrival_time.as_deref().unwrap_or("");
        let to_code = input.arrival_code.as_deref().unwrap_or("");
        println!("   To: {to_code}{to_term} {to_time}");
    }
    if let Some(d) = &input.date {
        println!("   Date: {d}");
    }

    match execute(&conn, &plan_id, &destination, &direction, &input).await {
        Ok(_) => {
            println!("✅ Flight leg updated");
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: set-flight failed: {e}");
            std::process::exit(1);
        }
    }
}

fn parse_args(args: &[String]) -> Result<FlightInput, String> {
    let mut input = FlightInput::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let take = |args: &[String], i: usize, flag: &str| -> Result<String, String> {
            args.get(i + 1)
                .cloned()
                .ok_or_else(|| format!("missing value for {flag}"))
        };
        match a.as_str() {
            "--flight" => {
                input.flight_number = Some(take(args, i, "--flight")?);
                i += 2;
            }
            "--airline" => {
                input.airline = Some(take(args, i, "--airline")?);
                i += 2;
            }
            "--airline-code" => {
                input.airline_code = Some(take(args, i, "--airline-code")?);
                i += 2;
            }
            "--from" => {
                input.departure_code = Some(take(args, i, "--from")?);
                i += 2;
            }
            "--dep-terminal" => {
                input.departure_terminal = Some(take(args, i, "--dep-terminal")?);
                i += 2;
            }
            "--dep" => {
                input.departure_time = Some(take(args, i, "--dep")?);
                i += 2;
            }
            "--to" => {
                input.arrival_code = Some(take(args, i, "--to")?);
                i += 2;
            }
            "--arr-terminal" => {
                input.arrival_terminal = Some(take(args, i, "--arr-terminal")?);
                i += 2;
            }
            "--arr" => {
                input.arrival_time = Some(take(args, i, "--arr")?);
                i += 2;
            }
            "--date" => {
                input.date = Some(take(args, i, "--date")?);
                i += 2;
            }
            "--booked-date" => {
                input.booked_date = Some(take(args, i, "--booked-date")?);
                i += 2;
            }
            // `--dest <slug>` is advertised in the Example and consumed
            // separately by read_destination(); accept-and-skip it here so
            // the catch-all below doesn't reject the documented invocation.
            "--dest" => {
                let _ = take(args, i, "--dest")?;
                i += 2;
            }
            "--plan-id" => {
                // consumed by the top-level plan resolver; skip flag + value
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => i += 1,
        }
    }
    // Fail-loud HH:MM validation BEFORE any DB write (this Err bubbles to main →
    // stderr + non-zero exit, writing NOTHING). --dep/--arr are independent
    // clock times on different airports (a red-eye may arrive "before" it
    // departs in wall-clock terms), so no dep ≤ arr ordering is enforced — only
    // that each is a real HH:MM.
    if let Some(d) = &input.departure_time {
        crate::checks::validate_time_flag("--dep", d)?;
    }
    if let Some(a) = &input.arrival_time {
        crate::checks::validate_time_flag("--arr", a)?;
    }
    Ok(input)
}

async fn read_destination(
    conn: &Connection,
    plan_id: &str,
    args: &[String],
) -> Result<String, String> {
    let mut dest_override = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--dest" {
            dest_override = args.get(i + 1).cloned();
            break;
        }
        i += 1;
    }
    crate::cascade::common::resolve_active_destination(conn, plan_id, dest_override.as_deref())
        .await
}

async fn execute(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    direction: &str,
    input: &FlightInput,
) -> Result<i64, String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();

    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    // 1a. UPDATE the requested direction's leg with the leg-level
    //     fields. Leg-level fields are: flight_number,
    //     departure_code, departure_terminal, departure_time,
    //     arrival_code, arrival_terminal, arrival_time, date
    //     (flight_date in the DB).
    if has_leg_level_fields(input) {
        update_flight_leg(conn, plan_id, destination, direction, input, &now_db).await?;
    }

    // 1b. SHARED flight-level fields (airline, airline_code,
    //     booked_date) apply to BOTH legs (outbound + return) because
    //     the TS in-memory shape has them on p3.flight (not on
    //     p3.flight[direction]). The user might pass --airline while
    //     updating only the outbound leg — both legs' airline col
    //     must be updated.
    if input.airline.is_some() || input.airline_code.is_some() || input.booked_date.is_some() {
        for dir in ["outbound", "return"] {
            update_flight_shared(conn, plan_id, destination, dir, input, &now_db).await?;
        }
    }

    // 2. plan_events + plan_event_data.
    let dest_process_so = next_dest_process_sort_order(
        conn,
        plan_id,
        destination,
        "process_3_transportation",
    )
    .await?;
    let timeline_base = next_timeline_sort_order(conn, plan_id).await?;

    // The TS event data is `data: { ...input, direction }` — the
    // user-passed input spread plus the literal `direction` key
    // (always present, even if the user didn't provide all input
    // fields). The KV keys we emit, in order, exactly match what TS
    // spread emits (Object.entries order is insertion order in V8).
    let mut kv: Vec<(&str, String)> = Vec::new();
    if let Some(v) = &input.flight_number {
        kv.push(("flightNumber", v.clone()));
    }
    if let Some(v) = &input.airline {
        kv.push(("airline", v.clone()));
    }
    if let Some(v) = &input.airline_code {
        kv.push(("airlineCode", v.clone()));
    }
    if let Some(v) = &input.departure_code {
        kv.push(("departureCode", v.clone()));
    }
    if let Some(v) = &input.departure_time {
        kv.push(("departureTime", v.clone()));
    }
    if let Some(v) = &input.arrival_code {
        kv.push(("arrivalCode", v.clone()));
    }
    if let Some(v) = &input.arrival_time {
        kv.push(("arrivalTime", v.clone()));
    }
    if let Some(v) = &input.date {
        kv.push(("date", v.clone()));
    }
    // `direction` is always added (per TS spread, the literal key is
    // added to the data object even if input has no fields).
    kv.push(("direction", direction.to_string()));

    insert_event(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_3_transportation",
        dest_process_so,
        "flight_leg_updated",
        &now_iso,
    )
    .await?;
    insert_kv(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_3_transportation",
        dest_process_so,
        &kv,
    )
    .await?;

    let timeline_so = timeline_base;
    insert_event(
        conn,
        plan_id,
        "timeline",
        "",
        "process_3_transportation",
        timeline_so,
        "flight_leg_updated",
        &now_iso,
    )
    .await?;
    insert_kv(
        conn,
        plan_id,
        "timeline",
        "",
        "process_3_transportation",
        timeline_so,
        &kv,
    )
    .await?;

    // 3. operation_runs audit row + plans.version bump (shared audit-triad back half).
    let summary = format!(
        "{} {} {}",
        destination,
        direction,
        input.flight_number.as_deref().unwrap_or("")
    );
    crate::cascade::common::record_operation(
        conn,
        plan_id,
        "set-flight",
        &summary,
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    Ok(version_after)
}

fn has_leg_level_fields(input: &FlightInput) -> bool {
    input.flight_number.is_some()
        || input.departure_code.is_some()
        || input.departure_terminal.is_some()
        || input.departure_time.is_some()
        || input.arrival_code.is_some()
        || input.arrival_terminal.is_some()
        || input.arrival_time.is_some()
        || input.date.is_some()
}

async fn update_flight_leg(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    direction: &str,
    input: &FlightInput,
    now_db: &str,
) -> Result<(), String> {
    // (column, value) pairs for the leg-level fields the user provided. The DAL
    // appends updated_at and does the INSERT…ON CONFLICT upsert.
    let mut cols: Vec<(&str, String)> = Vec::new();
    if let Some(v) = &input.flight_number {
        cols.push(("flight_number", v.clone()));
    }
    if let Some(v) = &input.departure_code {
        cols.push(("departure_code", v.clone()));
    }
    if let Some(v) = &input.departure_terminal {
        cols.push(("departure_terminal", v.clone()));
    }
    if let Some(v) = &input.departure_time {
        cols.push(("departure_time", v.clone()));
    }
    if let Some(v) = &input.arrival_code {
        cols.push(("arrival_code", v.clone()));
    }
    if let Some(v) = &input.arrival_terminal {
        cols.push(("arrival_terminal", v.clone()));
    }
    if let Some(v) = &input.arrival_time {
        cols.push(("arrival_time", v.clone()));
    }
    if let Some(v) = &input.date {
        cols.push(("flight_date", v.clone()));
    }

    flight_legs::upsert_leg(conn, plan_id, destination, direction, &cols, now_db)
        .await
        .map_err(|e| format!("flight_legs upsert ({direction}) failed: {e}"))
}

async fn update_flight_shared(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    direction: &str,
    input: &FlightInput,
    now_db: &str,
) -> Result<(), String> {
    // Shared flight-level fields apply to BOTH legs (see execute step 1b).
    let mut cols: Vec<(&str, String)> = Vec::new();
    if let Some(v) = &input.airline {
        cols.push(("airline", v.clone()));
    }
    if let Some(v) = &input.airline_code {
        cols.push(("airline_code", v.clone()));
    }
    if let Some(v) = &input.booked_date {
        cols.push(("booked_date", v.clone()));
    }

    flight_legs::upsert_leg(conn, plan_id, destination, direction, &cols, now_db)
        .await
        .map_err(|e| format!("flight_legs shared upsert ({direction}) failed: {e}"))
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

#[allow(clippy::too_many_arguments)]
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
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{ms:03}Z"
    )
}

fn now_db_datetime() -> String {
    let (year, month, day, hour, min, sec, _) = now_civil();
    format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}"
    )
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
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d_ = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d_ as u32, hour, min, sec, ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_args_minimal() {
        let i = parse_args(&[]).unwrap();
        assert!(i.flight_number.is_none());
    }

    // ── HH:MM fail-loud validation (set-flight --dep / --arr) ─────────
    #[test]
    fn parse_args_accepts_valid_dep_arr() {
        let i = parse_args(&s(&["--dep", "09:00", "--arr", "12:30"])).unwrap();
        assert_eq!(i.departure_time.as_deref(), Some("09:00"));
        assert_eq!(i.arrival_time.as_deref(), Some("12:30"));
    }

    #[test]
    fn parse_args_rejects_bad_dep() {
        let err = parse_args(&s(&["--dep", "9am"])).unwrap_err();
        assert!(err.contains("--dep") && err.contains("\"9am\""), "got: {err}");
        assert!(err.contains("HH:MM"), "got: {err}");
    }

    #[test]
    fn parse_args_rejects_bad_arr() {
        let err = parse_args(&s(&["--arr", "24:00"])).unwrap_err();
        assert!(err.contains("--arr") && err.contains("\"24:00\""), "got: {err}");
    }

    #[test]
    fn parse_args_full_leg_and_shared() {
        let i = parse_args(&[
            "--flight".to_string(), "SL396".to_string(),
            "--airline".to_string(), "Thai Lion Air".to_string(),
            "--airline-code".to_string(), "SL".to_string(),
            "--from".to_string(), "TPE".to_string(),
            "--dep".to_string(), "09:00".to_string(),
            "--to".to_string(), "KIX".to_string(),
            "--arr".to_string(), "12:30".to_string(),
            "--date".to_string(), "2026-02-13".to_string(),
            "--booked-date".to_string(), "2026-01-15".to_string(),
        ]).unwrap();
        assert_eq!(i.flight_number.as_deref(), Some("SL396"));
        assert_eq!(i.airline.as_deref(), Some("Thai Lion Air"));
        assert_eq!(i.airline_code.as_deref(), Some("SL"));
        assert_eq!(i.departure_code.as_deref(), Some("TPE"));
        assert_eq!(i.departure_time.as_deref(), Some("09:00"));
        assert_eq!(i.arrival_code.as_deref(), Some("KIX"));
        assert_eq!(i.arrival_time.as_deref(), Some("12:30"));
        assert_eq!(i.date.as_deref(), Some("2026-02-13"));
        assert_eq!(i.booked_date.as_deref(), Some("2026-01-15"));
    }

    #[test]
    fn has_leg_level_fields_only_shared() {
        let i = parse_args(&["--airline".to_string(), "X".to_string()]).unwrap();
        assert!(!has_leg_level_fields(&i));
    }

    #[test]
    fn has_leg_level_fields_with_leg() {
        let i = parse_args(&["--flight".to_string(), "SL396".to_string()]).unwrap();
        assert!(has_leg_level_fields(&i));
    }
}
