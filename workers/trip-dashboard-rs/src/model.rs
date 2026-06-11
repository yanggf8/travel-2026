//! Typed plan model + assembly from decoded Turso rows.
use crate::turso::Row;
use serde_json::Value;

#[derive(Debug, Default, PartialEq)]
pub struct Stop { pub title: String, pub address: String, pub lat: Option<f64>, pub lon: Option<f64>, pub maps_link: String, pub cost_estimate: i64 }

#[derive(Debug, Default, PartialEq)]
pub struct RouteSegment {
    pub from_place: String,
    pub to_place: String,
    pub mode: String, // driving|transit|walking
    pub duration_min: i64,
    pub notes: String,
    pub start_time: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct Session {
    pub session_type: String, // morning|noon|afternoon|evening
    pub focus_zh: String,
    pub transit_zh: String,
    pub activities: Vec<String>,
    pub meals: Vec<String>,
    pub stops: Vec<Stop>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Day {
    pub day_number: i64,
    pub date: String,
    pub day_type: String,
    pub theme: String,
    pub theme_zh: String,
    pub weather_label: String,
    pub temp_low_c: Option<f64>,
    pub temp_high_c: Option<f64>,
    pub precipitation_pct: Option<f64>,
    pub feels_like_low_c: Option<f64>,
    pub feels_like_high_c: Option<f64>,
    pub sessions: Vec<Session>, // ALWAYS 4: morning, noon, afternoon, evening
    pub route_segments: Vec<RouteSegment>,
}

/// The canonical 4 sessions, in display order. This is what makes "noon" impossible to drop.
pub const SESSION_ORDER: [&str; 4] = ["morning", "noon", "afternoon", "evening"];

fn s(row: &Row, key: &str) -> String {
    row.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}
fn i(row: &Row, key: &str) -> i64 {
    row.get(key).and_then(|v| v.as_str()).and_then(|s| s.parse().ok())
        .or_else(|| row.get(key).and_then(|v| v.as_i64())).unwrap_or(0)
}
/// Optional float column. Turso returns REALs as strings; accept either.
/// Returns None when the column is absent or NULL.
fn f(row: &Row, key: &str) -> Option<f64> {
    row.get(key).and_then(json_f64)
}

/// Build the 4 sessions for one day from activity + meal rows already filtered to that day.
pub fn build_sessions(activities: &[Row], meals: &[Row]) -> Vec<Session> {
    SESSION_ORDER.iter().map(|&st| {
        Session {
            session_type: st.to_string(),
            activities: activities.iter().filter(|r| s(r, "session_type") == st)
                .map(|r| s(r, "title")).collect(),
            meals: meals.iter().filter(|r| s(r, "session_type") == st)
                .map(|r| s(r, "meal")).collect(),
            ..Default::default()
        }
    }).collect()
}

#[derive(Debug, Default)]
pub struct Plan {
    pub plan_id: String,
    pub display_name: String,
    pub start_date: String,
    pub end_date: String,
    pub days: Vec<Day>,
    pub flights: Vec<Row>,
    pub hotel: Option<Row>,
    pub transfers: Vec<Row>,
}

/// Assemble a Plan from the pipeline result vectors (query order defined in the router/loader).
/// Row slices MUST be pre-sorted by their sort_order in the SQL query — this
/// function preserves input order and does not re-sort.
pub fn assemble(
    plan_rows: &[Row], day_rows: &[Row], session_rows: &[Row],
    activity_rows: &[Row], meal_rows: &[Row], flight_rows: &[Row],
    hotel_rows: &[Row], transfer_rows: &[Row], poi_rows: &[Row],
    route_rows: &[Row],
) -> Plan {
    let mut plan = Plan::default();
    if let Some(p) = plan_rows.first() {
        plan.plan_id = s(p, "plan_id");
        plan.display_name = s(p, "display_name");
        plan.start_date = s(p, "start_date");
        plan.end_date = s(p, "end_date");
    }
    plan.flights = flight_rows.to_vec();
    plan.hotel = hotel_rows.first().cloned();
    plan.transfers = transfer_rows.to_vec();
    for d in day_rows {
        let dn = i(d, "day_number");
        let acts: Vec<Row> = activity_rows.iter().filter(|r| i(r, "day_number") == dn).cloned().collect();
        let mls: Vec<Row> = meal_rows.iter().filter(|r| i(r, "day_number") == dn).cloned().collect();
        let mut sessions = build_sessions(&acts, &mls);
        merge_session_meta(&mut sessions, session_rows, dn);
        attach_stops(&mut sessions, &acts, poi_rows);
        // Route rows arrive pre-sorted by sort_order; preserve that order.
        let route_segments: Vec<RouteSegment> = route_rows.iter()
            .filter(|r| i(r, "day_number") == dn)
            .map(|r| RouteSegment {
                from_place: s(r, "from_place"),
                to_place: s(r, "to_place"),
                mode: s(r, "mode"),
                duration_min: i(r, "duration_min"),
                notes: s(r, "notes"),
                start_time: s(r, "start_time"),
            })
            .collect();
        plan.days.push(Day {
            day_number: dn, date: s(d, "date"), day_type: s(d, "day_type"),
            theme: s(d, "theme"), theme_zh: s(d, "theme_zh"),
            weather_label: s(d, "weather_label"),
            temp_low_c: f(d, "temp_low_c"),
            temp_high_c: f(d, "temp_high_c"),
            precipitation_pct: f(d, "precipitation_pct"),
            feels_like_low_c: f(d, "feels_like_low_c"),
            feels_like_high_c: f(d, "feels_like_high_c"),
            sessions, route_segments,
        });
    }
    plan
}

fn merge_session_meta(sessions: &mut [Session], session_rows: &[Row], day_number: i64) {
    for sess in sessions.iter_mut() {
        if let Some(r) = session_rows.iter().find(|r| i(r, "day_number") == day_number && s(r, "session_type") == sess.session_type) {
            sess.focus_zh = s(r, "focus_zh");
            sess.transit_zh = s(r, "transit_notes_zh");
        }
    }
}

fn attach_stops(sessions: &mut [Session], acts: &[Row], poi_rows: &[Row]) {
    for sess in sessions.iter_mut() {
        for a in acts.iter().filter(|r| s(r, "session_type") == sess.session_type) {
            let title = s(a, "title");
            let nt = norm_title(&title);
            let poi = poi_rows.iter().find(|p| norm_title(&s(p, "title")) == nt);
            let lat = poi.and_then(|p| p.get("lat")).and_then(json_f64);
            let lon = poi.and_then(|p| p.get("lon")).and_then(json_f64);
            let maps_link = match (lat, lon) {
                (Some(la), Some(lo)) => format!("https://www.google.com/maps?q={la},{lo}"),
                _ => format!("https://www.google.com/maps/search/{}", urlencode(&title)),
            };
            sess.stops.push(Stop {
                title,
                address: poi.map(|p| s(p, "address")).unwrap_or_default(),
                lat, lon, maps_link,
                cost_estimate: poi.map(|p| i(p, "cost_estimate")).unwrap_or(0),
            });
        }
    }
}

fn json_f64(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}
/// Normalize a title for tolerant POI matching (trim + lowercase).
fn norm_title(t: &str) -> String { t.trim().to_lowercase() }
fn urlencode(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z'|b'a'..=b'z'|b'0'..=b'9'|b'-'|b'_'|b'.'|b'~' => (b as char).to_string(),
        b' ' => "%20".to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn row(pairs: &[(&str, Value)]) -> Row {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn noon_meal_is_not_dropped() {
        let acts = vec![row(&[("session_type", json!("noon")), ("title", json!("Makishi Market"))])];
        let meals = vec![row(&[("session_type", json!("noon")), ("meal", json!("Lunch: Makishi"))])];
        let sessions = build_sessions(&acts, &meals);
        assert_eq!(sessions.len(), 4);
        let noon = sessions.iter().find(|s| s.session_type == "noon").unwrap();
        assert_eq!(noon.activities, vec!["Makishi Market".to_string()]);
        assert_eq!(noon.meals, vec!["Lunch: Makishi".to_string()]);
    }

    #[test]
    fn always_four_sessions_in_order() {
        let sessions = build_sessions(&[], &[]);
        let order: Vec<_> = sessions.iter().map(|s| s.session_type.as_str()).collect();
        assert_eq!(order, vec!["morning", "noon", "afternoon", "evening"]);
    }

    #[test]
    fn stop_gets_maps_link_from_poi_latlon() {
        let acts = vec![row(&[("session_type", json!("morning")), ("title", json!("Naminoue Shrine"))])];
        let pois = vec![row(&[("title", json!("Naminoue Shrine")), ("lat", json!("26.2156")), ("lon", json!("127.6691")), ("address", json!("Naha"))])];
        let mut sessions = build_sessions(&acts, &[]);
        super::attach_stops(&mut sessions, &acts, &pois);
        let m = &sessions.iter().find(|s| s.session_type=="morning").unwrap().stops[0];
        assert_eq!(m.maps_link, "https://www.google.com/maps?q=26.2156,127.6691");
        assert_eq!(m.address, "Naha");
    }

    #[test]
    fn stop_without_poi_falls_back_to_search_link() {
        let acts = vec![row(&[("session_type", json!("morning")), ("title", json!("Mystery Spot"))])];
        let pois: Vec<Row> = vec![]; // no matching POI
        let mut sessions = build_sessions(&acts, &[]);
        super::attach_stops(&mut sessions, &acts, &pois);
        let m = &sessions.iter().find(|s| s.session_type == "morning").unwrap().stops[0];
        assert!(m.maps_link.contains("/maps/search/"));
        assert!(m.lat.is_none() && m.lon.is_none());
    }

    #[test]
    fn assemble_attaches_route_segments() {
        let plan_rows = vec![row(&[("plan_id", json!("okinawa-2026")), ("display_name", json!("Okinawa")), ("start_date", json!("2026-06-12")), ("end_date", json!("2026-06-16"))])];
        let day_rows = vec![row(&[("day_number", json!("2")), ("date", json!("2026-06-13"))])];
        let route_rows = vec![
            row(&[("day_number", json!("2")), ("from_place", json!("Hotel")), ("to_place", json!("Naminoue")), ("mode", json!("driving")), ("duration_min", json!("12")), ("notes", json!("")), ("start_time", json!("09:00"))]),
            row(&[("day_number", json!("1")), ("from_place", json!("Airport")), ("to_place", json!("Hotel")), ("mode", json!("transit")), ("duration_min", json!("30")), ("notes", json!("")), ("start_time", json!(""))]),
        ];
        let plan = assemble(&plan_rows, &day_rows, &[], &[], &[], &[], &[], &[], &[], &route_rows);
        let day = plan.days.iter().find(|d| d.day_number == 2).unwrap();
        assert_eq!(day.route_segments.len(), 1);
        let seg = &day.route_segments[0];
        assert_eq!(seg.from_place, "Hotel");
        assert_eq!(seg.to_place, "Naminoue");
        assert_eq!(seg.mode, "driving");
        assert_eq!(seg.duration_min, 12);
    }

    #[test]
    fn assemble_populates_weather_detail_from_day_row() {
        let plan_rows = vec![row(&[("plan_id", json!("okinawa-2026")), ("display_name", json!("Okinawa")), ("start_date", json!("2026-06-12")), ("end_date", json!("2026-06-16"))])];
        // Turso returns REALs as strings — assert we parse those.
        let day_rows = vec![row(&[
            ("day_number", json!("2")), ("date", json!("2026-06-13")),
            ("temp_low_c", json!("26.4")), ("temp_high_c", json!("30.1")),
            ("precipitation_pct", json!("73")),
            ("feels_like_low_c", json!("28.0")), ("feels_like_high_c", json!("34.2")),
        ])];
        let plan = assemble(&plan_rows, &day_rows, &[], &[], &[], &[], &[], &[], &[], &[]);
        let day = plan.days.iter().find(|d| d.day_number == 2).unwrap();
        assert_eq!(day.temp_low_c, Some(26.4));
        assert_eq!(day.temp_high_c, Some(30.1));
        assert_eq!(day.precipitation_pct, Some(73.0));
        assert_eq!(day.feels_like_low_c, Some(28.0));
        assert_eq!(day.feels_like_high_c, Some(34.2));
    }

    #[test]
    fn assemble_weather_detail_is_none_when_absent() {
        let plan_rows = vec![row(&[("plan_id", json!("okinawa-2026")), ("display_name", json!("Okinawa")), ("start_date", json!("2026-06-12")), ("end_date", json!("2026-06-16"))])];
        let day_rows = vec![row(&[("day_number", json!("1")), ("date", json!("2026-06-12"))])];
        let plan = assemble(&plan_rows, &day_rows, &[], &[], &[], &[], &[], &[], &[], &[]);
        let day = plan.days.iter().find(|d| d.day_number == 1).unwrap();
        assert_eq!(day.temp_low_c, None);
        assert_eq!(day.precipitation_pct, None);
        assert_eq!(day.feels_like_high_c, None);
    }

    #[test]
    fn stop_carries_poi_cost_estimate() {
        let acts = vec![row(&[("session_type", json!("morning")), ("title", json!("Shuri Castle"))])];
        let pois = vec![row(&[("title", json!("Shuri Castle")), ("lat", json!("26.2")), ("lon", json!("127.7")), ("cost_estimate", json!("400"))])];
        let mut sessions = build_sessions(&acts, &[]);
        super::attach_stops(&mut sessions, &acts, &pois);
        let m = &sessions.iter().find(|s| s.session_type == "morning").unwrap().stops[0];
        assert_eq!(m.cost_estimate, 400);
    }

    #[test]
    fn stop_cost_estimate_defaults_zero_without_poi() {
        let acts = vec![row(&[("session_type", json!("morning")), ("title", json!("Free Beach"))])];
        let pois: Vec<Row> = vec![];
        let mut sessions = build_sessions(&acts, &[]);
        super::attach_stops(&mut sessions, &acts, &pois);
        let m = &sessions.iter().find(|s| s.session_type == "morning").unwrap().stops[0];
        assert_eq!(m.cost_estimate, 0);
    }

    #[test]
    fn poi_match_tolerates_whitespace_and_case() {
        let acts = vec![row(&[("session_type", json!("morning")), ("title", json!("  Naminoue SHRINE "))])];
        let pois = vec![row(&[("title", json!("Naminoue Shrine")), ("lat", json!("26.2")), ("lon", json!("127.6"))])];
        let mut sessions = build_sessions(&acts, &[]);
        super::attach_stops(&mut sessions, &acts, &pois);
        let m = &sessions.iter().find(|s| s.session_type == "morning").unwrap().stops[0];
        assert_eq!(m.maps_link, "https://www.google.com/maps?q=26.2,127.6");
    }
}
