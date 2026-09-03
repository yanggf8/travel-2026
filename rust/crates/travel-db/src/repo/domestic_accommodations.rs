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
        "SELECT id, destination, hotel_name, room_type, sea_view, max_occupancy, price_twd, currency, breakfast_included, source, image_url, updated_at \
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
            updated_at: row.get(11).unwrap_or_default(),
        });
    }
    Ok(out)
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
}
