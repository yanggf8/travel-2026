// `travel set-day-theme <day> [theme] [--zh "<chinese_title>"]` — port of
// src/cli/commands/day.ts set-day-theme. NO CASCADE.
//
// Mirrors the TS path:
//   1. UPDATE days.theme (and theme_zh if --zh)
//   2. UPDATE plans.version + 1
//   3. INSERT operation_runs (audit)
//   4. INSERT plan_events (1 dest_process + 1 timeline; both
//      itinerary_day_theme_set, process_5_daily_itinerary) with
//      plan_event_data KV (day_number, theme, theme_zh)
//
// Verified to be no-cascade: cascade_dirty_flags is UNCHANGED.

use libsql::Connection;
use travel_db::repo::days;

/// CLI entry: `travel set-day-theme <day> [theme] [--zh "<zh>"] [--dest <slug>]`.
pub async fn run(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    // 1. Parse args: set-day-theme <day> [theme] [--zh <zh>] [--dest <slug>]
    let parsed = parse_args(args)?;

    // 2. Resolve active destination from plan_metadata (or use --dest).
    let conn = match crate::db::connect_write().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: failed to connect to Turso (write tier): {e}");
            std::process::exit(1);
        }
    };

    let destination = match read_destination(&conn, &plan_id, parsed.dest.as_deref()).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // 3. Apply the mutation. Touch the day (sets updated_at) and emit
    //    the event.
    match execute(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        parsed.theme.as_deref(),
        parsed.theme_zh.as_deref(),
    )
    .await
    {
        Ok(_) => {
            println!("✅ Day theme updated");
            // Dashboard renders ZH by default: an English-only theme edit won't
            // show until theme_zh is also updated. Warn if a stale theme_zh exists.
            if parsed.theme.is_some() && parsed.theme_zh.is_none() {
                let zh = read_theme_zh(&conn, &plan_id, &destination, parsed.day)
                    .await
                    .unwrap_or_default();
                if !zh.trim().is_empty() {
                    crate::checks::warn_zh_stale(
                        "theme",
                        &format!(
                            "set-day-theme {} --zh \"<chinese title>\" --dest {destination}",
                            parsed.day
                        ),
                    );
                }
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: set-day-theme failed: {e}");
            std::process::exit(1);
        }
    }
}

#[derive(Default, Debug)]
struct ParsedArgs {
    day: i64,
    theme: Option<String>,
    theme_zh: Option<String>,
    dest: Option<String>,
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    if args.is_empty() {
        return Err(usage_error());
    }
    let mut p = ParsedArgs::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--zh" => {
                p.theme_zh = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --zh".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--dest" => {
                p.dest = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --dest".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--plan-id" => {
                // consumed by the top-level plan resolver; skip flag + value
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
    if positional.is_empty() {
        return Err(usage_error());
    }
    p.day = positional[0]
        .parse::<i64>()
        .map_err(|_| "Error: <day> must be a positive integer".to_string())?;
    if p.day < 1 {
        return Err("Error: <day> must be a positive integer".to_string());
    }
    if positional.len() >= 2 {
        p.theme = Some(positional[1].clone());
    }
    // The TS path uses validateIsoDate on theme_zh when present? No —
    // theme_zh is free text. Just pass it through.
    Ok(p)
}

fn usage_error() -> String {
    "Error: set-day-theme requires <day> and --zh \"<title>\"
Example: set-day-theme 1 --zh \"抵達京都\"
Example: set-day-theme 2 \"Kinkaku-ji full day\" --zh \"金閣寺・伏見稲荷\""
        .to_string()
}

async fn read_destination(
    conn: &Connection,
    plan_id: &str,
    dest_opt: Option<&str>,
) -> Result<String, String> {
    crate::cascade::common::resolve_active_destination(conn, plan_id, dest_opt).await
}

/// Read the current `days.theme_zh` (empty string if NULL/missing). Used only to
/// decide whether to warn that a default-ZH dashboard still shows the old theme.
async fn read_theme_zh(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<String, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(theme_zh, '') FROM days \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        return Ok(row.get::<String>(0).unwrap_or_default());
    }
    Ok(String::new())
}

async fn execute(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    theme: Option<&str>,
    theme_zh: Option<&str>,
) -> Result<i64, String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();

