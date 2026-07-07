// `travel add-lifetour-offer --url <lifetour url> --price <TWD> --hotel "<name>" [--depart YYYY-MM-DD] [--return YYYY-MM-DD] [--seats <N>] [--note "<text>"] [--run <run_id>]`
//
// Port of src/cli/commands/add-lifetour-offer.ts.

use crate::cascade::common::now_rfc3339;
use crate::tour_group_offers::{insert_tour_group_offers, base36_timestamp, Note, TourGroupOfferRow};

pub async fn run(rest: &[String]) -> Result<(), String> {
    // First check for --help/-h
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!("Usage:\n  travel add-lifetour-offer --url <lifetour url> --price <TWD> --hotel \"<name>\" [--depart YYYY-MM-DD] [--return YYYY-MM-DD] [--seats N] [--note \"<text>\"] [--run <run_id>]");
        return Ok(());
    }

    // Parse arguments
    let mut url = None;
    let mut price_str = None;
    let mut hotel = None;
    let mut depart = None;
    let mut ret = None;
    let mut nights_str = None;
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
            "--nights" => {
                i += 1;
                nights_str = rest.get(i).cloned();
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
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => {}
        }
        i += 1;
    }

    // Validate required fields
    let Some(url) = url else {
        eprintln!("Error: --url, --price and --hotel are required");
        std::process::exit(1);
    };
    let Some(price_str) = price_str else {
        eprintln!("Error: --url, --price and --hotel are required");
        std::process::exit(1);
    };
    let Some(hotel) = hotel else {
        eprintln!("Error: --url, --price and --hotel are required");
        std::process::exit(1);
    };

    let price = price_str.parse::<i64>().map_err(|e| format!("invalid price: {e}"))?;
    let seats = seats_str.and_then(|s| s.parse::<i64>().ok());
    let run_id = run_id.unwrap_or_else(|| "shaping-20260525-093508".to_string());

    // Determine region from URL
    let mut region = "okinawa".to_string();
    if url.contains("0001-0005") {
        region = "okinawa".to_string();
    } else if url.contains("0001-0001") {
        region = "tokyo".to_string();
    } else if url.contains("0001-0003") {
        region = "kansai".to_string();
    }

    // Determine nights
    let mut nights = nights_str.as_ref().and_then(|s| s.parse::<i64>().ok()).unwrap_or(3);

    // Determine dates
    let Some(depart) = depart else {
        eprintln!("Error: Please provide --depart and --return (YYYY-MM-DD)");
        eprintln!("Lifetour pages are usually search results, so dates are not embedded in the URL.");
        std::process::exit(1);
    };
    let Some(ret) = ret else {
        eprintln!("Error: Please provide --depart and --return (YYYY-MM-DD)");
        eprintln!("Lifetour pages are usually search results, so dates are not embedded in the URL.");
        std::process::exit(1);
    };

    // Infer nights from dates if not provided
    if nights_str.is_none()
        && let (Ok(depart_date), Ok(ret_date)) = (
            chrono::NaiveDate::parse_from_str(&depart, "%Y-%m-%d"),
            chrono::NaiveDate::parse_from_str(&ret, "%Y-%m-%d"),
        )
    {
        let diff = ret_date.signed_duration_since(depart_date);
        let diff_days = diff.num_days();
        nights = std::cmp::max(1, diff_days - 1);
    }

    let now = now_rfc3339();

    let depart_no_dash = depart.replace('-', "");
    let offer_id = format!("lifetour-okinawa-{}-{}n-{}", depart_no_dash, nights, base36_timestamp());

    let title = format!("【機加酒．沖繩自由行{}日】{}、五福旅遊", nights + 1, hotel);

    let notes = vec![
        Note { key: "source".to_string(), value: "lifetour_manual".to_string() },
        Note { key: "url".to_string(), value: url.clone() },
        Note { key: "observed_at".to_string(), value: now.clone() },
    ];

    let row = TourGroupOfferRow {
        run_id: run_id.clone(),
        offer_id: offer_id.clone(),
        source_id: "lifetour".to_string(),
        dest_region: region,
        depart_date: depart.clone(),
        return_date: ret.clone(),
        nights,
        price_per_person_twd: price,
        title,
        url: url.clone(),
        scraped_at: now,
        hotel_name: Some(hotel.clone()),
        hotel_star_rating: None,
        meals_included_count: Some(0),
        departure_status: Some("available".to_string()),
        seats_available: seats,
        min_group_size: Some(2),
        group_size_cap: None,
        raw_confidence: None,
        raw_note: note,
        raw_flight: None,
        raw_flight_outbound: None,
        raw_flight_return: None,
        notes,
        product_kind: Some("fit".to_string()),
    };

    // Connect to DB and insert
    let conn = crate::db::connect_write().await?;
    insert_tour_group_offers(&conn, &[row]).await?;

    println!("✅ Lifetour offer saved directly to Turso");
    println!("   {depart} → {ret} ({nights}n) | {price} TWD/pax | {hotel}");
    println!("   run: {run_id}");
    println!("   url: {url}");

    Ok(())
}
