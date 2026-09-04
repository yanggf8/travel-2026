// `travel delete-accommodation --id <id>`
// — delete one `domestic_accommodations` row by id.
//
// Slug-keyed GLOBAL reference data — NO --plan-id, NO audit triad (same family as
// add-transit / add-omiyage). Fail loud when the id does not exist
// (affected_row_count == 0). Parameterized DELETE via
// travel_db::repo::domestic_accommodations::delete_by_id.

use travel_db::repo::domestic_accommodations::delete_by_id;

pub async fn run(raw: &[String]) -> Result<(), String> {
    if raw.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let id = parse_args(raw)?;

    let conn = crate::db::connect_write().await?;
    let affected = delete_by_id(&conn, &id).await?;

    if affected == 0 {
        return Err(format!(
            "Error: no domestic_accommodations row with id '{id}' — list ids via `travel list-accommodations --dest <slug>`"
        ));
    }

    println!("✅ Deleted accommodation: {id}");
    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel delete-accommodation --id <id>\n  \
     (slug-keyed reference data — no --plan-id; removes one domestic_accommodations row)"
}

fn parse_args(raw: &[String]) -> Result<String, String> {
    let mut id: Option<String> = None;
    let mut i = 0;
    while i < raw.len() {
        let k = raw[i].as_str();
        match k {
            "--id" => {
                let v = raw
                    .get(i + 1)
                    .cloned()
                    .ok_or_else(|| "--id requires a value".to_string())?;
                if v.trim().is_empty() {
                    return Err("--id cannot be empty".to_string());
                }
                id = Some(v);
                i += 2;
            }
            "--plan-id" => {
                return Err(
                    "no --plan-id here — domestic_accommodations is destination-scoped reference data \
                     (delete-accommodation is global/slug-keyed and takes no --plan-id)"
                        .to_string(),
                );
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown flag for delete-accommodation: {other}"));
            }
            other => return Err(format!("unexpected positional argument: {other}")),
        }
    }
    id.ok_or_else(|| format!("--id <id> is required.\n{}", usage()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_id() {
        let id = parse_args(&a(&["--id", "jiufen_abc123"])).unwrap();
        assert_eq!(id, "jiufen_abc123");
    }

    #[test]
    fn rejects_missing_id() {
        let e = parse_args(&a(&[])).unwrap_err();
        assert!(e.contains("--id"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let e = parse_args(&a(&["--id", "x", "--bogus"])).unwrap_err();
        assert!(e.contains("unknown flag"));
    }

    #[test]
    fn rejects_plan_id() {
        let e = parse_args(&a(&["--id", "x", "--plan-id", "p"])).unwrap_err();
        assert!(e.contains("no --plan-id"));
    }
}
