// `travel query-accommodation --dest <slug> [--date YYYY-MM-DD] [--sea-view] [--hotel <name>] [--limit N]`
// — read `domestic_accommodations` with parameterized WHERE (OfferFilter pattern), plain-text table.
//
// `--dest` is REQUIRED and validated via `resolve_active_destination` (fail loud if missing/unknown).
// `--date` is OPTIONAL: validates ISO shape when provided, printed in header, no date predicate
// (future availability column placeholder — table has no date column yet).
// No JSON output, no sql_quote.

use crate::cascade::common::resolve_active_destination;
use travel_db::repo::domestic_accommodations::{DomesticAccommodationFilter, query};

#[derive(Debug)]
struct Args {
    dest: String,
    date: Option<String>,
    sea_view: bool,
    hotel: Option<String>,
    limit: i64,
}

pub async fn run(raw: &[String]) -> Result<(), String> {
    let args = parse_args(raw)?;

    // Resolve plan_id + validate dest exists for that plan (fail loud if missing/phantom).
    // Keep same resolver as other destination-scoped reads (offers/bookings excluded by design).
    let plan_id = crate::plan_resolver::resolve_plan_id(raw).await?;
    let conn = crate::db::connect_read().await?;
    let dest = resolve_active_destination(&conn, &plan_id, Some(&args.dest)).await?;

    let mut filter = DomesticAccommodationFilter::new().destination(&dest);
    if args.sea_view {
        filter = filter.sea_view_only();
    }
    if let Some(h) = &args.hotel {
        filter = filter.hotel_like(h);
    }

    let rows = query(&conn, filter, args.limit).await?;

    print_table(&rows, &args, &dest);
    Ok(())
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut dest: Option<String> = None;
    let mut date: Option<String> = None;
    let mut sea_view = false;
    let mut hotel: Option<String> = None;
    let mut limit: i64 = 50;
    let mut i = 0;
    while i < raw.len() {
        let k = raw[i].as_str();
        match k {
            "--dest" | "--destination" => {
                let v = val(raw, i, k)?;
                dest = Some(v);
                i += 2;
            }
            "--date" => {
                let v = val(raw, i, k)?;
                if !is_iso_date(&v) {
                    return Err(format!("Invalid --date format: {v} (expected YYYY-MM-DD)"));
                }
                date = Some(v);
                i += 2;
            }
            "--sea-view" => {
                sea_view = true;
                i += 1;
            }
            "--hotel" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--hotel cannot be empty".to_string());
                }
                hotel = Some(v);
                i += 2;
            }
            "--limit" => {
                let v = val(raw, i, k)?;
                let n: i64 = v
                    .parse()
                    .map_err(|_| "--limit must be an integer".to_string())?;
                if n <= 0 {
                    return Err("--limit must be > 0".to_string());
                }
                limit = n;
                i += 2;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other if other.starts_with("--") => {
                // Allow plan_resolver's pass-through flags to not error here; but reject truly unknown.
                // `--plan-id` / `--travel-date` etc are consumed by resolve_plan_id above — ignore them.
                if other == "--plan-id"
                    || other == "--travel-date"
                    || other == "--travel-start"
                    || other == "--travel-end"
                {
                    // value flag — skip value if present and not a flag
                    if raw.get(i + 1).is_some_and(|v| !v.starts_with("--")) {
                        i += 2;
                    } else {
                        i += 1;
                    }
                    continue;
                }
                return Err(format!("unknown flag for query-accommodation: {other}"));
            }
            other => return Err(format!("unexpected positional argument: {other}")),
        }
    }
    let dest = dest.ok_or_else(|| {
        format!(
            "--dest <slug> is required.\nUsage: travel query-accommodation --dest <slug> [--date YYYY-MM-DD] [--sea-view] [--hotel <name>] [--limit N]"
        )
    })?;
    Ok(Args {
        dest,
        date,
        sea_view,
        hotel,
        limit,
    })
}

