// `travel set-activity-time` and `travel set-activity-title` —
// port of src/cli/commands/activity.ts. NO CASCADE.
//
// Both commands look up an activity within a (day, session) by id or
// by case-insensitive title substring (matches the TS
// `findActivityIndex` behavior), then UPDATE the activities row +
// emit a plan_event.

use libsql::Connection;
use travel_db::repo::itinerary;

#[derive(Default, Debug)]
struct ActivityTime {
    day: i64,
    session: String,
    activity: String,
    start_time: Option<String>,
    end_time: Option<String>,
    is_fixed_time: Option<bool>,
    dest: Option<String>,
}

#[derive(Default, Debug)]
struct ActivityTitle {
    day: i64,
    session: String,
    activity: String,
    new_title: String,
    dest: Option<String>,
}

pub async fn run_time(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    let parsed = parse_time(args)?;

    if parsed.start_time.is_none()
        && parsed.end_time.is_none()
        && parsed.is_fixed_time.is_none()
    {
        eprintln!("Error: set-activity-time requires at least one of: --start, --end, --fixed");
        std::process::exit(1);
    }

    let conn = crate::db::connect_write().await?;
    let destination = match read_destination(&conn, &plan_id, &parsed.dest).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };

    let activity_id = find_activity(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &parsed.activity,
    )
    .await?;

    let (prev_start, prev_end, prev_fixed) =
        read_activity_time_fields(&conn, &plan_id, &destination, parsed.day, &parsed.session, &activity_id)
            .await?;
    let title = read_activity_title(&conn, &plan_id, &destination, parsed.day, &parsed.session, &activity_id)
        .await?;

    if let Some(s) = &parsed.start_time {
        update_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            &activity_id,
            "start_time",
            Some(s.clone()),
        )
        .await?;
    }
    if let Some(s) = &parsed.end_time {
        update_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            &activity_id,
            "end_time",
            Some(s.clone()),
        )
        .await?;
    }
    if let Some(b) = parsed.is_fixed_time {
        update_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            &activity_id,
            "is_fixed_time",
            Some(if b { "1".to_string() } else { "0".to_string() }),
        )
        .await?;
    }
    touch_day(&conn, &plan_id, &destination, parsed.day).await?;

    // eventData: { day_number, session, activity_id, title, from, to }
    //   from: { start_time, end_time, is_fixed_time } (current values, may
    //         be undefined — eventDataToKv flattens to a single '_value'
    //         key if ALL values are undefined; otherwise emits
    //         "k1=undefined, k2=v" strings).
    //   to:   { start_time, end_time, is_fixed_time } (after values)
    let new_start = parsed.start_time.clone().or(prev_start.clone());
    let new_end = parsed.end_time.clone().or(prev_end.clone());
    let new_fixed: Option<bool> = match parsed.is_fixed_time {
        Some(b) => Some(b),
        None => prev_fixed,
    };
    let from_value = format!(
        "start_time={}, end_time={}, is_fixed_time={}",
        render_optional(&prev_start),
        render_optional(&prev_end),
        render_optional_bool(prev_fixed)
    );
    let to_value = format!(
        "start_time={}, end_time={}, is_fixed_time={}",
        render_optional(&new_start),
        render_optional(&new_end),
        render_optional_bool(new_fixed)
    );

    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        ("activity_id", activity_id.clone()),
        ("title", title),
        ("from", from_value),
        ("to", to_value),
    ];

    execute_event(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &activity_id,
        "activity_time_updated",
        "set-activity-time",
        &format!("D{} {} {}", parsed.day, parsed.session, parsed.activity),
        &kv,
    )
    .await?;

    println!(
        "\n⏱️  Setting activity time:\n   Destination: {destination}\n   Day {} {}: \"{}\"\n   Start: {}\n   End: {}\n   Fixed: {}",
        parsed.day,
        parsed.session,
        parsed.activity,
        parsed.start_time.as_deref().unwrap_or(""),
        parsed.end_time.as_deref().unwrap_or(""),
        parsed.is_fixed_time.map(|b| b.to_string()).unwrap_or_default()
    );
    println!("✅ Activity time updated");
    Ok(())
}

