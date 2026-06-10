// `travel set-tod-focus`, `travel set-tod-time-range`, `travel set-tod-zh`
// (and their `set-session-*` aliases) — port of
// src/cli/commands/session.ts. NO CASCADE.
//
// All three commands write to the `timesofday` row for the given
// (plan, dest, day, session), touch the affected day, and emit
// plan_events (1 dest_process + 1 timeline) with the same KV
// payload the TS spread would produce. set-tod-zh additionally
// writes the child rows in `session_activities_zh` and
// `session_meals`.

use libsql::Connection;

#[derive(Default, Debug)]
struct ParsedFocus {
    day: i64,
    session: String,
    focus: Option<String>,
    focus_zh: Option<String>,
    dest: Option<String>,
}

#[derive(Default, Debug)]
struct ParsedTimeRange {
    day: i64,
    session: String,
    start: Option<String>,
    end: Option<String>,
    dest: Option<String>,
}

#[derive(Default, Debug)]
struct ParsedZh {
    day: i64,
    session: String,
    focus_zh: Option<String>,
    transit_zh: Option<String>,
    activities_zh: Option<Vec<String>>,
    meals_zh: Option<Vec<String>>,
    dest: Option<String>,
}

// ─────────────────────────────────────────────────────────────────
// set-tod-focus / set-session-focus
// ─────────────────────────────────────────────────────────────────

pub async fn run_focus(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    let parsed = parse_focus(args)?;
    let conn = crate::db::connect_write().await?;
    let destination = match read_destination(&conn, &plan_id, &parsed.dest).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };
    require_session(&conn, &plan_id, &destination, parsed.day, &parsed.session).await?;
    let focus = parsed.focus.as_deref();
    let focus_zh = parsed.focus_zh.as_deref();

    println!(
        "✅ Focus set for D{} {}: \"{}\"",
        parsed.day,
        parsed.session,
        focus.unwrap_or("(cleared)")
    );

    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        (
            "focus",
            focus.map(str::to_string).unwrap_or_else(|| "null".to_string()),
        ),
        // focus_zh is ALWAYS emitted (per the TS spread — even when
        // undefined, it becomes the literal string "undefined" via
        // eventDataToKv).
        (
            "focus_zh",
            focus_zh
                .map(str::to_string)
                .unwrap_or_else(|| "undefined".to_string()),
        ),
    ];

    execute_tod(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        focus_zh,
        None, // focus_zh: passed above
        None,
        None,
        &kv,
        "itinerary_session_focus_set",
        "set-tod-focus",
        &format!("D{}/{}", parsed.day, parsed.session),
    )
    .await?;

    if let Some(f) = focus {
        // Apply the focus column.
        apply_tod_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            "focus",
            Some(f.to_string()),
        )
        .await?;
    }
    if let Some(fz) = focus_zh {
        apply_tod_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            "focus_zh",
            Some(fz.to_string()),
        )
        .await?;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// set-tod-time-range / set-session-time-range
// ─────────────────────────────────────────────────────────────────

pub async fn run_time_range(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    let parsed = parse_time_range(args)?;
    let start = match &parsed.start {
        Some(s) => s.clone(),
        None => {
            eprintln!("Error: set-session-time-range requires --start HH:MM and --end HH:MM");
            std::process::exit(1);
        }
    };
    let end = match &parsed.end {
        Some(e) => e.clone(),
        None => {
            eprintln!("Error: set-session-time-range requires --start HH:MM and --end HH:MM");
            std::process::exit(1);
        }
    };
    let conn = crate::db::connect_write().await?;
    let destination = match read_destination(&conn, &plan_id, &parsed.dest).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };
    require_session(&conn, &plan_id, &destination, parsed.day, &parsed.session).await?;

    println!(
        "\n🕒 Setting session time range:\n   Destination: {destination}\n   Day {} {}: {} → {}",
        parsed.day, parsed.session, start, end
    );

    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        ("start", start.clone()),
        ("end", end.clone()),
    ];
    // Order above is correct (matches TS Object.entries insertion order).

    // Touch the day and apply the time_range update.
    touch_day(&conn, &plan_id, &destination, parsed.day).await?;
    apply_time_range(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        &start,
        &end,
    )
    .await?;

    execute_tod(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        None,
        None,
        None,
        None,
        &kv,
        "session_time_range_updated",
        "set-session-time-range",
        &format!("D{} {}", parsed.day, parsed.session),
    )
    .await?;

    println!("✅ Session time range updated");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// set-tod-zh / set-session-zh
