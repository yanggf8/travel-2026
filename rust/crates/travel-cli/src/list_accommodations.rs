// `travel list-accommodations --dest <slug> [--limit N]`
// — list all `domestic_accommodations` rows for a destination, plain-text table.
//
// Slug-keyed GLOBAL reference data — NO --plan-id, NO audit triad (same family as
// query-omiyage / add-transit). The slug is validated against destination_config
// (fail loud on an unknown destination). Read-only; no JSON output.

use travel_db::repo::domestic_accommodations::{DomesticAccommodationFilter, query};
use travel_db::repo::omiyage::config_slug_exists;

#[derive(Debug)]
struct Args {
    dest: String,
    limit: i64,
}

pub async fn run(raw: &[String]) -> Result<(), String> {
    if raw.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let args = parse_args(raw)?;

    let conn = crate::db::connect_read().await?;
    if !config_slug_exists(&conn, &args.dest).await? {
        return Err(format!(
            "Error: unknown destination '{}' — not in destination_config",
            args.dest
        ));
    }

    let filter = DomesticAccommodationFilter::new().destination(&args.dest);
    let rows = query(&conn, filter, args.limit).await?;

    print_table(&rows, &args);
    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel list-accommodations --dest <slug> [--limit N]\n  \
     (slug-keyed reference data — no --plan-id; lists every domestic_accommodations row incl. image/booking link status)"
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut dest: Option<String> = None;
    let mut limit: i64 = 50;
    let mut i = 0;
    while i < raw.len() {
        let k = raw[i].as_str();
        match k {
            "--dest" | "--destination" | "--slug" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err(format!("{k} cannot be empty"));
                }
                dest = Some(v);
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
            "--plan-id" => {
                return Err(
                    "no --plan-id here — domestic_accommodations is destination-scoped reference data \
                     (list-accommodations is global/slug-keyed and takes no --plan-id)"
                        .to_string(),
                );
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown flag for list-accommodations: {other}"));
            }
            other => return Err(format!("unexpected positional argument: {other}")),
        }
    }
    let dest = dest.ok_or_else(|| format!("--dest <slug> is required.\n{}", usage()))?;
    Ok(Args { dest, limit })
}

fn val(raw: &[String], i: usize, flag: &str) -> Result<String, String> {
    raw.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn yn(v: &Option<String>) -> &'static str {
    match v.as_deref() {
        Some(s) if !s.trim().is_empty() => "yes",
        _ => "-",
    }
}

fn print_table(
    rows: &[travel_db::repo::domestic_accommodations::DomesticAccommodationRow],
    args: &Args,
) {
    println!(
        "\nDomestic Accommodations — dest={} ({} result(s))",
        args.dest,
        rows.len()
    );
    if rows.is_empty() {
        println!("No accommodations found.");
        return;
    }
    let header = format!(
        "{:<30} │ {:<16} │ {:<14} │ {:>10} │ {:<8} │ {:<9} │ {:<5} │ {:<7} │ {}",
        "id", "hotel_name", "room_type", "price_twd", "sea_view", "breakfast", "image", "booking", "updated_at"
    );
    let bar = "─".repeat(header.chars().count());
    println!("{bar}");
    println!("{header}");
    println!("{bar}");
    for r in rows {
        let sea = if r.sea_view == 1 { "yes" } else { "no" };
        let bf = if r.breakfast_included == 1 { "yes" } else { "no" };
        let id: String = r.id.chars().take(30).collect();
        let hotel: String = r.hotel_name.chars().take(16).collect();
        let room: String = r.room_type.chars().take(14).collect();
        println!(
            "{:<30} │ {:<16} │ {:<14} │ {:>10} │ {:<8} │ {:<9} │ {:<5} │ {:<7} │ {}",
            id,
            hotel,
            room,
            format!("TWD {}", r.price_twd),
            sea,
            bf,
            yn(&r.image_url),
            yn(&r.booking_url),
            r.updated_at
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
    fn parses_required_dest() {
        let o = parse_args(&a(&["--dest", "jiufen"])).unwrap();
        assert_eq!(o.dest, "jiufen");
        assert_eq!(o.limit, 50);
    }

    #[test]
    fn parses_limit() {
        let o = parse_args(&a(&["--dest", "jiufen", "--limit", "10"])).unwrap();
        assert_eq!(o.limit, 10);
    }

    #[test]
    fn rejects_missing_dest() {
        let e = parse_args(&a(&["--limit", "10"])).unwrap_err();
        assert!(e.contains("--dest"));
    }

    #[test]
    fn rejects_bad_limit() {
        let e = parse_args(&a(&["--dest", "jiufen", "--limit", "0"])).unwrap_err();
        assert!(e.contains("--limit"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let e = parse_args(&a(&["--dest", "jiufen", "--bogus"])).unwrap_err();
        assert!(e.contains("unknown flag"));
    }

    #[test]
    fn rejects_plan_id() {
        let e = parse_args(&a(&["--dest", "jiufen", "--plan-id", "x"])).unwrap_err();
        assert!(e.contains("no --plan-id"));
    }
}
