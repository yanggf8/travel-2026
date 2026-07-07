// `travel add-offer --run <run_id> --source <id> --region <code> --depart <date> --return <date> --nights <N> --price <TWD> --title "<title>" --url <url> --hotel "<name>" [--kind fit|group_tour] [--seats <N>] [--baggage <N>] [--flight "<desc>"] [--note "<text>"]`
//
// Port of src/cli/commands/add-offer.ts.

use crate::cascade::common::now_rfc3339;
use crate::tour_group_offers::{insert_tour_group_offers, base36_timestamp, Note, TourGroupOfferRow};

pub async fn run(rest: &[String]) -> Result<(), String> {
    // First check for --help/-h
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel add-offer --run <run_id> --source <id> --region <code> --depart YYYY-MM-DD --return YYYY-MM-DD --nights N --price <TWD> --title \"<title>\" --url <url> --hotel \"<name>\" [--kind fit|group_tour] [--seats N] [--baggage N] [--flight \"<desc>\"] [--note \"<text>\"]");
        return Ok(());
    }

    // Parse arguments
    let mut run_id = None;
    let mut source = None;
    let mut region = None;
    let mut depart = None;
    let mut ret = None;
    let mut nights_str = None;
    let mut price_str = None;
    let mut title = None;
    let mut url = None;
    let mut hotel = None;
    let mut kind = None;
    let mut seats_str = None;
    let mut baggage_str = None;
    let mut flight = None;
    let mut note = None;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--run" => {
                i += 1;
                run_id = rest.get(i).cloned();
            }
            "--source" => {
                i += 1;
                source = rest.get(i).cloned();
            }
            "--region" => {
                i += 1;
                region = rest.get(i).cloned();
            }
            "--depart" => {
                i += 1;
                depart = rest.get(i).cloned();
            }
            "--return" => {
                i += 1;
                ret = rest.get(i).cloned();
            }
            "--nights" => {
                i += 1;
                nights_str = rest.get(i).cloned();
            }
            "--price" => {
                i += 1;
                price_str = rest.get(i).cloned();
            }
            "--title" => {
                i += 1;
                title = rest.get(i).cloned();
            }
            "--url" => {
                i += 1;
                url = rest.get(i).cloned();
            }
            "--hotel" => {
                i += 1;
                hotel = rest.get(i).cloned();
            }
            "--kind" => {
                i += 1;
                kind = rest.get(i).cloned();
            }
            "--seats" => {
                i += 1;
                seats_str = rest.get(i).cloned();
            }
            "--baggage" => {
                i += 1;
                baggage_str = rest.get(i).cloned();
            }
            "--flight" => {
                i += 1;
                flight = rest.get(i).cloned();
            }
            "--note" => {
                i += 1;
                note = rest.get(i).cloned();
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => {}
        }
        i += 1;
    }

    // Validate required fields
    let Some(run_id) = run_id else {
        eprintln!("Error: Missing required flags. See usage.");
        eprintln!("Required: --run --source --region --depart --return --nights --price --title --url");
        std::process::exit(1);
    };
    let Some(source) = source else {
        eprintln!("Error: Missing required flags. See usage.");
        eprintln!("Required: --run --source --region --depart --return --nights --price --title --url");
        std::process::exit(1);
    };
    let Some(region) = region else {
        eprintln!("Error: Missing required flags. See usage.");
        eprintln!("Required: --run --source --region --depart --return --nights --price --title --url");
        std::process::exit(1);
    };
    let Some(depart) = depart else {
        eprintln!("Error: Missing required flags. See usage.");
        eprintln!("Required: --run --source --region --depart --return --nights --price --title --url");
        std::process::exit(1);
    };
    let Some(ret) = ret else {
        eprintln!("Error: Missing required flags. See usage.");
        eprintln!("Required: --run --source --region --depart --return --nights --price --title --url");
        std::process::exit(1);
    };
    let Some(nights_str) = nights_str else {
        eprintln!("Error: Missing required flags. See usage.");
        eprintln!("Required: --run --source --region --depart --return --nights --price --title --url");
        std::process::exit(1);
    };
    let Some(price_str) = price_str else {
        eprintln!("Error: Missing required flags. See usage.");
        eprintln!("Required: --run --source --region --depart --return --nights --price --title --url");
        std::process::exit(1);
    };
    let Some(title) = title else {
        eprintln!("Error: Missing required flags. See usage.");
        eprintln!("Required: --run --source --region --depart --return --nights --price --title --url");
        std::process::exit(1);
    };
    let Some(url) = url else {
        eprintln!("Error: Missing required flags. See usage.");
        eprintln!("Required: --run --source --region --depart --return --nights --price --title --url");
        std::process::exit(1);
    };

    let nights = nights_str.parse::<i64>().map_err(|e| format!("invalid nights: {e}"))?;
    let price = price_str.parse::<i64>().map_err(|e| format!("invalid price: {e}"))?;
    let seats = seats_str.and_then(|s| s.parse::<i64>().ok());
    let product_kind = kind.unwrap_or_else(|| "fit".to_string());

    let now = now_rfc3339();

    // offer_id = `${source}-${region}-${depart_no_dashes}-${nights}n-${base36(Date.now())}`
    // (TS uses ONLY depart — NOT return).
    let depart_no_dash = depart.replace('-', "");
    let offer_id = format!("{}-{}-{}-{}n-{}", source, region, depart_no_dash, nights, base36_timestamp());

    let raw_note = note.as_ref().or(flight.as_ref()).cloned();

    let baggage = baggage_str.and_then(|s| s.parse::<i64>().ok());

    // Build notes
    let mut notes = vec![
        Note { key: "source".to_string(), value: "manual_cli".to_string() },
        Note { key: "observed_at".to_string(), value: now.clone() },
    ];
    if let Some(b) = baggage {
        notes.push(Note { key: "baggage_kg".to_string(), value: b.to_string() });
    }

    let row = TourGroupOfferRow {
        run_id: run_id.clone(),
        offer_id: offer_id.clone(),
        source_id: source,
        dest_region: region,
        depart_date: depart.clone(),
        return_date: ret.clone(),
        nights,
        price_per_person_twd: price,
        title,
        url,
        scraped_at: now,
        hotel_name: hotel.clone(),
        hotel_star_rating: None,
        meals_included_count: Some(0),
        departure_status: Some("available".to_string()),
        seats_available: seats,
        min_group_size: Some(2),
        group_size_cap: None,
        raw_confidence: None,
        raw_note,
        raw_flight: flight,
        raw_flight_outbound: None,
        raw_flight_return: None,
        notes,
        product_kind: Some(product_kind),
    };

    // Connect to DB and insert
    let conn = crate::db::connect_write().await?;
    insert_tour_group_offers(&conn, &[row]).await?;

    println!("✅ Offer added to DB");
    println!("   offer_id: {offer_id}");
    println!("   {depart} → {ret} | {nights}n | {price} TWD/pax | {}", if let Some(h) = &hotel { h.as_str() } else { "no hotel" });
    println!("   run: {run_id}");

    Ok(())
}
