//! `activities` + `bookings_*` table access for bookings views and `sync-bookings` writes.
//!
//! DAL boundary: owns the `bookings_current` / `bookings_current_payload` / `bookings_events` /
//! `bookings_event_data` domain-table SQL. The audit triad stays in `travel-cli` (`cascade::common`)
//! — sync-bookings writes no audit triad.

use libsql::Connection;
use std::collections::HashMap;

/// One `activities` row carrying a non-empty `book_by` deadline, keyed by its
/// `(day_number, session_type, sort_order)` position.
#[derive(Debug, Clone)]
pub struct BookByRow {
    pub day_number: i64,
    pub session_type: String,
    pub sort_order: i64,
    pub book_by: String,
}

/// Activities with a `book_by` deadline for one `(plan_id, destination)`.
/// Bound params (no string interpolation) — this replaces the `view_bookings.rs`
/// `sql_quote()` + `format!` pattern.
pub async fn book_by_deadlines(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<BookByRow>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number, session_type, sort_order, book_by \
             FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 \
               AND book_by IS NOT NULL AND book_by != ''",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("activities book_by: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("activities book_by row: {e}"))?
    {
        let book_by: String = r.get(3).unwrap_or_default();
        if book_by.is_empty() {
            continue;
        }
        out.push(BookByRow {
            day_number: r.get(0).unwrap_or(0),
            session_type: r.get(1).unwrap_or_default(),
            sort_order: r.get(2).unwrap_or(0),
            book_by,
        });
    }
    Ok(out)
}

/// Optional equality filters for `bookings_current` (`query-bookings`). Each `Some` field
/// becomes a bound `col = ?` predicate; `None` is skipped.
#[derive(Debug, Default, Clone)]
pub struct BookingsCurrentFilter<'a> {
    pub trip_id: Option<&'a str>,
    pub destination: Option<&'a str>,
    pub category: Option<&'a str>,
    pub status: Option<&'a str>,
}

/// The columns `query-bookings` renders from `bookings_current`.
#[derive(Debug, Clone)]
pub struct BookingCurrentRow {
    pub category: String,
    pub status: String,
    pub title: String,
    pub reference: String,
    pub book_by: String,
    pub price: Option<i64>,
}

/// Query `bookings_current` with bound params, preserving the SELECT/ORDER/LIMIT so row ordering
/// is identical to the prior inline SQL. Replaces `bookings.rs`'s `sql_quote()` WHERE.
/// Build the parameterized `bookings_current` WHERE fragment + bound params (pure, testable).
fn build_current_where(filter: &BookingsCurrentFilter<'_>) -> (String, Vec<libsql::Value>) {
    let mut conds: Vec<String> = Vec::new();
    let mut params: Vec<libsql::Value> = Vec::new();
    let mut push = |col: &str, value: Option<&str>| {
        if let Some(v) = value {
            params.push(libsql::Value::Text(v.to_string()));
            conds.push(format!("{col} = ?{}", params.len()));
        }
    };
    push("trip_id", filter.trip_id);
    push("destination", filter.destination);
    push("category", filter.category);
    push("status", filter.status);
    let clause = if conds.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conds.join(" AND "))
    };
    (clause, params)
}

