// `travel set-activity-geocode` — pin one activity on the day map WITHOUT a route
// segment (spec 2026-11-20-cli-itinerary-editing-design.md §2.3). The pin lives on
// the activity row itself (lat/lon/geo_source), so it follows `move-activity`.
//
//   travel set-activity-geocode <day> <session> <id|title>
//       (--lat <f> --lon <f> [--source <t>] | --clear) [--dest <slug>] [--plan-id ...]
//
// One transaction: match → read → validate → write → touch_day → audit triad.

use travel_db::repo::itinerary::{self, ActivityGeo};

pub(crate) const USAGE: &str = "travel set-activity-geocode <day> <session> <id|title> \
(--lat <f> --lon <f> [--source <t>] | --clear) [--dest <slug>]\n  \
(pin one activity on the day map without a route segment; coordinates come from a real \
geocode — never guess; same value = no change, no version bump; audited)";

#[derive(Debug, PartialEq)]
enum Action {
    Set { lat: f64, lon: f64, source: Option<String> },
    Clear,
}

#[derive(Debug)]
struct Parsed {
    day: i64,
    session: String,
    key: String,
    action: Action,
    dest: Option<String>,
}

fn parse(args: &[String]) -> Result<Parsed, String> {
    let (mut lat, mut lon, mut source): (Option<f64>, Option<f64>, Option<String>) =
        (None, None, None);
    let mut clear = false;
    let mut dest: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let val = |i: usize, flag: &str| -> Result<String, String> {
        args.get(i + 1)
            .cloned()
            .ok_or_else(|| format!("{flag} requires a value"))
    };
    let num = |v: String, flag: &str| -> Result<f64, String> {
        v.parse::<f64>()
            .map_err(|_| format!("{flag} must be a decimal number (got '{v}')"))
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--lat" => {
                lat = Some(num(val(i, "--lat")?, "--lat")?);
                i += 2;
            }
            "--lon" => {
                lon = Some(num(val(i, "--lon")?, "--lon")?);
                i += 2;
            }
            "--source" => {
                let v = val(i, "--source")?;
                if v.trim().is_empty() {
                    return Err("--source cannot be empty".to_string());
                }
                source = Some(v);
                i += 2;
            }
            "--clear" => {
                clear = true;
                i += 1;
            }
            "--dest" => {
                dest = Some(val(i, "--dest")?);
                i += 2;
            }
            f if crate::plan_resolver::is_resolver_flag(f) => i += 2,
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}. Usage:\n{USAGE}"));
            }
            _ => {
                positional.push(args[i].clone());
                i += 1;
            }
        }
    }
    if positional.len() != 3 {
        return Err(format!("Usage:\n{USAGE}"));
    }
    let action = if clear {
        if lat.is_some() || lon.is_some() || source.is_some() {
            return Err("--clear cannot be combined with --lat/--lon/--source".to_string());
        }
        Action::Clear
    } else {
        match (lat, lon) {
            (Some(lat), Some(lon)) => {
                crate::checks::validate_coords(lat, lon)?;
                Action::Set { lat, lon, source }
            }
            (None, None) => {
                return Err(format!("give --lat and --lon, or --clear. Usage:\n{USAGE}"));
            }
            _ => return Err("--lat and --lon must be given together".to_string()),
        }
    };
    let day = positional[0]
        .parse::<i64>()
        .ok()
        .filter(|d| *d >= 1)
        .ok_or_else(|| format!("<day> must be a positive integer. Usage:\n{USAGE}"))?;
    let session = positional[1].clone();
    if !["morning", "noon", "afternoon", "evening"].contains(&session.as_str()) {
        return Err("<session> must be one of: morning|noon|afternoon|evening".to_string());
    }
    Ok(Parsed { day, session, key: positional[2].clone(), action, dest })
}

/// The pin after applying `action`, or `None` when nothing would change.
fn next_geo(before: &Option<ActivityGeo>, action: &Action) -> Option<Option<ActivityGeo>> {
    match action {
        Action::Clear => before.as_ref().map(|_| None),
        Action::Set { lat, lon, source } => {
            if let Some(b) = before {
                let same_coords = b.lat == *lat && b.lon == *lon;
                // An omitted --source keeps no claim about provenance, so it only
                // matters when the coordinates themselves change.
                if same_coords && (source.is_none() || source.as_ref() == b.source.as_ref()) {
                    return None;
                }
            }
            Some(Some(ActivityGeo { lat: *lat, lon: *lon, source: source.clone() }))
        }
    }
}

