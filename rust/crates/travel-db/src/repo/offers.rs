//! Global `offers` table access.

use libsql::Connection;

/// Typed row for the global `offers` table (including OTA provenance columns).
#[derive(Debug, Clone, Default)]
pub struct OfferRow {
    pub id: String,
    pub source_file: Option<String>,
    pub source_id: String,
    pub offer_type: String,
    pub name: Option<String>,
    pub price_per_person: Option<i64>,
    pub currency: Option<String>,
    pub region: Option<String>,
    pub destination: Option<String>,
    pub departure_date: Option<String>,
    pub return_date: Option<String>,
    pub nights: Option<i64>,
    pub availability: Option<String>,
    pub hotel_name: Option<String>,
    pub airline: Option<String>,
    pub flight_outbound: Option<String>,
    pub flight_return: Option<String>,
    pub scraped_at: String,
    pub capture_id: Option<String>,
    pub produced_by_job_id: Option<String>,
    pub produced_by_attempt_id: Option<String>,
    pub parser_method: Option<String>,
    pub capture_checksum: Option<String>,
    pub parser_rule_checksum: Option<String>,
    pub normalizer_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsertResult {
    pub inserted: u64,
    pub deduped: u64,
}

/// A parameterized `WHERE` fragment over the `offers` table plus its bound values, so callers
/// build dynamic offer queries without string-interpolating values (the `sql_quote()`+`format!`
/// anti-pattern this DAL retires). `clause` is `""` or `"WHERE …"` with `?1`,`?2`,… placeholders;
/// `params` are the matching values in order.
#[derive(Debug, Default, Clone)]
pub struct OfferWhere {
    pub clause: String,
    pub params: Vec<libsql::Value>,
}

/// Builder for an `offers` `WHERE` clause with bound parameters. Each `with_*` call appends one
/// predicate and its placeholder; `build()` joins them with ` AND `.
#[derive(Debug, Default)]
pub struct OfferFilter {
    conds: Vec<String>,
    params: Vec<libsql::Value>,
}

impl OfferFilter {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_placeholder(&self) -> usize {
        self.params.len() + 1
    }

    fn push_text(&mut self, col_op: &str, value: &str) {
        let n = self.next_placeholder();
        self.conds.push(format!("{col_op} ?{n}"));
        self.params.push(libsql::Value::Text(value.to_string()));
    }

    /// `destination = ?`
    pub fn destination(mut self, value: &str) -> Self {
        self.push_text("destination =", value);
        self
    }

    /// `region = ?`
    pub fn region(mut self, value: &str) -> Self {
        self.push_text("region =", value);
        self
    }

    /// `type = ?`
    pub fn offer_type(mut self, value: &str) -> Self {
        self.push_text("type =", value);
        self
    }

    /// `source_id = ?`
    pub fn source_id(mut self, value: &str) -> Self {
        self.push_text("source_id =", value);
        self
    }

    /// `departure_date >= ?`
    pub fn departure_from(mut self, value: &str) -> Self {
        self.push_text("departure_date >=", value);
        self
    }

    /// `departure_date <= ?`
    pub fn departure_to(mut self, value: &str) -> Self {
        self.push_text("departure_date <=", value);
        self
    }

    /// `price_per_person <= ?`
    pub fn max_price(mut self, value: i64) -> Self {
        let n = self.next_placeholder();
        self.conds.push(format!("price_per_person <= ?{n}"));
        self.params.push(libsql::Value::Integer(value));
        self
    }

    /// `source_id IN (?, ?, …)` from a comma-separated list (trimmed, empties dropped).
    /// A no-op when the list is empty (so the caller's other predicates still apply).
    pub fn source_id_in_csv(mut self, csv: &str) -> Self {
        let items: Vec<&str> = csv.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        if items.is_empty() {
            return self;
        }
        let mut placeholders = Vec::with_capacity(items.len());
        for item in items {
            let n = self.next_placeholder();
            placeholders.push(format!("?{n}"));
            self.params.push(libsql::Value::Text(item.to_string()));
        }
        self.conds.push(format!("source_id IN ({})", placeholders.join(",")));
        self
    }

    /// Build the final `WHERE …`/empty fragment + the bound params in placeholder order.
    pub fn build(self) -> OfferWhere {
        let clause = if self.conds.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", self.conds.join(" AND "))
        };
        OfferWhere {
            clause,
            params: self.params,
        }
    }
}

