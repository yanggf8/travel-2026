// `travel set-route-segment` and `travel set-route-segments-bulk` —
// port of src/cli/commands/route.ts. NO CASCADE.
//
// Both commands write day_route_segments + bump plans.version +
// emit a plan_event (route_segment_updated for single, or
// route_segments_bulk_updated for bulk). Verified no-cascade:
// cascade_dirty_flags UNCHANGED.

use libsql::Connection;

#[derive(Default, Debug)]
struct SegmentInput {
    from: String,
    to: String,
    mode: String,
    duration: Option<i64>,
    notes: Option<String>,
    start_time: Option<String>,
}

pub async fn run(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    // set-route-segment <day> <sort_order> <from> <to> <mode> [--duration N] [--notes "..."] [--start-time HH:MM] [--dest slug]
    if args.len() < 5 {
        eprintln!("Error: set-route-segment requires <day> <sort_order> <from> <to> <mode>");
        eprintln!("Example: set-route-segment 1 0 \"關渡\" \"嘟嘟房桃園機場貨運1站\" driving --duration 45");
        std::process::exit(1);
    }
    let day_str = &args[0];
    let sort_order_str = &args[1];
    let from = args[2].clone();
    let to = args[3].clone();
    let mode = args[4].clone();
    if mode != "transit" && mode != "walking" && mode != "driving" {
        eprintln!("Error: <mode> must be one of: transit | walking | driving");
        std::process::exit(1);
    }
    let day: i64 = match day_str.parse() {
        Ok(n) if n >= 1 => n,
        _ => {
            eprintln!("Error: <day> must be a positive integer");
            std::process::exit(1);
        }
    };
    let sort_order: i64 = match sort_order_str.parse() {
        Ok(n) if n >= 0 => n,
        _ => {
            eprintln!("Error: <sort_order> must be a non-negative integer (0-based)");
            std::process::exit(1);
        }
    };
    let duration = parse_optional_int_flag(&args[5..], "--duration")?;
    let notes = parse_optional_string_flag(&args[5..], "--notes")?;
    let start_time = parse_optional_string_flag(&args[5..], "--start-time")?;
    let destination = match read_destination(&conn_write().await?, &plan_id, &args[5..]).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    let dur_label = match duration {
        Some(d) => format!(", {d} min"),
        None => String::new(),
    };
    let start_label = match &start_time {
        Some(s) => format!(", start {s}"),
        None => String::new(),
    };
    println!(
        "\n🗺️  Setting route segment:\n   Destination: {destination}\n   Day {day} slot {sort_order}: {from} → {to} ({mode}{dur_label}{start_label})"
    );
    if let Some(n) = &notes {
        println!("   Notes: {n}");
    }

    match execute_single(
        &conn_write().await?,
        &plan_id,
        &destination,
        day,
        sort_order,
        &from,
        &to,
        &mode,
        duration,
        notes.as_deref(),
        start_time.as_deref(),
    )
    .await
    {
        Ok(_) => {
            println!("✅ Route segment updated");
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: set-route-segment failed: {e}");
            std::process::exit(1);
        }
    }
}

pub async fn run_bulk(
    args: &[String],
    plan_id: String,
) -> Result<(), String> {
    // set-route-segments-bulk <day> --json '[{...}, ...]' [--dest slug]
    if args.is_empty() {
        eprintln!("Error: set-route-segments-bulk requires <day> --json '[...]'");
        std::process::exit(1);
    }
    let day_str = &args[0];
    let day: i64 = match day_str.parse() {
        Ok(n) if n >= 1 => n,
        _ => {
            eprintln!("Error: <day> must be a positive integer");
            std::process::exit(1);
        }
    };
    let json_opt = match parse_optional_string_flag(&args[1..], "--json")? {
        Some(j) => j,
        None => {
            eprintln!("Error: --json is required with a JSON array of route segments");
            std::process::exit(1);
        }
    };
    let segments: Vec<SegmentInput> = match parse_bulk_json(&json_opt) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: Invalid --json: {e}");
            std::process::exit(1);
        }
    };
    for (i, s) in segments.iter().enumerate() {
        if s.mode != "transit" && s.mode != "walking" && s.mode != "driving" {
            eprintln!("Error: Invalid mode in segment {i}: {}", s.mode);
            std::process::exit(1);
        }
    }
    let destination = match read_destination(&conn_write().await?, &plan_id, &args[1..]).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    println!(
        "\n🛤️  Replacing all route segments for Day {day}:\n   Destination: {destination}\n   {} segments",
        segments.len()
    );
    for s in &segments {
        let dur = s
            .duration
            .map(|d| format!(", {d}min"))
            .unwrap_or_default();
        println!("   {} → {} ({}{})", s.from, s.to, s.mode, dur);
    }

    match execute_bulk(
        &conn_write().await?,
        &plan_id,
        &destination,
        day,
        &segments,
    )
    .await
    {
        Ok(_) => {
            println!("✅ Route segments replaced");
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: set-route-segments-bulk failed: {e}");
            std::process::exit(1);
        }
    }
}

