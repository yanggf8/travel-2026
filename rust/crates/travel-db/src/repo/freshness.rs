//! Freshness aggregates over `offers` / `plan_offer_provenance`.
//!
//! Both query paths return the same `(count, newest_scraped_at)` shape; the caller turns that into
//! a recommendation. Bound params only (replaces `freshness.rs`'s `sql_quote()` interpolation).

use crate::repo::offers::OfferWhere;
use libsql::Connection;

/// `(count, newest_scraped_at)` for a freshness probe.
#[derive(Debug, Clone, Default)]
pub struct FreshnessCount {
    pub count: i64,
    pub newest: Option<String>,
}

/// Legacy `offers`-table freshness: `COUNT(*)`, `MAX(COALESCE(last_seen_at, scraped_at))` over a
/// parameterized WHERE — liveness follows re-observation, not first-seen. Build `where_built`
/// with `repo::offers::OfferFilter` (e.g. `.source_id(..).region(..)`).
pub async fn offers_freshness(
    conn: &Connection,
    where_built: OfferWhere,
) -> Result<FreshnessCount, String> {
    let sql = format!(
        "SELECT COUNT(*) AS cnt, MAX(COALESCE(last_seen_at, scraped_at)) AS newest FROM offers {}",
        where_built.clause
    );
    run_count_newest(conn, &sql, where_built.params).await
}

/// Plan-scoped freshness from `plan_offer_provenance` for one `(plan_id, destination, source_id)`.
pub async fn plan_provenance_freshness(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    source_id: &str,
) -> Result<FreshnessCount, String> {
    let sql = "SELECT SUM(COALESCE(offer_count, 0)) AS cnt, MAX(scraped_at) AS newest \
               FROM plan_offer_provenance \
               WHERE plan_id = ?1 AND destination = ?2 AND source_id = ?3";
    let params = vec![
        libsql::Value::Text(plan_id.to_string()),
        libsql::Value::Text(destination.to_string()),
        libsql::Value::Text(source_id.to_string()),
    ];
    run_count_newest(conn, sql, params).await
}

async fn run_count_newest(
    conn: &Connection,
    sql: &str,
    params: Vec<libsql::Value>,
) -> Result<FreshnessCount, String> {
    let mut rows = conn
        .query(sql, params)
        .await
        .map_err(|e| format!("failed to query freshness from Turso: {e}"))?;
    let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("failed to read freshness row: {e}"))?
    else {
        return Ok(FreshnessCount::default());
    };
    // Both queries alias the columns `cnt` and `newest`; read by name regardless of order.
    Ok(FreshnessCount {
        count: column_i64(&row, "cnt").unwrap_or(0),
        newest: column_string(&row, "newest").filter(|s| !s.is_empty()),
    })
}

fn column_i64(row: &libsql::Row, name: &str) -> Option<i64> {
    let idx = column_index(row, name)?;
    match row.get_value(idx).ok()? {
        libsql::Value::Integer(n) => Some(n),
        libsql::Value::Real(f) => Some(f as i64),
        libsql::Value::Text(s) => s.parse().ok(),
        _ => None,
    }
}

fn column_string(row: &libsql::Row, name: &str) -> Option<String> {
    let idx = column_index(row, name)?;
    match row.get_value(idx).ok()? {
        libsql::Value::Text(s) => Some(s),
        _ => None,
    }
}

fn column_index(row: &libsql::Row, name: &str) -> Option<i32> {
    let n = row.column_count();
    (0..n).find(|&i| row.column_name(i) == Some(name))
}

#[cfg(test)]
mod tests {
    use crate::repo::offers::OfferFilter;

    // The legacy freshness path builds its WHERE from OfferFilter; lock that source_id is always
    // present (bound) and region/date predicates append in order with sequential placeholders.
    #[test]
    fn legacy_freshness_where_is_parameterized() {
        let w = OfferFilter::new()
            .source_id("besttour")
            .region("kansai")
            .departure_from("2026-09-01")
            .departure_to("2026-09-30")
            .build();
        assert_eq!(
            w.clause,
            "WHERE source_id = ?1 AND region = ?2 AND departure_date >= ?3 \
             AND departure_date <= ?4"
        );
        assert_eq!(w.params.len(), 4);
    }
}
