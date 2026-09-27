// `travel add-accommodation --dest <slug> --hotel <name> --room-type <type> --price <twd>
//   [--image-url <url>] [--booking-url <url>] [--sea-view] [--breakfast]
//   [--room-size <sqm>] [--rooms-left <n>] [--free-cancel-until YYYY-MM-DD]
//   [--price-source <name>] [--price-checked YYYY-MM-DD] [--bathtub yes|no]`
// — add one `domestic_accommodations` row (Taiwan domestic stay reference data).
//
// The decision-fact flags mirror update-accommodation's so a candidate scraped off
// an OTA page is ONE mutation, not add-then-update: splitting the write is how a
// half-recorded candidate (price with no read date, cancel deadline assumed rather
// than read) ends up as wrong data that a later verification pass has to catch.
// `--price-source` without `--price-checked` stamps today — same rule as update.
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
    room_size: Option<i64>,
    rooms_left: Option<i64>,
    free_cancel_until: Option<String>,
    price_source: Option<String>,
    price_checked: Option<String>,
    /// Per-room bathtub: None = unverified (stays NULL), Some(1)/Some(0) = read off
    /// the room-type facility list. Valued `yes|no` — NOT a bare flag, because an
    /// absent flag must mean "not verified", never a guessed "no".
    bathtub: Option<i64>,
    /// Recommended order (1 = first choice). Usually set later, after the whole
    /// shortlist is compared — hence the reminder when it is missing.
    ranking: Option<i64>,
    /// 優劣比較與推薦理由 — the comparison content the dashboard renders.
    notes: Option<String>,
    /// WGS84 coordinates for the dashboard's per-candidate location minimap.
    /// Sourced from a Google Maps place search or OSM/Nominatim — never guessed.
    latitude: Option<f64>,
    longitude: Option<f64>,
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
        room_size_sqm: args.room_size,
        price_source: args.price_source.clone(),
        price_checked_at: args.price_checked.clone(),
        free_cancel_until: args.free_cancel_until.clone(),
        rooms_left: args.rooms_left,
        has_bathtub: args.bathtub,
        ranking: args.ranking,
        notes: args.notes.clone(),
        latitude: args.latitude,
        longitude: args.longitude,
    };

    let affected = insert(&conn, &row).await?;
    if affected == 0 {
        println!("Accommodation already exists (id={id}) — nothing added.");
        println!("Next: update facts via `travel update-accommodation --id {id} [--image-url <url>] [--price <twd>] ...`.");
        return Ok(());
    }

    println!(
        "✅ Added accommodation: {} {} TWD {} for {}",
        args.hotel, args.room_type, args.price, args.dest
    );
    println!("  id: {id}");
    if let Some(n) = args.room_size {
        println!("  room size: {n}m²");
    }
    if let Some(s) = &args.price_source {
        println!("  price source: {s} (checked {})", args.price_checked.as_deref().unwrap_or("?"));
    }
    if let Some(d) = &args.free_cancel_until {
        println!("  free cancel until: {d}");
    }
    if let Some(n) = args.rooms_left {
        println!("  rooms left: {n}");
    }
    if let Some(t) = args.bathtub {
        println!("  bathtub: {}", if t == 1 { "yes" } else { "no" });
    }
    if let Some(n) = args.ranking {
        println!("  rank: {n}");
    }
    if let Some(s) = &args.notes {
        println!("  notes: {s}");
    }
    if let (Some(lat), Some(lon)) = (args.latitude, args.longitude) {
        println!("  location: {lat}, {lon}");
    }
    // Location reminder — the dashboard's candidate minimap renders from lat/lon;
    // without them the card has no 位置小圖.
    if args.latitude.is_none() {
        println!(
            "💡 位置未定：查得 WGS84 座標後卡片才有位置小圖 —— \
             `travel update-accommodation --id {id} --lat <f> --lon <f>`（Google Maps place search 或 OSM 查得，不可猜測）"
        );
    }
    if args.image_url.is_none() {
        println!("  image: (none) — add via `travel update-accommodation --id {id} --image-url <url>`");
    }
    if args.booking_url.is_none() {
        println!("  booking: (none) — add via `travel update-accommodation --id {id} --booking-url <url>`");
    }
    // Comparison content reminder — the dashboard's 比較/推薦排序 block renders from
    // `notes` + `ranking`; a candidate added without them shows up as a bare card.
    // The agent (or human) writing candidates should finish the comparison, not
    // leave the section as a bare price grid.
    if args.notes.is_none() || args.ranking.is_none() {
        println!(
            "💡 比較內容未齊：寫入優劣與推薦順位後卡片才有比較段落 —— \
             `travel update-accommodation --id {id} --notes \"<優劣與推薦理由>\" --rank <1=首選>`"
        );
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
     [--image-url <url>] [--booking-url <url>] [--sea-view] [--breakfast] \
     [--room-size <sqm>] [--rooms-left <n>] [--free-cancel-until <YYYY-MM-DD>] \
     [--price-source <name>] [--price-checked <YYYY-MM-DD>] [--bathtub <yes|no>] \
     [--rank <n>] [--notes \"<優劣比較與推薦理由>\"] [--lat <f> --lon <f>]\n  \
     (slug-keyed reference data — no --plan-id; idempotent on the same dest|hotel|room|price.\n  \
      --price-source without --price-checked stamps today, so a quoted rate always carries its read date.\n  \
      --lat/--lon 是 WGS84 座標（Google Maps place search 或 OSM 查得，不可猜測），\n  \
      dashboard 會用它渲染每張候選卡的位置小圖。)\n  \
      --notes/--rank 是比較段落的內容來源；補齊前 CLI 會提醒。)"
}

fn parse_args(raw: &[String]) -> Result<Args, String> {
    use crate::update_accommodation::{date, int};

    let mut dest: Option<String> = None;
    let mut hotel: Option<String> = None;
    let mut room_type: Option<String> = None;
    let mut price: Option<i64> = None;
    let mut image_url: Option<String> = None;
    let mut booking_url: Option<String> = None;
    let mut sea_view = false;
    let mut breakfast = false;
    let mut room_size: Option<i64> = None;
    let mut rooms_left: Option<i64> = None;
    let mut free_cancel_until: Option<String> = None;
    let mut price_source: Option<String> = None;
    let mut price_checked: Option<String> = None;
    let mut bathtub: Option<i64> = None;
    let mut ranking: Option<i64> = None;
    let mut notes: Option<String> = None;
    let mut latitude: Option<f64> = None;
    let mut longitude: Option<f64> = None;
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
            "--room-size" | "--sqm" => {
                room_size = Some(int(raw, i, k, 1)?);
                i += 2;
            }
            "--rooms-left" => {
                rooms_left = Some(int(raw, i, k, 0)?);
                i += 2;
            }
            "--free-cancel-until" => {
                free_cancel_until = Some(date(raw, i, k)?);
                i += 2;
            }
            "--price-source" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--price-source cannot be empty".to_string());
                }
                price_source = Some(v);
                i += 2;
            }
            "--price-checked" | "--price-checked-at" => {
                price_checked = Some(date(raw, i, k)?);
                i += 2;
            }
            "--bathtub" => {
                let v = val(raw, i, k)?;
                bathtub = Some(match v.as_str() {
                    "yes" | "true" | "1" => 1,
                    "no" | "false" | "0" => 0,
                    other => {
                        return Err(format!("--bathtub must be yes or no (got '{other}')"))
                    }
                });
                i += 2;
            }
            "--rank" | "--ranking" => {
                ranking = Some(int(raw, i, k, 1)?);
                i += 2;
            }
            "--notes" | "--note" => {
                let v = val(raw, i, k)?;
                if v.trim().is_empty() {
                    return Err("--notes cannot be empty (omit the flag, or use update-accommodation to clear)".to_string());
                }
                notes = Some(v);
                i += 2;
            }
            "--lat" | "--latitude" => {
                latitude = Some(coord(raw, i, k, -90.0, 90.0)?);
                i += 2;
            }
            "--lon" | "--lng" | "--longitude" => {
                longitude = Some(coord(raw, i, k, -180.0, 180.0)?);
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
    // A quoted rate is only meaningful with the date it was read — same rule as
    // update-accommodation, applied at add time so the first row is already complete.
    if price_source.is_some() && price_checked.is_none() {
        price_checked = Some(crate::update_accommodation::today());
    }
    // Half a coordinate pair would render a minimap centered in the ocean —
    // the pair must arrive together (and be updated together).
    match (latitude, longitude) {
        (Some(_), None) | (None, Some(_)) => {
            return Err("--lat and --lon must be given together".to_string());
        }
        (Some(lat), Some(lon)) => {
            if (lat == 0.0 && lon == 0.0) || (lat.abs() < 1e-9 && lon.abs() < 1e-9) {
                return Err("--lat/--lon look like a null island (0,0) — geocode a real place".to_string());
            }
        }
        _ => {}
    }
    Ok(Args {
        dest,
        hotel,
        room_type,
        price,
        image_url,
        booking_url,
        sea_view,
        breakfast,
        room_size,
        rooms_left,
        free_cancel_until,
        price_source,
        price_checked,
        bathtub,
        ranking,
        notes,
        latitude,
        longitude,
    })
}

