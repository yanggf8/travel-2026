use crate::db;
use crate::ota::common::{self, AGENT_NORMALIZER_VERSION};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use travel_db::checksum::sha256_hex;
use travel_db::ids::new_run_id;
use travel_db::repo::{captures, ota_jobs};

const VALID_TYPES: &[&str] = &["package", "flight", "hotel"];
const PRICE_CEILING: i64 = 10_000_000;

/// One agent-extracted offer (the parsed shape of a TSV row). Built by `parse_tsv` and mapped to
/// an `OfferRow` for insert. (Was shared with the retired regex parser; now owned here, the sole
/// consumer — the agent IS the parser, so there is no in-CLI text parser to share it with.)
#[derive(Debug, Clone)]
pub struct ParsedOffer {
    pub product_type: String,
    pub departure_date: String,
    pub return_date: String,
    pub nights: Option<i64>,
    pub price_per_person: i64,
    pub currency: String,
    pub flight_outbound: Option<String>,
    pub flight_return: Option<String>,
    pub airline: Option<String>,
    pub hotel_name: Option<String>,
}

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
    if s.len() == 10
        && s.as_bytes().get(4) == Some(&b'-')
        && s.as_bytes().get(7) == Some(&b'-')
        && s[..4].chars().all(|c| c.is_ascii_digit())
        && s[5..7].chars().all(|c| c.is_ascii_digit())
        && s[8..10].chars().all(|c| c.is_ascii_digit())
    {
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
    // Reject a repeated header column: the row HashMap keeps only the rightmost value, so a dup
    // would silently use the wrong field. (Python parse_tsv: len(set(header)) != len(header).)
    for (i, h) in headers.iter().enumerate() {
        if headers[i + 1..].contains(h) {
            return Err(format!("Error: duplicate TSV header column '{h}'"));
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
        // A row whose field count != header count would silently shift every later column
        // (wrong price/date written). Hard-error like Python parse_tsv ("TSV columns must align").
        if cols.len() != headers.len() {
            return Err(format!(
                "Error: offer[{idx}] has {} fields but header has {} (row={line:?}); \
                 TSV columns must align",
                cols.len(),
                headers.len()
            ));
        }
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

        let nights = match row.get("nights").map(|s| s.trim()) {
            Some(t) if !t.is_empty() && t != "-" => {
                let n: i64 = t
                    .parse()
                    .map_err(|_| format!("Error: offer[{idx}] nights={t:?} not an int"))?;
                if n < 0 {
                    return Err(format!(
                        "Error: offer[{idx}] nights must be >= 0 (got {n})"
                    ));
                }
                Some(n)
            }
            _ => None,
        };

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
    let positional = common::positionals(args, &["--capture", "--claim-token", "--tsv", "--dest"]);
    if positional.is_empty() {
        return Err(
            "Usage: travel ota write-offers <job_id> --capture <capture_id> \
             --claim-token <token> --tsv <path> --dest <slug>"
                .to_string(),
        );
    }
    let job_id = positional[0].as_str();
    let mut capture_id: Option<String> = None;
    let mut claim_token: Option<String> = None;
    let mut tsv_path: Option<String> = None;
    let mut dest: Option<String> = None;
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
            "--dest" => {
                dest = Some(args.get(i + 1).ok_or("missing --dest")?.clone());
                i += 2;
            }
            _ => i += 1,
        }
    }
    let capture_id = capture_id.ok_or("Error: --capture is required")?;
    let claim_token = claim_token.ok_or("Error: --claim-token is required")?;
    let tsv_path = tsv_path.ok_or("Error: --tsv is required")?;
    let dest = dest.ok_or("Error: --dest <slug> is required")?;

    let conn = db::connect_write().await?;
    let job = ota_jobs::get(&conn, job_id)
        .await?
        .ok_or_else(|| format!("Error: job_id '{job_id}' not found"))?;
    if job.claim_token.as_deref() != Some(claim_token.as_str()) {
        return Err(format!("Error: claim_token mismatch for job_id={job_id}"));
    }

    // Validate --dest against destination_config (fail loud on a bad slug).
    {
        let mut r = conn
            .query(
                "SELECT 1 FROM destination_config WHERE slug = ?1",
                libsql::params![dest.clone()],
            )
            .await
            .map_err(|e| e.to_string())?;
        if r.next().await.map_err(|e| e.to_string())?.is_none() {
            return Err(format!("Error: --dest '{dest}' is not a registered destination"));
        }
    }
    // Region for the offer row: region_label if present, else region_code, else NULL.
    let params = ota_jobs::get_params(&conn, job_id).await?;
    let region = params
        .iter()
        .find(|(k, _)| k == "region_label")
        .or_else(|| params.iter().find(|(k, _)| k == "region_code"))
        .map(|(_, v)| v.clone());

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

    let rows: Vec<_> = parsed
        .iter()
        .map(|p| {
            common::parsed_to_offer_row(
                &source_id,
                &p.product_type,
                &dest,
                region.as_deref(),
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
            )
        })
        .collect();

    let out = common::write_offers(
        &conn,
        common::WriteOffersInput {
            job_id,
            claim_token: &claim_token,
            source_id: &source_id,
            product_type: &job.product_type,
            attempt_id: &attempt_id,
            capture_id: &capture_id,
            started_at: &started_at,
            candidate_count,
            rows,
        },
    )
    .await?;

    println!("job_id\t{job_id}");
    println!("attempt_id\t{attempt_id}");
    println!("capture_id\t{capture_id}");
    println!("candidates\t{candidate_count}");
    println!("inserted\t{}", out.inserted);
    println!("deduped\t{}", out.deduped);
    println!("parser_method\tagent_parse");
    println!("status\tsucceeded");
    Ok(())
}
