// `travel view-prices --start YYYY-MM-DD --end YYYY-MM-DD
//   [--region name|--destination slug] [--hotel-per-night N] [--nights N]
//   [--package N] [--pax N]`
//
// Port of src/cli/commands/view-prices.ts (compare package vs separate
// flight+hotel). Read-only computation + plain-text table.
//
// Differences from the TS original (intentional, per the migration plan):
//   - `--json` flag removed (root CLI drops user-facing JSON output; plain
//     table is the only mode, same as the query-offers Rust port).
//
// Reads the raw `offers` table directly (the TS queryOffers legacy path, used
// when no planId is supplied). Flight offers drive the rows; hotel offers
// auto-detect a per-night price when --hotel-per-night is omitted. Leave days
// come from the Turso holiday calendar (db::load_holiday_calendar).

use crate::db;
use crate::leave::calculate_leave_days;
use libsql::Value;

const USD_TO_TWD: i64 = 32;

struct FlightOffer {
    source_id: String,
    price_per_person: Option<i64>,
    currency: String,
    departure_date: Option<String>,
    return_date: Option<String>,
    airline: Option<String>,
}

struct Row {
    depart_date: String,
    return_date: String,
    out_airline: String,
    out_price: i64,
    in_price: i64,
    flight_total: i64,
    hotel_total: Option<i64>,
    separate_total: Option<i64>,
    diff: Option<i64>,
    leave_days: i64,
}

struct Opts {
    start: String,
    end: String,
    region: Option<String>,
    destination: Option<String>,
    hotel_per_night: Option<i64>,
    nights: Option<i64>,
    package_price: Option<i64>,
    pax: i64,
}

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return Ok(());
    }

    let mut start: Option<String> = None;
    let mut end: Option<String> = None;
    let mut region: Option<String> = None;
    let mut destination: Option<String> = None;
    let mut hotel_per_night: Option<i64> = None;
    let mut nights: Option<i64> = None;
    let mut package_price: Option<i64> = None;
    let mut pax: i64 = 2;

    let mut i = 0;
    while i < args.len() {
        let key = args[i].as_str();
        let mut val = || -> Option<String> {
            i += 1;
            args.get(i).cloned()
        };
        match key {
            "--start" => start = val(),
            "--end" => end = val(),
            "--region" => region = val(),
            "--destination" => destination = val(),
            "--hotel-per-night" => hotel_per_night = val().and_then(|s| s.parse().ok()),
            "--nights" => nights = val().and_then(|s| s.parse().ok()),
            "--package" => package_price = val().and_then(|s| s.parse().ok()),
            "--pax" => {
                let v = val();
                match v.as_deref().and_then(|s| s.parse::<i64>().ok()) {
                    Some(p) if p > 0 => pax = p,
                    _ => {
                        eprintln!("Error: --pax must be a positive integer");
                        std::process::exit(1);
                    }
                }
            }
            other if other.starts_with("--") => {
                eprintln!("Error: unknown flag for view-prices: {other}");
                std::process::exit(1);
            }
            _ => {}
        }
        i += 1;
    }

    let (start, end) = match (start, end) {
        (Some(s), Some(e)) => (s, e),
        _ => {
            eprintln!("Error: view-prices requires --start YYYY-MM-DD and --end YYYY-MM-DD");
            eprintln!("Example: view-prices --start 2026-02-24 --end 2026-02-28 --region kansai --hotel-per-night 3000 --nights 4");
            std::process::exit(1);
        }
    };

    let opts = Opts {
        start,
        end,
        region,
        destination,
        hotel_per_night,
        nights,
        package_price,
        pax,
    };

    show_price_comparison(&opts).await
}