pub async fn run_title(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    let parsed = parse_title(args)?;

    if parsed.new_title.trim().is_empty() {
        eprintln!("Error: <new_title> cannot be empty");
        std::process::exit(1);
    }

    // Fail loud on a broken embedded Maps URL (the /maps/dir/?...&... form the
    // dashboard linkifier truncates at the first '&'). Reject before any write.
    if let Err(reason) = crate::checks::check_title_map_url(&parsed.new_title) {
        eprintln!("Error: {reason}");
        std::process::exit(1);
    }

    let conn = crate::db::connect_write().await?;
    let destination = match read_destination(&conn, &plan_id, &parsed.dest).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };

    let activity_id = find_activity(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &parsed.activity,
    )
    .await?;

    let previous_title = read_activity_title(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &activity_id,
    )
    .await?;
    let previous_poi_id = read_activity_poi_id(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &activity_id,
    )
    .await?;

    // Fail-proof title↔poi_id guard (clear-and-notify): re-theming an activity
    // by editing its title leaves any existing poi_id pointing at the OLD place
    // (the dashboard would then attach the wrong POI's coords/price/pin). When
    // the title ACTUALLY changes and the row currently HAS a poi_id, clear the
    // now-unverified link and print a plain agent-first note with a
    // copy-pasteable re-link command. An idempotent re-run (same title) — or a
    // row with no poi_id — clears nothing and is silent (prior behavior).
    let title_changed = parsed.new_title != previous_title;
    let stale_poi: Option<String> = previous_poi_id
        .filter(|p| !p.is_empty())
        .filter(|_| title_changed);

    itinerary::update_activity_title(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &activity_id,
        &parsed.new_title,
        stale_poi.is_some(),
        &now_db_datetime(),
    )
    .await?;
    touch_day(&conn, &plan_id, &destination, parsed.day).await?;

    // event data: { day_number, session, activity_id, from_title,
    // to_title } — 5 keys (from_title and to_title both present
    // even when previous was undefined, in which case it's
    // 'undefined').
    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        ("activity_id", activity_id.clone()),
        (
            "from_title",
            if previous_title.is_empty() {
                "undefined".to_string()
            } else {
                previous_title
            },
        ),
        ("to_title", parsed.new_title.clone()),
    ];

    execute_event(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &activity_id,
        "activity_title_updated",
        "set-activity-title",
        &format!("D{} {} {}", parsed.day, parsed.session, parsed.activity),
        &kv,
    )
    .await?;

    println!(
        "\n✏️  Renaming activity:\n   Destination: {destination}\n   Day {} {}: \"{}\"\n   New title: \"{}\"\n✅ Activity title updated",
        parsed.day,
        parsed.session,
        parsed.activity,
        parsed.new_title
    );
    // Agent-first note (NOT an error — the write succeeded): the title changed
    // and the POI link was cleared because it is no longer verified. Name the
    // old poi_id and give the exact, copy-pasteable re-link command so the agent
    // can restore it if the place is actually unchanged.
    if let Some(old_poi) = stale_poi {
        println!(
            "note: cleared poi_id (was '{old_poi}') because the title changed — the POI link is no longer verified. If this is the same place, re-link with: travel set-activity-poi {} {} {old_poi}",
            parsed.day, parsed.session
        );
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// Arg parsing
// ─────────────────────────────────────────────────────────────────

fn parse_time(args: &[String]) -> Result<ActivityTime, String> {
    let mut p = ActivityTime::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                p.dest = Some(arg_value(args, i, "--dest")?);
                i += 2;
            }
            "--start" => {
                p.start_time = Some(arg_value(args, i, "--start")?);
                i += 2;
            }
            "--end" => {
                p.end_time = Some(arg_value(args, i, "--end")?);
                i += 2;
            }
            "--fixed" => {
                let v = arg_value(args, i, "--fixed")?;
                p.is_fixed_time = Some(parse_bool(&v)?);
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
    if positional.len() < 3 {
        return Err("Usage: set-activity-time <day> <session> <activity> --start HH:MM --end HH:MM [--fixed true|false]".to_string());
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
    p.activity = positional[2].clone();
    // Fail-loud HH:MM validation BEFORE any DB write (this Err bubbles to main →
    // stderr + non-zero exit, writing NOTHING). When BOTH times are present,
    // also enforce start ≤ end.
    if let Some(s) = &p.start_time {
        crate::checks::validate_time_flag("--start", s)?;
    }
    if let Some(e) = &p.end_time {
        crate::checks::validate_time_flag("--end", e)?;
    }
    if let (Some(s), Some(e)) = (&p.start_time, &p.end_time) {
        crate::checks::validate_start_le_end("--start", s, "--end", e)?;
    }
    Ok(p)
}

fn parse_title(args: &[String]) -> Result<ActivityTitle, String> {
    let mut p = ActivityTitle::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                p.dest = Some(arg_value(args, i, "--dest")?);
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
    if positional.len() < 4 {
        return Err(
            "Usage: set-activity-title <day> <session> <activity> <new_title> [--dest <slug>]"
                .to_string(),
        );
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
    p.activity = positional[2].clone();
    p.new_title = positional[3..].join(" ");
    Ok(p)
}

fn arg_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn parse_bool(s: &str) -> Result<bool, String> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "y" => Ok(true),
        "false" | "0" | "no" | "n" => Ok(false),
        _ => Err(format!("Invalid --fixed value: {s} (use true|false)")),
    }
}

fn render_optional(v: &Option<String>) -> String {
    match v {
        Some(s) if !s.is_empty() => s.clone(),
        _ => "undefined".to_string(),
    }
}

fn render_optional_bool(b: Option<bool>) -> String {
    match b {
        Some(b) => b.to_string(),
        None => "undefined".to_string(),
    }
}

// ─────────────────────────────────────────────────────────────────
// DB helpers
// ─────────────────────────────────────────────────────────────────

async fn read_destination(
    conn: &Connection,
    plan_id: &str,
    dest_opt: &Option<String>,
) -> Result<String, String> {
    crate::cascade::common::resolve_active_destination(conn, plan_id, dest_opt.as_deref()).await
}

async fn find_activity(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    id_or_title: &str,
) -> Result<String, String> {
    // First try exact id match.
    let mut rows = conn
        .query(
            "SELECT id, title FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
               AND session_type = ?4 AND id = ?5",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                id_or_title.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities exact id query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities exact id row read failed: {e}"))?
    {
        let id: String = row
            .get(0)
            .map_err(|e| format!("id col read failed: {e}"))?;
        return Ok(id);
    }

    // Fall back to case-insensitive title substring match.
    let mut rows = conn
        .query(
            "SELECT id, title FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
               AND session_type = ?4",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities list query failed: {e}"))?;
    let needle = id_or_title.to_lowercase();
    let mut best: Option<(String, String)> = None;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities list row read failed: {e}"))?
    {
        let id: String = row
            .get(0)
            .map_err(|e| format!("id col read failed: {e}"))?;
        let title: Option<String> = row.get(1).ok();
        let t = title.as_deref().unwrap_or("");
        if t.to_lowercase().contains(&needle) {
            // TS uses .findIndex (first match); mirror that.
            if best.is_none() {
                best = Some((id, t.to_string()));
            }
        }
    }
    if let Some((id, _)) = best {
        return Ok(id);
    }

    Err(format!(
        "Activity not found: \"{id_or_title}\" in Day {day} {session}"
    ))
}

