// `travel set-poi-title <slug> <poi_id> <title> [--segments]`
// — renames one `destination_pois` title (localization pass: seeds carry
// English titles, the dashboard should read Chinese/Japanese). Destination-ref
// data is slug-keyed GLOBAL data, NOT plan-keyed: NO audit triad (mirrors
// set-poi-coords). Plain-text output only.
//
// `--segments` also renames every `day_route_segments` endpoint label of that
// destination that still equals the OLD title — segment labels match POIs by
// exact normalized title keys, so leaving them behind would silently unpin
// those stops on the next snapshot.

use travel_db::repo::{destination_ref, route_segments};

#[derive(Debug, Default)]
struct ParsedArgs {
    slug: String,
    poi_id: String,
    title: String,
    segments: bool,
}

/// CLI entry: `travel set-poi-title <slug> <poi_id> <title> [--segments]`.
pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }

    let parsed = parse_args(args)?;

    let conn = crate::db::connect_write().await?;

    let Some(old) = destination_ref::poi_title(&conn, &parsed.slug, &parsed.poi_id).await? else {
        return Err(format!(
            "unknown (slug, poi_id) = ({}, {}) — no row in destination_pois",
            parsed.slug, parsed.poi_id
        ));
    };
    if old == parsed.title {
        return Err(format!(
            "title of ({}, {}) is already \"{}\" — nothing to do",
            parsed.slug, parsed.poi_id, parsed.title
        ));
    }

    let affected = destination_ref::set_poi_title(
        &conn,
        &parsed.slug,
        &parsed.poi_id,
        &parsed.title,
    )
    .await?;
    if affected != 1 {
        return Err(format!(
            "destination_pois title UPDATE affected {affected} rows (expected 1)"
        ));
    }

    println!(
        "✅ destination_pois title: {} / {}",
        parsed.slug, parsed.poi_id
    );
    println!("  {} → {}", old, parsed.title);

    if parsed.segments {
        let (from_n, to_n) =
            route_segments::rename_place_labels(&conn, &parsed.slug, &old, &parsed.title).await?;
        println!(
            "  day_route_segments renamed: {} from_place, {} to_place",
            from_n, to_n
        );
    } else if old != parsed.title {
        println!("  ℹ pass --segments to also rename matching segment labels (they match POIs by exact title)");
    }

    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel set-poi-title <slug> <poi_id> <title> [--segments]\n  (slug-keyed reference data — no --plan-id. --segments also renames the day_route_segments endpoint labels that equal the old title, keeping the exact-key POI match.)"
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut segments = false;
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--segments" => {
                segments = true;
                i += 1;
            }
            "--plan-id" => {
                return Err(
                    "set-poi-title is global/slug-keyed reference data and takes no --plan-id \
                     (a POI title is shared across every plan that uses the destination)"
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

    if positional.len() < 3 {
        return Err(format!("missing required arguments.\n{}", usage()));
    }
    let slug = positional[0].clone();
    if slug.trim().is_empty() {
        return Err("<slug> cannot be empty".to_string());
    }
    let poi_id = positional[1].clone();
    if poi_id.trim().is_empty() {
        return Err("<poi_id> cannot be empty".to_string());
    }
    let title = positional[2..].join(" ");
    if title.trim().is_empty() {
        return Err("<title> cannot be empty".to_string());
    }

    Ok(ParsedArgs {
        slug,
        poi_id,
        title: title.trim().to_string(),
        segments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_minimal() {
        let p = parse_args(&[
            "osaka_kyoto_2026".to_string(),
            "tenryuji".to_string(),
            "天龍寺".to_string(),
        ])
        .unwrap();
        assert_eq!(p.slug, "osaka_kyoto_2026");
        assert_eq!(p.poi_id, "tenryuji");
        assert_eq!(p.title, "天龍寺");
        assert!(!p.segments);
    }

    #[test]
    fn parse_args_title_may_contain_spaces() {
        let p = parse_args(&[
            "jiufen".to_string(),
            "poi_x".to_string(),
            "京嵐山".to_string(),
            "にしき".to_string(),
        ])
        .unwrap();
        assert_eq!(p.title, "京嵐山 にしき");
    }

    #[test]
    fn parse_args_with_segments() {
        let p = parse_args(&[
            "s".to_string(),
            "p".to_string(),
            "t".to_string(),
            "--segments".to_string(),
        ])
        .unwrap();
        assert!(p.segments);
    }

    #[test]
    fn parse_args_rejects_missing_positional() {
        assert!(parse_args(&["s".to_string(), "p".to_string()]).is_err());
    }

    #[test]
    fn parse_args_rejects_unknown_flag() {
        assert!(parse_args(&[
            "s".to_string(),
            "p".to_string(),
            "t".to_string(),
            "--bogus".to_string(),
        ])
        .is_err());
    }
}