async fn show_price_comparison(opts: &Opts) -> Result<(), String> {
    let conn = db::connect_read().await?;

    let flight_offers = query_flights(&conn, opts).await?;
    if flight_offers.is_empty() {
        return Err(
            "Missing Turso flight offers. Import scraper output with npm run db:import:turso before running view-prices.".to_string(),
        );
    }

    let duration = opts.nights.map(|n| n + 1).unwrap_or(5);
    let pax = opts.pax;
    let hotel_nights = opts.nights.unwrap_or(duration - 1);

    // Auto-detect or use provided hotel price.
    let mut hotel_per_night = opts.hotel_per_night;
    let mut hotel_source = "manual";
    if hotel_per_night.is_none() {
        if let Some(detected) = auto_detect_hotel_price(&conn, opts).await? {
            hotel_per_night = Some(detected);
            hotel_source = "auto (Turso hotel offers)";
        }
    }

    let mut rows: Vec<Row> = Vec::new();
    for offer in &flight_offers {
        let (Some(depart), Some(out_price)) = (offer.departure_date.clone(), offer.price_per_person)
        else {
            continue;
        };
        let in_price = 0i64;
        let flight_total = if offer.currency == "USD" {
            (out_price as f64 * USD_TO_TWD as f64).round() as i64
        } else {
            out_price * pax
        };
        let hotel_total = hotel_per_night.map(|h| h * hotel_nights);
        let separate_total = hotel_total.map(|h| flight_total + h);
        let diff = match (separate_total, opts.package_price) {
            (Some(sep), Some(pkg)) => Some(pkg - sep),
            _ => None,
        };

        let return_date = offer.return_date.clone().unwrap_or_else(|| depart.clone());
        let leave_days = compute_leave(&depart, &return_date).await.unwrap_or(0);

        rows.push(Row {
            depart_date: depart,
            return_date,
            out_airline: offer
                .airline
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| Some(offer.source_id.clone()))
                .unwrap_or_else(|| "?".to_string()),
            out_price,
            in_price,
            flight_total,
            hotel_total,
            separate_total,
            diff,
            leave_days,
        });
    }

    if rows.is_empty() {
        eprintln!("Error: No valid flight data with nonstop prices found.");
        std::process::exit(1);
    }

    // Sort by separate total (cheapest first); None sorts last.
    rows.sort_by(|a, b| {
        let av = a.separate_total.unwrap_or(i64::MAX);
        let bv = b.separate_total.unwrap_or(i64::MAX);
        av.cmp(&bv)
    });

    let scope = opts
        .destination
        .clone()
        .or_else(|| opts.region.clone())
        .unwrap_or_else(|| "all".to_string());

    println!("\n  PACKAGE vs SEPARATE BOOKING COMPARISON");
    println!("  Scope: {scope} | {duration} days | {pax} pax");
    println!("  Flight data: Turso offers ({} to {})", opts.start, opts.end);
    if let Some(h) = hotel_per_night {
        println!(
            "  Hotel: TWD {}/night x {} nights = TWD {} ({})",
            locale(h),
            hotel_nights,
            locale(h * hotel_nights),
            hotel_source
        );
    }
    if let Some(p) = opts.package_price {
        println!("  Package baseline: TWD {} ({} pax)", locale(p), pax);
    }
    println!();

    let bar = "-".repeat(95);
    println!("{bar}");
    println!("  Depart       Return       Outbound              Inbound               Flight     Hotel      Total      Leave");
    println!("{bar}");

    for (i, r) in rows.iter().enumerate() {
        let marker = if i == 0 { " *" } else { "" };
        let depart_col = pad_end(&format!("{} ()", &r.depart_date[5.min(r.depart_date.len())..]), 10);
        let return_col = pad_end(&format!("{} ()", &r.return_date[5.min(r.return_date.len())..]), 10);
        let out_str = pad_end(&format!("US${} {}", r.out_price, truncate(&r.out_airline, 8)), 20);
        let in_str = pad_end(&format!("US${} ", r.in_price), 20);
        let flight_str = pad_end(&format!("TWD {}", locale(r.flight_total)), 10);
        let hotel_str = match r.hotel_total {
            Some(h) => pad_end(&format!("TWD {}", locale(h)), 10),
            None => pad_end("-", 10),
        };
        let total_str = match r.separate_total {
            Some(t) => pad_end(&format!("TWD {}", locale(t)), 10),
            None => pad_end("-", 10),
        };
        let leave_str = pad_end(&format!("{}d{marker}", r.leave_days), 6);
        println!("  {depart_col} {return_col} {out_str} {in_str} {flight_str} {hotel_str} {total_str} {leave_str}");
    }
    println!("{bar}");

    // Package comparison.
    if let Some(pkg) = opts.package_price {
        println!("\n  Package vs Separate:");
        for r in &rows {
            let (Some(sep), Some(diff)) = (r.separate_total, r.diff) else {
                continue;
            };
            let diff_str = if diff > 0 {
                format!(
                    "Package costs TWD {} more (+{}%)",
                    locale(diff),
                    ((diff as f64 / sep as f64) * 100.0).round() as i64
                )
            } else if diff < 0 {
                format!(
                    "Separate costs TWD {} more (+{}%)",
                    locale(diff.abs()),
                    ((diff.abs() as f64 / pkg as f64) * 100.0).round() as i64
                )
            } else {
                "Same price".to_string()
            };
            println!("    {} (): {}", &r.depart_date[5.min(r.depart_date.len())..], diff_str);
        }
    }

    // Best value summary.
    let best = &rows[0];
    println!("\n  * Cheapest: {} () departure", best.depart_date);
    println!("    Outbound: {}  - US${}", best.out_airline, best.out_price);
    println!("    Inbound:    - US${}", best.in_price);
    if let Some(sep) = best.separate_total {
        println!("    Separate total: TWD {} ({} pax)", locale(sep), pax);
    }
    println!("    Leave needed: {} days", best.leave_days);

    println!("\n  Warning: LCC fares do not include checked baggage (~TWD 1,500-2,000/person round-trip)");
    println!();

    Ok(())
}

