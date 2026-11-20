// `travel update-accommodation --id <id> [--image-url u] [--booking-url u]
//   [--price N] [--room-type t] [--breakfast yes|no]
//   [--room-size N] [--price-source s] [--price-checked YYYY-MM-DD]
//   [--free-cancel-until YYYY-MM-DD] [--rooms-left N] [--bathtub yes|no]
//   [--lat f --lon f | --lat clear --lon clear]`
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
use travel_db::repo::domestic_accommodations::{PriceBasis, update_fields};

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
    /// Tri-state, same shape as breakfast: None = leave alone (stays NULL/unchanged),
    /// Some = set has_bathtub 1/0. Verified per ROOM TYPE, not the property.
    bathtub: Option<bool>,
    /// None = leave alone; Some(n>=1) = set ranking (1 = 首選); Some(0) = clear
    /// (back to 未排序 — the candidate drops out of the recommendation).
    ranking: Option<i64>,
    /// None = leave alone; Some(text) = set notes (優劣比較與推薦理由);
    /// Some("clear") = clear the notes back to NULL.
    notes: Option<String>,
    /// WGS84 coordinates for the dashboard's location minimap — None = leave
    /// alone. `--lat` and `--lon` must be given together; `--lat clear --lon clear`
    /// clears them back to NULL.
    latitude: Option<f64>,
    longitude: Option<f64>,
    /// Set by `--lat clear --lon clear` — writes NULL to both coordinate columns.
    clear_coords: bool,
    /// Tax basis of the published rate: None = leave alone; Some("incl"|"excl")
    /// = set; Some("clear") = back to NULL (口徑未確認) — same sentinel shape as
    /// `--notes clear`.
    price_basis: Option<String>,
}

impl Args {
    /// Ordered (column, value) pairs for the UPDATE. Column names are literals
    /// owned here — only values are bound.
    fn sets(&self) -> Vec<(&'static str, Value)> {
        let mut out: Vec<(&'static str, Value)> = Vec::new();
        let text = |col: &'static str, v: &Option<String>, out: &mut Vec<(&'static str, Value)>| {
            if let Some(s) = v {
                out.push((col, Value::Text(s.clone())));
            }
        };
        text("image_url", &self.image_url, &mut out);
        text("booking_url", &self.booking_url, &mut out);
        text("price_source", &self.price_source, &mut out);
        text("price_checked_at", &self.price_checked, &mut out);
        // Tax basis rides with the source, not the read date: setting it alone
        // never stamps price_checked_at (it is not a re-quoted price).
        if let Some(b) = &self.price_basis {
            out.push((
                "price_basis",
                if b == "clear" { Value::Null } else { Value::Text(b.clone()) },
            ));
        }
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
        if let Some(b) = self.bathtub {
            out.push(("has_bathtub", Value::Integer(i64::from(b))));
        }
        if let Some(n) = self.ranking {
            // 0 is the documented "clear" sentinel → NULL (未排序).
            out.push((
                "ranking",
                if n == 0 { Value::Null } else { Value::Integer(n) },
            ));
        }
        if let Some(s) = &self.notes {
            // "clear" is the documented sentinel → NULL (比較內容未寫).
            out.push((
                "notes",
                if s == "clear" { Value::Null } else { Value::Text(s.clone()) },
            ));
        }
        // The pair moves together (a half-updated coordinate would center the
        // minimap in the ocean). "clear" on both → NULL.
        if self.clear_coords {
            out.push(("latitude", Value::Null));
            out.push(("longitude", Value::Null));
        } else if let (Some(lat), Some(lon)) = (self.latitude, self.longitude) {
            out.push(("latitude", Value::Real(lat)));
            out.push(("longitude", Value::Real(lon)));
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
    let mut sets = args.sets();

    let conn = crate::db::connect_write().await?;

    // 改價格來源而沒給口徑 → 舊口徑屬於舊來源，清回 NULL（未知），不強制必填。
    // 清除與 UPDATE 在「同一條」SQL 裡原子完成（CASE WHEN price_source IS NOT 新值），
    // 並行更新不會洗掉對方剛寫入的口徑；同值重設（修拼字）不清。
    // 事前 SELECT 只用來決定要不要印 note（純顯示，不影響資料）。
    let guard_source = match (args.price_source.as_deref(), args.price_basis.is_none()) {
        (Some(src), true) => Some(src),
        _ => None,
    };
    let mut basis_cleared = false;
    let affected = if let Some(new_source) = guard_source {
        if let Some((old_source, old_basis)) =
            travel_db::repo::domestic_accommodations::price_source_and_basis(&conn, &args.id)
                .await?
        {
            basis_cleared =
                basis_clear_on_source_change(old_source.as_deref(), new_source, old_basis.as_deref());
        }
        travel_db::repo::domestic_accommodations::update_fields_guarding_basis(
            &conn, &args.id, &sets, new_source,
        )
        .await?
    } else {
        update_fields(&conn, &args.id, &sets).await?
    };

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
            Value::Real(x) => format!("{x}"),
            Value::Null => "(cleared)".to_string(),
            other => format!("{other:?}"),
        };
        println!("  {col}: {shown}");
    }
    if basis_cleared {
        println!("note: price_basis cleared (price source changed) — set --price-basis incl|excl");
    }
    Ok(())
}

