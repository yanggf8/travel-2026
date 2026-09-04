//! Domestic accommodations — normalized table `domestic_accommodations` (no JSON).
//!
//! Slug-keyed reference data for Taiwan domestic stays (Phase A).
//! Read-only query with parameterized WHERE (OfferFilter pattern), no sql_quote.

use libsql::Connection;

/// One domestic accommodation row selected by the query.
#[derive(Debug, Clone)]
pub struct DomesticAccommodationRow {
    pub id: String,
    pub destination: String,
    pub hotel_name: String,
    pub room_type: String,
    pub sea_view: i64,
    pub max_occupancy: Option<i64>,
    pub price_twd: i64,
    pub currency: String,
    pub breakfast_included: i64,
    pub source: Option<String>,
    pub image_url: Option<String>,
    pub booking_url: Option<String>,
    /// Decision facts shown next to the price (all optional).
    pub room_size_sqm: Option<i64>,
    pub price_source: Option<String>,
    /// When the rate was read. A published price with no date is a lie waiting to happen.
    pub price_checked_at: Option<String>,
    pub free_cancel_until: Option<String>,
    pub rooms_left: Option<i64>,
    pub updated_at: String,
}

/// Builder for `domestic_accommodations` WHERE clause with bound params.
/// Pattern mirrors `OfferFilter`: each predicate appends `col = ?N` + Value.
#[derive(Debug, Default)]
pub struct DomesticAccommodationFilter {
    conds: Vec<String>,
    params: Vec<libsql::Value>,
}

impl DomesticAccommodationFilter {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_placeholder(&self) -> usize {
        self.params.len() + 1
    }

    /// `destination = ?`
    pub fn destination(mut self, value: &str) -> Self {
        let n = self.next_placeholder();
        self.conds.push(format!("destination = ?{n}"));
        self.params.push(libsql::Value::Text(value.to_string()));
        self
    }

    /// `sea_view = 1` (no value bind, fixed predicate)
    pub fn sea_view_only(mut self) -> Self {
        self.conds.push("sea_view = 1".to_string());
        self
    }

    /// `hotel_name LIKE %value%` (bound, escaped by binding)
    pub fn hotel_like(mut self, value: &str) -> Self {
        let n = self.next_placeholder();
        self.conds.push(format!("hotel_name LIKE ?{n}"));
        self.params
            .push(libsql::Value::Text(format!("%{value}%")));
        self
    }