async fn read_activity_time_fields(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
) -> Result<(Option<String>, Option<String>, Option<bool>), String> {
    let mut rows = conn
        .query(
            "SELECT start_time, end_time, is_fixed_time FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
               AND session_type = ?4 AND id = ?5",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                activity_id.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities time query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities time row read failed: {e}"))?
    {
        let s: Option<String> = row.get(0).ok();
        let e: Option<String> = row.get(1).ok();
        let f: Option<i64> = row.get(2).ok();
        return Ok((s, e, f.map(|v| v != 0)));
    }
    Err(format!("activity row disappeared: id={activity_id}"))
}

async fn read_activity_title(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
) -> Result<String, String> {
    let mut rows = conn
        .query(
            "SELECT title FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
               AND session_type = ?4 AND id = ?5",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                activity_id.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities title query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities title row read failed: {e}"))?
    {
        let t: String = row
            .get(0)
            .map_err(|e| format!("title col read failed: {e}"))?;
        return Ok(t);
    }
    Err(format!("activity row disappeared: id={activity_id}"))
}

/// Read an activity's current `sort_order` (used by add-activity --after to
/// find the anchor's slot).
async fn read_activity_sort_order(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT sort_order FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
               AND session_type = ?4 AND id = ?5",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                activity_id.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities sort_order query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities sort_order row read failed: {e}"))?
    {
        return row.get::<i64>(0).map_err(|e| format!("sort_order col read failed: {e}"));
    }
    Err(format!("activity row disappeared: id={activity_id}"))
}

// Read the current poi_id of an activity. Returns None when the column is NULL
// (or an empty string — treated the same as "no link" by the title guard);
// Some(id) when a non-empty link is present. Errs only if the row vanished.
async fn read_activity_poi_id(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT poi_id FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
               AND session_type = ?4 AND id = ?5",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                activity_id.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities poi_id query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities poi_id row read failed: {e}"))?
    {
        let p: Option<String> = row.get(0).ok();
        return Ok(p.filter(|s| !s.is_empty()));
    }
    Err(format!("activity row disappeared: id={activity_id}"))
}

#[allow(clippy::too_many_arguments)]
async fn update_field(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
    field: &str,
    value: Option<String>,
) -> Result<(), String> {
    itinerary::update_activity_field(
        conn,
        plan_id,
        destination,
        day,
        session,
        activity_id,
        field,
        value,
        &now_db_datetime(),
    )
    .await
}

