// `travel set-clothing-tip <tip_id> --kind <kind> [--threshold N] --sort N --zh "<text>" --en "<text>"`
// — inserts or fully replaces one clothing-advice band in `clothing_tips`, the GLOBAL
// reference table the dashboard reads for the "what to wear" line under each day's
// weather. NOT plan-keyed: NO --plan-id, NO audit triad (same family as add-transit).
// Plain-text output: the written row, then every band in the order the worker reads them.
//
// How the worker evaluates the rows (render::day::clothing_tip):
//   1. temperature group — temp_low_max (low <= N) | temp_high_max (high <= N) |
//      temp_else (always; the catch-all, no threshold): FIRST match in sort order
//   2. swing_min (high - low >= N): first match
//   3. rain_min (rain% >= N): first match
// Texts may use {lo} / {hi} (the rounded feels-like low/high temperature).

use travel_db::repo::clothing_tips::{self, ClothingTip, KINDS, PLACEHOLDERS};

const VALUE_FLAGS: [&str; 5] = ["--kind", "--threshold", "--sort", "--zh", "--en"];

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let tip = parse_args(args)?;

    let conn = crate::db::connect_write().await?;
    let affected = clothing_tips::upsert(&conn, &tip).await?;
    if affected != 1 {
        return Err(format!(
            "clothing_tips upsert affected {affected} rows (expected 1) for {}",
            tip.tip_id
        ));
    }

    println!(
        "✅ clothing_tips upserted: {} ({} {}, sort {})",
        tip.tip_id,
        tip.kind,
        fmt_threshold(tip.threshold),
        tip.sort_order
    );
    println!("  zh: {}", tip.text_zh);
    println!("  en: {}", tip.text_en);
    println!();
    println!("All bands (worker read order; temperature group first-match, then swing_min, then rain_min):");
    println!("  sort\ttip_id\tkind\tthreshold\ttext_zh");
    for t in clothing_tips::list(&conn).await? {
        println!(
            "  {}\t{}\t{}\t{}\t{}",
            t.sort_order,
            t.tip_id,
            t.kind,
            fmt_threshold(t.threshold),
            t.text_zh
        );
    }
    Ok(())
}

fn usage() -> String {
    format!(
        "Usage:\n  travel set-clothing-tip <tip_id> --kind <{}> [--threshold N] --sort N --zh \"<text>\" --en \"<text>\"\n  \
         (GLOBAL reference data — no --plan-id; insert-or-replace by tip_id. --threshold is required \
         for every kind except temp_else. Texts may use {{lo}} / {{hi}}.)",
        KINDS.join("|")
    )
}

fn fmt_threshold(t: Option<f64>) -> String {
    t.map(|v| v.to_string()).unwrap_or_else(|| "—".to_string())
}

fn parse_args(args: &[String]) -> Result<ClothingTip, String> {
    if args.iter().any(|a| a == "--plan-id") {
        return Err(
            "set-clothing-tip is global reference data and takes no --plan-id \
             (the bands apply to every plan's dashboard)"
                .to_string(),
        );
    }
    crate::plan_resolver::reject_unknown_flags(args, &VALUE_FLAGS, &[])?;

    let mut kind = None;
    let mut threshold = None;
    let mut sort = None;
    let mut zh = None;
    let mut en = None;
    let mut positional: Vec<&String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if VALUE_FLAGS.contains(&a) {
            let v = args
                .get(i + 1)
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))?;
            match a {
                "--kind" => kind = Some(v),
                "--threshold" => threshold = Some(v),
                "--sort" => sort = Some(v),
                "--zh" => zh = Some(v),
                _ => en = Some(v),
            }
            i += 2;
        } else {
            positional.push(&args[i]);
            i += 1;
        }
    }

    let tip_id = match positional.as_slice() {
        [id] => id.to_string(),
        [] => return Err(format!("missing <tip_id>.\n{}", usage())),
        more => {
            return Err(format!(
                "expected one <tip_id>, got {} positionals ({}) — quote multi-word texts",
                more.len(),
                more.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ))
        }
    };
    if tip_id.is_empty()
        || !tip_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "<tip_id> must be non-empty ASCII letters/digits/_/- (got \"{tip_id}\")"
        ));
    }

    let kind = kind.ok_or("--kind is required")?;
    if !KINDS.contains(&kind.as_str()) {
        return Err(format!("--kind must be one of {} (got \"{kind}\")", KINDS.join("|")));
    }
    let threshold = match (kind.as_str(), threshold) {
        ("temp_else", None) => None,
        ("temp_else", Some(_)) => {
            return Err("--kind temp_else is the catch-all band and takes no --threshold".to_string())
        }
        (_, None) => return Err(format!("--threshold N is required for --kind {kind}")),
        (_, Some(v)) => {
            let n: f64 = v
                .parse()
                .map_err(|_| format!("--threshold must be a number (got \"{v}\")"))?;
            if !n.is_finite() {
                return Err(format!("--threshold must be finite (got \"{v}\")"));
            }
            Some(n)
        }
    };
    let sort = sort.ok_or("--sort N is required")?;
    let sort_order: i64 = sort
        .parse()
        .map_err(|_| format!("--sort must be an integer (got \"{sort}\")"))?;
    let text_zh = zh.ok_or("--zh \"<text>\" is required")?;
    let text_en = en.ok_or("--en \"<text>\" is required")?;
    check_text("--zh", &text_zh)?;
    check_text("--en", &text_en)?;

    Ok(ClothingTip {
        tip_id,
        kind,
        threshold,
        sort_order,
        text_zh,
        text_en,
    })
}