pub async fn query_current(
    conn: &Connection,
    filter: &BookingsCurrentFilter<'_>,
    limit: i64,
) -> Result<Vec<BookingCurrentRow>, String> {
    let (where_clause, params) = build_current_where(filter);

    // Only the columns the caller actually renders. (The prior inline SQL also selected a
    // phantom `payload_text` column that does not exist on `bookings_current` — 17 cols, no
    // payload_text — so `query-bookings` errored in production with "no such column". Dropping
    // it both fixes the command and keeps the projection to what's used.)
    let sql = format!(
        "SELECT category, status, title, reference, book_by, price_amount \
         FROM bookings_current {where_clause} \
         ORDER BY category, destination, updated_at DESC LIMIT {limit}"
    );

    let mut rows = conn
        .query(&sql, params)
        .await
        .map_err(|e| format!("failed to query bookings from Turso: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("failed to read booking row: {e}"))?
    {
        out.push(BookingCurrentRow {
            category: r.get(0).unwrap_or_default(),
            status: r.get(1).unwrap_or_default(),
            title: r.get(2).unwrap_or_default(),
            reference: r.get(3).unwrap_or_default(),
            book_by: r.get(4).unwrap_or_default(),
            price: r.get(5).ok(),
        });
    }
    Ok(out)
}

/// Prior `bookings_current` row fields used for created-vs-updated diffing in `sync-bookings`.
#[derive(Debug, Clone)]
pub struct ExistingBooking {
    pub status: Option<String>,
    pub reference: Option<String>,
    pub book_by: Option<String>,
    pub price_amount: Option<i64>,
    pub title: Option<String>,
}

/// Typed payload for a `bookings_current` upsert (+ payload KV child rows).
#[derive(Debug, Clone)]
pub struct BookingCurrentWrite {
    pub booking_key: String,
    pub trip_id: String,
    pub destination: String,
    pub category: String,
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
    pub payload_kv: Vec<(String, String)>,
}

/// Typed payload for a `bookings_events` insert (+ event_data KV child rows).
#[derive(Debug, Clone)]
pub struct BookingEventWrite {
    pub booking_key: String,
    pub event_type: String,
    pub new_status: String,
    pub reference: Option<String>,
    pub book_by: Option<String>,
    pub amount: Option<i64>,
    pub currency: String,
    pub payload_kv: Vec<(String, String)>,
}

/// Snapshot existing `bookings_current` rows for the given trip_ids (diff read — must run BEFORE
/// stale deletes).
pub async fn current_snapshot_for_trips(
    conn: &Connection,
    trip_ids: &[String],
) -> Result<HashMap<String, ExistingBooking>, String> {
    let mut existing: HashMap<String, ExistingBooking> = HashMap::new();
    for tid in trip_ids {
        let mut rows = conn
            .query(
                "SELECT booking_key, status, reference, book_by, price_amount, title \
                 FROM bookings_current WHERE trip_id = ?1",
                libsql::params![tid.clone()],
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
    Ok(existing)
}

/// Delete stale `bookings_current` rows for one trip_id (payload child rows first).
pub async fn delete_current_for_trip(conn: &Connection, trip_id: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM bookings_current_payload WHERE booking_key IN \
         (SELECT booking_key FROM bookings_current WHERE trip_id = ?1)",
        libsql::params![trip_id.to_string()],
    )
    .await
    .map_err(|e| format!("bookings_current_payload DELETE failed: {e}"))?;
    conn.execute(
        "DELETE FROM bookings_current WHERE trip_id = ?1",
        libsql::params![trip_id.to_string()],
    )
    .await
    .map_err(|e| format!("bookings_current DELETE failed: {e}"))?;
    Ok(())
}

/// Upsert one `bookings_current` row and replace its payload KV child rows.
pub async fn upsert_current(conn: &Connection, row: &BookingCurrentWrite) -> Result<(), String> {
    conn.execute(
        "INSERT INTO bookings_current \
            (booking_key, trip_id, destination, category, subtype, title, status, \
             reference, book_by, booked_at, source_id, offer_id, selected_date, \
             price_amount, price_currency, origin_path, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, datetime('now')) \
         ON CONFLICT(booking_key) DO UPDATE SET \
            status = ?7, reference = ?8, book_by = ?9, booked_at = ?10, \
            price_amount = ?14, updated_at = datetime('now')",
        libsql::params![
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
        libsql::params![row.booking_key.clone()],
    )
    .await
    .map_err(|e| format!("bookings_current_payload DELETE failed: {e}"))?;

    for (i, (k, v)) in row.payload_kv.iter().enumerate() {
        conn.execute(
            "INSERT INTO bookings_current_payload (booking_key, sort_order, key, value) \
             VALUES (?1, ?2, ?3, ?4)",
            libsql::params![row.booking_key.clone(), i as i64, k.clone(), v.clone()],
        )
        .await
        .map_err(|e| format!("bookings_current_payload INSERT failed: {e}"))?;
    }
    Ok(())
}

/// Full trip-scoped re-sync of `bookings_current` (+ payload) — the `mark-booked` /
/// `save()→syncBookingsToDb()` path. DELETEs the trip's rows (via `delete_current_for_trip`)
/// then does a PLAIN `INSERT` per booking (NOT the `upsert_current` ON CONFLICT path — a
/// duplicate booking_key must fail loud here, matching the pre-DAL inline code) plus its
/// payload KV rows. Caller has already skipped the empty-list case (no rows ⇒ no delete/insert).
pub async fn resync_current(
    conn: &Connection,
    trip_id: &str,
    rows: &[BookingCurrentWrite],
) -> Result<(), String> {
    delete_current_for_trip(conn, trip_id).await?;
    for row in rows {
        conn.execute(
            "INSERT INTO bookings_current \
                (booking_key, trip_id, destination, category, subtype, title, status, \
                 reference, book_by, booked_at, source_id, offer_id, selected_date, \
                 price_amount, price_currency, origin_path, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, datetime('now'))",
            libsql::params![
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

        for (i, (k, v)) in row.payload_kv.iter().enumerate() {
            conn.execute(
                "INSERT INTO bookings_current_payload (booking_key, sort_order, key, value) \
                 VALUES (?1, ?2, ?3, ?4)",
                libsql::params![row.booking_key.clone(), i as i64, k.clone(), v.clone()],
            )
            .await
            .map_err(|e| format!("bookings_current_payload INSERT failed: {e}"))?;
        }
    }
    Ok(())
}

/// Insert one `bookings_events` row and its event_data KV child rows (correlated subquery).
pub async fn insert_event(conn: &Connection, event: &BookingEventWrite) -> Result<(), String> {
    conn.execute(
        "INSERT INTO bookings_events \
            (booking_key, event_type, new_status, reference, book_by, amount, currency, event_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
        libsql::params![
            event.booking_key.clone(),
            event.event_type.clone(),
            event.new_status.clone(),
            opt(&event.reference),
            opt(&event.book_by),
            opt_int(event.amount),
            event.currency.clone(),
        ],
    )
    .await
    .map_err(|e| format!("bookings_events INSERT failed: {e}"))?;

    for (i, (k, v)) in event.payload_kv.iter().enumerate() {
        conn.execute(
            "INSERT INTO bookings_event_data (booking_key, event_at, sort_order, key, value) \
             SELECT ?1, event_at, ?2, ?3, ?4 FROM bookings_events \
             WHERE booking_key = ?1 ORDER BY event_at DESC LIMIT 1",
            libsql::params![event.booking_key.clone(), i as i64, k.clone(), v.clone()],
        )
        .await
        .map_err(|e| format!("bookings_event_data INSERT failed: {e}"))?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(p: &[libsql::Value]) -> Vec<String> {
        p.iter()
            .filter_map(|v| match v {
                libsql::Value::Text(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn current_where_empty_when_no_filters() {
        let (clause, params) = build_current_where(&BookingsCurrentFilter::default());
        assert_eq!(clause, "");
        assert!(params.is_empty());
    }

    #[test]
    fn current_where_binds_each_present_filter_in_order() {
        let f = BookingsCurrentFilter {
            trip_id: Some("tokyo-2026"),
            destination: Some("tokyo_2026"),
            category: Some("activity"),
            status: Some("pending"),
        };
        let (clause, params) = build_current_where(&f);
        assert_eq!(
            clause,
            "WHERE trip_id = ?1 AND destination = ?2 AND category = ?3 AND status = ?4"
        );
        assert_eq!(
            texts(&params),
            vec!["tokyo-2026", "tokyo_2026", "activity", "pending"]
        );
    }

    #[test]
    fn current_where_skips_none_and_renumbers() {
        let f = BookingsCurrentFilter {
            destination: Some("kyoto_2026"),
            status: Some("booked"),
            ..Default::default()
        };
        let (clause, params) = build_current_where(&f);
        assert_eq!(clause, "WHERE destination = ?1 AND status = ?2");
        assert_eq!(texts(&params), vec!["kyoto_2026", "booked"]);
    }

    #[test]
    fn current_where_quote_value_is_bound_not_interpolated() {
        let f = BookingsCurrentFilter {
            category: Some("o'reilly"),
            ..Default::default()
        };
        let (clause, params) = build_current_where(&f);
        assert_eq!(clause, "WHERE category = ?1");
        assert_eq!(texts(&params), vec!["o'reilly"]);
    }
}