async fn touch_day(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<(), String> {
    // Domain write now lives in the DAL; CLI computes `now` fresh (now_db_datetime()),
    // preserving the prior inline timing + byte-identical SQL.
    itinerary::touch_day(conn, plan_id, destination, day, &now_db_datetime()).await
}

#[allow(clippy::too_many_arguments)]
async fn execute_event(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
    event_name: &str,
    command_type: &str,
    command_summary: &str,
    kv: &[(&str, String)],
) -> Result<(), String> {
    let _ = (day, session, activity_id);
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;
    let dest_process_so = next_dest_process_sort_order(
        conn,
        plan_id,
        destination,
        "process_5_daily_itinerary",
    )
    .await?;
    let timeline_base = next_timeline_sort_order(conn, plan_id).await?;
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
    let timeline_so = timeline_base;
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

// ── P1 Rust-port STUBS (batch 1) — see docs/plans/2026-06-10-rust-port-audit.md ──
// Port behavior from src/cli/commands/activity.ts. Signatures fixed by main.rs
// dispatch (delete-activity/remove-activity → run_delete; set-activity-booking →
// run_booking). Replace the bodies; reuse the helpers above (read_destination,
// arg parsing, etc.). Writes/deletes against the `activities` table.

#[derive(Default, Debug)]
struct ActivityDelete {
    day: i64,
    session: String,
    activity: String,
    dest: Option<String>,
}

#[derive(Default, Debug)]
struct ActivityBooking {
    day: i64,
    session: String,
    activity: String,
    status: String,
    booking_ref: Option<String>,
    book_by: Option<String>,
    dest: Option<String>,
}

#[derive(Default, Debug)]
struct ActivityReorder {
    day: i64,
    session: String,
    tokens: Vec<String>,
    dest: Option<String>,
}

#[derive(Default, Debug)]
struct ActivityAdd {
    day: i64,
    session: String,
    title: String,
    area: Option<String>,
    nearest_station: Option<String>,
    duration_min: Option<i64>,
    start_time: Option<String>,
    end_time: Option<String>,
    is_fixed_time: bool,
    priority: String,
    notes: Option<String>,
    dest: Option<String>,
    /// `--after <id|title>`: insert directly after this activity instead of
    /// appending at the end of the session.
    after: Option<String>,
}

// ── delete-activity / remove-activity ──────────────────────────────
//
// Port of deleteActivityCommand: resolve the activity within (day,
// session) by id OR case-insensitive title substring, then DELETE the
// row (mirrors the `remove_activity` dispatch) and emit an
// `activity_removed` plan_event.
pub async fn run_delete(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_delete(args)?;

    let conn = crate::db::connect_write().await?;
    let destination = read_destination(&conn, &plan_id, &parsed.dest).await?;

    let activity_id = find_activity(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &parsed.activity,
    )
    .await?;

    let title = read_activity_title(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &activity_id,
    )
    .await?;

    println!("\n🗑️  Deleting activity:");
    println!("   Day {} {}: \"{}\"", parsed.day, parsed.session, title);

    itinerary::delete_activity(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &activity_id,
    )
    .await?;
    touch_day(&conn, &plan_id, &destination, parsed.day).await?;

    // event data mirrors removeActivity emitEvent:
    //   { day_number, session, activity_id, title }
    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        ("activity_id", activity_id.clone()),
        ("title", title.clone()),
    ];

    execute_event(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &activity_id,
        "activity_removed",
        "delete-activity",
        &format!("D{}/{}/{}", parsed.day, parsed.session, title),
        &kv,
    )
    .await?;

    println!("✅ Activity deleted");
    Ok(())
}

// ── move-activity ──────────────────────────────────────────────────
//
// Move an activity to another session (and optionally another day) WITHOUT
// delete+re-add — so the row keeps its id, poi_id, booking fields, tags, etc.
// (delete+re-add was the old workaround and dropped all of those). The activity
// is appended (max sort_order + 1) in the target session; reorder afterward if
// a specific position is needed.
//
//   travel move-activity <day> <from-session> <to-session> <id|title>
//                        [--to-day N] [--dest slug]
pub async fn run_move(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_move(args)?;

    let conn = crate::db::connect_write().await?;
    let destination = read_destination(&conn, &plan_id, &parsed.dest).await?;

    let from_day = parsed.day;
    let to_day = parsed.to_day.unwrap_or(parsed.day);
    if from_day == to_day && parsed.from_session == parsed.to_session {
        return Err(
            "move-activity: source and target (day, session) are identical — nothing to move"
                .to_string(),
        );
    }

    let activity_id = find_activity(
        &conn,
        &plan_id,
        &destination,
        from_day,
        &parsed.from_session,
        &parsed.activity,
    )
    .await?;
    let title =
        read_activity_title(&conn, &plan_id, &destination, from_day, &parsed.from_session, &activity_id)
            .await?;

    // Verify the target day/session exists (fail loud, like the set-* writers).
    require_session_exists(&conn, &plan_id, &destination, to_day, &parsed.to_session).await?;

    let new_sort =
        next_activity_sort_order(&conn, &plan_id, &destination, to_day, &parsed.to_session).await?;

    println!("\n↔️  Moving activity:");
    println!(
        "   D{} {} → D{} {}: \"{}\"",
        from_day, parsed.from_session, to_day, parsed.to_session, title
    );

    // Re-key the row by id; every other column (poi_id, booking_*, tags via the
    // separate activity_tags table keyed on activity_id) is preserved untouched.
    itinerary::move_activity(
        &conn,
        &plan_id,
        &destination,
        &activity_id,
        to_day,
        &parsed.to_session,
        new_sort,
    )
    .await?;

    touch_day(&conn, &plan_id, &destination, from_day).await?;
    if to_day != from_day {
        touch_day(&conn, &plan_id, &destination, to_day).await?;
    }

    let kv: Vec<(&str, String)> = vec![
        ("activity_id", activity_id.clone()),
        ("title", title.clone()),
        ("from_day", from_day.to_string()),
        ("from_session", parsed.from_session.clone()),
        ("to_day", to_day.to_string()),
        ("to_session", parsed.to_session.clone()),
    ];
    // Audit under the TARGET day/session (where the activity now lives).
    execute_event(
        &conn,
        &plan_id,
        &destination,
        to_day,
        &parsed.to_session,
        &activity_id,
        "activity_moved",
        "move-activity",
        &format!(
            "D{}/{} → D{}/{}: {}",
            from_day, parsed.from_session, to_day, parsed.to_session, title
        ),
        &kv,
    )
    .await?;

    println!("✅ Activity moved (id preserved: {activity_id})");
    Ok(())
}

struct MoveArgs {
    day: i64,
    from_session: String,
    to_session: String,
    activity: String,
    to_day: Option<i64>,
    dest: Option<String>,
}

fn parse_move(args: &[String]) -> Result<MoveArgs, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut to_day: Option<i64> = None;
    let mut dest: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--to-day" => {
                to_day = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --to-day".to_string())?
                        .parse::<i64>()
                        .map_err(|_| "--to-day must be an integer".to_string())?,
                );
                i += 2;
            }
            "--dest" => {
                dest = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --dest".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--plan-id" => i += 2, // consumed by the resolver
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => {
                positional.push(args[i].clone());
                i += 1;
            }
        }
    }
    if positional.len() < 4 {
        return Err(
            "Usage: move-activity <day> <from-session> <to-session> <id|title> [--to-day N] [--dest slug]"
                .to_string(),
        );
    }
    let day = positional[0]
        .parse::<i64>()
        .map_err(|_| "<day> must be a positive integer".to_string())?;
    Ok(MoveArgs {
        day,
        from_session: positional[1].clone(),
        to_session: positional[2].clone(),
        activity: positional[3..].join(" "),
        to_day,
        dest,
    })
}

