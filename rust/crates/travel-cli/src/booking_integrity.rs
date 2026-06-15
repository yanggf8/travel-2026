//! check-booking-integrity — port of src/cli/commands/turso.ts
//! (check-booking-integrity handler) + checkBookingIntegrity in
//! turso-service.ts. Read-only: compares bookings derived from the plan's
//! normalized tables against the rows already in `bookings_current`.
//!
//! This module ALSO hosts the shared booking extractor (`extract_bookings`,
//! `BookingRow`, `payload_to_kv`) reused by sync_bookings.rs, ported from
//! scripts/extract-bookings.ts. The extraction reads normalized tables
//! directly but reproduces the row shape the TS extractor produces when it
//! runs over the assembled plan object (see sync_bookings.rs for the package
//! field-degradation notes).

use crate::db;
use libsql::{params, Connection};

// ── booking row model ────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct BookingRow {
    pub booking_key: String,
    pub trip_id: String,
    pub destination: String,
    pub category: String, // package | transfer | activity
    pub subtype: Option<String>,
    pub title: String,
    pub status: String,
    pub reference: Option<String>,
    pub book_by: Option<String>,
    pub booked_at: Option<String>,
    pub source_id: Option<String>,
    pub offer_id: Option<String>,
    pub selected_date: Option<String>,
    pub price_amount: Option<i64>,
    pub price_currency: String,
    pub origin_path: String,
    /// Open payload as ordered key/value pairs (rendered as readable text,
    /// never JSON) — persisted to bookings_current_payload / bookings_event_data.
    pub payload: Vec<(String, String)>,
}

/// Flatten the already-ordered payload, dropping empty values (mirrors
/// payloadToKv skipping null/empty entries).
pub fn payload_to_kv(payload: &[(String, String)]) -> Vec<(String, String)> {
    payload
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .cloned()
        .collect()
}

fn build_key(parts: &[&str]) -> String {
    parts.join(":")
}

// ── status mapping ───────────────────────────────────────────────────

fn map_package_status(status: Option<&str>) -> String {
    match status {
        Some("booked") => "booked",
        Some("confirmed") => "confirmed",
        _ => "pending",
    }
    .to_string()
}

fn map_transfer_status(status: Option<&str>) -> String {
    match status {
        Some("booked") => "booked",
        Some("confirmed") => "confirmed",
        Some("planned") => "planned",
        _ => "pending",
    }
    .to_string()
}

fn map_activity_status(status: Option<&str>) -> String {
    match status {
        Some("booked") => "booked",
        Some("confirmed") => "confirmed",
        Some("pending") => "pending",
        Some("waitlist") => "waitlist",
        Some("not_required") => "skipped",
        _ => "pending",
    }
    .to_string()
}

// ── extractor ────────────────────────────────────────────────────────

/// List the destinations belonging to this plan (mirrors iterating
/// plan.destinations keys). Reads plan_destinations.
async fn list_destinations(conn: &Connection, plan_id: &str) -> Result<Vec<String>, String> {
    let mut rows = conn
        .query(
            "SELECT slug FROM plan_destinations WHERE plan_id = ?1 ORDER BY slug",
            params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_destinations query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("plan_destinations row read failed: {e}"))?
    {
        let slug: String = r.get(0).unwrap_or_default();
        if !slug.is_empty() {
            out.push(slug);
        }
    }
    Ok(out)
}

/// Extract all bookings (package + transfer + activity) for every destination
/// of `plan_id`, keyed by `trip_id`. Returns (rows, warnings).
pub async fn extract_bookings(
    conn: &Connection,
    plan_id: &str,
    trip_id: &str,
) -> Result<(Vec<BookingRow>, Vec<String>), String> {
    let mut warnings: Vec<String> = Vec::new();
    let dests = list_destinations(conn, plan_id).await?;
    if dests.is_empty() {
        warnings.push("No destinations found in plan".to_string());
        return Ok((Vec::new(), warnings));
    }

    let mut rows: Vec<BookingRow> = Vec::new();
    for dest in &dests {
        rows.extend(extract_package(conn, plan_id, trip_id, dest).await?);
        rows.extend(extract_transfer(conn, plan_id, trip_id, dest).await?);
        rows.extend(extract_activity(conn, plan_id, trip_id, dest).await?);
    }
    Ok((rows, warnings))
}

