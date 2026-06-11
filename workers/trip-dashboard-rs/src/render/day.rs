use crate::model::{Day, RouteSegment};
use super::{esc, session};

/// Icon for a route-segment travel mode.
fn mode_icon(mode: &str) -> &'static str {
    match mode {
        "driving" => "🚗",
        "walking" => "🚶",
        _ => "🚌", // transit + any unknown/default
    }
}

/// Render the per-day "今日路線" (today's route) block from door-to-door segments.
fn render_route_block(segments: &[RouteSegment], lang: &str) -> String {
    let heading = if lang == "en" { "Today's route" } else { "今日路線" };
    let mut h = String::new();
    h.push_str(&format!("<div class=\"route-block\"><div class=\"route-heading\">{heading}</div>"));
    for seg in segments {
        h.push_str("<div class=\"route-seg\">");
        h.push_str(&format!("<span class=\"route-mode\">{}</span> ", mode_icon(&seg.mode)));
        h.push_str(&format!("{} → {}", esc(&seg.from_place), esc(&seg.to_place)));
        if !seg.start_time.is_empty() {
            h.push_str(&format!(" · {}", esc(&seg.start_time)));
        }
        h.push_str(&format!(" · ~{} min", seg.duration_min));
        if !seg.notes.is_empty() {
            h.push_str(&format!(" · {}", esc(&seg.notes)));
        }
        h.push_str("</div>");
    }
    h.push_str("</div>");
    h
}

pub fn render(day: &Day, plan_id: &str, lang: &str) -> String {
    let theme = if lang == "zh" && !day.theme_zh.is_empty() { &day.theme_zh } else { &day.theme };
    let mut h = String::new();
    h.push_str(&format!("<section class=\"day day-{}\">", esc(&day.day_type)));
    h.push_str(&format!("<h2>Day {} · {}</h2>", day.day_number, esc(&day.date)));
    h.push_str(&format!("<div class=\"theme\">{}</div>", esc(theme)));
    if !day.weather_label.is_empty() {
        h.push_str(&format!("<div class=\"weather\">🌧️ {}</div>", esc(&day.weather_label)));
    }
    h.push_str(&super::map::day_map_img(plan_id, day.day_number));
    if !day.route_segments.is_empty() {
        h.push_str(&render_route_block(&day.route_segments, lang));
    }
    for sess in &day.sessions {
        // skip wholly-empty sessions to avoid 4 empty boxes on a light day
        if sess.activities.is_empty() && sess.meals.is_empty() && sess.focus_zh.is_empty() { continue; }
        h.push_str(&session::render(sess, lang));
        h.push_str(&super::map::stop_list(&sess.stops));
    }
    h.push_str("</section>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Day, Session, RouteSegment};
    #[test]
    fn renders_route_block() {
        let day = Day {
            day_number: 2, date: "2026-06-13".into(), day_type: "full".into(),
            route_segments: vec![RouteSegment {
                from_place: "HOTEL AZAT NAHA".into(),
                to_place: "波上宮".into(),
                mode: "driving".into(),
                duration_min: 12,
                notes: "".into(),
                start_time: "09:00".into(),
            }],
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh");
        assert!(html.contains("今日路線"));
        assert!(html.contains("HOTEL AZAT NAHA"));
        assert!(html.contains("波上宮"));
        assert!(html.contains("🚗"));
        assert!(html.contains("~12 min"));
    }
    #[test]
    fn renders_zh_theme_and_sessions() {
        let day = Day {
            day_number: 3, date: "2026-06-14".into(), day_type: "full".into(),
            theme_zh: "壺屋陶器街".into(),
            sessions: vec![Session{ session_type:"noon".into(), meals:vec!["Lunch".into()], ..Default::default()}],
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh");
        assert!(html.contains("壺屋陶器街"));
        assert!(html.contains("中午"));
        assert!(html.contains("Lunch"));
    }
    #[test]
    fn empty_session_is_skipped() {
        let day = Day {
            day_number: 1, date: "2026-06-12".into(), day_type: "arrival".into(),
            sessions: vec![
                Session{ session_type:"morning".into(), activities: vec!["Arrive".into()], ..Default::default()},
                Session{ session_type:"noon".into(), ..Default::default()}, // empty → skipped
            ],
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "en");
        assert!(html.contains("Arrive"));
        // the empty noon session should NOT emit a session block
        assert!(!html.contains("session-noon"));
    }
    #[test]
    fn day_includes_day_map_image() {
        let day = Day { day_number: 2, date: "2026-06-13".into(), day_type: "full".into(), ..Default::default() };
        let html = render(&day, "okinawa-2026", "en");
        assert!(html.contains("/map/okinawa-2026/day-2.png"));
    }
}