fn usage() -> &'static str {
    "Usage:\n  travel update-accommodation --id <id> [--image-url <url>] [--booking-url <url>] \
     [--price <twd>] [--room-type <type>] [--breakfast yes|no] \
     [--room-size <sqm>] [--price-source <name>] [--price-checked <YYYY-MM-DD>] \
     [--price-basis <incl|excl|clear>] \
     [--free-cancel-until <YYYY-MM-DD>] [--rooms-left <n>] [--bathtub <yes|no>] \
     [--rank <n|0=清除>] [--notes \"<優劣比較與推薦理由>|clear=清除>] \
     [--lat <f> --lon <f>|--lat clear --lon clear]\n  \
     (slug-keyed reference data — no --plan-id; at least one field is required.\n  \
      --price-source without --price-checked stamps today, so a published rate always carries its read date.\n  \
      --price-basis 是價格的含稅口徑（incl=含稅及服務費、excl=未稅結帳另加、clear=回未確認）；\
      單獨改口徑不會蓋 price_checked_at（不是重新查價）。\n  \
      只給 --price-source 而未給 --price-basis 時，若來源真的變了，口徑會清回 NULL 並印 note \
      （舊口徑屬於舊來源；來源同值重設不清）。\n  \
      --lat/--lon 是 WGS84 座標（Google Maps place search 或 OSM 查得，不可猜測），dashboard 會用它渲染候選卡的位置小圖。\n  \
      --notes 與 --rank 是候選比較段落的內容：notes=優劣與推薦理由、rank=推薦順位（1=首選）。)"
}

/// Pure rule for the source-change side effect: when `--price-source` changes
/// the stored source and a basis was recorded, the OLD basis belongs to the OLD
/// source (Booking's incl would publish beside an Agoda rate) — clear it.
/// Same-value re-set (fixing a typo) and nothing-recorded both return false.
fn basis_clear_on_source_change(
    old_source: Option<&str>,
    new_source: &str,
    old_basis: Option<&str>,
) -> bool {
    old_basis.is_some() && old_source != Some(new_source)
}

/// Plain-text label for a stored basis value (CLI output only; dirty values
/// print raw — `validate data` is what flags them).
pub(crate) fn basis_label(v: &str) -> String {
    match v {
        "incl" => "incl（含稅及服務費）".to_string(),
        "excl" => "excl（未稅，結帳另加）".to_string(),
        other => other.to_string(),
    }
}

/// `YYYY-MM-DD` shape check — a malformed date would render as garbage on the page.
pub(crate) fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

