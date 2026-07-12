// `travel add-omiyage <slug> <item_id> --buy-at <poi_id>
//   --location-source-url <url> --location-confidence verified|reviewed
//   [--name] [--category] [--item-source-url] [--item-confidence]
//   [--notes] [--purchase-note]`
// — adds/updates omiyage item + purchase-location rows (destination-scoped
// reference data). Slug-keyed GLOBAL reference data, NOT plan-keyed: NO audit
// triad (no plan_events/operation_runs/plans.version), mirroring add-transit.
// Plain-text output only.
//
// Flow: parse (fail loud on bad confidence/URL/blank requireds) → connect_write
// → repo::omiyage::write_item_and_location (ONE atomic transactional writer).

use travel_db::repo::omiyage::{self, ItemFlags, LocationInput, WriteOutcome};

#[derive(Debug, Default)]
struct ParsedArgs {
    slug: String,
    item_id: String,
    buy_at: String,
    location_source_url: String,
    location_confidence: String,
    name: Option<String>,
    category: Option<String>,
    item_source_url: Option<String>,
    item_confidence: Option<String>,
    notes: Option<String>,
    purchase_note: Option<String>,
}

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }

    let parsed = parse_args(args)?;
    let fetched_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    let conn = crate::db::connect_write().await?;
    let outcome = omiyage::write_item_and_location(
        &conn,
        &parsed.slug,
        &parsed.item_id,
        ItemFlags {
            name: parsed.name.as_deref(),
            category: parsed.category.as_deref(),
            notes: parsed.notes.as_deref(),
            source_url: parsed.item_source_url.as_deref(),
            confidence: parsed.item_confidence.as_deref(),
        },
        LocationInput {
            poi_id: &parsed.buy_at,
            purchase_note: parsed.purchase_note.as_deref(),
            source_url: &parsed.location_source_url,
            confidence: &parsed.location_confidence,
        },
        &fetched_at,
    )
    .await
    .map_err(map_repo_err)?;

    match outcome {
        WriteOutcome::CreatedItemAndLocation => {
            let cat = parsed.category.as_deref().unwrap_or("?");
            println!(
                "✅ omiyage item created: {} ({}) @ {} [{}]",
                parsed.item_id, cat, parsed.buy_at, parsed.location_confidence
            );
        }
        WriteOutcome::UpsertedLocationOnly => {
            println!(
                "✅ omiyage seller added: {} @ {} [{}]",
                parsed.item_id, parsed.buy_at, parsed.location_confidence
            );
            println!("  (item already existed — location only)");
        }
    }
    println!("  slug: {}", parsed.slug);
    if let Some(name) = &parsed.name {
        println!("  name: {name}");
    }
    if let Some(cat) = &parsed.category {
        println!("  category: {cat}");
    }
    if let Some(note) = &parsed.purchase_note {
        println!("  purchase note: {note}");
    }
    println!("  location confidence: {}", parsed.location_confidence);
    // No audit triad — reference data like add-transit.
    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel add-omiyage <slug> <item_id> --buy-at <poi_id> \
     --location-source-url <url> --location-confidence verified|reviewed \
     [--name <text>] [--category <text>] [--item-source-url <url>] \
     [--item-confidence verified|reviewed] [--notes <text>] [--purchase-note <text>]\n  \
     (slug-keyed reference data — no --plan-id; omiyage is destination-scoped)"
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut buy_at: Option<String> = None;
    let mut location_source_url: Option<String> = None;
    let mut location_confidence: Option<String> = None;
    let mut name: Option<String> = None;
    let mut category: Option<String> = None;
    let mut item_source_url: Option<String> = None;
    let mut item_confidence: Option<String> = None;
    let mut notes: Option<String> = None;
    let mut purchase_note: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--buy-at" => {
                buy_at = Some(arg_value(args, i, "--buy-at")?);
                i += 2;
            }
            "--location-source-url" => {
                location_source_url = Some(arg_value(args, i, "--location-source-url")?);
                i += 2;
            }
            "--location-confidence" => {
                location_confidence = Some(arg_value(args, i, "--location-confidence")?);
                i += 2;
            }
            "--name" => {
                name = Some(arg_value(args, i, "--name")?);
                i += 2;
            }
            "--category" => {
                category = Some(arg_value(args, i, "--category")?);
                i += 2;
            }
            "--item-source-url" => {
                item_source_url = Some(arg_value(args, i, "--item-source-url")?);
                i += 2;
            }
            "--item-confidence" => {
                item_confidence = Some(arg_value(args, i, "--item-confidence")?);
                i += 2;
            }
            "--notes" => {
                notes = Some(arg_value(args, i, "--notes")?);
                i += 2;
            }
            "--purchase-note" => {
                purchase_note = Some(arg_value(args, i, "--purchase-note")?);
                i += 2;
            }
            "--plan-id" => {
                return Err(
                    "no --plan-id here — omiyage is destination-scoped reference data \
                     (add-omiyage is global/slug-keyed and takes no --plan-id)"
                        .to_string(),
                );
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => {
                positional.push(a.clone());
                i += 1;
            }
        }
    }

    if positional.len() < 2 {
        return Err(format!("missing required arguments.\n{}", usage()));
    }
    if positional.len() > 2 {
        return Err(format!(
            "excess positional argument(s): expected <slug> <item_id>, got {} positionals",
            positional.len()
        ));
    }
    let slug = positional[0].clone();
    let item_id = positional[1].clone();
    if slug.trim().is_empty() {
        return Err("<slug> cannot be empty".to_string());
    }
    if item_id.trim().is_empty() {
        return Err("<item_id> cannot be empty".to_string());
    }

    let buy_at = require_nonblank_opt(buy_at, "--buy-at")?;
    let location_source_url =
        require_nonblank_opt(location_source_url, "--location-source-url")?;
    let location_confidence =
        require_nonblank_opt(location_confidence, "--location-confidence")?;

    validate_confidence(&location_confidence, "--location-confidence")?;
    if let Some(ref conf) = item_confidence {
        validate_confidence(conf, "--item-confidence")?;
    }

    validate_http_url(&location_source_url, "--location-source-url")?;
    if let Some(ref url) = item_source_url {
        validate_http_url(url, "--item-source-url")?;
    }

    Ok(ParsedArgs {
        slug,
        item_id,
        buy_at,
        location_source_url,
        location_confidence,
        name,
        category,
        item_source_url,
        item_confidence,
        notes,
        purchase_note,
    })
}

