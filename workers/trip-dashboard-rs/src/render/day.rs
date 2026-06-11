use crate::model::Day;
use super::{esc, session};

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
    use crate::model::{Day, Session};
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
