// `travel add-besttour-offer --url <besttour itinerary url> --price <TWD> --hotel "<name>" [--depart YYYY-MM-DD] [--return YYYY-MM-DD] [--seats <N>] [--note "<text>"] [--run <run_id>]`
//
// Port of src/cli/commands/add-besttour-offer.ts.

use crate::cascade::common::now_rfc3339;
use crate::tour_group_offers::{insert_tour_group_offers, base36_timestamp, Note, TourGroupOfferRow};

pub async fn run(rest: &[String]) -> Result<(), String> {
    // First check for --help/-h
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel add-besttour-offer --url <besttour itinerary url> --price <TWD> --hotel \"<name>\" [--depart YYYY-MM-DD] [--return YYYY-MM-DD] [--seats N] [--note \"<text>\"] [--run <run_id>]");
        return Ok(());
    }

    // Parse arguments
    let mut url = None;
    let mut price_str = None;
    let mut hotel = None;
    let mut depart = None;
    let mut ret = None;
    let mut seats_str = None;
    let mut note = None;
    let mut run_id = None;

    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--url" => {
                i += 1;
                url = rest.get(i).cloned();
            }
            "--price" => {
                i += 1;
                price_str = rest.get(i).cloned();
            }
            "--hotel" => {
                i += 1;
                hotel = rest.get(i).cloned();
            }
            "--depart" => {
                i += 1;
                depart = rest.get(i).cloned();
            }
            "--return" => {
                i += 1;
                ret = rest.get(i).cloned();
            }
            "--seats" => {
                i += 1;
                seats_str = rest.get(i).cloned();
            }
            "--note" => {
                i += 1;
                note = rest.get(i).cloned();
            }
            "--run" => {
                i += 1;
                run_id = rest.get(i).cloned();
            }
            _ => {}
        }
        i += 1;
    }

    // Validate required fields
    let Some(url) = url else {
        eprintln!("Error: --url, --price and --hotel are required for add-besttour-offer");
        std::process::exit(1);
    };
    let Some(price_str) = price_str else {
        eprintln!("Error: --url, --price and --hotel are required for add-besttour-offer");
        std::process::exit(1);
    };
    let Some(hotel) = hotel else {
        eprintln!("Error: --url, --price and --hotel are required for add-besttour-offer");
        std::process::exit(1);
    };

    let price = price_str.parse::<i64>().map_err(|e| format!("invalid price: {e}"))?;
    let seats = seats_str.and_then(|s| s.parse::<i64>().ok());
    let run_id = run_id.unwrap_or_else(|| "shaping-20260525-093508".to_string());

    // Parse product code from URL
    // URL pattern: /itinerary/OKA04FD260612BU
    let mut nights = 3;
    let mut inferred_depart: Option<String> = None;

    if let Some(captures) = regex::Regex::new(r"/itinerary/(OKA(\d{2})([A-Z]{2}))(\d{6})([A-Z0-9]+)").unwrap().captures(&url) {
        let days_code_str = captures.get(2).unwrap().as_str();
        if let Ok(days_code) = days_code_str.parse::<i64>() {
            nights = std::cmp::max(1, days_code - 1);
        }

        let date_part = captures.get(4).unwrap().as_str();
        if date_part.len() == 6 {
            let year = 2000 + date_part[0..2].parse::<i32>().unwrap_or(26);
            let month = &date_part[2..4];
            let day = &date_part[4..6];
            inferred_depart = Some(format!("{}-{}-{}", year, month, day));
        }
    }

    // Determine depart date
    let Some(depart) = depart.or(inferred_depart) else {
        eprintln!("Error: Could not determine depart date. Please pass --depart YYYY-MM-DD");
        std::process::exit(1);
    };

    // Determine return date
    let return_date = if let Some(r) = ret {
        r
    } else {
        // Calculate return date: depart + nights
        let depart_date = chrono::NaiveDate::parse_from_str(&depart, "%Y-%m-%d")
            .map_err(|e| format!("invalid depart date: {e}"))?;
        let return_date = depart_date + chrono::Duration::days(nights);
        return_date.format("%Y-%m-%d").to_string()
    };

    let now = now_rfc3339();

    let depart_no_dash = depart.replace('-', "");
    let offer_id = format!("besttour-okinawa-{}-{}n-{}", depart_no_dash, nights, base36_timestamp());

    let title = format!("【機加酒．沖繩自由行{}日】{}、亞洲航空、20公斤托運行李", nights + 1, hotel);

    let notes = vec![
        Note { key: "source".to_string(), value: "besttour_manual".to_string() },
        Note { key: "url".to_string(), value: url.clone() },
        Note { key: "baggage_kg".to_string(), value: "20".to_string() },
        Note { key: "observed_at".to_string(), value: now.clone() },
    ];

    let row = TourGroupOfferRow {
        run_id: run_id.clone(),
        offer_id: offer_id.clone(),
        source_id: "besttour".to_string(),
        dest_region: "okinawa".to_string(),
        depart_date: depart.clone(),
        return_date: return_date.clone(),
        nights,
        price_per_person_twd: price,
        title,
        url: url.clone(),
        scraped_at: now,
        hotel_name: Some(hotel.clone()),
        hotel_star_rating: Some(4),
        meals_included_count: Some(0),
        departure_status: Some("available".to_string()),
        seats_available: seats,
        min_group_size: Some(2),
        group_size_cap: None,
        raw_confidence: None,
        raw_note: note,
        raw_flight: Some("FD230 / FD231".to_string()),
        raw_flight_outbound: None,
        raw_flight_return: None,
        notes,
        product_kind: Some("fit".to_string()),
    };

    // Connect to DB and insert
    let conn = crate::db::connect_write().await?;
    insert_tour_group_offers(&conn, &[row]).await?;

    println!("✅ BestTour offer saved directly to Turso");
    println!("   {depart} → {return_date} ({nights}n) | {price} TWD/pax | {hotel}");
    println!("   run: {run_id}");
    println!("   url: {url}");

    Ok(())
}
