//! Global `offers` table access.

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
}