pub(crate) fn today() -> String {
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
    let mut lat_clear = false;
    let mut lon_clear = false;
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
            "--bathtub" => {
                let v = val(raw, i, k)?;
                a.bathtub = Some(match v.as_str() {
                    "yes" | "true" | "1" => true,
                    "no" | "false" | "0" => false,
                    other => return Err(format!("--bathtub must be yes or no (got '{other}')")),
                });
                i += 2;
            }
            // --rank 0 = 清除順位（回未排序）；n>=1 = 排序（1 = 首選）。
            "--rank" | "--ranking" => {
                a.ranking = Some(int(raw, i, k, 0)?);
                i += 2;
            }
            // --notes "clear" = 清除比較文字；其他非空值 = 設定優劣與推薦理由。
            "--notes" | "--note" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err(
                        "--notes cannot be empty — use `--notes clear` to unset, or omit the flag"
                            .to_string(),
                    );
                }
                a.notes = Some(v);
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
            // Tax basis: incl|excl (validated enum) or `clear` (back to NULL).
            "--price-basis" => {
                let v = val(raw, i, k)?;
                a.price_basis = Some(match v.as_str() {
                    "clear" => "clear".to_string(),
                    other => PriceBasis::parse(other)
                        .map_err(|e| format!("--price-basis must be incl or excl ({e})"))?
                        .as_str()
                        .to_string(),
                });
                i += 2;
            }
            // WGS84 coordinates for the minimap. `clear` on both flags = clear.
            "--lat" | "--latitude" => {
                let v = val(raw, i, k)?;
                if v == "clear" {
                    lat_clear = true;
                } else {
                    a.latitude = Some(coord_value(&v, k)?);
                }
                i += 2;
            }
            "--lon" | "--lng" | "--longitude" => {
                let v = val(raw, i, k)?;
                if v == "clear" {
                    lon_clear = true;
                } else {
                    a.longitude = Some(coord_value(&v, k)?);
                }
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
    // Coordinates move as a pair: half-updated (or half-cleared) coords would
    // center the minimap in the ocean. Mixing clear with a value is a typo.
    // (Validated BEFORE the empty-sets check so a lone `--lon clear` reports the
    // real problem, not "at least one field".)
    match (lat_clear, lon_clear, a.latitude, a.longitude) {
        (true, true, None, None) => a.clear_coords = true,
        (true, true, _, _) => {
            return Err("--lat clear cannot be mixed with a --lat/--lon value".to_string())
        }
        (true, false, ..) | (false, true, ..) => {
            return Err("--lat clear and --lon clear must be given together".to_string())
        }
        (false, false, Some(_), None) | (false, false, None, Some(_)) => {
            return Err("--lat and --lon must be given together".to_string())
        }
        (false, false, Some(lat), Some(lon)) => {
            // Range / finiteness / null-island rules live in ONE place.
            crate::checks::validate_coords(lat, lon)?;
        }
        (false, false, None, None) => {}
    }
    if a.sets().is_empty() {
        return Err(format!("at least one field to update is required.\n{}", usage()));
    }
    // A quoted rate is only meaningful with the date it was read.
    if a.price_source.is_some() && a.price_checked.is_none() {
        a.price_checked = Some(today());
    }
    Ok(a)
}

/// Parse a coordinate flag value as a number; range / null-island checks are done on
/// the PAIR by `checks::validate_coords` once both flags are known.
fn coord_value(v: &str, flag: &str) -> Result<f64, String> {
    v.parse()
        .map_err(|_| format!("{flag} must be a decimal number (got '{v}')"))
}

