// `travel set-activity-poi <day> <session> <poi_id> [--match "<title substring>"]`
// — link an itinerary activity to a destination POI by id, so the dashboard
// attaches the POI's ticket price + map pin via a durable FK instead of a
// fragile exact-title match. NO CASCADE.
//
// Mirrors the neighboring set-activity-* commands:
//   1. Resolve the target activity by (plan_id, destination, day, session).
//      If that (day, session) holds >1 activity, `--match <substring>`
//      disambiguates against a case-insensitive title substring. Zero OR >1
//      matches without a unique disambiguator → FAIL LOUD (non-zero, no write).
//   2. Validate <poi_id> EXISTS in destination_pois for the plan's dest slug.
//      Unknown poi_id → FAIL LOUD.
//   3. UPDATE activities SET poi_id = ? … ; require rows_affected == 1.
//   4. Audit triad: operation_runs + plan_events (dest_process + timeline)
//      + plan_event_data KV + plans.version bump — same as set-activity-title.

use libsql::Connection;
use travel_db::repo::itinerary;

#[derive(Default, Debug)]
struct ParsedArgs {
    day: i64,
    session: String,
    poi_id: String,
    match_substr: Option<String>,
    dest: Option<String>,
}

/// CLI entry: `travel set-activity-poi <day> <session> <poi_id> [--match "<sub>"] [--dest <slug>]`.
pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_args(args)?;

    let conn = match crate::db::connect_write().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: failed to connect to Turso (write tier): {e}");
            std::process::exit(1);
        }
    };

    let destination = match read_destination(&conn, &plan_id, &parsed.dest).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    match execute(&conn, &plan_id, &destination, &parsed).await {
        Ok(title) => {
            println!("\n📍 Linking activity to POI:");
            println!("   Destination: {destination}");
            println!("   Day {} {}: \"{}\"", parsed.day, parsed.session, title);
            println!("   POI id: {}", parsed.poi_id);
            println!("✅ Activity linked to POI");
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: set-activity-poi failed: {e}");
            std::process::exit(1);
        }
    }
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut p = ParsedArgs::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--match" => {
                p.match_substr = Some(arg_value(args, i, "--match")?);
                i += 2;
            }
            "--dest" => {
                p.dest = Some(arg_value(args, i, "--dest")?);
                i += 2;
            }
            // Accept-and-skip plan-selection flags (plan resolution is done in main.rs via
            // plan_resolver::resolve_plan_id, matching the neighboring set-* mutations;
            // the resolved plan_id is passed into run()).
            f if crate::plan_resolver::is_resolver_flag(f) => {
                let _ = arg_value(args, i, f)?;
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
    if positional.len() < 3 {
        return Err(usage_error());
    }
    p.day = positional[0]
        .parse::<i64>()
        .map_err(|_| "<day> must be a positive integer".to_string())?;
    if p.day < 1 {
        return Err("<day> must be a positive integer".to_string());
    }
    p.session = positional[1].clone();
    if !["morning", "noon", "afternoon", "evening"].contains(&p.session.as_str()) {
        return Err("<session> must be one of: morning|noon|afternoon|evening".to_string());
    }
    p.poi_id = positional[2].clone();
    if p.poi_id.trim().is_empty() {
        return Err("<poi_id> cannot be empty".to_string());
    }
    Ok(p)
}

fn usage_error() -> String {
    "Usage: set-activity-poi <day> <session> <poi_id> [--match \"<title substring>\"] [--dest <slug>]
Example: set-activity-poi 2 morning shuri_castle --match \"Shurijo\""
        .to_string()
}

fn arg_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("missing value for {flag}"))
}

async fn read_destination(
    conn: &Connection,
    plan_id: &str,
    dest_opt: &Option<String>,
) -> Result<String, String> {
    crate::cascade::common::resolve_active_destination(conn, plan_id, dest_opt.as_deref()).await
}

/// Resolve the unique target activity (id, title) in (day, session), narrowed
/// by an optional case-insensitive title substring. Fails loud on zero or >1.
async fn resolve_activity(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    match_substr: &Option<String>,
) -> Result<(String, String), String> {
    let mut rows = conn
        .query(
            "SELECT id, title FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
               AND session_type = ?4 ORDER BY sort_order",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities list query failed: {e}"))?;

    let needle = match_substr.as_ref().map(|s| s.to_lowercase());
    let mut matches: Vec<(String, String)> = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities list row read failed: {e}"))?
    {
        let id: String = row.get(0).map_err(|e| format!("id col read failed: {e}"))?;
        let title: Option<String> = row.get(1).ok();
        let t = title.as_deref().unwrap_or("");
        match &needle {
            Some(n) if !t.to_lowercase().contains(n) => continue,
            _ => matches.push((id, t.to_string())),
        }
    }

    match matches.len() {
        0 => Err(format!(
            "no activity found in Day {day} {session}{}",
            match_substr
                .as_ref()
                .map(|s| format!(" matching \"{s}\""))
                .unwrap_or_default()
        )),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => Err(format!(
            "{n} activities match Day {day} {session}{} — disambiguate with --match \"<title substring>\"",
            match_substr
                .as_ref()
                .map(|s| format!(" \"{s}\""))
                .unwrap_or_default()
        )),
    }
}

