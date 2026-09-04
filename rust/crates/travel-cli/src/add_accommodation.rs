// `travel add-accommodation --dest <slug> --hotel <name> --room-type <type> --price <twd>
//   [--image-url <url>] [--booking-url <url>] [--sea-view] [--breakfast]`
// — add one `domestic_accommodations` row (Taiwan domestic stay reference data).
//
// Slug-keyed GLOBAL reference data — NO --plan-id, NO audit triad (same family as
// add-transit / add-omiyage). The slug is validated against destination_config
// (fail loud on an unknown destination). Parameterized INSERT OR IGNORE: an
// affected_row_count of 0 is a natural dedup (id already exists), not a failure —
// it is surfaced as "already exists".
//
// The id is deterministic: `{dest}_{fnv1a64(dest|hotel|room_type|price):016x}` —
// stable across runs and toolchains (unlike DefaultHasher), so re-adding the same
// stay is idempotent.

use travel_db::repo::domestic_accommodations::{NewDomesticAccommodation, insert};
use travel_db::repo::omiyage::config_slug_exists;

#[derive(Debug)]
struct Args {
    dest: String,
    hotel: String,
    room_type: String,
    price: i64,
    image_url: Option<String>,
    booking_url: Option<String>,
    sea_view: bool,
    breakfast: bool,
}

pub async fn run(raw: &[String]) -> Result<(), String> {
    if raw.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let args = parse_args(raw)?;

    let conn = crate::db::connect_write().await?;
    if !config_slug_exists(&conn, &args.dest).await? {
        return Err(format!(
            "Error: unknown destination '{}' — not in destination_config (register it first)",
            args.dest
        ));
    }

    let id = accommodation_id(&args.dest, &args.hotel, &args.room_type, args.price);
    let row = NewDomesticAccommodation {
        id: id.clone(),
        destination: args.dest.clone(),
        hotel_name: args.hotel.clone(),
        room_type: args.room_type.clone(),
        sea_view: i64::from(args.sea_view),
        max_occupancy: None,
        price_twd: args.price,
        breakfast_included: i64::from(args.breakfast),
        source: Some("manual".to_string()),
        image_url: args.image_url.clone(),
        booking_url: args.booking_url.clone(),
    };

    let affected = insert(&conn, &row).await?;
    if affected == 0 {
        println!("Accommodation already exists (id={id}) — nothing added.");
        println!("Next: update links via `travel update-accommodation --id {id} [--image-url <url>] [--booking-url <url>]`.");
        return Ok(());
    }

    println!(
        "✅ Added accommodation: {} {} TWD {} for {}",
        args.hotel, args.room_type, args.price, args.dest
    );
    println!("  id: {id}");
    if args.image_url.is_none() {
        println!("  image: (none) — add via `travel update-accommodation --id {id} --image-url <url>`");
    }
    if args.booking_url.is_none() {
        println!("  booking: (none) — add via `travel update-accommodation --id {id} --booking-url <url>`");
    }
    Ok(())
}

/// Deterministic id: `{dest}_{fnv1a64(dest|hotel|room_type|price):016x}`.
pub fn accommodation_id(dest: &str, hotel: &str, room_type: &str, price: i64) -> String {
    format!(
        "{dest}_{:016x}",
        fnv1a64(&format!("{dest}|{hotel}|{room_type}|{price}"))
    )
}