pub(crate) fn val(raw: &[String], i: usize, flag: &str) -> Result<String, String> {
    raw.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

pub(crate) fn int(raw: &[String], i: usize, flag: &str, min: i64) -> Result<i64, String> {
    let v = val(raw, i, flag)?;
    let n: i64 = v
        .parse()
        .map_err(|_| format!("{flag} must be an integer"))?;
    if n < min {
        return Err(format!("{flag} must be >= {min}"));
    }
    Ok(n)
}

pub(crate) fn date(raw: &[String], i: usize, flag: &str) -> Result<String, String> {
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

    fn txt(v: &str) -> Value {
        Value::Text(v.to_string())
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
    fn bathtub_yes_no_sets_column_and_rejects_junk() {
        let yes = parse_args(&a(&["--id", "x", "--bathtub", "yes"])).unwrap();
        assert_eq!(yes.bathtub, Some(true));
        assert!(yes.sets().contains(&("has_bathtub", Value::Integer(1))));
        let no = parse_args(&a(&["--id", "x", "--bathtub", "no"])).unwrap();
        assert_eq!(no.bathtub, Some(false));
        assert!(no.sets().contains(&("has_bathtub", Value::Integer(0))));
        // Omitted = leave alone (no column in the UPDATE).
        let untouched = parse_args(&a(&["--id", "x", "--room-size", "18"])).unwrap();
        assert_eq!(untouched.bathtub, None);
        assert!(!untouched.sets().iter().any(|(c, _)| *c == "has_bathtub"));
        assert!(parse_args(&a(&["--id", "x", "--bathtub", "maybe"]))
            .unwrap_err()
            .contains("yes or no"));
    }

    #[test]
    fn rank_and_notes_set_columns_and_clear_sentinels() {
        let o = parse_args(&a(&[
            "--id", "x", "--rank", "1", "--notes", "9.6 評分傑出；缺點：無電梯",
        ]))
        .unwrap();
        assert!(o.sets().contains(&("ranking", Value::Integer(1))));
        assert!(matches!(
            o.sets().iter().find(|(c, _)| *c == "notes"),
            Some((_, Value::Text(s))) if s.contains("無電梯")
        ));
        // Sentinels: --rank 0 / --notes clear write SQL NULL (back to 未排序/未寫).
        let cleared = parse_args(&a(&["--id", "x", "--rank", "0", "--notes", "clear"])).unwrap();
        assert!(cleared.sets().contains(&("ranking", Value::Null)));
        assert!(cleared.sets().contains(&("notes", Value::Null)));
        // Omitted = leave alone (no column in the UPDATE).
        let untouched = parse_args(&a(&["--id", "x", "--room-size", "18"])).unwrap();
        assert!(!untouched.sets().iter().any(|(c, _)| *c == "ranking" || *c == "notes"));
        // Junk rejected.
        assert!(parse_args(&a(&["--id", "x", "--rank", "-1"]))
            .unwrap_err()
            .contains(">= 0"));
        assert!(parse_args(&a(&["--id", "x", "--notes", "  "]))
            .unwrap_err()
            .contains("cannot be empty"));
    }

    #[test]
    fn lat_lon_set_clear_and_must_pair() {
        // Set: both flags together → two Real columns.
        let o = parse_args(&a(&[
            "--id", "x", "--lat", "25.1330327", "--lon", "121.807191",
        ]))
        .unwrap();
        assert!(o
            .sets()
            .contains(&("latitude", Value::Real(25.1330327))));
        assert!(o
            .sets()
            .contains(&("longitude", Value::Real(121.807191))));
        // Clear: both `clear` → two NULL columns.
        let c = parse_args(&a(&["--id", "x", "--lat", "clear", "--lon", "clear"])).unwrap();
        assert!(c.sets().contains(&("latitude", Value::Null)));
        assert!(c.sets().contains(&("longitude", Value::Null)));
        // Omitted = leave alone.
        let untouched = parse_args(&a(&["--id", "x", "--room-size", "18"])).unwrap();
        assert!(!untouched
            .sets()
            .iter()
            .any(|(col, _)| *col == "latitude" || *col == "longitude"));
        // Half a pair → reject.
        assert!(parse_args(&a(&["--id", "x", "--lat", "25.1"]))
            .unwrap_err()
            .contains("together"));
        // Single clear → reject.
        assert!(parse_args(&a(&["--id", "x", "--lon", "clear"]))
            .unwrap_err()
            .contains("together"));
        // Mixing clear with a value → reject.
        assert!(parse_args(&a(&["--id", "x", "--lat", "clear", "--lon", "121"]))
            .unwrap_err()
            .contains("together"));
        // Out of range / non-numeric / null island → reject.
        assert!(parse_args(&a(&["--id", "x", "--lat", "91", "--lon", "121"]))
            .unwrap_err()
            .contains("between -90 and 90"));
        assert!(parse_args(&a(&["--id", "x", "--lat", "north", "--lon", "121"]))
            .unwrap_err()
            .contains("decimal number"));
        assert!(parse_args(&a(&["--id", "x", "--lat", "0", "--lon", "0"]))
            .unwrap_err()
            .contains("null island"));
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

    #[test]
    fn price_basis_sets_clears_and_rejects_junk() {
        // Set: validated value lands in the UPDATE.
        let o = parse_args(&a(&["--id", "x", "--price-basis", "excl"])).unwrap();
        assert!(o.sets().contains(&("price_basis", txt("excl"))));
        // Clear sentinel → SQL NULL (口徑回未確認).
        let c = parse_args(&a(&["--id", "x", "--price-basis", "clear"])).unwrap();
        assert!(c.sets().contains(&("price_basis", Value::Null)));
        // Junk fails loud naming the legal set + the clear sentinel.
        assert!(parse_args(&a(&["--id", "x", "--price-basis", "incl. tax"]))
            .unwrap_err()
            .contains("incl or excl"));
        // Omitted = leave alone (no column in the UPDATE).
        let untouched = parse_args(&a(&["--id", "x", "--room-size", "18"])).unwrap();
        assert!(!untouched.sets().iter().any(|(c, _)| *c == "price_basis"));
    }

    #[test]
    fn price_basis_alone_does_not_touch_price_checked_at() {
        // 口徑是價格的屬性，不是「重新查價」— only --price-source stamps today.
        let o = parse_args(&a(&["--id", "x", "--price-basis", "incl"])).unwrap();
        assert!(o.sets().iter().any(|(c, _)| *c == "price_basis"));
        assert!(!o.sets().iter().any(|(c, _)| *c == "price_checked_at"));
        assert_eq!(o.price_checked, None);
    }

    #[test]
    fn price_source_with_basis_keeps_the_new_basis_and_stamps_date() {
        let o = parse_args(&a(&["--id", "x", "--price-source", "Agoda", "--price-basis", "excl"]))
            .unwrap();
        let cols: Vec<&str> = o.sets().iter().map(|(c, _)| *c).collect();
        assert!(cols.contains(&"price_source"));
        assert!(cols.contains(&"price_basis"));
        assert!(cols.contains(&"price_checked_at"), "source still stamps today");
        assert!(o.sets().contains(&("price_basis", txt("excl"))));
    }

    #[test]
    fn source_change_clears_basis_only_when_source_really_changed_and_basis_was_set() {
        // Change of source + a recorded basis → the old basis belongs to the old
        // source; clearing it to NULL beats publishing Booking's incl beside an
        // Agoda rate.
        assert!(basis_clear_on_source_change(
            Some("Booking.com"),
            "Agoda",
            Some("incl")
        ));
        // Same-value re-set (fixing a typo in the source text) → no clear.
        assert!(!basis_clear_on_source_change(
            Some("Booking.com"),
            "Booking.com",
            Some("incl")
        ));
        // Source appears where none was recorded → a change.
        assert!(basis_clear_on_source_change(None, "Booking.com", Some("incl")));
        // Nothing recorded to clear → no write, no note.
        assert!(!basis_clear_on_source_change(Some("Booking.com"), "Agoda", None));
    }
}