// ── set-activity-booking ───────────────────────────────────────────
//
// Port of setActivityBookingCommand + setActivityBookingStatus:
// resolve activity, UPDATE booking_status (+ booking_ref, book_by when
// provided), force booking_required=1 for booked/pending/waitlist, and
// emit an `activity_booking_updated` plan_event.
pub async fn run_booking(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_booking(args)?;

    let conn = crate::db::connect_write().await?;
    let destination = read_destination(&conn, &plan_id, &parsed.dest).await?;

    let activity_id = find_activity(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &parsed.activity,
    )
    .await?;

    let title = read_activity_title(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &activity_id,
    )
    .await?;
    let previous_status = read_booking_status(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &activity_id,
    )
    .await?;

    println!("\n🎫 Setting activity booking status:");
    println!("   Destination: {destination}");
    println!("   Day {} {}: \"{}\"", parsed.day, parsed.session, parsed.activity);
    println!("   Status: {}", parsed.status);
    if let Some(r) = &parsed.booking_ref {
        println!("   Reference: {r}");
    }
    if let Some(b) = &parsed.book_by {
        println!("   Book by: {b}");
    }

    // booking_status — always written.
    update_field(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &activity_id,
        "booking_status",
        Some(parsed.status.clone()),
    )
    .await?;
    // booking_ref / book_by — only when provided (matches TS `!== undefined`).
    if let Some(r) = &parsed.booking_ref {
        update_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            &activity_id,
            "booking_ref",
            Some(r.clone()),
        )
        .await?;
    }
    if let Some(b) = &parsed.book_by {
        update_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            &activity_id,
            "book_by",
            Some(b.clone()),
        )
        .await?;
    }
    // booked/pending/waitlist imply booking_required = true.
    if matches!(parsed.status.as_str(), "booked" | "pending" | "waitlist") {
        update_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            &activity_id,
            "booking_required",
            Some("1".to_string()),
        )
        .await?;
    }
    touch_day(&conn, &plan_id, &destination, parsed.day).await?;

    // event data mirrors setActivityBookingStatus emitEvent:
    //   { day_number, session, activity_id, title, from_status,
    //     to_status, booking_ref, book_by, upgraded_from_string }
    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        ("activity_id", activity_id.clone()),
        ("title", title),
        ("from_status", render_optional(&previous_status)),
        ("to_status", parsed.status.clone()),
        ("booking_ref", render_optional(&parsed.booking_ref)),
        ("book_by", render_optional(&parsed.book_by)),
        // Rust path always operates on a real row (never an upgraded
        // string activity), so this is always false.
        ("upgraded_from_string", "false".to_string()),
    ];

    execute_event(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &activity_id,
        "activity_booking_updated",
        "set-activity-booking",
        &format!("D{} {} {}", parsed.day, parsed.session, parsed.activity),
        &kv,
    )
    .await?;

    println!("✅ Activity booking status updated");
    Ok(())
}

// ── add-activity ───────────────────────────────────────────────────
//
// Insert a brand-new activity row into a (day, session), assigning the
// next sort_order and a fresh UUID, then emit an `activity_added`
// plan_event (full audit triad via execute_event). This is the missing
// create counterpart to set-activity-* / delete-activity, used to flesh
// out an itinerary without raw SQL.
pub async fn run_add(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_add(args)?;

    if parsed.title.trim().is_empty() {
        eprintln!("Error: <title> cannot be empty");
        std::process::exit(1);
    }

    // Fail loud on a broken embedded Maps URL (the /maps/dir/?...&... form the
    // dashboard linkifier truncates at the first '&'). Reject before any write.
    if let Err(reason) = crate::checks::check_title_map_url(&parsed.title) {
        eprintln!("Error: {reason}");
        std::process::exit(1);
    }

    let conn = crate::db::connect_write().await?;
    let destination = read_destination(&conn, &plan_id, &parsed.dest).await?;

    // The day must already exist (scaffold-itinerary creates day rows).
    if !day_exists(&conn, &plan_id, &destination, parsed.day).await? {
        return Err(format!(
            "Day {} does not exist for destination {destination} — scaffold the itinerary first",
            parsed.day
        ));
    }

    let activity_id = new_activity_id();
    // Position: append at the end (default), or insert directly after a named
    // activity when `--after <id|title>` is given. For --after we open a gap by
    // shifting every later row up by one, then take that slot.
    let sort_order = match &parsed.after {
        None => {
            next_activity_sort_order(&conn, &plan_id, &destination, parsed.day, &parsed.session)
                .await?
        }
        Some(anchor) => {
            let anchor_id =
                find_activity(&conn, &plan_id, &destination, parsed.day, &parsed.session, anchor)
                    .await?;
            let anchor_so = read_activity_sort_order(
                &conn,
                &plan_id,
                &destination,
                parsed.day,
                &parsed.session,
                &anchor_id,
            )
            .await?;
            // Shift later rows up to free the (anchor_so + 1) slot. Descending
            // order is not required since +1 keeps values distinct, but the gap
            // must be opened before the INSERT.
            itinerary::shift_activities_after(
                &conn,
                &plan_id,
                &destination,
                parsed.day,
                &parsed.session,
                anchor_so,
            )
            .await?;
            anchor_so + 1
        }
    };

    itinerary::insert_activity(
        &conn,
        &activity_id,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        sort_order,
        &parsed.title,
        parsed.area.clone(),
        parsed.nearest_station.clone(),
        parsed.duration_min,
        parsed.start_time.clone(),
        parsed.end_time.clone(),
        parsed.is_fixed_time,
        &parsed.priority,
        parsed.notes.clone(),
        &now_db_datetime(),
    )
    .await?;
    touch_day(&conn, &plan_id, &destination, parsed.day).await?;

    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        ("activity_id", activity_id.clone()),
        ("title", parsed.title.clone()),
        ("sort_order", sort_order.to_string()),
    ];

    execute_event(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &activity_id,
        "activity_added",
        "add-activity",
        &format!("D{} {} {}", parsed.day, parsed.session, parsed.title),
        &kv,
    )
    .await?;

    println!(
        "\n➕ Adding activity:\n   Destination: {destination}\n   Day {} {}: \"{}\"\n   Sort order: {}\n✅ Activity added (id={})",
        parsed.day, parsed.session, parsed.title, sort_order, activity_id
    );
    Ok(())
}

