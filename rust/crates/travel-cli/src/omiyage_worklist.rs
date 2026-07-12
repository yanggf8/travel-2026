// `travel omiyage-worklist --slug <slug>` — READ-ONLY discovery of omiyage-tagged
// POIs as an unverified research worklist. Writes NOTHING. Reference data — no
// --plan-id, no audit. The agent gwebcdb-verifies each candidate then persists
// via `add-omiyage`.
//
// Fail-loud paths (distinct messages):
//   unknown dest  → not in destination_config
//   empty         → known dest, zero omiyage-tagged POIs

use travel_db::repo::omiyage::{self, WorklistPoi};

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

    let pois = omiyage::omiyage_worklist_pois(&conn, &slug).await?;
    if pois.is_empty() {
        return Err(format!(
            "Error: no omiyage-tagged POIs for '{slug}' — tag shopping POIs with tag='omiyage' first, or add omiyage directly with: travel add-omiyage ..."
        ));
    }

    render(&slug, &pois);
    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel omiyage-worklist --slug <slug>\n  \
     (slug-keyed reference data — no --plan-id; lists omiyage-tagged POIs as an unverified research worklist; writes nothing)"
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
                     (omiyage-worklist is global/slug-keyed and takes no --plan-id)"
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

/// Print the research worklist: header + WARNING + per-POI notes/hint + verify + template.
/// Notes are printed VERBATIM — never parsed into item/seller facts.
fn render(slug: &str, pois: &[WorklistPoi]) {
    println!("Omiyage research worklist: {slug}");
    println!("WARNING: POI notes are hints, not item or seller evidence.");
    println!();

    for p in pois {
        println!("POI {} — {}", p.poi_id, p.title);
        println!("  area: {}", p.area);
        let note = p
            .notes
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("(none)");
        println!("  note hint: {note}");
        println!("  already sourced here: {}", p.already_sourced);
        println!();
        println!("  VERIFY BEFORE ADDING:");
        println!("    1. official item/product page");
        println!(
            "    2. official branch/floor-guide page proving sale at {}",
            p.poi_id
        );
        println!();
        println!("  CONFIRM WITH:");
        println!(
            "    travel add-omiyage {slug} <item_id> --buy-at {} \\",
            p.poi_id
        );
        println!("      --location-source-url <url> --location-confidence <confidence> \\");
        println!("      --name <name> --category <category> \\");
        println!("      --item-source-url <url> --item-confidence <confidence>");
        println!();
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
