// `travel swap-days <dayA> <dayB> [--dest <slug>]` — port of the
// swap-days subcommand in src/cli/commands/day.ts (StateManager.swapDays).
// NO CASCADE.
//
// The TS path swaps the *content* of two days while preserving each day's
// date / day_number / day_type:
//   - all four session objects (morning/noon/afternoon/evening), which carry
//     focus, transit/booking notes, time range, activities, meals, etc.
//   - the day-level `theme` (and `notes`, which has no DB column — no-op).
//
// In the normalized tables this means re-pointing, between the two
// day_numbers:
//   - days.theme / days.theme_zh (day-level fields)
//   - timesofday rows (session content)
//   - activities rows (per-session activity rows; PK is id, so just flip
//     day_number)
//   - session_meals / session_activities_zh child rows
// then touch the itinerary and emit a `days_swapped` plan_event + bump
// plans.version + record an operation_runs audit row.

use libsql::Connection;

#[derive(Default, Debug)]
struct ParsedArgs {
    day_a: i64,
    day_b: i64,
    dest: Option<String>,
}

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_args(args)?;

    let conn = crate::db::connect_write().await?;
    let destination = read_destination(&conn, &plan_id, &parsed.dest).await?;

    // Validate both days exist (mirror TS: throw if missing).
    require_day(&conn, &plan_id, &destination, parsed.day_a, "dayA").await?;
    require_day(&conn, &plan_id, &destination, parsed.day_b, "dayB").await?;

    println!("\n🔄 Swapping days:");
    println!("   Destination: {destination}");
    println!("   Day {} ↔ Day {}", parsed.day_a, parsed.day_b);

    swap_content(&conn, &plan_id, &destination, parsed.day_a, parsed.day_b).await?;

    // touchItinerary — bump updated_at on both day rows.
    touch_day(&conn, &plan_id, &destination, parsed.day_a).await?;
    touch_day(&conn, &plan_id, &destination, parsed.day_b).await?;

    let kv: Vec<(&str, String)> = vec![
        ("day_a", parsed.day_a.to_string()),
        ("day_b", parsed.day_b.to_string()),
    ];
    execute_event(
        &conn,
        &plan_id,
        &destination,
        "days_swapped",
        "swap-days",
        &format!("D{} ↔ D{}", parsed.day_a, parsed.day_b),
        &kv,
    )
    .await?;

    println!("✅ Days swapped successfully");
    Ok(())
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut p = ParsedArgs::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                p.dest = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --dest".to_string())?
                        .clone(),
                );
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => {
                positional.push(a.clone());
                i += 1;
            }
        }
    }
    if positional.len() < 2 {
        return Err(
            "Error: swap-days requires <dayA> <dayB>\nExample: swap-days 2 3".to_string(),
        );
    }
    p.day_a = positional[0]
        .parse::<i64>()
        .map_err(|_| "<dayA> must be a positive integer".to_string())?;
    p.day_b = positional[1]
        .parse::<i64>()
        .map_err(|_| "<dayB> must be a positive integer".to_string())?;
    if p.day_a < 1 || p.day_b < 1 {
        return Err("<dayA>/<dayB> must be positive integers".to_string());
    }
    if p.day_a == p.day_b {
        return Err("Error: dayA and dayB must be different".to_string());
    }
    Ok(p)
}

// Swap mutable content between two day_numbers using a temporary
// out-of-range day_number to avoid PK collisions during the flip.
async fn swap_content(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day_a: i64,
    day_b: i64,
) -> Result<(), String> {
    // 1. Day-level fields: theme / theme_zh.
    let (theme_a, theme_zh_a) = read_day_theme(conn, plan_id, destination, day_a).await?;
    let (theme_b, theme_zh_b) = read_day_theme(conn, plan_id, destination, day_b).await?;
    set_day_theme(conn, plan_id, destination, day_a, &theme_b, &theme_zh_b).await?;
    set_day_theme(conn, plan_id, destination, day_b, &theme_a, &theme_zh_a).await?;

    // 2. Session-scoped rows: timesofday, activities, session_meals,
    //    session_activities_zh. Re-point day_number a → TMP, b → a, TMP → b.
    //    A negative TMP day_number can never collide with a real day and
    //    is removed by the third step.
    const TMP: i64 = -999_999;
    for table in SESSION_TABLES {
        repoint(conn, table, plan_id, destination, day_a, TMP).await?;
        repoint(conn, table, plan_id, destination, day_b, day_a).await?;
        repoint(conn, table, plan_id, destination, TMP, day_b).await?;
    }
    Ok(())
}

