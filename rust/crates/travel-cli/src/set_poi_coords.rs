// `travel set-poi-coords <slug> <poi_id> <lat> <lon> [--source <url|provider>] [--confidence verified|reviewed]`
// — sets geocoded lat/lon on one `destination_pois` row (map pins for the
// dashboard). Destination-ref data is slug-keyed GLOBAL data, NOT plan-keyed:
// NO audit triad (no `plan_events`/`operation_runs`/`plans.version` — mirrors
// the rest of the destination_ref DAL). Plain-text output only.
//
// Flow:
//   1. Parse args (fail loud on malformed lat/lon or a bad --confidence value).
//   2. Fail loud unless (slug, poi_id) already exists in destination_pois.
//   3. UPDATE destination_pois SET lat/lon + COALESCE(source_url)/COALESCE(confidence);
//      require rows_affected == 1.

use travel_db::repo::destination_ref;

#[derive(Debug, Default)]
struct ParsedArgs {
    slug: String,
    poi_id: String,
    lat: f64,
    lon: f64,
    source: Option<String>,
    confidence: Option<String>,
}

/// CLI entry: `travel set-poi-coords <slug> <poi_id> <lat> <lon> [--source <s>] [--confidence <c>]`.
pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }

    let parsed = parse_args(args)?;

    let conn = crate::db::connect_write().await?;

    if !destination_ref::poi_coords_exists(&conn, &parsed.slug, &parsed.poi_id).await? {
        return Err(format!(
            "unknown (slug, poi_id) = ({}, {}) — no row in destination_pois",
            parsed.slug, parsed.poi_id
        ));
    }

    let affected = destination_ref::set_poi_coords(
        &conn,
        &parsed.slug,
        &parsed.poi_id,
        parsed.lat,
        parsed.lon,
        parsed.source.as_deref(),
        parsed.confidence.as_deref(),
    )
    .await?;

    if affected != 1 {
        return Err(format!(
            "destination_pois UPDATE affected {affected} rows (expected 1) for ({}, {})",
            parsed.slug, parsed.poi_id
        ));
    }

    println!("destination_pois coords set: {} / {}", parsed.slug, parsed.poi_id);
    println!("  lat: {}", parsed.lat);
    println!("  lon: {}", parsed.lon);
    if let Some(ref s) = parsed.source {
        println!("  source: {s}");
    }
    if let Some(ref c) = parsed.confidence {
        println!("  confidence: {c}");
    }

    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel set-poi-coords <slug> <poi_id> <lat> <lon> [--source <url|provider>] [--confidence verified|reviewed]"
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut source: Option<String> = None;
    let mut confidence: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--source" => {
                source = Some(arg_value(args, i, "--source")?);
                i += 2;
            }
            "--confidence" => {
                confidence = Some(arg_value(args, i, "--confidence")?);
                i += 2;
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

    if positional.len() < 4 {
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
    let lat: f64 = positional[2]
        .parse()
        .map_err(|_| format!("<lat> must be a number (got \"{}\")", positional[2]))?;
    let lon: f64 = positional[3]
        .parse()
        .map_err(|_| format!("<lon> must be a number (got \"{}\")", positional[3]))?;

    if let Some(ref c) = confidence {
        if c != "verified" && c != "reviewed" {
            return Err(format!("--confidence must be verified|reviewed (got \"{c}\")"));
        }
    }

    Ok(ParsedArgs {
        slug,
        poi_id,
        lat,
        lon,
        source,
        confidence,
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
    fn parse_args_minimal() {
        let p = parse_args(&[
            "okinawa_2026".to_string(),
            "poi_one".to_string(),
            "35.6812".to_string(),
            "139.7671".to_string(),
        ])
        .unwrap();
        assert_eq!(p.slug, "okinawa_2026");
        assert_eq!(p.poi_id, "poi_one");
        assert!((p.lat - 35.6812).abs() < 1e-9);
        assert!((p.lon - 139.7671).abs() < 1e-9);
        assert!(p.source.is_none());
        assert!(p.confidence.is_none());
    }

    #[test]
    fn parse_args_with_source_and_confidence() {
        let p = parse_args(&[
            "okinawa_2026".to_string(),
            "poi_one".to_string(),
            "35.6812".to_string(),
            "139.7671".to_string(),
            "--source".to_string(),
            "test-provider".to_string(),
            "--confidence".to_string(),
            "verified".to_string(),
        ])
        .unwrap();
        assert_eq!(p.source.as_deref(), Some("test-provider"));
        assert_eq!(p.confidence.as_deref(), Some("verified"));
    }

    #[test]
    fn parse_args_rejects_bad_confidence() {
        assert!(parse_args(&[
            "s".to_string(),
            "p".to_string(),
            "1.0".to_string(),
            "2.0".to_string(),
            "--confidence".to_string(),
            "bogus".to_string(),
        ])
        .is_err());
    }

    #[test]
    fn parse_args_rejects_bad_lat() {
        assert!(parse_args(&[
            "s".to_string(),
            "p".to_string(),
            "notanumber".to_string(),
            "2.0".to_string(),
        ])
        .is_err());
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
            "1.0".to_string(),
            "2.0".to_string(),
            "--bogus".to_string(),
            "x".to_string(),
        ])
        .is_err());
    }
}
