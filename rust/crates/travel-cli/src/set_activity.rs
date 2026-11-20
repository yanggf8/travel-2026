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
    // Shared with `update-activity --title`: a row that already carries a trip
    // record keeps its title (the record would otherwise describe another place).
    if title_changed {
        travel_db::repo::records::ensure_title_editable(&conn, &plan_id, &destination, &activity_id)
            .await?;
    }
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
        title_changed,
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
            "note: cleared poi_id (was '{old_poi}') because the title changed — the POI link is no longer verified. If this is the same place, re-link with: {}",
            relink_hint(parsed.day, &parsed.session, &old_poi, &parsed.new_title, &plan_id, &destination)
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
            f if crate::plan_resolver::is_resolver_flag(f) => {
                // any plan-selection flag (--plan-id / --plan-path / --travel-date /
                // --travel-start / --travel-end) is consumed by the resolver; skip flag + value
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
            f if crate::plan_resolver::is_resolver_flag(f) => {
                // any plan-selection flag (--plan-id / --plan-path / --travel-date /
                // --travel-start / --travel-end) is consumed by the resolver; skip flag + value
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

pub(crate) async fn read_destination(
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

pub(crate) async fn touch_day(
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

pub(crate) async fn read_version(conn: &Connection, plan_id: &str) -> Result<i64, String> {
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

pub(crate) fn now_db_datetime() -> String {
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
    recommended: bool,
    requested_by: Option<String>,
    lat: Option<f64>,
    lon: Option<f64>,
    geo_source: Option<String>,
}

// ── delete-activity / remove-activity ──────────────────────────────
//
// Port of deleteActivityCommand: resolve the activity within (day,
// session) by id OR case-insensitive title substring, then DELETE the
// row (mirrors the `remove_activity` dispatch) and emit an
// `activity_removed` plan_event.
/// `set-activity-notes <day> <session> <id|title> --notes "<t>" | --clear [--dest slug]`
/// — change one activity's notes IN PLACE (id, poi_id and every other column
/// untouched). Unique text/id match (ambiguous → Err listing candidates);
/// works on variant days too (editing alternatives is normal planning).
/// The edit that used to force delete-and-recreate, churning activity ids.
pub async fn run_set_notes(args: &[String], plan_id: String) -> Result<(), String> {
    const USAGE: &str =
        "travel set-activity-notes <day> <session> <id|title> --notes \"<text>\" | --clear [--dest <slug>]";
    let mut dest: Option<String> = None;
    let mut notes: Option<String> = None;
    let mut clear = false;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                dest = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --dest".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--notes" => {
                notes = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --notes".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--clear" => {
                clear = true;
                i += 1;
            }
            f if crate::plan_resolver::is_resolver_flag(f) => i += 2,
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}. Usage:\n{USAGE}"));
            }
            _ => {
                positional.push(a.clone());
                i += 1;
            }
        }
    }
    if positional.len() != 3 {
        return Err(format!("Usage:\n{USAGE}"));
    }
    let day = positional[0]
        .parse::<i64>()
        .map_err(|_| format!("<day> must be a positive integer. Usage:\n{USAGE}"))?;
    if day < 1 {
        return Err(format!("<day> must be a positive integer. Usage:\n{USAGE}"));
    }
    let session = positional[1].clone();
    if !["morning", "noon", "afternoon", "evening"].contains(&session.as_str()) {
        return Err("<session> must be one of: morning|noon|afternoon|evening".to_string());
    }
    if notes.is_none() && !clear {
        return Err(format!("pass --notes \"<text>\" or --clear. Usage:\n{USAGE}"));
    }
    if notes.is_some() && clear {
        return Err(format!("--notes and --clear are mutually exclusive. Usage:\n{USAGE}"));
    }
    let key = positional[2].clone();
    let value = if clear { None } else { notes.clone() };

    let conn = crate::db::connect_write().await?;
    let destination = read_destination(&conn, &plan_id, &dest).await?;
    let activity_id = travel_db::repo::records::match_activity_unique(
        &conn, &plan_id, &destination, day, &session, &key,
    )
    .await?;

    println!(
        "\n📝 Setting activity notes: D{day}/{session} {key} → {}",
        if clear { "(clear)".to_string() } else { value.clone().unwrap_or_default() }
    );

    let outcome = async {
        crate::cascade::common::begin_immediate(&conn, "set-activity-notes").await?;
        let version_before = read_version(&conn, &plan_id).await?;
        itinerary::update_activity_field(
            &conn,
            &plan_id,
            &destination,
            day,
            &session,
            &activity_id,
            "notes",
            value.clone(),
            &now_db_datetime(),
        )
        .await?;
        touch_day(&conn, &plan_id, &destination, day).await?;
        crate::record::emit_record_event(
            &conn,
            &plan_id,
            &destination,
            "activity_notes_set",
            &[
                ("activity_id", activity_id.clone()),
                ("day_number", day.to_string()),
                ("session", session.clone()),
                (
                    "notes",
                    value.clone().unwrap_or_else(|| "undefined".to_string()),
                ),
            ],
            version_before,
            "set-activity-notes",
            &format!("D{day}/{session} {activity_id} notes set"),
        )
        .await?;
        Ok(())
    }
    .await;
    crate::cascade::common::finish_tx(&conn, "set-activity-notes", outcome).await?;
    println!("✅ Notes updated (id preserved: {activity_id})");
    Ok(())
}

// ── update-activity ────────────────────────────────────────────────
pub(crate) const UPDATE_USAGE: &str = "travel update-activity <day> <session> <id|title> \
[--title <t>] [--area <t>] [--station <t>] [--duration <min>] [--start HH:MM] [--end HH:MM] \
[--fixed true|false] [--priority must|want|optional] [--notes <t> | --append-notes <t>] \
[--requested-by <name>] \
[--clear area,station,duration,start,end,notes,requested_by] [--dest <slug>]\n  \
(--notes REPLACES the whole note; --append-notes adds a line without overwriting)\n  \
(edit one activity in place by unique id/title match; only the given fields change, id/poi_id/\
booking/record untouched; same values = no change, no version bump — audited)";

#[derive(Debug)]
struct ActivityUpdate {
    day: i64,
    session: String,
    key: String,
    patch: itinerary::ActivityPatch,
    dest: Option<String>,
}

fn nonempty(flag: &str, v: String) -> Result<String, String> {
    if v.trim().is_empty() {
        return Err(format!(
            "{flag} cannot be empty (use --clear to unset a nullable field)"
        ));
    }
    Ok(v)
}

/// `--requested-by` / add-activity `--requested-by`: who asked for this stop. 1-16
/// characters after trimming (Unicode chars, not bytes); no newline, `http`, `<`, `>`.
/// Too long FAILS — never truncated (a clipped name publishes a name nobody gave).
pub(crate) fn validate_requested_by(v: &str) -> Result<String, String> {
    let t = v.trim();
    let n = t.chars().count();
    if n == 0 {
        return Err("--requested-by cannot be empty (use --clear requested_by to unset)".to_string());
    }
    if n > 16 {
        return Err(format!(
            "--requested-by must be 1-16 characters (got {n}); use a short name, not a sentence"
        ));
    }
    if t.contains('\n') || t.contains('\r') || t.contains('<') || t.contains('>') || t.to_lowercase().contains("http") {
        return Err("--requested-by must be a plain name (no newline, '<', '>' or URL)".to_string());
    }
    Ok(t.to_string())
}

fn parse_update(args: &[String]) -> Result<ActivityUpdate, String> {
    let mut patch = itinerary::ActivityPatch::default();
    let mut dest: Option<String> = None;
    let mut clear: Vec<String> = Vec::new();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                dest = Some(arg_value(args, i, "--dest")?);
                i += 2;
            }
            "--title" => {
                patch.title = Some(nonempty("--title", arg_value(args, i, "--title")?)?);
                i += 2;
            }
            "--area" => {
                patch.area = Some(Some(nonempty("--area", arg_value(args, i, "--area")?)?));
                i += 2;
            }
            "--station" => {
                patch.nearest_station =
                    Some(Some(nonempty("--station", arg_value(args, i, "--station")?)?));
                i += 2;
            }
            "--duration" => {
                let v = arg_value(args, i, "--duration")?;
                let n = v
                    .parse::<i64>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| "--duration must be a positive integer (minutes)".to_string())?;
                patch.duration_min = Some(Some(n));
                i += 2;
            }
            "--start" => {
                let v = arg_value(args, i, "--start")?;
                crate::checks::validate_time_flag("--start", &v)?;
                patch.start_time = Some(Some(v));
                i += 2;
            }
            "--end" => {
                let v = arg_value(args, i, "--end")?;
                crate::checks::validate_time_flag("--end", &v)?;
                patch.end_time = Some(Some(v));
                i += 2;
            }
            "--fixed" => {
                patch.is_fixed_time = Some(parse_bool(&arg_value(args, i, "--fixed")?)?);
                i += 2;
            }
            "--priority" => {
                let v = arg_value(args, i, "--priority")?;
                if !["must", "want", "optional"].contains(&v.as_str()) {
                    return Err(format!("Invalid --priority: {v} (use must|want|optional)"));
                }
                patch.priority = Some(v);
                i += 2;
            }
            "--notes" => {
                patch.notes = Some(Some(nonempty("--notes", arg_value(args, i, "--notes")?)?));
                i += 2;
            }
            "--append-notes" => {
                patch.append_notes =
                    Some(nonempty("--append-notes", arg_value(args, i, "--append-notes")?)?);
                i += 2;
            }
            "--requested-by" => {
                patch.requested_by = Some(Some(validate_requested_by(&arg_value(
                    args,
                    i,
                    "--requested-by",
                )?)?));
                i += 2;
            }
            "--clear" => {
                let v = arg_value(args, i, "--clear")?;
                for f in v.split(',') {
                    clear.push(f.trim().to_string());
                }
                i += 2;
            }
            f if crate::plan_resolver::is_resolver_flag(f) => i += 2,
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}. Usage:\n{UPDATE_USAGE}"));
            }
            _ => {
                positional.push(a.clone());
                i += 1;
            }
        }
    }
    if positional.len() != 3 {
        return Err(format!("Usage:\n{UPDATE_USAGE}"));
    }
    if patch.append_notes.is_some() && patch.notes.is_some() {
        return Err("--append-notes and --notes conflict (replace vs append) — pass one".to_string());
    }
    let mut seen: Vec<&String> = Vec::new();
    for f in &clear {
        if seen.contains(&f) {
            continue; // `--clear start,start` is one clear, not a conflict
        }
        seen.push(f);
        // Only nullable columns can be cleared; a value flag for the same field conflicts.
        let (given, slot) = match f.as_str() {
            "area" => (patch.area.is_some(), &mut patch.area),
            "station" => (patch.nearest_station.is_some(), &mut patch.nearest_station),
            "start" => (patch.start_time.is_some(), &mut patch.start_time),
            "end" => (patch.end_time.is_some(), &mut patch.end_time),
            "notes" => {
                if patch.append_notes.is_some() {
                    return Err("--append-notes and --clear notes conflict".to_string());
                }
                (patch.notes.is_some(), &mut patch.notes)
            }
            "requested_by" => (patch.requested_by.is_some(), &mut patch.requested_by),
            "duration" => {
                if patch.duration_min.is_some() {
                    return Err("--duration and --clear duration conflict".to_string());
                }
                patch.duration_min = Some(None);
                continue;
            }
            other => {
                return Err(format!(
                    "--clear accepts only area,station,duration,start,end,notes,requested_by (got \"{other}\"; \
                     title/priority/fixed cannot be NULL)"
                ));
            }
        };
        if given {
            return Err(format!("--{f} and --clear {f} conflict"));
        }
        *slot = Some(None);
    }
    if patch.is_empty() {
        return Err(format!(
            "pass at least one field flag or --clear. Usage:\n{UPDATE_USAGE}"
        ));
    }
    let day = positional[0]
        .parse::<i64>()
        .ok()
        .filter(|d| *d >= 1)
        .ok_or_else(|| format!("<day> must be a positive integer. Usage:\n{UPDATE_USAGE}"))?;
    let session = positional[1].clone();
    if !["morning", "noon", "afternoon", "evening"].contains(&session.as_str()) {
        return Err("<session> must be one of: morning|noon|afternoon|evening".to_string());
    }
    Ok(ActivityUpdate {
        day,
        session,
        key: positional[2].clone(),
        patch,
        dest,
    })
}

