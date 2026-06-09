// `travel import-tour-group-offers --run <run_id> --file <path>`
//
// Port of src/cli/commands/tour-group.ts (import part).

use crate::cascade::common::now_rfc3339;
use crate::tour_group_offers::{find_scrape_attempt, insert_tour_group_offers, upsert_scrape_attempt, Note, ScrapeAttemptRow, TourGroupOfferRow};
use serde_json::Value;
use std::fs;

// Mirrors the envelope file shape. `scraped_at` is parsed for completeness but
// unread (the TS import doesn't use the envelope-level scraped_at either —
// each offer carries its own).
#[derive(Debug)]
#[allow(dead_code)]
struct ImportFileEnvelope {
    run_id: String,
    scraped_at: String,
    source_id: String,
    dest_region: String,
    nights: i64,
    tour_group_offers: Vec<Value>,
}

pub async fn run(rest: &[String]) -> Result<(), String> {
    // First check for --help/-h
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel import-tour-group-offers --run <run_id> --file <path>");
        return Ok(());
    }

    // Parse arguments
    let mut run_id = None;
    let mut file = None;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--run" => {
                i += 1;
                run_id = rest.get(i).cloned();
            }
            "--file" => {
                i += 1;
                file = rest.get(i).cloned();
            }
            _ => {}
        }
        i += 1;
    }

    let Some(run_id) = run_id else {
        eprintln!("Error: import-tour-group-offers requires --run <run_id> and --file <path>");
        std::process::exit(1);
    };
    let Some(file) = file else {
        eprintln!("Error: import-tour-group-offers requires --run <run_id> and --file <path>");
        std::process::exit(1);
    };

    // Resolve to an absolute path WITHOUT requiring existence (mirrors TS
    // path.resolve), so a missing file produces the TS "file not found" message
    // rather than a canonicalize() error.
    let abs_path = std::path::absolute(&file)
        .unwrap_or_else(|_| std::path::Path::new(&file).to_path_buf());

    if !abs_path.exists() {
        eprintln!("Error: file not found: {}", abs_path.display());
        std::process::exit(1);
    }

    // Parse envelope
    let content = fs::read_to_string(&abs_path).map_err(|e| format!("read file failed: {e}"))?;
    let envelope_value: Value = serde_json::from_str(&content).map_err(|e| format!("parse json failed: {e}"))?;

    let envelope = ImportFileEnvelope {
        run_id: envelope_value["run_id"].as_str().ok_or_else(|| "missing run_id".to_string())?.to_string(),
        scraped_at: envelope_value["scraped_at"].as_str().ok_or_else(|| "missing scraped_at".to_string())?.to_string(),
        source_id: envelope_value["source_id"].as_str().ok_or_else(|| "missing source_id".to_string())?.to_string(),
        dest_region: envelope_value["dest_region"].as_str().ok_or_else(|| "missing dest_region".to_string())?.to_string(),
        nights: envelope_value["nights"].as_i64().ok_or_else(|| "missing nights".to_string())?,
        tour_group_offers: if let Some(arr) = envelope_value["tour_group_offers"].as_array() {
            arr.clone()
        } else {
            vec![]
        },
    };

    if envelope.run_id != run_id {
        eprintln!("Error: file run_id \"{}\" does not match --run \"{}\"", envelope.run_id, run_id);
        std::process::exit(1);
    }

    let conn = crate::db::connect_write().await?;

    let existing = find_scrape_attempt(&conn, &envelope.run_id, &envelope.source_id, &envelope.dest_region, envelope.nights).await?;
    if existing.is_none() {
        eprintln!(
            "Error: no pending attempt found for (run={}, source={}, region={}, nights={}). Seed it before importing.",
            envelope.run_id, envelope.source_id, envelope.dest_region, envelope.nights
        );
        std::process::exit(1);
    }

    let mut parsed = Vec::new();
    let mut skipped = Vec::new();

    for raw in &envelope.tour_group_offers {
        match parse_offer_row(&envelope.run_id, raw) {
            Ok(row) => parsed.push(row),
            Err(missing) => {
                let offer_id = raw["offer_id"].as_str().unwrap_or("<no-id>").to_string();
                skipped.push((offer_id, missing));
            }
        }
    }

    let parsed_len = parsed.len();
    let skipped_len = skipped.len();
    if !parsed.is_empty() {
        // Legacy raw_json is already decomposed into typed cols + notes inside
        // parse_offer_row, so the rows are insert-ready as-is.
        insert_tour_group_offers(&conn, &parsed).await?;
    }

    let status = if parsed_len == 0 {
        "failed"
    } else if !skipped.is_empty() {
        "partial"
    } else {
        "ok"
    };

    // Build error message (plain text instead of JSON as allowed divergence)
    let error = if !skipped.is_empty() {
        let mut msg = format!("skipped {}: ", skipped.len());
        for (i, (offer_id, missing)) in skipped.iter().enumerate() {
            if i > 0 {
                msg.push_str(", ");
            }
            if i >= 5 {
                msg.push_str("...");
                break;
            }
            msg.push_str(&format!("{} (missing: {})", offer_id, missing.join(", ")));
        }
        Some(msg)
    } else {
        None
    };

    let attempted_at = now_rfc3339();

    let scrape_attempt = ScrapeAttemptRow {
        run_id: envelope.run_id.clone(),
        source_id: envelope.source_id.clone(),
        dest_region: envelope.dest_region.clone(),
        nights: envelope.nights,
        status: status.to_string(),
        offer_count: Some(envelope.tour_group_offers.len() as i64),
        parsed_count: Some(parsed_len as i64),
        skipped_count: Some(skipped_len as i64),
        error,
        attempted_at: Some(attempted_at),
    };

    upsert_scrape_attempt(&conn, &scrape_attempt).await?;

    println!("✅ Imported {} offers (skipped {}). Attempt status: {}", parsed_len, skipped_len, status);

    Ok(())
}

