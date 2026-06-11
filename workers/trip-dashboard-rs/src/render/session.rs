use crate::model::Session;
use super::{esc, render_activity_text};

/// Render one session block. Empty sessions are caller-skipped (see day.rs).
pub fn render(sess: &Session, lang: &str) -> String {
    let label = match (sess.session_type.as_str(), lang) {
        ("morning", "zh") => "上午", ("noon", "zh") => "中午",
        ("afternoon", "zh") => "下午", ("evening", "zh") => "晚上",
        ("morning", _) => "Morning", ("noon", _) => "Noon",
        ("afternoon", _) => "Afternoon", _ => "Evening",
    };
    let mut h = String::new();
    h.push_str(&format!("<div class=\"session session-{}\"><div class=\"session-label\">{}</div>", esc(&sess.session_type), label));
    if !sess.focus_zh.is_empty() {
        h.push_str(&format!("<div class=\"session-focus\">{}</div>", esc(&sess.focus_zh)));
    }
    for a in &sess.activities {
        // render_activity_text (port of the JS worker's renderActivityText):
        // escapes, turns \n into <br>, and renders an embedded "Google Maps：<url>"
        // tail as a short labeled link instead of dumping the giant URL inline.
        h.push_str(&format!("<div class=\"activity\">{}</div>", render_activity_text(a)));
    }
    for m in &sess.meals {
        h.push_str(&format!("<div class=\"meal\">🍽️ {}</div>", esc(m)));
    }
    if !sess.transit_zh.is_empty() {
        h.push_str(&format!("<div class=\"transit\">🚃 {}</div>", esc(&sess.transit_zh)));
    }
    h.push_str("</div>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Session;
    #[test]
    fn noon_meal_renders() {
        let sess = Session { session_type: "noon".into(), meals: vec!["Lunch: Makishi".into()], ..Default::default() };
        let html = render(&sess, "zh");
        assert!(html.contains("中午"));
        assert!(html.contains("Lunch: Makishi"));
    }
    #[test]
    fn activity_ampersand_escaped_once() {
        let sess = Session { session_type: "morning".into(), activities: vec!["Museum & Art".into()], ..Default::default() };
        let html = render(&sess, "en");
        assert!(html.contains("Museum &amp; Art"));
        assert!(!html.contains("amp;amp;"));
    }
    #[test]
    fn activity_embedded_map_url_becomes_labeled_link() {
        // The okinawa bug: an activity title carrying an embedded "Google Maps：<url>"
        // tail must render as a short labeled link, not dump %0A / the raw URL.
        let sess = Session {
            session_type: "evening".into(),
            activities: vec!["晚餐：ステーキ88 — 牧志駅步行5分\nGoogle Maps：https://www.google.com/maps/search/abc".into()],
            ..Default::default()
        };
        let html = render(&sess, "zh");
        assert!(html.contains("🗺️ Google Maps"), "got: {html}");
        assert!(html.contains("<a href=\"https://www.google.com/maps/search/abc\""), "got: {html}");
        assert!(!html.contains("%0A"), "got: {html}");
    }
    #[test]
    fn activity_newline_becomes_br() {
        let sess = Session { session_type: "morning".into(), activities: vec!["line a\nline b".into()], ..Default::default() };
        let html = render(&sess, "en");
        assert!(html.contains("line a<br>line b"), "got: {html}");
    }
}
