// `travel update-offer <offer-id> <date> <availability> [price] [seats] [source]`
//
// Port of the TS StateManager.updateOfferAvailability() +
// saveWithTracking('update-offer', ...) path. Despite living next to
// select-offer, this one does NOT fire the populate cascade — it only
// updates a single date's pricing/availability and emits one audit event.
//
// The acceptance gate is row parity against
//   TRAVEL_PLAN_ID=test-set-dates-2026 \
//     npm run travel -- update-offer <offer> <date> <avail> [price] [seats] [source]
// measured by before/after DB-row diff on the disposable plan.
//
// Writes (active destination only):
//   1. plan_offer_date_pricing — UPSERT the (offer, date) row. The TS path
//      merges `{ ...previousEntry, ...newData }` in memory, so OMITTED
//      price/seats PRESERVE the existing values; only availability (and any
//      provided price/seats) change. `note` is in-memory only — never
//      persisted (syncNormalizedTables does not write it).
//   2. plan_events / plan_event_data — one `offer_availability_updated`
//      event (process_3_4_packages) on BOTH dest_process and timeline.
//      KV: date, from (previous availability), offer_id, price, seats_remaining,
//      source, to. Omitted price/seats render as "undefined"; omitted source
//      defaults to "cli" (the CLI passes `source || 'cli'`).
//   3. operation_runs — 1 row, command_type='update-offer',
//      command_summary='<offer_id> <date> <availability>'.
//   4. plans.version — +1.
//
// Does NOT write:
//   - process_statuses (no cascade — availability change only)
//   - plan_offers.availability (the top-level column is untouched; only
//     the per-date pricing row changes)
//
// Volatile fields (normalized out of the diff): updated_at, run_id,
// started_at/completed_at, event_at.

use super::cascade::common::{
    insert_event, insert_kv_rows, next_dest_process_sort_order, next_timeline_sort_order,
    now_db_datetime, now_rfc3339, read_version, record_operation,
};
use crate::status::format_date;
use libsql::Connection;
use travel_db::repo::plan_offers;

const P34: &str = "process_3_4_packages";
const VALID_AVAILABILITY: &[&str] = &["available", "sold_out", "limited"];

