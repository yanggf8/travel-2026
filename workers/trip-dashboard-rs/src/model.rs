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

/// One itinerary activity row, carrying the booking fields the pending-alert
/// renderer needs. `title` is the (possibly multi-line) display text; the
/// booking_* fields drive the pending-booking alerts (feature #3).
#[derive(Debug, Default, PartialEq, Clone)]
pub struct Activity {
    pub title: String,
    pub booking_status: String,
    pub book_by: String,
    pub booking_url: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct Session {
    pub session_type: String, // morning|noon|afternoon|evening
    pub focus_zh: String,
    pub transit_zh: String,
    pub activities: Vec<Activity>,
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
                .map(|r| Activity {
                    title: s(r, "title"),
                    booking_status: s(r, "booking_status"),
                    book_by: s(r, "book_by"),
                    booking_url: s(r, "booking_url"),
                }).collect(),
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
    // ---- transit cheat-sheet (feature #4) ----
    /// `transit_hotel_station` / `_zh` from itinerary_metadata (home base).
    pub transit_hotel_station: String,
    pub transit_hotel_station_zh: String,
    /// `(destination, lang, line)` rows from itinerary_transit_key_lines, in
    /// SQL sort_order. Keyed at render time by `dest:lang`.
    pub transit_key_lines: Vec<(String, String, String)>,
}

/// Assemble a Plan from the pipeline result vectors (query order defined in the router/loader).
/// Row slices MUST be pre-sorted by their sort_order in the SQL query — this
/// function preserves input order and does not re-sort.
pub fn assemble(
    plan_rows: &[Row], day_rows: &[Row], session_rows: &[Row],
    activity_rows: &[Row], meal_rows: &[Row], flight_rows: &[Row],
    hotel_rows: &[Row], transfer_rows: &[Row], poi_rows: &[Row],
    route_rows: &[Row], transit_key_line_rows: &[Row], itin_meta_rows: &[Row],
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
    // ---- transit cheat-sheet (feature #4) ----
    if let Some(m) = itin_meta_rows.first() {
        plan.transit_hotel_station = s(m, "transit_hotel_station");
        plan.transit_hotel_station_zh = s(m, "transit_hotel_station_zh");
    }
    plan.transit_key_lines = transit_key_line_rows.iter()
        .map(|r| (s(r, "destination"), s(r, "lang"), s(r, "line")))
        .collect();
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
            // Prefer the durable poi_id FK; fall back to a normalized title
            // match only when the activity has no poi_id set. This keeps
            // unlinked activities working by title and makes linked ones robust
            // to title drift (e.g. "Shurijo Castle Park" vs POI "Shuri Castle").
            let act_poi_id = s(a, "poi_id");
            let poi = if !act_poi_id.is_empty() {
                poi_rows.iter().find(|p| s(p, "poi_id") == act_poi_id)
            } else {
                let nt = norm_title(&title);
                poi_rows.iter().find(|p| norm_title(&s(p, "title")) == nt)
            };
            let lat = poi.and_then(|p| p.get("lat")).and_then(json_f64);
            let lon = poi.and_then(|p| p.get("lon")).and_then(json_f64);
            let maps_link = match (lat, lon) {
                // Clean coord link — always correct, leave untouched.
                (Some(la), Some(lo)) => format!("https://www.google.com/maps?q={la},{lo}"),
                // No POI coords → search-link fallback. But the title is often a
                // MULTI-LINE blob with an embedded "Google Maps：<url>" tail; encoding
                // the whole blob produces a garbage maps/search/<%0A…nested-url…> link.
                // If the text already carries an embedded maps URL, the inline labeled
                // link (render_activity_text) covers it — emit NO stop search link.
                // Otherwise search on a CLEAN short venue name (first line, trimmed).
                _ if has_embedded_maps_url(&title) => String::new(),
                // The activity is not a geocodable place (a flight line, an
                // airport-process step, a transit instruction, a bare meal label,
                // a bare time) → emit NO search link; render/map.rs renders an
                // empty maps_link as plain text. This mirrors the TS worker, which
                // never search-links a bare title, and the CLI's stop_link_problem
                // guard (rust/crates/travel-cli/src/checks.rs).
                _ if !is_linkable_stop(&title) => String::new(),
                _ => format!("https://www.google.com/maps/search/{}", crate::render::urlencode(&search_query_from_title(&title))),
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

/// True when the activity title already embeds a Google-Maps URL (the
/// multi-line "…\nGoogle Maps：https://…maps…" pattern). In that case the inline
/// labeled link from render_activity_text already gives the user a map link, so
/// the stop list must NOT also emit a (broken) maps/search/<whole-blob> link.
fn has_embedded_maps_url(title: &str) -> bool {
    let lower = title.to_lowercase();
    let has_url = lower.contains("https://") || lower.contains("http://");
    has_url && (lower.contains("maps.google") || lower.contains("google.com/maps") || lower.contains("/maps/"))
}

/// Derive a short, clean search query from a possibly-multi-line activity title:
/// True when an activity title is plausibly a geocodable PLACE worth a
/// `/maps/search/` link. False for non-places — flight lines, airport-process
/// steps, transit instructions, bare meal labels, bare times — which would
/// otherwise produce garbage pins (`/maps/search/CI120 起飛...`). Mirrors the TS
/// worker (never search-links a bare title) and the CLI's `stop_link_problem`
/// (rust/crates/travel-cli/src/checks.rs). Checks the cleaned venue query, so
/// `晚餐：リウボウ 安里店` (a real venue) still links while bare `晚餐` does not.
fn is_linkable_stop(title: &str) -> bool {
    let first = title.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if first.is_empty() {
        return false;
    }
    // Transit instruction (a leg, not a place): "A → B", "A -> B".
    if first.contains('→') || first.contains("->") {
        return false;
    }
    // Flight line: an airline code (2 letters + 3-4 digits, e.g. CI120) anywhere,
    // or a boarding/departure verb.
    if has_flight_code(first)
        || first.contains("起飛") || first.contains("起降") || first.contains("登機")
    {
        return false;
    }
    // Airport / travel-process step (not a destination).
    const PROCESS: [&str; 9] = [
        "報到", "託運", "出境", "入境", "安檢", "航廈", "候機", "接駁", "停車場",
    ];
    if PROCESS.iter().any(|w| first.contains(w)) {
        return false;
    }
    // What would actually be searched: if cleaning leaves nothing (bare meal
    // label, bare time, mode word) it can't geocode.
    let q = search_query_from_title(title);
    let q = q.trim();
    if q.is_empty() {
        return false;
    }
    // Bare meal label with no venue after it ("晚餐", "Lunch") — search_query
    // only strips the prefix when a "：/:" follows, so a lone label survives as
    // the whole query. It can't geocode.
    if is_meal_prefix(q) {
        return false;
    }
    // Bare time ("04:00", "14:30") with nothing place-like after.
    if is_bare_time(q) {
        return false;
    }
    // Mode-only word (port of stop_link_problem MODE_ONLY).
    const MODE_ONLY: [&str; 6] = ["步行", "單軌", "單軌電車", "巴士", "電車", "接駁巴士"];
    if MODE_ONLY.contains(&q) {
        return false;
    }
    true
}

/// True when `s` contains an airline-style code: 2 ASCII letters then 3–4 digits
/// (CI120, BR123, JX1234), as a standalone token at the start of a word.
fn has_flight_code(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // token boundary: start of string or after a space
        let at_boundary = i == 0 || bytes[i - 1] == b' ';
        if at_boundary
            && i + 2 < bytes.len()
            && bytes[i].is_ascii_alphabetic()
            && bytes[i + 1].is_ascii_alphabetic()
        {
            let digits = bytes[i + 2..].iter().take_while(|b| b.is_ascii_digit()).count();
            if (3..=4).contains(&digits) {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// True when the trimmed string is JUST a clock time ("4:00", "04:00", "14:30").
fn is_bare_time(s: &str) -> bool {
    let s = s.trim();
    let mut parts = s.split(':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(h), Some(m), None) => {
            !h.is_empty() && h.len() <= 2 && h.bytes().all(|b| b.is_ascii_digit())
                && m.len() == 2 && m.bytes().all(|b| b.is_ascii_digit())
        }
        _ => false,
    }
}

/// take the FIRST line, drop any leading "晚餐：/午餐：/…" meal prefix, and cut a
/// trailing " — …" / "（…）" descriptor so the maps/search query is just the venue.
fn search_query_from_title(title: &str) -> String {
    // First non-empty line only.
    let first_line = title.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    // Drop a leading "<meal label>：" prefix (晚餐：/午餐：/早餐：/Lunch:/Dinner:).
    let after_prefix = match first_line.find(['：', ':']) {
        Some(idx) if idx <= 12 && is_meal_prefix(&first_line[..idx]) => first_line[idx + first_line[idx..].chars().next().map_or(1, |c| c.len_utf8())..].trim(),
        _ => first_line,
    };
    // Cut at the first descriptor separator: " — " (em dash) or "（" (full-width paren).
    let mut q = after_prefix;
    if let Some(idx) = q.find(" — ") { q = q[..idx].trim_end(); }
    if let Some(idx) = q.find('（') { q = q[..idx].trim_end(); }
    if let Some(idx) = q.find(" (") { q = q[..idx].trim_end(); }
    q.trim().to_string()
}

/// Recognize a short leading label as a meal prefix to strip (so the venue name
/// — not "晚餐" — becomes the search query). Kept deliberately small/specific.
fn is_meal_prefix(label: &str) -> bool {
    matches!(label.trim(), "晚餐" | "午餐" | "早餐" | "宵夜" | "下午茶"
        | "Lunch" | "Dinner" | "Breakfast" | "Brunch" | "lunch" | "dinner" | "breakfast")
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
        assert_eq!(noon.activities.len(), 1);
        assert_eq!(noon.activities[0].title, "Makishi Market".to_string());
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

    // The okinawa bug: an activity with a MULTI-LINE title carrying an embedded
    // maps URL must NOT produce a maps/search/<whole-blob> link (which contained
    // %0A for the newline and a nested https%3A for the embedded URL). The inline
    // labeled link from render_activity_text covers it instead → no stop link.
    #[test]
    fn stop_with_embedded_maps_url_emits_no_search_link() {
        let blob = "晚餐：ステーキ88 — 牧志駅步行5分\nGoogle Maps：https://www.google.com/maps/search/abc";
        let acts = vec![row(&[("session_type", json!("evening")), ("title", json!(blob))])];
        let pois: Vec<Row> = vec![]; // no POI coords → would otherwise fall back to search
        let mut sessions = build_sessions(&acts, &[]);
        super::attach_stops(&mut sessions, &acts, &pois);
        let m = &sessions.iter().find(|s| s.session_type == "evening").unwrap().stops[0];
        assert_eq!(m.maps_link, "", "embedded-URL stop must suppress the broken search fallback");
        assert!(!m.maps_link.contains("%0A"));
        assert!(!m.maps_link.contains("https%3A"));
    }

    // A multi-line title WITHOUT an embedded maps URL still gets a search link,
    // but on a CLEAN short venue name (first line, meal-prefix + descriptor cut)
    // — never the whole blob with %0A.
    #[test]
    fn stop_search_query_is_clean_first_line() {
        let blob = "晚餐：安里家（アグー豚しゃぶ）— 飯店步行5分\n營業：週五 17:00–23:00";
        let acts = vec![row(&[("session_type", json!("evening")), ("title", json!(blob))])];
        let pois: Vec<Row> = vec![];
        let mut sessions = build_sessions(&acts, &[]);
        super::attach_stops(&mut sessions, &acts, &pois);
        let m = &sessions.iter().find(|s| s.session_type == "evening").unwrap().stops[0];
        assert!(m.maps_link.contains("/maps/search/"), "got: {}", m.maps_link);
        assert!(!m.maps_link.contains("%0A"), "no newline in query, got: {}", m.maps_link);
        // The meal prefix "晚餐" and the descriptor/2nd line are gone; venue remains.
        assert!(!m.maps_link.contains("%E6%99%9A%E9%A4%90"), "meal prefix 晚餐 stripped, got: {}", m.maps_link);
        let decoded = m.maps_link.replace("/maps/search/", "");
        assert!(!decoded.contains("E7%87%9F%E6%A5%AD"), "2nd line 營業 dropped, got: {}", m.maps_link);
    }

    #[test]
    fn search_query_helper_strips_meal_prefix_and_descriptor() {
        assert_eq!(super::search_query_from_title("晚餐：安里家（アグー）— 飯店步行5分"), "安里家");
        assert_eq!(super::search_query_from_title("首里そば\nLine2"), "首里そば");
        assert_eq!(super::search_query_from_title("Lunch: Makishi Market — open till 5"), "Makishi Market");
        assert_eq!(super::search_query_from_title("Plain Venue"), "Plain Venue");
    }

    #[test]
    fn is_linkable_stop_rejects_non_places_keeps_venues() {
        use super::is_linkable_stop as ok;
        // Real okinawa-2026 day-1 NON-places (must NOT link):
        assert!(!ok("CI120 起飛 TPE T2 08:00 → OKA T1 10:45"), "flight line");
        assert!(!ok("T2 報到、託運、出境安檢（建議起飛前2.5–3小時抵達）"), "airport process");
        assert!(!ok("停車場接駁巴士 → 桃園機場第二航廈（T2，約10–15分）"), "transit instruction");
        assert!(!ok("晚餐"), "bare meal label");
        assert!(!ok("04:00"), "bare time");
        assert!(!ok("04:00 自家出發開車：紅樹林 → 大園出國停車場第三停車場"), "instruction w/ arrow");
        assert!(!ok("步行"), "mode-only word");
        assert!(!ok("接駁巴士"), "mode-only word");
        // Real places / venues (must STILL link):
        assert!(ok("首里城公園"), "real place");
        assert!(ok("晚餐：リウボウ 安里店"), "meal prefix + real venue");
        assert!(ok("識名園"), "real place");
        assert!(ok("HOTEL AZAT NAHA"), "hotel — letters but no flight-code digit pattern");
        assert!(ok("Makishi Market"), "english venue");
    }
    #[test]
    fn flight_code_and_bare_time_helpers() {
        assert!(super::has_flight_code("CI120 起飛"));
        assert!(super::has_flight_code("foo BR1234 bar"));
        assert!(!super::has_flight_code("HOTEL AZAT NAHA")); // letters, no NN+digits token
        assert!(!super::has_flight_code("T2 報到"));
        assert!(super::is_bare_time("04:00"));
        assert!(super::is_bare_time("14:30"));
        assert!(!super::is_bare_time("首里城"));
        assert!(!super::is_bare_time("04:00 出發")); // has more than the time
    }

    #[test]
    fn has_embedded_maps_url_detects_google_maps() {
        assert!(super::has_embedded_maps_url("foo\nGoogle Maps：https://www.google.com/maps/search/x"));
        assert!(super::has_embedded_maps_url("導航：https://maps.google.com/?q=1,2"));
        assert!(!super::has_embedded_maps_url("just a venue name"));
        assert!(!super::has_embedded_maps_url("see https://example.com/foo")); // url but not maps
    }

    #[test]
    fn assemble_attaches_route_segments() {
        let plan_rows = vec![row(&[("plan_id", json!("okinawa-2026")), ("display_name", json!("Okinawa")), ("start_date", json!("2026-06-12")), ("end_date", json!("2026-06-16"))])];
        let day_rows = vec![row(&[("day_number", json!("2")), ("date", json!("2026-06-13"))])];
        let route_rows = vec![
            row(&[("day_number", json!("2")), ("from_place", json!("Hotel")), ("to_place", json!("Naminoue")), ("mode", json!("driving")), ("duration_min", json!("12")), ("notes", json!("")), ("start_time", json!("09:00"))]),
            row(&[("day_number", json!("1")), ("from_place", json!("Airport")), ("to_place", json!("Hotel")), ("mode", json!("transit")), ("duration_min", json!("30")), ("notes", json!("")), ("start_time", json!(""))]),
        ];
        let plan = assemble(&plan_rows, &day_rows, &[], &[], &[], &[], &[], &[], &[], &route_rows, &[], &[]);
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
        let plan = assemble(&plan_rows, &day_rows, &[], &[], &[], &[], &[], &[], &[], &[], &[], &[]);
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
        let plan = assemble(&plan_rows, &day_rows, &[], &[], &[], &[], &[], &[], &[], &[], &[], &[]);
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

    // The core fix: an activity carrying poi_id matches the POI by ID and gets
    // its ¥400 price + pin EVEN THOUGH its title diverges from the POI title.
    #[test]
    fn stop_matches_poi_by_id_despite_title_drift() {
        let acts = vec![row(&[
            ("session_type", json!("morning")),
            // Title intentionally != POI title (the Shuri gap).
            ("title", json!("Shurijo Castle Park (首里城公園) — reconstruction grounds")),
            ("poi_id", json!("shuri_castle")),
        ])];
        let pois = vec![row(&[
            ("poi_id", json!("shuri_castle")),
            ("title", json!("Shuri Castle (首里城)")),
            ("lat", json!("26.217")),
            ("lon", json!("127.719")),
            ("cost_estimate", json!("400")),
        ])];
        let mut sessions = build_sessions(&acts, &[]);
        super::attach_stops(&mut sessions, &acts, &pois);
        let m = &sessions.iter().find(|s| s.session_type == "morning").unwrap().stops[0];
        assert_eq!(m.cost_estimate, 400, "linked activity must inherit the POI price by id");
        assert_eq!(m.maps_link, "https://www.google.com/maps?q=26.217,127.719");
    }

    // An activity with NO poi_id still matches by normalized title (fallback).
    #[test]
    fn stop_without_poi_id_falls_back_to_title_match() {
        let acts = vec![row(&[
            ("session_type", json!("morning")),
            ("title", json!("Naminoue Shrine")),
            // poi_id absent (NULL) → title fallback path.
        ])];
        let pois = vec![row(&[
            ("poi_id", json!("naminoue")),
            ("title", json!("Naminoue Shrine")),
            ("cost_estimate", json!("0")),
            ("address", json!("Naha")),
        ])];
        let mut sessions = build_sessions(&acts, &[]);
        super::attach_stops(&mut sessions, &acts, &pois);
        let m = &sessions.iter().find(|s| s.session_type == "morning").unwrap().stops[0];
        assert_eq!(m.address, "Naha", "unlinked activity still matches by title");
    }

    // build_sessions populates the full Activity (title + booking fields), not
    // just a title string — pending-alert rendering (feature #3) reads these.
    #[test]
    fn build_sessions_populates_activity_booking_fields() {
        let acts = vec![row(&[
            ("session_type", json!("morning")),
            ("title", json!("Churaumi Aquarium")),
            ("booking_status", json!("pending")),
            ("book_by", json!("2026-05-01")),
            ("booking_url", json!("https://book.example/churaumi")),
        ])];
        let sessions = build_sessions(&acts, &[]);
        let m = sessions.iter().find(|s| s.session_type == "morning").unwrap();
        assert_eq!(m.activities.len(), 1);
        let a = &m.activities[0];
        assert_eq!(a.title, "Churaumi Aquarium");
        assert_eq!(a.booking_status, "pending");
        assert_eq!(a.book_by, "2026-05-01");
        assert_eq!(a.booking_url, "https://book.example/churaumi");
    }

    // Activity booking fields default to empty strings when columns are absent.
    #[test]
    fn build_sessions_activity_defaults_empty_when_columns_absent() {
        let acts = vec![row(&[("session_type", json!("noon")), ("title", json!("Free walk"))])];
        let sessions = build_sessions(&acts, &[]);
        let a = &sessions.iter().find(|s| s.session_type == "noon").unwrap().activities[0];
        assert_eq!(a.title, "Free walk");
        assert_eq!(a.booking_status, "");
        assert_eq!(a.book_by, "");
        assert_eq!(a.booking_url, "");
    }

    // ---- transit cheat-sheet assembly (feature #4) ----
    #[test]
    fn assemble_populates_transit_station_and_key_lines() {
        let plan_rows = vec![row(&[("plan_id", json!("okinawa-2026")), ("display_name", json!("Okinawa")), ("start_date", json!("2026-06-12")), ("end_date", json!("2026-06-16"))])];
        let itin_meta = vec![row(&[
            ("transit_hotel_station", json!("Asato Station")),
            ("transit_hotel_station_zh", json!("安里站")),
        ])];
        let key_lines = vec![
            row(&[("destination", json!("okinawa_2026")), ("lang", json!("en")), ("line", json!("Yui Rail: airport - Asato"))]),
            row(&[("destination", json!("okinawa_2026")), ("lang", json!("zh")), ("line", json!("單軌電車：機場－安里"))]),
        ];
        let plan = assemble(&plan_rows, &[], &[], &[], &[], &[], &[], &[], &[], &[], &key_lines, &itin_meta);
        assert_eq!(plan.transit_hotel_station, "Asato Station");
        assert_eq!(plan.transit_hotel_station_zh, "安里站");
        assert_eq!(plan.transit_key_lines.len(), 2);
        assert_eq!(plan.transit_key_lines[0], ("okinawa_2026".into(), "en".into(), "Yui Rail: airport - Asato".into()));
        assert_eq!(plan.transit_key_lines[1].1, "zh");
    }

    // No itinerary_metadata + no key lines → empty transit fields.
    #[test]
    fn assemble_transit_empty_when_absent() {
        let plan_rows = vec![row(&[("plan_id", json!("okinawa-2026")), ("display_name", json!("Okinawa")), ("start_date", json!("2026-06-12")), ("end_date", json!("2026-06-16"))])];
        let plan = assemble(&plan_rows, &[], &[], &[], &[], &[], &[], &[], &[], &[], &[], &[]);
        assert_eq!(plan.transit_hotel_station, "");
        assert_eq!(plan.transit_hotel_station_zh, "");
        assert!(plan.transit_key_lines.is_empty());
    }

    // A wrong poi_id must NOT silently fall back to a title match — the id is
    // authoritative once set (avoids attaching the wrong price).
    #[test]
    fn stop_with_poi_id_does_not_fall_back_to_title() {
        let acts = vec![row(&[
            ("session_type", json!("morning")),
            ("title", json!("Naminoue Shrine")),
            ("poi_id", json!("does_not_exist")),
        ])];
        let pois = vec![row(&[
            ("poi_id", json!("naminoue")),
            ("title", json!("Naminoue Shrine")),
            ("cost_estimate", json!("999")),
        ])];
        let mut sessions = build_sessions(&acts, &[]);
        super::attach_stops(&mut sessions, &acts, &pois);
        let m = &sessions.iter().find(|s| s.session_type == "morning").unwrap().stops[0];
        assert_eq!(m.cost_estimate, 0, "an unmatched poi_id must not borrow the title-match price");
    }
}
