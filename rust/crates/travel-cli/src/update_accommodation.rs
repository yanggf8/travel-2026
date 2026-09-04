// `travel update-accommodation --id <id> [--image-url u] [--booking-url u]
//   [--price N] [--room-type t] [--breakfast yes|no]
//   [--room-size N] [--price-source s] [--price-checked YYYY-MM-DD]
//   [--free-cancel-until YYYY-MM-DD] [--rooms-left N]`
// — update one `domestic_accommodations` row's optional decision facts.
//
// Slug-keyed GLOBAL reference data — NO --plan-id, NO audit triad (same family as
// add-transit / add-omiyage). At least one field is required. Fail loud when the id
// does not exist (affected_row_count == 0).
//
// `--price-source` implies `--price-checked` = today unless one is given: a rate
// quoted from an OTA is only meaningful with the date it was read, and the dashboard
// renders that date beside the price.

use libsql::Value;
use travel_db::repo::domestic_accommodations::update_fields;

#[derive(Debug, Default)]
struct Args {
    id: String,
    image_url: Option<String>,
    booking_url: Option<String>,
    room_size: Option<i64>,
    price_source: Option<String>,
    price_checked: Option<String>,
    free_cancel_until: Option<String>,
    rooms_left: Option<i64>,
    price: Option<i64>,
    room_type: Option<String>,
    /// Tri-state: None = leave alone, Some(true/false) = set.
    breakfast: Option<bool>,
}

impl Args {
    /// Ordered (column, value) pairs for the UPDATE. Column names are literals
    /// owned here — only values are bound.
    fn sets(&self) -> Vec<(&'static str, Value)> {
        let mut out: Vec<(&'static str, Value)> = Vec::new();
        let mut text = |col: &'static str, v: &Option<String>, out: &mut Vec<(&'static str, Value)>| {
            if let Some(s) = v {
                out.push((col, Value::Text(s.clone())));
            }
        };
        text("image_url", &self.image_url, &mut out);
        text("booking_url", &self.booking_url, &mut out);
        text("price_source", &self.price_source, &mut out);
        text("price_checked_at", &self.price_checked, &mut out);
        text("free_cancel_until", &self.free_cancel_until, &mut out);
        text("room_type", &self.room_type, &mut out);
        if let Some(n) = self.price {
            out.push(("price_twd", Value::Integer(n)));
        }
        if let Some(b) = self.breakfast {
            out.push(("breakfast_included", Value::Integer(i64::from(b))));
        }
        if let Some(n) = self.room_size {
            out.push(("room_size_sqm", Value::Integer(n)));
        }
        if let Some(n) = self.rooms_left {
            out.push(("rooms_left", Value::Integer(n)));
        }
        out
    }
}

pub async fn run(raw: &[String]) -> Result<(), String> {
    if raw.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let args = parse_args(raw)?;
    let sets = args.sets();

    let conn = crate::db::connect_write().await?;
    let affected = update_fields(&conn, &args.id, &sets).await?;

    if affected == 0 {
        return Err(format!(
            "Error: no domestic_accommodations row with id '{}' — list ids via `travel list-accommodations --dest <slug>`",
            args.id
        ));
    }

    println!("✅ Updated accommodation: {}", args.id);
    for (col, val) in &sets {
        let shown = match val {
            Value::Text(s) => s.clone(),
            Value::Integer(n) => n.to_string(),
            other => format!("{other:?}"),
        };
        println!("  {col}: {shown}");
    }
    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel update-accommodation --id <id> [--image-url <url>] [--booking-url <url>] \
     [--price <twd>] [--room-type <type>] [--breakfast yes|no] \
     [--room-size <sqm>] [--price-source <name>] [--price-checked <YYYY-MM-DD>] \
     [--free-cancel-until <YYYY-MM-DD>] [--rooms-left <n>]\n  \
     (slug-keyed reference data — no --plan-id; at least one field is required.\n  \
      --price-source without --price-checked stamps today, so a published rate always carries its read date.)"
}

/// `YYYY-MM-DD` shape check — a malformed date would render as garbage on the page.
fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

fn today() -> String {
    // Local date without pulling in chrono: the CLI already links time via libsql,
    // but a plain UTC date is enough for a "checked on" stamp.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    civil_from_days(secs.div_euclid(86_400))
}

