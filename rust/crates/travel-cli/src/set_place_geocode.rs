// `travel set-place-geocode "<place>" --lat <f> --lon <f> (--dest <slug> | --context "<ctx>")`
// — manually pins a route-segment place label that Nominatim cannot resolve
// (typically a localized name after a `set-poi-title` pass: 京都國際會館 has no
// Nominatim hit in Japan, so the stop silently drops off the map). The row is
// written into `route_place_geocodes` under the EXACT query key
// `snapshot_maps::resolve_place` reads (same `normalize_place` formula), so
// the next `snapshot-maps` run is a guaranteed cache hit. Reference/cache data
// — NO audit triad. Plain-text output only.

use travel_db::repo::geocodes;

#[derive(Debug, Default)]
struct ParsedArgs {
    place: String,
    lat: f64,
    lon: f64,
    dest: Option<String>,
    context: Option<String>,
}

/// The cache key `resolve_place` reads with — shared verbatim so a manual row
/// can never miss by construction.
pub(crate) fn cache_key(place: &str, context: &str) -> String {
    let (search, ctx) = crate::snapshot_maps::normalize_place(place, context);
    format!("{}|{}", search.trim().to_ascii_lowercase(), ctx)
}

/// CLI entry: `travel set-place-geocode "<place>" --lat N --lon N (--dest <slug> | --context "<ctx>")`.
pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }

    let parsed = parse_args(args)?;

    let conn = crate::db::connect_write().await?;

    let context = match (&parsed.dest, &parsed.context) {
        (Some(dest), None) => {
            crate::snapshot_maps::geocode_context(&conn, dest).await?
        }
        (None, Some(ctx)) => ctx.clone(),
        // parse_args already rejects both-given / neither-given.
        _ => unreachable!("parse_args validated the --dest/--context pair"),
    };

    let key = cache_key(&parsed.place, &context);
    geocodes::upsert_manual(
        &conn,
        &key,
        &parsed.place,
        parsed.lat,
        parsed.lon,
        &context,
        &chrono::Utc::now().to_rfc3339(),
    )
    .await?;

    println!("✅ route_place_geocodes manual pin");
    println!("  place: {}", parsed.place);
    println!("  query_key: {key}");
    println!("  lat/lon: {}, {}", parsed.lat, parsed.lon);
    println!("  (next `snapshot-maps` run resolves this label from the cache; re-run it to pick the pin up)");

    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel set-place-geocode \"<place>\" --lat <f> --lon <f> (--dest <slug> | --context \"<ctx>\")\n  (manually pin a segment place label Nominatim cannot resolve; writes the exact cache key snapshot-maps reads)"
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut lat: Option<f64> = None;
    let mut lon: Option<f64> = None;
    let mut dest: Option<String> = None;
    let mut context: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--lat" => {
                let v = arg_value(args, i, "--lat")?;
                lat = Some(
                    v.parse()
                        .map_err(|_| format!("--lat must be a number (got \"{v}\")"))?,
                );
                i += 2;
            }
            "--lon" => {
                let v = arg_value(args, i, "--lon")?;
                lon = Some(
                    v.parse()
                        .map_err(|_| format!("--lon must be a number (got \"{v}\")"))?,
                );
                i += 2;
            }
            "--dest" => {
                dest = Some(arg_value(args, i, "--dest")?);
                i += 2;
            }
            "--context" => {
                context = Some(arg_value(args, i, "--context")?);
                i += 2;
            }
            "--plan-id" => {
                return Err(
                    "set-place-geocode is global reference/cache data and takes no --plan-id"
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

    if positional.is_empty() {
        return Err(format!("missing required <place>.\n{}", usage()));
    }
    let place = positional.join(" ");
    if place.trim().is_empty() {
        return Err("<place> cannot be empty".to_string());
    }
    let lat = lat.ok_or_else(|| format!("missing required --lat.\n{}", usage()))?;
    let lon = lon.ok_or_else(|| format!("missing required --lon.\n{}", usage()))?;
    let (dest, context) = match (dest, context) {
        (Some(_), Some(_)) => {
            return Err("pass either --dest or --context, not both".to_string());
        }
        (None, None) => {
            return Err(format!(
                "missing geocode context — pass --dest <slug> (derived from destination_config) or --context \"<ctx>\".\n{}",
                usage()
            ));
        }
        pair => pair,
    };

    Ok(ParsedArgs {
        place: place.trim().to_string(),
        lat,
        lon,
        dest,
        context,
    })
}

fn arg_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("missing value for {flag}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_matches_resolve_place_formula() {
        // Same shape as the row snapshot-maps reads: search lowercased, ctx as-is.
        assert_eq!(
            cache_key("京都國際會館", "Kyoto, Japan"),
            "京都國際會館|Kyoto, Japan"
        );
        assert_eq!(cache_key("Kifune Shrine", "Kyoto, Japan"), "kifune shrine|Kyoto, Japan");
    }

    #[test]
    fn cache_key_applies_place_aliases() {
        // normalize_place rewrites known aliases before keying — a manual pin
        // for 京都站 must land on the SAME key the renderer would look up.
        assert_eq!(
            cache_key("京都站", "Kyoto, Japan"),
            "kyoto station building|Kyoto, Japan"
        );
    }

    #[test]
    fn parse_args_full() {
        let p = parse_args(&[
            "京都國際會館".to_string(),
            "--lat".to_string(),
            "35.0629052".to_string(),
            "--lon".to_string(),
            "135.7852262".to_string(),
            "--context".to_string(),
            "Kyoto, Japan".to_string(),
        ])
        .unwrap();
        assert_eq!(p.place, "京都國際會館");
        assert!((p.lat - 35.0629052).abs() < 1e-9);
        assert!((p.lon - 135.7852262).abs() < 1e-9);
        assert_eq!(p.context.as_deref(), Some("Kyoto, Japan"));
    }

    #[test]
    fn parse_args_place_may_contain_spaces() {
        let p = parse_args(&[
            "京嵐山".to_string(),
            "にしき".to_string(),
            "--lat".to_string(),
            "1.0".to_string(),
            "--lon".to_string(),
            "2.0".to_string(),
            "--dest".to_string(),
            "osaka_kyoto_2026".to_string(),
        ])
        .unwrap();
        assert_eq!(p.place, "京嵐山 にしき");
        assert_eq!(p.dest.as_deref(), Some("osaka_kyoto_2026"));
    }

    #[test]
    fn parse_args_rejects_missing_coords_or_context() {
        let base = vec![
            "p".to_string(),
            "--lat".to_string(),
            "1.0".to_string(),
            "--lon".to_string(),
            "2.0".to_string(),
        ];
        assert!(parse_args(&base).is_err(), "no dest/context");
        assert!(
            parse_args(&{
                let mut v = base.clone();
                v.extend(["--dest".to_string(), "s".to_string(), "--context".to_string(), "c".to_string()]);
                v
            })
            .is_err(),
            "both dest and context"
        );
        assert!(
            parse_args(&[
                "p".to_string(),
                "--lon".to_string(),
                "2.0".to_string(),
                "--dest".to_string(),
                "s".to_string(),
            ])
            .is_err(),
            "missing lat"
        );
    }
}
