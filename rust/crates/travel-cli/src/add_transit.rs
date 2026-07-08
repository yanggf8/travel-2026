// `travel add-transit <slug> <from_station> <to_station> --minutes N [--line "<text>"]
//   [--kind metro|rail|walk|bus|estimate] [--source <url|provider>]
//   [--confidence verified|reviewed|estimate]`
// — adds/updates one `destination_transit` row (station-pair transit metadata that
// `derive-routes` attaches to auto-derived route legs). Slug-keyed GLOBAL reference
// data, NOT plan-keyed: NO audit triad (no plan_events/operation_runs/plans.version),
// mirroring set-poi-coords. Plain-text output only.
//
// The pair_key is `transit_key::primary_pair_key(from, to)` — the SAME normalization
// derive-routes looks up by — so a pair added here is found by the very next
// derive-routes run (the whole point: no more raw `db exec INSERT` for discovered pairs).
//
// Flow: parse (fail loud on bad minutes/kind/confidence or from==to) → connect_write →
// destination_ref::upsert_transit (INSERT OR REPLACE, idempotent) → assert rows==1.

use travel_db::repo::destination_ref;

#[derive(Debug, Default)]
struct ParsedArgs {
    slug: String,
    from_station: String,
    to_station: String,
    minutes: i64,
    line: String,
    kind: String,
    source: Option<String>,
    confidence: String,
}

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }

    let parsed = parse_args(args)?;
    let pair_key = crate::transit_key::primary_pair_key(&parsed.from_station, &parsed.to_station);
    let fetched_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    let conn = crate::db::connect_write().await?;
    let affected = destination_ref::upsert_transit(
        &conn,
        &parsed.slug,
        &pair_key,
        &parsed.kind,
        parsed.minutes,
        &parsed.line,
        &parsed.from_station,
        &parsed.to_station,
        parsed.source.as_deref(),
        &parsed.confidence,
        &fetched_at,
    )
    .await?;

    if affected != 1 {
        return Err(format!(
            "destination_transit upsert affected {affected} rows (expected 1) for {pair_key}"
        ));
    }

    println!(
        "✅ destination_transit added: {} → {} ({} min) [{}]",
        parsed.from_station, parsed.to_station, parsed.minutes, parsed.kind
    );
    println!("  slug: {}  pair_key: {pair_key}", parsed.slug);
    if !parsed.line.is_empty() {
        println!("  line: {}", parsed.line);
    }
    println!("  confidence: {}", parsed.confidence);
    println!("Next: re-run `derive-routes` — this station pair's minutes now attach to any derived leg that uses it.");
    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel add-transit <slug> <from_station> <to_station> --minutes N \
     [--line \"<text>\"] [--kind metro|rail|walk|bus|estimate] [--source <url|provider>] \
     [--confidence verified|reviewed|estimate]\n  \
     (slug-keyed reference data — no --plan-id; pair is found by the next derive-routes run)"
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut minutes: Option<i64> = None;
    let mut line = String::new();
    let mut kind = "estimate".to_string();
    let mut source: Option<String> = None;
    let mut confidence = "estimate".to_string();
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--minutes" => {
                let v = arg_value(args, i, "--minutes")?;
                let n: i64 = v
                    .parse()
                    .map_err(|_| format!("--minutes must be an integer (got \"{v}\")"))?;
                if n < 0 {
                    return Err(format!("--minutes must be >= 0 (got {n})"));
                }
                minutes = Some(n);
                i += 2;
            }
            "--line" => {
                line = arg_value(args, i, "--line")?;
                i += 2;
            }
            "--kind" => {
                kind = arg_value(args, i, "--kind")?;
                i += 2;
            }
            "--source" => {
                source = Some(arg_value(args, i, "--source")?);
                i += 2;
            }
            "--confidence" => {
                confidence = arg_value(args, i, "--confidence")?;
                i += 2;
            }
            "--plan-id" => {
                return Err(
                    "add-transit is global/slug-keyed reference data and takes no --plan-id \
                     (a station pair's transit time is shared across every plan using the destination)"
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
    let from_station = positional[1].clone();
    let to_station = positional[2].clone();
    if from_station.trim().is_empty() || to_station.trim().is_empty() {
        return Err("<from_station> and <to_station> cannot be empty".to_string());
    }
    // Same normalized station on both sides = no leg (nothing to time).
    if crate::transit_key::norm_station(&from_station) == crate::transit_key::norm_station(&to_station) {
        return Err(format!(
            "<from_station> and <to_station> normalize to the same station (\"{}\") — no transit leg",
            crate::transit_key::norm_station(&from_station)
        ));
    }

    let minutes = minutes.ok_or("--minutes <N> is required")?;

    const KINDS: [&str; 5] = ["metro", "rail", "walk", "bus", "estimate"];
    if !KINDS.contains(&kind.as_str()) {
        return Err(format!("--kind must be one of {KINDS:?} (got \"{kind}\")"));
    }
    const CONFS: [&str; 3] = ["verified", "reviewed", "estimate"];
    if !CONFS.contains(&confidence.as_str()) {
        return Err(format!("--confidence must be one of {CONFS:?} (got \"{confidence}\")"));
    }

    Ok(ParsedArgs {
        slug,
        from_station,
        to_station,
        minutes,
        line,
        kind,
        source,
        confidence,
    })
}

fn arg_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_minimal_valid() {
        let p = parse_args(&a(&["tokyo_2026", "Shibuya", "Harajuku", "--minutes", "3"])).unwrap();
        assert_eq!(p.slug, "tokyo_2026");
        assert_eq!(p.minutes, 3);
        assert_eq!(p.kind, "estimate");
        assert_eq!(p.confidence, "estimate");
    }

    #[test]
    fn rejects_unknown_flag() {
        let e = parse_args(&a(&["s", "A", "B", "--minutes", "3", "--bogus"]));
        assert!(e.unwrap_err().contains("unknown argument: --bogus"));
    }

    #[test]
    fn rejects_missing_minutes() {
        assert!(parse_args(&a(&["s", "A", "B"])).unwrap_err().contains("--minutes"));
    }

    #[test]
    fn rejects_non_integer_minutes() {
        assert!(parse_args(&a(&["s", "A", "B", "--minutes", "ten"]))
            .unwrap_err()
            .contains("integer"));
    }

    #[test]
    fn rejects_negative_minutes() {
        assert!(parse_args(&a(&["s", "A", "B", "--minutes", "-5"]))
            .unwrap_err()
            .contains(">= 0"));
    }

    #[test]
    fn rejects_same_station() {
        // Normalized equal (case/space-insensitive) → no leg.
        assert!(parse_args(&a(&["s", "Shibuya", "  shibuya ", "--minutes", "3"]))
            .unwrap_err()
            .contains("same station"));
    }

    #[test]
    fn rejects_bad_kind() {
        assert!(parse_args(&a(&["s", "A", "B", "--minutes", "3", "--kind", "teleport"]))
            .unwrap_err()
            .contains("--kind"));
    }

    #[test]
    fn rejects_bad_confidence() {
        assert!(parse_args(&a(&["s", "A", "B", "--minutes", "3", "--confidence", "sure"]))
            .unwrap_err()
            .contains("--confidence"));
    }

    #[test]
    fn rejects_plan_id() {
        assert!(parse_args(&a(&["s", "A", "B", "--minutes", "3", "--plan-id", "x"]))
            .unwrap_err()
            .contains("no --plan-id"));
    }
}