async fn extract_package(
    conn: &Connection,
    plan_id: &str,
    trip_id: &str,
    dest: &str,
) -> Result<Vec<BookingRow>, String> {
    // selected offer for this destination
    let mut sel = conn
        .query(
            "SELECT selected_offer_id FROM plan_offer_selection \
             WHERE plan_id = ?1 AND destination = ?2",
            params![plan_id.to_string(), dest.to_string()],
        )
        .await
        .map_err(|e| format!("plan_offer_selection query failed: {e}"))?;
    let selected_offer_id: Option<String> = match sel
        .next()
        .await
        .map_err(|e| format!("plan_offer_selection row read failed: {e}"))?
    {
        Some(r) => r.get(0).ok().flatten(),
        None => None,
    };
    let Some(selected_offer_id) = selected_offer_id.filter(|s| !s.is_empty()) else {
        return Ok(Vec::new());
    };

    // package process status
    let mut st = conn
        .query(
            "SELECT status FROM process_statuses \
             WHERE plan_id = ?1 AND destination = ?2 AND process_id = 'process_3_4_packages'",
            params![plan_id.to_string(), dest.to_string()],
        )
        .await
        .map_err(|e| format!("process_statuses query failed: {e}"))?;
    let status: Option<String> = match st
        .next()
        .await
        .map_err(|e| format!("process_statuses row read failed: {e}"))?
    {
        Some(r) => r.get(0).ok().flatten(),
        None => None,
    };

    // The assembled plan does not surface source_id / price / hotel name /
    // selected_date / selected_at to the TS extractor, so these degrade to
    // NULL and the title becomes `package - <offer_id>`. Reproduce that.
    let title = format!("package - {selected_offer_id}");

    Ok(vec![BookingRow {
        booking_key: build_key(&[trip_id, dest, "package", &selected_offer_id]),
        trip_id: trip_id.to_string(),
        destination: dest.to_string(),
        category: "package".to_string(),
        subtype: Some("package".to_string()),
        title,
        status: map_package_status(status.as_deref()),
        reference: None,
        book_by: None,
        booked_at: None,
        source_id: None,
        offer_id: Some(selected_offer_id),
        selected_date: None,
        price_amount: None,
        price_currency: "TWD".to_string(),
        origin_path: format!("destinations.{dest}.process_3_4_packages"),
        payload: Vec::new(),
    }])
}

async fn extract_transfer(
    conn: &Connection,
    plan_id: &str,
    trip_id: &str,
    dest: &str,
) -> Result<Vec<BookingRow>, String> {
    let mut out = Vec::new();
    for direction in ["arrival", "departure"] {
        let mut rows = conn
            .query(
                "SELECT status, selected_title, selected_route, selected_price_yen, \
                        selected_schedule, selected_id, selected_duration_min, \
                        selected_booking_url, selected_notes \
                 FROM airport_transfers \
                 WHERE plan_id = ?1 AND destination = ?2 AND direction = ?3",
                params![plan_id.to_string(), dest.to_string(), direction.to_string()],
            )
            .await
            .map_err(|e| format!("airport_transfers query failed: {e}"))?;
        let Some(r) = rows
            .next()
            .await
            .map_err(|e| format!("airport_transfers row read failed: {e}"))?
        else {
            continue;
        };

        let seg_status: Option<String> = r.get(0).ok().flatten();
        // Only emit transfers actually booked in advance.
        if seg_status.as_deref() != Some("booked") {
            continue;
        }
        // A selected transfer exists only if there is a selected_title (the
        // normalized projection of segment.selected). No title → no selected.
        let title: Option<String> = r.get(1).ok().flatten();
        let Some(title) = title.filter(|t| !t.is_empty()) else {
            continue;
        };

        let route: Option<String> = r.get(2).ok().flatten();
        let price_yen: Option<i64> = r.get(3).ok().flatten();
        let schedule: Option<String> = r.get(4).ok().flatten();
        let sel_id: Option<String> = r.get(5).ok().flatten();
        let duration_min: Option<i64> = r.get(6).ok().flatten();
        let booking_url: Option<String> = r.get(7).ok().flatten();
        let notes: Option<String> = r.get(8).ok().flatten();

        // payload = { ...selected, route, schedule } in selected-field order.
        let mut payload: Vec<(String, String)> = Vec::new();
        if let Some(v) = &sel_id {
            payload.push(("id".to_string(), v.clone()));
        }
        payload.push(("title".to_string(), title.clone()));
        if let Some(v) = &route {
            payload.push(("route".to_string(), v.clone()));
        }
        if let Some(v) = duration_min {
            payload.push(("duration_min".to_string(), v.to_string()));
        }
        if let Some(v) = price_yen {
            payload.push(("price_yen".to_string(), v.to_string()));
        }
        if let Some(v) = &schedule {
            payload.push(("schedule".to_string(), v.clone()));
        }
        if let Some(v) = &booking_url {
            payload.push(("booking_url".to_string(), v.clone()));
        }
        if let Some(v) = &notes {
            payload.push(("notes".to_string(), v.clone()));
        }

        out.push(BookingRow {
            booking_key: build_key(&[trip_id, dest, "transfer", direction]),
            trip_id: trip_id.to_string(),
            destination: dest.to_string(),
            category: "transfer".to_string(),
            subtype: Some(direction.to_string()),
            title: format!("{title} ({direction})"),
            status: map_transfer_status(seg_status.as_deref()),
            reference: None,
            book_by: None,
            booked_at: None,
            source_id: None,
            offer_id: None,
            selected_date: None,
            price_amount: price_yen,
            price_currency: "JPY".to_string(),
            origin_path: format!(
                "destinations.{dest}.process_3_transportation.airport_transfers.{direction}"
            ),
            payload,
        });
    }
    Ok(out)
}

