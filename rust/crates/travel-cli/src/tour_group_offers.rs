// Shared tour-group offer functionality:
//   - TourGroupOfferRow struct
//   - insert_tour_group_offers()
//   - normalize_legacy_raw_json()
//   - validate_offer_row()
//   - find_scrape_attempt()
//   - upsert_scrape_attempt()
//
// Port of src/services/tour-group-service.ts.

use libsql::Connection;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct TourGroupOfferRow {
    pub run_id: String,
    pub offer_id: String,
    pub source_id: String,
    pub dest_region: String,
    pub depart_date: String,
    pub return_date: String,
    pub nights: i64,
    pub price_per_person_twd: i64,
    pub title: String,
    pub url: String,
    pub scraped_at: String,
    pub hotel_name: Option<String>,
    pub hotel_star_rating: Option<i64>,
    pub meals_included_count: Option<i64>,
    pub departure_status: Option<String>,
    pub seats_available: Option<i64>,
    pub min_group_size: Option<i64>,
    pub group_size_cap: Option<i64>,
    pub raw_confidence: Option<String>,
    pub raw_note: Option<String>,
    pub raw_flight: Option<String>,
    pub raw_flight_outbound: Option<String>,
    pub raw_flight_return: Option<String>,
    pub notes: Vec<Note>,
    pub product_kind: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Note {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct ScrapeAttemptRow {
    pub run_id: String,
    pub source_id: String,
    pub dest_region: String,
    pub nights: i64,
    pub status: String,
    pub offer_count: Option<i64>,
    pub parsed_count: Option<i64>,
    pub skipped_count: Option<i64>,
    pub error: Option<String>,
    pub attempted_at: Option<String>,
}

// REQUIRED_OFFER_FIELDS / validate_offer_row mirror tour-group-service.ts. The
// import path validates inline in parse_offer_row (it must list ALL missing
// fields from the raw JSON before the typed struct is built), so this typed-row
// validator is retained as a tested utility for future callers.
#[allow(dead_code)]
const REQUIRED_OFFER_FIELDS: &[&str] = &[
    "run_id",
    "offer_id",
    "source_id",
    "dest_region",
    "depart_date",
    "return_date",
    "nights",
    "price_per_person_twd",
    "title",
    "url",
    "scraped_at",
];

#[allow(dead_code)]
pub fn validate_offer_row(row: &TourGroupOfferRow) -> Result<(), Vec<String>> {
    let mut missing = Vec::new();

    for field in REQUIRED_OFFER_FIELDS {
        let value = match *field {
            "run_id" => Some(&row.run_id),
            "offer_id" => Some(&row.offer_id),
            "source_id" => Some(&row.source_id),
            "dest_region" => Some(&row.dest_region),
            "depart_date" => Some(&row.depart_date),
            "return_date" => Some(&row.return_date),
            "nights" => None, // Checked separately as i64
            "price_per_person_twd" => None, // Checked separately as i64
            "title" => Some(&row.title),
            "url" => Some(&row.url),
            "scraped_at" => Some(&row.scraped_at),
            _ => None,
        };

        if let Some(s) = value {
            if s.is_empty() {
                missing.push(field.to_string());
            }
        } else if *field == "nights" || *field == "price_per_person_twd" {
            // These are i64, always present in the struct
        } else {
            missing.push(field.to_string());
        }
    }

    if !missing.is_empty() {
        return Err(missing);
    }

    Ok(())
}

pub(crate) fn flatten_note_value(v: &Value) -> String {
    match v {
        Value::Null | Value::Bool(_) => String::new(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr.iter()
            .map(flatten_note_value)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("; "),
        Value::Object(obj) => obj.iter()
            .map(|(k, v)| format!("{}={}", k, flatten_note_value(v)))
            .collect::<Vec<_>>()
            .join("; "),
    }
}

pub async fn insert_tour_group_offers(
    conn: &Connection,
    rows: &[TourGroupOfferRow],
) -> Result<(), String> {
    if rows.is_empty() {
        return Ok(());
    }

    for row in rows {
        // Insert into shaping_tour_group_offers
        conn.execute(
            "INSERT OR REPLACE INTO shaping_tour_group_offers (
                run_id, offer_id, source_id, dest_region, depart_date, return_date, nights,
                price_per_person_twd, title, url, scraped_at,
                hotel_name, hotel_star_rating, meals_included_count, departure_status,
                seats_available, min_group_size, group_size_cap,
                raw_confidence, raw_note, raw_flight, raw_flight_outbound, raw_flight_return,
                product_kind
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11,
                ?12, ?13, ?14, ?15,
                ?16, ?17, ?18,
                ?19, ?20, ?21, ?22, ?23,
                ?24
            )",
            libsql::params![
                row.run_id.clone(),
                row.offer_id.clone(),
                row.source_id.clone(),
                row.dest_region.clone(),
                row.depart_date.clone(),
                row.return_date.clone(),
                row.nights,
                row.price_per_person_twd,
                row.title.clone(),
                row.url.clone(),
                row.scraped_at.clone(),
                row.hotel_name.clone(),
                row.hotel_star_rating,
                row.meals_included_count,
                row.departure_status.clone(),
                row.seats_available,
                row.min_group_size,
                row.group_size_cap,
                row.raw_confidence.clone(),
                row.raw_note.clone(),
                row.raw_flight.clone(),
                row.raw_flight_outbound.clone(),
                row.raw_flight_return.clone(),
                row.product_kind.clone().unwrap_or_else(|| "group_tour".to_string()),
            ],
        )
        .await
        .map_err(|e| format!("insert shaping_tour_group_offers failed: {e}"))?;

        // Delete existing notes for this offer
        conn.execute(
            "DELETE FROM shaping_tour_group_offer_notes WHERE run_id = ?1 AND offer_id = ?2",
            libsql::params![row.run_id.clone(), row.offer_id.clone()],
        )
        .await
        .map_err(|e| format!("delete notes failed: {e}"))?;

        // Insert new notes
        for (i, note) in row.notes.iter().enumerate() {
            if note.value.is_empty() {
                continue;
            }
            conn.execute(
                "INSERT INTO shaping_tour_group_offer_notes (
                    run_id, offer_id, sort_order, key, value
                ) VALUES (?1, ?2, ?3, ?4, ?5)",
                libsql::params![
                    row.run_id.clone(),
                    row.offer_id.clone(),
                    i as i64,
                    note.key.clone(),
                    note.value.clone(),
                ],
            )
            .await
            .map_err(|e| format!("insert note failed: {e}"))?;
        }
    }

    Ok(())
}

pub async fn find_scrape_attempt(
    conn: &Connection,
    run_id: &str,
    source_id: &str,
    dest_region: &str,
    nights: i64,
) -> Result<Option<ScrapeAttemptRow>, String> {
    let mut rows = conn.query(
        "SELECT run_id, source_id, dest_region, nights, status,
               offer_count, parsed_count, skipped_count, error, attempted_at
         FROM shaping_tour_group_scrape_attempts
         WHERE run_id = ?1 AND source_id = ?2 AND dest_region = ?3 AND nights = ?4",
        libsql::params![
            run_id.to_string(),
            source_id.to_string(),
            dest_region.to_string(),
            nights,
        ],
    )
    .await
    .map_err(|e| format!("find_scrape_attempt failed: {e}"))?;

    if let Some(row) = rows.next().await.map_err(|e| format!("row fetch failed: {e}"))? {
        let run_id: String = row.get(0).map_err(|e| format!("get run_id: {e}"))?;
        let source_id: String = row.get(1).map_err(|e| format!("get source_id: {e}"))?;
        let dest_region: String = row.get(2).map_err(|e| format!("get dest_region: {e}"))?;
        let nights: i64 = row.get(3).map_err(|e| format!("get nights: {e}"))?;
        let status: String = row.get(4).map_err(|e| format!("get status: {e}"))?;
        let offer_count: Option<i64> = row.get(5).map_err(|e| format!("get offer_count: {e}"))?;
        let parsed_count: Option<i64> = row.get(6).map_err(|e| format!("get parsed_count: {e}"))?;
        let skipped_count: Option<i64> = row.get(7).map_err(|e| format!("get skipped_count: {e}"))?;
        let error: Option<String> = row.get(8).map_err(|e| format!("get error: {e}"))?;
        let attempted_at: Option<String> = row.get(9).map_err(|e| format!("get attempted_at: {e}"))?;

        Ok(Some(ScrapeAttemptRow {
            run_id,
            source_id,
            dest_region,
            nights,
            status,
            offer_count,
            parsed_count,
            skipped_count,
            error,
            attempted_at,
        }))
    } else {
        Ok(None)
    }
}

pub async fn upsert_scrape_attempt(
    conn: &Connection,
    row: &ScrapeAttemptRow,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO shaping_tour_group_scrape_attempts (
            run_id, source_id, dest_region, nights, status,
            offer_count, parsed_count, skipped_count, error, attempted_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ON CONFLICT(run_id, source_id, dest_region, nights) DO UPDATE SET
            status = excluded.status,
            offer_count = excluded.offer_count,
            parsed_count = excluded.parsed_count,
            skipped_count = excluded.skipped_count,
            error = excluded.error,
            attempted_at = excluded.attempted_at",
        libsql::params![
            row.run_id.clone(),
            row.source_id.clone(),
            row.dest_region.clone(),
            row.nights,
            row.status.clone(),
            row.offer_count,
            row.parsed_count,
            row.skipped_count,
            row.error.clone(),
            row.attempted_at.clone(),
        ],
    )
    .await
    .map_err(|e| format!("upsert_scrape_attempt failed: {e}"))?;

    Ok(())
}

// Helper to generate base36 timestamp for offer_id (like Date.now().toString(36))
pub fn base36_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut s = String::new();
    let mut n = now;

    if n == 0 {
        return "0".to_string();
    }

    const BASE36_CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

    while n > 0 {
        let d = (n % 36) as u32;
        s.insert(0, BASE36_CHARS[d as usize] as char);
        n /= 36;
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cascade::common::now_rfc3339;

    #[test]
    fn test_base36_timestamp() {
        let ts = base36_timestamp();
        assert!(!ts.is_empty());
        assert!(ts.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn test_validate_offer_row() {
        let row = TourGroupOfferRow {
            run_id: "run123".to_string(),
            offer_id: "offer123".to_string(),
            source_id: "besttour".to_string(),
            dest_region: "okinawa".to_string(),
            depart_date: "2026-06-01".to_string(),
            return_date: "2026-06-05".to_string(),
            nights: 4,
            price_per_person_twd: 15000,
            title: "Test Offer".to_string(),
            url: "https://example.com".to_string(),
            scraped_at: now_rfc3339(),
            hotel_name: Some("Test Hotel".to_string()),
            hotel_star_rating: Some(4),
            meals_included_count: Some(0),
            departure_status: Some("available".to_string()),
            seats_available: Some(2),
            min_group_size: Some(2),
            group_size_cap: None,
            raw_confidence: None,
            raw_note: None,
            raw_flight: None,
            raw_flight_outbound: None,
            raw_flight_return: None,
            notes: vec![],
            product_kind: Some("fit".to_string()),
        };

        assert!(validate_offer_row(&row).is_ok());
    }
}
