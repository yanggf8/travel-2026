pub mod activity_text;
pub mod alerts;
pub mod auth;
pub mod day;
pub mod index;
pub mod map;
pub mod session;
pub mod share;
pub mod summary;
use crate::model::Plan;
pub use activity_text::render_activity_text;

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
        esc(title),
        crate::styles::CSS,
        body,
    )
}

/// Render a full plan page: plan overview map, booking summary, then each day card.
/// `token` is the access token the page was loaded with — threaded into the
/// auth-gated voucher link so a click carries the same token (else 403).
/// `owner_chrome` is the logged-in owner top bar (copy share link); empty for viewers.
pub fn render_plan(
    plan: &Plan,
    lang: &str,
    token: Option<&str>,
    map_status: &map::MapStatus,
    owner_chrome: &str,
) -> String {
    let mut body = String::new();
    if !owner_chrome.is_empty() {
        body.push_str(owner_chrome);
        body.push_str(share::COPY_SCRIPT);
    }
    // Non-meal pending-booking alerts BEFORE the summary (mirror render.ts:1388).
    body.push_str(&alerts::render_pending_alerts(plan, lang, false));
    // Plan overview map ABOVE the booking summary (its own frame, never inside
    // the summary's dashed box) — user visibility request.
    body.push_str(&map::plan_map_slot(
        &plan.plan_id,
        map_status.plan.as_deref(),
        lang,
        map_status.legend_for("plan.png"),
    ));
    body.push_str(&render_companion_options(plan, lang));
    // The logistics map only exists where the trip has an airport segment
    // (flights). A DOMESTIC self-drive plan has none — rendering the slot
    // anyway just showed a 地圖尚未產生 placeholder that wasted vertical
    // space (user request 2026-09-28). With flights it renders as a compact
    // inset (崁入小圖) instead of a second full-size map.
    // Supplementary insets sit side by side under the overview: the far-off
    // day-trip map (only when one exists) and the hotel/airport map.
    let show_logistics = map_status.plan_logistics.is_some() || !plan.flights.is_empty();
    if map_status.plan_excursion.is_some() || show_logistics {
        body.push_str("<div class=\"map-insets\">");
        if let Some(v) = map_status.plan_excursion.as_deref() {
            body.push_str(&map::plan_excursion_map_slot(
                &plan.plan_id,
                v,
                lang,
                map_status.legend_for("plan-excursion.png"),
            ));
        }
        if show_logistics {
            body.push_str(&map::plan_logistics_map_slot(
                &plan.plan_id,
                map_status.plan_logistics.as_deref(),
                lang,
                map_status.legend_for("plan-logistics.png"),
            ));
        }
        body.push_str("</div>");
    }
    body.push_str(&summary::render(plan, lang, token));
    for d in &plan.days {
        let map_ver = map_status
            .days
            .get(&d.day_number)
            .and_then(|v| v.as_deref());
        body.push_str(&day::render_with_logistics(
            d,
            &plan.plan_id,
            lang,
            map_ver,
            map_status
                .day_logistics
                .get(&d.day_number)
                .and_then(|v| v.as_deref()),
            map_status.legend_for(&format!("day-{}.png", d.day_number)),
            map_status.legend_for(&format!("day-{}-logistics.png", d.day_number)),
        ));
    }
    // Meal pending-booking alerts AFTER the day cards (mirror render.ts:1393),
    // then the transit cheat-sheet (mirror render.ts:1394).
    body.push_str(&alerts::render_pending_alerts(plan, lang, true));
    body.push_str(&alerts::render_transit_summary(plan, lang));
    page(&plan.display_name, &body, lang)
}