async fn conn_write() -> Result<Connection, String> {
    crate::db::connect_write().await
}

fn parse_optional_int_flag(args: &[String], flag: &str) -> Result<Option<i64>, String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            let v = args
                .get(i + 1)
                .ok_or_else(|| format!("missing value for {flag}"))?;
            let n: i64 = v
                .parse()
                .map_err(|_| format!("{flag} must be a number (got \"{v}\")"))?;
            return Ok(Some(n));
        }
        i += 1;
    }
    Ok(None)
}

fn parse_optional_string_flag(
    args: &[String],
    flag: &str,
) -> Result<Option<String>, String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            let v = args
                .get(i + 1)
                .ok_or_else(|| format!("missing value for {flag}"))?;
            return Ok(Some(v.clone()));
        }
        i += 1;
    }
    Ok(None)
}

/// Hand-rolled minimal JSON parser for the bulk-segments format:
///   `[{"from":"A","to":"B","mode":"walking","duration":5,"notes":"...","start_time":"HH:MM"}, ...]`
/// Avoids adding a serde dep. Recognized keys: from, to, mode,
/// duration, notes, start_time.
fn parse_bulk_json(s: &str) -> Result<Vec<SegmentInput>, String> {
    let s = s.trim();
    if !s.starts_with('[') || !s.ends_with(']') {
        return Err("expected JSON array".to_string());
    }
    let inner = &s[1..s.len() - 1];
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    // Split top-level objects (depth-0 commas). We don't need to
    // handle nested arrays/objects beyond tracking `{` / `}` depth
    // (none expected here, but defensive).
    let objects = split_top_level_objects(inner)?;
    let mut out: Vec<SegmentInput> = Vec::new();
    for obj_str in objects {
        let mut seg = SegmentInput {
            from: String::new(),
            to: String::new(),
            mode: String::new(),
            duration: None,
            notes: None,
            start_time: None,
        };
        let kv = parse_object_kv(obj_str.trim())?;
        for (k, v) in kv {
            match k.as_str() {
                "from" => seg.from = v,
                "to" => seg.to = v,
                "mode" => seg.mode = v,
                "duration" => {
                    seg.duration = Some(
                        v.parse::<i64>()
                            .map_err(|_| format!("duration must be a number: {v}"))?,
                    );
                }
                "notes" => seg.notes = Some(v),
                "start_time" => seg.start_time = Some(v),
                _ => {} // ignore unknown keys
            }
        }
        if seg.from.is_empty() || seg.to.is_empty() || seg.mode.is_empty() {
            return Err("each segment needs from, to, mode".to_string());
        }
        out.push(seg);
    }
    Ok(out)
}

fn split_top_level_objects(s: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    let mut start = 0usize;
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    out.push(s[start..=i].to_string());
                } else if depth < 0 {
                    return Err("unbalanced braces".to_string());
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err("unbalanced braces".to_string());
    }
    Ok(out)
}

