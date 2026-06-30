use chrono::Utc;
use libsql::Connection;
use travel_db::repo::offers::{self, OfferRow};
use travel_db::repo::observations::{self, ObservationInput};
use travel_db::repo::{ota_attempts, ota_jobs};

pub const AGENT_NORMALIZER_VERSION: &str = "travel-ota-agent-v1";

pub const VALID_PARAM_KEYS: &[&str] = &[
    "depart_date",
    "return_date",
    "nights",
    "pax",
    "region_code",
    "region_label",
    "destination",
];

pub fn now_iso() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Validate that `value` is an exact `%Y-%m-%dT%H:%M:%SZ` UTC timestamp.
/// reap-stale compares it lexically against stored ISO leases, so a differently-formatted
/// `--now` would mis-order; fail loud instead.
pub fn validate_iso_z(value: &str) -> Result<(), String> {
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%SZ")
        .map(|_| ())
        .map_err(|e| format!("invalid timestamp {value:?}; expected YYYY-MM-DDTHH:MM:SSZ ({e})"))
}

/// Extract positional (non-flag) args, skipping flag VALUES.
///
/// A naive `args.iter().filter(|a| !a.starts_with("--"))` sweeps up the VALUE of a
/// value-taking flag (e.g. `besttour` after `--source`), so positionals bind to the wrong
/// tokens when flags precede them. This walks the arg list and, when it sees a flag in
/// `value_flags`, consumes the following token as that flag's value rather than a positional.
pub fn positionals<'a>(args: &'a [String], value_flags: &[&str]) -> Vec<&'a String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a.starts_with("--") {
            if value_flags.contains(&a.as_str()) {
                i += 2; // skip the flag and its value
            } else {
                i += 1; // boolean / unknown flag — no value to skip
            }
        } else {
            out.push(a);
            i += 1;
        }
    }
    out
}

pub fn lease_expires(now: &str, lease_seconds: i64) -> Result<String, String> {
    let dt = chrono::DateTime::parse_from_rfc3339(now)
        .map_err(|e| format!("invalid ISO timestamp {now}: {e}"))?;
    Ok((dt + chrono::Duration::seconds(lease_seconds))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string())
}

pub async fn table_exists(
    conn: &Connection,
    table: &str,
    col: &str,
    value: &str,
) -> Result<bool, String> {
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
    if code.is_empty() { None } else { Some(code) }
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

/// Infer the airline (ZH name) from a flight number's 2-letter carrier prefix.
/// Parity with the Python `infer_airline_from_flight_number` / Rust `offer_to_row`.
pub fn infer_airline_from_flight_number(flight_number: &str) -> Option<&'static str> {
    if flight_number.len() < 2 {
        return None;
    }
    match &flight_number[..2] {
        "IT" => Some("台灣虎航"),
        "CI" => Some("中華航空"),
        "BR" => Some("長榮航空"),
        "JX" => Some("星宇航空"),
        "MM" => Some("樂桃航空"),
        "TR" => Some("酷航"),
        "GK" => Some("捷星航空"),
        _ => None,
    }
}

/// `offers.airline`: the extracted airline, else inferred from the outbound flight number
/// (so a CI120 offer writes 中華航空, not NULL). Parity with Python `_row_airline`.
fn row_airline(airline: Option<&str>, flight_outbound: Option<&str>) -> Option<String> {
    if let Some(explicit) = ne(airline) {
        return Some(explicit);
    }
    ne(flight_outbound)
        .and_then(|f| infer_airline_from_flight_number(&f).map(|s| s.to_string()))
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

/// Make every offer id unique IN-BATCH (mutates the rows in TSV/parse order).
///
/// `offer_row_id` keys only on (source_id, product_code, departure_date, nights) — fine for the
/// regex path (one package per date), but an agent flight list has many offers sharing the SAME
/// (date, nights), so they ALL collapse to ONE id and `ON CONFLICT(id, scraped_at) DO NOTHING`
/// would write only the first and silently drop the rest. Extend each id with a flight-number
/// token + price (the real discriminators), and a trailing sequence ONLY for genuine remaining
/// ties — so re-running the SAME capture still dedups deterministically (stable input order).
/// Parity with Python `_disambiguate_ids`.
pub fn disambiguate_ids(rows: &mut [OfferRow]) {
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in rows.iter_mut() {
        let base = row.id.clone();
        // Prefer a flight number as the discriminator (ASCII, meaningful). CJK airline names
        // sanitize to underscore-mush, so don't put them in the id — the airline lives in its own
        // column anyway. Fall back to price, then a sequence for genuine remaining ties.
        let fno = row
            .flight_outbound
            .as_deref()
            .map(sanitize_id_part)
            .filter(|f| f != "unknown" && f.chars().any(|c| c.is_alphanumeric()));
        let price = row.price_per_person.unwrap_or(0);
        let cand = match &fno {
            Some(token) => format!("{base}_{token}_{price}"),
            None => format!("{base}_{price}"),
        };
        // Suffix a sequence until the id is genuinely unused IN THIS BATCH. Checking the FINAL id
        // set (not a per-cand counter) prevents a synthesized `cand_2` from colliding with another
        // row whose natural cand already equals `cand_2` (which ON CONFLICT would silently drop).
        let mut final_id = cand.clone();
        let mut k = 2;
        while used.contains(&final_id) {
            final_id = format!("{cand}_{k}");
            k += 1;
        }
        used.insert(final_id.clone());
        row.id = final_id;
    }
}

/// Inputs for the shared offer-write orchestration.
pub struct WriteOffersInput<'a> {
    pub job_id: &'a str,
    pub claim_token: &'a str,
    pub source_id: &'a str,
    pub product_type: &'a str,
    pub attempt_id: &'a str,
    pub capture_id: &'a str,
    pub started_at: &'a str,
    pub candidate_count: i64,
    /// Offer rows to write (their ids are disambiguated in-batch before insert).
    pub rows: Vec<OfferRow>,
}

