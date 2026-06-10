// `travel set-activity-time` and `travel set-activity-title` —
// port of src/cli/commands/activity.ts. NO CASCADE.
//
// Both commands look up an activity within a (day, session) by id or
// by case-insensitive title substring (matches the TS
// `findActivityIndex` behavior), then UPDATE the activities row +
// emit a plan_event.

use libsql::Connection;

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

    conn.execute(
        "UPDATE activities SET title = ?1, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 \
           AND session_type = ?6 AND id = ?7",
        libsql::params![
            parsed.new_title.clone(),
            now_db_datetime(),
            plan_id.to_string(),
            destination.to_string(),
            parsed.day,
            parsed.session.clone(),
            activity_id.clone()
        ],
    )
    .await
    .map_err(|e| format!("activities UPDATE title failed: {e}"))?;
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
    Err(format!(
        "plan_metadata row missing for plan_id={plan_id}"
    ))
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
    let sql = format!(
        "UPDATE activities SET {field} = ?1, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 \
           AND session_type = ?6 AND id = ?7"
    );
    conn.execute(
        &sql,
        libsql::params![
            value,
            now_db_datetime(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string(),
            activity_id.to_string()
        ],
    )
    .await
    .map_err(|e| format!("activities UPDATE {field} failed: {e}"))?;
    Ok(())
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

    conn.execute(
        "DELETE FROM activities \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
           AND session_type = ?4 AND id = ?5",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            parsed.day,
            parsed.session.clone(),
            activity_id.clone()
        ],
    )
    .await
    .map_err(|e| format!("activities DELETE failed: {e}"))?;
    // Clean up any tag child rows for the removed activity.
    conn.execute(
        "DELETE FROM activity_tags WHERE activity_id = ?1",
        libsql::params![activity_id.clone()],
    )
    .await
    .map_err(|e| format!("activity_tags DELETE failed: {e}"))?;
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
    let sort_order =
        next_activity_sort_order(&conn, &plan_id, &destination, parsed.day, &parsed.session).await?;

    conn.execute(
        "INSERT INTO activities \
            (id, plan_id, destination, day_number, session_type, sort_order, \
             title, area, nearest_station, duration_min, start_time, end_time, \
             is_fixed_time, priority, notes, booking_required, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 0, ?16)",
        libsql::params![
            activity_id.clone(),
            plan_id.to_string(),
            destination.to_string(),
            parsed.day,
            parsed.session.clone(),
            sort_order,
            parsed.title.clone(),
            parsed.area.clone(),
            parsed.nearest_station.clone(),
            parsed.duration_min,
            parsed.start_time.clone(),
            parsed.end_time.clone(),
            if parsed.is_fixed_time { 1_i64 } else { 0_i64 },
            parsed.priority.clone(),
            parsed.notes.clone(),
            now_db_datetime()
        ],
    )
    .await
    .map_err(|e| format!("activities INSERT failed: {e}"))?;
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
            "Usage: add-activity <day> <session> <title> [--area ..] [--station ..] [--duration MIN] [--start HH:MM] [--end HH:MM] [--fixed true|false] [--priority must|want|optional] [--notes ..] [--dest <slug>]"
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

async fn next_activity_sort_order(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities MAX(sort_order) query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities MAX(sort_order) row read failed: {e}"))?
    {
        let m: i64 = row
            .get(0)
            .map_err(|e| format!("activities MAX(sort_order) col read failed: {e}"))?;
        return Ok(m + 1);
    }
    Ok(0)
}

// UUIDv4-shaped id for a new activity (same generator the audit run_id uses).
fn new_activity_id() -> String {
    new_run_id()
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
        validate_iso_date(b)?;
    }
    Ok(p)
}

// Mirrors validateIsoDate: strict YYYY-MM-DD with a real-date check.
fn validate_iso_date(input: &str) -> Result<(), String> {
    let bytes = input.as_bytes();
    let valid_shape = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && input[0..4].chars().all(|c| c.is_ascii_digit())
        && input[5..7].chars().all(|c| c.is_ascii_digit())
        && input[8..10].chars().all(|c| c.is_ascii_digit());
    if !valid_shape {
        return Err(format!("--book-by must be YYYY-MM-DD format (got: \"{input}\")"));
    }
    let year: i32 = input[0..4].parse().unwrap();
    let month: u32 = input[5..7].parse().unwrap();
    let day: u32 = input[8..10].parse().unwrap();
    if month < 1 || month > 12 || day < 1 || day > days_in_month(year, month) {
        return Err(format!("--book-by is not a valid date: \"{input}\""));
    }
    Ok(())
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
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
}