/// Parse a coordinate flag value as f64 within [min, max].
fn coord(raw: &[String], i: usize, flag: &str, min: f64, max: f64) -> Result<f64, String> {
    let v = val(raw, i, flag)?;
    let n: f64 = v
        .parse()
        .map_err(|_| format!("{flag} must be a decimal number (got '{v}')"))?;
    if !(min..=max).contains(&n) {
        return Err(format!("{flag} must be between {min} and {max} (got {v})"));
    }
    Ok(n)
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
        assert!(o.room_size.is_none());
        assert!(o.price_source.is_none());
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
    fn parses_decision_facts_in_one_command() {
        let o = parse_args(&a(&[
            "--dest", "jiufen", "--hotel", "不厭晴", "--room-type", "海景豪華四人房（附陽台）",
            "--price", "5800", "--sea-view", "--room-size", "45", "--rooms-left", "3",
            "--free-cancel-until", "2026-10-05", "--price-source", "Booking.com",
            "--price-checked", "2026-09-27",
        ]))
        .unwrap();
        assert_eq!(o.room_size, Some(45));
        assert_eq!(o.rooms_left, Some(3));
        assert_eq!(o.free_cancel_until.as_deref(), Some("2026-10-05"));
        assert_eq!(o.price_source.as_deref(), Some("Booking.com"));
        assert_eq!(o.price_checked.as_deref(), Some("2026-09-27"));
    }

    #[test]
    fn bathtub_yes_no_and_unverified_default() {
        let yes = parse_args(&a(&[
            "--dest", "jiufen", "--hotel", "曉宅山", "--room-type", "四人房附浴缸（暮宅山）",
            "--price", "7895", "--bathtub", "yes",
        ]))
        .unwrap();
        assert_eq!(yes.bathtub, Some(1));
        let no = parse_args(&a(&[
            "--dest", "jiufen", "--hotel", "柳園", "--room-type", "側面海景豪華雙人房",
            "--price", "1600", "--bathtub", "no",
        ]))
        .unwrap();
        assert_eq!(no.bathtub, Some(0));
        // Omitting the flag means UNVERIFIED (NULL) — never a guessed "no".
        let unverified = parse_args(&a(&[
            "--dest", "jiufen", "--hotel", "H", "--room-type", "R", "--price", "1",
        ]))
        .unwrap();
        assert_eq!(unverified.bathtub, None);
        assert!(parse_args(&a(&[
            "--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1", "--bathtub", "maybe",
        ]))
        .unwrap_err()
        .contains("yes or no"));
    }

    #[test]
    fn parses_rank_and_notes() {
        let o = parse_args(&a(&[
            "--dest", "jiufen", "--hotel", "魚礁十五號", "--room-type", "四人房－附浴缸",
            "--price", "4000", "--rank", "1",
            "--notes", "9.6 評分傑出＋44m²；免訂金、10/5 前免費取消；缺點：無電梯",
        ]))
        .unwrap();
        assert_eq!(o.ranking, Some(1));
        assert!(o.notes.as_deref().unwrap().contains("無電梯"));
    }

    #[test]
    fn rank_and_notes_default_to_none_and_reject_bad_input() {
        let o = parse_args(&a(&["--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1"]))
            .unwrap();
        assert_eq!(o.ranking, None);
        assert_eq!(o.notes, None);
        assert!(parse_args(&a(&[
            "--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1", "--rank", "0",
        ]))
        .unwrap_err()
        .contains(">= 1"));
        assert!(parse_args(&a(&[
            "--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1", "--notes", "  ",
        ]))
        .unwrap_err()
        .contains("cannot be empty"));
    }

    #[test]
    fn parses_lat_lon_pair() {
        let o = parse_args(&a(&[
            "--dest", "jiufen", "--hotel", "魚礁十五號", "--room-type", "四人房－附浴缸",
            "--price", "4000", "--lat", "25.1330327", "--lon", "121.807191",
        ]))
        .unwrap();
        assert!((o.latitude.unwrap() - 25.1330327).abs() < 1e-9);
        assert!((o.longitude.unwrap() - 121.807191).abs() < 1e-9);
    }

    #[test]
    fn lat_lon_must_come_as_a_pair_and_be_in_range() {
        // Half a pair → reject (a minimap needs both).
        assert!(parse_args(&a(&[
            "--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1", "--lat", "25.1",
        ]))
        .unwrap_err()
        .contains("together"));
        assert!(parse_args(&a(&[
            "--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1", "--lon", "121.8",
        ]))
        .unwrap_err()
        .contains("together"));
        // Out of range → reject.
        assert!(parse_args(&a(&[
            "--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1",
            "--lat", "91", "--lon", "121",
        ]))
        .unwrap_err()
        .contains("between -90 and 90"));
        assert!(parse_args(&a(&[
            "--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1",
            "--lat", "25", "--lon", "200",
        ]))
        .unwrap_err()
        .contains("between -180 and 180"));
        // Non-numeric → reject.
        assert!(parse_args(&a(&[
            "--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1",
            "--lat", "north", "--lon", "121",
        ]))
        .unwrap_err()
        .contains("decimal number"));
        // Null island → reject (0,0 is a sentinel, not a place).
        assert!(parse_args(&a(&[
            "--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1",
            "--lat", "0", "--lon", "0",
        ]))
        .unwrap_err()
        .contains("null island"));
    }

    #[test]
    fn price_source_stamps_today_when_no_date_given() {
        let o = parse_args(&a(&[
            "--dest", "jiufen", "--hotel", "H", "--room-type", "R", "--price", "1",
            "--price-source", "Booking.com",
        ]))
        .unwrap();
        let d = o.price_checked.expect("price_checked must be stamped");
        assert!(crate::update_accommodation::valid_date(&d), "stamped date must be YYYY-MM-DD: {d}");
        assert!(d >= "2026-01-01".to_string(), "sane stamp: {d}");
    }

    #[test]
    fn rooms_left_zero_is_allowed_but_negative_is_not() {
        assert_eq!(
            parse_args(&a(&["--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1", "--rooms-left", "0"]))
                .unwrap()
                .rooms_left,
            Some(0)
        );
        assert!(parse_args(&a(&["--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1", "--rooms-left", "-1"]))
            .unwrap_err()
            .contains(">= 0"));
    }

    #[test]
    fn rejects_bad_date_facts() {
        assert!(parse_args(&a(&[
            "--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1",
            "--free-cancel-until", "2026/10/05",
        ]))
        .unwrap_err()
        .contains("YYYY-MM-DD"));
        assert!(parse_args(&a(&[
            "--dest", "d", "--hotel", "H", "--room-type", "R", "--price", "1",
            "--price-checked", "09-27",
        ]))
        .unwrap_err()
        .contains("YYYY-MM-DD"));
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