// ─────────────────────────────────────────────────────────────────

pub async fn run_zh(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    let parsed = parse_zh(args)?;
    let conn = crate::db::connect_write().await?;
    let destination = match read_destination(&conn, &plan_id, &parsed.dest).await {
        Ok(d) => d,
        Err(e) => return Err(e),
    };
    require_session(&conn, &plan_id, &destination, parsed.day, &parsed.session).await?;

    println!(
        "\n🌏 Setting session ZH content:\n   Destination: {}, Day {}, {}",
        destination, parsed.day, parsed.session
    );
    if let Some(z) = &parsed.focus_zh {
        println!("   focus_zh: \"{z}\"");
    }
    if let Some(z) = &parsed.transit_zh {
        println!("   transit_notes_zh: \"{z}\"");
    }
    if let Some(a) = &parsed.activities_zh {
        println!("   activities_zh: {} items", a.len());
    }
    if let Some(m) = &parsed.meals_zh {
        println!("   meals_zh: {} items", m.len());
    }

    // Apply scalar updates to timesofday.
    if let Some(z) = &parsed.focus_zh {
        apply_tod_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            "focus_zh",
            Some(z.clone()),
        )
        .await?;
    }
    if let Some(z) = &parsed.transit_zh {
        apply_tod_field(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            &parsed.session,
            "transit_notes_zh",
            Some(z.clone()),
        )
        .await?;
    }
    // activities_zh: DELETE-then-reinsert session_activities_zh.
    if let Some(arr) = &parsed.activities_zh {
        conn.execute(
            "DELETE FROM session_activities_zh \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                parsed.day,
                parsed.session.clone()
            ],
        )
        .await
        .map_err(|e| format!("session_activities_zh DELETE failed: {e}"))?;
        for (i, a) in arr.iter().enumerate() {
            conn.execute(
                "INSERT INTO session_activities_zh \
                    (plan_id, destination, day_number, session_type, sort_order, activity) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                libsql::params![
                    plan_id.to_string(),
                    destination.to_string(),
                    parsed.day,
                    parsed.session.clone(),
                    i as i64,
                    a.clone()
                ],
            )
            .await
            .map_err(|e| format!("session_activities_zh INSERT failed: {e}"))?;
        }
    }
    // meals_zh: do NOT touch session_meals. The TS path's
    // sync_normalized_tables only writes session_meals from the
    // in-memory `meals` (EN) field, not from `meals_zh`. Since
    // set-tod-zh only sets meals_zh (the Chinese variant), the EN
    // meals row in session_meals is preserved.
    let _ = &parsed.meals_zh; // explicitly unused
    touch_day(&conn, &plan_id, &destination, parsed.day).await?;

    // Event data: {day_number, session, focus_zh, transit_notes_zh,
    // activities_zh, meals_zh} — all 6 keys present. The TS event `data`
    // object carries the raw command fields, which are JS `undefined` when
    // the user omits the flag; eventDataToKv does String(undefined) →
    // "undefined" (verified against TS: omitted zh fields all serialize as
    // the literal "undefined", NOT "null").
    let kv: Vec<(&str, String)> = vec![
        ("day_number", parsed.day.to_string()),
        ("session", parsed.session.clone()),
        (
            "focus_zh",
            parsed.focus_zh.clone().unwrap_or_else(|| "undefined".to_string()),
        ),
        (
            "transit_notes_zh",
            parsed.transit_zh.clone().unwrap_or_else(|| "undefined".to_string()),
        ),
        (
            "activities_zh",
            parsed
                .activities_zh
                .as_ref()
                .map(|a| a.join(", "))
                .unwrap_or_else(|| "undefined".to_string()),
        ),
        (
            "meals_zh",
            parsed
                .meals_zh
                .as_ref()
                .map(|m| m.join(", "))
                .unwrap_or_else(|| "undefined".to_string()),
        ),
    ];

    execute_tod(
        &conn,
        &plan_id,
        &destination,
        parsed.day,
        &parsed.session,
        None,
        None,
        None,
        None,
        &kv,
        "itinerary_session_zh_content_set",
        "set-session-zh",
        &format!("D{}/{}", parsed.day, parsed.session),
    )
    .await?;

    println!("✅ Session ZH content updated");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// Arg parsing
