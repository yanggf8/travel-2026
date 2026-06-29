use crate::db;
use crate::ota::common::{self, NORMALIZER_VERSION};
use crate::ota::regex_parse;
use crate::ota::settour_parse;
use travel_db::ids::new_run_id;
use travel_db::repo::{captures, ota_jobs, parser_rules};

pub async fn run(args: &[String]) -> Result<(), String> {
    let positional = common::positionals(args, &["--source", "--product-type", "--claim-token"]);
    if positional.len() < 2 {
        return Err(
            "Usage: travel ota parse <job_id> <capture_id> --source <id> \
             --product-type <type> --claim-token <token> [--dry-run]"
                .to_string(),
        );
    }
    let job_id = positional[0].as_str();
    let capture_id = positional[1].as_str();
    let mut source_id: Option<String> = None;
    let mut product_type: Option<String> = None;
    let mut claim_token: Option<String> = None;
    let mut dry_run = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--source" => {
                source_id = Some(args.get(i + 1).ok_or("missing --source")?.clone());
                i += 2;
            }
            "--product-type" => {
                product_type = Some(args.get(i + 1).ok_or("missing --product-type")?.clone());
                i += 2;
            }
            "--claim-token" => {
                claim_token = Some(args.get(i + 1).ok_or("missing --claim-token")?.clone());
                i += 2;
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            _ => i += 1,
        }
    }
    let source_id = source_id.ok_or("Error: --source is required")?;
    let product_type = product_type.ok_or("Error: --product-type is required")?;
    let claim_token = claim_token.ok_or("Error: --claim-token is required")?;

    let conn = db::connect_write().await?;
    let job = ota_jobs::get(&conn, job_id)
        .await?
        .ok_or_else(|| format!("Error: job_id '{job_id}' not found"))?;
    if job.claim_token.as_deref() != Some(claim_token.as_str()) {
        return Err(format!("Error: claim_token mismatch for job_id={job_id}"));
    }
    if job.source_id != source_id {
        return Err(format!(
            "Error: job source_id='{}' does not match --source='{source_id}'",
            job.source_id
        ));
    }
    if job.product_type != product_type {
        return Err(format!(
            "Error: job product_type='{}' does not match --product-type='{product_type}'",
            job.product_type
        ));
    }

    let cap = captures::get(&conn, capture_id)
        .await?
        .ok_or_else(|| format!("Error: capture_id '{capture_id}' not found"))?;
    if cap.source_id != source_id {
        return Err(format!(
            "Error: capture source_id='{}' does not match --source='{source_id}'",
            cap.source_id
        ));
    }

    let rule = parser_rules::get(&conn, &source_id, &product_type)
        .await?
        .ok_or_else(|| {
            format!(
                "Error: no parser_rules row for source_id='{source_id}' product_type='{product_type}'"
            )
        })?;

    let capture_checksum = captures::capture_checksum(&cap.raw_text);
    let parser_rule_checksum = parser_rules::checksum(&rule);

    let parsed = if rule.has_custom_parser {
        match source_id.as_str() {
            "settour" => settour_parse::parse_settour(&cap.raw_text, &rule)?,
            other => {
                return Err(format!(
                    "Error: custom parser for source_id='{other}' not yet ported to Rust; use write-offers"
                ));
            }
        }
    } else {
        regex_parse::parse_generic(&cap.raw_text, &rule)?
    };

    let candidate_count = parsed.len() as i64;
    let url = cap.url.as_deref().unwrap_or("");
    let scraped_at = common::now_iso();
    let attempt_id = new_run_id();
    let started_at = scraped_at.clone();

    if dry_run {
        for p in &parsed {
            println!(
                "offer\t{}\t{}\t{}~{}\t{}n\tpp={}\t{:?}\t{:?}",
                common::offer_row_id(
                    &source_id,
                    &common::product_code_from_url(url).unwrap_or_default(),
                    Some(&p.departure_date),
                    p.nights,
                ),
                common::offer_row_kind(&p.product_type),
                p.departure_date,
                p.return_date,
                p.nights.unwrap_or(0),
                p.price_per_person,
                p.airline,
                p.hotel_name,
            );
        }
        println!("offers\t{candidate_count}");
        println!("dry_run\ttrue");
        return Ok(());
    }

    let rows: Vec<_> = parsed
        .iter()
        .map(|p| {
            common::parsed_to_offer_row(
                &source_id,
                &p.product_type,
                url,
                capture_id,
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
                "regex",
                &capture_checksum,
                Some(&parser_rule_checksum),
                NORMALIZER_VERSION,
            )
        })
        .collect();

    let out = common::write_offers(
        &conn,
        common::WriteOffersInput {
            job_id,
            claim_token: &claim_token,
            source_id: &source_id,
            product_type: &product_type,
            attempt_id: &attempt_id,
            capture_id,
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
    println!("capture_checksum\t{capture_checksum}");
    println!("parser_rule_checksum\t{parser_rule_checksum}");
    println!("status\tsucceeded");
    Ok(())
}
