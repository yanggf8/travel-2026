use crate::db;
use crate::ota::common::{self, AGENT_NORMALIZER_VERSION};
use crate::ota::regex_parse::ParsedOffer;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use travel_db::checksum::sha256_hex;
use travel_db::ids::new_run_id;
use travel_db::repo::{captures, offers, ota_attempts, ota_jobs};

const VALID_TYPES: &[&str] = &["package", "flight", "hotel"];
const PRICE_CEILING: i64 = 10_000_000;

const KNOWN_HEADERS: &[&str] = &[
    "type",
    "price_per_person",
    "departure_date",
    "return_date",
    "nights",
    "airline",
    "flight_outbound",
    "flight_return",
    "hotel_name",
    "currency",
];

fn normalize_date(value: &str) -> Result<String, String> {
    let t = value.trim();
    if t.is_empty() || t == "-" {
        return Ok(String::new());
    }
    let s = t.replace('/', "-");
    if s.len() == 10 && s.as_bytes().get(4) == Some(&b'-') && s.as_bytes().get(7) == Some(&b'-') {
        Ok(s)
    } else {
        Err(format!("invalid date {value:?}; expected YYYY-MM-DD"))
    }
}

fn parse_tsv(path: &Path) -> Result<Vec<ParsedOffer>, String> {
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("failed to read TSV {}: {e}", path.display()))?;
    let mut lines = raw.lines().filter(|l| !l.trim().is_empty());
    let header_line = lines
        .next()
        .ok_or_else(|| format!("TSV {} is empty", path.display()))?;
    let headers: Vec<&str> = header_line.split('\t').map(str::trim).collect();
    for h in &headers {
        if !KNOWN_HEADERS.contains(h) {
            return Err(format!(
                "Error: unknown TSV header column '{h}'; known: {}",
                KNOWN_HEADERS.join(", ")
            ));
        }
    }
    if !headers.iter().any(|h| *h == "type") {
        return Err("Error: TSV header must include 'type'".to_string());
    }
    if !headers.iter().any(|h| *h == "price_per_person") {
        return Err("Error: TSV header must include 'price_per_person'".to_string());
    }

    let mut offers = Vec::new();
    for (idx, line) in lines.enumerate() {
        let cols: Vec<&str> = line.split('\t').collect();
        let mut row: HashMap<String, String> = HashMap::new();
        for (i, h) in headers.iter().enumerate() {
            if let Some(v) = cols.get(i) {
                row.insert(h.to_string(), v.trim().to_string());
            }
        }
        let kind = row.get("type").map(|s| s.trim()).unwrap_or("");
        if !VALID_TYPES.contains(&kind) {
            return Err(format!(
                "Error: offer[{idx}] type={kind:?} invalid; must be package|flight|hotel"
            ));
        }
        let price_raw = row
            .get("price_per_person")
            .ok_or_else(|| format!("Error: offer[{idx}] missing price_per_person"))?;
        if price_raw.trim().is_empty() || price_raw.trim() == "-" {
            return Err(format!("Error: offer[{idx}] missing price_per_person"));
        }
        let price_s = price_raw.replace(',', "");
        let price: i64 = price_s.parse().map_err(|_| {
            format!("Error: offer[{idx}] price_per_person={price_raw:?} not an int")
        })?;
        if price <= 0 {
            return Err(format!(
                "Error: offer[{idx}] price_per_person must be > 0 (got {price})"
            ));
        }
        if price > PRICE_CEILING {
            return Err(format!(
                "Error: offer[{idx}] price_per_person {price} exceeds ceiling {PRICE_CEILING}"
            ));
        }

        let depart = normalize_date(row.get("departure_date").map(|s| s.as_str()).unwrap_or(""))?;
        let ret = normalize_date(row.get("return_date").map(|s| s.as_str()).unwrap_or(""))?;
        if !depart.is_empty() && !ret.is_empty() && ret < depart {
            return Err(format!(
                "Error: offer[{idx}] return_date {ret} is before departure_date {depart}"
            ));
        }

        let nights = row.get("nights").and_then(|s| {
            let t = s.trim();
            if t.is_empty() || t == "-" {
                None
            } else {
                t.parse().ok()
            }
        });

        let cell = |key: &str| -> Option<String> {
            row.get(key).and_then(|v| {
                let t = v.trim();
                if t.is_empty() || t == "-" {
                    None
                } else {
                    Some(t.to_string())
                }
            })
        };

        offers.push(ParsedOffer {
            product_type: kind.to_string(),
            departure_date: depart,
            return_date: ret,
            nights,
            price_per_person: price,
            currency: cell("currency").unwrap_or_else(|| "TWD".to_string()),
            flight_outbound: cell("flight_outbound"),
            flight_return: cell("flight_return"),
            airline: cell("airline"),
            hotel_name: cell("hotel_name"),
        });
    }
    Ok(offers)
}