fn parse_object_kv(s: &str) -> Result<Vec<(String, String)>, String> {
    let s = s.trim();
    if !s.starts_with('{') || !s.ends_with('}') {
        return Err("expected JSON object".to_string());
    }
    let inner = &s[1..s.len() - 1];
    let mut out: Vec<(String, String)> = Vec::new();
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < inner.len() {
        // Skip whitespace + comma
        while i < inner.len() && (bytes[i] as char).is_whitespace() || bytes[i] == b',' {
            i += 1;
        }
        if i >= inner.len() {
            break;
        }
        // Parse key
        if bytes[i] != b'"' {
            return Err("expected string key".to_string());
        }
        i += 1;
        let key_start = i;
        let mut in_str = true;
        while i < inner.len() && in_str {
            if bytes[i] == b'\\' && i + 1 < inner.len() {
                i += 2;
                continue;
            }
            if bytes[i] == b'"' {
                in_str = false;
            } else {
                i += 1;
            }
        }
        if in_str {
            return Err("unterminated string".to_string());
        }
        let key = inner[key_start..i].to_string();
        i += 1; // consume closing quote
        // Skip ws + colon
        while i < inner.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= inner.len() || bytes[i] != b':' {
            return Err("expected ':'".to_string());
        }
        i += 1;
        while i < inner.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        // Parse value (string or number)
        if i >= inner.len() {
            return Err("expected value".to_string());
        }
        if bytes[i] == b'"' {
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
            let val = inner[val_start..i].to_string();
            i += 1; // closing quote
            out.push((key, val));
        } else if bytes[i] == b'-' || (bytes[i] as char).is_ascii_digit() {
            let val_start = i;
            while i < inner.len() {
                let c = bytes[i] as char;
                if c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == 'E' {
                    i += 1;
                } else {
                    break;
                }
            }
            let val = inner[val_start..i].to_string();
            out.push((key, val));
        } else {
            return Err(format!("unexpected value at {i}"));
        }
    }
    Ok(out)
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

#[allow(clippy::too_many_arguments)]
async fn execute_single(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    sort_order: i64,
    from: &str,
    to: &str,
    mode: &str,
    duration: Option<i64>,
    notes: Option<&str>,
    start_time: Option<&str>,
) -> Result<i64, String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();

    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    // 1. DELETE the existing segment for (day, sort_order), then
    //    INSERT the new one. The TS path also touches days.updated_at
    //    for the affected day; we mirror that.
    conn.execute(
        "DELETE FROM day_route_segments \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND sort_order = ?4",
        libsql::params![plan_id.to_string(), destination.to_string(), day, sort_order],
    )
    .await
    .map_err(|e| format!("day_route_segments DELETE failed: {e}"))?;
    conn.execute(
        "INSERT INTO day_route_segments \
            (plan_id, destination, day_number, sort_order, from_place, to_place, \
             mode, duration_min, notes, start_time) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            day,
            sort_order,
            from.to_string(),
            to.to_string(),
            mode.to_string(),
            duration,
            notes.map(str::to_string),
            start_time.map(str::to_string)
        ],
    )
    .await
    .map_err(|e| format!("day_route_segments INSERT failed: {e}"))?;

    // 2. Touch days.updated_at for the affected day (TS does this via
    //    touchItinerary → process_5_daily_itinerary.updated_at, which
    //    sync_normalized_tables propagates to the days row).
    conn.execute(
        "UPDATE days SET updated_at = ?1 WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4",
        libsql::params![now_db.clone(), plan_id.to_string(), destination.to_string(), day],
    )
    .await
    .map_err(|e| format!("days touch UPDATE failed: {e}"))?;

    // 3. plan_events + plan_event_data.
    let dest_process_so = next_dest_process_sort_order(
        conn,
        plan_id,
        destination,
        "process_5_daily_itinerary",
    )
    .await?;
    let timeline_base = next_timeline_sort_order(conn, plan_id).await?;

    // The TS event data is `{day_number, sort_order, from_place,
    // to_place, mode, duration_min}` — 6 keys, in that order. NOT
    // start_time, NOT notes.
    let kv: Vec<(&str, String)> = vec![
        ("day_number", day.to_string()),
        ("sort_order", sort_order.to_string()),
        ("from_place", from.to_string()),
        ("to_place", to.to_string()),
        ("mode", mode.to_string()),
        (
            "duration_min",
            duration.map(|n| n.to_string()).unwrap_or_default(),
        ),
    ];

    insert_event(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_5_daily_itinerary",
        dest_process_so,
        "route_segment_updated",
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

    let timeline_so = timeline_base;
    insert_event(
        conn,
        plan_id,
        "timeline",
        "",
        "process_5_daily_itinerary",
        timeline_so,
        "route_segment_updated",
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

    // 4. operation_runs + plans.version.
    let run_id = new_run_id();
    let summary = format!("D{day} slot{sort_order} {from}→{to}");
    conn.execute(
        "INSERT INTO operation_runs \
            (run_id, plan_id, command_type, command_summary, status, \
             version_before, version_after, started_at, completed_at) \
         VALUES (?1, ?2, 'set-route-segment', ?3, 'completed', ?4, ?5, ?6, ?6)",
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

async fn execute_bulk(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    segments: &[SegmentInput],
) -> Result<i64, String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();

    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    // 1. DELETE ALL segments for (plan, dest, day) + INSERT new.
    conn.execute(
        "DELETE FROM day_route_segments \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
        libsql::params![plan_id.to_string(), destination.to_string(), day],
    )
    .await
    .map_err(|e| format!("day_route_segments DELETE failed: {e}"))?;
    for (i, s) in segments.iter().enumerate() {
        conn.execute(
            "INSERT INTO day_route_segments \
                (plan_id, destination, day_number, sort_order, from_place, to_place, \
                 mode, duration_min, notes, start_time) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                i as i64,
                s.from.clone(),
                s.to.clone(),
                s.mode.clone(),
                s.duration,
                s.notes.clone(),
                s.start_time.clone()
            ],
        )
        .await
        .map_err(|e| format!("day_route_segments INSERT[{i}] failed: {e}"))?;
    }

    // 2. Touch days.updated_at.
    conn.execute(
        "UPDATE days SET updated_at = ?1 WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4",
        libsql::params![now_db.clone(), plan_id.to_string(), destination.to_string(), day],
    )
    .await
    .map_err(|e| format!("days touch UPDATE failed: {e}"))?;

    // 3. plan_events + plan_event_data.
    let dest_process_so = next_dest_process_sort_order(
        conn,
        plan_id,
        destination,
        "process_5_daily_itinerary",
    )
    .await?;
    let timeline_base = next_timeline_sort_order(conn, plan_id).await?;

    let kv: Vec<(&str, String)> = vec![
        ("day_number", day.to_string()),
        ("segment_count", segments.len().to_string()),
    ];

    insert_event(
        conn,
        plan_id,
        "dest_process",
        destination,
        "process_5_daily_itinerary",
        dest_process_so,
        "route_segments_bulk_updated",
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

    let timeline_so = timeline_base;
    insert_event(
        conn,
        plan_id,
        "timeline",
        "",
        "process_5_daily_itinerary",
        timeline_so,
        "route_segments_bulk_updated",
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

    // 4. operation_runs + plans.version.
    let run_id = new_run_id();
    let summary = format!("D{day} {} segments", segments.len());
    conn.execute(
        "INSERT INTO operation_runs \
            (run_id, plan_id, command_type, command_summary, status, \
             version_before, version_after, started_at, completed_at) \
         VALUES (?1, ?2, 'set-route-segments-bulk', ?3, 'completed', ?4, ?5, ?6, ?6)",
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
    fn parse_optional_int_flag_present() {
        let args = vec!["--duration".to_string(), "45".to_string()];
        assert_eq!(parse_optional_int_flag(&args, "--duration").unwrap(), Some(45));
    }

    #[test]
    fn parse_optional_int_flag_absent() {
        let args = vec![];
        assert_eq!(parse_optional_int_flag(&args, "--duration").unwrap(), None);
    }

    #[test]
    fn parse_optional_int_flag_bad() {
        let args = vec!["--duration".to_string(), "abc".to_string()];
        assert!(parse_optional_int_flag(&args, "--duration").is_err());
    }

    #[test]
    fn parse_optional_string_flag_basic() {
        let args = vec!["--notes".to_string(), "hello".to_string()];
        assert_eq!(
            parse_optional_string_flag(&args, "--notes").unwrap(),
            Some("hello".to_string())
        );
    }

    #[test]
    fn parse_bulk_json_basic() {
        let s = r#"[{"from":"a","to":"b","mode":"walking"},{"from":"c","to":"d","mode":"transit","duration":12,"notes":"JR"}]"#;
        let segs = parse_bulk_json(s).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].from, "a");
        assert_eq!(segs[0].to, "b");
        assert_eq!(segs[0].mode, "walking");
        assert!(segs[0].duration.is_none());
        assert_eq!(segs[1].duration, Some(12));
        assert_eq!(segs[1].notes.as_deref(), Some("JR"));
    }
}