const SESSION_TABLES: &[&str] = &[
    "timesofday",
    "activities",
    "session_meals",
    "session_activities_zh",
];

async fn repoint(
    conn: &Connection,
    table: &str,
    plan_id: &str,
    destination: &str,
    from_day: i64,
    to_day: i64,
) -> Result<(), String> {
    let sql = format!(
        "UPDATE {table} SET day_number = ?1 \
         WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4"
    );
    conn.execute(
        &sql,
        libsql::params![to_day, plan_id.to_string(), destination.to_string(), from_day],
    )
    .await
    .map_err(|e| format!("{table} re-point day_number failed: {e}"))?;
    Ok(())
}

async fn require_day(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    label: &str,
) -> Result<(), String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM days WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("days lookup failed: {e}"))?;
    if rows
        .next()
        .await
        .map_err(|e| format!("days row read failed: {e}"))?
        .is_none()
    {
        return Err(format!("Day {day} not found ({label})"));
    }
    Ok(())
}

async fn read_day_theme(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<(Option<String>, Option<String>), String> {
    let mut rows = conn
        .query(
            "SELECT theme, theme_zh FROM days \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("days theme query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("days theme row read failed: {e}"))?
    {
        let theme: Option<String> = row.get(0).ok();
        let theme_zh: Option<String> = row.get(1).ok();
        return Ok((theme, theme_zh));
    }
    Err(format!("Day {day} disappeared"))
}

async fn set_day_theme(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    theme: &Option<String>,
    theme_zh: &Option<String>,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET theme = ?1, theme_zh = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5",
        libsql::params![
            theme.clone(),
            theme_zh.clone(),
            plan_id.to_string(),
            destination.to_string(),
            day
        ],
    )
    .await
    .map_err(|e| format!("days theme UPDATE failed: {e}"))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// Shared helpers (mirrors set_activity.rs / set_day_theme.rs)
// ─────────────────────────────────────────────────────────────────

async fn read_destination(
    conn: &Connection,
    plan_id: &str,
    dest_opt: &Option<String>,
) -> Result<String, String> {
    if let Some(d) = dest_opt {
        return Ok(d.clone());
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
    Err(format!("plan_metadata row missing for plan_id={plan_id}"))
}

async fn touch_day(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET updated_at = ?1 WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4",
        libsql::params![now_db_datetime(), plan_id.to_string(), destination.to_string(), day],
    )
    .await
    .map_err(|e| format!("days touch UPDATE failed: {e}"))?;
    Ok(())
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

    let run_id = new_run_id();
    conn.execute(
        "INSERT INTO operation_runs \
            (run_id, plan_id, command_type, command_summary, status, \
             version_before, version_after, started_at, completed_at) \
         VALUES (?1, ?2, ?3, ?4, 'completed', ?5, ?6, ?7, ?7)",
        libsql::params![
            run_id,
            plan_id.to_string(),
            command_type.to_string(),
            command_summary.to_string(),
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
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}", p1, p2, p3, p4, p5)
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
    fn parse_args_ok() {
        let p = parse_args(&["2".to_string(), "3".to_string()]).unwrap();
        assert_eq!(p.day_a, 2);
        assert_eq!(p.day_b, 3);
    }

    #[test]
    fn parse_args_same_day_rejected() {
        assert!(parse_args(&["2".to_string(), "2".to_string()]).is_err());
    }

    #[test]
    fn parse_args_missing() {
        assert!(parse_args(&["2".to_string()]).is_err());
    }

    #[test]
    fn parse_args_with_dest() {
        let p = parse_args(&[
            "1".to_string(),
            "4".to_string(),
            "--dest".to_string(),
            "tokyo_2026".to_string(),
        ])
        .unwrap();
        assert_eq!(p.dest.as_deref(), Some("tokyo_2026"));
    }
}
