// `travel set-flight <outbound|return> [opts...]` — port of
// src/cli/commands/set-flight.ts. NO CASCADE.
//
// Mirrors the TS path:
//   1. UPDATE flight_legs for the given <direction>: sets leg-level
//      fields (flight_number, departure_*, arrival_*, flight_date).
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
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => i += 1,
        }
    }
    Ok(input)
}

async fn read_destination(
    conn: &Connection,
    plan_id: &str,
    args: &[String],
) -> Result<String, String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--dest"
            && let Some(d) = args.get(i + 1)
        {
            return Ok(d.clone());
        }
        i += 1;
    }
    let mut rows = conn
        .query(
            "SELECT active_destination FROM plan_metadata WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_metadata query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_metadata row read failed: {e}"))?
    {
        let dest: String = row
            .get(0)
            .map_err(|e| format!("active_destination col read failed: {e}"))?;
        if dest.is_empty() {
            return Err("plan_metadata.active_destination is empty".to_string());
        }
        return Ok(dest);
    }
    Err(format!(
        "plan_metadata row missing for plan_id={plan_id}"
    ))
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

    // 3. operation_runs + plans.version.
    let run_id = new_run_id();
    let summary = format!(
        "{} {} {}",
        destination,
        direction,
        input.flight_number.as_deref().unwrap_or("")
    );
    conn.execute(
        "INSERT INTO operation_runs \
            (run_id, plan_id, command_type, command_summary, status, \
             version_before, version_after, started_at, completed_at) \
         VALUES (?1, ?2, 'set-flight', ?3, 'completed', ?4, ?5, ?6, ?6)",
        libsql::params![
            run_id,
            plan_id.to_string(),
            summary,
            version_before,
            version_after,
            now_db.clone()
        ],
    )
    .await
    .map_err(|e| format!("operation_runs INSERT failed: {e}"))?;

    conn.execute(
        "UPDATE plans SET version = ?1, updated_at = ?2 WHERE plan_id = ?3",
        libsql::params![version_after, now_db, plan_id.to_string()],
    )
    .await
    .map_err(|e| format!("plans UPDATE failed: {e}"))?;

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
    let mut sets: Vec<String> = Vec::new();
    let mut vals: Vec<String> = Vec::new();
    let mut p: i32 = 1;
    if let Some(v) = &input.flight_number {
        sets.push(format!("flight_number = ?{p}"));
        vals.push(v.clone());
        p += 1;
    }
    if let Some(v) = &input.departure_code {
        sets.push(format!("departure_code = ?{p}"));
        vals.push(v.clone());
        p += 1;
    }
    if let Some(v) = &input.departure_terminal {
        sets.push(format!("departure_terminal = ?{p}"));
        vals.push(v.clone());
        p += 1;
    }
    if let Some(v) = &input.departure_time {
        sets.push(format!("departure_time = ?{p}"));
        vals.push(v.clone());
        p += 1;
    }
    if let Some(v) = &input.arrival_code {
        sets.push(format!("arrival_code = ?{p}"));
        vals.push(v.clone());
        p += 1;
    }
    if let Some(v) = &input.arrival_terminal {
        sets.push(format!("arrival_terminal = ?{p}"));
        vals.push(v.clone());
        p += 1;
    }
    if let Some(v) = &input.arrival_time {
        sets.push(format!("arrival_time = ?{p}"));
        vals.push(v.clone());
        p += 1;
    }
    if let Some(v) = &input.date {
        sets.push(format!("flight_date = ?{p}"));
        vals.push(v.clone());
        p += 1;
    }
    sets.push(format!("updated_at = ?{p}"));
    vals.push(now_db.to_string());
    p += 1;
    let sql = format!(
        "UPDATE flight_legs SET {} \
         WHERE plan_id = ?{} AND destination = ?{} AND direction = ?{} AND leg_order = 0",
        sets.join(", "),
        p,
        p + 1,
        p + 2
    );
    vals.push(plan_id.to_string());
    vals.push(destination.to_string());
    vals.push(direction.to_string());
    let params: Vec<libsql::Value> = vals
        .iter()
        .map(|v| libsql::Value::Text(v.clone()))
        .collect();
    conn.execute(&sql, params)
        .await
        .map_err(|e| format!("flight_legs UPDATE ({direction}) failed: {e}"))?;
    Ok(())
}

async fn update_flight_shared(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    direction: &str,
    input: &FlightInput,
    now_db: &str,
) -> Result<(), String> {
    let mut sets: Vec<String> = Vec::new();
    let mut vals: Vec<String> = Vec::new();
    let mut p: i32 = 1;
    if let Some(v) = &input.airline {
        sets.push(format!("airline = ?{p}"));
        vals.push(v.clone());
        p += 1;
    }
    if let Some(v) = &input.airline_code {
        sets.push(format!("airline_code = ?{p}"));
        vals.push(v.clone());
        p += 1;
    }
    if let Some(v) = &input.booked_date {
        sets.push(format!("booked_date = ?{p}"));
        vals.push(v.clone());
        p += 1;
    }
    sets.push(format!("updated_at = ?{p}"));
    vals.push(now_db.to_string());
    p += 1;
    let sql = format!(
        "UPDATE flight_legs SET {} \
         WHERE plan_id = ?{} AND destination = ?{} AND direction = ?{} AND leg_order = 0",
        sets.join(", "),
        p,
        p + 1,
        p + 2
    );
    vals.push(plan_id.to_string());
    vals.push(destination.to_string());
    vals.push(direction.to_string());
    let params: Vec<libsql::Value> = vals
        .iter()
        .map(|v| libsql::Value::Text(v.clone()))
        .collect();
    conn.execute(&sql, params)
        .await
        .map_err(|e| format!("flight_legs shared UPDATE ({direction}) failed: {e}"))?;
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

fn new_run_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        ^ (n as u128);
    let p1 = (nanos & 0xFFFF_FFFF) as u32;
    let p2 = ((nanos >> 32) & 0xFFFF) as u16;
    let p3 = ((nanos >> 48) & 0x0FFF) as u16;
    let p4 = 0x8000 | (((nanos >> 60) & 0x3FFF) as u16);
    let p5 = (nanos as u64) ^ 0xDEAD_BEEF_CAFE_F00D;
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        p1, p2, p3, p4, p5
    )
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

    #[test]
    fn parse_args_minimal() {
        let i = parse_args(&[]).unwrap();
        assert!(i.flight_number.is_none());
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