async fn extract_activity(
    conn: &Connection,
    plan_id: &str,
    trip_id: &str,
    dest: &str,
) -> Result<Vec<BookingRow>, String> {
    // Iterate sessions day-by-day. noon is included so a booking-tracked lunch
    // reservation (e.g. a sit-down restaurant in the noon session) reaches the
    // ledger — the row-level filter below still requires booking_required||has_status,
    // so ordinary untracked noon meals never appear. (TS originally skipped noon.)
    let mut rows = conn
        .query(
            "SELECT day_number, session_type, id, title, booking_status, booking_required, \
                    booking_ref, book_by, cost_estimate, area, booking_url \
             FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 \
               AND session_type IN ('morning','noon','afternoon','evening') \
             ORDER BY day_number, \
               CASE session_type WHEN 'morning' THEN 0 WHEN 'noon' THEN 1 WHEN 'afternoon' THEN 2 ELSE 3 END, \
               sort_order",
            params![plan_id.to_string(), dest.to_string()],
        )
        .await
        .map_err(|e| format!("activities query failed: {e}"))?;

    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("activities row read failed: {e}"))?
    {
        let day_number: i64 = r.get(0).unwrap_or(0);
        let session: String = r.get(1).unwrap_or_default();
        let id: Option<String> = r.get(2).ok().flatten();
        let title: Option<String> = r.get(3).ok().flatten();
        let booking_status: Option<String> = r.get(4).ok().flatten();
        let booking_required: i64 = r.get(5).unwrap_or(0);
        let booking_ref: Option<String> = r.get(6).ok().flatten();
        let book_by: Option<String> = r.get(7).ok().flatten();
        let cost_estimate: Option<i64> = r.get(8).ok().flatten();
        let area: Option<String> = r.get(9).ok().flatten();
        let booking_url: Option<String> = r.get(10).ok().flatten();

        // Skip activities that don't need booking tracking.
        let has_status = booking_status.as_deref().map(|s| !s.is_empty()).unwrap_or(false);
        let required = booking_required != 0;
        if !has_status && !required {
            continue;
        }

        let activity_id = id
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("day{day_number}_{session}"));
        let title = title.filter(|t| !t.is_empty()).unwrap_or_else(|| "Unknown activity".to_string());

        // payload = { area, booking_url, tags } — tags omitted (TS spreads an
        // array → comma-joined; activity_tags not surfaced by the assembled
        // plan's per-activity object in the same shape, and empty values are
        // dropped by payloadToKv anyway).
        let mut payload: Vec<(String, String)> = Vec::new();
        if let Some(v) = &area {
            payload.push(("area".to_string(), v.clone()));
        }
        if let Some(v) = &booking_url {
            payload.push(("booking_url".to_string(), v.clone()));
        }

        out.push(BookingRow {
            booking_key: build_key(&[
                trip_id,
                dest,
                "activity",
                &day_number.to_string(),
                &session,
                &activity_id,
            ]),
            trip_id: trip_id.to_string(),
            destination: dest.to_string(),
            category: "activity".to_string(),
            subtype: Some(format!("day{day_number}_{session}")),
            title,
            status: map_activity_status(booking_status.as_deref()),
            reference: booking_ref.filter(|s| !s.is_empty()),
            book_by: book_by.filter(|s| !s.is_empty()),
            booked_at: None,
            source_id: None,
            offer_id: None,
            selected_date: None,
            price_amount: cost_estimate,
            price_currency: "JPY".to_string(),
            origin_path: format!(
                "destinations.{dest}.process_5_daily_itinerary.days[{}].{session}",
                day_number - 1
            ),
            payload,
        });
    }
    Ok(out)
}