fn parse_add(args: &[String]) -> Result<ActivityAdd, String> {
    let mut p = ActivityAdd::default();
    p.priority = "want".to_string();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                p.dest = Some(arg_value(args, i, "--dest")?);
                i += 2;
            }
            "--area" => {
                p.area = Some(arg_value(args, i, "--area")?);
                i += 2;
            }
            "--station" => {
                p.nearest_station = Some(arg_value(args, i, "--station")?);
                i += 2;
            }
            "--duration" => {
                let v = arg_value(args, i, "--duration")?;
                p.duration_min = Some(
                    v.parse::<i64>()
                        .map_err(|_| "--duration must be an integer (minutes)".to_string())?,
                );
                i += 2;
            }
            "--start" => {
                p.start_time = Some(arg_value(args, i, "--start")?);
                i += 2;
            }
            "--end" => {
                p.end_time = Some(arg_value(args, i, "--end")?);
                i += 2;
            }
            "--fixed" => {
                let v = arg_value(args, i, "--fixed")?;
                p.is_fixed_time = parse_bool(&v)?;
                i += 2;
            }
            "--priority" => {
                p.priority = arg_value(args, i, "--priority")?;
                i += 2;
            }
            "--notes" => {
                p.notes = Some(arg_value(args, i, "--notes")?);
                i += 2;
            }
            "--after" => {
                p.after = Some(arg_value(args, i, "--after")?);
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
    if positional.len() < 3 {
        return Err(
            "Usage: add-activity <day> <session> <title> [--after <id|title>] [--area ..] [--station ..] [--duration MIN] [--start HH:MM] [--end HH:MM] [--fixed true|false] [--priority must|want|optional] [--notes ..] [--dest <slug>]"
                .to_string(),
        );
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
    // Title is the remaining positionals joined (mirrors set-activity-title).
    p.title = positional[2..].join(" ");
    if !["must", "want", "optional"].contains(&p.priority.as_str()) {
        return Err("--priority must be one of: must | want | optional".to_string());
    }
    // Fail-loud HH:MM validation BEFORE any DB write (this Err bubbles to main →
    // stderr + non-zero exit, writing NOTHING). When BOTH times are present,
    // also enforce start ≤ end.
    if let Some(s) = &p.start_time {
        crate::checks::validate_time_flag("--start", s)?;
    }
    if let Some(e) = &p.end_time {
        crate::checks::validate_time_flag("--end", e)?;
    }
    if let (Some(s), Some(e)) = (&p.start_time, &p.end_time) {
        crate::checks::validate_start_le_end("--start", s, "--end", e)?;
    }
    Ok(p)
}

async fn day_exists(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM days WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("days existence query failed: {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("days existence row read failed: {e}"))?
        .is_some())
}

/// Fail loud if the target (day, session) has no `timesofday` row — used by
/// move-activity so an activity can't be moved into a non-existent session.
async fn require_session_exists(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
) -> Result<(), String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM timesofday \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("timesofday existence query failed: {e}"))?;
    let exists = rows
        .next()
        .await
        .map_err(|e| format!("timesofday existence row read failed: {e}"))?
        .is_some();
    if !exists {
        return Err(format!(
            "no session for D{day}/{session} (destination={destination}); \
             scaffold the itinerary first (travel scaffold-itinerary)"
        ));
    }
    Ok(())
}

async fn next_activity_sort_order(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
) -> Result<i64, String> {
    itinerary::next_activity_sort_order(conn, plan_id, destination, day, session).await
}

// UUIDv4-shaped id for a new activity (same generator the audit run_id uses).
fn new_activity_id() -> String {
    new_run_id()
}

// ── reorder-activities ─────────────────────────────────────────────
//
// Rewrite the sort_order of every activity in a (day, session) to match a
// caller-supplied ordering. Since add-activity only appends (sort_order =
// max+1), this is the primitive for inserting in the middle, moving, or
// fully resequencing without a delete-and-re-add dance.
//
//   travel reorder-activities <day> <session> <id|title> <id|title> ... [--dest]
//
// The token list MUST name every current activity in the session exactly
// once (resolved by id, else case-insensitive title substring). Any
// missing/extra/duplicate/ambiguous token is a hard error — we refuse to
// silently drop or duplicate a row. All UPDATEs run in one pass and then a
// single audit event fires.
pub async fn run_reorder(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_reorder(args)?;

    let conn = crate::db::connect_write().await?;
    let destination = read_destination(&conn, &plan_id, &parsed.dest).await?;

    // Current activities in the session, in existing order.
    let current = list_session_activities(&conn, &plan_id, &destination, parsed.day, &parsed.session).await?;
    if current.is_empty() {
        return Err(format!(
            "No activities in Day {} {} to reorder",
            parsed.day, parsed.session
        ));
    }
    if parsed.tokens.len() != current.len() {
        return Err(format!(
            "reorder-activities must list all {} activities in Day {} {} exactly once (got {} token(s))",
            current.len(),
            parsed.day,
            parsed.session,
            parsed.tokens.len()
        ));
    }

    // Resolve each token to a current activity id, enforcing a bijection.
    let mut resolved: Vec<String> = Vec::with_capacity(parsed.tokens.len());
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    for token in &parsed.tokens {
        let id = resolve_token(&current, token)?;
        if !used.insert(id.clone()) {
            return Err(format!(
                "Activity \"{token}\" resolves to one already listed — each activity must appear exactly once"
            ));
        }
        resolved.push(id);
    }

    // Apply the new order. Two-phase to avoid transient PK-ish collisions on
    // (plan,dest,day,session,sort_order) is unnecessary (sort_order isn't part
    // of the PK — id is), so a direct rewrite per id is safe.
    for (new_order, id) in resolved.iter().enumerate() {
        itinerary::update_activity_sort_order(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            id,
            new_order as i64,
            &now_db_datetime(),
        )
        .await?;
    }
    touch_day(&conn, &plan_id, &destination, parsed.day).await?;

    let order_summary = resolved.join(",");
    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        ("count", resolved.len().to_string()),
        ("new_order", order_summary),
    ];

    // Audit event is session-scoped; pass the first activity id as the
    // representative entity (execute_event ignores it beyond logging).
    execute_event(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &resolved[0],
        "activities_reordered",
        "reorder-activities",
        &format!("D{} {} ({} items)", parsed.day, parsed.session, resolved.len()),
        &kv,
    )
    .await?;

    println!(
        "\n🔀 Reordering activities:\n   Destination: {destination}\n   Day {} {} — {} activities",
        parsed.day, parsed.session, resolved.len()
    );
    // Re-list in the new order for confirmation.
    let after = list_session_activities(&conn, &plan_id, &destination, parsed.day, &parsed.session).await?;
    for (i, (_, title)) in after.iter().enumerate() {
        println!("   {}. {}", i, first_line(title));
    }
    println!("✅ Activities reordered");
    Ok(())
}

