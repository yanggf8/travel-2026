// `travel delete-accommodation-image --id <accommodation_id> [--url <image_url>] [--all]`
// — remove one gallery photo (or every photo) of a domestic accommodation.
//
// Slug-keyed GLOBAL reference data — NO --plan-id, NO audit triad (same family as
// delete-accommodation). Fail loud when nothing matched (affected_row_count == 0),
// so a typo'd id/url is never a silent no-op.

use travel_db::repo::domestic_accommodation_images::{delete, delete_all_for};

#[derive(Debug)]
struct Args {
    id: String,
    url: Option<String>,
}

pub async fn run(raw: &[String]) -> Result<(), String> {
    if raw.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let args = parse_args(raw)?;

    let conn = crate::db::connect_write().await?;
    let affected = match &args.url {
        Some(u) => delete(&conn, &args.id, u).await?,
        None => delete_all_for(&conn, &args.id).await?,
    };

    if affected == 0 {
        return Err(match &args.url {
            Some(u) => format!(
                "Error: no gallery photo '{u}' on accommodation '{}' — list them via `travel list-accommodation-images --id {}`",
                args.id, args.id
            ),
            None => format!(
                "Error: accommodation '{}' has no gallery photos — nothing to delete",
                args.id
            ),
        });
    }

    match &args.url {
        Some(u) => println!("🗑️  Deleted gallery photo from {}\n  {u}", args.id),
        None => println!(
            "🗑️  Deleted all {affected} gallery photo(s) from {}",
            args.id
        ),
    }
    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel delete-accommodation-image --id <accommodation_id> --url <image_url>\n  \
     travel delete-accommodation-image --id <accommodation_id> --all\n  \
     (slug-keyed reference data — no --plan-id; fails loud when nothing matched)"
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut id: Option<String> = None;
    let mut url: Option<String> = None;
    let mut all = false;
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
            "--url" | "--image-url" | "--image" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--url cannot be empty".to_string());
                }
                url = Some(v);
                i += 2;
            }
            "--all" => {
                all = true;
                i += 1;
            }
            "--plan-id" => {
                return Err(
                    "no --plan-id here — domestic_accommodation_images is destination-scoped reference data \
                     (delete-accommodation-image is global/slug-keyed and takes no --plan-id)"
                        .to_string(),
                );
            }
            other if other.starts_with("--") => {
                return Err(format!(
                    "unknown flag for delete-accommodation-image: {other}"
                ));
            }
            other => return Err(format!("unexpected positional argument: {other}")),
        }
    }
    let id = id.ok_or_else(|| format!("--id <accommodation_id> is required.\n{}", usage()))?;
    match (url, all) {
        (Some(_), true) => Err("pass either --url or --all, not both".to_string()),
        (None, false) => Err(format!(
            "--url <image_url> is required (or --all to clear the gallery).\n{}",
            usage()
        )),
        (u, _) => Ok(Args { id, url: u }),
    }
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
    fn parses_single_url_delete() {
        let o = parse_args(&a(&["--id", "acc1", "--url", "https://img"])).unwrap();
        assert_eq!(o.id, "acc1");
        assert_eq!(o.url.as_deref(), Some("https://img"));
    }

    #[test]
    fn parses_all() {
        let o = parse_args(&a(&["--id", "acc1", "--all"])).unwrap();
        assert!(o.url.is_none(), "--all clears the whole gallery");
    }

    #[test]
    fn rejects_url_and_all_together() {
        let e = parse_args(&a(&["--id", "x", "--url", "u", "--all"])).unwrap_err();
        assert!(e.contains("not both"));
    }

    #[test]
    fn rejects_missing_target() {
        assert!(parse_args(&a(&["--url", "u"])).unwrap_err().contains("--id"));
        assert!(parse_args(&a(&["--id", "x"])).unwrap_err().contains("--url"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let e = parse_args(&a(&["--id", "x", "--all", "--bogus"])).unwrap_err();
        assert!(e.contains("unknown flag"));
    }

    #[test]
    fn rejects_plan_id() {
        let e = parse_args(&a(&["--id", "x", "--all", "--plan-id", "p"])).unwrap_err();
        assert!(e.contains("no --plan-id"));
    }
}
