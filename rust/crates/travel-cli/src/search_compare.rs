// Port of src/cli/commands/search-compare.ts:
//   - compare-offers  → run_compare  (read `offers` table for a region,
//     type=package, print a comparison table + best-value summary)
//   - search-offers   → run_search   (drive registered OTA scrapers)
//
// Signatures fixed by main.rs dispatch. Plain-text output.
//
// compare-offers reads the raw `offers` table (the TS loadScrapedOffers path
// goes through queryOffers with no planId → legacy `offers` table query). The
// currency comes straight from the offer row: the TS getOtaSourceCurrency
// override is best-effort and falls back to the row currency on any error,
// and every package OTA source here is TWD, so the row currency is the
// parity-correct value.
//
// search-offers: the OTA scrapers the TS globalRegistry would invoke are all
// DECOMMISSIONED (Python URL scrapers archived; the live path is the Rust
// chromeport CDP driver, a separate binary). There is no in-process scraper
// registry in travel-cli, so run_search validates inputs and reports that no
// scraper is registered for the source — the same shape the TS registry
// returns for an unknown/unregistered source (success=false, exit 1).

use crate::db;
use libsql::Value;

// ── compare-offers ───────────────────────────────────────────────────

const STALE_THRESHOLD_MS: i64 = 24 * 60 * 60 * 1000;

struct CompareOffer {
    source_id: String,
    scraped_at: String,
    price_per_person: Option<i64>,
    price_total: Option<i64>,
    currency: String,
    airline: String,
    hotel: String,
}

// The TS loadScrapedOffers (queryOffers legacy path) sets source_name =
// row.source_id (raw lowercase id). The display-name map only exists in the
// dead parseScrapedFile path the command never reaches. Mirror the live path:
// use the raw source_id verbatim.
fn source_display_name(id: &str) -> String {
    id.to_string()
}

pub async fn run_compare(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Compare scraped offers from the Turso offers table for a region.\n\
             \n\
             Usage:\n  travel compare-offers --region <name> [--date YYYY-MM-DD] [--pax N]"
        );
        return Ok(());
    }

    let mut region: Option<String> = None;
    let mut filter_date: Option<String> = None;
    let mut pax: i64 = 2;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--region" => {
                i += 1;
                region = args.get(i).cloned();
            }
            "--date" => {
                i += 1;
                filter_date = args.get(i).cloned();
            }
            "--pax" => {
                i += 1;
                match args.get(i).and_then(|s| s.parse::<i64>().ok()) {
                    Some(p) if p > 0 => pax = p,
                    _ => {
                        eprintln!("Error: --pax must be a positive integer");
                        std::process::exit(1);
                    }
                }
            }
            other if other.starts_with("--") => {
                eprintln!("Error: unknown flag for compare-offers: {other}");
                std::process::exit(1);
            }
            _ => {}
        }
        i += 1;
    }

    let Some(region) = region else {
        eprintln!("Error: compare-offers requires --region <name>");
        eprintln!("Example: compare-offers --region osaka");
        std::process::exit(1);
    };

    if let Some(d) = &filter_date {
        if !is_iso_date(d) {
            eprintln!("Error: --date: Invalid date format: {d} (expected YYYY-MM-DD)");
            std::process::exit(1);
        }
    }

    let offers = load_scraped_offers(&region, filter_date.as_deref(), pax).await?;
    if offers.is_empty() {
        println!("\nNo Turso offers found for region \"{region}\".");
        println!("Import scraper output with npm run db:import:turso before running compare-offers.");
        std::process::exit(1);
    }

    print_offer_comparison(&offers, &region, filter_date.as_deref(), pax);
    Ok(())
}

async fn load_scraped_offers(
    region: &str,
    filter_date: Option<&str>,
    pax: i64,
) -> Result<Vec<CompareOffer>, String> {
    let conn = db::connect_read().await?;

    // queryOffers legacy path: offers table, region + (optional) date + type=package.
    let mut conds: Vec<String> = vec![format!("region = {}", sql_lit(region))];
    if let Some(d) = filter_date {
        conds.push(format!("departure_date >= {}", sql_lit(d)));
        conds.push(format!("departure_date <= {}", sql_lit(d)));
    }
    conds.push("type = 'package'".to_string());
    let sql = format!(
        "SELECT id, source_id, price_per_person, currency, airline, hotel_name, name, \
         departure_date, return_date, scraped_at \
         FROM offers WHERE {} \
         ORDER BY scraped_at DESC, price_per_person ASC LIMIT 500",
        conds.join(" AND ")
    );

    let mut rows = conn
        .query(&sql, ())
        .await
        .map_err(|e| format!("failed to query offers: {e}"))?;

    let mut offers: Vec<CompareOffer> = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("failed to read offer row: {e}"))?
    {
        let price = int_col(&row, 2);
        let currency = {
            let c = str_col(&row, 3);
            if c.is_empty() { "TWD".to_string() } else { c }
        };
        let hotel = {
            let h = str_col(&row, 5);
            if h.is_empty() { str_col(&row, 6) } else { h }
        };
        offers.push(CompareOffer {
            source_id: str_col(&row, 1),
            scraped_at: str_col(&row, 9),
            price_per_person: price,
            price_total: price.map(|p| p * pax),
            currency,
            airline: str_col(&row, 4),
            hotel,
        });
    }

    // Sort by price (lowest first; None last) — matches the TS comparator.
    offers.sort_by(|a, b| match (a.price_per_person, b.price_per_person) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, _) => std::cmp::Ordering::Greater,
        (_, None) => std::cmp::Ordering::Less,
        (Some(x), Some(y)) => x.cmp(&y),
    });

    Ok(offers)
}

