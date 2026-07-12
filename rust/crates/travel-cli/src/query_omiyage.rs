// `travel query-omiyage --slug <slug>`
// — read-only plain-text view of destination-scoped omiyage recommendations
// (items + purchase locations joined to destination_pois). Slug-keyed GLOBAL
// reference data — NO --plan-id, NO audit triad.
//
// Fail-loud paths (distinct messages):
//   unknown dest  → not in destination_config
//   empty         → known dest, zero omiyage rows
//   corrupt       → location row whose poi_id has no destination_pois join

use travel_db::repo::omiyage::{self, OmiyageRow};

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }

    let slug = parse_args(args)?;
    let conn = crate::db::connect_read().await?;

    if !omiyage::config_slug_exists(&conn, &slug).await? {
        return Err(format!(
            "Error: unknown destination '{slug}' — not in destination_config"
        ));
    }

    let rows = omiyage::query_omiyage(&conn, &slug).await?;
    if rows.is_empty() {
        return Err(format!(
            "Error: no sourced omiyage for '{slug}' — add with: travel add-omiyage ..."
        ));
    }

    // Fail loud on any orphan location (LEFT JOIN left poi fields NULL).
    for r in &rows {
        let orphan = r.poi_title.is_none() && r.area.is_none() && r.station.is_none();
        if orphan {
            return Err(format!(
                "Error: corrupt omiyage row — location poi_id '{}' for item '{}' has no destination_pois row (run validate data)",
                r.poi_id, r.item_id
            ));
        }
    }

    render(&slug, &rows);
    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel query-omiyage --slug <slug>\n  \
     (slug-keyed reference data — no --plan-id; lists omiyage items + purchase locations)"
}

fn parse_args(args: &[String]) -> Result<String, String> {
    let mut slug: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--slug" => {
                slug = Some(arg_value(args, i, "--slug")?);
                i += 2;
            }
            "--plan-id" => {
                return Err(
                    "no --plan-id here — omiyage is destination-scoped reference data \
                     (query-omiyage is global/slug-keyed and takes no --plan-id)"
                        .to_string(),
                );
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            other => {
                return Err(format!(
                    "unexpected positional argument: {other}\n{}",
                    usage()
                ));
            }
        }
    }

    let slug = slug.ok_or_else(|| format!("--slug is required.\n{}", usage()))?;
    if slug.trim().is_empty() {
        return Err("--slug cannot be empty".to_string());
    }
    Ok(slug)
}

fn arg_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Group consecutive rows sharing the same item_id under one item header.
/// Rows arrive pre-ordered: category, name, item_id, poi_title, poi_id.
fn render(slug: &str, rows: &[OmiyageRow]) {
    println!("# Omiyage ({slug})");
    println!();

    let mut last_category: Option<&str> = None;
    let mut last_item_id: Option<&str> = None;

    for r in rows {
        // Category header when category changes.
        if last_category != Some(r.category.as_str()) {
            if last_category.is_some() {
                println!();
            }
            println!("## {}", r.category);
            println!();
            last_category = Some(r.category.as_str());
            last_item_id = None; // force item header under new category
        }

        // Item header once per item_id (consecutive rows already grouped by ORDER BY).
        if last_item_id != Some(r.item_id.as_str()) {
            println!("  {} — {}", r.item_id, r.name);
            if let Some(notes) = &r.item_notes {
                println!("    notes: {notes}");
            }
            println!(
                "    item source: {} [{}] @ {}",
                r.item_source_url, r.item_confidence, r.item_fetched_at
            );
            last_item_id = Some(r.item_id.as_str());
        }

        // One seller/location block per row.
        let title = r.poi_title.as_deref().unwrap_or("?");
        println!("    @ {title} ({})", r.poi_id);
        println!("      area: {}", dash_opt(r.area.as_deref()));
        println!("      station: {}", dash_opt(r.station.as_deref()));
        println!("      address: {}", dash_opt(r.address.as_deref()));
        println!("      hours: {}", dash_opt(r.hours.as_deref()));
        if let Some(note) = &r.purchase_note {
            println!("      purchase note: {note}");
        }
        println!(
            "      loc source: {} [{}] @ {}",
            r.loc_source_url, r.loc_confidence, r.loc_fetched_at
        );
    }
}

fn dash_opt(v: Option<&str>) -> &str {
    match v {
        Some(s) if !s.is_empty() => s,
        _ => "—",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_slug() {
        let s = parse_args(&a(&["--slug", "tokyo_2026"])).unwrap();
        assert_eq!(s, "tokyo_2026");
    }

    #[test]
    fn rejects_missing_slug() {
        let e = parse_args(&a(&[])).unwrap_err();
        assert!(e.contains("--slug"));
    }

    #[test]
    fn rejects_plan_id() {
        let e = parse_args(&a(&["--slug", "x", "--plan-id", "p"])).unwrap_err();
        assert!(e.contains("plan-id") || e.contains("--plan-id"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let e = parse_args(&a(&["--slug", "x", "--slugg", "y"])).unwrap_err();
        assert!(e.contains("unknown argument: --slugg"));
    }

    #[test]
    fn rejects_positional() {
        let e = parse_args(&a(&["tokyo_2026"])).unwrap_err();
        assert!(e.contains("positional") || e.contains("--slug"));
    }
}