/// Run update-offer. `price`/`seats` are `None` when the positional arg was
/// omitted. `source_arg` is the raw positional source (None when omitted);
/// the KV/event uses `source_arg` defaulted to "cli", but the header only
/// prints the `Source:` line when the arg was actually provided (mirrors the
/// TS `if (source)` guard).
pub async fn run(
    offer_id: String,
    date: String,
    availability: String,
    price: Option<i64>,
    seats: Option<i64>,
    source_arg: Option<String>,
    plan_id: String,
) -> Result<(), String> {
    if !VALID_AVAILABILITY.contains(&availability.as_str()) {
        eprintln!("Error: availability must be: available | sold_out | limited");
        std::process::exit(1);
    }
    let source = source_arg.clone().unwrap_or_else(|| "cli".to_string());

    // Header (byte-parity with offers.ts updateOfferCommand).
    println!("\n📝 Updating offer availability:");
    println!("   Offer: {offer_id}");
    println!("   Date: {}", format_date(&date));
    println!("   Availability: {availability}");
    if let Some(p) = price {
        println!("   Price: TWD {}", thousands(p));
    }
    if let Some(s) = seats {
        println!("   Seats: {s}");
    }
    if source_arg.is_some() {
        println!("   Source: {source}");
    }

    let conn = crate::db::connect_write()
        .await
        .map_err(|e| format!("failed to connect to Turso (write tier): {e}"))?;

    let dest = read_active_destination(&conn, &plan_id).await?;
    execute(
        &conn, &plan_id, &dest, &offer_id, &date, &availability, price, seats, &source,
    )
    .await?;

    println!("✅ Offer availability updated");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn execute(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    offer_id: &str,
    date: &str,
    availability: &str,
    price: Option<i64>,
    seats: Option<i64>,
    source: &str,
) -> Result<i64, String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    // Read the current (offer, date) pricing row: the previous availability
    // feeds the event `from` KV, and the existing price/seats are PRESERVED
    // when the args omit them (mirrors `{ ...previousEntry, ...data }`).
    let existing = read_date_pricing(conn, plan_id, dest, offer_id, date).await?;
    let Some(existing) = existing else {
        // The TS in-memory path would create a fresh entry, but the flush
        // INSERT requires price (NOT NULL). Selecting a brand-new date with
        // no price is a data error — fail loud, no silent NULL.
        return Err(format!(
            "no existing pricing row for offer {offer_id} on {date} — \
             update-offer cannot create a new date row without a price \
             (no local-data fallback)"
        ));
    };
    let previous_availability = existing.availability.clone();

    // Merge: provided values win; omitted values keep the existing row's.
    let merged_price = price.unwrap_or(existing.price);
    let merged_seats = seats.or(existing.seats_remaining);

    // --- 1. plan_offer_date_pricing UPSERT (one row) ---
    plan_offers::upsert_date_pricing(
        conn, plan_id, dest, offer_id, date, merged_price, availability, merged_seats, &now_db,
    )
    .await?;

    // --- 2. offer_availability_updated event (dest_process + timeline) ---
    // KV value rendering mirrors TS `String(value)`: omitted price/seats
    // become "undefined".
    let price_kv = price.map(|p| p.to_string()).unwrap_or_else(|| "undefined".to_string());
    let seats_kv = seats.map(|s| s.to_string()).unwrap_or_else(|| "undefined".to_string());
    let from_kv = previous_availability.unwrap_or_else(|| "undefined".to_string());
    let kv: Vec<(&str, String)> = vec![
        ("date", date.to_string()),
        ("from", from_kv),
        ("offer_id", offer_id.to_string()),
        ("price", price_kv),
        ("seats_remaining", seats_kv),
        ("source", source.to_string()),
        ("to", availability.to_string()),
    ];

    let dest_so = next_dest_process_sort_order(conn, plan_id, dest, P34).await?;
    insert_event(
        conn, plan_id, "dest_process", dest, P34, dest_so, "offer_availability_updated",
        &now_iso, None, None,
    )
    .await?;
    insert_kv_rows(conn, plan_id, "dest_process", dest, P34, dest_so, &kv).await?;

    let tl_so = next_timeline_sort_order(conn, plan_id).await?;
    insert_event(
        conn, plan_id, "timeline", "", P34, tl_so, "offer_availability_updated", &now_iso,
        None, None,
    )
    .await?;
    insert_kv_rows(conn, plan_id, "timeline", "", P34, tl_so, &kv).await?;

    // --- 3. operation_runs audit row + plans.version bump (shared audit-triad back half) ---
    let summary = format!("{offer_id} {date} {availability}");
    record_operation(
        conn,
        plan_id,
        "update-offer",
        &summary,
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    Ok(version_after)
}

struct DatePricing {
    price: i64,
    availability: Option<String>,
    seats_remaining: Option<i64>,
}

async fn read_date_pricing(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    offer_id: &str,
    date: &str,
) -> Result<Option<DatePricing>, String> {
    let mut rows = conn
        .query(
            "SELECT price, availability, seats_remaining FROM plan_offer_date_pricing \
             WHERE plan_id = ?1 AND destination = ?2 AND offer_id = ?3 AND date = ?4",
            libsql::params![
                plan_id.to_string(),
                dest.to_string(),
                offer_id.to_string(),
                date.to_string()
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let price: i64 = row.get(0).unwrap_or(0);
        let availability: Option<String> = match row.get::<String>(1) {
            Ok(s) if !s.is_empty() => Some(s),
            _ => None,
        };
        let seats_remaining: Option<i64> = row.get::<i64>(2).ok();
        return Ok(Some(DatePricing {
            price,
            availability,
            seats_remaining,
        }));
    }
    Ok(None)
}

async fn read_active_destination(conn: &Connection, plan_id: &str) -> Result<String, String> {
    let mut rows = conn
        .query(
            "SELECT active_destination FROM plan_metadata WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_metadata query failed: {e}"))?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let dest: String = row.get(0).map_err(|e| e.to_string())?;
        if dest.is_empty() {
            return Err(format!(
                "plan_metadata.active_destination is empty for plan_id={plan_id}"
            ));
        }
        return Ok(dest);
    }
    Err(format!(
        "plan_metadata row missing for plan_id={plan_id} (no local-data fallback)"
    ))
}

/// Render an integer with thousands separators, matching JS
/// `Number.prototype.toLocaleString()` for the en-US default (e.g.
/// 25000 → "25,000").
fn thousands(n: i64) -> String {
    let neg = n < 0;
    let digits = n.unsigned_abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::new();
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_formats_like_tolocale() {
        assert_eq!(thousands(25_000), "25,000");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(0), "0");
    }
}
