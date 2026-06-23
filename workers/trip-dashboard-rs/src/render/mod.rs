pub mod auth;
pub mod session;
pub mod day;
pub mod map;
pub mod summary;
pub mod index;
pub mod activity_text;
pub mod alerts;
pub use activity_text::render_activity_text;
use crate::model::Plan;

/// Wrap a rendered body in the full HTML page shell: charset, mobile viewport,
/// `notranslate` (the ZH content must not be browser-auto-translated), and the
/// inlined stylesheet.
pub fn page(title: &str, body: &str, lang: &str) -> String {
    let lang_attr = if lang == "en" { "en" } else { "zh-TW" };
    format!(
        "<!doctype html><html lang=\"{lang_attr}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"google\" content=\"notranslate\">\
         <title>{}</title><style>{}</style></head><body>{}</body></html>",
        esc(title), crate::styles::CSS, body,
    )
}

/// Render a full plan page: booking summary, plan map, then each day card.
/// `token` is the access token the page was loaded with — threaded into the
/// auth-gated voucher link so a click carries the same token (else 403).
pub fn render_plan(plan: &Plan, lang: &str, token: Option<&str>) -> String {
    let mut body = String::new();
    // Non-meal pending-booking alerts BEFORE the summary (mirror render.ts:1388).
    body.push_str(&alerts::render_pending_alerts(plan, lang, false));
    body.push_str(&summary::render(plan, lang, token));
    body.push_str(&map::plan_map_img(&plan.plan_id));
    for d in &plan.days {
        body.push_str(&day::render(d, &plan.plan_id, lang));
    }
    // Meal pending-booking alerts AFTER the day cards (mirror render.ts:1393),
    // then the transit cheat-sheet (mirror render.ts:1394).
    body.push_str(&alerts::render_pending_alerts(plan, lang, true));
    body.push_str(&alerts::render_transit_summary(plan, lang));
    page(&plan.display_name, &body, lang)
}

/// Escape text for HTML TEXT content and DOUBLE-QUOTED attribute values only.
/// (Escapes & < > ". Not safe for single-quoted attrs, unquoted attrs, URLs, or
/// JS/CSS contexts — build those from trusted components instead.)
/// Escape ONCE — never double-escape (the old TS bug rendered `&amp;amp;`).
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Percent-encode a string for a URL path/query component (RFC 3986 unreserved
/// set passes through; space → `%20`; every other byte → `%XX` over its UTF-8
/// bytes). Single shared implementation — do NOT re-roll this per module.
pub fn urlencode(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        b' ' => "%20".to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

/// Escape a URL for a double-quoted HTML attribute (href/src).
/// Neutralizes attribute-breaking chars (" < > space) via percent-encoding but
/// does NOT touch `&`, so query strings (`?q=a&z=15`) survive intact.
/// Use this for URLs; use esc() for text/non-URL attribute values.
pub fn esc_url_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("%22"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            ' ' => out.push_str("%20"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escapes_ampersand_once() {
        assert_eq!(esc("Museum & Art"), "Museum &amp; Art");
        assert!(!esc("Museum & Art").contains("amp;amp;"));
    }
    #[test]
    fn esc_url_attr_preserves_ampersand_neutralizes_quotes() {
        assert_eq!(esc_url_attr("https://x/?q=a&z=15"), "https://x/?q=a&z=15"); // & preserved
        assert_eq!(esc_url_attr("https://x/?q=\"a\""), "https://x/?q=%22a%22"); // quote neutralized
        assert!(!esc_url_attr("a b").contains(' ')); // space encoded
    }

    #[test]
    fn page_shell_has_notranslate_and_lang() {
        let html = page("My Trip", "<p>hi</p>", "zh");
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("lang=\"zh-TW\""));
        assert!(html.contains("content=\"notranslate\""));
        assert!(html.contains("<title>My Trip</title>"));
        assert!(html.contains("<p>hi</p>"));
    }

    #[test]
    fn page_shell_en_lang() {
        let html = page("Trip", "", "en");
        assert!(html.contains("lang=\"en\""));
    }

    #[test]
    fn render_plan_includes_summary_map_and_days() {
        use crate::model::{Plan, Day};
        let plan = Plan {
            plan_id: "okinawa-2026".into(),
            display_name: "Okinawa".into(),
            days: vec![Day { day_number: 1, date: "2026-06-21".into(), day_type: "arrival".into(), ..Default::default() }],
            ..Default::default()
        };
        let html = render_plan(&plan, "en", None);
        assert!(html.contains("booking-summary"));
        assert!(html.contains("/map/okinawa-2026/plan.png"));
        assert!(html.contains("Day 1"));
    }
}