/// Validation of the MERGED row (what the DB would hold after the patch). Only what the
/// user touched is judged: a given title must be map-URL-clean, and when either time
/// endpoint was given the pair must be valid HH:MM with `start <= end` — a dirty stored
/// value on the other side fails loud instead of being skipped.
fn check_merged_activity(
    patch: &itinerary::ActivityPatch,
    row: &itinerary::ActivityRow,
) -> Result<(), String> {
    if patch.title.is_some() {
        crate::checks::check_title_map_url(&row.title)?;
    }
    let touched = matches!(patch.start_time, Some(Some(_))) || matches!(patch.end_time, Some(Some(_)));
    if touched {
        if let (Some(st), Some(en)) = (&row.start_time, &row.end_time) {
            crate::checks::validate_time_flag("start_time (stored)", st)?;
            crate::checks::validate_time_flag("end_time (stored)", en)?;
            crate::checks::validate_start_le_end("start_time", st, "end_time", en)?;
        }
    }
    Ok(())
}

/// Copy-pasteable re-link command: pins the row with `--match` (the CURRENT title —
/// several activities may share the session) and names plan + destination.
fn relink_hint(day: i64, session: &str, poi_id: &str, title: &str, plan_id: &str, dest: &str) -> String {
    let q = title.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "travel set-activity-poi {day} {session} {poi_id} --match \"{q}\" --plan-id {plan_id} --dest {dest}"
    )
}

