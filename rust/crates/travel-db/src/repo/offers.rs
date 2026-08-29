//! Global `offers` table access.

use crate::checksum::sha256_hex;
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
    /// Stable logical identity: the pre-disambiguation base id
    /// (`offer_row_id(source_id, product_code, departure, nights)`), captured before
    /// `disambiguate_ids` overwrites `id` with batch-order-dependent tie suffixes.
    pub offer_key: Option<String>,
    /// sha256 content fingerprint (see [`dedup_key`]); UNIQUE — the re-ingest dedup key.
    pub dedup_key: Option<String>,
    /// When this content was last observed; NULL on legacy rows. Liveness readers use
    /// `COALESCE(last_seen_at, scraped_at)`.
    pub last_seen_at: Option<String>,
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

    /// `capture_id = ?`
    pub fn capture_id(mut self, value: &str) -> Self {
        self.push_text("capture_id =", value);
        self
    }

    /// `produced_by_job_id = ?`
    pub fn produced_by_job_id(mut self, value: &str) -> Self {
        self.push_text("produced_by_job_id =", value);
        self
    }

    /// `produced_by_attempt_id = ?`
    pub fn produced_by_attempt_id(mut self, value: &str) -> Self {
        self.push_text("produced_by_attempt_id =", value);
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

    /// A departure-date window with optional inclusion of undated offers.
    ///
    /// When `start`/`end` are both `None` this is a no-op. Otherwise, with `include_undated=false`
    /// it adds a `departure_date IS NOT NULL` guard plus tight `>= ?`/`<= ?` bounds; with
    /// `include_undated=true` it drops the guard and widens each bound to
    /// `(departure_date IS NULL OR departure_date >= ?)`. Mirrors the `db query-offers` date block.
    pub fn departure_window(
        mut self,
        start: Option<&str>,
        end: Option<&str>,
        include_undated: bool,
    ) -> Self {
        if start.is_none() && end.is_none() {
            return self;
        }
        if !include_undated {
            self.conds.push("departure_date IS NOT NULL".to_string());
        }
        if let Some(s) = start {
            let n = self.next_placeholder();
            if include_undated {
                self.conds
                    .push(format!("(departure_date IS NULL OR departure_date >= ?{n})"));
            } else {
                self.conds.push(format!("departure_date >= ?{n}"));
            }
            self.params.push(libsql::Value::Text(s.to_string()));
        }
        if let Some(e) = end {
            let n = self.next_placeholder();
            if include_undated {
                self.conds
                    .push(format!("(departure_date IS NULL OR departure_date <= ?{n})"));
            } else {
                self.conds.push(format!("departure_date <= ?{n}"));
            }
            self.params.push(libsql::Value::Text(e.to_string()));
        }
        self
    }

    /// Only offers observed within the last `hours`. Liveness reads
    /// `COALESCE(last_seen_at, scraped_at)` so a re-observed (deduped) offer stays fresh.
    pub fn fresh_within_hours(mut self, hours: i64) -> Self {
        self.conds
            .push("COALESCE(last_seen_at, scraped_at) IS NOT NULL".to_string());
        let n = self.next_placeholder();
        self.conds.push(format!(
            "julianday(COALESCE(last_seen_at, scraped_at)) >= (julianday('now') - (?{n} / 24.0))"
        ));
        self.params.push(libsql::Value::Integer(hours));
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

/// Content fields hashed into [`dedup_key`], in hash order. Provenance/temporal/derived columns
/// are deliberately excluded — see [`DEDUP_EXCLUDED_FIELDS`].
pub const DEDUP_HASH_FIELDS: &[&str] = &[
    "offer_key",
    "source_id",
    "type",
    "name",
    "price_per_person",
    "currency",
    "destination",
    "departure_date",
    "return_date",
    "nights",
    "availability",
    "hotel_name",
    "airline",
    "flight_outbound",
    "flight_return",
];

/// Columns excluded from the dedup hash and why that is safe: provenance (`id`, `source_file`,
/// `capture_id`, `produced_by_*`, `parser_*`, `capture_checksum`, `parser_rule_checksum`,
/// `normalizer_version`), temporal (`scraped_at`, `created_at`, `last_seen_at`, `dedup_key`),
/// derived per job (`region`), or live-schema legacy columns not present on [`OfferRow`]
/// (`raw_data`, `hotel_area`, `includes` — the agent path never sets them).
pub const DEDUP_EXCLUDED_FIELDS: &[&str] = &[
    "id",
    "source_file",
    "region",
    "scraped_at",
    "last_seen_at",
    "dedup_key",
    "created_at",
    "raw_data",
    "hotel_area",
    "includes",
    "capture_id",
    "produced_by_job_id",
    "produced_by_attempt_id",
    "parser_method",
    "capture_checksum",
    "parser_rule_checksum",
    "normalizer_version",
];

/// Normalized length-prefixed text component: `"<char-len>:<trimmed lowercased>"`, with an
/// explicit sentinel for NULL/empty so `None` and `""` hash identically and `("ab","c")` can
/// never collide with `("a","bc")`.
fn hash_text(v: Option<&str>) -> String {
    match v.map(str::trim).filter(|t| !t.is_empty()) {
        None => "0:\u{0}".to_string(),
        Some(t) => {
            let collapsed = t.split_whitespace().collect::<Vec<_>>().join(" ");
            format!("{}:{}", collapsed.chars().count(), collapsed.to_ascii_lowercase())
        }
    }
}

fn hash_num(v: Option<i64>) -> String {
    v.map_or_else(|| "0:\u{0}".to_string(), |n| format!("i:{n}"))
}

/// sha256 content fingerprint for re-ingest dedup: domain-tagged `offer-v1` plus
/// length-prefixed normalized components in [`DEDUP_HASH_FIELDS`] order. Price is INSIDE the
/// hash — a price change must be a new row, or the UNIQUE index + DO NOTHING would silently
/// drop the exact signal this pipeline exists to capture.
pub fn dedup_key(row: &OfferRow) -> String {
    let mut buf = String::from("offer-v1");
    let components = [
        hash_text(row.offer_key.as_deref()),
        hash_text(Some(row.source_id.as_str())),
        hash_text(Some(row.offer_type.as_str())),
        hash_text(row.name.as_deref()),
        hash_num(row.price_per_person),
        hash_text(row.currency.as_deref()),
        hash_text(row.destination.as_deref()),
        hash_text(row.departure_date.as_deref()),
        hash_text(row.return_date.as_deref()),
        hash_num(row.nights),
        hash_text(row.availability.as_deref()),
        hash_text(row.hotel_name.as_deref()),
        hash_text(row.airline.as_deref()),
        hash_text(row.flight_outbound.as_deref()),
        hash_text(row.flight_return.as_deref()),
    ];
    for c in components {
        buf.push('\u{1f}');
        buf.push_str(&c);
    }
    sha256_hex(&buf)
}

/// Insert one offer row. Returns inserted=1, or deduped=1 when the content already exists
/// (PK `(id, scraped_at)` collision, or a `dedup_key` match when the same content is
/// re-ingested under a fresh `scraped_at`); on dedup the surviving row's `last_seen_at` is
/// bumped to the incoming `scraped_at`.
pub async fn insert(conn: &Connection, row: &OfferRow) -> Result<InsertResult, String> {
    let key = dedup_key(row);
    let affected = conn
        .execute(
            "INSERT INTO offers \
             (id, source_file, source_id, type, name, price_per_person, currency, region, \
              destination, departure_date, return_date, nights, availability, hotel_name, airline, \
              flight_outbound, flight_return, scraped_at, capture_id, produced_by_job_id, \
              produced_by_attempt_id, parser_method, capture_checksum, parser_rule_checksum, \
              normalizer_version, offer_key, dedup_key, last_seen_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
              ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28) \
             ON CONFLICT DO NOTHING",
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
                row.offer_key.clone(),
                key.clone(),
                row.scraped_at.clone(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    if affected == 1 {
        return Ok(InsertResult {
            inserted: 1,
            deduped: 0,
        });
    }
    // Content (or PK) already present: bump last_seen_at on the surviving row. The bare
    // DO NOTHING is required — a targeted (id, scraped_at) clause would RAISE on the
    // dedup_key unique violation instead of ignoring it.
    conn.execute(
        "UPDATE offers SET last_seen_at = ?1 WHERE dedup_key = ?2",
        libsql::params![row.scraped_at.clone(), key],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(InsertResult {
        inserted: 0,
        deduped: 1,
    })
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
        // dedup columns are not in latest()'s positional projection; None keeps the SELECT
        // list untouched so adding table columns can never shift these indexes.
        offer_key: None,
        dedup_key: None,
        last_seen_at: None,
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

    #[test]
    fn departure_window_none_is_noop() {
        let w = OfferFilter::new().departure_window(None, None, false).build();
        assert_eq!(w.clause, "");
        assert!(w.params.is_empty());
    }

    #[test]
    fn departure_window_tight_adds_not_null_guard() {
        let w = OfferFilter::new()
            .departure_window(Some("2026-09-01"), Some("2026-09-30"), false)
            .build();
        assert_eq!(
            w.clause,
            "WHERE departure_date IS NOT NULL AND departure_date >= ?1 \
             AND departure_date <= ?2"
        );
        assert_eq!(text(&w.params[0]), Some("2026-09-01"));
        assert_eq!(text(&w.params[1]), Some("2026-09-30"));
    }

    #[test]
    fn departure_window_undated_widens_bounds_and_drops_guard() {
        let w = OfferFilter::new()
            .departure_window(Some("2026-09-01"), None, true)
            .build();
        assert_eq!(
            w.clause,
            "WHERE (departure_date IS NULL OR departure_date >= ?1)"
        );
        assert_eq!(text(&w.params[0]), Some("2026-09-01"));
    }

    #[test]
    fn fresh_within_hours_binds_the_hour_count() {
        let w = OfferFilter::new().fresh_within_hours(24).build();
        assert_eq!(
            w.clause,
            "WHERE COALESCE(last_seen_at, scraped_at) IS NOT NULL AND \
             julianday(COALESCE(last_seen_at, scraped_at)) >= (julianday('now') - (?1 / 24.0))"
        );
        assert!(matches!(w.params[0], libsql::Value::Integer(24)));
    }

    #[test]
    fn offer_filter_provenance_ids_are_exact_equals() {
        let w = OfferFilter::new()
            .capture_id("cap-1")
            .produced_by_job_id("job-1")
            .produced_by_attempt_id("att-1")
            .build();
        assert_eq!(
            w.clause,
            "WHERE capture_id = ?1 AND produced_by_job_id = ?2 AND produced_by_attempt_id = ?3"
        );
        assert_eq!(text(&w.params[0]), Some("cap-1"));
        assert_eq!(text(&w.params[1]), Some("job-1"));
        assert_eq!(text(&w.params[2]), Some("att-1"));
    }

    #[test]
    fn placeholders_keep_numbering_across_mixed_predicates() {
        // destination(?1) + window(?2,?3) + fresh(?4) — numbering must stay sequential.
        let w = OfferFilter::new()
            .destination("tokyo_2026")
            .departure_window(Some("2026-09-01"), Some("2026-09-30"), true)
            .fresh_within_hours(48)
            .build();
        assert_eq!(
            w.clause,
            "WHERE destination = ?1 AND \
             (departure_date IS NULL OR departure_date >= ?2) AND \
             (departure_date IS NULL OR departure_date <= ?3) AND \
             COALESCE(last_seen_at, scraped_at) IS NOT NULL AND \
             julianday(COALESCE(last_seen_at, scraped_at)) >= (julianday('now') - (?4 / 24.0))"
        );
        assert_eq!(w.params.len(), 4);
    }

    fn sample_row() -> OfferRow {
        OfferRow {
            id: "zztest_pc123_20260901_3n".to_string(),
            source_id: "zztest".to_string(),
            offer_type: "package".to_string(),
            price_per_person: Some(25500),
            destination: Some("tokyo_2026".to_string()),
            departure_date: Some("2026-09-01".to_string()),
            return_date: Some("2026-09-04".to_string()),
            nights: Some(3),
            hotel_name: Some("Shinagawa Prince".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn dedup_key_is_deterministic_and_differs_on_price() {
        let a = dedup_key(&sample_row());
        assert_eq!(a, dedup_key(&sample_row()));
        assert_eq!(a.len(), 64, "sha256 hex");
        let mut price_changed = sample_row();
        price_changed.price_per_person = Some(26900);
        assert_ne!(dedup_key(&price_changed), a, "price is inside the hash");
    }

    #[test]
    fn dedup_key_ignores_provenance_and_temporal_fields() {
        let a = dedup_key(&sample_row());
        let mut reingest = sample_row();
        reingest.scraped_at = "2026-09-10T00:00:00Z".to_string();
        reingest.capture_id = Some("other-cap".to_string());
        reingest.produced_by_job_id = Some("other-job".to_string());
        reingest.produced_by_attempt_id = Some("other-attempt".to_string());
        reingest.region = Some("kanto".to_string());
        reingest.id = "zztest_pc123_20260901_3n_2".to_string();
        assert_eq!(dedup_key(&reingest), a, "re-ingest of same content must dedupe");
    }

    #[test]
    fn dedup_key_normalizes_case_whitespace_and_empty() {
        let a = dedup_key(&sample_row());
        let mut messy = sample_row();
        messy.hotel_name = Some("  shinagawa   PRINCE ".to_string());
        assert_eq!(dedup_key(&messy), a);
        let mut empty = sample_row();
        empty.hotel_name = Some("".to_string());
        let mut null = sample_row();
        null.hotel_name = None;
        assert_eq!(dedup_key(&empty), dedup_key(&null));
    }

    #[test]
    fn dedup_key_is_length_prefixed_ambiguity_safe() {
        let mut a = sample_row();
        a.hotel_name = Some("x".to_string());
        a.availability = Some("yz".to_string());
        let mut b = sample_row();
        b.hotel_name = Some("xy".to_string());
        b.availability = Some("z".to_string());
        assert_ne!(dedup_key(&a), dedup_key(&b), "length prefixes prevent ab|c == a|bc");
    }

    #[test]
    fn offers_column_set_is_covered_by_hash_or_excluded_lists() {
        // Drift guard: every `offers` column must be a conscious member of DEDUP_HASH_FIELDS or
        // DEDUP_EXCLUDED_FIELDS, so a future column can't silently escape the content hash.
        let all: std::collections::HashSet<_> = DEDUP_HASH_FIELDS
            .iter()
            .chain(DEDUP_EXCLUDED_FIELDS)
            .copied()
            .collect();
        for f in DEDUP_HASH_FIELDS {
            assert!(!DEDUP_EXCLUDED_FIELDS.contains(f), "{f} listed twice");
        }
        // The known live column set (schema.sql:544 + this change's 3 columns).
        let live = [
            "id", "source_id", "type", "name", "price_per_person", "currency", "region",
            "destination", "departure_date", "return_date", "nights", "availability",
            "hotel_name", "hotel_area", "airline", "flight_outbound", "flight_return",
            "includes", "scraped_at", "created_at", "source_file", "capture_id",
            "produced_by_job_id", "produced_by_attempt_id", "parser_method", "capture_checksum",
            "parser_rule_checksum", "normalizer_version", "offer_key", "dedup_key",
            "last_seen_at",
        ];
        for col in live {
            assert!(all.contains(col), "column {col} missing from hash+excluded lists");
        }
    }
}