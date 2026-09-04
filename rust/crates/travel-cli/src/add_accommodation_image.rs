// `travel add-accommodation-image --id <accommodation_id> --url <image_url> [--label <text>] [--sort N]`
// — add one gallery photo to a `domestic_accommodations` stay.
//
// Slug-keyed GLOBAL reference data — NO --plan-id, NO audit triad (same family as
// add-accommodation / add-transit / add-omiyage). The accommodation id is validated
// (fail loud when it does not exist). PK is (accommodation_id, image_url), so
// re-adding the same photo is a natural dedup, surfaced as "already exists".
// Without --sort the photo goes to the end (MAX(sort_order) + 1).

use travel_db::repo::domestic_accommodation_images::{
    accommodation_exists, insert, next_sort_order,
};

#[derive(Debug)]
struct Args {
    id: String,
    url: String,
    label: String,
    sort: Option<i64>,
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

    let sort = match args.sort {
        Some(n) => n,
        None => next_sort_order(&conn, &args.id).await?,
    };
    let affected = insert(&conn, &args.id, &args.url, &args.label, sort).await?;
    if affected == 0 {
        println!(
            "Gallery photo already exists for {} — nothing added.\n  {}",
            args.id, args.url
        );
        return Ok(());
    }

    println!("✅ Added gallery photo to {}", args.id);
    println!("  url:   {}", args.url);
    println!(
        "  label: {}",
        if args.label.is_empty() {
            "(none)"
        } else {
            &args.label
        }
    );
    println!("  sort:  {sort}");
    println!(
        "Next: review with `travel list-accommodation-images --id {}`.",
        args.id
    );
    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel add-accommodation-image --id <accommodation_id> --url <image_url> [--label <text>] [--sort N]\n  \
     (slug-keyed reference data — no --plan-id; idempotent on the same id+url, appends by default)"
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut id: Option<String> = None;
    let mut url: Option<String> = None;
    let mut label = String::new();
    let mut sort: Option<i64> = None;
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
            "--label" | "--caption" => {
                label = val(raw, i, k)?;
                i += 2;
            }
            "--sort" | "--sort-order" => {
                let v = val(raw, i, k)?;
                let n: i64 = v
                    .parse()
                    .map_err(|_| "--sort must be an integer".to_string())?;
                if n < 0 {
                    return Err("--sort must be >= 0".to_string());
                }
                sort = Some(n);
                i += 2;
            }
            "--plan-id" => {
                return Err(
                    "no --plan-id here — domestic_accommodation_images is destination-scoped reference data \
                     (add-accommodation-image is global/slug-keyed and takes no --plan-id)"
                        .to_string(),
                );
            }
            other if other.starts_with("--") => {
                return Err(format!(
                    "unknown flag for add-accommodation-image: {other}"
                ));
            }
            other => return Err(format!("unexpected positional argument: {other}")),
        }
    }
    let id = id.ok_or_else(|| format!("--id <accommodation_id> is required.\n{}", usage()))?;
    let url = url.ok_or_else(|| format!("--url <image_url> is required.\n{}", usage()))?;
    Ok(Args {
        id,
        url,
        label,
        sort,
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
    fn parses_required_fields() {
        let o = parse_args(&a(&["--id", "acc1", "--url", "https://img"])).unwrap();
        assert_eq!(o.id, "acc1");
        assert_eq!(o.url, "https://img");
        assert!(o.label.is_empty());
        assert!(o.sort.is_none(), "no --sort means append");
    }

    #[test]
    fn parses_label_and_sort() {
        let o = parse_args(&a(&[
            "--id", "acc1", "--url", "https://img", "--label", "海景四人房", "--sort", "3",
        ]))
        .unwrap();
        assert_eq!(o.label, "海景四人房");
        assert_eq!(o.sort, Some(3));
    }

    #[test]
    fn rejects_missing_fields() {
        assert!(parse_args(&a(&["--url", "u"])).unwrap_err().contains("--id"));
        assert!(parse_args(&a(&["--id", "x"])).unwrap_err().contains("--url"));
    }

    #[test]
    fn rejects_bad_sort() {
        assert!(parse_args(&a(&["--id", "x", "--url", "u", "--sort", "n"]))
            .unwrap_err()
            .contains("--sort"));
        assert!(parse_args(&a(&["--id", "x", "--url", "u", "--sort", "-1"]))
            .unwrap_err()
            .contains(">= 0"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let e = parse_args(&a(&["--id", "x", "--url", "u", "--bogus"])).unwrap_err();
        assert!(e.contains("unknown flag"));
    }

    #[test]
    fn rejects_plan_id() {
        let e = parse_args(&a(&["--id", "x", "--url", "u", "--plan-id", "p"])).unwrap_err();
        assert!(e.contains("no --plan-id"));
    }
}
