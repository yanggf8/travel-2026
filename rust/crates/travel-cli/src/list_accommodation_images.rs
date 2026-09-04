// `travel list-accommodation-images --dest <slug> | --id <accommodation_id>`
// — list the gallery photos the dashboard renders per domestic accommodation.
//
// Read-only, slug-keyed reference data — NO --plan-id. Plain-text table (no JSON).
// This is the publish-check surface for "does every candidate have enough photos":
// the per-hotel counts print at the bottom.

use travel_db::repo::domestic_accommodation_images::{
    DomesticAccommodationImageRow, list_by_accommodation, list_by_destination,
};
use travel_db::repo::omiyage::config_slug_exists;

#[derive(Debug)]
enum Scope {
    Dest(String),
    Id(String),
}

pub async fn run(raw: &[String]) -> Result<(), String> {
    if raw.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let scope = parse_args(raw)?;

    let conn = crate::db::connect_read().await?;
    let (title, rows) = match &scope {
        Scope::Dest(dest) => {
            if !config_slug_exists(&conn, dest).await? {
                return Err(format!(
                    "Error: unknown destination '{dest}' — not in destination_config"
                ));
            }
            (format!("dest={dest}"), list_by_destination(&conn, dest).await?)
        }
        Scope::Id(id) => (
            format!("id={id}"),
            list_by_accommodation(&conn, id).await?,
        ),
    };

    print_table(&title, &rows);
    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel list-accommodation-images --dest <slug>\n  \
     travel list-accommodation-images --id <accommodation_id>\n  \
     (read-only, slug-keyed reference data — no --plan-id)"
}

fn parse_args(raw: &[String]) -> Result<Scope, String> {
    let mut dest: Option<String> = None;
    let mut id: Option<String> = None;
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
            "--id" | "--accommodation-id" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--id cannot be empty".to_string());
                }
                id = Some(v);
                i += 2;
            }
            "--plan-id" => {
                return Err(
                    "no --plan-id here — domestic_accommodation_images is destination-scoped reference data \
                     (list-accommodation-images is global/slug-keyed and takes no --plan-id)"
                        .to_string(),
                );
            }
            other if other.starts_with("--") => {
                return Err(format!(
                    "unknown flag for list-accommodation-images: {other}"
                ));
            }
            other => return Err(format!("unexpected positional argument: {other}")),
        }
    }
    match (dest, id) {
        (Some(_), Some(_)) => {
            Err("pass either --dest or --id, not both".to_string())
        }
        (Some(d), None) => Ok(Scope::Dest(d)),
        (None, Some(i)) => Ok(Scope::Id(i)),
        (None, None) => Err(format!("--dest <slug> or --id <accommodation_id> is required.\n{}", usage())),
    }
}

fn val(raw: &[String], i: usize, flag: &str) -> Result<String, String> {
    raw.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn print_table(title: &str, rows: &[DomesticAccommodationImageRow]) {
    println!(
        "\nAccommodation Gallery — {} ({} photo(s))",
        title,
        rows.len()
    );
    if rows.is_empty() {
        println!("No gallery photos found.");
        println!(
            "Add one via `travel add-accommodation-image --id <accommodation_id> --url <url> --label <text>`."
        );
        return;
    }
    let header = format!(
        "{:<16} │ {:<30} │ {:>4} │ {:<14} │ {}",
        "hotel_name", "accommodation_id", "sort", "label", "image_url"
    );
    let bar = "─".repeat(header.chars().count());
    println!("{bar}");
    println!("{header}");
    println!("{bar}");
    for r in rows {
        let hotel: String = r.hotel_name.chars().take(16).collect();
        let id: String = r.accommodation_id.chars().take(30).collect();
        let label: String = if r.label.is_empty() {
            "-".to_string()
        } else {
            r.label.chars().take(14).collect()
        };
        println!(
            "{:<16} │ {:<30} │ {:>4} │ {:<14} │ {}",
            hotel, id, r.sort_order, label, r.image_url
        );
    }
    println!("{bar}");
    for (hotel, n) in per_hotel_counts(rows) {
        println!("  {hotel}: {n} photo(s)");
    }
}

/// Per-hotel photo counts, in first-seen (price-ascending) order.
/// Pure — unit-tested without a DB.
fn per_hotel_counts(rows: &[DomesticAccommodationImageRow]) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for r in rows {
        match out.iter_mut().find(|(h, _)| *h == r.hotel_name) {
            Some((_, n)) => *n += 1,
            None => out.push((r.hotel_name.clone(), 1)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    fn img(hotel: &str, id: &str) -> DomesticAccommodationImageRow {
        DomesticAccommodationImageRow {
            accommodation_id: id.to_string(),
            image_url: "https://img".to_string(),
            label: String::new(),
            sort_order: 1,
            hotel_name: hotel.to_string(),
        }
    }

    #[test]
    fn parses_dest_scope() {
        assert!(matches!(
            parse_args(&a(&["--dest", "jiufen"])).unwrap(),
            Scope::Dest(d) if d == "jiufen"
        ));
    }

    #[test]
    fn parses_id_scope() {
        assert!(matches!(
            parse_args(&a(&["--id", "acc1"])).unwrap(),
            Scope::Id(i) if i == "acc1"
        ));
    }

    #[test]
    fn rejects_both_scopes() {
        let e = parse_args(&a(&["--dest", "jiufen", "--id", "acc1"])).unwrap_err();
        assert!(e.contains("not both"));
    }

    #[test]
    fn rejects_no_scope() {
        let e = parse_args(&[]).unwrap_err();
        assert!(e.contains("--dest"));
    }

    #[test]
    fn rejects_plan_id() {
        let e = parse_args(&a(&["--dest", "jiufen", "--plan-id", "p"])).unwrap_err();
        assert!(e.contains("no --plan-id"));
    }

    #[test]
    fn counts_photos_per_hotel_in_order() {
        let rows = vec![
            img("山城逸境", "a"),
            img("山城逸境", "a"),
            img("海論", "b"),
        ];
        assert_eq!(
            per_hotel_counts(&rows),
            vec![("山城逸境".to_string(), 2), ("海論".to_string(), 1)]
        );
    }
}
