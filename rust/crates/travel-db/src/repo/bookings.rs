//! `activities` + `bookings_current` table access for the bookings views.

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