fn shown(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "(none)".to_string())
}

/// `update-activity` — edit one activity IN PLACE (spec 2026-11-19 §1).
pub async fn run_update(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_update(args)?;
    let conn = crate::db::connect_write().await?;
    let destination = read_destination(&conn, &plan_id, &parsed.dest).await?;
    let (day, session) = (parsed.day, parsed.session.clone());
    let mut activity_id = String::new();

    // Match, read, validate and write all happen INSIDE the write transaction, so a
    // concurrent move/delete cannot slip between the match and the UPDATE.
    let outcome = async {
        crate::cascade::common::begin_immediate(&conn, "update-activity").await?;
        activity_id = travel_db::repo::records::match_activity_unique(
            &conn,
            &plan_id,
            &destination,
            day,
            &session,
            &parsed.key,
        )
        .await?;
        let before =
            itinerary::read_activity_row(&conn, &plan_id, &destination, day, &session, &activity_id)
                .await?
                .ok_or_else(|| format!("activity {activity_id} not found"))?;
        let mut after = parsed.patch.apply(&before)?;
        let title_changed = after.title != before.title;
        let stale_poi: Option<String> = before
            .poi_id
            .clone()
            .filter(|p| !p.is_empty())
            .filter(|_| title_changed);
        if stale_poi.is_some() {
            after.poi_id = None;
        }
        let diff = itinerary::activity_row_diff(&before, &after);
        // Same values as stored: "No change" comes BEFORE any validation of the row.
        if diff.is_empty() {
            return Ok(None);
        }
        // Safety net: `--notes` REPLACES. Say so when real prior text is being dropped.
        let overwrite_warn = match (&parsed.patch.notes, before.notes.as_deref()) {
            (Some(Some(new)), Some(old))
                if !old.trim().is_empty() && !new.contains(old.trim()) =>
            {
                Some(format!(
                    "warn: 取代了 {} 字備註；舊內容保留在 plan_event_data from.notes（或改用 --append-notes）",
                    old.chars().count()
                ))
            }
            _ => None,
        };
        // Title edits never clear a pin (spec Q1) — but say so, with the coordinates.
        let pin_note = if title_changed {
            itinerary::read_activity_geo(&conn, &plan_id, &destination, day, &session, &activity_id)
                .await?
                .map(|g| {
                    format!(
                        "note: 活動已有座標 ({},{})，標題改變不會清除；地點若換了請 set-activity-geocode",
                        g.lat, g.lon
                    )
                })
        } else {
            None
        };
        check_merged_activity(&parsed.patch, &after)?;
        if title_changed {
            travel_db::repo::records::ensure_title_editable(
                &conn,
                &plan_id,
                &destination,
                &activity_id,
            )
            .await?;
        }
        let version_before = read_version(&conn, &plan_id).await?;
        itinerary::update_activity_fields(
            &conn,
            &plan_id,
            &destination,
            day,
            &session,
            &activity_id,
            &after,
            &now_db_datetime(),
        )
        .await?;
        touch_day(&conn, &plan_id, &destination, day).await?;
        let changed: Vec<&str> = diff.iter().map(|(c, _, _)| *c).collect();
        let mut kv: Vec<(String, String)> = vec![
            ("day_number".into(), day.to_string()),
            ("session".into(), session.clone()),
            ("activity_id".into(), activity_id.clone()),
            ("title".into(), after.title.clone()),
            ("changed".into(), changed.join(",")),
        ];
        let undef = |v: &Option<String>| v.clone().unwrap_or_else(|| "undefined".to_string());
        for (col, from, to) in &diff {
            kv.push((format!("from.{col}"), undef(from)));
            kv.push((format!("to.{col}"), undef(to)));
        }
        if parsed.patch.append_notes.is_some() {
            kv.push(("notes_mode".into(), "append".into()));
        }
        let kv_ref: Vec<(&str, String)> = kv.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        crate::record::emit_record_event(
            &conn,
            &plan_id,
            &destination,
            "activity_updated",
            &kv_ref,
            version_before,
            "update-activity",
            &format!("D{day}/{session} {activity_id} updated: {}", changed.join(",")),
        )
        .await?;
        Ok(Some((
            before.title.clone(),
            after.title.clone(),
            diff,
            stale_poi,
            overwrite_warn,
            pin_note,
        )))
    }
    .await;
    let result = crate::cascade::common::finish_tx(&conn, "update-activity", outcome).await?;
    match result {
        None => println!("ℹ️ No change (id={activity_id})"),
        Some((old_title, new_title, diff, stale_poi, overwrite_warn, pin_note)) => {
            println!("\n📝 Updating activity: D{day}/{session} {old_title}");
            for (col, from, to) in &diff {
                println!("   {col}: {} → {}", shown(from), shown(to));
            }
            println!("✅ Activity updated (id preserved: {activity_id})");
            if let Some(w) = overwrite_warn {
                println!("{w}");
            }
            if let Some(n) = pin_note {
                println!("{n}");
            }
            if let Some(old_poi) = stale_poi {
                println!(
                    "note: cleared poi_id (was '{old_poi}') because the title changed — the POI link is no longer verified. If this is the same place, re-link with: {}",
                    relink_hint(day, &session, &old_poi, &new_title, &plan_id, &destination)
                );
            }
        }
    }
    Ok(())
}

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

    // Match, validate and every write run in ONE transaction: a title matching several
    // activities FAILS (candidates listed) instead of silently moving the first one,
    // and a failing event write rolls the re-key back.
    let outcome = async {
        crate::cascade::common::begin_immediate(&conn, "move-activity").await?;
        let activity_id = travel_db::repo::records::match_activity_unique(
            &conn,
            &plan_id,
            &destination,
            from_day,
            &parsed.from_session,
            &parsed.activity,
        )
        .await?;
        let title = read_activity_title(
            &conn,
            &plan_id,
            &destination,
            from_day,
            &parsed.from_session,
            &activity_id,
        )
        .await?;

        // Verify the target day/session exists (fail loud, like the set-* writers).
        require_session_exists(&conn, &plan_id, &destination, to_day, &parsed.to_session).await?;

        let new_sort =
            next_activity_sort_order(&conn, &plan_id, &destination, to_day, &parsed.to_session)
                .await?;

        // Re-key the row by id; every other column (poi_id, booking_*, lat/lon,
        // requested_by, tags via activity_tags keyed on activity_id) is untouched.
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
        Ok((activity_id, title))
    }
    .await;
    let (activity_id, title) =
        crate::cascade::common::finish_tx(&conn, "move-activity", outcome).await?;

    println!("\n↔️  Moving activity:");
    println!(
        "   D{} {} → D{} {}: \"{}\"",
        from_day, parsed.from_session, to_day, parsed.to_session, title
    );
    println!("✅ Activity moved (id preserved: {activity_id})");
    println!("note: move-activity only appends to the END of the target session; to set a position use reorder-activities");
    Ok(())
}