// ── check-booking-integrity command ──────────────────────────────────

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel check-booking-integrity [--plan-id <id>] [--trip-id <id>]"
        );
        return Ok(());
    }
    let effective_plan_id = option_value(args, "--plan-id").unwrap_or(plan_id);
    let trip_id =
        option_value(args, "--trip-id").unwrap_or_else(|| effective_plan_id.replace('-', "_"));

    let conn = db::connect_read().await?;

    println!(
        "Checking booking integrity (plan \"{effective_plan_id}\" vs Turso DB)..."
    );

    // Plan-derived bookings.
    let (plan_bookings, warnings) =
        extract_bookings(&conn, &effective_plan_id, &trip_id).await?;

    // DB rows for this trip.
    let db_bookings = query_db_bookings(&conn, &trip_id).await?;

    let mut matches = 0i64;
    let mut mismatches: Vec<String> = Vec::new();
    let mut plan_only: Vec<String> = Vec::new();
    let mut db_only: Vec<String> = Vec::new();

    for pb in &plan_bookings {
        match db_bookings.iter().find(|d| d.booking_key == pb.booking_key) {
            None => plan_only.push(format!("{} ({}: {})", pb.booking_key, pb.category, pb.title)),
            Some(d) => {
                let mut diffs: Vec<String> = Vec::new();
                if d.status.as_deref() != Some(pb.status.as_str()) {
                    diffs.push(format!(
                        "status: plan={} db={}",
                        pb.status,
                        d.status.clone().unwrap_or_default()
                    ));
                }
                if norm(&d.reference) != norm(&pb.reference) {
                    diffs.push(format!(
                        "reference: plan={} db={}",
                        opt_str(&pb.reference),
                        opt_str(&d.reference)
                    ));
                }
                if norm(&d.book_by) != norm(&pb.book_by) {
                    diffs.push(format!(
                        "book_by: plan={} db={}",
                        opt_str(&pb.book_by),
                        opt_str(&d.book_by)
                    ));
                }
                if d.price_amount != pb.price_amount {
                    diffs.push(format!(
                        "price: plan={} db={}",
                        opt_int_str(pb.price_amount),
                        opt_int_str(d.price_amount)
                    ));
                }
                if d.title.as_deref() != Some(pb.title.as_str()) {
                    diffs.push(format!(
                        "title: plan=\"{}\" db=\"{}\"",
                        pb.title,
                        d.title.clone().unwrap_or_default()
                    ));
                }
                if diffs.is_empty() {
                    matches += 1;
                } else {
                    mismatches.push(format!("{}: {}", pb.booking_key, diffs.join(", ")));
                }
            }
        }
    }

    for d in &db_bookings {
        if !plan_bookings.iter().any(|p| p.booking_key == d.booking_key) {
            db_only.push(format!(
                "{} ({}: {})",
                d.booking_key,
                d.category.clone().unwrap_or_default(),
                d.title.clone().unwrap_or_default()
            ));
        }
    }

    for w in &warnings {
        mismatches.push(format!("extractor warning: {w}"));
    }

    println!("\nResults:");
    println!("  Matches:    {matches}");
    println!("  Mismatches: {}", mismatches.len());
    println!("  Plan-only:  {}", plan_only.len());
    println!("  DB-only:    {}", db_only.len());
    if !mismatches.is_empty() {
        println!("\nMismatches:");
        for m in &mismatches {
            println!("  - {m}");
        }
    }
    if !plan_only.is_empty() {
        println!("\nPlan-only (not in DB):");
        for p in &plan_only {
            println!("  - {p}");
        }
    }
    if !db_only.is_empty() {
        println!("\nDB-only (not in plan):");
        for d in &db_only {
            println!("  - {d}");
        }
    }
    Ok(())
}

struct DbBooking {
    booking_key: String,
    category: Option<String>,
    title: Option<String>,
    status: Option<String>,
    reference: Option<String>,
    book_by: Option<String>,
    price_amount: Option<i64>,
}

async fn query_db_bookings(conn: &Connection, trip_id: &str) -> Result<Vec<DbBooking>, String> {
    let mut rows = conn
        .query(
            "SELECT booking_key, category, title, status, reference, book_by, price_amount \
             FROM bookings_current WHERE trip_id = ?1 \
             ORDER BY category, destination, updated_at DESC",
            params![trip_id.to_string()],
        )
        .await
        .map_err(|e| format!("bookings_current query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("bookings_current row read failed: {e}"))?
    {
        out.push(DbBooking {
            booking_key: r.get(0).unwrap_or_default(),
            category: r.get(1).ok().flatten(),
            title: r.get(2).ok().flatten(),
            status: r.get(3).ok().flatten(),
            reference: r.get(4).ok().flatten(),
            book_by: r.get(5).ok().flatten(),
            price_amount: r.get(6).ok().flatten(),
        });
    }
    Ok(out)
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

/// Treat None and Some("") as equal (mirrors `x || null`).
fn norm(v: &Option<String>) -> Option<String> {
    match v {
        Some(s) if s.is_empty() => None,
        other => other.clone(),
    }
}

fn opt_str(v: &Option<String>) -> String {
    match v {
        Some(s) => s.clone(),
        None => "null".to_string(),
    }
}

fn opt_int_str(v: Option<i64>) -> String {
    match v {
        Some(n) => n.to_string(),
        None => "null".to_string(),
    }
}