/// FNV-1a 64-bit — stable hash (no toolchain-dependent DefaultHasher).
fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn usage() -> &'static str {
    "Usage:\n  travel add-accommodation --dest <slug> --hotel <name> --room-type <type> --price <twd> \
     [--image-url <url>] [--booking-url <url>] [--sea-view] [--breakfast]\n  \
     (slug-keyed reference data — no --plan-id; idempotent on the same dest|hotel|room|price)"
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    let mut dest: Option<String> = None;
    let mut hotel: Option<String> = None;
    let mut room_type: Option<String> = None;
    let mut price: Option<i64> = None;
    let mut image_url: Option<String> = None;
    let mut booking_url: Option<String> = None;
    let mut sea_view = false;
    let mut breakfast = false;
    let mut i = 0;
    while i < raw.len() {
        let k = raw[i].as_str();
        match k {
            "--dest" | "--destination" | "--slug" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err(format!("{k} cannot be empty"));
                }
                dest = Some(v);
                i += 2;
            }
            "--hotel" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--hotel cannot be empty".to_string());
                }
                hotel = Some(v);
                i += 2;
            }
            "--room-type" | "--room_type" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--room-type cannot be empty".to_string());
                }
                room_type = Some(v);
                i += 2;
            }
            "--price" => {
                let v = val(raw, i, k)?;
                let n: i64 = v
                    .parse()
                    .map_err(|_| "--price must be an integer (TWD)".to_string())?;
                if n <= 0 {
                    return Err("--price must be > 0".to_string());
                }
                price = Some(n);
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
            "--sea-view" => {
                sea_view = true;
                i += 1;
            }
            "--breakfast" => {
                breakfast = true;
                i += 1;
            }
            "--plan-id" => {
                return Err(
                    "no --plan-id here — domestic_accommodations is destination-scoped reference data \
                     (add-accommodation is global/slug-keyed and takes no --plan-id)"
                        .to_string(),
                );
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown flag for add-accommodation: {other}"));
            }
            other => return Err(format!("unexpected positional argument: {other}")),
        }
    }
    let dest = dest.ok_or_else(|| format!("--dest <slug> is required.\n{}", usage()))?;
    let hotel = hotel.ok_or_else(|| "--hotel <name> is required".to_string())?;
    let room_type = room_type.ok_or_else(|| "--room-type <type> is required".to_string())?;
    let price = price.ok_or_else(|| "--price <twd> is required".to_string())?;
    Ok(Args {
        dest,
        hotel,
        room_type,
        price,
        image_url,
        booking_url,
        sea_view,
        breakfast,
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
        let o = parse_args(&a(&[
            "--dest", "jiufen", "--hotel", "海論", "--room-type", "海景雙人房", "--price", "5200",
        ]))
        .unwrap();
        assert_eq!(o.dest, "jiufen");
        assert_eq!(o.hotel, "海論");
        assert_eq!(o.price, 5200);
        assert!(o.image_url.is_none());
        assert!(o.booking_url.is_none());
        assert!(!o.sea_view);
    }

    #[test]
    fn parses_optional_urls_and_flags() {
        let o = parse_args(&a(&[
            "--dest", "jiufen", "--hotel", "H", "--room-type", "R", "--price", "100",
            "--image-url", "https://img", "--booking-url", "https://book", "--sea-view", "--breakfast",
        ]))
        .unwrap();
        assert_eq!(o.image_url.as_deref(), Some("https://img"));
        assert_eq!(o.booking_url.as_deref(), Some("https://book"));
        assert!(o.sea_view);
        assert!(o.breakfast);
    }

    #[test]
    fn rejects_missing_fields() {
        assert!(parse_args(&a(&["--hotel", "H", "--room-type", "R", "--price", "1"])).unwrap_err().contains("--dest"));
        assert!(parse_args(&a(&["--dest", "d", "--room-type", "R", "--price", "1"])).unwrap_err().contains("--hotel"));
        assert!(parse_args(&a(&["--dest", "d", "--hotel", "H", "--price", "1"])).unwrap_err().contains("--room-type"));
        assert!(parse_args(&a(&["--dest", "d", "--hotel", "H", "--room-type", "R"])).unwrap_err().contains("--price"));
    }

    #[test]
    fn rejects_bad_price() {
        assert!(parse_args(&a(&["--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "5k"])).unwrap_err().contains("--price"));
        assert!(parse_args(&a(&["--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "0"])).unwrap_err().contains("> 0"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let e = parse_args(&a(&["--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1", "--bogus"])).unwrap_err();
        assert!(e.contains("unknown flag"));
    }

    #[test]
    fn rejects_plan_id() {
        let e = parse_args(&a(&["--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1", "--plan-id", "x"])).unwrap_err();
        assert!(e.contains("no --plan-id"));
    }

    #[test]
    fn id_is_deterministic_and_scoped() {
        let id1 = accommodation_id("jiufen", "海論", "海景雙人房", 5200);
        let id2 = accommodation_id("jiufen", "海論", "海景雙人房", 5200);
        assert_eq!(id1, id2, "same tuple must give the same id");
        assert!(id1.starts_with("jiufen_"));
        let id3 = accommodation_id("jiufen", "海論", "海景雙人房", 5300);
        assert_ne!(id1, id3, "different price must give a different id");
    }
}