/// Render an explicit companion-options block from the temporary, unconfirmed
/// itinerary note. This keeps alternatives visually separate from the official
/// day schedule while the group is deciding.
fn render_companion_options(plan: &Plan, lang: &str) -> String {
    let title = plan.days.iter().flat_map(|d| d.sessions.iter())
        .flat_map(|s| s.activities.iter())
        .find(|a| a.title.starts_with("同行方案（可複選"))
        .map(|a| a.title.as_str());
    let Some(title) = title else { return String::new(); };
    let mut sections = title.split("\n\n");
    let heading = sections.next().unwrap_or("同行方案");
    let cards: Vec<&str> = sections.filter(|s| s.starts_with("A｜") || s.starts_with("B｜")).collect();
    if cards.is_empty() { return String::new(); }
    let label = if lang == "en" { "Companion options · select any" } else { "同行方案 · 可複選" };
    let mut out = format!("<section class=\"companion-options\"><h2>{}</h2><p class=\"companion-hint\">{}</p><div class=\"companion-grid\">", esc(label), esc(heading));
    for card in cards {
        let mut lines = card.lines();
        let name = lines.next().unwrap_or_default();
        let details = lines.map(esc).collect::<Vec<_>>().join("<br>");
        out.push_str(&format!("<article class=\"companion-card\"><h3>{}</h3><p>{}</p></article>", esc(name), details));
    }
    out.push_str("</div></section>");
    out
}