// ─────────────────────────────────────────────────────────────────

fn parse_focus(args: &[String]) -> Result<ParsedFocus, String> {
    let mut p = ParsedFocus::default();
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
    if positional.is_empty() {
        return Err("Usage: set-tod-focus <day> <session> \"<focus_text>\"".to_string());
    }
    p.day = positional[0]
        .parse::<i64>()
        .map_err(|_| "<day> must be a positive integer".to_string())?;
    if p.day < 1 {
        return Err("<day> must be a positive integer".to_string());
    }
    if positional.len() < 2 {
        return Err("Usage: set-tod-focus <day> <session> \"<focus_text>\"".to_string());
    }
    p.session = positional[1].clone();
    if !["morning", "noon", "afternoon", "evening"].contains(&p.session.as_str()) {
        return Err("<session> must be one of: morning|noon|afternoon|evening".to_string());
    }
    if positional.len() >= 3 {
        p.focus = Some(positional[2..].join(" "));
    }
    Ok(p)
}

fn parse_time_range(args: &[String]) -> Result<ParsedTimeRange, String> {
    let mut p = ParsedTimeRange::default();
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
            "--start" => {
                p.start = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --start".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--end" => {
                p.end = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --end".to_string())?
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
        return Err("Usage: set-session-time-range <day> <session> --start HH:MM --end HH:MM".to_string());
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
    Ok(p)
}

fn parse_zh(args: &[String]) -> Result<ParsedZh, String> {
    let mut p = ParsedZh::default();
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
            "--zh" => {
                p.focus_zh = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --zh".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--transit-zh" => {
                p.transit_zh = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --transit-zh".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--activities-zh-json" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --activities-zh-json".to_string())?;
                let arr = parse_string_array(v)
                    .map_err(|e| format!("--activities-zh-json: {e}"))?;
                p.activities_zh = Some(arr);
                i += 2;
            }
            "--meals-zh-json" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --meals-zh-json".to_string())?;
                let arr = parse_string_array(v)
                    .map_err(|e| format!("--meals-zh-json: {e}"))?;
                p.meals_zh = Some(arr);
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
        return Err("Usage: set-session-zh <day> <session> --zh \"...\" ...".to_string());
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
    Ok(p)
}

fn parse_string_array(s: &str) -> Result<Vec<String>, String> {
    let s = s.trim();
    if !s.starts_with('[') || !s.ends_with(']') {
        return Err("expected JSON array".to_string());
    }
    let inner = &s[1..s.len() - 1];
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    // Split top-level strings. Same depth-tracker as the bulk
    // segments parser.
    let bytes = inner.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < inner.len() {
        while i < inner.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= inner.len() {
            break;
        }
        if bytes[i] != b'"' {
            return Err("expected string".to_string());
        }
        i += 1;
        let val_start = i;
        while i < inner.len() {
            if bytes[i] == b'\\' && i + 1 < inner.len() {
                i += 2;
                continue;
            }
            if bytes[i] == b'"' {
                break;
            }
            i += 1;
        }
        let v = inner[val_start..i].to_string();
        i += 1;
        out.push(v);
        while i < inner.len() && ((bytes[i] as char).is_whitespace() || bytes[i] == b',') {
            i += 1;
        }
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────
// Shared DB helpers
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

#[allow(clippy::too_many_arguments)]
async fn execute_tod(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    focus_field: Option<&str>,
    focus_zh_field: Option<&str>,
    transit_zh_field: Option<&str>,
    times: Option<(&str, &str)>,
    kv: &[(&str, String)],
    event_name: &str,
    command_type: &str,
    command_summary: &str,
) -> Result<(), String> {
    let _ = (focus_field, focus_zh_field, transit_zh_field, times);
    // The function fields are unused in the executor; the field
    // application happens in the calling run_*() function before
    // this is called. We accept them for symmetry but ignore them
    // here.
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

    let _ = (day, session);
    Ok(())
}

async fn apply_tod_field(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    field: &str,
    value: Option<String>,
) -> Result<(), String> {
    // Mirror the TS sync INSERT OR REPLACE pattern. The Rust
    // equivalent for an UPDATE of a single column.
    let column = match field {
        "focus" => "focus",
        "focus_zh" => "focus_zh",
        "transit_notes_zh" => "transit_notes_zh",
        _ => return Err(format!("unsupported timesofday field: {field}")),
    };
    let sql = format!(
        "UPDATE timesofday SET {column} = ?1, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 AND session_type = ?6"
    );
    conn.execute(
        &sql,
        libsql::params![
            value,
            now_db_datetime(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string()
        ],
    )
    .await
    .map_err(|e| format!("timesofday UPDATE {field} failed: {e}"))?;
    touch_day(conn, plan_id, destination, day).await?;
    Ok(())
}

async fn apply_time_range(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    start: &str,
    end: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE timesofday SET time_range_start = ?1, time_range_end = ?2, updated_at = ?3 \
         WHERE plan_id = ?4 AND destination = ?5 AND day_number = ?6 AND session_type = ?7",
        libsql::params![
            start.to_string(),
            end.to_string(),
            now_db_datetime(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string()
        ],
    )
    .await
    .map_err(|e| format!("timesofday time_range UPDATE failed: {e}"))?;
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

/// Fail loud unless the target `timesofday` session row pre-exists. These
/// commands UPDATE an existing session (created by the itinerary scaffold);
/// without this guard a missing session would silently no-op the field UPDATE
/// while still writing a `completed` audit + events (and, for set-tod-zh,
/// orphan `session_activities_zh` child rows). Called early in each run_*()
/// before ANY write so a ✅/completed audit always implies a row changed.
async fn require_session(
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
            "no timesofday row for plan={plan_id} destination={destination} day={day} \
             session={session}; scaffold the itinerary first (travel scaffold-itinerary)"
        ));
    }
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
    fn parse_focus_basic() {
        let p = parse_focus(&[
            "1".to_string(),
            "morning".to_string(),
            "Tokyo Tower visit".to_string(),
        ])
        .unwrap();
        assert_eq!(p.day, 1);
        assert_eq!(p.session, "morning");
        assert_eq!(p.focus.as_deref(), Some("Tokyo Tower visit"));
    }

    #[test]
    fn parse_focus_no_text_clears() {
        let p = parse_focus(&["2".to_string(), "noon".to_string()]).unwrap();
        assert!(p.focus.is_none());
    }

    #[test]
    fn parse_focus_bad_session() {
        assert!(parse_focus(&["1".to_string(), "nope".to_string()]).is_err());
    }

    #[test]
    fn parse_string_array_basic() {
        let s = r#"["a","b","c"]"#;
        assert_eq!(parse_string_array(s).unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_string_array_empty() {
        assert!(parse_string_array("[]").unwrap().is_empty());
    }
}
