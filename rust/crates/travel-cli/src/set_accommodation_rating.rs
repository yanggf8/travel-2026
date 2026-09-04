// `travel set-accommodation-rating --id <accommodation_id> --source <name>
//    --score <n> [--scale <n>] [--reviews <n>]`
// `travel set-accommodation-rating --id <accommodation_id> --source <name> --clear`
// — record ONE review source's guest rating for a domestic accommodation.
//
// Slug-keyed GLOBAL reference data — NO --plan-id, NO audit triad.
// One row per SOURCE by design: Booking.com scores out of 10 and Google out of 5,
// so the scale is stored with the score and the dashboard renders them separately.
// Averaging them would publish a number nobody actually rated.
//
// --scale defaults from the source name (booking/agoda → 10, google → 5) so the
// common cases need no flag; anything else must state its scale explicitly.

use travel_db::repo::domestic_accommodation_images::accommodation_exists;
use travel_db::repo::domestic_accommodation_ratings::{delete, list_by_accommodation, upsert};

#[derive(Debug)]
struct Args {
    id: String,
    source: String,
    score: f64,
    scale: f64,
    reviews: Option<i64>,
    clear: bool,
}

pub async fn run(raw: &[String]) -> Result<(), String> {
    if raw.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let args = parse_args(raw)?;

    let conn = crate::db::connect_write().await?;
    if !accommodation_exists(&conn, &args.id).await? {
        return Err(format!(
            "Error: no domestic_accommodations row with id '{}' — list ids via `travel list-accommodations --dest <slug>`",
            args.id
        ));
    }

    if args.clear {
        let affected = delete(&conn, &args.id, &args.source).await?;
        if affected == 0 {
            return Err(format!(
                "Error: {} has no '{}' rating to clear",
                args.id, args.source
            ));
        }
        println!("🗑️  Cleared {} rating for {}", args.source, args.id);
        return Ok(());
    }

    upsert(
        &conn,
        &args.id,
        &args.source,
        args.score,
        args.scale,
        args.reviews,
    )
    .await?;

    println!("✅ Rating recorded for {}", args.id);
    for r in list_by_accommodation(&conn, &args.id).await? {
        let reviews = match r.review_count {
            Some(n) => format!("{n} review(s)"),
            None => "count unknown".to_string(),
        };
        println!(
            "  {} {}/{} · {} · checked {}",
            r.source, r.score, r.scale, reviews, r.checked_at
        );
    }
    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel set-accommodation-rating --id <accommodation_id> --source <name> --score <n> [--scale <n>] [--reviews <n>]\n  \
     travel set-accommodation-rating --id <accommodation_id> --source <name> --clear\n  \
     (slug-keyed reference data — no --plan-id; one row per source, re-running a source overwrites it.\n  \
      --scale defaults to 10 for booking/agoda and 5 for google; state it for any other source.)"
}

