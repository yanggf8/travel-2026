//! mark-booked — port of src/cli/commands/mark-booked.ts.
//!
//! Transitions P3+4 (packages), P3 (transport), P4 (accommodation) from
//! selected/populated → booking → booked for one destination, then records
//! the confirmation and re-points the workflow at the daily itinerary.
//!
//! Per-process rule (mirrors the TS loop):
//!   - no status row            → skip ("no status")
//!   - already booked/confirmed → skip ("already <status>")
//!   - status ∉ {selected,populated} → skip ("cannot book from <status>")
//!   - else → status = 'booked', clear the dirty flag
//!
//! Side effects on success (matching saveWithTracking + the TS body):
//!   - process_statuses + event_log_dest_processes status → booked
//!   - cascade_dirty_flags.dirty → 0 for the booked processes
//!   - event_log_state.current_focus → '<dest>.process_5_daily_itinerary'
//!   - event_log_next_actions replaced with the 3 follow-up actions
//!   - 1 booking_confirmed timeline plan_event (+ KV data)
//!   - operation_runs audit row + plans.version bump
//!   - bookings re-synced into bookings_current (TS: save() → syncBookingsToDb)
//!
//! --dry-run prints the planned transitions without writing.

use crate::db;
use libsql::{params, Connection};

const PROCESSES: &[(&str, &str)] = &[
    ("process_3_4_packages", "P3+4 Packages"),
    ("process_3_transportation", "P3 Transport"),
    ("process_4_accommodation", "P4 Accommodation"),
];

const NEXT_ACTIONS: &[&str] = &[
    "plan_daily_itinerary",
    "book_teamlab_tickets",
    "research_restaurant_reservations",
];

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel mark-booked [--dest slug] [--dry-run]");
        return Ok(());
    }
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let dest_opt = option_value(args, "--dest");

    let conn = db::connect_write().await?;
    let destination = read_destination(&conn, &plan_id, dest_opt.as_deref()).await?;

    println!("\n🎫 Marking booking as confirmed for {destination}:");

    struct Plan {
        process_id: &'static str,
        name: &'static str,
        from: String,
    }
    let mut to_book: Vec<Plan> = Vec::new();

    for (process_id, name) in PROCESSES {
        let current = read_process_status(&conn, &plan_id, &destination, process_id).await?;
        match current.as_deref() {
            None | Some("") => {
                println!("   ⏭️  {name}: skipped (no status)");
            }
            Some(s @ ("booked" | "confirmed")) => {
                println!("   ✓  {name}: already {s}");
            }
            Some(s) if s == "selected" || s == "populated" => {
                if !dry_run {
                    to_book.push(Plan {
                        process_id,
                        name,
                        from: s.to_string(),
                    });
                } else {
                    println!("   ✅ {name}: {s} → booking → booked");
                }
            }
            Some(s) => {
                println!("   ⚠️  {name}: cannot book from {s}");
            }
        }
    }

    if dry_run {
        println!("\n🔸 DRY RUN - no changes saved");
        return Ok(());
    }

    let now_db = now_db_datetime();
    let now_iso = now_rfc3339();

    let version_before = read_version(&conn, &plan_id).await?;
    let version_after = version_before + 1;

    for p in &to_book {
        set_process_status(&conn, &plan_id, &destination, p.process_id, "booked", &now_db).await?;
        clear_dirty(&conn, &plan_id, &destination, p.process_id, &now_db).await?;
        println!("   ✅ {}: {} → booking → booked", p.name, p.from);
    }

    // booking_confirmed timeline event.
    let processes_joined = PROCESSES
        .iter()
        .map(|(id, _)| *id)
        .collect::<Vec<_>>()
        .join(", ");
    let timeline_so = next_timeline_sort_order(&conn, &plan_id).await?;
    insert_timeline_event(
        &conn,
        &plan_id,
        timeline_so,
        "booking_confirmed",
        &now_iso,
        &[
            ("destination", destination.clone()),
            ("processes", processes_joined),
            ("confirmed_at", now_iso.clone()),
        ],
    )
    .await?;

    set_next_actions(&conn, &plan_id).await?;
    set_focus(&conn, &plan_id, &destination, &now_db).await?;

    let run_id = new_run_id();
    conn.execute(
        "INSERT INTO operation_runs \
            (run_id, plan_id, command_type, command_summary, status, \
             version_before, version_after, started_at, completed_at) \
         VALUES (?1, ?2, 'mark-booked', ?3, 'completed', ?4, ?5, ?6, ?6)",
        params![
            run_id,
            plan_id.clone(),
            destination.clone(),
            version_before,
            version_after,
            now_db.clone()
        ],
    )
    .await
    .map_err(|e| format!("operation_runs INSERT failed: {e}"))?;

    conn.execute(
        "UPDATE plans SET version = ?1, updated_at = ?2 WHERE plan_id = ?3",
        params![version_after, now_db.clone(), plan_id.clone()],
    )
    .await
    .map_err(|e| format!("plans UPDATE failed: {e}"))?;

    // Re-sync bookings (TS: save() → syncBookingsToDb()).
    let trip_id = plan_id.replace('-', "_");
    if let Err(e) = sync_bookings_inline(&conn, &plan_id, &trip_id).await {
        eprintln!("Warning: booking sync after mark-booked failed: {e}");
    }

    println!("\n✅ Booking marked as confirmed");
    println!("\nNext action: Plan daily itinerary with scaffold-itinerary or /p5-itinerary");
    Ok(())
}

