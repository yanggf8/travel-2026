use chrono::Utc;
use libsql::Connection;
use travel_db::repo::offers::OfferRow;

pub const NORMALIZER_VERSION: &str = "travel-ota-regex-v1";
pub const AGENT_NORMALIZER_VERSION: &str = "travel-ota-agent-v1";

pub const VALID_PARAM_KEYS: &[&str] = &[
    "depart_date",
    "return_date",
    "nights",
    "pax",
    "region_code",
    "region_label",
];

pub fn now_iso() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub fn lease_expires(now: &str, lease_seconds: i64) -> Result<String, String> {
    let dt = chrono::DateTime::parse_from_rfc3339(now)
        .map_err(|e| format!("invalid ISO timestamp {now}: {e}"))?;
    Ok((dt + chrono::Duration::seconds(lease_seconds))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string())
}

pub async fn table_exists(conn: &Connection, table: &str, col: &str, value: &str) -> Result<bool, String> {
    let sql = format!("SELECT 1 FROM {table} WHERE {col} = ?1 LIMIT 1");
    let mut rows = conn
        .query(&sql, libsql::params![value.to_string()])
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows.next().await.map_err(|e| e.to_string())?.is_some())
}

pub fn offer_row_kind(product_type: &str) -> &'static str {
    match product_type {
        "flight" => "flight",
        "hotel" => "hotel",
        _ => "package",
    }
}

pub fn sanitize_id_part(value: &str) -> String {
    let out: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

pub fn product_code_from_url(url: &str) -> Option<String> {
    let base = url.split('?').next()?.trim_end_matches('/');
    let last = base.rsplit('/').next()?;
    let mut code = last.to_string();
    for suf in [".html", ".htm"] {
        if let Some(stripped) = code.strip_suffix(suf) {
            code = stripped.to_string();
        }
    }
    if code.is_empty() {
        None
    } else {
        Some(code)
    }
}

pub fn infer_destination(url: &str) -> Option<String> {
    let lower = url.to_lowercase();
    if lower.contains("kix") || lower.contains("%e4%ba%ac%e9%83%bd") {
        Some("osaka_2026".to_string())
    } else {
        None
    }
}

pub fn infer_region(url: &str) -> Option<String> {
    let lower = url.to_lowercase();
    if lower.contains("%e4%ba%ac%e9%83%bd") {
        Some("kyoto".to_string())
    } else if lower.contains("kix") {
        Some("kansai".to_string())
    } else {
        None
    }
}

pub fn offer_row_id(
    source_id: &str,
    product_code: &str,
    departure_date: Option<&str>,
    nights: Option<i64>,
) -> String {
    let mut parts = vec![source_id.to_string(), sanitize_id_part(product_code)];
    if let Some(d) = departure_date {
        if !d.is_empty() {
            parts.push(d.replace('-', ""));
        }
    }
    if let Some(n) = nights {
        parts.push(format!("{n}n"));
    }
    parts.join("_")
}

pub fn infer_airline_from_flight(flight: &str) -> Option<String> {
    if flight.len() < 2 {
        return None;
    }
    let prefix = &flight[..2];
    match prefix {
        "IT" => Some("台灣虎航".to_string()),
        "CI" => Some("中華航空".to_string()),
        "BR" => Some("長榮航空".to_string()),
        "JX" => Some("星宇航空".to_string()),
        "MM" => Some("樂桃航空".to_string()),
        "TR" => Some("酷航".to_string()),
        "GK" => Some("捷星航空".to_string()),
        _ => None,
    }
}

pub fn row_airline(explicit: Option<&str>, flight_outbound: Option<&str>) -> Option<String> {
    if let Some(a) = explicit {
        let t = a.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    flight_outbound
        .and_then(|f| infer_airline_from_flight(f.trim()))
}

pub fn ne(value: Option<&str>) -> Option<String> {
    value.and_then(|v| {
        let t = v.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

/// Map a parsed offer into a DB row with provenance.
pub fn parsed_to_offer_row(
    source_id: &str,
    product_type: &str,
    url: &str,
    capture_id: &str,
    scraped_at: &str,
    departure_date: Option<&str>,
    return_date: Option<&str>,
    nights: Option<i64>,
    price_per_person: i64,
    currency: &str,
    hotel_name: Option<&str>,
    airline: Option<&str>,
    flight_outbound: Option<&str>,
    flight_return: Option<&str>,
    job_id: &str,
    attempt_id: &str,
    parser_method: &str,
    capture_checksum: &str,
    parser_rule_checksum: Option<&str>,
    normalizer_version: &str,
) -> OfferRow {
    let product_code = product_code_from_url(url).unwrap_or_default();
    OfferRow {
        id: offer_row_id(
            source_id,
            &product_code,
            departure_date,
            nights,
        ),
        source_file: Some(format!("capture:{capture_id}")),
        source_id: source_id.to_string(),
        offer_type: offer_row_kind(product_type).to_string(),
        price_per_person: Some(price_per_person),
        currency: Some(currency.to_string()),
        region: infer_region(url),
        destination: infer_destination(url),
        departure_date: ne(departure_date),
        return_date: ne(return_date),
        nights,
        hotel_name: ne(hotel_name),
        airline: row_airline(airline, flight_outbound),
        flight_outbound: ne(flight_outbound),
        flight_return: ne(flight_return),
        scraped_at: scraped_at.to_string(),
        capture_id: Some(capture_id.to_string()),
        produced_by_job_id: Some(job_id.to_string()),
        produced_by_attempt_id: Some(attempt_id.to_string()),
        parser_method: Some(parser_method.to_string()),
        capture_checksum: Some(capture_checksum.to_string()),
        parser_rule_checksum: parser_rule_checksum.map(|s| s.to_string()),
        normalizer_version: Some(normalizer_version.to_string()),
        ..Default::default()
    }
}