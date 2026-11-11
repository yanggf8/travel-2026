use super::{esc, esc_url_attr, render_activity_text, urlencode};
use crate::i18n::t;
use crate::model::{Meal, Session};

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
pub fn render_meal(meal: &Meal, lang: &str) -> String {
    let badge = super::ai_rec_badge(&meal.source, lang);
    match split_meal_pin(&meal.text) {
        Some((label, query, trailing)) => {
            let url = format!("https://www.google.com/maps/search/{}", urlencode(&query));
            let mut h = format!(
                "<a class=\"meal\" href=\"{}\" target=\"_blank\" rel=\"noopener\">🍽️ {} 🗺️</a>",
                esc_url_attr(&url),
                esc(&label),
            );
            if !trailing.is_empty() {
                h.push_str(&format!(
                    "<span class=\"meal meal-alt\">{}</span>",
                    esc(&trailing)
                ));
            }
            h.push_str(&badge);
            h
        }
        None => format!(
            "<div class=\"meal\">🍽️ {}{}</div>",
            render_activity_text(&meal.text),
            badge
        ),
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
            while j < c.len() && c[j].is_whitespace() {
                j += 1;
            }
            if matches_map_kw(&c, j) {
                let label: String = c[..i].iter().collect();
                // query starts after `map:` (+ optional whitespace).
                let mut k = j + 4; // len("map:")
                while k < c.len() && c[k].is_whitespace() {
                    k += 1;
                }
                // query runs until the NEXT pipe (or end) — this is the FIX.
                let q_start = k;
                while k < c.len() && !is_pipe(c[k]) {
                    k += 1;
                }
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
fn is_pipe(ch: char) -> bool {
    ch == '｜' || ch == '|'
}

/// Case-insensitive `map:` at index `i`.
fn matches_map_kw(c: &[char], i: usize) -> bool {
    let kw = ['m', 'a', 'p', ':'];
    i + kw.len() <= c.len()
        && c[i..i + kw.len()]
            .iter()
            .zip(&kw)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
}

/// Leading meal-role prefixes stripped by [`meal_short`] when the text before
/// the first `：` ends with one (「主選：」「晚餐主選：」「備選：」「備案：」).
/// A keyword list the worker matches data against — code, not chrome.
const MEAL_ROLE_PREFIXES: [&str; 3] = ["主選", "備選", "備案"];

/// Display width of [`meal_short`]'s fallback cut, in characters.
const MEAL_SHORT_MAX_CHARS: usize = 30;

/// The SHORT form of a `session_meals.meal` string for the plan page's meal
/// overview (unescaped; the caller escapes):
/// 1. drop a leading role prefix — the text up to the first `：` when it ends
///    with 主選/備選/備案 and holds no pipe (「主選：」, 「晚餐主選：」);
/// 2. drop the `｜map:<query>` pin and anything after it (the label half of
///    [`split_meal_pin`], the same parser the day cards use);
/// 3. keep 「料理｜店名」: everything up to the first `：` or `；` AFTER the
///    first pipe (`｜` or `|`).
///
/// Without that structure (no pipe, or no `：`/`；` after it) the text is kept
/// whole up to [`MEAL_SHORT_MAX_CHARS`] characters, longer text cut there with
/// an ellipsis.
/// 「主選：鰻魚飯｜京極かねよ：新京極六角，…；…｜map:京極かねよ」 → 「鰻魚飯｜京極かねよ」;
/// 「去程午餐｜樂桃不含機上餐；…」 → 「去程午餐｜樂桃不含機上餐」.
pub fn meal_short(meal: &str) -> String {
    let mut text = meal.trim();
    if let Some((head, rest)) = text.split_once('：') {
        if !head.contains(is_pipe) && MEAL_ROLE_PREFIXES.iter().any(|p| head.trim_end().ends_with(p)) {
            text = rest.trim_start();
        }
    }
    let label = match split_meal_pin(text) {
        Some((label, _, _)) => label,
        None => text.to_string(),
    };
    if let Some(pipe) = label.find(is_pipe) {
        let after = pipe + label[pipe..].chars().next().map_or(0, char::len_utf8);
        if let Some(stop) = label[after..].find(|c| c == '：' || c == '；') {
            return label[..after + stop].trim().to_string();
        }
    }
    let label = label.trim();
    if label.chars().count() <= MEAL_SHORT_MAX_CHARS {
        label.to_string()
    } else {
        let cut: String = label.chars().take(MEAL_SHORT_MAX_CHARS).collect();
        format!("{}\u{2026}", cut.trim_end())
    }
}

/// Localized session name (上午/中午/下午/晚上 · Morning/Noon/Afternoon/Evening).
/// Shared by the session block and the plan page's meal overview.
pub fn session_label(session_type: &str, lang: &str) -> &'static str {
    match session_type {
        "morning" => t("sessionMorning", lang),
        "noon" => t("sessionNoon", lang),
        "afternoon" => t("sessionAfternoon", lang),
        "evening" => t("sessionEvening", lang),
        // An unknown session type keeps the old untranslated fallback label.
        _ => t("sessionEvening", "en"),
    }
}

/// Render one session block. Empty sessions are caller-skipped (see day.rs).
pub fn render(sess: &Session, lang: &str) -> String {
    let label = session_label(&sess.session_type, lang);
    let mut h = String::new();
    h.push_str(&format!(
        "<div class=\"session session-{}\"><div class=\"session-label\">{}</div>",
        esc(&sess.session_type),
        label
    ));
    if !sess.focus_zh.is_empty() {
        h.push_str(&format!(
            "<div class=\"session-focus\">{}</div>",
            esc(&sess.focus_zh)
        ));
    }
    for a in &sess.activities {
        // render_activity_text (port of the JS worker's renderActivityText):
        // escapes, turns \n into <br>, and renders an embedded "Google Maps：<url>"
        // tail as a short labeled link instead of dumping the giant URL inline.
        h.push_str(&format!(
            "<div class=\"activity\">{}{}</div>",
            render_activity_text(&a.title),
            super::ai_rec_badge(&a.source, lang)
        ));
    }
    for m in &sess.meals {
        h.push_str(&render_meal(m, lang));
    }
    if !sess.transit_zh.is_empty() {
        h.push_str(&format!(
            "<div class=\"transit\">🚃 {}</div>",
            render_activity_text(&sess.transit_zh)
        ));
    }
    h.push_str("</div>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Activity, Meal, Session};
    /// Build an Activity carrying only a display title (the common test case).
    fn act(title: &str) -> Activity {
        Activity {
            title: title.into(),
            ..Default::default()
        }
    }
    fn meal(text: &str) -> Meal {
        Meal {
            text: text.into(),
            source: "confirmed".into(),
        }
    }
    #[test]
    fn noon_meal_renders() {
        let sess = Session {
            session_type: "noon".into(),
            meals: vec![meal("Lunch: Makishi")],
            ..Default::default()
        };
        let html = render(&sess, "zh");
        assert!(html.contains("中午"));
        assert!(html.contains("Lunch: Makishi"));
    }
    #[test]
    fn activity_ampersand_escaped_once() {
        let sess = Session {
            session_type: "morning".into(),
            activities: vec![act("Museum & Art")],
            ..Default::default()
        };
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
        assert!(
            html.contains("<a href=\"https://www.google.com/maps/search/abc\""),
            "got: {html}"
        );
        assert!(!html.contains("%0A"), "got: {html}");
    }
    #[test]
    fn activity_newline_becomes_br() {
        let sess = Session {
            session_type: "morning".into(),
            activities: vec![act("line a\nline b")],
            ..Default::default()
        };
        let html = render(&sess, "en");
        assert!(html.contains("line a<br>line b"), "got: {html}");
    }

    #[test]
    fn transit_embedded_map_url_becomes_labeled_link() {
        let sess = Session {
            session_type: "morning".into(),
            transit_zh: "04:00 自家出發開車：紅樹林 → 大園出國停車場第三停車場（約45–60分）\n地址：桃園市大園區中山南路544號旁\nGoogle Maps 導航：https://www.google.com/maps/dir/%E6%96%B0%E5%8C%97/%E6%A1%83%E5%9C%92".into(),
            ..Default::default()
        };
        let html = render(&sess, "zh");
        assert!(html.contains("🗺️ Google Maps 導航"), "got: {html}");
        assert!(
            html.contains(
                "<a href=\"https://www.google.com/maps/dir/%E6%96%B0%E5%8C%97/%E6%A1%83%E5%9C%92\""
            ),
            "got: {html}"
        );
        assert!(
            !html.contains(">https://www.google.com/maps/dir/"),
            "got: {html}"
        );
    }

    // ---- meal map-pin rendering (port of renderMealPill, FIX 備案 over-capture) ----

    // A meal with a ｜map:<query> pin renders the LABEL as a clickable maps link;
    // the raw "｜map:…" convention text must NOT appear.
    #[test]
    fn meal_pin_renders_clickable_label_not_raw_query() {
        let out = render_meal(&meal("晚餐：ステーキ88｜map:ステーキ88 那覇"), "zh");
        assert!(out.contains("<a class=\"meal\" href=\""), "got: {out}");
        assert!(out.contains("maps/search/"), "got: {out}");
        // Visible clickable text is the LABEL (no raw "map:" / the query blob).
        assert!(out.contains("🍽️ 晚餐：ステーキ88 🗺️"), "got: {out}");
        // The convention marker is gone from the rendered output.
        assert!(!out.contains("｜map:"), "got: {out}");
        assert!(!out.contains("map:ステーキ"), "got: {out}");
        // href encodes the place name (那覇 → %-encoded UTF-8), not the raw label.
        assert!(
            out.contains("%E9%82%A3%E8%A6%87"),
            "那覇 encoded missing; got: {out}"
        );
    }

    // The 備案 over-capture FIX: with a trailing ｜備案：<backup>, the maps query is
    // ONLY the place name (stops at the next pipe); 備案 stays as plain text.
    #[test]
    fn meal_pin_trailing_alt_is_not_swallowed_into_query() {
        let out = render_meal(&meal("晚餐：A｜map:A 那覇 ｜備案：B"), "zh");
        // Link present, label clickable.
        assert!(out.contains("<a class=\"meal\" href=\""), "got: {out}");
        assert!(out.contains("🍽️ 晚餐：A 🗺️"), "got: {out}");
        // The query is "A 那覇" only — encoded as "A%20" + 那覇 — and does NOT
        // contain the 備案 backup text nor a second encoded pipe.
        // 備案 chars (備=%E5%82%99 案=%E6%A1%88) must NOT appear in the href query.
        let href = &out[..out.find("\" target").unwrap_or(out.len())];
        assert!(
            !href.contains("%E5%82%99"),
            "備 leaked into query; got: {out}"
        );
        assert!(
            !href.contains("%E6%A1%88"),
            "案 leaked into query; got: {out}"
        );
        // 備案：B is still shown to the user as plain (escaped) text after the link.
        assert!(out.contains("備案：B"), "got: {out}");
        // The convention marker itself is gone.
        assert!(!out.contains("map:A"), "got: {out}");
    }

    // A meal with NO pin → plain div pill, no <a>, no panic.
    #[test]
    fn meal_without_pin_is_plain_pill() {
        let out = render_meal(&meal("Hotel breakfast"), "zh");
        assert_eq!(out, "<div class=\"meal\">🍽️ Hotel breakfast</div>");
        assert!(!out.contains("<a "), "got: {out}");
    }

    #[test]
    fn meal_embedded_google_maps_url_becomes_labeled_link() {
        let out = render_meal(&meal("午餐（實際）：安里屋すば（Asatoya Suba・沖繩そば・安里・飯店附近） Google Maps：https://www.google.com/maps/search/%E5%AE%89%E9%87%8C%E5%B1%8B%E3%81%99%E3%81%B0%20%E9%82%A3%E8%A6%87%20%E5%AE%89%E9%87%8C"), "zh");
        assert!(out.contains("🗺️ Google Maps"), "got: {out}");
        assert!(
            out.contains("<a href=\"https://www.google.com/maps/search/%E5%AE%89%E9%87%8C"),
            "got: {out}"
        );
        assert!(
            !out.contains(">https://www.google.com/maps/search/"),
            "got: {out}"
        );
    }

    #[test]
    fn meal_bare_google_maps_url_becomes_short_map_link() {
        let out = render_meal(&meal("Lunch https://www.google.com/maps/search/Asatoya"), "zh");
        assert!(out.contains(">🗺️ Google Maps</a>"), "got: {out}");
        assert!(
            !out.contains(">https://www.google.com/maps/search/"),
            "got: {out}"
        );
    }

    // esc/esc_url_attr: a meal label with &/< escapes ONCE in the visible text;
    // the href preserves the query via esc_url_attr (no double-escape).
    #[test]
    fn meal_pin_escapes_label_and_preserves_href() {
        let out = render_meal(&meal("Diner & <Café>｜map:Café & Bar 那覇"), "zh");
        // Visible label escaped once.
        assert!(out.contains("Diner &amp; &lt;Café&gt;"), "got: {out}");
        assert!(!out.contains("amp;amp;"), "double-escape; got: {out}");
        // href present and well-formed (esc_url_attr neutralizes < > " but the
        // query was percent-encoded first, so the place name survives).
        assert!(
            out.contains("<a class=\"meal\" href=\"https://www.google.com/maps/search/"),
            "got: {out}"
        );
        // The ampersand in the query was percent-encoded (%26), not left raw.
        let href_part = &out[..out.find("\" target").unwrap_or(out.len())];
        assert!(
            href_part.contains("%26"),
            "& not encoded in query; got: {out}"
        );
    }

    // Integration: render() wires render_meal into the session block; a pinned
    // meal yields a meal link in the session HTML and no raw ｜map: text.
    #[test]
    fn session_render_uses_meal_links() {
        let sess = Session {
            session_type: "evening".into(),
            meals: vec![meal("晚餐：ステーキ88｜map:ステーキ88 那覇")],
            ..Default::default()
        };
        let html = render(&sess, "zh");
        assert!(html.contains("<a class=\"meal\" href=\""), "got: {html}");
        assert!(html.contains("maps/search/"), "got: {html}");
        assert!(!html.contains("｜map:"), "got: {html}");
    }

    #[test]
    fn ai_recommended_meal_renders_badge_en() {
        let html = render_meal(
            &Meal {
                text: "Lunch".into(),
                source: "ai_recommended".into(),
            },
            "en",
        );
        assert!(html.contains("ai-rec-badge"), "got: {html}");
        assert!(html.contains("🤖 AI-recommended (unconfirmed)"), "got: {html}");
    }

    #[test]
    fn confirmed_meal_has_no_badge() {
        let html = render_meal(
            &Meal {
                text: "Lunch".into(),
                source: "confirmed".into(),
            },
            "en",
        );
        assert!(!html.contains("ai-rec-badge"), "got: {html}");
    }

    #[test]
    fn ai_recommended_activity_renders_badge_zh() {
        let sess = Session {
            session_type: "morning".into(),
            activities: vec![Activity {
                title: "Suggested spot".into(),
                source: "ai_recommended".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&sess, "zh");
        assert!(html.contains("ai-rec-badge"), "got: {html}");
        assert!(html.contains("🤖 建議（未確認）"), "got: {html}");
    }

    #[test]
    fn confirmed_activity_has_no_badge() {
        let sess = Session {
            session_type: "morning".into(),
            activities: vec![Activity {
                title: "Confirmed spot".into(),
                source: "confirmed".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&sess, "zh");
        assert!(!html.contains("ai-rec-badge"), "got: {html}");
    }

    // ---- meal_short (plan page meal overview) ----
    #[test]
    fn meal_short_keeps_dish_and_shop_from_a_pinned_meal() {
        assert_eq!(
            meal_short("主選：鰻魚飯｜京極かねよ：新京極六角，排隊約 20 分；招牌錦糸丼｜map:京極かねよ"),
            "鰻魚飯｜京極かねよ"
        );
        // A meal-name role prefix (「晚餐主選：」) and 備選/備案 are stripped too.
        assert_eq!(meal_short("晚餐主選：湯豆腐｜奧丹：南禪寺參道"), "湯豆腐｜奧丹");
        assert_eq!(meal_short("備選：親子丼｜鳥岩樓：西陣｜map:鳥岩樓"), "親子丼｜鳥岩樓");
        assert_eq!(meal_short("備案：蕎麥麵｜本家尾張屋；近車站"), "蕎麥麵｜本家尾張屋");
    }

    #[test]
    fn meal_short_cuts_at_the_first_semicolon_after_the_pipe() {
        assert_eq!(meal_short("去程午餐｜樂桃不含機上餐；機場先買便當"), "去程午餐｜樂桃不含機上餐");
        // An ASCII pipe counts; a colon BEFORE the pipe is not the cut (and the
        // text before it is not a role prefix, so it stays).
        assert_eq!(meal_short("Lunch：Ramen|Ichiran：Shibuya"), "Lunch：Ramen|Ichiran");
    }

    #[test]
    fn meal_short_falls_back_to_thirty_chars_with_ellipsis() {
        // No pipe: short text whole, long text cut at 30 chars + 「…」.
        assert_eq!(meal_short("Hotel breakfast"), "Hotel breakfast");
        assert_eq!(meal_short("晚餐：ステーキ88｜map:ステーキ88 那覇"), "晚餐：ステーキ88");
        assert_eq!(
            meal_short("Ramen & <gyoza> at the station food court near the hotel"),
            "Ramen & <gyoza> at the station\u{2026}"
        );
        // A pipe with no ：/； after it is not the 料理｜店名 shape → same fallback.
        let long = "早餐｜飯店自助早餐含京都在地漬物與湯豆腐吧台還有現煎玉子燒與咖啡";
        let cut: String = long.chars().take(30).collect();
        assert_eq!(meal_short(long), format!("{cut}\u{2026}"));
        assert_eq!(meal_short("早餐｜飯店"), "早餐｜飯店");
        assert_eq!(meal_short("  "), "");
    }

    #[test]
    fn session_label_is_bilingual_with_an_english_fallback() {
        assert_eq!(session_label("noon", "zh"), "中午");
        assert_eq!(session_label("evening", "en"), "Evening");
        assert_eq!(session_label("morning", "zh"), "上午");
        assert_eq!(session_label("afternoon", "en"), "Afternoon");
        assert_eq!(session_label("brunch", "zh"), "Evening");
    }
}
