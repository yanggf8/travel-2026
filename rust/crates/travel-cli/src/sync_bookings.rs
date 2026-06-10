//! sync-bookings — port of src/cli/commands/turso.ts (sync-bookings handler)
//! + the underlying syncBookingsFromPlanJson / extract-bookings.ts logic.
//!
//! Reads the normalized plan tables (package selection, airport transfers,
//! itinerary activities), derives flat booking rows, and upserts them into
//! `bookings_current` (+ `bookings_current_payload` child rows) with a diff-
//! based audit trail into `bookings_events` (+ `bookings_event_data`).
//!
//! The booking row shape is reproduced to match the TS extractor as observed
//! when it runs over the *assembled* plan object (TursoRepository.create):
//!   - package: source_id/price/selected_date/booked_at are NULL and the
//!     title degrades to `package - <offer_id>` (the assembled plan does not
//!     surface the nested chosen-offer fields the extractor reads). subtype
//!     defaults to `package`.
//!   - transfer: only `status='booked'` directions are emitted, price in JPY,
//!     title `<selected_title> (<direction>)`, payload from the selected row.
//!   - activity: only rows with a booking_status or booking_required=1, status
//!     mapped (not_required→skipped), price in JPY from cost_estimate.
//!
//! trip_id defaults to plan_id with hyphens→underscores (toDestSlug), or
//! --trip-id. --dry-run reports the count without writing.

use crate::booking_integrity::{extract_bookings, payload_to_kv, BookingRow};
use crate::db;
use libsql::{params, Connection};
use std::collections::HashMap;

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel sync-bookings [--plan-id <id>] [--trip-id <id>] [--dry-run]"
        );
        return Ok(());
    }

    let dry_run = args.iter().any(|a| a == "--dry-run");
    // --plan-id overrides the resolved plan_id (TS: --plan-id || env || ctx).
    let effective_plan_id = option_value(args, "--plan-id").unwrap_or(plan_id);
    let trip_id =
        option_value(args, "--trip-id").unwrap_or_else(|| to_dest_slug(&effective_plan_id));

    println!("Syncing bookings from plan \"{effective_plan_id}\"...");

    // Write tier — we upsert bookings_current + events.
    let conn = db::connect_write().await?;

    let (bookings, warnings) = extract_bookings(&conn, &effective_plan_id, &trip_id).await?;

    if !warnings.is_empty() {
        eprintln!("Warnings:");
        for w in &warnings {
            eprintln!("  - {w}");
        }
    }

    if bookings.is_empty() {
        // TS: returns { synced: 0 } and prints the synced line.
        println!(
            "{} 0 bookings to Turso.",
            if dry_run { "Would sync" } else { "Synced" }
        );
        return Ok(());
    }

    if dry_run {
        println!("Would sync {} bookings to Turso.", bookings.len());
        return Ok(());
    }

    // Fetch existing rows for the affected trip_ids for diff-based events.
    let mut trip_ids: Vec<String> = bookings.iter().map(|b| b.trip_id.clone()).collect();
    trip_ids.sort();
    trip_ids.dedup();

    let mut existing: HashMap<String, ExistingBooking> = HashMap::new();
    for tid in &trip_ids {
        let mut rows = conn
            .query(
                "SELECT booking_key, status, reference, book_by, price_amount, title \
                 FROM bookings_current WHERE trip_id = ?1",
                params![tid.clone()],
            )
            .await
            .map_err(|e| format!("bookings_current existing query failed: {e}"))?;
        while let Some(r) = rows
            .next()
            .await
            .map_err(|e| format!("bookings_current existing row read failed: {e}"))?
        {
            let key: String = r.get(0).unwrap_or_default();
            existing.insert(
                key.clone(),
                ExistingBooking {
                    status: r.get(1).ok().flatten(),
                    reference: r.get(2).ok().flatten(),
                    book_by: r.get(3).ok().flatten(),
                    price_amount: r.get(4).ok().flatten(),
                    title: r.get(5).ok().flatten(),
                },
            );
        }
    }

    // DELETE stale rows for these trip_ids (payload child rows first).
    for tid in &trip_ids {
        conn.execute(
            "DELETE FROM bookings_current_payload WHERE booking_key IN \
             (SELECT booking_key FROM bookings_current WHERE trip_id = ?1)",
            params![tid.clone()],
        )
        .await
        .map_err(|e| format!("bookings_current_payload DELETE failed: {e}"))?;
        conn.execute(
            "DELETE FROM bookings_current WHERE trip_id = ?1",
            params![tid.clone()],
        )
        .await
        .map_err(|e| format!("bookings_current DELETE failed: {e}"))?;
    }

    // Upsert current bookings + diff-based events.
    for row in &bookings {
        upsert_booking(&conn, row).await?;

        let prev = existing.get(&row.booking_key);
        match prev {
            None => emit_event(&conn, &row.booking_key, "created", row).await?,
            Some(p) => {
                if p.status.as_deref() != Some(row.status.as_str())
                    || p.reference.as_deref() != row.reference.as_deref()
                    || p.book_by.as_deref() != row.book_by.as_deref()
                    || p.price_amount != row.price_amount
                    || p.title.as_deref() != Some(row.title.as_str())
                {
                    emit_event(&conn, &row.booking_key, "updated", row).await?;
                }
            }
        }
    }

    println!("Synced {} bookings to Turso.", bookings.len());
    Ok(())
}

