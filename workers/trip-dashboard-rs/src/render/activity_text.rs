//! Render free-form activity text into safe HTML, with map links rendered as
//! short labeled links instead of dumping a giant encoded URL into a
//! `maps/search/<blob>` query.
//!
//! This is a faithful port of the JS worker's `renderActivityText()`
//! (`workers/trip-dashboard/src/render.ts`), which the user has validated by
//! hand. The JS does, in order, on the RAW activity text:
//!   1. If it already contains `<span` → return as-is (already HTML).
//!   2. esc() the text (`& < > "`).
//!   3. Collapse a `\n` that immediately precedes a "label：<url>" map tail
//!      into two spaces, so the map link stays inline (not its own line).
//!   4. Convert the remaining `\n` → `<br>`.
//!   5. A labeled link "Google Maps 導航：<url>" / "地圖：<url>" / "Map: <url>"
//!      renders as `<a href="<url>">🗺️ <label></a>` — the LABEL is the
//!      clickable text, NOT the giant URL.
//!   6. Any remaining bare URL → linkify with the URL as its own text.
//!
//! WHY HAND-ROLLED (not the `regex` crate): the worker is a wasm binary built
//! with `opt-level = "s"` + LTO to stay small; pulling in `regex` would add its
//! DFA engine + Unicode tables (hundreds of KB) for what is a line-oriented,
//! fixed-label parse. Rust's `regex` also lacks the lookbehind the JS step 6
//! uses. Hand-rolling is both leaner for wasm and clearer here.

use super::esc;

const LINK_STYLE: &str = "color:#2563eb;font-size:11px;word-break:break-all;white-space:nowrap";

/// Map-link labels recognized at the head of a "label：<url>" tail. Matches the
/// JS alternation `Google Maps|地圖|地图|導航|导航|Map|Directions`. Longest-first
/// so "Google Maps" wins over a hypothetical shorter prefix.
const LABELS: &[&str] = &["Google Maps", "Directions", "地圖", "地图", "導航", "导航", "Map"];

/// Render activity text → safe HTML. See module docs for the ported algorithm.
pub fn render_activity_text(text: &str) -> String {
    // (1) already HTML → pass through untouched.
    if text.contains("<span") {
        return text.to_string();
    }
    // (2) escape, then (3) collapse the pre-map-tail newline, then (4) \n→<br>.
    let escaped = esc(text);
    let collapsed = collapse_pre_label_newlines(&escaped);
    let with_breaks = collapsed.replace('\n', "<br>");
    // (5) labeled links, then (6) bare-URL linkify outside existing anchors.
    let labeled = linkify_labeled(&with_breaks);
    linkify_bare_urls_outside_anchors(&labeled)
}

/// JS step 3: `/\n(?=\s*(?:<label>)[^:：<]*[:：]\s*https?:\/\/)/gi → '  '`.
/// Replace a `\n` with two spaces when it is followed by optional whitespace and
/// then a labeled-map tail, so the link attaches to the prior line.
fn collapse_pre_label_newlines(s: &str) -> String {
    let bytes: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '\n' && lookahead_is_label_map_tail(&bytes, i + 1) {
            out.push_str("  ");
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    out
}

/// Lookahead for `\s*(?:<label>)[^:：<]*[:：]\s*https?:\/\/` starting at `start`.
fn lookahead_is_label_map_tail(c: &[char], start: usize) -> bool {
    let mut i = start;
    // \s*
    while i < c.len() && c[i].is_whitespace() {
        i += 1;
    }
    // (?:<label>) — case-insensitive prefix match.
    let after_label = match match_label_at(c, i) {
        Some(end) => end,
        None => return false,
    };
    // [^:：<]*  (anything but a colon or '<')
    let mut i = after_label;
    while i < c.len() && c[i] != ':' && c[i] != '：' && c[i] != '<' {
        i += 1;
    }
    // [:：]
    if i >= c.len() || (c[i] != ':' && c[i] != '：') {
        return false;
    }
    i += 1;
    // \s*
    while i < c.len() && c[i].is_whitespace() {
        i += 1;
    }
    // https?:\/\/
    starts_with_http_scheme(c, i)
}

/// If one of LABELS matches (case-insensitive) at `i`, return the end index.
fn match_label_at(c: &[char], i: usize) -> Option<usize> {
    for label in LABELS {
        let lc: Vec<char> = label.chars().collect();
        if i + lc.len() <= c.len()
            && c[i..i + lc.len()]
                .iter()
                .zip(&lc)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            return Some(i + lc.len());
        }
    }
    None
}