fn require_nonblank_opt(val: Option<String>, flag: &str) -> Result<String, String> {
    match val {
        None => Err(format!("{flag} is required")),
        Some(s) if s.trim().is_empty() => Err(format!("{flag} cannot be blank")),
        Some(s) => Ok(s),
    }
}

fn validate_confidence(val: &str, flag: &str) -> Result<(), String> {
    const CONFS: [&str; 2] = ["verified", "reviewed"];
    if !CONFS.contains(&val) {
        return Err(format!("{flag} must be one of {CONFS:?} (got \"{val}\")"));
    }
    Ok(())
}

fn validate_http_url(url: &str, flag: &str) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!(
            "{flag} must start with http:// or https:// (got \"{url}\")"
        ));
    }
    Ok(())
}

fn arg_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Map repo field-name errors to CLI flag names where helpful.
fn map_repo_err(e: String) -> String {
    e.replace("requires --source_url", "requires --item-source-url")
        .replace(
            "requires non-blank source_url",
            "requires non-blank --item-source-url",
        )
        .replace("requires --confidence", "requires --item-confidence")
        .replace(
            "requires non-blank confidence",
            "requires non-blank --item-confidence",
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_minimal_with_full_bundle() {
        let p = parse_args(&a(&[
            "tokyo_2026",
            "tokyo_banana",
            "--name",
            "Tokyo Banana",
            "--category",
            "和菓子",
            "--buy-at",
            "poi1",
            "--item-source-url",
            "https://example.com/i",
            "--item-confidence",
            "verified",
            "--location-source-url",
            "https://example.com/l",
            "--location-confidence",
            "reviewed",
        ]))
        .unwrap();
        assert_eq!(p.slug, "tokyo_2026");
        assert_eq!(p.item_id, "tokyo_banana");
        assert_eq!(p.buy_at, "poi1");
        assert_eq!(p.location_confidence, "reviewed");
        assert_eq!(p.name.as_deref(), Some("Tokyo Banana"));
    }

    #[test]
    fn parses_location_only_for_existing_item() {
        let p = parse_args(&a(&[
            "s",
            "item_x",
            "--buy-at",
            "poi2",
            "--location-source-url",
            "https://example.com/l2",
            "--location-confidence",
            "verified",
        ]))
        .unwrap();
        assert!(p.name.is_none());
        assert!(p.category.is_none());
        assert!(p.item_source_url.is_none());
        assert!(p.item_confidence.is_none());
    }

    #[test]
    fn rejects_plan_id() {
        let e = parse_args(&a(&[
            "s",
            "i",
            "--buy-at",
            "p",
            "--location-source-url",
            "https://example.com/l",
            "--location-confidence",
            "verified",
            "--plan-id",
            "x",
        ]));
        assert!(e.unwrap_err().contains("no --plan-id"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let e = parse_args(&a(&[
            "s",
            "i",
            "--buy-at",
            "p",
            "--location-source-url",
            "https://example.com/l",
            "--location-confidence",
            "verified",
            "--bogus",
        ]));
        assert!(e.unwrap_err().contains("unknown argument: --bogus"));
    }

    #[test]
    fn rejects_bad_confidence() {
        let e = parse_args(&a(&[
            "s",
            "i",
            "--buy-at",
            "p",
            "--location-source-url",
            "https://example.com/l",
            "--location-confidence",
            "guess",
        ]));
        assert!(e.unwrap_err().contains("--location-confidence"));
    }

    #[test]
    fn rejects_non_http_url() {
        let e = parse_args(&a(&[
            "s",
            "i",
            "--buy-at",
            "p",
            "--location-source-url",
            "ftp://example.com/l",
            "--location-confidence",
            "verified",
        ]));
        assert!(e.unwrap_err().contains("http://"));
    }

    #[test]
    fn rejects_missing_location_url() {
        let e = parse_args(&a(&[
            "s",
            "i",
            "--buy-at",
            "p",
            "--location-confidence",
            "verified",
        ]));
        assert!(e.unwrap_err().contains("--location-source-url"));
    }

    #[test]
    fn rejects_excess_positional() {
        let e = parse_args(&a(&[
            "s",
            "i",
            "extra",
            "--buy-at",
            "p",
            "--location-source-url",
            "https://example.com/l",
            "--location-confidence",
            "verified",
        ]));
        assert!(e.unwrap_err().contains("excess positional"));
    }

    #[test]
    fn rejects_missing_flag_value() {
        let e = parse_args(&a(&[
            "s",
            "i",
            "--buy-at",
            "p",
            "--location-source-url",
        ]));
        assert!(e.unwrap_err().contains("needs a value"));
    }
}