pub async fn run(args: &[String]) -> Result<(), String> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if positional.is_empty() {
        return Err(
            "Usage: travel ota write-offers <job_id> --capture <capture_id> \
             --claim-token <token> --tsv <path>"
                .to_string(),
        );
    }
    let job_id = positional[0].as_str();
    let mut capture_id: Option<String> = None;
    let mut claim_token: Option<String> = None;
    let mut tsv_path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--capture" => {
                capture_id = Some(args.get(i + 1).ok_or("missing --capture")?.clone());
                i += 2;
            }
            "--claim-token" => {
                claim_token = Some(args.get(i + 1).ok_or("missing --claim-token")?.clone());
                i += 2;
            }
            "--tsv" => {
                tsv_path = Some(args.get(i + 1).ok_or("missing --tsv")?.clone());
                i += 2;
            }
            _ => i += 1,
        }
    }
    let capture_id = capture_id.ok_or("Error: --capture is required")?;
    let claim_token = claim_token.ok_or("Error: --claim-token is required")?;
    let tsv_path = tsv_path.ok_or("Error: --tsv is required")?;

    let conn = db::connect_write().await?;
    let job = ota_jobs::get(&conn, job_id)
        .await?
        .ok_or_else(|| format!("Error: job_id '{job_id}' not found"))?;
    if job.claim_token.as_deref() != Some(claim_token.as_str()) {
        return Err(format!("Error: claim_token mismatch for job_id={job_id}"));
    }

    let cap = captures::get(&conn, &capture_id)
        .await?
        .ok_or_else(|| format!("Error: capture_id '{capture_id}' not found"))?;
    if cap.source_id != job.source_id {
        return Err(format!(
            "Error: capture source_id='{}' does not match job source_id='{}'",
            cap.source_id, job.source_id
        ));
    }

    let parsed = parse_tsv(Path::new(&tsv_path))?;
    let expected_offer_type = common::offer_row_kind(&job.product_type);
    for (idx, p) in parsed.iter().enumerate() {
        let actual_offer_type = common::offer_row_kind(&p.product_type);
        if actual_offer_type != expected_offer_type {
            return Err(format!(
                "Error: offer[{idx}] type='{}' is incompatible with job product_type='{}'",
                p.product_type, job.product_type
            ));
        }
    }
    let candidate_count = parsed.len() as i64;
    let capture_checksum = captures::capture_checksum(&cap.raw_text);
    let parser_rule_checksum = Some(format!(
        "agent_parse:{}",
        sha256_hex(AGENT_NORMALIZER_VERSION)
    ));

    let url = cap.url.as_deref().unwrap_or("");
    let scraped_at = common::now_iso();
    let attempt_id = new_run_id();
    let started_at = scraped_at.clone();
    let source_id = job.source_id.clone();

    let affected = ota_jobs::mark_running(&conn, job_id, &claim_token, &started_at).await?;
    if affected != 1 {
        return Err(format!(
            "Error: claim rejected before write-offers — job_id={job_id} token may have been reclaimed"
        ));
    }

    let mut inserted_count = 0i64;
    let mut deduped_count = 0i64;
    for p in &parsed {
        let row = common::parsed_to_offer_row(
            &source_id,
            &p.product_type,
            url,
            &capture_id,
            &scraped_at,
            Some(&p.departure_date),
            Some(&p.return_date),
            p.nights,
            p.price_per_person,
            &p.currency,
            p.hotel_name.as_deref(),
            p.airline.as_deref(),
            p.flight_outbound.as_deref(),
            p.flight_return.as_deref(),
            job_id,
            &attempt_id,
            "agent_parse",
            &capture_checksum,
            parser_rule_checksum.as_deref(),
            AGENT_NORMALIZER_VERSION,
        );
        let result = offers::insert(&conn, &row).await?;
        inserted_count += result.inserted as i64;
        deduped_count += result.deduped as i64;
    }

    let finished_at = common::now_iso();
    let attempt_no = ota_jobs::next_attempt_no(&conn, job_id).await?;
    ota_attempts::record(
        &conn,
        &ota_attempts::AttemptInput {
            attempt_id: attempt_id.clone(),
            job_id: job_id.to_string(),
            attempt_no,
            claim_token: claim_token.clone(),
            outcome: "succeeded".to_string(),
            capture_id: Some(capture_id.clone()),
            candidate_count,
            inserted_count,
            deduped_count,
            error_detail: None,
            started_at: started_at.clone(),
            finished_at: finished_at.clone(),
        },
    )
    .await?;

    let affected =
        ota_jobs::finish(&conn, job_id, &claim_token, "succeeded", None, &finished_at).await?;
    if affected == 0 {
        return Err(format!(
            "Error: finish rejected after write-offers — job_id={job_id}"
        ));
    }

    println!("job_id\t{job_id}");
    println!("attempt_id\t{attempt_id}");
    println!("capture_id\t{capture_id}");
    println!("candidates\t{candidate_count}");
    println!("inserted\t{inserted_count}");
    println!("deduped\t{deduped_count}");
    println!("parser_method\tagent_parse");
    println!("status\tsucceeded");
    Ok(())
}