/// `https://` or `http://` at index `i` (case-insensitive scheme).
fn starts_with_http_scheme(c: &[char], i: usize) -> bool {
    let rest: String = c[i..(i + 8).min(c.len())].iter().collect();
    let lower = rest.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

/// JS step 5: replace each "label[^:：<]*：<url>" with a labeled anchor whose
/// clickable text is the label (trimmed), not the URL.
/// Regex: `/(<label>[^:：<]*)[:：]\s*(https?:\/\/[^\s<&"，。、]+)/gi`.
fn linkify_labeled(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < c.len() {
        if let Some((label, url, next)) = match_labeled_link(&c, i) {
            out.push_str(&format!(
                "<a href=\"{}\" target=\"_blank\" rel=\"noopener\" style=\"{}\">🗺️ {}</a>",
                url,
                LINK_STYLE,
                label.trim()
            ));
            i = next;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// Try to match `(<label>[^:：<]*)[:：]\s*(url)` at `i`.
/// Returns (label_text, url_text, end_index_after_url).
fn match_labeled_link(c: &[char], i: usize) -> Option<(String, String, usize)> {
    let label_start = i;
    let after_label = match_label_at(c, i)?;
    // [^:：<]*
    let mut j = after_label;
    while j < c.len() && c[j] != ':' && c[j] != '：' && c[j] != '<' {
        j += 1;
    }
    // The whole "label[^:：<]*" run, captured as group 1.
    let label: String = c[label_start..j].iter().collect();
    // [:：]
    if j >= c.len() || (c[j] != ':' && c[j] != '：') {
        return None;
    }
    j += 1;
    // \s*
    while j < c.len() && c[j].is_whitespace() {
        j += 1;
    }
    // (https?:\/\/[^\s<&"，。、]+)
    if !starts_with_http_scheme(c, j) {
        return None;
    }
    let url_start = j;
    while j < c.len() && !is_url_terminator(c[j]) {
        j += 1;
    }
    if j == url_start {
        return None;
    }
    let url: String = c[url_start..j].iter().collect();
    Some((label, url, j))
}

/// URL character-class terminator: the JS `[^\s<&"，。、]` negation.
fn is_url_terminator(ch: char) -> bool {
    ch.is_whitespace()
        || ch == '<'
        || ch == '&'
        || ch == '"'
        || ch == '，'
        || ch == '。'
        || ch == '、'
}

/// JS step 6: linkify any remaining BARE url with the url as its own text, but
/// skip URLs already wrapped in an `<a ...>` from step 5. The JS uses a
/// `(?<!href=")` lookbehind; Rust's regex has none, so instead we split the
/// string on existing `<a ...>...</a>` anchor spans and only linkify the gaps.
fn linkify_bare_urls_outside_anchors(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<a ") {
        // Linkify the segment before the anchor.
        out.push_str(&linkify_bare_urls(&rest[..start]));
        // Find the matching </a> and copy the anchor span verbatim.
        match rest[start..].find("</a>") {
            Some(rel_end) => {
                let end = start + rel_end + "</a>".len();
                out.push_str(&rest[start..end]);
                rest = &rest[end..];
            }
            None => {
                // Malformed (no closing tag) — copy the remainder verbatim and stop.
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(&linkify_bare_urls(rest));
    out
}

/// Linkify every bare `https?://…` run in a segment known to contain NO anchors.
fn linkify_bare_urls(seg: &str) -> String {
    let c: Vec<char> = seg.chars().collect();
    let mut out = String::with_capacity(seg.len());
    let mut i = 0;
    while i < c.len() {
        if starts_with_http_scheme(&c, i) {
            let start = i;
            while i < c.len() && !is_url_terminator(c[i]) {
                i += 1;
            }
            let url: String = c[start..i].iter().collect();
            out.push_str(&format!(
                "<a href=\"{}\" target=\"_blank\" rel=\"noopener\" style=\"{}\">{}</a>",
                url, LINK_STYLE, url
            ));
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // (a) "foo\nGoogle Maps：<url>" → labeled link, NOT a %0A or raw-url link text.
    #[test]
    fn labeled_map_link_renders_short_label_not_giant_url() {
        let out = render_activity_text("foo\nGoogle Maps：https://maps.example/x");
        assert!(out.contains("<a href=\"https://maps.example/x\""), "got: {out}");
        assert!(out.contains("🗺️ Google Maps"), "got: {out}");
        // The clickable text must be the LABEL, not the URL.
        assert!(out.contains(">🗺️ Google Maps</a>"), "got: {out}");
        // No literal newline left, no %0A encoding of it.
        assert!(!out.contains('\n'), "got: {out}");
        assert!(!out.contains("%0A"), "got: {out}");
    }

    // (b) plain text → just escaped text.
    #[test]
    fn plain_text_is_just_escaped() {
        assert_eq!(render_activity_text("plain text"), "plain text");
        assert_eq!(render_activity_text("Museum & Art"), "Museum &amp; Art");
    }

    // (c) a bare URL → linkified with the URL as the link text.
    #[test]
    fn bare_url_is_linkified_with_url_as_text() {
        let out = render_activity_text("see https://x.com/y");
        assert!(out.contains("<a href=\"https://x.com/y\""), "got: {out}");
        assert!(out.contains(">https://x.com/y</a>"), "got: {out}");
        assert!(out.starts_with("see "), "got: {out}");
    }

    // (d) multi-line "a\nb" → a<br>b.
    #[test]
    fn newlines_become_br() {
        assert_eq!(render_activity_text("a\nb"), "a<br>b");
    }

    // (e) text already containing <span → returned unchanged.
    #[test]
    fn already_html_passthrough() {
        let html = "<span class=\"x\">已是 HTML</span>";
        assert_eq!(render_activity_text(html), html);
    }

    // The real okinawa pattern: a driving leg with an embedded dir URL after 導航.
    #[test]
    fn real_driving_leg_with_navigation_label() {
        let raw = "04:00 自家出發開車：紅樹林 → 大園（約45–60分）\n地址：桃園市\nGoogle Maps 導航：https://www.google.com/maps/dir/A/B";
        let out = render_activity_text(raw);
        // The 導航 line attaches inline (newline collapsed to spaces, no <br> before it).
        assert!(out.contains("<a href=\"https://www.google.com/maps/dir/A/B\""), "got: {out}");
        assert!(out.contains("🗺️ Google Maps 導航"), "got: {out}");
        // The FIRST newline (before 地址) still becomes a <br>.
        assert!(out.contains("地址：桃園市"), "got: {out}");
        assert!(out.contains("<br>地址"), "got: {out}");
        // No malformed-link artifacts.
        assert!(!out.contains("%0A"), "got: {out}");
    }

    // The real okinawa pattern: a meal with an embedded maps/search URL.
    #[test]
    fn real_meal_with_google_maps_search_label() {
        let raw = "晚餐：ステーキハウス88 — 牧志駅步行5分\nGoogle Maps：https://www.google.com/maps/search/abc";
        let out = render_activity_text(raw);
        assert!(out.contains("<a href=\"https://www.google.com/maps/search/abc\""), "got: {out}");
        assert!(out.contains("🗺️ Google Maps"), "got: {out}");
        // Label is clickable text; the long URL is NOT the visible text.
        assert!(out.contains(">🗺️ Google Maps</a>"), "got: {out}");
        assert!(!out.contains("%0A"), "got: {out}");
    }

    // Zh "地圖：" label variant.
    #[test]
    fn zh_ditu_label_variant() {
        let out = render_activity_text("波上宮\n地圖：https://maps.example/n");
        assert!(out.contains("🗺️ 地圖"), "got: {out}");
        assert!(out.contains("<a href=\"https://maps.example/n\""), "got: {out}");
    }

    // "Map: <url>" English label with ASCII colon + space.
    #[test]
    fn english_map_label_ascii_colon() {
        let out = render_activity_text("Beach\nMap: https://maps.example/b");
        assert!(out.contains("🗺️ Map"), "got: {out}");
        assert!(out.contains("<a href=\"https://maps.example/b\""), "got: {out}");
    }

    // A bare URL AND a labeled URL in the same text: labeled stays labeled, bare
    // stays bare — the bare-url pass must NOT re-wrap the labeled anchor's href.
    #[test]
    fn mixed_labeled_and_bare_urls() {
        let raw = "info https://bare.example/a\nGoogle Maps：https://maps.example/c";
        let out = render_activity_text(raw);
        // Labeled one shows the label.
        assert!(out.contains(">🗺️ Google Maps</a>"), "got: {out}");
        // Bare one shows the URL as text.
        assert!(out.contains(">https://bare.example/a</a>"), "got: {out}");
        // Exactly two anchors — the labeled href was not double-linkified.
        assert_eq!(out.matches("<a ").count(), 2, "got: {out}");
    }

    // Quotes in the text are escaped (esc runs first), so the URL char-class
    // never swallows a stray quote into an href.
    #[test]
    fn quotes_are_escaped_before_linkify() {
        let out = render_activity_text("say \"hi\"");
        assert_eq!(out, "say &quot;hi&quot;");
    }

    // Empty input → empty output.
    #[test]
    fn empty_input() {
        assert_eq!(render_activity_text(""), "");
    }

    // A labeled link with NO preceding newline still renders inline.
    #[test]
    fn labeled_link_same_line() {
        let out = render_activity_text("壺屋 Google Maps：https://maps.example/q");
        assert!(out.contains(">🗺️ Google Maps</a>"), "got: {out}");
        assert!(!out.contains("<br>"), "got: {out}");
    }
}