struct ExistingBooking {
    status: Option<String>,
    reference: Option<String>,
    book_by: Option<String>,
    price_amount: Option<i64>,
    title: Option<String>,
}

async fn upsert_booking(conn: &Connection, row: &BookingRow) -> Result<(), String> {
    conn.execute(
        "INSERT INTO bookings_current \
            (booking_key, trip_id, destination, category, subtype, title, status, \
             reference, book_by, booked_at, source_id, offer_id, selected_date, \
             price_amount, price_currency, origin_path, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, datetime('now')) \
         ON CONFLICT(booking_key) DO UPDATE SET \
            status = ?7, reference = ?8, book_by = ?9, booked_at = ?10, \
            price_amount = ?14, updated_at = datetime('now')",
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
    .map_err(|e| format!("bookings_current upsert failed: {e}"))?;

    conn.execute(
        "DELETE FROM bookings_current_payload WHERE booking_key = ?1",
        params![row.booking_key.clone()],
    )
    .await
    .map_err(|e| format!("bookings_current_payload DELETE failed: {e}"))?;

    for (i, (k, v)) in payload_to_kv(&row.payload).into_iter().enumerate() {
        conn.execute(
            "INSERT INTO bookings_current_payload (booking_key, sort_order, key, value) \
             VALUES (?1, ?2, ?3, ?4)",
            params![row.booking_key.clone(), i as i64, k, v],
        )
        .await
        .map_err(|e| format!("bookings_current_payload INSERT failed: {e}"))?;
    }
    Ok(())
}

async fn emit_event(
    conn: &Connection,
    booking_key: &str,
    event_type: &str,
    row: &BookingRow,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO bookings_events \
            (booking_key, event_type, new_status, reference, book_by, amount, currency, event_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
        params![
            booking_key.to_string(),
            event_type.to_string(),
            row.status.clone(),
            opt(&row.reference),
            opt(&row.book_by),
            opt_int(row.price_amount),
            row.price_currency.clone(),
        ],
    )
    .await
    .map_err(|e| format!("bookings_events INSERT failed: {e}"))?;

    for (i, (k, v)) in payload_to_kv(&row.payload).into_iter().enumerate() {
        conn.execute(
            "INSERT INTO bookings_event_data (booking_key, event_at, sort_order, key, value) \
             SELECT ?1, event_at, ?2, ?3, ?4 FROM bookings_events \
             WHERE booking_key = ?1 ORDER BY event_at DESC LIMIT 1",
            params![booking_key.to_string(), i as i64, k, v],
        )
        .await
        .map_err(|e| format!("bookings_event_data INSERT failed: {e}"))?;
    }
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────

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

fn to_dest_slug(plan_id: &str) -> String {
    plan_id.replace('-', "_")
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