#[derive(Debug)]
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
            f if crate::plan_resolver::is_resolver_flag(f) => {
                // any plan-selection flag (--plan-id / --plan-path / --travel-date /
                // --travel-start / --travel-end) is consumed by the resolver; skip flag + value
                i += 2;
            }
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
        if parsed.recommended {
            "ai_recommended"
        } else {
            "confirmed"
        },
        &now_db_datetime(),
        parsed.requested_by.clone(),
        match (parsed.lat, parsed.lon) {
            (Some(lat), Some(lon)) => Some(itinerary::ActivityGeo {
                lat,
                lon,
                source: parsed.geo_source.clone(),
            }),
            _ => None,
        },
    )
    .await?;
    touch_day(&conn, &plan_id, &destination, parsed.day).await?;

    let mut kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        ("activity_id", activity_id.clone()),
        ("title", parsed.title.clone()),
        ("sort_order", sort_order.to_string()),
    ];
    if let Some(r) = &parsed.requested_by {
        kv.push(("requested_by", r.clone()));
    }
    if let (Some(lat), Some(lon)) = (parsed.lat, parsed.lon) {
        kv.push(("lat", lat.to_string()));
        kv.push(("lon", lon.to_string()));
    }

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
    println!(
        "Next: run `derive-routes --day {} --dest {destination}` to cascade transit between the day's activities.",
        parsed.day
    );

    // Post-add POI hint (read-only): if the new title UNAMBIGUOUSLY matches one
    // GEOCODED destination_pois row (same rule as `set-activity-poi --auto`),
    // nudge the agent to link it so the day's map pin renders. Silent on
    // no-match / ambiguous / ungeocoded, and on any lookup error — the add has
    // already succeeded and this must never fail the command.
    if let Ok(pois) =
        crate::set_activity_poi::list_destination_pois_for_auto(&conn, &destination).await
    {
        if let crate::set_activity_poi::AutoMatch::Link(poi_id) =
            crate::set_activity_poi::resolve_auto_match(&parsed.title, &pois)
        {
            println!(
                "💡 matches POI '{poi_id}' — link it for a map pin: set-activity-poi {} {} {poi_id}",
                parsed.day, parsed.session
            );
        }
    }

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
            "--recommended" => {
                p.recommended = true;
                i += 1;
            }
            "--requested-by" => {
                p.requested_by = Some(validate_requested_by(&arg_value(args, i, "--requested-by")?)?);
                i += 2;
            }
            "--lat" => {
                let v = arg_value(args, i, "--lat")?;
                p.lat = Some(v.parse::<f64>().map_err(|_| format!("--lat must be a number (got '{v}')"))?);
                i += 2;
            }
            "--lon" => {
                let v = arg_value(args, i, "--lon")?;
                p.lon = Some(v.parse::<f64>().map_err(|_| format!("--lon must be a number (got '{v}')"))?);
                i += 2;
            }
            "--source" => {
                p.geo_source = Some(nonempty("--source", arg_value(args, i, "--source")?)?);
                i += 2;
            }
            f if crate::plan_resolver::is_resolver_flag(f) => {
                // any plan-selection flag (--plan-id / --plan-path / --travel-date /
                // --travel-start / --travel-end) is consumed by the resolver; skip flag + value
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
            "Usage: add-activity <day> <session> <title> [--after <id|title>] [--recommended] [--area ..] [--station ..] [--duration MIN] [--start HH:MM] [--end HH:MM] [--fixed true|false] [--priority must|want|optional] [--notes ..] [--requested-by <name>] [--lat <f> --lon <f> [--source <t>]] [--dest <slug>]"
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
    // Pin coordinates move as a pair and pass the shared validator (range, finite,
    // not null island); --source alone is meaningless.
    match (p.lat, p.lon) {
        (Some(lat), Some(lon)) => {
            crate::checks::validate_coords(lat, lon)?;
        }
        (None, None) => {
            if p.geo_source.is_some() {
                return Err("--source needs --lat/--lon".to_string());
            }
        }
        _ => return Err("--lat and --lon must be given together".to_string()),
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
            f if crate::plan_resolver::is_resolver_flag(f) => {
                // any plan-selection flag (--plan-id / --plan-path / --travel-date /
                // --travel-start / --travel-end) is consumed by the resolver; skip flag + value
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
            f if crate::plan_resolver::is_resolver_flag(f) => {
                // any plan-selection flag (--plan-id / --plan-path / --travel-date /
                // --travel-start / --travel-end) is consumed by the resolver; skip flag + value
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
            f if crate::plan_resolver::is_resolver_flag(f) => {
                // any plan-selection flag (--plan-id / --plan-path / --travel-date /
                // --travel-start / --travel-end) is consumed by the resolver; skip flag + value
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

    // ── update-activity parse / merge ──
    fn upd(v: &[&str]) -> Result<ActivityUpdate, String> {
        let mut a = vec!["2", "morning", "鼻頭角"];
        a.extend_from_slice(v);
        parse_update(&s(&a))
    }

    #[test]
    fn requested_by_validation_boundaries() {
        assert_eq!(validate_requested_by("  Ping ").unwrap(), "Ping");
        // exactly 16 chars passes (CJK and emoji count as chars, not bytes)
        let sixteen = "指".repeat(16);
        assert_eq!(validate_requested_by(&sixteen).unwrap(), sixteen);
        let emoji16 = "😀".repeat(16);
        assert!(validate_requested_by(&emoji16).is_ok());
        // 17 FAILS — never truncated
        assert!(validate_requested_by(&"指".repeat(17)).unwrap_err().contains("1-16"));
        assert!(validate_requested_by("   ").unwrap_err().contains("empty"));
        for bad in ["a\nb", "a<b", "a>b", "see http://x", "HTTPS"] {
            assert!(validate_requested_by(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parse_update_append_notes_and_requested_by() {
        let u = upd(&["--append-notes", "停車 50 元"]).unwrap();
        assert_eq!(u.patch.append_notes.as_deref(), Some("停車 50 元"));
        assert!(upd(&["--append-notes", "  "]).is_err());
        // mutually exclusive with --notes and --clear notes (either order)
        assert!(upd(&["--append-notes", "a", "--notes", "b"]).unwrap_err().contains("conflict"));
        assert!(upd(&["--notes", "b", "--append-notes", "a"]).unwrap_err().contains("conflict"));
        assert!(upd(&["--append-notes", "a", "--clear", "notes"]).unwrap_err().contains("conflict"));
        // requested_by: set, clear, value+clear conflict, too long
        let r = upd(&["--requested-by", "Ping"]).unwrap();
        assert_eq!(r.patch.requested_by, Some(Some("Ping".to_string())));
        let c = upd(&["--clear", "requested_by"]).unwrap();
        assert_eq!(c.patch.requested_by, Some(None));
        assert!(upd(&["--requested-by", "Ping", "--clear", "requested_by"]).unwrap_err().contains("conflict"));
        assert!(upd(&["--requested-by", &"x".repeat(17)]).is_err());
    }

    #[test]
    fn parse_add_new_flags() {
        let base = ["1", "morning", "川岩"];
        let add = |extra: &[&str]| {
            let mut v: Vec<&str> = base.to_vec();
            v.extend_from_slice(extra);
            parse_add(&s(&v))
        };
        let p = add(&["--requested-by", "Yang", "--lat", "25.1", "--lon", "121.8", "--source", "osm"]).unwrap();
        assert_eq!(p.requested_by.as_deref(), Some("Yang"));
        assert_eq!((p.lat, p.lon), (Some(25.1), Some(121.8)));
        assert_eq!(p.geo_source.as_deref(), Some("osm"));
        assert!(add(&["--lat", "25.1"]).unwrap_err().contains("together"));
        assert!(add(&["--lon", "121.8"]).unwrap_err().contains("together"));
        assert!(add(&["--source", "osm"]).unwrap_err().contains("--lat/--lon"));
        assert!(add(&["--lat", "0", "--lon", "0"]).unwrap_err().contains("null island"));
        assert!(add(&["--lat", "95", "--lon", "1"]).unwrap_err().contains("between"));
        assert!(add(&["--requested-by", &"x".repeat(17)]).is_err());
    }

    #[test]
    fn parse_move_still_rejects_position_flags() {
        // move-activity only appends; position is reorder-activities' job.
        assert!(parse_move(&s(&["1", "morning", "noon", "x", "--after", "y"])).unwrap_err().contains("unknown argument"));
        assert!(parse_move(&s(&["1", "morning", "noon", "x", "--first"])).is_err());
    }

    #[test]
    fn parse_update_sets_only_given_fields() {
        let u = upd(&["--duration", "45", "--area", "鼻頭", "--fixed", "true", "--plan-id", "p"]).unwrap();
        assert_eq!(u.day, 2);
        assert_eq!(u.key, "鼻頭角");
        assert_eq!(u.patch.duration_min, Some(Some(45)));
        assert_eq!(u.patch.area, Some(Some("鼻頭".to_string())));
        assert_eq!(u.patch.is_fixed_time, Some(true));
        assert_eq!(u.patch.notes, None);
        assert_eq!(u.patch.start_time, None);
        assert_eq!(u.patch.title, None);
    }

    #[test]
    fn parse_update_clear_whitelist_and_nulls() {
        let u = upd(&["--clear", "area, station,duration,start,end,notes"]).unwrap();
        assert_eq!(u.patch.area, Some(None));
        assert_eq!(u.patch.nearest_station, Some(None));
        assert_eq!(u.patch.duration_min, Some(None));
        assert_eq!(u.patch.start_time, Some(None));
        assert_eq!(u.patch.end_time, Some(None));
        assert_eq!(u.patch.notes, Some(None));
        for bad in ["title", "priority", "fixed", "nonsense"] {
            let e = upd(&["--clear", bad]).unwrap_err();
            assert!(e.contains("--clear accepts only"), "{bad}: {e}");
        }
    }

    #[test]
    fn parse_update_value_and_clear_same_field_conflict() {
        assert!(upd(&["--area", "x", "--clear", "area"]).unwrap_err().contains("conflict"));
        assert!(upd(&["--clear", "start", "--start", "09:00"]).unwrap_err().contains("conflict"));
        assert!(upd(&["--duration", "5", "--clear", "duration"]).unwrap_err().contains("conflict"));
    }

    #[test]
    fn parse_update_requires_a_field_and_valid_values() {
        assert!(upd(&[]).unwrap_err().contains("at least one"));
        assert!(upd(&["--duration", "0"]).unwrap_err().contains("positive"));
        assert!(upd(&["--duration", "-5"]).unwrap_err().contains("positive"));
        assert!(upd(&["--duration", "abc"]).unwrap_err().contains("positive"));
        assert!(upd(&["--priority", "critical"]).unwrap_err().contains("must|want|optional"));
        assert!(upd(&["--start", "9am"]).unwrap_err().contains("HH:MM"));
        assert!(upd(&["--title", "  "]).unwrap_err().contains("cannot be empty"));
        assert!(upd(&["--area", ""]).unwrap_err().contains("--clear"));
        assert!(upd(&["--bogus", "x"]).unwrap_err().contains("unknown argument"));
        assert!(parse_update(&s(&["0", "morning", "x", "--area", "a"])).is_err());
        assert!(parse_update(&s(&["1", "dawn", "x", "--area", "a"])).is_err());
        assert!(parse_update(&s(&["1", "morning", "--area", "a"])).is_err());
    }

    #[test]
    fn merged_time_order_uses_stored_value_for_the_missing_side() {
        let stored = itinerary::ActivityRow {
            title: "t".into(),
            area: None,
            nearest_station: None,
            duration_min: None,
            start_time: Some("10:00".into()),
            end_time: None,
            is_fixed_time: false,
            priority: "want".into(),
            notes: None,
            poi_id: None,
            requested_by: None,
        };
        // only --end given, earlier than the stored start
        let p = upd(&["--end", "09:00"]).unwrap().patch;
        assert!(check_merged_activity(&p, &p.apply(&stored).unwrap()).unwrap_err().contains("after"));
        let p = upd(&["--end", "10:00"]).unwrap().patch;
        assert!(check_merged_activity(&p, &p.apply(&stored).unwrap()).is_ok());
        // clearing start makes any end fine
        let p = upd(&["--clear", "start", "--end", "09:00"]).unwrap().patch;
        assert!(check_merged_activity(&p, &p.apply(&stored).unwrap()).is_ok());
        // a dirty STORED endpoint fails loud when the user touches the other one
        let mut dirty = stored.clone();
        dirty.start_time = Some("9am".into());
        let p = upd(&["--end", "08:00"]).unwrap().patch;
        assert!(check_merged_activity(&p, &p.apply(&dirty).unwrap()).unwrap_err().contains("not a valid HH:MM"));
        // ...but an untouched pair is not judged (duration-only edit on dirty data)
        let p = upd(&["--duration", "5"]).unwrap().patch;
        assert!(check_merged_activity(&p, &p.apply(&dirty).unwrap()).is_ok());
    }

    #[test]
    fn parse_update_clear_duplicates_are_one_clear() {
        let u = upd(&["--clear", "start,start"]).unwrap();
        assert_eq!(u.patch.start_time, Some(None));
        let u = upd(&["--clear", "area", "--clear", "area,notes"]).unwrap();
        assert_eq!(u.patch.area, Some(None));
        assert_eq!(u.patch.notes, Some(None));
        // a real value flag + clear is still a conflict
        assert!(upd(&["--clear", "start,start", "--start", "09:00"]).unwrap_err().contains("conflict"));
    }

    #[test]
    fn relink_hint_is_copy_pasteable() {
        let h = relink_hint(2, "morning", "poi_x", "鼻頭角 \"北\"", "ne-coast-2026", "ne_coast_2026");
        assert_eq!(
            h,
            "travel set-activity-poi 2 morning poi_x --match \"鼻頭角 \\\"北\\\"\" --plan-id ne-coast-2026 --dest ne_coast_2026"
        );
    }


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
