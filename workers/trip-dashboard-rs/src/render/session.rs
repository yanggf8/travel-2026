use crate::model::Session;
use super::esc;

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
        h.push_str(&format!("<div class=\"activity\">{}</div>", esc(a)));
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
}
