use crate::model::Session;
use super::{esc, esc_url_attr, render_activity_text, urlencode};

/// Render one meal line. Meals carry a map-pin convention
/// `"<label>｜map:<query>"` (full-width `｜` or ASCII `|`, case-insensitive on
/// `map:`). This is a port of the JS worker's `renderMealPill()`
/// (`workers/trip-dashboard/src/render.ts`), with one deliberate FIX:
///
/// The JS regex `/^(.*?)\s*[｜|]\s*map:\s*(.+)$/i` captures the query with a
/// GREEDY `(.+)$`, so a meal that also carries a TRAILING `｜備案：<text>`
/// (backup option) AFTER the map query — e.g.
/// `…｜map:ステーキ88 那覇 ｜備案：やっぱりステーキ` — has the 備案 tail swallowed
/// into the maps query, polluting the link target. Here the query stops at the
/// NEXT `｜`/`|` (or end of string); any trailing `｜備案：…` is kept as PLAIN
/// TEXT after the link. So the map link points at ONLY the place name.
///
/// With a pin → `<a class="meal" href="…maps/search/<query>">🍽️ <label> 🗺️</a>`
/// (label is the clickable text, NOT the raw query), optionally followed by the
/// trailing 備案 text as a plain `<span>`. Without a pin → the existing plain
/// `<div class="meal">🍽️ <meal></div>` form.
pub fn render_meal(meal: &str) -> String {
    match split_meal_pin(meal) {
        Some((label, query, trailing)) => {
            let url = format!("https://www.google.com/maps/search/{}", urlencode(&query));
            let mut h = format!(
                "<a class=\"meal\" href=\"{}\" target=\"_blank\" rel=\"noopener\">🍽️ {} 🗺️</a>",
                esc_url_attr(&url),
                esc(&label),
            );
            if !trailing.is_empty() {
                h.push_str(&format!("<span class=\"meal meal-alt\">{}</span>", esc(&trailing)));
            }
            h
        }
        None => format!("<div class=\"meal\">🍽️ {}</div>", esc(meal)),
    }
}

/// Parse `"<label>｜map:<query>[｜<trailing>]"`. Returns
/// `(label, query, trailing)` where `query` stops at the NEXT pipe (so a
/// `｜備案：…` tail does not pollute it) and `trailing` is that remainder kept
/// verbatim (including its leading pipe), or empty. `None` when there is no
/// `｜map:` / `|map:` marker. Case-insensitive on the `map:` keyword.
fn split_meal_pin(meal: &str) -> Option<(String, String, String)> {
    let c: Vec<char> = meal.chars().collect();
    // Find the first pipe (｜ or |) that is immediately followed (after optional
    // whitespace) by a case-insensitive `map:`. This is group-1's terminator.
    let mut i = 0;
    while i < c.len() {
        if is_pipe(c[i]) {
            // Skip whitespace after the pipe, then test for `map:`.
            let mut j = i + 1;
            while j < c.len() && c[j].is_whitespace() { j += 1; }
            if matches_map_kw(&c, j) {
                let label: String = c[..i].iter().collect();
                // query starts after `map:` (+ optional whitespace).
                let mut k = j + 4; // len("map:")
                while k < c.len() && c[k].is_whitespace() { k += 1; }
                // query runs until the NEXT pipe (or end) — this is the FIX.
                let q_start = k;
                while k < c.len() && !is_pipe(c[k]) { k += 1; }
                let query: String = c[q_start..k].iter().collect();
                let trailing: String = c[k..].iter().collect();
                return Some((
                    label.trim().to_string(),
                    query.trim().to_string(),
                    trailing.trim().to_string(),
                ));
            }
        }
        i += 1;
    }
    None
}

/// Full-width `｜` (U+FF5C) or ASCII `|`.
fn is_pipe(ch: char) -> bool { ch == '｜' || ch == '|' }

