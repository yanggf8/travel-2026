//! Owner-only plans index: list every plan as a link to `/?plan=<plan_id>`.
//! `plan_id` is a controlled slug (hyphenated), safe in an href by construction;
//! all displayed text still goes through esc().

use std::collections::HashMap;

use super::{esc, share};
use crate::i18n::t;
use crate::turso::Row;

fn rs(row: &Row, k: &str) -> String {
    row.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

pub fn render(
    plan_rows: &[Row],
    plan_share_tokens: &HashMap<String, String>,
    public_origin: &str,
    lang: &str,
) -> String {
    let mut h = String::new();
    h.push_str(&format!("<h1>{}</h1>", esc(t("plans", lang))));
    h.push_str("<ul class=\"plan-index\">");
    for p in plan_rows {
        let plan_id = rs(p, "plan_id");
        if plan_id.is_empty() { continue; }
        let name = {
            let dn = rs(p, "display_name");
            if dn.is_empty() { plan_id.clone() } else { dn }
        };
        let start = rs(p, "start_date");
        let end = rs(p, "end_date");
        let dates = match (start.is_empty(), end.is_empty()) {
            (false, false) => format!("{} – {}", start, end),
            (false, true) => start.clone(),
            _ => String::new(),
        };
        // plan_id is a known-safe slug; embed directly in the href.
        h.push_str(&format!(
            "<li class=\"plan-card\"><a class=\"plan-card-main\" href=\"/?plan={}\">",
            esc(&plan_id)
        ));
        h.push_str(&format!("<div class=\"plan-card-name\">{}</div>", esc(&name)));
        if !dates.is_empty() {
            h.push_str(&format!("<div class=\"plan-card-dates\">{}</div>", esc(&dates)));
        }
        h.push_str("</a><div class=\"plan-card-actions\">");
        if let Some(token) = plan_share_tokens.get(&plan_id) {
            h.push_str(&share::copy_button(
                &share::share_url(public_origin, &plan_id, token),
                lang,
            ));
        } else {
            h.push_str(&format!(
                r#"<span class="copy-share-missing">{}</span>"#,
                esc(t("noShareLink", lang)),
            ));
        }
        h.push_str("</div></li>");
    }
    h.push_str("</ul>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turso::Row;

    #[test]
    fn plan_appears_as_link() {
        let mut p = Row::new();
        p.insert("plan_id".into(), serde_json::json!("okinawa-2026"));
        p.insert("display_name".into(), serde_json::json!("Okinawa June 2026"));
        p.insert("start_date".into(), serde_json::json!("2026-06-21"));
        p.insert("end_date".into(), serde_json::json!("2026-06-24"));
        let html = render(&[p], &HashMap::new(), "https://example.dev", "en");
        assert!(html.contains("href=\"/?plan=okinawa-2026\""));
        assert!(html.contains("Okinawa June 2026"));
        assert!(html.contains("2026-06-21"));
    }

    #[test]
    fn falls_back_to_plan_id_when_name_absent() {
        let mut p = Row::new();
        p.insert("plan_id".into(), serde_json::json!("tokyo-2026"));
        let html = render(&[p], &HashMap::new(), "https://example.dev", "en");
        assert!(html.contains("href=\"/?plan=tokyo-2026\""));
        assert!(html.contains("tokyo-2026"));
    }

    #[test]
    fn heading_is_localized() {
        let html = render(&[], &HashMap::new(), "https://example.dev", "zh");
        assert!(html.contains("行程"));
    }

    #[test]
    fn owner_index_includes_copy_share_button_when_token_exists() {
        let mut p = Row::new();
        p.insert("plan_id".into(), serde_json::json!("okinawa-2026"));
        let mut tokens = HashMap::new();
        tokens.insert("okinawa-2026".into(), "share123".into());
        let html = render(&[p], &tokens, "https://example.dev", "en");
        assert!(html.contains("copy-share-btn"));
        assert!(html.contains("data-copy-url=\"https://example.dev/?plan=okinawa-2026&amp;token=share123\""));
        assert!(!html.contains("<a class=\"plan-card\""));
    }
}