fn parse_offer_row(run_id: &str, raw: &Value) -> Result<TourGroupOfferRow, Vec<String>> {
    // Validate ALL required fields up front and collect every missing one
    // (mirrors tour-group-service.ts validateOfferRow over REQUIRED_OFFER_FIELDS:
    // a value is "missing" when undefined/null/empty-string). This must list
    // every missing field, not short-circuit on the first.
    let mut missing: Vec<String> = Vec::new();
    let str_present = |k: &str| -> bool {
        raw.get(k).and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false)
    };
    let num_present = |k: &str| -> bool {
        raw.get(k).map(|v| v.is_number()).unwrap_or(false)
    };
    for k in ["run_id", "offer_id", "source_id", "dest_region", "depart_date", "return_date", "title", "url", "scraped_at"] {
        if !str_present(k) { missing.push(k.to_string()); }
    }
    for k in ["nights", "price_per_person_twd"] {
        if !num_present(k) { missing.push(k.to_string()); }
    }
    // REQUIRED_OFFER_FIELDS order in TS: run_id, offer_id, source_id, dest_region,
    // depart_date, return_date, nights, price_per_person_twd, title, url, scraped_at.
    // Re-sort `missing` into that canonical order for parity with the TS diagnostic.
    let order = ["run_id", "offer_id", "source_id", "dest_region", "depart_date", "return_date", "nights", "price_per_person_twd", "title", "url", "scraped_at"];
    missing.sort_by_key(|m| order.iter().position(|o| o == m).unwrap_or(usize::MAX));
    if !missing.is_empty() {
        return Err(missing);
    }

    // All required fields are present; extract them (the matches below cannot
    // hit their None arms now, but keep them for type extraction).
    let offer_id = match raw["offer_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            missing.push("offer_id".to_string());
            return Err(missing);
        }
    };
    let source_id = match raw["source_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            missing.push("source_id".to_string());
            return Err(missing);
        }
    };
    let dest_region = match raw["dest_region"].as_str() {
        Some(s) => s.to_string(),
        None => {
            missing.push("dest_region".to_string());
            return Err(missing);
        }
    };
    let depart_date = match raw["depart_date"].as_str() {
        Some(s) => s.to_string(),
        None => {
            missing.push("depart_date".to_string());
            return Err(missing);
        }
    };
    let return_date = match raw["return_date"].as_str() {
        Some(s) => s.to_string(),
        None => {
            missing.push("return_date".to_string());
            return Err(missing);
        }
    };
    let nights = match raw["nights"].as_i64() {
        Some(i) => i,
        None => {
            missing.push("nights".to_string());
            return Err(missing);
        }
    };
    let price_per_person_twd = match raw["price_per_person_twd"].as_i64() {
        Some(i) => i,
        None => {
            missing.push("price_per_person_twd".to_string());
            return Err(missing);
        }
    };
    let title = match raw["title"].as_str() {
        Some(s) => s.to_string(),
        None => {
            missing.push("title".to_string());
            return Err(missing);
        }
    };
    let url = match raw["url"].as_str() {
        Some(s) => s.to_string(),
        None => {
            missing.push("url".to_string());
            return Err(missing);
        }
    };
    let scraped_at = match raw["scraped_at"].as_str() {
        Some(s) => s.to_string(),
        None => {
            missing.push("scraped_at".to_string());
            return Err(missing);
        }
    };

    // Optional fields
    let hotel_name = raw["hotel_name"].as_str().map(|s| s.to_string());
    let hotel_star_rating = raw["hotel_star_rating"].as_i64();
    let meals_included_count = raw["meals_included_count"].as_i64();
    let departure_status = raw["departure_status"].as_str().map(|s| s.to_string());
    let seats_available = raw["seats_available"].as_i64();
    let min_group_size = raw["min_group_size"].as_i64();
    let group_size_cap = raw["group_size_cap"].as_i64();
    let mut raw_confidence = raw["raw_confidence"].as_str().map(|s| s.to_string());
    let mut raw_note = raw["raw_note"].as_str().map(|s| s.to_string());
    let mut raw_flight = raw["raw_flight"].as_str().map(|s| s.to_string());
    let mut raw_flight_outbound = raw["raw_flight_outbound"].as_str().map(|s| s.to_string());
    let mut raw_flight_return = raw["raw_flight_return"].as_str().map(|s| s.to_string());
    let product_kind = raw["product_kind"].as_str().map(|s| s.to_string());

    // Parse notes
    let mut notes = Vec::new();
    if let Some(notes_arr) = raw["notes"].as_array() {
        for note_val in notes_arr {
            if let (Some(key), Some(value)) = (note_val["key"].as_str(), note_val["value"].as_str()) {
                notes.push(Note { key: key.to_string(), value: value.to_string() });
            }
        }
    }

    // Legacy `raw_json` decomposition (mirrors tour-group-service.ts
    // normalizeLegacyRawJson): scraper envelopes may carry a JSON-string
    // `raw_json` field. Known keys → typed columns (only if not already set);
    // every other key → a flat notes row (nested values flattened, NOT JSON).
    // This is the landing-zone boundary, so parsing the file's JSON is allowed.
    if let Some(raw_json_str) = raw["raw_json"].as_str()
        && let Ok(serde_json::Value::Object(map)) =
            serde_json::from_str::<serde_json::Value>(raw_json_str)
    {
        for (k, v) in &map {
            let as_str = || -> Option<String> {
                match v {
                    serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
                    serde_json::Value::Null => None,
                    other => Some(other.to_string()),
                }
            };
            match k.as_str() {
                "confidence" => { if raw_confidence.is_none() { raw_confidence = as_str(); } }
                "note" => { if raw_note.is_none() { raw_note = as_str(); } }
                "flight" => { if raw_flight.is_none() { raw_flight = as_str(); } }
                "flight_outbound" => { if raw_flight_outbound.is_none() { raw_flight_outbound = as_str(); } }
                "flight_return" => { if raw_flight_return.is_none() { raw_flight_return = as_str(); } }
                _ => {
                    let val = crate::tour_group_offers::flatten_note_value(v);
                    if !val.is_empty() && val != "null" && val != "undefined" {
                        notes.push(Note { key: k.clone(), value: val });
                    }
                }
            }
        }
    }

    Ok(TourGroupOfferRow {
        run_id: run_id.to_string(),
        offer_id,
        source_id,
        dest_region,
        depart_date,
        return_date,
        nights,
        price_per_person_twd,
        title,
        url,
        scraped_at,
        hotel_name,
        hotel_star_rating,
        meals_included_count,
        departure_status,
        seats_available,
        min_group_size,
        group_size_cap,
        raw_confidence,
        raw_note,
        raw_flight,
        raw_flight_outbound,
        raw_flight_return,
        notes,
        product_kind,
    })
}