// ── booking re-sync (reuse the shared extractor) ─────────────────────

async fn sync_bookings_inline(
    conn: &Connection,
    plan_id: &str,
    trip_id: &str,
) -> Result<(), String> {
    use crate::booking_integrity::{extract_bookings, payload_to_kv};

    let (bookings, _warnings) = extract_bookings(conn, plan_id, trip_id).await?;
    if bookings.is_empty() {
        return Ok(());
    }

    conn.execute(
        "DELETE FROM bookings_current_payload WHERE booking_key IN \
         (SELECT booking_key FROM bookings_current WHERE trip_id = ?1)",
        params![trip_id.to_string()],
    )
    .await
    .map_err(|e| format!("bookings_current_payload DELETE failed: {e}"))?;
    conn.execute(
        "DELETE FROM bookings_current WHERE trip_id = ?1",
        params![trip_id.to_string()],
    )
    .await
    .map_err(|e| format!("bookings_current DELETE failed: {e}"))?;

    for row in &bookings {
        conn.execute(
            "INSERT INTO bookings_current \
                (booking_key, trip_id, destination, category, subtype, title, status, \
                 reference, book_by, booked_at, source_id, offer_id, selected_date, \
                 price_amount, price_currency, origin_path, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, datetime('now'))",
            params![
                row.booking_key.clone(),
                row.trip_id.clone(),
                row.destination.clone(),
                row.category.clone(),
                opt(&row.subtype),
                row.title.clone(),
                row.status.clone(),
                opt(&row.reference),
                opt(&row.book_by),
                opt(&row.booked_at),
                opt(&row.source_id),
                opt(&row.offer_id),
                opt(&row.selected_date),
                opt_int(row.price_amount),
                row.price_currency.clone(),
                row.origin_path.clone(),
            ],
        )
        .await
        .map_err(|e| format!("bookings_current INSERT failed: {e}"))?;

        for (i, (k, v)) in payload_to_kv(&row.payload).into_iter().enumerate() {
            conn.execute(
                "INSERT INTO bookings_current_payload (booking_key, sort_order, key, value) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![row.booking_key.clone(), i as i64, k, v],
            )
            .await
            .map_err(|e| format!("bookings_current_payload INSERT failed: {e}"))?;
        }
    }
    Ok(())
}

// ── status / focus / next-action writers ─────────────────────────────

