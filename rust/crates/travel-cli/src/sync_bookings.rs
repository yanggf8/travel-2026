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
use travel_db::repo::bookings::{
    self, BookingCurrentWrite, BookingEventWrite, ExistingBooking,
};

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

    let existing = bookings::current_snapshot_for_trips(&conn, &trip_ids).await?;

    // DELETE stale rows for these trip_ids (payload child rows first).
    for tid in &trip_ids {
        bookings::delete_current_for_trip(&conn, tid).await?;
    }

    // Upsert current bookings + diff-based events.
    for row in &bookings {
        bookings::upsert_current(&conn, &to_current_write(row)).await?;

        let prev = existing.get(&row.booking_key);
        match prev {
            None => {
                bookings::insert_event(&conn, &to_event_write(row, "created")).await?;
            }
            Some(p) => {
                if booking_changed(p, row) {
                    bookings::insert_event(&conn, &to_event_write(row, "updated")).await?;
                }
            }
        }
    }

    println!("✅ Synced {} bookings to Turso.", bookings.len());
    Ok(())
}

fn booking_changed(prev: &ExistingBooking, row: &BookingRow) -> bool {
    prev.status.as_deref() != Some(row.status.as_str())
        || prev.reference.as_deref() != row.reference.as_deref()
        || prev.book_by.as_deref() != row.book_by.as_deref()
        || prev.price_amount != row.price_amount
        || prev.title.as_deref() != Some(row.title.as_str())
}

fn to_current_write(row: &BookingRow) -> BookingCurrentWrite {
    BookingCurrentWrite {
        booking_key: row.booking_key.clone(),
        trip_id: row.trip_id.clone(),
        destination: row.destination.clone(),
        category: row.category.clone(),
        subtype: row.subtype.clone(),
        title: row.title.clone(),
        status: row.status.clone(),
        reference: row.reference.clone(),
        book_by: row.book_by.clone(),
        booked_at: row.booked_at.clone(),
        source_id: row.source_id.clone(),
        offer_id: row.offer_id.clone(),
        selected_date: row.selected_date.clone(),
        price_amount: row.price_amount,
        price_currency: row.price_currency.clone(),
        origin_path: row.origin_path.clone(),
        payload_kv: payload_to_kv(&row.payload),
    }
}

fn to_event_write(row: &BookingRow, event_type: &str) -> BookingEventWrite {
    BookingEventWrite {
        booking_key: row.booking_key.clone(),
        event_type: event_type.to_string(),
        new_status: row.status.clone(),
        reference: row.reference.clone(),
        book_by: row.book_by.clone(),
        amount: row.price_amount,
        currency: row.price_currency.clone(),
        payload_kv: payload_to_kv(&row.payload),
    }
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