// All activities in a (day, session), ordered by current sort_order.
// Returns (id, title) pairs.
async fn list_session_activities(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut rows = conn
        .query(
            "SELECT id, title FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4 \
             ORDER BY sort_order",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities list query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities list row read failed: {e}"))?
    {
        let id: String = row.get(0).map_err(|e| format!("id col read failed: {e}"))?;
        let title: Option<String> = row.get(1).ok();
        out.push((id, title.unwrap_or_default()));
    }
    Ok(out)
}

// Resolve a token to exactly one activity id from `current`: exact id match
// first, else case-insensitive title substring with a UNIQUE hit.
fn resolve_token(current: &[(String, String)], token: &str) -> Result<String, String> {
    if let Some((id, _)) = current.iter().find(|(id, _)| id == token) {
        return Ok(id.clone());
    }
    let needle = token.to_lowercase();
    let mut matches = current
        .iter()
        .filter(|(_, title)| title.to_lowercase().contains(&needle));
    let first = matches.next();
    match (first, matches.next()) {
        (None, _) => Err(format!("Activity not found for token: \"{token}\"")),
        (Some(_), Some(_)) => Err(format!(
            "Activity token \"{token}\" is ambiguous (matches multiple) — use a longer title or the id"
        )),
        (Some((id, _)), None) => Ok(id.clone()),
    }
}

fn first_line(s: &str) -> &str {
    s.split('\n').next().unwrap_or(s)
}

fn parse_reorder(args: &[String]) -> Result<ActivityReorder, String> {
    let mut p = ActivityReorder::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                p.dest = Some(arg_value(args, i, "--dest")?);
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
    if positional.len() < 3 {
        return Err(
            "Usage: reorder-activities <day> <session> <id-or-title> <id-or-title> ... [--dest <slug>]\n  (list ALL activities in the session, in the desired order)"
                .to_string(),
        );
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
    p.tokens = positional[2..].to_vec();
    Ok(p)
}

fn parse_delete(args: &[String]) -> Result<ActivityDelete, String> {
    let mut p = ActivityDelete::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                p.dest = Some(arg_value(args, i, "--dest")?);
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
    if positional.len() < 3 {
        return Err(
            "Usage: delete-activity <day> <session> <activity_id_or_title> [--dest <slug>]"
                .to_string(),
        );
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
    p.activity = positional[2].clone();
    Ok(p)
}

fn parse_booking(args: &[String]) -> Result<ActivityBooking, String> {
    let mut p = ActivityBooking::default();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                p.dest = Some(arg_value(args, i, "--dest")?);
                i += 2;
            }
            "--ref" => {
                p.booking_ref = Some(arg_value(args, i, "--ref")?);
                i += 2;
            }
            "--book-by" => {
                p.book_by = Some(arg_value(args, i, "--book-by")?);
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
    if positional.len() < 4 {
        return Err(
            "Usage: set-activity-booking <day> <session> <activity> <status> [--ref \"...\"] [--book-by YYYY-MM-DD] [--dest <slug>]"
                .to_string(),
        );
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
    p.activity = positional[2].clone();
    p.status = positional[3].clone();
    if !["not_required", "pending", "booked", "waitlist"].contains(&p.status.as_str()) {
        return Err(
            "<status> must be one of: not_required | pending | booked | waitlist".to_string(),
        );
    }
    if let Some(b) = &p.book_by {
        // Shared canonical ISO-date check (crate::checks). Passing "--book-by" as
        // the field name keeps the error strings byte-identical to the old local
        // copy ("--book-by must be YYYY-MM-DD format …" / "… is not a valid date").
        crate::checks::validate_iso_date(b, "--book-by")?;
    }
    Ok(p)
}

async fn read_booking_status(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT booking_status FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
               AND session_type = ?4 AND id = ?5",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                activity_id.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities booking_status query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities booking_status row read failed: {e}"))?
    {
        let s: Option<String> = row.get(0).ok();
        return Ok(s);
    }
    Err(format!("activity row disappeared: id={activity_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── HH:MM fail-loud validation (set-activity-time / add-activity) ──
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_time_accepts_valid_clocks() {
        let p = parse_time(&s(&["1", "morning", "checkout", "--start", "09:00", "--end", "11:30"]))
            .unwrap();
        assert_eq!(p.start_time.as_deref(), Some("09:00"));
        assert_eq!(p.end_time.as_deref(), Some("11:30"));
    }

    #[test]
    fn parse_time_rejects_bad_start() {
        let err = parse_time(&s(&["1", "morning", "checkout", "--start", "9am"])).unwrap_err();
        assert!(err.contains("--start") && err.contains("\"9am\""), "got: {err}");
        assert!(err.contains("HH:MM"), "got: {err}");
    }

    #[test]
    fn parse_time_rejects_bad_end() {
        let err = parse_time(&s(&["1", "morning", "checkout", "--end", "25:00"])).unwrap_err();
        assert!(err.contains("--end") && err.contains("\"25:00\""), "got: {err}");
    }

    #[test]
    fn parse_time_rejects_start_after_end() {
        let err =
            parse_time(&s(&["1", "morning", "checkout", "--start", "14:00", "--end", "09:00"]))
                .unwrap_err();
        assert!(err.contains("start must be"), "got: {err}");
    }

    #[test]
    fn parse_add_rejects_bad_start() {
        let err = parse_add(&s(&["3", "evening", "Stroll", "--start", "noon"])).unwrap_err();
        assert!(err.contains("--start") && err.contains("\"noon\""), "got: {err}");
    }

    #[test]
    fn parse_add_rejects_start_after_end() {
        let err =
            parse_add(&s(&["3", "evening", "Stroll", "--start", "20:00", "--end", "18:00"]))
                .unwrap_err();
        assert!(err.contains("start must be"), "got: {err}");
    }

    #[test]
    fn parse_add_accepts_valid_clocks() {
        let p =
            parse_add(&s(&["3", "evening", "Stroll", "--start", "18:00", "--end", "20:00"])).unwrap();
        assert_eq!(p.start_time.as_deref(), Some("18:00"));
        assert_eq!(p.end_time.as_deref(), Some("20:00"));
    }

    #[test]
    fn parse_bool_valid() {
        assert_eq!(parse_bool("true").unwrap(), true);
        assert_eq!(parse_bool("false").unwrap(), false);
        assert_eq!(parse_bool("YES").unwrap(), true);
        assert_eq!(parse_bool("0").unwrap(), false);
    }

    #[test]
    fn parse_bool_invalid() {
        assert!(parse_bool("nope").is_err());
    }

    #[test]
    fn render_optional_some() {
        assert_eq!(render_optional(&Some("11:00".to_string())), "11:00");
    }

    #[test]
    fn render_optional_none() {
        assert_eq!(render_optional(&None), "undefined");
    }

    #[test]
    fn render_optional_empty() {
        assert_eq!(render_optional(&Some(String::new())), "undefined");
    }

    #[test]
    fn parse_add_minimal() {
        let args: Vec<String> = ["3", "evening", "Kokusai-dori", "evening", "stroll"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let p = parse_add(&args).unwrap();
        assert_eq!(p.day, 3);
        assert_eq!(p.session, "evening");
        assert_eq!(p.title, "Kokusai-dori evening stroll");
        assert_eq!(p.priority, "want"); // default
        assert!(!p.is_fixed_time);
    }

    #[test]
    fn parse_add_with_flags() {
        let args: Vec<String> = [
            "1", "afternoon", "Naha", "Main", "Place",
            "--station", "Makishi",
            "--duration", "90",
            "--priority", "must",
            "--fixed", "true",
            "--dest", "okinawa_2026",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let p = parse_add(&args).unwrap();
        assert_eq!(p.title, "Naha Main Place");
        assert_eq!(p.nearest_station.as_deref(), Some("Makishi"));
        assert_eq!(p.duration_min, Some(90));
        assert_eq!(p.priority, "must");
        assert!(p.is_fixed_time);
        assert_eq!(p.dest.as_deref(), Some("okinawa_2026"));
    }

    #[test]
    fn parse_add_bad_session() {
        let args: Vec<String> = ["1", "lunchtime", "X"].iter().map(|s| s.to_string()).collect();
        assert!(parse_add(&args).is_err());
    }

    #[test]
    fn parse_add_bad_priority() {
        let args: Vec<String> = ["1", "noon", "X", "--priority", "critical"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(parse_add(&args).is_err());
    }

    #[test]
    fn parse_reorder_ok() {
        let args: Vec<String> = ["1", "morning", "abc", "Drive home", "CI120", "--dest", "okinawa_2026"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let p = parse_reorder(&args).unwrap();
        assert_eq!(p.day, 1);
        assert_eq!(p.session, "morning");
        assert_eq!(p.tokens, vec!["abc", "Drive home", "CI120"]);
        assert_eq!(p.dest.as_deref(), Some("okinawa_2026"));
    }

    #[test]
    fn parse_reorder_needs_tokens() {
        // day + session but no activity tokens
        let args: Vec<String> = ["1", "morning"].iter().map(|s| s.to_string()).collect();
        assert!(parse_reorder(&args).is_err());
    }

    #[test]
    fn parse_reorder_bad_session() {
        let args: Vec<String> = ["1", "lunch", "a", "b"].iter().map(|s| s.to_string()).collect();
        assert!(parse_reorder(&args).is_err());
    }

    #[test]
    fn resolve_token_by_id_and_title() {
        let cur = vec![
            ("id-1".to_string(), "Drive home to parking".to_string()),
            ("id-2".to_string(), "CI120 to Okinawa".to_string()),
        ];
        // exact id
        assert_eq!(resolve_token(&cur, "id-2").unwrap(), "id-2");
        // case-insensitive substring
        assert_eq!(resolve_token(&cur, "drive home").unwrap(), "id-1");
        // not found
        assert!(resolve_token(&cur, "shuttle").is_err());
    }

    #[test]
    fn resolve_token_ambiguous_is_error() {
        let cur = vec![
            ("id-1".to_string(), "Lunch at market".to_string()),
            ("id-2".to_string(), "Lunch at hotel".to_string()),
        ];
        assert!(resolve_token(&cur, "lunch").is_err());
    }
}