fn print_offer_comparison(
    offers: &[CompareOffer],
    region: &str,
    filter_date: Option<&str>,
    pax: i64,
) {
    println!("\n  PACKAGE COMPARISON: {}", region.to_uppercase());
    println!("  Pax: {} | Filter date: {}", pax, filter_date.unwrap_or("(all)"));
    println!("  Found {} scraped file(s)\n", offers.len());

    if offers.is_empty() {
        return;
    }

    let bar = "-".repeat(95);
    println!("{bar}");
    let total_header = truncate(&pad_end(&format!("Total ({pax}pax)"), 15), 15);
    println!("  OTA               Price/person    {total_header}  Details");
    println!("{bar}");

    for o in offers {
        let price_str = match o.price_per_person {
            Some(p) => format!("{} {}", o.currency, locale(p)),
            None => "(no price)".to_string(),
        };
        let total_str = match o.price_total {
            Some(t) => format!("{} {}", o.currency, locale(t)),
            None => "-".to_string(),
        };
        let mut details: Vec<String> = Vec::new();
        if !o.airline.is_empty() {
            details.push(o.airline.clone());
        }
        if !o.hotel.is_empty() {
            details.push(truncate(&o.hotel, 20));
        }
        let detail_str = {
            let joined = truncate(&details.join(" | "), 30);
            if joined.is_empty() { "-".to_string() } else { joined }
        };
        println!(
            "  {} {} {} {}",
            pad_end(&source_display_name(&o.source_id), 15),
            pad_end(&price_str, 15),
            pad_end(&total_str, 15),
            detail_str
        );
    }
    println!("{bar}");

    // Staleness warning (>24h old).
    let now_ms = chrono::Utc::now().timestamp_millis();
    let stale = offers
        .iter()
        .filter(|o| {
            if o.scraped_at.is_empty() {
                return false;
            }
            match parse_iso_to_ms(&o.scraped_at) {
                Some(t) => now_ms - t > STALE_THRESHOLD_MS,
                None => false,
            }
        })
        .count();
    if stale > 0 {
        println!("\n  Warning: {stale} offer(s) have stale data (>24h old). Consider re-scraping.");
    }

    // Best value recommendation.
    let best = &offers[0];
    if let Some(p) = best.price_per_person {
        println!(
            "\n  Best value: {} at {} {}/person",
            source_display_name(&best.source_id),
            best.currency,
            locale(p)
        );
        if !best.hotel.is_empty() {
            println!("   Hotel: {}", best.hotel);
        }
        if !best.airline.is_empty() {
            println!("   Airline: {}", best.airline);
        }
    }
    println!();
}

// ── search-offers ────────────────────────────────────────────────────