/// Conventional max score for the well-known sources. `None` = caller must pass --scale.
fn default_scale(source: &str) -> Option<f64> {
    let s = source.to_ascii_lowercase();
    if s.contains("booking") || s.contains("agoda") {
        Some(10.0)
    } else if s.contains("google") {
        Some(5.0)
    } else {
        None
    }
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut id: Option<String> = None;
    let mut source: Option<String> = None;
    let mut score: Option<f64> = None;
    let mut scale: Option<f64> = None;
    let mut reviews: Option<i64> = None;
    let mut clear = false;
    let mut i = 0;
    while i < raw.len() {
        let k = raw[i].as_str();
        match k {
            "--id" | "--accommodation-id" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--id cannot be empty".to_string());
                }
                id = Some(v);
                i += 2;
            }
            "--source" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--source cannot be empty".to_string());
                }
                source = Some(v);
                i += 2;
            }
            "--score" => {
                let v = val(raw, i, k)?;
                let n: f64 = v
                    .parse()
                    .map_err(|_| "--score must be a number".to_string())?;
                if !(n > 0.0) {
                    return Err("--score must be > 0".to_string());
                }
                score = Some(n);
                i += 2;
            }
            "--scale" => {
                let v = val(raw, i, k)?;
                let n: f64 = v
                    .parse()
                    .map_err(|_| "--scale must be a number".to_string())?;
                if !(n > 0.0) {
                    return Err("--scale must be > 0".to_string());
                }
                scale = Some(n);
                i += 2;
            }
            "--reviews" | "--review-count" => {
                let v = val(raw, i, k)?;
                let n: i64 = v
                    .parse()
                    .map_err(|_| "--reviews must be an integer".to_string())?;
                if n < 0 {
                    return Err("--reviews must be >= 0".to_string());
                }
                reviews = Some(n);
                i += 2;
            }
            "--clear" => {
                clear = true;
                i += 1;
            }
            "--plan-id" => {
                return Err(
                    "no --plan-id here — domestic_accommodation_ratings is destination-scoped reference data \
                     (set-accommodation-rating is global/slug-keyed and takes no --plan-id)"
                        .to_string(),
                );
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown flag for set-accommodation-rating: {other}"));
            }
            other => return Err(format!("unexpected positional argument: {other}")),
        }
    }
    let id = id.ok_or_else(|| format!("--id <accommodation_id> is required.\n{}", usage()))?;
    let source = source.ok_or_else(|| format!("--source <name> is required.\n{}", usage()))?;
    if clear {
        return Ok(Args {
            id,
            source,
            score: 0.0,
            scale: 0.0,
            reviews: None,
            clear: true,
        });
    }
    let score = score.ok_or_else(|| format!("--score <n> is required.\n{}", usage()))?;
    let scale = match scale.or_else(|| default_scale(&source)) {
        Some(s) => s,
        None => {
            return Err(format!(
                "--scale <n> is required for source '{source}' (only booking/agoda and google have a known default)"
            ));
        }
    };
    if score > scale {
        return Err(format!("--score {score} cannot exceed --scale {scale}"));
    }
    Ok(Args {
        id,
        source,
        score,
        scale,
        reviews,
        clear: false,
    })
}

fn val(raw: &[String], i: usize, flag: &str) -> Result<String, String> {
    raw.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn booking_scale_defaults_to_ten() {
        let o = parse_args(&a(&[
            "--id", "x", "--source", "Booking.com", "--score", "9.0", "--reviews", "266",
        ]))
        .unwrap();
        assert_eq!(o.scale, 10.0);
        assert_eq!(o.reviews, Some(266));
    }

    #[test]
    fn google_scale_defaults_to_five() {
        let o = parse_args(&a(&["--id", "x", "--source", "Google 地圖", "--score", "4.6"]));
        // A non-ASCII source name still matches on the ASCII "google" substring.
        assert_eq!(o.unwrap().scale, 5.0);
    }

    #[test]
    fn unknown_source_requires_explicit_scale() {
        let e = parse_args(&a(&["--id", "x", "--source", "TripAdvisor", "--score", "4.5"]))
            .unwrap_err();
        assert!(e.contains("--scale"), "{e}");
        let o = parse_args(&a(&[
            "--id", "x", "--source", "TripAdvisor", "--score", "4.5", "--scale", "5",
        ]))
        .unwrap();
        assert_eq!(o.scale, 5.0);
    }

    #[test]
    fn rejects_score_above_scale() {
        let e = parse_args(&a(&["--id", "x", "--source", "google", "--score", "9.0"])).unwrap_err();
        assert!(e.contains("cannot exceed"), "{e}");
    }

    #[test]
    fn clear_needs_no_score() {
        let o = parse_args(&a(&["--id", "x", "--source", "google", "--clear"])).unwrap();
        assert!(o.clear);
    }

    #[test]
    fn rejects_missing_fields() {
        assert!(parse_args(&a(&["--source", "google", "--score", "4"]))
            .unwrap_err()
            .contains("--id"));
        assert!(parse_args(&a(&["--id", "x", "--score", "4"]))
            .unwrap_err()
            .contains("--source"));
        assert!(parse_args(&a(&["--id", "x", "--source", "google"]))
            .unwrap_err()
            .contains("--score"));
    }

    #[test]
    fn rejects_unknown_flag_and_plan_id() {
        assert!(parse_args(&a(&["--id", "x", "--source", "google", "--score", "4", "--bogus"]))
            .unwrap_err()
            .contains("unknown flag"));
        assert!(parse_args(&a(&["--id", "x", "--source", "google", "--score", "4", "--plan-id", "p"]))
            .unwrap_err()
            .contains("no --plan-id"));
    }
}