async fn query_flights(
    conn: &libsql::Connection,
    opts: &Opts,
) -> Result<Vec<FlightOffer>, String> {
    let sql = build_offers_sql(opts, "flight", 500);
    let mut rows = conn
        .query(&sql, ())
        .await
        .map_err(|e| format!("failed to query flight offers: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("failed to read flight row: {e}"))?
    {
        out.push(FlightOffer {
            source_id: str_col(&row, 0),
            price_per_person: int_col(&row, 1),
            currency: {
                let c = str_col(&row, 2);
                if c.is_empty() { "TWD".to_string() } else { c }
            },
            departure_date: opt_str_col(&row, 3),
            return_date: opt_str_col(&row, 4),
            airline: opt_str_col(&row, 5),
        });
    }
    Ok(out)
}

async fn auto_detect_hotel_price(
    conn: &libsql::Connection,
    opts: &Opts,
) -> Result<Option<i64>, String> {
    let sql = build_offers_sql(opts, "hotel", 50);
    let mut rows = conn
        .query(&sql, ())
        .await
        .map_err(|e| format!("failed to query hotel offers: {e}"))?;
    let mut prices: Vec<i64> = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("failed to read hotel row: {e}"))?
    {
        if let Some(p) = int_col(&row, 1) {
            if p > 0 {
                prices.push(p);
            }
        }
    }
    prices.sort_unstable();
    Ok(prices.first().copied())
}

/// SELECT source_id, price_per_person, currency, departure_date, return_date, airline
/// FROM offers — region/destination/date/type filtered, mirrors queryOffers
/// legacy path WHERE clauses.
fn build_offers_sql(opts: &Opts, kind: &str, limit: i64) -> String {
    let mut conds: Vec<String> = Vec::new();
    if let Some(d) = &opts.destination {
        conds.push(format!("destination = {}", sql_lit(d)));
    }
    if let Some(r) = &opts.region {
        conds.push(format!("region = {}", sql_lit(r)));
    }
    conds.push(format!("departure_date >= {}", sql_lit(&opts.start)));
    conds.push(format!("departure_date <= {}", sql_lit(&opts.end)));
    conds.push(format!("type = {}", sql_lit(kind)));
    let where_clause = format!("WHERE {}", conds.join(" AND "));
    format!(
        "SELECT source_id, price_per_person, currency, departure_date, return_date, airline \
         FROM offers {where_clause} \
         ORDER BY scraped_at DESC, price_per_person ASC LIMIT {limit}"
    )
}

async fn compute_leave(start: &str, end: &str) -> Result<i64, String> {
    let year = crate::leave::year_from_date(start)?;
    let calendar = db::load_holiday_calendar("tw", year).await?;
    let result = calculate_leave_days(start, end, &calendar)?;
    Ok(result.leave_days as i64)
}

fn str_col(row: &libsql::Row, i: i32) -> String {
    match row.get_value(i) {
        Ok(Value::Text(s)) => s,
        Ok(Value::Integer(n)) => n.to_string(),
        Ok(Value::Real(f)) => f.to_string(),
        _ => String::new(),
    }
}

fn opt_str_col(row: &libsql::Row, i: i32) -> Option<String> {
    match row.get_value(i) {
        Ok(Value::Text(s)) if !s.is_empty() => Some(s),
        _ => None,
    }
}

fn int_col(row: &libsql::Row, i: i32) -> Option<i64> {
    match row.get_value(i) {
        Ok(Value::Integer(n)) => Some(n),
        Ok(Value::Real(f)) => Some(f.round() as i64),
        _ => None,
    }
}

fn sql_lit(v: &str) -> String {
    format!("'{}'", v.replace('\'', "''"))
}

fn pad_end(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn locale(n: i64) -> String {
    let neg = n < 0;
    let digits = n.abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::new();
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}

fn print_usage() {
    println!(
        "Compare package vs separate booking (flight + hotel) prices.\n\
         \n\
         Usage:\n  \
           travel view-prices --start YYYY-MM-DD --end YYYY-MM-DD \
         [--region name|--destination slug] [--hotel-per-night N] [--nights N] [--package N] [--pax N]"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_fmt() {
        assert_eq!(locale(0), "0");
        assert_eq!(locale(1000), "1,000");
        assert_eq!(locale(1234567), "1,234,567");
    }

    #[test]
    fn offers_sql_filters() {
        let opts = Opts {
            start: "2026-02-24".into(),
            end: "2026-02-28".into(),
            region: Some("kansai".into()),
            destination: None,
            hotel_per_night: None,
            nights: Some(4),
            package_price: None,
            pax: 2,
        };
        let sql = build_offers_sql(&opts, "flight", 500);
        assert!(sql.contains("region = 'kansai'"));
        assert!(sql.contains("type = 'flight'"));
        assert!(sql.contains("departure_date >= '2026-02-24'"));
    }
}