    pub fn build(self) -> DomesticAccommodationWhere {
        let clause = if self.conds.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", self.conds.join(" AND "))
        };
        DomesticAccommodationWhere {
            clause,
            params: self.params,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct DomesticAccommodationWhere {
    pub clause: String,
    pub params: Vec<libsql::Value>,
}

/// Query `domestic_accommodations` with parameterized WHERE + limit.
pub async fn query(
    conn: &Connection,
    filter: DomesticAccommodationFilter,
    limit: i64,
) -> Result<Vec<DomesticAccommodationRow>, String> {
    let built = filter.build();
    let sql = format!(
        "SELECT id, destination, hotel_name, room_type, sea_view, max_occupancy, price_twd, currency, breakfast_included, source, image_url, booking_url, room_size_sqm, price_source, price_checked_at, free_cancel_until, rooms_left, updated_at \
         FROM domestic_accommodations {} ORDER BY price_twd ASC, hotel_name ASC LIMIT {limit}",
        built.clause
    );
    let mut rows = conn
        .query(&sql, built.params)
        .await
        .map_err(|e| format!("failed to query domestic_accommodations: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("failed to read domestic_accommodations row: {e}"))?
    {
        out.push(DomesticAccommodationRow {
            id: row.get(0).unwrap_or_default(),
            destination: row.get(1).unwrap_or_default(),
            hotel_name: row.get(2).unwrap_or_default(),
            room_type: row.get(3).unwrap_or_default(),
            sea_view: row.get(4).unwrap_or(0),
            max_occupancy: row.get(5).ok(),
            price_twd: row.get(6).unwrap_or(0),
            currency: row.get(7).unwrap_or_else(|_| "TWD".to_string()),
            breakfast_included: row.get(8).unwrap_or(0),
            source: row.get(9).ok(),
            image_url: row.get(10).ok(),
            booking_url: row.get(11).ok(),
            room_size_sqm: row.get(12).ok(),
            price_source: row.get(13).ok(),
            price_checked_at: row.get(14).ok(),
            free_cancel_until: row.get(15).ok(),
            rooms_left: row.get(16).ok(),
            updated_at: row.get(17).unwrap_or_default(),
        });
    }
    Ok(out)
}

/// Row to INSERT via `add-accommodation` (slug-keyed reference data write).
#[derive(Debug, Clone)]
pub struct NewDomesticAccommodation {
    pub id: String,
    pub destination: String,
    pub hotel_name: String,
    pub room_type: String,
    pub sea_view: i64,
    pub max_occupancy: Option<i64>,
    pub price_twd: i64,
    pub breakfast_included: i64,
    pub source: Option<String>,
    pub image_url: Option<String>,
    pub booking_url: Option<String>,
}

/// INSERT OR IGNORE one row. Returns affected rows: 1 = inserted, 0 = id already
/// exists (natural dedup — NOT an error; the CLI surfaces it as "already exists").
pub async fn insert(conn: &Connection, row: &NewDomesticAccommodation) -> Result<u64, String> {
    conn.execute(
        "INSERT OR IGNORE INTO domestic_accommodations \
         (id, destination, hotel_name, room_type, sea_view, max_occupancy, price_twd, currency, breakfast_included, source, image_url, booking_url, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'TWD', ?8, ?9, ?10, ?11, datetime('now'))",
        libsql::params![
            row.id.clone(),
            row.destination.clone(),
            row.hotel_name.clone(),
            row.room_type.clone(),
            row.sea_view,
            row.max_occupancy,
            row.price_twd,
            row.breakfast_included,
            row.source.clone(),
            row.image_url.clone(),
            row.booking_url.clone(),
        ],
    )
    .await
    .map_err(|e| format!("domestic_accommodations INSERT failed: {e}"))
}

/// Build the UPDATE for the caller-supplied columns (pure — unit-testable).
/// `sets` is an ordered list of `(column, value)`; the caller guarantees the column
/// names are literals it owns (never user input), so only the VALUES are bound.
/// `updated_at` always bumps. Caller guarantees `sets` is non-empty.
pub fn build_update(id: &str, sets: &[(&str, libsql::Value)]) -> (String, Vec<libsql::Value>) {
    let mut frags: Vec<String> = Vec::new();
    let mut params: Vec<libsql::Value> = Vec::new();
    for (col, val) in sets {
        params.push(val.clone());
        frags.push(format!("{col} = ?{}", params.len()));
    }
    frags.push("updated_at = datetime('now')".to_string());
    params.push(libsql::Value::Text(id.to_string()));
    let sql = format!(
        "UPDATE domestic_accommodations SET {} WHERE id = ?{}",
        frags.join(", "),
        params.len()
    );
    (sql, params)
}

/// UPDATE the given columns by id. Returns affected rows (0 = unknown id).
pub async fn update_fields(
    conn: &Connection,
    id: &str,
    sets: &[(&str, libsql::Value)],
) -> Result<u64, String> {
    let (sql, params) = build_update(id, sets);
    conn.execute(&sql, params)
        .await
        .map_err(|e| format!("domestic_accommodations UPDATE failed: {e}"))
}

/// DELETE one row by id. Returns affected rows (0 = unknown id).
pub async fn delete_by_id(conn: &Connection, id: &str) -> Result<u64, String> {
    conn.execute(
        "DELETE FROM domestic_accommodations WHERE id = ?1",
        libsql::params![id.to_string()],
    )
    .await
    .map_err(|e| format!("domestic_accommodations DELETE failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_empty_is_no_where() {
        let w = DomesticAccommodationFilter::new().build();
        assert_eq!(w.clause, "");
        assert!(w.params.is_empty());
    }

    #[test]
    fn filter_destination_and_sea_view() {
        let w = DomesticAccommodationFilter::new()
            .destination("jiufen")
            .sea_view_only()
            .build();
        assert_eq!(w.clause, "WHERE destination = ?1 AND sea_view = 1");
        assert_eq!(w.params.len(), 1);
        assert!(matches!(&w.params[0], libsql::Value::Text(s) if s == "jiufen"));
    }

    #[test]
    fn filter_hotel_like_is_bound() {
        let w = DomesticAccommodationFilter::new()
            .hotel_like("海論")
            .build();
        assert_eq!(w.clause, "WHERE hotel_name LIKE ?1");
        assert!(matches!(&w.params[0], libsql::Value::Text(s) if s == "%海論%"));
    }

    #[test]
    fn hotel_like_with_quote_is_bound_not_interpolated() {
        let w = DomesticAccommodationFilter::new()
            .hotel_like("a'b")
            .build();
        assert_eq!(w.clause, "WHERE hotel_name LIKE ?1");
        assert!(matches!(&w.params[0], libsql::Value::Text(s) if s == "%a'b%"));
    }

    fn txt(v: &str) -> libsql::Value {
        libsql::Value::Text(v.to_string())
    }

    #[test]
    fn build_update_two_columns() {
        let (sql, params) = build_update(
            "id1",
            &[("image_url", txt("https://img")), ("booking_url", txt("https://book"))],
        );
        assert_eq!(
            sql,
            "UPDATE domestic_accommodations SET image_url = ?1, booking_url = ?2, updated_at = datetime('now') WHERE id = ?3"
        );
        assert_eq!(params.len(), 3);
        assert!(matches!(&params[2], libsql::Value::Text(s) if s == "id1"));
    }

    #[test]
    fn build_update_single_column() {
        let (sql, params) = build_update("id2", &[("booking_url", txt("https://book"))]);
        assert_eq!(
            sql,
            "UPDATE domestic_accommodations SET booking_url = ?1, updated_at = datetime('now') WHERE id = ?2"
        );
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn build_update_binds_non_text_values_in_order() {
        let (sql, params) = build_update(
            "id3",
            &[
                ("room_size_sqm", libsql::Value::Integer(18)),
                ("rooms_left", libsql::Value::Integer(1)),
                ("price_source", txt("Booking.com")),
            ],
        );
        assert!(sql.starts_with(
            "UPDATE domestic_accommodations SET room_size_sqm = ?1, rooms_left = ?2, price_source = ?3, updated_at = datetime('now')"
        ));
        assert!(matches!(params[0], libsql::Value::Integer(18)));
        assert!(matches!(&params[2], libsql::Value::Text(s) if s == "Booking.com"));
        assert!(matches!(&params[3], libsql::Value::Text(s) if s == "id3"));
    }
}