/// Non-empty, and every `{…}` is a placeholder the worker fills — a typo such as
/// `{low}` would otherwise be published verbatim.
fn check_text(flag: &str, text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err(format!("{flag} text cannot be empty"));
    }
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        if rest[..open].contains('}') {
            return Err(format!("{flag} has a stray '}}' in \"{text}\""));
        }
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| format!("{flag} has an unclosed '{{' in \"{text}\""))?;
        let name = &after[..close];
        if !PLACEHOLDERS.contains(&name) {
            return Err(format!(
                "{flag} uses unknown placeholder {{{name}}} — only {{lo}} / {{hi}} are filled"
            ));
        }
        rest = &after[close + 1..];
    }
    if rest.contains('}') {
        return Err(format!("{flag} has a stray '}}' in \"{text}\""));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_threshold_band() {
        let t = parse_args(&a(&[
            "low-5", "--kind", "temp_low_max", "--threshold", "5", "--sort", "20", "--zh",
            "🧥 早晚{lo}°C", "--en", "🧥 {lo}°C",
        ]))
        .unwrap();
        assert_eq!(t.tip_id, "low-5");
        assert_eq!(t.kind, "temp_low_max");
        assert_eq!(t.threshold, Some(5.0));
        assert_eq!(t.sort_order, 20);
        assert_eq!(t.text_zh, "🧥 早晚{lo}°C");
    }

    #[test]
    fn parses_negative_threshold_and_else_band() {
        let t = parse_args(&a(&[
            "x", "--kind", "temp_low_max", "--threshold", "-2.5", "--sort", "1", "--zh", "z",
            "--en", "e",
        ]))
        .unwrap();
        assert_eq!(t.threshold, Some(-2.5));
        let e = parse_args(&a(&[
            "hot", "--kind", "temp_else", "--sort", "70", "--zh", "炎熱{hi}", "--en", "Hot {hi}",
        ]))
        .unwrap();
        assert_eq!(e.threshold, None);
    }

    #[test]
    fn rejects_unknown_flag_and_plan_id() {
        let e = parse_args(&a(&["x", "--kind", "rain_min", "--thresh", "3"])).unwrap_err();
        assert!(e.contains("unknown argument: --thresh"), "{e}");
        let e = parse_args(&a(&["x", "--plan-id", "p"])).unwrap_err();
        assert!(e.contains("no --plan-id"), "{e}");
    }

    #[test]
    fn rejects_bad_kind_and_threshold_rules() {
        let base = |extra: &[&str]| {
            let mut v = vec!["x", "--sort", "1", "--zh", "z", "--en", "e"];
            v.extend_from_slice(extra);
            parse_args(&a(&v))
        };
        assert!(base(&["--kind", "snow"]).unwrap_err().contains("--kind must be one of"));
        assert!(base(&["--kind", "rain_min"]).unwrap_err().contains("--threshold N is required"));
        assert!(base(&["--kind", "temp_else", "--threshold", "3"])
            .unwrap_err()
            .contains("takes no --threshold"));
        assert!(base(&["--kind", "rain_min", "--threshold", "lots"])
            .unwrap_err()
            .contains("must be a number"));
        assert!(base(&["--kind", "rain_min", "--threshold", "NaN"])
            .unwrap_err()
            .contains("finite"));
    }

    #[test]
    fn rejects_missing_parts() {
        assert!(parse_args(&a(&["--kind", "temp_else", "--sort", "1", "--zh", "z", "--en", "e"]))
            .unwrap_err()
            .contains("missing <tip_id>"));
        assert!(parse_args(&a(&["x", "y", "--kind", "temp_else", "--sort", "1", "--zh", "z", "--en", "e"]))
            .unwrap_err()
            .contains("expected one <tip_id>"));
        assert!(parse_args(&a(&["x", "--kind", "temp_else", "--zh", "z", "--en", "e"]))
            .unwrap_err()
            .contains("--sort"));
        assert!(parse_args(&a(&["x", "--kind", "temp_else", "--sort", "1", "--en", "e"]))
            .unwrap_err()
            .contains("--zh"));
        assert!(parse_args(&a(&["bad id", "--kind", "temp_else", "--sort", "1", "--zh", "z", "--en", "e"]))
            .unwrap_err()
            .contains("<tip_id>"));
    }

    #[test]
    fn rejects_unknown_placeholder_and_stray_braces() {
        assert!(check_text("--zh", "早晚{low}°C").unwrap_err().contains("{low}"));
        assert!(check_text("--zh", "早晚{lo°C").unwrap_err().contains("unclosed"));
        assert!(check_text("--en", "x } y").unwrap_err().contains("stray"));
        assert!(check_text("--zh", "氣溫 } 炎熱 {hi}°C").unwrap_err().contains("stray"));
        assert!(check_text("--en", "   ").unwrap_err().contains("empty"));
        assert!(check_text("--en", "{lo}–{hi}°C").is_ok());
    }
}
