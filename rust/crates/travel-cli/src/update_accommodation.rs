// `travel update-accommodation --id <id> [--image-url <url>] [--booking-url <url>]`
// — update one `domestic_accommodations` row's image_url / booking_url.
//
// Slug-keyed GLOBAL reference data — NO --plan-id, NO audit triad (same family as
// add-transit / add-omiyage). At least one of --image-url / --booking-url is
// required. Fail loud when the id does not exist (affected_row_count == 0).
// Parameterized UPDATE via travel_db::repo::domestic_accommodations::update_fields.

use travel_db::repo::domestic_accommodations::update_fields;

#[derive(Debug)]
struct Args {
    id: String,
    image_url: Option<String>,
    booking_url: Option<String>,
}

pub async fn run(raw: &[String]) -> Result<(), String> {
    if raw.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let args = parse_args(raw)?;

    let conn = crate::db::connect_write().await?;
    let affected = update_fields(
        &conn,
        &args.id,
        args.image_url.as_deref(),
        args.booking_url.as_deref(),
    )
    .await?;

    if affected == 0 {
        return Err(format!(
            "Error: no domestic_accommodations row with id '{}' — list ids via `travel list-accommodations --dest <slug>`",
            args.id
        ));
    }

    println!("✅ Updated accommodation: {}", args.id);
    if let Some(u) = &args.image_url {
        println!("  image_url: {u}");
    }
    if let Some(u) = &args.booking_url {
        println!("  booking_url: {u}");
    }
    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel update-accommodation --id <id> [--image-url <url>] [--booking-url <url>]\n  \
     (slug-keyed reference data — no --plan-id; at least one of --image-url/--booking-url is required)"
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut id: Option<String> = None;
    let mut image_url: Option<String> = None;
    let mut booking_url: Option<String> = None;
    let mut i = 0;
    while i < raw.len() {
        let k = raw[i].as_str();
        match k {
            "--id" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--id cannot be empty".to_string());
                }
                id = Some(v);
                i += 2;
            }
            "--image-url" | "--image" => {
                image_url = Some(val(raw, i, k)?);
                i += 2;
            }
            "--booking-url" | "--booking" => {
                booking_url = Some(val(raw, i, k)?);
                i += 2;
            }
            "--plan-id" => {
                return Err(
                    "no --plan-id here — domestic_accommodations is destination-scoped reference data \
                     (update-accommodation is global/slug-keyed and takes no --plan-id)"
                        .to_string(),
                );
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown flag for update-accommodation: {other}"));
            }
            other => return Err(format!("unexpected positional argument: {other}")),
        }
    }
    let id = id.ok_or_else(|| format!("--id <id> is required.\n{}", usage()))?;
    if image_url.is_none() && booking_url.is_none() {
        return Err(format!(
            "at least one of --image-url / --booking-url is required.\n{}",
            usage()
        ));
    }
    Ok(Args {
        id,
        image_url,
        booking_url,
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
    fn parses_image_only() {
        let o = parse_args(&a(&["--id", "x1", "--image-url", "https://img"])).unwrap();
        assert_eq!(o.id, "x1");
        assert_eq!(o.image_url.as_deref(), Some("https://img"));
        assert!(o.booking_url.is_none());
    }

    #[test]
    fn parses_both() {
        let o = parse_args(&a(&[
            "--id", "x1", "--image-url", "https://img", "--booking-url", "https://book",
        ]))
        .unwrap();
        assert_eq!(o.booking_url.as_deref(), Some("https://book"));
    }

    #[test]
    fn rejects_missing_id() {
        let e = parse_args(&a(&["--image-url", "https://img"])).unwrap_err();
        assert!(e.contains("--id"));
    }

    #[test]
    fn rejects_no_fields() {
        let e = parse_args(&a(&["--id", "x1"])).unwrap_err();
        assert!(e.contains("at least one"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let e = parse_args(&a(&["--id", "x1", "--image-url", "u", "--bogus"])).unwrap_err();
        assert!(e.contains("unknown flag"));
    }

    #[test]
    fn rejects_plan_id() {
        let e = parse_args(&a(&["--id", "x1", "--image-url", "u", "--plan-id", "p"])).unwrap_err();
        assert!(e.contains("no --plan-id"));
    }
}