/// Insert one offer row. Returns inserted=1 or deduped=1 on ON CONFLICT DO NOTHING.
pub async fn insert(conn: &Connection, row: &OfferRow) -> Result<InsertResult, String> {
    let affected = conn
        .execute(
            "INSERT INTO offers \
             (id, source_file, source_id, type, name, price_per_person, currency, region, \
              destination, departure_date, return_date, nights, availability, hotel_name, airline, \
              flight_outbound, flight_return, scraped_at, capture_id, produced_by_job_id, \
              produced_by_attempt_id, parser_method, capture_checksum, parser_rule_checksum, \
              normalizer_version) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
              ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25) \
             ON CONFLICT(id, scraped_at) DO NOTHING",
            libsql::params![
                row.id.clone(),
                row.source_file.clone(),
                row.source_id.clone(),
                row.offer_type.clone(),
                row.name.clone(),
                row.price_per_person,
                row.currency.clone(),
                row.region.clone(),
                row.destination.clone(),
                row.departure_date.clone(),
                row.return_date.clone(),
                row.nights,
                row.availability.clone(),
                row.hotel_name.clone(),
                row.airline.clone(),
                row.flight_outbound.clone(),
                row.flight_return.clone(),
                row.scraped_at.clone(),
                row.capture_id.clone(),
                row.produced_by_job_id.clone(),
                row.produced_by_attempt_id.clone(),
                row.parser_method.clone(),
                row.capture_checksum.clone(),
                row.parser_rule_checksum.clone(),
                row.normalizer_version.clone(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    if affected == 1 {
        Ok(InsertResult {
            inserted: 1,
            deduped: 0,
        })
    } else {
        Ok(InsertResult {
            inserted: 0,
            deduped: 1,
        })
    }
}

/// Latest offer row for `(id, scraped_at)` PK lookup.
pub async fn latest(
    conn: &Connection,
    id: &str,
    scraped_at: &str,
) -> Result<Option<OfferRow>, String> {
    let mut rows = conn
        .query(
            "SELECT id, source_file, source_id, type, name, price_per_person, currency, region, \
             destination, departure_date, return_date, nights, availability, hotel_name, airline, \
             flight_outbound, flight_return, scraped_at, capture_id, produced_by_job_id, \
             produced_by_attempt_id, parser_method, capture_checksum, parser_rule_checksum, \
             normalizer_version \
             FROM offers WHERE id = ?1 AND scraped_at = ?2",
            libsql::params![id.to_string(), scraped_at.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    Ok(Some(OfferRow {
        id: row.get(0).map_err(|e| e.to_string())?,
        source_file: row.get(1).ok(),
        source_id: row.get(2).map_err(|e| e.to_string())?,
        offer_type: row.get(3).map_err(|e| e.to_string())?,
        name: row.get(4).ok(),
        price_per_person: row.get(5).ok(),
        currency: row.get(6).ok(),
        region: row.get(7).ok(),
        destination: row.get(8).ok(),
        departure_date: row.get(9).ok(),
        return_date: row.get(10).ok(),
        nights: row.get(11).ok(),
        availability: row.get(12).ok(),
        hotel_name: row.get(13).ok(),
        airline: row.get(14).ok(),
        flight_outbound: row.get(15).ok(),
        flight_return: row.get(16).ok(),
        scraped_at: row.get(17).map_err(|e| e.to_string())?,
        capture_id: row.get(18).ok(),
        produced_by_job_id: row.get(19).ok(),
        produced_by_attempt_id: row.get(20).ok(),
        parser_method: row.get(21).ok(),
        capture_checksum: row.get(22).ok(),
        parser_rule_checksum: row.get(23).ok(),
        normalizer_version: row.get(24).ok(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_row_constructs_with_defaults() {
        let row = OfferRow {
            id: "zztest_20260629".to_string(),
            source_id: "zztest".to_string(),
            offer_type: "package".to_string(),
            scraped_at: "2026-06-29T12:00:00Z".to_string(),
            ..Default::default()
        };
        assert_eq!(row.source_id, "zztest");
        assert_eq!(row.offer_type, "package");
    }

    fn text(v: &libsql::Value) -> Option<&str> {
        match v {
            libsql::Value::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }

    #[test]
    fn offer_filter_empty_is_no_where() {
        let w = OfferFilter::new().build();
        assert_eq!(w.clause, "");
        assert!(w.params.is_empty());
    }

    #[test]
    fn offer_filter_numbers_placeholders_in_order() {
        let w = OfferFilter::new()
            .destination("tokyo_2026")
            .region("kanto")
            .max_price(30000)
            .departure_from("2026-09-01")
            .build();
        assert_eq!(
            w.clause,
            "WHERE destination = ?1 AND region = ?2 AND price_per_person <= ?3 \
             AND departure_date >= ?4"
        );
        assert_eq!(text(&w.params[0]), Some("tokyo_2026"));
        assert_eq!(text(&w.params[1]), Some("kanto"));
        assert!(matches!(w.params[2], libsql::Value::Integer(30000)));
        assert_eq!(text(&w.params[3]), Some("2026-09-01"));
    }

    #[test]
    fn offer_filter_source_csv_expands_to_in_list() {
        let w = OfferFilter::new()
            .source_id_in_csv(" besttour , settour ,, ")
            .build();
        assert_eq!(w.clause, "WHERE source_id IN (?1,?2)");
        assert_eq!(text(&w.params[0]), Some("besttour"));
        assert_eq!(text(&w.params[1]), Some("settour"));
    }

    #[test]
    fn offer_filter_empty_csv_adds_no_condition() {
        let w = OfferFilter::new().source_id_in_csv("  , ,").destination("x").build();
        assert_eq!(w.clause, "WHERE destination = ?1");
        assert_eq!(w.params.len(), 1);
    }

    #[test]
    fn offer_filter_quote_chars_are_bound_not_interpolated() {
        // A value with a single quote must NOT need escaping — it's a bound param.
        let w = OfferFilter::new().destination("o'brien").build();
        assert_eq!(w.clause, "WHERE destination = ?1");
        assert_eq!(text(&w.params[0]), Some("o'brien"));
    }
}