/// AI-recommended provenance badge. Empty string for confirmed/any non-ai_recommended source, so
/// callers can append it unconditionally. Bilingual: ZH default, English on ?lang=en.
pub fn ai_rec_badge(source: &str, lang: &str) -> String {
    if source != "ai_recommended" {
        return String::new();
    }
    let label = if lang == "en" {
        "\u{1F916} AI-recommended (unconfirmed)"
    } else {
        "\u{1F916} 建議（未確認）"
    };
    format!("<span class=\"ai-rec-badge\">{}</span>", esc(label))
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
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "%20".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
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
        use crate::model::{Day, Plan};
        let plan = Plan {
            plan_id: "okinawa-2026".into(),
            display_name: "Okinawa".into(),
            days: vec![Day {
                day_number: 1,
                date: "2026-06-21".into(),
                day_type: "arrival".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let map_status = map::MapStatus {
            plan: Some("etag1".into()),
            plan_logistics: Some("etag3".into()),
            plan_excursion: None,
            days: [(1i64, Some("etag2".into()))].into_iter().collect(),
            day_logistics: std::collections::HashMap::new(),
            legends: [
                (
                    "plan.png".to_string(),
                    vec![map::LegendStop {
                        seq: 1,
                        label: "首里城".into(),
                        lat: 26.2186,
                        lon: 127.7196,
                    }],
                ),
                (
                    "day-1.png".to_string(),
                    vec![map::LegendStop {
                        seq: 2,
                        label: "国際通り".into(),
                        lat: 26.2125,
                        lon: 127.6809,
                    }],
                ),
            ]
            .into_iter()
            .collect(),
        };
        let html = render_plan(&plan, "en", None, &map_status, "");
        assert!(html.contains("booking-summary"));
        assert!(html.contains("/map/okinawa-2026/plan.png"));
        assert!(html.contains("Day 1"));
        // Plan map renders ABOVE the booking summary (its own frame, not in the
        // summary's dashed box).
        let map_pos = html.find("planmap").expect("planmap img missing");
        let summary_pos = html.find("booking-summary").expect("summary missing");
        assert!(
            map_pos < summary_pos,
            "plan map must render before the booking summary"
        );
        // Summary carries the dashed-frame class.
        assert!(html.contains("booking-summary summary-box"));
        // Legends thread through to the plan map and the day map from MapStatus.
        assert!(html.contains(">首里城</a>"), "plan legend missing: {html}");
        assert!(html.contains(">国際通り</a>"), "day legend missing: {html}");
        let plan_leg = html.find("首里城").expect("plan legend");
        let day_leg = html.find("国際通り").expect("day legend");
        let plan_map = html.find("plan.png").expect("plan map img");
        let day_map = html.find("day-1.png").expect("day map img");
        assert!(plan_leg > plan_map, "plan legend sits under its map");
        assert!(day_leg > day_map, "day legend sits under its map");
    }

    #[test]
    fn render_plan_omits_logistics_map_for_domestic() {
        // Domestic plan: no flights AND no logistics PNG in R2 → the slot must
        // not render at all (it used to show a 地圖尚未產生 placeholder that
        // wasted the vertical space a self-drive trip does not need).
        use crate::model::Plan;
        let plan = Plan {
            plan_id: "jiufen-2026".into(),
            display_name: "九份".into(),
            ..Default::default()
        };
        let map_status = map::MapStatus {
            plan: Some("etag1".into()),
            ..Default::default()
        };
        let html = render_plan(&plan, "zh", None, &map_status, "");
        assert!(html.contains("/map/jiufen-2026/plan.png"));
        assert!(
            !html.contains("plan-logistics.png"),
            "domestic plan must not render the logistics map slot: {html}"
        );
        assert!(!html.contains("地圖尚未產生"), "{html}");
    }

    #[test]
    fn render_plan_keeps_logistics_slot_when_flights_exist() {
        // A Japan plan with flights keeps the logistics slot even before the PNG
        // exists (so the missing-map state is visible, prompting a snapshot run)
        // — it renders as a compact inset, not a second full-size map.
        use crate::model::Plan;
        use crate::turso::Row;
        let mut f = Row::new();
        f.insert("flight_number".into(), serde_json::json!("CI120"));
        f.insert("departure_code".into(), serde_json::json!("TPE"));
        f.insert("arrival_code".into(), serde_json::json!("KIX"));
        let plan = Plan {
            plan_id: "kyoto-2026".into(),
            display_name: "Kyoto".into(),
            flights: vec![f],
            ..Default::default()
        };
        let map_status = map::MapStatus::default();
        let html = render_plan(&plan, "zh", None, &map_status, "");
        // The placeholder branch emits no <img>, so assert on the slot's caption
        // and its inset frame — not on the PNG URL.
        assert!(
            html.contains("住宿與機場"),
            "flights exist → logistics slot must render: {html}"
        );
        assert!(html.contains("map-frame--inset"), "{html}");
    }

    #[test]
    fn render_plan_puts_excursion_inset_beside_logistics() {
        use crate::model::Plan;
        let plan = Plan {
            plan_id: "osaka-nov-2026".into(),
            display_name: "Kyoto".into(),
            ..Default::default()
        };
        let map_status = map::MapStatus {
            plan: Some("p".into()),
            plan_logistics: Some("l".into()),
            plan_excursion: Some("e".into()),
            ..Default::default()
        };
        let html = render_plan(&plan, "zh", None, &map_status, "");
        let row = html.split("<div class=\"map-insets\">").nth(1).expect("insets row");
        let row = row.split("</figure></div>").next().unwrap_or("");
        let exc = row.find("plan-excursion.png").expect("excursion inset in the row");
        let log = row.find("plan-logistics.png").expect("logistics inset in the row");
        assert!(exc < log, "day trip first, then hotels/airports: {row}");

        // No excursion PNG → no day-trip slot and no placeholder for it.
        let none = render_plan(&plan, "zh", None, &map::MapStatus { plan_logistics: Some("l".into()), ..Default::default() }, "");
        assert!(!none.contains("plan-excursion.png"));
        assert!(!none.contains("一日遊"), "{none}");
    }

    #[test]
    fn companion_options_render_as_separate_cards() {
        use crate::model::{Activity, Day, Plan, Session};
        let plan = Plan {
            days: vec![Day {
                sessions: vec![Session {
                    activities: vec![Activity {
                        title: "同行方案（可複選，尚未確認）\n\nA｜北海岸\n路線：野柳 → 金山\n\nB｜坪林\n路線：茶博館 → 老街".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render_companion_options(&plan, "zh");
        assert!(html.contains("同行方案 · 可複選"), "{html}");
        assert!(html.contains("<h3>A｜北海岸</h3>"), "{html}");
        assert!(html.contains("<h3>B｜坪林</h3>"), "{html}");
    }
}