async fn set_process_status(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
    status: &str,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO process_statuses (plan_id, destination, process_id, status, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(plan_id, destination, process_id) DO UPDATE SET status = ?4, updated_at = ?5",
        params![
            plan_id.to_string(),
            dest.to_string(),
            process_id.to_string(),
            status.to_string(),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| format!("process_statuses upsert failed: {e}"))?;
    conn.execute(
        "INSERT INTO event_log_dest_processes (plan_id, destination, process_id, status, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(plan_id, destination, process_id) DO UPDATE SET status = ?4, updated_at = ?5",
        params![
            plan_id.to_string(),
            dest.to_string(),
            process_id.to_string(),
            status.to_string(),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| format!("event_log_dest_processes upsert failed: {e}"))?;
    Ok(())
}

async fn clear_dirty(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO cascade_dirty_flags (plan_id, destination, process_id, dirty, last_changed) \
         VALUES (?1, ?2, ?3, 0, ?4) \
         ON CONFLICT(plan_id, destination, process_id) DO UPDATE SET dirty = 0, last_changed = ?4",
        params![
            plan_id.to_string(),
            dest.to_string(),
            process_id.to_string(),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| format!("cascade_dirty_flags upsert failed: {e}"))?;
    Ok(())
}

async fn set_next_actions(conn: &Connection, plan_id: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM event_log_next_actions WHERE plan_id = ?1",
        params![plan_id.to_string()],
    )
    .await
    .map_err(|e| format!("event_log_next_actions DELETE failed: {e}"))?;
    for (i, action) in NEXT_ACTIONS.iter().enumerate() {
        conn.execute(
            "INSERT INTO event_log_next_actions (plan_id, sort_order, action) VALUES (?1, ?2, ?3)",
            params![plan_id.to_string(), i as i64, action.to_string()],
        )
        .await
        .map_err(|e| format!("event_log_next_actions INSERT failed: {e}"))?;
    }
    Ok(())
}

async fn set_focus(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    now_db: &str,
) -> Result<(), String> {
    let focus = format!("{dest}.process_5_daily_itinerary");
    conn.execute(
        "UPDATE event_log_state SET current_focus = ?1, updated_at = ?2 WHERE plan_id = ?3",
        params![focus, now_db.to_string(), plan_id.to_string()],
    )
    .await
    .map_err(|e| format!("event_log_state UPDATE failed: {e}"))?;
    Ok(())
}

// ── plan_events (timeline) ───────────────────────────────────────────

async fn next_timeline_sort_order(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'timeline'",
            params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_events MAX(timeline) query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_events MAX(timeline) row read failed: {e}"))?
    {
        let m: i64 = row.get(0).unwrap_or(-1);
        return Ok(m + 1);
    }
    Ok(0)
}

async fn insert_timeline_event(
    conn: &Connection,
    plan_id: &str,
    sort_order: i64,
    event: &str,
    event_at: &str,
    kv: &[(&str, String)],
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_events \
         WHERE plan_id = ?1 AND scope = 'timeline' AND destination = '' \
           AND process_id = '' AND sort_order = ?2",
        params![plan_id.to_string(), sort_order],
    )
    .await
    .map_err(|e| format!("plan_events DELETE failed: {e}"))?;
    conn.execute(
        "INSERT INTO plan_events \
            (plan_id, scope, destination, process_id, sort_order, event, event_at, from_state, to_state) \
         VALUES (?1, 'timeline', '', '', ?2, ?3, ?4, NULL, NULL)",
        params![plan_id.to_string(), sort_order, event.to_string(), event_at.to_string()],
    )
    .await
    .map_err(|e| format!("plan_events INSERT failed: {e}"))?;
    conn.execute(
        "DELETE FROM plan_event_data \
         WHERE plan_id = ?1 AND scope = 'timeline' AND destination = '' \
           AND process_id = '' AND sort_order = ?2",
        params![plan_id.to_string(), sort_order],
    )
    .await
    .map_err(|e| format!("plan_event_data DELETE failed: {e}"))?;
    for (k, v) in kv {
        conn.execute(
            "INSERT INTO plan_event_data \
                (plan_id, scope, destination, process_id, sort_order, key, value) \
             VALUES (?1, 'timeline', '', '', ?2, ?3, ?4)",
            params![plan_id.to_string(), sort_order, k.to_string(), v.clone()],
        )
        .await
        .map_err(|e| format!("plan_event_data INSERT failed: {e}"))?;
    }
    Ok(())
}

// ── reads ────────────────────────────────────────────────────────────

async fn read_destination(
    conn: &Connection,
    plan_id: &str,
    dest_opt: Option<&str>,
) -> Result<String, String> {
    if let Some(d) = dest_opt {
        return Ok(d.to_string());
    }
    let mut rows = conn
        .query(
            "SELECT active_destination FROM plan_metadata WHERE plan_id = ?1",
            params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_metadata query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_metadata row read failed: {e}"))?
    {
        let dest: String = row.get(0).unwrap_or_default();
        if dest.is_empty() {
            return Err("plan_metadata.active_destination is empty".to_string());
        }
        return Ok(dest);
    }
    Err(format!("plan_metadata row missing for plan_id={plan_id}"))
}

async fn read_process_status(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT status FROM process_statuses \
             WHERE plan_id = ?1 AND destination = ?2 AND process_id = ?3",
            params![plan_id.to_string(), dest.to_string(), process_id.to_string()],
        )
        .await
        .map_err(|e| format!("process_statuses query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("process_statuses row read failed: {e}"))?
    {
        return Ok(row.get(0).ok().flatten());
    }
    Ok(None)
}

async fn read_version(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT version FROM plans WHERE plan_id = ?1",
            params![plan_id.to_string()],
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

// ── small helpers ────────────────────────────────────────────────────

fn option_value(args: &[String], name: &str) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == name {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}

fn opt(v: &Option<String>) -> libsql::Value {
    match v {
        Some(s) => libsql::Value::Text(s.clone()),
        None => libsql::Value::Null,
    }
}

fn opt_int(v: Option<i64>) -> libsql::Value {
    match v {
        Some(n) => libsql::Value::Integer(n),
        None => libsql::Value::Null,
    }
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
    let (y, mo, d, h, mi, s, ms) = now_civil();
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z")
}

fn now_db_datetime() -> String {
    let (y, mo, d, h, mi, s, _) = now_civil();
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
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