/// Result of a successful offer write.
pub struct WriteOffersOutput {
    pub inserted: i64,
    pub deduped: i64,
}

/// Shared write-and-finalize for `parse` and `write-offers` (was copy-pasted between them):
/// disambiguate ids → mark_running → bump attempts → insert loop → record attempt → finish.
///
/// On any insert failure mid-loop, records a `failed` attempt, an `parse_warning` observation,
/// and flips the job to `failed` (token-guarded) so it is NOT left wedged in `running` with no
/// audit trail. Then re-raises the original error.
pub async fn write_offers(
    conn: &Connection,
    mut input: WriteOffersInput<'_>,
) -> Result<WriteOffersOutput, String> {
    disambiguate_ids(&mut input.rows);

    let affected =
        ota_jobs::mark_running(conn, input.job_id, input.claim_token, input.started_at).await?;
    if affected != 1 {
        return Err(format!(
            "Error: claim rejected before write — job_id={} token may have been reclaimed",
            input.job_id
        ));
    }
    // Count this attempt up front so a crash/lease-expiry still advances toward max_attempts.
    ota_jobs::bump_attempts(conn, input.job_id, input.claim_token, input.started_at).await?;

    let mut inserted = 0i64;
    let mut deduped = 0i64;
    for row in &input.rows {
        match offers::insert(conn, row).await {
            Ok(result) => {
                inserted += result.inserted as i64;
                deduped += result.deduped as i64;
            }
            Err(e) => {
                record_failed(conn, &input, inserted, deduped, &e).await;
                return Err(e);
            }
        }
    }

    let finished_at = now_iso();
    let attempt_no = ota_jobs::next_attempt_no(conn, input.job_id).await?;
    ota_attempts::record(
        conn,
        &ota_attempts::AttemptInput {
            attempt_id: input.attempt_id.to_string(),
            job_id: input.job_id.to_string(),
            attempt_no,
            claim_token: input.claim_token.to_string(),
            outcome: "succeeded".to_string(),
            capture_id: Some(input.capture_id.to_string()),
            candidate_count: input.candidate_count,
            inserted_count: inserted,
            deduped_count: deduped,
            error_detail: None,
            started_at: input.started_at.to_string(),
            finished_at: finished_at.clone(),
        },
    )
    .await?;

    let affected = ota_jobs::finish(
        conn,
        input.job_id,
        input.claim_token,
        "succeeded",
        None,
        &finished_at,
    )
    .await?;
    if affected == 0 {
        return Err(format!(
            "Error: finish rejected after write — job_id={} token may have been reclaimed",
            input.job_id
        ));
    }

    Ok(WriteOffersOutput { inserted, deduped })
}

