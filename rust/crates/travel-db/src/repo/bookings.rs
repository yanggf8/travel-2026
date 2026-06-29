//! `activities`-table access for the bookings view.

use libsql::Connection;

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