fn val(raw: &[String], i: usize, flag: &str) -> Result<String, String> {
    raw.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && b[..4].iter().all(u8::is_ascii_digit) && b[5..7].iter().all(u8::is_ascii_digit) && b[8..10].iter().all(u8::is_ascii_digit)
}

fn print_usage() {
    println!(
        "Usage:\n  travel query-accommodation --dest <slug> [--date YYYY-MM-DD] [--sea-view] [--hotel <name>] [--limit N]\n  (destination validated via resolve_active_destination; plain-text table; --date optional, printed in header when provided)"
    );
}

fn dash(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}

fn print_table(
    rows: &[travel_db::repo::domestic_accommodations::DomesticAccommodationRow],
    args: &Args,
    dest: &str,
) {
    let date_label = args.date.as_deref().unwrap_or("-");
    println!("\nDomestic Accommodations — dest={dest} date={date_label} ({} result(s))", rows.len());
    if rows.is_empty() {
        println!("No accommodations found.");
        return;
    }
    // Columns: hotel_name | room_type | sea_view | price_twd | breakfast | source
    let header = format!(
        "{:<16} │ {:<14} │ {:<8} │ {:>10} │ {:<9} │ {}",
        "hotel_name", "room_type", "sea_view", "price_twd", "breakfast", "source"
    );
    let bar = "─".repeat(header.chars().count());
    println!("{bar}");
    println!("{header}");
    println!("{bar}");
    for r in rows {
        let sea = if r.sea_view == 1 { "yes" } else { "no" };
        let bf = if r.breakfast_included == 1 { "yes" } else { "no" };
        let hotel: String = r.hotel_name.chars().take(16).collect();
        let room: String = r.room_type.chars().take(14).collect();
        let src = r.source.as_deref().unwrap_or("-");
        println!(
            "{:<16} │ {:<14} │ {:<8} │ {:>10} │ {:<9} │ {}",
            dash(&hotel),
            dash(&room),
            sea,
            format!("TWD {}", r.price_twd),
            bf,
            dash(src)
        );
    }
    println!("{bar}");
    println!("Showing {} of max {} results.", rows.len(), args.limit);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_required_fields() {
        let o = parse_args(&a(&["--dest", "jiufen", "--date", "2026-09-03"])).unwrap();
        assert_eq!(o.dest, "jiufen");
        assert_eq!(o.date.as_deref(), Some("2026-09-03"));
        assert!(!o.sea_view);
        assert_eq!(o.limit, 50);
    }

    #[test]
    fn parses_without_date() {
        let o = parse_args(&a(&["--dest", "jiufen"])).unwrap();
        assert_eq!(o.dest, "jiufen");
        assert!(o.date.is_none());
    }

    #[test]
    fn parses_optional_flags() {
        let o = parse_args(&a(&[
            "--dest", "jiufen", "--date", "2026-09-03", "--sea-view", "--hotel", "海論", "--limit", "10",
        ]))
        .unwrap();
        assert!(o.sea_view);
        assert_eq!(o.hotel.as_deref(), Some("海論"));
        assert_eq!(o.limit, 10);
    }

    #[test]
    fn rejects_missing_dest() {
        let e = parse_args(&a(&["--date", "2026-09-03"])).unwrap_err();
        assert!(e.contains("--dest"));
    }

    #[test]
    fn date_optional_no_error() {
        // --date is now optional; no error when missing
        let o = parse_args(&a(&["--dest", "jiufen"])).unwrap();
        assert!(o.date.is_none());
    }

    #[test]
    fn rejects_bad_date_format() {
        let e = parse_args(&a(&["--dest", "jiufen", "--date", "2026/09/03"])).unwrap_err();
        assert!(e.contains("Invalid --date"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let e = parse_args(&a(&["--dest", "jiufen", "--date", "2026-09-03", "--bogus"])).unwrap_err();
        assert!(e.contains("unknown flag"));
    }
}