/// Best-effort failure bookkeeping: a failed attempt row, a parse_warning observation, and a
/// `failed` terminal status so the job isn't stuck in `running`. Errors here are swallowed (the
/// original write error is what the caller surfaces); a leftover `running` job is later reaped.
async fn record_failed(
    conn: &Connection,
    input: &WriteOffersInput<'_>,
    inserted: i64,
    deduped: i64,
    error: &str,
) {
    let finished_at = now_iso();
    let attempt_no = ota_jobs::next_attempt_no(conn, input.job_id)
        .await
        .unwrap_or(1);
    let _ = ota_attempts::record(
        conn,
        &ota_attempts::AttemptInput {
            attempt_id: input.attempt_id.to_string(),
            job_id: input.job_id.to_string(),
            attempt_no,
            claim_token: input.claim_token.to_string(),
            outcome: "failed".to_string(),
            capture_id: Some(input.capture_id.to_string()),
            candidate_count: input.candidate_count,
            inserted_count: inserted,
            deduped_count: deduped,
            error_detail: Some(error.to_string()),
            started_at: input.started_at.to_string(),
            finished_at: finished_at.clone(),
        },
    )
    .await;
    let _ = observations::record(
        conn,
        &ObservationInput {
            source_id: input.source_id.to_string(),
            product_type: Some(input.product_type.to_string()),
            job_id: Some(input.job_id.to_string()),
            attempt_id: Some(input.attempt_id.to_string()),
            observation_type: "parse_warning".to_string(),
            severity: "error".to_string(),
            detail: Some(error.to_string()),
            observed_at: finished_at.clone(),
            ..Default::default()
        },
    )
    .await;
    let _ = ota_jobs::finish(
        conn,
        input.job_id,
        input.claim_token,
        "failed",
        None,
        &finished_at,
    )
    .await;
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
        id: offer_row_id(source_id, &product_code, departure_date, nights),
        source_file: Some(format!("capture:{capture_id}")),
        source_id: source_id.to_string(),
        offer_type: offer_row_kind(product_type).to_string(),
        price_per_person: Some(price_per_person),
        currency: Some(currency.to_string()),
        region: None,
        destination: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, flight: Option<&str>, price: i64) -> OfferRow {
        OfferRow {
            id: id.to_string(),
            source_id: "zztest".to_string(),
            offer_type: "flight".to_string(),
            price_per_person: Some(price),
            flight_outbound: flight.map(|s| s.to_string()),
            scraped_at: "2026-06-29T12:00:00Z".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn disambiguate_keeps_distinct_offers_unique() {
        // Three flight offers sharing the SAME base id (same source/date/nights) must end with
        // three distinct ids so ON CONFLICT(id, scraped_at) doesn't drop two of them.
        let mut rows = vec![
            row("zztest_x_20260901_4n", Some("CI120"), 12589),
            row("zztest_x_20260901_4n", Some("BR128"), 13990),
            row("zztest_x_20260901_4n", Some("CI120"), 12589), // genuine tie → seq suffix
        ];
        disambiguate_ids(&mut rows);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids[0], "zztest_x_20260901_4n_CI120_12589");
        assert_eq!(ids[1], "zztest_x_20260901_4n_BR128_13990");
        assert_eq!(ids[2], "zztest_x_20260901_4n_CI120_12589_2");
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 3, "all ids must be unique: {ids:?}");
    }

    #[test]
    fn disambiguate_is_deterministic_across_reruns() {
        let build = || {
            vec![
                row("zztest_x_20260901_4n", Some("CI120"), 12589),
                row("zztest_x_20260901_4n", Some("CI120"), 12589),
            ]
        };
        let mut a = build();
        let mut b = build();
        disambiguate_ids(&mut a);
        disambiguate_ids(&mut b);
        let ids_a: Vec<&str> = a.iter().map(|r| r.id.as_str()).collect();
        let ids_b: Vec<&str> = b.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids_a, ids_b, "same input order → same ids → snapshot dedup works");
    }

    #[test]
    fn airline_inferred_from_flight_number_when_blank() {
        assert_eq!(infer_airline_from_flight_number("CI120"), Some("中華航空"));
        assert_eq!(infer_airline_from_flight_number("IT212"), Some("台灣虎航"));
        assert_eq!(infer_airline_from_flight_number("ZZ999"), None);
        assert_eq!(infer_airline_from_flight_number("C"), None);
    }

    #[test]
    fn row_airline_prefers_explicit_then_infers() {
        assert_eq!(row_airline(Some("樂桃航空"), Some("CI120")).as_deref(), Some("樂桃航空"));
        assert_eq!(row_airline(None, Some("CI120")).as_deref(), Some("中華航空"));
        assert_eq!(row_airline(Some("  "), Some("CI120")).as_deref(), Some("中華航空"));
        assert_eq!(row_airline(None, Some("ZZ999")), None);
        assert_eq!(row_airline(None, None), None);
    }

    #[test]
    fn positionals_skips_flag_values() {
        let args: Vec<String> = ["--source", "besttour", "--product-type", "flight", "JOB", "CAP"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let pos = positionals(&args, &["--source", "--product-type", "--claim-token"]);
        assert_eq!(pos.iter().map(|s| s.as_str()).collect::<Vec<_>>(), vec!["JOB", "CAP"]);
    }

    #[test]
    fn validate_iso_z_rejects_loose_formats() {
        assert!(validate_iso_z("2026-06-29T15:10:02Z").is_ok());
        assert!(validate_iso_z("2026-6-29 10:00").is_err());
        assert!(validate_iso_z("2026-06-29T15:10:02+08:00").is_err());
    }
}