    // 0. The target `days` row MUST pre-exist (it's created by the itinerary
    //    scaffold, not this command). A plain UPDATE would silently no-op on a
    //    missing day and still write a `completed` audit — fail loud instead so
    //    a ✅/completed audit always implies a row actually changed.
    if !days::exists(conn, plan_id, destination, day).await? {
        return Err(format!(
            "no days row for plan={plan_id} destination={destination} day={day}; \
             scaffold the itinerary first (travel scaffold-itinerary)"
        ));
    }

    // 1. Read current plans.version (fail loud if missing).
    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    // 2. UPDATE days.theme / theme_zh / updated_at via the DAL (only the target
    //    day's row is touched). theme+zh / theme / theme_zh / touch-only are the
    //    four variants — set_theme picks by which fields are Some; updated_at is
    //    always bumped (touchItinerary).
    days::set_theme(conn, plan_id, destination, day, theme, theme_zh, &now_db).await?;

    // 3. INSERT 2 plan_events + 6 plan_event_data rows. Sort-order
    //    assignment: per-bucket max+1 (dest_process) and global max+1
    //    (timeline) — same rule as the set-dates cascade.
    let dest_process_so = next_dest_process_sort_order(
        conn,
        plan_id,
        destination,
        "process_5_daily_itinerary",
    )
    .await?;
    let timeline_base = next_timeline_sort_order(conn, plan_id).await?;

    // dest_process event
    insert_event(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_5_daily_itinerary",
        dest_process_so,
        "itinerary_day_theme_set",
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
        &[
            ("day_number", day.to_string()),
            ("theme", theme.unwrap_or("null").to_string()),
            ("theme_zh", theme_zh.unwrap_or("null").to_string()),
        ],
    )
    .await?;

    // timeline event
    let timeline_so = timeline_base;
    insert_event(
        conn,
        plan_id,
        "timeline",
        "",
        "process_5_daily_itinerary",
        timeline_so,
        "itinerary_day_theme_set",
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
        &[
            ("day_number", day.to_string()),
            ("theme", theme.unwrap_or("null").to_string()),
            ("theme_zh", theme_zh.unwrap_or("null").to_string()),
        ],
    )
    .await?;

    // 4. operation_runs audit row + plans.version bump (shared audit-triad back half).
    let summary = format!("D{day}");
    crate::cascade::common::record_operation(
        conn,
        plan_id,
        "set-day-theme",
        &summary,
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    Ok(version_after)
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
        let p = parse_args(&["1".to_string()]).unwrap();
        assert_eq!(p.day, 1);
        assert!(p.theme.is_none());
        assert!(p.theme_zh.is_none());
    }

    #[test]
    fn parse_args_with_theme_and_zh() {
        let p = parse_args(&[
            "2".to_string(),
            "Kinkaku-ji full day".to_string(),
            "--zh".to_string(),
            "金閣寺".to_string(),
        ])
        .unwrap();
        assert_eq!(p.day, 2);
        assert_eq!(p.theme.as_deref(), Some("Kinkaku-ji full day"));
        assert_eq!(p.theme_zh.as_deref(), Some("金閣寺"));
    }

    #[test]
    fn parse_args_invalid_day() {
        assert!(parse_args(&["abc".to_string()]).is_err());
        assert!(parse_args(&["0".to_string()]).is_err());
        assert!(parse_args(&["-1".to_string()]).is_err());
    }

    #[test]
    fn parse_args_empty() {
        assert!(parse_args(&[]).is_err());
    }
}