/// Fail loud unless `poi_id` exists in destination_pois for this dest slug.
async fn assert_poi_exists(
    conn: &Connection,
    destination: &str,
    poi_id: &str,
) -> Result<(), String> {
    if itinerary::poi_exists(conn, destination, poi_id).await? {
        return Ok(());
    }
    Err(format!(
        "unknown poi_id \"{poi_id}\" for destination \"{destination}\" \
         (no row in destination_pois)"
    ))
}

async fn execute(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    parsed: &ParsedArgs,
) -> Result<String, String> {
    // 1. Resolve the unique target activity (fail loud on 0 / >1).
    let (activity_id, title) = resolve_activity(
        conn,
        plan_id,
        destination,
        parsed.day,
        &parsed.session,
        &parsed.match_substr,
    )
    .await?;

    // 2. Validate the poi_id exists (fail loud before any write).
    assert_poi_exists(conn, destination, &parsed.poi_id).await?;

    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    // 3. UPDATE activities.poi_id — require rows_affected == 1.
    let affected = itinerary::set_activity_poi(
        conn,
        plan_id,
        destination,
        parsed.day,
        &parsed.session,
        &activity_id,
        &parsed.poi_id,
        &now_db,
    )
    .await?;
    if affected != 1 {
        return Err(format!(
            "activities UPDATE affected {affected} rows (expected 1) for id={activity_id}"
        ));
    }
    touch_day(conn, plan_id, destination, parsed.day).await?;

    // 4. Audit triad — plan_events (dest_process + timeline) + KV.
    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        ("activity_id", activity_id.clone()),
        ("title", title.clone()),
        ("poi_id", parsed.poi_id.clone()),
    ];

    let dest_process_so =
        next_dest_process_sort_order(conn, plan_id, destination, "process_5_daily_itinerary").await?;
    let timeline_so = next_timeline_sort_order(conn, plan_id).await?;
    insert_event(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_5_daily_itinerary",
        dest_process_so,
        "activity_poi_linked",
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
        &kv,
    )
    .await?;
    insert_event(
        conn,
        plan_id,
        "timeline",
        "",
        "process_5_daily_itinerary",
        timeline_so,
        "activity_poi_linked",
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
        &kv,
    )
    .await?;

    // operation_runs audit row + plans.version bump (audit-triad back half).
    crate::cascade::common::record_operation(
        conn,
        plan_id,
        "set-activity-poi",
        &format!("D{} {} → {}", parsed.day, parsed.session, parsed.poi_id),
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    Ok(title)
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
        let v: i64 = row.get(0).map_err(|e| format!("version col read failed: {e}"))?;
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
        let m: i64 = row.get(0).map_err(|e| format!("plan_events MAX col read failed: {e}"))?;
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
        let p = parse_args(&[
            "2".to_string(),
            "morning".to_string(),
            "shuri_castle".to_string(),
        ])
        .unwrap();
        assert_eq!(p.day, 2);
        assert_eq!(p.session, "morning");
        assert_eq!(p.poi_id, "shuri_castle");
        assert!(p.match_substr.is_none());
    }

    #[test]
    fn parse_args_with_match_and_dest() {
        let p = parse_args(&[
            "2".to_string(),
            "morning".to_string(),
            "shuri_castle".to_string(),
            "--match".to_string(),
            "Shurijo".to_string(),
            "--dest".to_string(),
            "okinawa_2026".to_string(),
        ])
        .unwrap();
        assert_eq!(p.match_substr.as_deref(), Some("Shurijo"));
        assert_eq!(p.dest.as_deref(), Some("okinawa_2026"));
    }

    #[test]
    fn parse_args_accepts_and_skips_plan_id() {
        let p = parse_args(&[
            "1".to_string(),
            "afternoon".to_string(),
            "kokusaidori".to_string(),
            "--plan-id".to_string(),
            "okinawa-2026".to_string(),
        ])
        .unwrap();
        assert_eq!(p.day, 1);
        assert_eq!(p.poi_id, "kokusaidori");
    }

    #[test]
    fn parse_args_rejects_bad_session() {
        assert!(parse_args(&[
            "2".to_string(),
            "lunch".to_string(),
            "x".to_string()
        ])
        .is_err());
    }

    #[test]
    fn parse_args_rejects_missing_positional() {
        assert!(parse_args(&["2".to_string(), "morning".to_string()]).is_err());
    }

    #[test]
    fn parse_args_rejects_bad_day() {
        assert!(parse_args(&[
            "0".to_string(),
            "morning".to_string(),
            "x".to_string()
        ])
        .is_err());
    }
}