pub async fn run_search(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Search for travel offers from registered OTA sources.\n\
             \n\
             Usage:\n  travel search-offers --dest <slug> [--start <date>] [--end <date>] \
             [--pax N] [--types package,flight,hotel] [--source <id>]\n\
             \n\
             Note: in-process OTA scrapers are DECOMMISSIONED. Live capture is done\n\
             via the chromeport CDP driver (a separate binary):\n  \
               ./rust/target/debug/chromeport fetch interact <url> --source <id> ...\n  \
               ./rust/target/debug/chromeport parse capture <capture-id> --source <id>"
        );
        return Ok(());
    }

    let mut dest: Option<String> = None;
    let mut start: Option<String> = None;
    let mut end: Option<String> = None;
    let mut pax: i64 = 2;
    let mut types: Option<String> = None;
    let mut source: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dest" => {
                i += 1;
                dest = args.get(i).cloned();
            }
            "--start" => {
                i += 1;
                start = args.get(i).cloned();
            }
            "--end" => {
                i += 1;
                end = args.get(i).cloned();
            }
            "--pax" => {
                i += 1;
                match args.get(i).and_then(|s| s.parse::<i64>().ok()) {
                    Some(p) if p > 0 => pax = p,
                    _ => {
                        eprintln!("Error: --pax must be a positive integer");
                        std::process::exit(1);
                    }
                }
            }
            "--types" => {
                i += 1;
                types = args.get(i).cloned();
            }
            "--source" => {
                i += 1;
                source = args.get(i).cloned();
            }
            other if other.starts_with("--") => {
                eprintln!("Error: unknown flag for search-offers: {other}");
                std::process::exit(1);
            }
            _ => {}
        }
        i += 1;
    }

    let Some(destination) = dest else {
        eprintln!("Error: search-offers requires --dest <slug>");
        std::process::exit(1);
    };

    // Resolve dates: explicit --start/--end, or the destination's confirmed
    // date anchor (date_anchors table).
    let (start_date, end_date) = match (start, end) {
        (Some(s), Some(e)) => (s, e),
        _ => {
            let anchor = read_confirmed_dates(&destination).await?;
            match anchor {
                Some((s, e)) => (start_or(s), end_or(e)),
                None => {
                    eprintln!("Error: search-offers requires --start and --end (or destination confirmed dates in plan).");
                    eprintln!("Fix: set-dates <start> <end> first, or pass --start/--end explicitly.");
                    std::process::exit(1);
                }
            }
        }
    };

    if !is_iso_date(&start_date) || !is_iso_date(&end_date) {
        eprintln!("Error: --start/--end must be YYYY-MM-DD");
        std::process::exit(1);
    }
    if start_date > end_date {
        eprintln!("Error: --start {start_date} is after --end {end_date}");
        std::process::exit(1);
    }

    if let Some(t) = &types {
        for p in t.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()) {
            if !matches!(p.as_str(), "package" | "flight" | "hotel") {
                eprintln!("Error: --types must be a comma-separated list of: package,flight,hotel");
                std::process::exit(1);
            }
        }
    }

    let days = day_span(&start_date, &end_date).unwrap_or(0);
    println!("\nsearch-offers ({destination})");
    println!("   Dates: {start_date} → {end_date} ({days} days)");
    println!("   Pax: {pax}");
    if let Some(t) = &types {
        println!("   Types: {t}");
    }
    if let Some(s) = &source {
        println!("   Source: {s}");
    }

    // No in-process scraper registry — every source is unregistered here.
    println!("\nResults:");
    let label = source.as_deref().unwrap_or("(all sources)");
    println!("  FAIL {label}: 0 offer(s)");
    println!(
        "     Errors: No in-process scraper registered. OTA capture is DECOMMISSIONED — \
         use the chromeport CDP driver (see --help)."
    );
    println!();

    std::process::exit(1);
}

async fn read_confirmed_dates(destination: &str) -> Result<Option<(String, String)>, String> {
    let conn = db::connect_read().await?;
    let mut rows = conn
        .query(
            "SELECT start_date, end_date FROM date_anchors WHERE destination = ?1 LIMIT 1",
            libsql::params![destination.to_string()],
        )
        .await
        .map_err(|e| format!("date_anchors query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("date_anchors row read failed: {e}"))?
    {
        let s: String = row.get(0).unwrap_or_default();
        let e: String = row.get(1).unwrap_or_default();
        if !s.is_empty() && !e.is_empty() {
            return Ok(Some((s, e)));
        }
    }
    Ok(None)
}

fn start_or(s: String) -> String {
    s
}
fn end_or(e: String) -> String {
    e
}

// ── shared helpers ───────────────────────────────────────────────────

fn day_span(start: &str, end: &str) -> Option<i64> {
    let s = chrono::NaiveDate::parse_from_str(start, "%Y-%m-%d").ok()?;
    let e = chrono::NaiveDate::parse_from_str(end, "%Y-%m-%d").ok()?;
    Some((e - s).num_days() + 1)
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..10].iter().all(u8::is_ascii_digit)
}

fn parse_iso_to_ms(raw: &str) -> Option<i64> {
    use chrono::NaiveDateTime;
    if (raw.contains('Z') || raw.contains('+'))
        && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw)
    {
        return Some(dt.timestamp_millis());
    }
    let normalized = raw.replace(' ', "T");
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(&normalized, fmt) {
            return Some(dt.and_utc().timestamp_millis());
        }
    }
    None
}

fn str_col(row: &libsql::Row, i: i32) -> String {
    match row.get_value(i) {
        Ok(Value::Text(s)) => s,
        Ok(Value::Integer(n)) => n.to_string(),
        Ok(Value::Real(f)) => f.to_string(),
        _ => String::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names() {
        // Mirrors TS loadScrapedOffers: source_name = raw source_id.
        assert_eq!(source_display_name("besttour"), "besttour");
        assert_eq!(source_display_name("unknown_x"), "unknown_x");
    }

    #[test]
    fn iso_validation() {
        assert!(is_iso_date("2026-02-24"));
        assert!(!is_iso_date("2026-2-24"));
        assert!(!is_iso_date("not-a-date"));
    }

    #[test]
    fn span() {
        assert_eq!(day_span("2026-02-24", "2026-02-28"), Some(5));
    }

    #[test]
    fn locale_fmt() {
        assert_eq!(locale(31888), "31,888");
    }
}
