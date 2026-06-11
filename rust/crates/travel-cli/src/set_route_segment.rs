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
    // Write-time map-link guard: a stop that won't form a valid Google Maps query
    // is rejected here, before it reaches the DB / dashboard. This is the fix for
    // the broken-transit-link bug — caught at creation, not in a late review.
    for (label, stop) in [("from", &from), ("to", &to)] {
        if let Err(reason) = crate::validate_itinerary::check_stop_linkable(stop) {
            eprintln!("Error: <{label}> \"{stop}\" {reason}.");
            eprintln!("Hint: use a clean place name (e.g. \"赤嶺駅\", \"iias 沖縄豊崎\") — no （…）notes, +步行, or clock times inside the stop.");
            std::process::exit(1);
        }
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
    // set-route-segments-bulk <day> --seg "from|to|mode[|dur[|start[|notes]]]" ... [--dest slug]
    // Plain-text, agent-first: repeat --seg once per segment (pipe-delimited,
    // like set-airport-transfer's --selected). No JSON.
    if args.is_empty() {
        eprintln!("Error: set-route-segments-bulk requires <day> --seg \"from|to|mode\" [--seg ...]");
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
    let specs = collect_repeated_flag(&args[1..], "--seg");
    if specs.is_empty() {
        eprintln!("Error: at least one --seg \"from|to|mode[|duration[|start_time[|notes]]]\" is required");
        std::process::exit(1);
    }
    let mut segments: Vec<SegmentInput> = Vec::with_capacity(specs.len());
    for (i, spec) in specs.iter().enumerate() {
        match parse_seg_spec(spec) {
            Ok(s) => {
                // Write-time map-link guard (same as single-segment path).
                for (label, stop) in [("from", &s.from), ("to", &s.to)] {
                    if let Err(reason) = crate::validate_itinerary::check_stop_linkable(stop) {
                        eprintln!("Error: --seg #{i} <{label}> \"{stop}\" {reason}.");
                        eprintln!("Hint: use a clean place name — no （…）notes, +步行, or clock times inside the stop.");
                        std::process::exit(1);
                    }
                }
                segments.push(s);
            }
            Err(e) => {
                eprintln!("Error: --seg #{i} invalid ({spec:?}): {e}");
                std::process::exit(1);
            }
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

/// Collect every value of a repeatable flag: `--seg X --seg Y` → ["X", "Y"].
/// (Other flags like `--dest <v>` are skipped along with their value.)
fn collect_repeated_flag(args: &[String], flag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            if let Some(v) = args.get(i + 1) {
                out.push(v.clone());
            }
            i += 2;
        } else if args[i].starts_with("--") {
            // skip an unrelated flag and its value (best-effort)
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

/// Parse one plain-text segment spec:
///   `from|to|mode[|duration[|start_time[|notes]]]`
/// Pipe-delimited (same convention as set-airport-transfer's --selected).
/// mode ∈ transit|walking|driving. Trailing fields are optional; an empty
/// field means "omitted" (e.g. `a|b|walking||` → no duration, no start_time).
fn parse_seg_spec(spec: &str) -> Result<SegmentInput, String> {
    let parts: Vec<&str> = spec.split('|').collect();
    if parts.len() < 3 {
        return Err("expected at least from|to|mode".to_string());
    }
    let from = parts[0].trim().to_string();
    let to = parts[1].trim().to_string();
    let mode = parts[2].trim().to_string();
    if from.is_empty() || to.is_empty() {
        return Err("from and to must be non-empty".to_string());
    }
    if mode != "transit" && mode != "walking" && mode != "driving" {
        return Err(format!("mode must be transit|walking|driving (got {mode:?})"));
    }
    let field = |idx: usize| -> Option<String> {
        parts.get(idx).map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_string)
    };
    let duration = match field(3) {
        Some(d) => Some(
            d.parse::<i64>()
                .map_err(|_| format!("duration must be a number (got {d:?})"))?,
        ),
        None => None,
    };
    Ok(SegmentInput {
        from,
        to,
        mode,
        duration,
        start_time: field(4),
        notes: field(5),
    })
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

    // 0. The target `days` row MUST pre-exist (it's created by the itinerary
    //    scaffold, not this command). Without this guard the segment INSERT
    //    below would write an orphan row and still emit a `completed` audit —
    //    fail loud instead so a ✅/completed audit always implies the parent
    //    day was real.
    if !day_exists(conn, plan_id, destination, day).await? {
        return Err(format!(
            "no days row for plan={plan_id} destination={destination} day={day}; \
             scaffold the itinerary first (travel scaffold-itinerary)"
        ));
    }

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

    // 0. The target `days` row MUST pre-exist (see execute_single).
    if !day_exists(conn, plan_id, destination, day).await? {
        return Err(format!(
            "no days row for plan={plan_id} destination={destination} day={day}; \
             scaffold the itinerary first (travel scaffold-itinerary)"
        ));
    }

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
    fn parse_seg_spec_minimal() {
        // Plain-text, agent-first: "from|to|mode" — pipe-delimited, like
        // set-airport-transfer's --selected. No JSON.
        let s = parse_seg_spec("a|b|walking").unwrap();
        assert_eq!(s.from, "a");
        assert_eq!(s.to, "b");
        assert_eq!(s.mode, "walking");
        assert!(s.duration.is_none());
        assert!(s.notes.is_none());
        assert!(s.start_time.is_none());
    }

    #[test]
    fn parse_seg_spec_full() {
        // from|to|mode|duration|start_time|notes — trailing fields optional;
        // empty field = omitted.
        let s = parse_seg_spec("c|d|transit|12|09:30|JR").unwrap();
        assert_eq!(s.from, "c");
        assert_eq!(s.to, "d");
        assert_eq!(s.mode, "transit");
        assert_eq!(s.duration, Some(12));
        assert_eq!(s.start_time.as_deref(), Some("09:30"));
        assert_eq!(s.notes.as_deref(), Some("JR"));
    }

    #[test]
    fn parse_seg_spec_empty_optional_fields() {
        // "c|d|transit||" → duration & start_time omitted (empty), no notes.
        let s = parse_seg_spec("c|d|transit||").unwrap();
        assert_eq!(s.duration, None);
        assert_eq!(s.start_time, None);
        assert_eq!(s.notes, None);
    }

    #[test]
    fn parse_seg_spec_rejects_bad_mode() {
        assert!(parse_seg_spec("a|b|teleport").is_err());
    }

    #[test]
    fn parse_seg_spec_rejects_missing_fields() {
        assert!(parse_seg_spec("a|b").is_err());
    }
}