fn fmt_geo(g: &Option<ActivityGeo>) -> String {
    match g {
        None => "(none)".to_string(),
        Some(g) => match &g.source {
            Some(s) => format!("{},{} [{s}]", g.lat, g.lon),
            None => format!("{},{}", g.lat, g.lon),
        },
    }
}

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse(args)?;
    let conn = crate::db::connect_write().await?;
    let destination = crate::set_activity::read_destination(&conn, &plan_id, &parsed.dest).await?;
    let (day, session) = (parsed.day, parsed.session.clone());

    let outcome = async {
        crate::cascade::common::begin_immediate(&conn, "set-activity-geocode").await?;
        let activity_id = travel_db::repo::records::match_activity_unique(
            &conn, &plan_id, &destination, day, &session, &parsed.key,
        )
        .await?;
        let before =
            itinerary::read_activity_geo(&conn, &plan_id, &destination, day, &session, &activity_id)
                .await?;
        let Some(after) = next_geo(&before, &parsed.action) else {
            return Ok((activity_id, None));
        };
        let title = {
            let row = itinerary::read_activity_row(
                &conn, &plan_id, &destination, day, &session, &activity_id,
            )
            .await?
            .ok_or_else(|| format!("activity {activity_id} not found"))?;
            row.title
        };
        let version_before = crate::set_activity::read_version(&conn, &plan_id).await?;
        itinerary::set_activity_geocode(
            &conn, &plan_id, &destination, day, &session, &activity_id, after.as_ref(),
        )
        .await?;
        crate::set_activity::touch_day(&conn, &plan_id, &destination, day).await?;
        let undef = |v: Option<String>| v.unwrap_or_else(|| "undefined".to_string());
        let f = |g: &Option<ActivityGeo>| {
            (
                undef(g.as_ref().map(|g| g.lat.to_string())),
                undef(g.as_ref().map(|g| g.lon.to_string())),
                undef(g.as_ref().and_then(|g| g.source.clone())),
            )
        };
        let (bl, bo, bs) = f(&before);
        let (al, ao, a_s) = f(&after);
        let kv: Vec<(&str, String)> = vec![
            ("day_number", day.to_string()),
            ("session", session.clone()),
            ("activity_id", activity_id.clone()),
            ("title", title.clone()),
            ("from.lat", bl),
            ("from.lon", bo),
            ("from.geo_source", bs),
            ("to.lat", al),
            ("to.lon", ao),
            ("to.geo_source", a_s),
        ];
        let event = if after.is_some() { "activity_geocode_set" } else { "activity_geocode_cleared" };
        crate::record::emit_record_event(
            &conn,
            &plan_id,
            &destination,
            event,
            &kv,
            version_before,
            "set-activity-geocode",
            &format!("D{day}/{session} {activity_id} pin {}", fmt_geo(&after)),
        )
        .await?;
        Ok((activity_id, Some((title, before, after))))
    }
    .await;
    let (activity_id, change) =
        crate::cascade::common::finish_tx(&conn, "set-activity-geocode", outcome).await?;
    match change {
        None => println!("ℹ️ No change (id={activity_id})"),
        Some((title, before, after)) => {
            println!("\n📍 Activity pin: D{day}/{session} {title}");
            println!("   pin: {} → {}", fmt_geo(&before), fmt_geo(&after));
            println!("✅ Pin updated (id={activity_id}); run snapshot-maps to refresh the map");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_set_and_clear() {
        let p = parse(&a(&["1", "morning", "川岩", "--lat", "25.1", "--lon", "121.8", "--source", "osm"])).unwrap();
        assert_eq!(p.action, Action::Set { lat: 25.1, lon: 121.8, source: Some("osm".into()) });
        let c = parse(&a(&["1", "morning", "川岩", "--clear"])).unwrap();
        assert_eq!(c.action, Action::Clear);
    }

    #[test]
    fn parse_rejects_bad_input() {
        // clear is exclusive with the value flags
        assert!(parse(&a(&["1", "morning", "x", "--clear", "--lat", "25.1", "--lon", "121.8"])).is_err());
        // pair
        assert!(parse(&a(&["1", "morning", "x", "--lat", "25.1"])).unwrap_err().contains("together"));
        assert!(parse(&a(&["1", "morning", "x"])).is_err());
        // shared validator: range, null island, NaN
        assert!(parse(&a(&["1", "morning", "x", "--lat", "91", "--lon", "121"])).unwrap_err().contains("between -90 and 90"));
        assert!(parse(&a(&["1", "morning", "x", "--lat", "0", "--lon", "0"])).unwrap_err().contains("null island"));
        assert!(parse(&a(&["1", "morning", "x", "--lat", "NaN", "--lon", "121"])).is_err());
        assert!(parse(&a(&["1", "morning", "x", "--lat", "north", "--lon", "121"])).unwrap_err().contains("decimal"));
        assert!(parse(&a(&["0", "morning", "x", "--clear"])).is_err());
        assert!(parse(&a(&["1", "dawn", "x", "--clear"])).is_err());
    }

    #[test]
    fn next_geo_idempotence() {
        let pin = Some(ActivityGeo { lat: 25.1, lon: 121.8, source: Some("osm".into()) });
        let same = Action::Set { lat: 25.1, lon: 121.8, source: Some("osm".into()) };
        assert_eq!(next_geo(&pin, &same), None);
        let same_no_src = Action::Set { lat: 25.1, lon: 121.8, source: None };
        assert_eq!(next_geo(&pin, &same_no_src), None);
        let moved = Action::Set { lat: 25.2, lon: 121.8, source: None };
        assert!(next_geo(&pin, &moved).is_some());
        let new_src = Action::Set { lat: 25.1, lon: 121.8, source: Some("map".into()) };
        assert!(next_geo(&pin, &new_src).is_some());
        assert!(next_geo(&None, &same).is_some());
        assert_eq!(next_geo(&None, &Action::Clear), None);
        assert_eq!(next_geo(&pin, &Action::Clear), Some(None));
    }
}
