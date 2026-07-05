// `travel set-route-segment` and `travel set-route-segments-bulk` —
// port of src/cli/commands/route.ts. NO CASCADE.
//
// Both commands write day_route_segments + bump plans.version +
// emit a plan_event (route_segment_updated for single, or
// route_segments_bulk_updated for bulk). Verified no-cascade:
// cascade_dirty_flags UNCHANGED.

use libsql::Connection;
use travel_db::repo::route_segments;

#[derive(Default, Debug)]
struct SegmentInput {
    from: String,
    to: String,
    mode: String,
    duration: Option<i64>,
    notes: Option<String>,
    start_time: Option<String>,
}

/// Fail-loud guard bundle for a single (from, to, mode) route segment. Reuses
/// the lint's OWN predicates verbatim (`crate::checks`) so the WRITE guard and
/// the READ-ONLY lint in `validate_itinerary.rs` agree exactly — no drift.
///
/// Mirrors `validate_itinerary::lint`'s per-leg logic, in the same order:
///   (#3) each of from/to must form a usable Maps stop: reject if it cleans to
///        empty, retains stray （）()＋+, carries a clock time, or is a
///        mode-only word (步行/單軌/巴士…). The segment's `mode` column already
///        carries the travel mode, so a mode word standing as a PLACE is a bug.
///   (#4) cross-country ground leg: compute place_country on the CLEANED stops
///        (matching the lint, which cleans before comparing) and reject when
///        both are known AND differ (an ocean-spanning route, e.g. TW↔JP).
///   (#5) walking-over-rail: if mode=="walking" and `"<from> <to>"` mentions a
///        rail/bus mode → reject (the lint uses the raw from/to as context).
///
/// Returns `Err(reason)` naming the offending stop/rule; `Ok(())` when clean.
/// Pure: no DB, no I/O — so it's unit-tested directly and called before any write.
fn guard_segment(from: &str, to: &str, mode: &str) -> Result<(), String> {
    // (#3) per-stop map-link integrity — use the lint's check_stop_linkable,
    // which is exactly clean_stop → stop_link_problem.
    for (label, stop) in [("from", from), ("to", to)] {
        if let Err(reason) = crate::checks::check_stop_linkable(stop) {
            return Err(format!("<{label}> \"{stop}\" {reason} [rule: map-link/stop]"));
        }
    }

    // The lint compares place_country on the CLEANED stop strings; do the same
    // so the guard never disagrees with what the lint would flag.
    let from_clean = crate::checks::clean_stop(from);
    let to_clean = crate::checks::clean_stop(to);

    // (#4) cross-country ground leg — both known AND different.
    if let (Some(a), Some(b)) = (
        crate::checks::place_country(&from_clean),
        crate::checks::place_country(&to_clean),
    ) {
        if a != b {
            return Err(format!(
                "cross-country ground leg: \"{from}\" ({a}) → \"{to}\" ({b}) would draw an ocean-spanning route [rule: cross-country]"
            ));
        }
    }

    // (#5) walking leg that actually rides rail/bus — context is the raw pair
    // (matches the lint's `format!("{from} {to}")` ctx for route legs).
    if mode == "walking" && crate::checks::mentions_rail_or_bus(&format!("{from} {to}")) {
        return Err(format!(
            "mode=walking but \"{from} {to}\" names a rail/bus leg — set mode=transit [rule: walking-over-rail]"
        ));
    }

    Ok(())
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
    let mode = match crate::checks::normalize_mode(&args[4]) {
        Ok(m) => m.to_string(),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };
    // Write-time fail-loud guard bundle (#3 map-link, #4 cross-country, #5
    // walking-over-rail): a segment that would draw a broken / ocean-spanning /
    // mis-moded Maps route is rejected here, before it reaches the DB /
    // dashboard. Same predicates the validate-itinerary lint uses, so the guard
    // and the lint agree exactly. Caught at creation, not in a late review.
    if let Err(reason) = guard_segment(&from, &to, &mode) {
        eprintln!("Error: route segment rejected — {reason}.");
        eprintln!("Hint: use a clean place name (e.g. \"赤嶺駅\", \"iias 沖縄豊崎\") — no （…）notes, +步行, or clock times inside the stop; keep both stops in the same country; use mode=transit for a rail/bus leg.");
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

    let source = if has_flag(&args[5..], "--recommended") {
        "ai_recommended"
    } else {
        "confirmed"
    };
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
        source,
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
    // Parse + guard ALL segments BEFORE any write, collecting every bad one.
    // A bulk write is all-or-nothing: if any segment is malformed or fails the
    // fail-loud guard bundle (#3 map-link / #4 cross-country / #5 walking-over-
    // rail), the WHOLE batch is rejected and NOTHING is written — a partial bad
    // batch must never half-write. We report every offending --seg, not just the
    // first, so the fix can be made in one pass.
    let mut segments: Vec<SegmentInput> = Vec::with_capacity(specs.len());
    let mut problems: Vec<String> = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        match parse_seg_spec(spec) {
            Ok(s) => {
                if let Err(reason) = guard_segment(&s.from, &s.to, &s.mode) {
                    problems.push(format!("--seg #{i} {reason}"));
                }
                segments.push(s);
            }
            Err(e) => {
                problems.push(format!("--seg #{i} invalid ({spec:?}): {e}"));
            }
        }
    }
    if !problems.is_empty() {
        eprintln!(
            "Error: batch rejected — {} of {} segment(s) bad; NOTHING was written:",
            problems.len(),
            specs.len()
        );
        for p in &problems {
            eprintln!("  - {p}");
        }
        eprintln!("Hint: use clean place names — no （…）notes, +步行, or clock times inside a stop; keep both stops in the same country; use mode=transit for a rail/bus leg.");
        std::process::exit(1);
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

    let source = if has_flag(&args[1..], "--recommended") {
        "ai_recommended"
    } else {
        "confirmed"
    };
    match execute_bulk(
        &conn_write().await?,
        &plan_id,
        &destination,
        day,
        &segments,
        source,
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

/// Value-less flag scan — does NOT consume a following token as a value.
fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
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
        } else if args[i] == "--recommended" {
            // value-less flag — skip one token only
            i += 1;
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
    if from.is_empty() || to.is_empty() {
        return Err("from and to must be non-empty".to_string());
    }
    let mode = crate::checks::normalize_mode(parts[2])?.to_string();
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
    let dest_override = dest_flag(args);
    crate::cascade::common::resolve_active_destination(conn, plan_id, dest_override.as_deref())
        .await
}

/// Scan `--dest <slug>` out of a raw arg slice (first occurrence).
fn dest_flag(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--dest" {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
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
    source: &str,
) -> Result<i64, String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();

    // 0. The target `days` row MUST pre-exist (it's created by the itinerary
    //    scaffold, not this command). Without this guard the segment INSERT
    //    below would write an orphan row and still emit a `completed` audit —
    //    fail loud instead so a ✅/completed audit always implies the parent
    //    day was real.
    if !route_segments::day_exists(conn, plan_id, destination, day).await? {
        return Err(format!(
            "no days row for plan={plan_id} destination={destination} day={day}; \
             scaffold the itinerary first (travel scaffold-itinerary)"
        ));
    }

    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    // 1. DELETE the existing segment for (day, sort_order), then INSERT the new
    //    one (domain writes via the DAL). The TS path also touches days.updated_at
    //    for the affected day; we mirror that.
    route_segments::delete_slot(conn, plan_id, destination, day, sort_order).await?;
    route_segments::insert_segment(
        conn,
        plan_id,
        destination,
        day,
        sort_order,
        &route_segments::SegmentWrite {
            from_place: from.to_string(),
            to_place: to.to_string(),
            mode: mode.to_string(),
            duration_min: duration,
            notes: notes.map(str::to_string),
            start_time: start_time.map(str::to_string),
        },
        source,
        "",
    )
    .await?;

    // 2. Touch days.updated_at for the affected day (TS does this via
    //    touchItinerary → process_5_daily_itinerary.updated_at, which
    //    sync_normalized_tables propagates to the days row).
    route_segments::touch_day(conn, plan_id, destination, day, &now_db).await?;

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
    let summary = format!("D{day} slot{sort_order} {from}→{to}");
    crate::cascade::common::record_operation(
        conn,
        plan_id,
        "set-route-segment",
        &summary,
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    Ok(version_after)
}

async fn execute_bulk(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    segments: &[SegmentInput],
    source: &str,
) -> Result<i64, String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();

    // 0. The target `days` row MUST pre-exist (see execute_single).
    if !route_segments::day_exists(conn, plan_id, destination, day).await? {
        return Err(format!(
            "no days row for plan={plan_id} destination={destination} day={day}; \
             scaffold the itinerary first (travel scaffold-itinerary)"
        ));
    }

    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    // 1. DELETE ALL segments for (plan, dest, day) + INSERT new (domain writes via the DAL).
    route_segments::delete_all_for_day(conn, plan_id, destination, day).await?;
    for (i, s) in segments.iter().enumerate() {
        route_segments::insert_segment(
            conn,
            plan_id,
            destination,
            day,
            i as i64,
            &route_segments::SegmentWrite {
                from_place: s.from.clone(),
                to_place: s.to.clone(),
                mode: s.mode.clone(),
                duration_min: s.duration,
                notes: s.notes.clone(),
                start_time: s.start_time.clone(),
            },
            source,
            &format!("[{i}]"),
        )
        .await?;
    }

    // 2. Touch days.updated_at.
    route_segments::touch_day(conn, plan_id, destination, day, &now_db).await?;

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
    let summary = format!("D{day} {} segments", segments.len());
    crate::cascade::common::record_operation(
        conn,
        plan_id,
        "set-route-segments-bulk",
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

    // ── guard_segment: the fail-loud bundle (#3 / #4 / #5) ────────────────
    // These use the lint's OWN predicates verbatim, so the guard and the
    // validate-itinerary lint agree exactly. The good case must PASS; each
    // junk class must be REJECTED with a reason naming the stop/rule.

    #[test]
    fn guard_passes_normal_monorail_segment() {
        // The reference good leg: 赤嶺駅 → iias 沖縄豊崎, taxi (a real 単軌-area
        // segment driven by taxi). Both are clean place names, same country, and
        // mode isn't walking — must PASS untouched.
        assert!(
            guard_segment("\u{8D64}\u{5DBA}\u{99C5}", "iias \u{6C96}\u{7E04}\u{8C4A}\u{5D0E}", "driving").is_ok()
        ); // 赤嶺駅 → iias 沖縄豊崎
        // and the same pair as a transit leg is also fine
        assert!(
            guard_segment("\u{8D64}\u{5DBA}\u{99C5}", "iias \u{6C96}\u{7E04}\u{8C4A}\u{5D0E}", "transit").is_ok()
        );
    }

    #[test]
    fn guard_rejects_mode_only_stop() {
        // (#3) "步行" is a mode word, not a place — REJECT, naming the stop.
        let e = guard_segment("\u{6B65}\u{884C}", "iias \u{6C96}\u{7E04}\u{8C4A}\u{5D0E}", "driving")
            .expect_err("mode-only '步行' as a place must be rejected"); // 步行 → iias…
        assert!(e.contains("from"), "reason must name the offending side: {e}");
        assert!(e.contains("map-link/stop"), "reason must name the rule: {e}");
        // also as the `to` stop
        assert!(
            guard_segment("\u{8D64}\u{5DBA}\u{99C5}", "\u{55AE}\u{8ECC}", "driving").is_err()
        ); // 赤嶺駅 → 單軌 (mode word only)
    }

    #[test]
    fn guard_rejects_empty_after_clean_stop() {
        // (#3) a stop that cleans to empty (pure junk/mode/notes) has no usable
        // place name — REJECT.
        let raw = "\u{FF08}\u{55AE}\u{8ECC}\u{7D04}2\u{5206}\u{FF09}+\u{6B65}\u{884C}"; // （單軌約2分）+步行
        let e = guard_segment(raw, "iias \u{6C96}\u{7E04}\u{8C4A}\u{5D0E}", "driving")
            .expect_err("a stop that cleans to empty must be rejected");
        assert!(e.contains("map-link/stop"), "reason must name the rule: {e}");
    }

    #[test]
    fn guard_rejects_clock_time_only_stop() {
        // (#3) a bare clock time cleans to empty → no place → REJECT.
        assert!(
            guard_segment("08:30", "iias \u{6C96}\u{7E04}\u{8C4A}\u{5D0E}", "driving").is_err()
        );
    }

    #[test]
    fn guard_rejects_cross_country_pair() {
        // (#4) 紅樹林 (TW) → 那覇機場 (JP) is an ocean-spanning ground leg — REJECT.
        let e = guard_segment(
            "\u{7D05}\u{6A39}\u{6797}",
            "\u{90A3}\u{8987}\u{6A5F}\u{5834}",
            "driving",
        )
        .expect_err("a TW→JP ground leg must be rejected"); // 紅樹林 → 那覇機場
        assert!(e.contains("cross-country"), "reason must name the rule: {e}");
        assert!(e.contains("TW") && e.contains("JP"), "reason must name both countries: {e}");
    }

    #[test]
    fn guard_rejects_walking_over_rail() {
        // (#5) mode=walking but the leg names rail (單軌) — REJECT.
        let e = guard_segment(
            "\u{8D64}\u{5DBA}\u{99C5}",
            "\u{55AE}\u{8ECC}\u{6CBF}\u{7DDA}\u{67D0}\u{7AD9}",
            "walking",
        )
        .expect_err("a walking leg that rides rail must be rejected");
        assert!(e.contains("walking-over-rail"), "reason must name the rule: {e}");
        // the SAME pair as transit is fine (it's the walking claim that's wrong)
        assert!(
            guard_segment(
                "\u{8D64}\u{5DBA}\u{99C5}",
                "\u{55AE}\u{8ECC}\u{6CBF}\u{7DDA}\u{67D0}\u{7AD9}",
                "transit"
            )
            .is_ok()
        );
    }

    #[test]
    fn guard_does_not_false_reject_legit_parenthetical_place() {
        // Anti-false-positive: a real place whose parenthetical note clean_stop
        // strips (安里駅（單軌）) must PASS — clean_stop yields 安里駅, a valid stop.
        assert!(
            guard_segment(
                "\u{5B89}\u{91CC}\u{99C5}\u{FF08}\u{55AE}\u{8ECC}\u{FF09}",
                "\u{8D64}\u{5DBA}\u{99C5}",
                "transit"
            )
            .is_ok()
        ); // 安里駅（單軌）→ 赤嶺駅
        // an airport / hotel ASCII name is fine too
        assert!(guard_segment("Naha Airport", "Hotel Azat Naha", "driving").is_ok());
    }
}