/// Howard Hinnant's days-from-civil, inverted. Pure — unit-tested.
fn civil_from_days(z: i64) -> String {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut id: Option<String> = None;
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
                a.image_url = Some(val(raw, i, k)?);
                i += 2;
            }
            "--booking-url" | "--booking" => {
                a.booking_url = Some(val(raw, i, k)?);
                i += 2;
            }
            "--room-size" | "--sqm" => {
                a.room_size = Some(int(raw, i, k, 1)?);
                i += 2;
            }
            "--rooms-left" => {
                a.rooms_left = Some(int(raw, i, k, 0)?);
                i += 2;
            }
            // A re-checked rate is an UPDATE, not a new row — the id is opaque and
            // deliberately not re-derived from the new price.
            "--price" => {
                a.price = Some(int(raw, i, k, 1)?);
                i += 2;
            }
            "--room-type" | "--room_type" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--room-type cannot be empty".to_string());
                }
                a.room_type = Some(v);
                i += 2;
            }
            "--breakfast" => {
                let v = val(raw, i, k)?;
                a.breakfast = Some(match v.as_str() {
                    "yes" | "true" | "1" => true,
                    "no" | "false" | "0" => false,
                    other => return Err(format!("--breakfast must be yes or no (got '{other}')")),
                });
                i += 2;
            }
            "--price-source" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--price-source cannot be empty".to_string());
                }
                a.price_source = Some(v);
                i += 2;
            }
            "--price-checked" | "--price-checked-at" => {
                a.price_checked = Some(date(raw, i, k)?);
                i += 2;
            }
            "--free-cancel-until" => {
                a.free_cancel_until = Some(date(raw, i, k)?);
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
    a.id = id.ok_or_else(|| format!("--id <id> is required.\n{}", usage()))?;
    if a.sets().is_empty() {
        return Err(format!("at least one field to update is required.\n{}", usage()));
    }
    // A quoted rate is only meaningful with the date it was read.
    if a.price_source.is_some() && a.price_checked.is_none() {
        a.price_checked = Some(today());
    }
    Ok(a)
}

fn val(raw: &[String], i: usize, flag: &str) -> Result<String, String> {
    raw.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn int(raw: &[String], i: usize, flag: &str, min: i64) -> Result<i64, String> {
    let v = val(raw, i, flag)?;
    let n: i64 = v
        .parse()
        .map_err(|_| format!("{flag} must be an integer"))?;
    if n < min {
        return Err(format!("{flag} must be >= {min}"));
    }
    Ok(n)
}

fn date(raw: &[String], i: usize, flag: &str) -> Result<String, String> {
    let v = val(raw, i, flag)?;
    if !valid_date(&v) {
        return Err(format!("{flag} must be YYYY-MM-DD"));
    }
    Ok(v)
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
        assert_eq!(o.sets().len(), 1);
    }

    #[test]
    fn parses_decision_facts() {
        let o = parse_args(&a(&[
            "--id", "x1", "--room-size", "18", "--rooms-left", "1",
            "--free-cancel-until", "2026-09-28", "--price-source", "Booking.com",
            "--price-checked", "2026-09-04",
        ]))
        .unwrap();
        assert_eq!(o.room_size, Some(18));
        assert_eq!(o.rooms_left, Some(1));
        assert_eq!(o.free_cancel_until.as_deref(), Some("2026-09-28"));
        assert_eq!(o.price_checked.as_deref(), Some("2026-09-04"));
        assert_eq!(o.sets().len(), 5);
    }

    #[test]
    fn price_source_stamps_today_when_no_date_given() {
        let o = parse_args(&a(&["--id", "x1", "--price-source", "Booking.com"])).unwrap();
        let d = o.price_checked.expect("price_checked must be stamped");
        assert!(valid_date(&d), "stamped date must be YYYY-MM-DD: {d}");
        assert!(d >= "2026-01-01".to_string(), "sane stamp: {d}");
    }

    #[test]
    fn rooms_left_zero_is_allowed_but_negative_is_not() {
        assert_eq!(
            parse_args(&a(&["--id", "x", "--rooms-left", "0"])).unwrap().rooms_left,
            Some(0)
        );
        assert!(parse_args(&a(&["--id", "x", "--rooms-left", "-1"]))
            .unwrap_err()
            .contains(">= 0"));
    }

    #[test]
    fn parses_price_room_type_and_breakfast() {
        let o = parse_args(&a(&[
            "--id", "x", "--price", "7199", "--room-type", "豪華雙人房", "--breakfast", "no",
        ]))
        .unwrap();
        assert_eq!(o.price, Some(7199));
        assert_eq!(o.room_type.as_deref(), Some("豪華雙人房"));
        assert_eq!(o.breakfast, Some(false));
        let cols: Vec<&str> = o.sets().iter().map(|(c, _)| *c).collect();
        assert!(cols.contains(&"price_twd"));
        assert!(cols.contains(&"room_type"));
        assert!(cols.contains(&"breakfast_included"));
    }

    #[test]
    fn breakfast_yes_sets_one_and_rejects_junk() {
        assert_eq!(
            parse_args(&a(&["--id", "x", "--breakfast", "yes"])).unwrap().breakfast,
            Some(true)
        );
        assert!(parse_args(&a(&["--id", "x", "--breakfast", "maybe"]))
            .unwrap_err()
            .contains("yes or no"));
    }

    #[test]
    fn rejects_bad_date() {
        let e = parse_args(&a(&["--id", "x", "--free-cancel-until", "2026/09/28"])).unwrap_err();
        assert!(e.contains("YYYY-MM-DD"));
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

    #[test]
    fn civil_from_days_matches_known_epochs() {
        assert_eq!(civil_from_days(0), "1970-01-01");
        assert_eq!(civil_from_days(19_723), "2024-01-01"); // leap-year boundary
        assert_eq!(civil_from_days(20_698), "2026-09-02");
    }
}