/// Case-insensitive `map:` at index `i`.
fn matches_map_kw(c: &[char], i: usize) -> bool {
    let kw = ['m', 'a', 'p', ':'];
    i + kw.len() <= c.len()
        && c[i..i + kw.len()].iter().zip(&kw).all(|(a, b)| a.eq_ignore_ascii_case(b))
}

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
        h.push_str(&format!("<div class=\"activity\">{}</div>", render_activity_text(&a.title)));
    }
    for m in &sess.meals {
        h.push_str(&render_meal(m));
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
    use crate::model::{Session, Activity};
    /// Build an Activity carrying only a display title (the common test case).
    fn act(title: &str) -> Activity { Activity { title: title.into(), ..Default::default() } }
    #[test]
    fn noon_meal_renders() {
        let sess = Session { session_type: "noon".into(), meals: vec!["Lunch: Makishi".into()], ..Default::default() };
        let html = render(&sess, "zh");
        assert!(html.contains("中午"));
        assert!(html.contains("Lunch: Makishi"));
    }
    #[test]
    fn activity_ampersand_escaped_once() {
        let sess = Session { session_type: "morning".into(), activities: vec![act("Museum & Art")], ..Default::default() };
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
            activities: vec![act("晚餐：ステーキ88 — 牧志駅步行5分\nGoogle Maps：https://www.google.com/maps/search/abc")],
            ..Default::default()
        };
        let html = render(&sess, "zh");
        assert!(html.contains("🗺️ Google Maps"), "got: {html}");
        assert!(html.contains("<a href=\"https://www.google.com/maps/search/abc\""), "got: {html}");
        assert!(!html.contains("%0A"), "got: {html}");
    }
    #[test]
    fn activity_newline_becomes_br() {
        let sess = Session { session_type: "morning".into(), activities: vec![act("line a\nline b")], ..Default::default() };
        let html = render(&sess, "en");
        assert!(html.contains("line a<br>line b"), "got: {html}");
    }

    // ---- meal map-pin rendering (port of renderMealPill, FIX 備案 over-capture) ----

    // A meal with a ｜map:<query> pin renders the LABEL as a clickable maps link;
    // the raw "｜map:…" convention text must NOT appear.
    #[test]
    fn meal_pin_renders_clickable_label_not_raw_query() {
        let out = render_meal("晚餐：ステーキ88｜map:ステーキ88 那覇");
        assert!(out.contains("<a class=\"meal\" href=\""), "got: {out}");
        assert!(out.contains("maps/search/"), "got: {out}");
        // Visible clickable text is the LABEL (no raw "map:" / the query blob).
        assert!(out.contains("🍽️ 晚餐：ステーキ88 🗺️"), "got: {out}");
        // The convention marker is gone from the rendered output.
        assert!(!out.contains("｜map:"), "got: {out}");
        assert!(!out.contains("map:ステーキ"), "got: {out}");
        // href encodes the place name (那覇 → %-encoded UTF-8), not the raw label.
        assert!(out.contains("%E9%82%A3%E8%A6%87"), "那覇 encoded missing; got: {out}");
    }

    // The 備案 over-capture FIX: with a trailing ｜備案：<backup>, the maps query is
    // ONLY the place name (stops at the next pipe); 備案 stays as plain text.
    #[test]
    fn meal_pin_trailing_alt_is_not_swallowed_into_query() {
        let out = render_meal("晚餐：A｜map:A 那覇 ｜備案：B");
        // Link present, label clickable.
        assert!(out.contains("<a class=\"meal\" href=\""), "got: {out}");
        assert!(out.contains("🍽️ 晚餐：A 🗺️"), "got: {out}");
        // The query is "A 那覇" only — encoded as "A%20" + 那覇 — and does NOT
        // contain the 備案 backup text nor a second encoded pipe.
        // 備案 chars (備=%E5%82%99 案=%E6%A1%88) must NOT appear in the href query.
        let href = &out[..out.find("\" target").unwrap_or(out.len())];
        assert!(!href.contains("%E5%82%99"), "備 leaked into query; got: {out}");
        assert!(!href.contains("%E6%A1%88"), "案 leaked into query; got: {out}");
        // 備案：B is still shown to the user as plain (escaped) text after the link.
        assert!(out.contains("備案：B"), "got: {out}");
        // The convention marker itself is gone.
        assert!(!out.contains("map:A"), "got: {out}");
    }

    // A meal with NO pin → plain div pill, no <a>, no panic.
    #[test]
    fn meal_without_pin_is_plain_pill() {
        let out = render_meal("Hotel breakfast");
        assert_eq!(out, "<div class=\"meal\">🍽️ Hotel breakfast</div>");
        assert!(!out.contains("<a "), "got: {out}");
    }

    // esc/esc_url_attr: a meal label with &/< escapes ONCE in the visible text;
    // the href preserves the query via esc_url_attr (no double-escape).
    #[test]
    fn meal_pin_escapes_label_and_preserves_href() {
        let out = render_meal("Diner & <Café>｜map:Café & Bar 那覇");
        // Visible label escaped once.
        assert!(out.contains("Diner &amp; &lt;Café&gt;"), "got: {out}");
        assert!(!out.contains("amp;amp;"), "double-escape; got: {out}");
        // href present and well-formed (esc_url_attr neutralizes < > " but the
        // query was percent-encoded first, so the place name survives).
        assert!(out.contains("<a class=\"meal\" href=\"https://www.google.com/maps/search/"), "got: {out}");
        // The ampersand in the query was percent-encoded (%26), not left raw.
        let href_part = &out[..out.find("\" target").unwrap_or(out.len())];
        assert!(href_part.contains("%26"), "& not encoded in query; got: {out}");
    }

    // Integration: render() wires render_meal into the session block; a pinned
    // meal yields a meal link in the session HTML and no raw ｜map: text.
    #[test]
    fn session_render_uses_meal_links() {
        let sess = Session {
            session_type: "evening".into(),
            meals: vec!["晚餐：ステーキ88｜map:ステーキ88 那覇".into()],
            ..Default::default()
        };
        let html = render(&sess, "zh");
        assert!(html.contains("<a class=\"meal\" href=\""), "got: {html}");
        assert!(html.contains("maps/search/"), "got: {html}");
        assert!(!html.contains("｜map:"), "got: {html}");
    }
}
