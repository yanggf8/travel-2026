use crate::checksum::sha256_hex;
use libsql::Connection;

#[derive(Debug, Clone)]
pub struct ParserRuleRow {
    pub source_id: String,
    pub product_type: String,
    pub date_range_rx: String,
    pub nights_rx: String,
    pub nights_is_days: bool,
    pub price_marker: String,
    pub price_amount_rx: String,
    pub price_basis: String,
    pub pax_divisor: i64,
    pub flight_rx: String,
    pub hotel_anchor_rx: String,
    pub airline_rx: String,
    pub hotel_name_rx: String,
    pub currency: String,
    pub has_custom_parser: bool,
}

/// Load parser rule for `(source_id, product_type)`. Requires product_type — no source-only fallback.
pub async fn get(
    conn: &Connection,
    source_id: &str,
    product_type: &str,
) -> Result<Option<ParserRuleRow>, String> {
    let mut rows = conn
        .query(
            "SELECT source_id, product_type, date_range_rx, nights_rx, nights_is_days, \
             price_marker, price_amount_rx, price_basis, pax_divisor, flight_rx, \
             hotel_anchor_rx, airline_rx, hotel_name_rx, currency, has_custom_parser \
             FROM parser_rules WHERE source_id = ?1 AND product_type = ?2",
            libsql::params![source_id.to_string(), product_type.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    Ok(Some(ParserRuleRow {
        source_id: row.get(0).map_err(|e| e.to_string())?,
        product_type: row.get(1).map_err(|e| e.to_string())?,
        date_range_rx: row.get(2).map_err(|e| e.to_string())?,
        nights_rx: row.get(3).map_err(|e| e.to_string())?,
        nights_is_days: row.get::<i64>(4).unwrap_or(0) != 0,
        price_marker: row.get(5).map_err(|e| e.to_string())?,
        price_amount_rx: row.get(6).map_err(|e| e.to_string())?,
        price_basis: row.get(7).unwrap_or_else(|_| "total".to_string()),
        pax_divisor: row.get(8).unwrap_or(2),
        flight_rx: row.get(9).map_err(|e| e.to_string())?,
        hotel_anchor_rx: row.get(10).map_err(|e| e.to_string())?,
        airline_rx: row.get(11).unwrap_or_default(),
        hotel_name_rx: row.get(12).unwrap_or_default(),
        currency: row.get(13).unwrap_or_else(|_| "TWD".to_string()),
        has_custom_parser: row.get::<i64>(14).unwrap_or(0) != 0,
    }))
}

/// Deterministic ordered-field string for checksum input (not debug formatting).
pub fn checksum_input(rule: &ParserRuleRow) -> String {
    let nights_is_days = if rule.nights_is_days { "1" } else { "0" };
    let has_custom = if rule.has_custom_parser { "1" } else { "0" };
    let pax = rule.pax_divisor.to_string();
    [
        rule.source_id.as_str(),
        rule.product_type.as_str(),
        rule.date_range_rx.as_str(),
        rule.nights_rx.as_str(),
        nights_is_days,
        rule.price_marker.as_str(),
        rule.price_amount_rx.as_str(),
        rule.price_basis.as_str(),
        pax.as_str(),
        rule.flight_rx.as_str(),
        rule.hotel_anchor_rx.as_str(),
        rule.airline_rx.as_str(),
        rule.hotel_name_rx.as_str(),
        rule.currency.as_str(),
        has_custom,
    ]
    .join("|")
}

/// SHA-256 hex of the deterministic parser-rule checksum input.
pub fn checksum(rule: &ParserRuleRow) -> String {
    sha256_hex(&checksum_input(rule))
}