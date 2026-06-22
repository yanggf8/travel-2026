//! Pending-booking alerts (feature #3) + transit cheat-sheet (feature #4).
//!
//! Ports of the legacy TS worker's `renderPendingAlerts` (render.ts:1248-1284)
//! and `renderTransitSummary` (render.ts:1286-1313).
//!
//! Pending "meal" alerts are pending ACTIVITIES whose title matches a meal regex
//! (NOT `session_meals`) — see render.ts:1242-1246. The split between the two
//! alert blocks is by `is_meal_title(title) == meal_only`.

use crate::model::Plan;
use super::{esc, esc_url_attr};
use crate::i18n::t;

/// True when a (trimmed) activity title begins with a meal label. Hand-rolled
/// port of `MEAL_TITLE_RE = /^(早餐|午餐|晚餐|宵夜|breakfast|lunch|dinner|supper)/i`
/// (render.ts:1242). ASCII alternatives are matched case-insensitively; the
/// CJK alternatives match verbatim.
pub fn is_meal_title(title: &str) -> bool {
    let t = title.trim_start();
    const CJK: [&str; 4] = ["早餐", "午餐", "晚餐", "宵夜"];
    for p in CJK {
        if t.starts_with(p) {
            return true;
        }
    }
    // Case-insensitive ASCII prefixes.
    let lower = t.to_ascii_lowercase();
    const ASCII: [&str; 4] = ["breakfast", "lunch", "dinner", "supper"];
    ASCII.iter().any(|p| lower.starts_with(p))
}

/// Split an activity title into `(visible_title, embedded_maps_url)` by stripping
/// a trailing `"\nGoogle Maps：<url>"` line (full-width `：` or ASCII `:`, the
/// `Google Maps` keyword case-insensitive). Port of render.ts:1264-1266.
///
/// Returns the cleaned, trimmed title and `Some(url)` when a maps URL was found.
fn strip_embedded_maps(raw: &str) -> (String, Option<String>) {
    // Scan each LINE of the raw string directly (no lowercased copy → no
    // byte-offset misalignment when a char's lowercase form changes its byte
    // length, e.g. İ→i̇ / ẞ→ss). `match_indices('\n')` yields valid char
    // boundaries in `raw` itself, so the slices below are always safe.
    let needle = "google maps";
    for (nl, _) in raw.match_indices('\n') {
        // The line content starts right after the newline; skip leading spaces.
        let line = &raw[nl + 1..];
        if line.trim_start().to_lowercase().starts_with(needle) {
            // Everything from the newline onward is the embedded tail → drop it.
            let visible = raw[..nl].trim().to_string();
            // Pull a URL out of the tail (after `：`/`:`), if present.
            let url = extract_maps_url(line);
            return (visible, url);
        }
    }
    (raw.trim().to_string(), None)
}

/// Extract the first `http(s)://…` token from a "Google Maps：<url>" tail,
/// stopping at whitespace (mirrors the `(https?:\/\/\S+)` capture). Matches the
/// scheme case-insensitively WITHOUT slicing via a lowercased copy's offsets
/// (which would misalign on length-changing lowercase chars).
fn extract_maps_url(tail: &str) -> Option<String> {
    // Walk char boundaries of the RAW tail; test each candidate start against
    // the lowercase scheme prefixes via the raw bytes (ASCII, case-insensitive).
    let bytes = tail.as_bytes();
    for (i, _) in tail.char_indices() {
        let starts_http = bytes[i..].len() >= 7
            && bytes[i..i + 7].eq_ignore_ascii_case(b"http://");
        let starts_https = bytes[i..].len() >= 8
            && bytes[i..i + 8].eq_ignore_ascii_case(b"https://");
        if starts_http || starts_https {
            let rest = &tail[i..];
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            return Some(rest[..end].to_string());
        }
    }
    None
}

/// Parse an ISO date string (`YYYY-MM-DD`, optionally with a `T…` time tail) into
/// epoch milliseconds (UTC midnight when no time component). Hand-rolled so the
/// urgency check is fully unit-testable WITHOUT the `worker::Date` JS runtime —
/// this also sidesteps the `worker::Date::new(...).as_millis()` NaN→u64 hazard
/// (garbage input → `None` → NON-urgent), exactly as the spec's GUARD requires.
fn parse_iso_date_millis(s: &str) -> Option<f64> {
    let date_part = s.trim().split(['T', ' ']).next().unwrap_or("");
    let mut it = date_part.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if it.next().is_some() { return None; } // extra dashes → not a plain date
    if !(1..=12).contains(&m) || y < 1970 || y > 9999 {
        return None;
    }
    // Reject impossible days-of-month (Apr-31, Feb-30, Feb-29 in a common year)
    // instead of silently rolling them forward via days_from_civil.
    if d < 1 || d > days_in_month(y, m) {
        return None;
    }
    Some(days_from_civil(y, m, d) as f64 * 86_400_000.0)
}

/// Days in a given month, accounting for leap years (proleptic Gregorian).
fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 },
        _ => 0,
    }
}

/// Days since 1970-01-01 (proleptic Gregorian). Howard Hinnant's `days_from_civil`.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Urgency: `book_by` non-empty AND parses to a date `<= now_ms`. Empty or
/// unparseable `book_by` → NOT urgent (port of render.ts:1268, with the GUARD).
fn is_urgent(book_by: &str, now_ms: f64) -> bool {
    if book_by.trim().is_empty() {
        return false;
    }
    match parse_iso_date_millis(book_by) {
        Some(ms) if ms > 0.0 => ms <= now_ms,
        _ => false,
    }
}

/// Render one alert `<div>` (port of render.ts:1269-1276). `title` is already
/// the cleaned visible title; `maps_url` is the preferred link (embedded > booking_url).
fn alert_html(title: &str, maps_url: Option<&str>, book_by: &str, urgent: bool, lang: &str) -> String {
    let urgent_class = if urgent { " alert-urgent" } else { "" };
    let icon = if urgent { "\u{26A0}\u{FE0F}" } else { "\u{23F3}" };
    let map_link = match maps_url {
        Some(u) if !u.is_empty() => format!(
            " <a href=\"{}\" target=\"_blank\" rel=\"noopener\" style=\"font-size:12px\">\u{1F5FA}\u{FE0F}</a>",
            esc_url_attr(u)
        ),
        _ => String::new(),
    };
    // When book_by is empty, OMIT the "Book by …" suffix entirely (spec).
    let book_by_suffix = if book_by.trim().is_empty() {
        String::new()
    } else {
        format!(" \u{2014} {} {}", t("bookBy", lang), esc(book_by))
    };
    format!(
        "<div class=\"alert{urgent_class}\">\
           <span class=\"alert-icon\">{icon}</span>\
           <div class=\"alert-text\">\
             <strong>{title}</strong>{map_link}{book_by_suffix}\
           </div>\
         </div>",
        title = esc(title),
    )
}

/// Render pending-booking alerts for the given `meal_only` slice. `now()` is a
/// lazy supplier of epoch millis — invoked AT MOST ONCE, and only when the first
/// matching alert is actually emitted. This single pass replaces the former
/// pre-scan + render double scan (each activity's meal predicate ran twice), and
/// still avoids calling `worker::Date::now()` on plans with no pending bookings.
/// Testable core: tests pass a closure returning a fixed `now_ms`.
fn render_pending_alerts_with<F: FnMut() -> f64>(
    plan: &Plan, lang: &str, meal_only: bool, mut now: F,
) -> String {
    let mut out = String::new();
    let mut now_ms: Option<f64> = None;
    for day in &plan.days {
        for sess in &day.sessions {
            for a in &sess.activities {
                if a.booking_status != "pending" {
                    continue;
                }
                if is_meal_title(&a.title) != meal_only {
                    continue;
                }
                let (title, embedded_url) = strip_embedded_maps(&a.title);
                // Prefer the embedded maps URL; else the booking_url.
                let maps_url = embedded_url
                    .or_else(|| (!a.booking_url.is_empty()).then(|| a.booking_url.clone()));
                // Resolve "now" once, lazily, on the first emitted alert.
                let n = *now_ms.get_or_insert_with(&mut now);
                let urgent = is_urgent(&a.book_by, n);
                out.push_str(&alert_html(&title, maps_url.as_deref(), &a.book_by, urgent, lang));
            }
        }
    }
    out
}

/// Public entry: render pending-booking alerts. `meal_only=false` → non-meal
/// activities (rendered before the booking summary); `meal_only=true` → meal
/// activities (rendered after the day cards). Mirrors render.ts:1388 & 1393.
///
/// "now" comes from `worker::Date::now()` (epoch millis), resolved lazily so the
/// JS boundary is never crossed for a plan with no pending bookings.
pub fn render_pending_alerts(plan: &Plan, lang: &str, meal_only: bool) -> String {
    render_pending_alerts_with(plan, lang, meal_only, || worker::Date::now().as_millis() as f64)
}

/// Render the transit cheat-sheet (feature #4) — port of render.ts:1290-1313.
/// Picks the `dest:lang` key-line bucket and the matching hotel-station column
/// (`_zh` when `lang != "en"`). Returns `""` when there is NO station AND no
/// key lines (render.ts:1289 `if (!transit) return ''`).
pub fn render_transit_summary(plan: &Plan, lang: &str) -> String {
    let dest = plan.plan_id.replace('-', "_");
    let want_zh = lang != "en";
    let lang_key = if want_zh { "zh" } else { "en" };

    let key_lines: Vec<&str> = plan.transit_key_lines.iter()
        .filter(|(d, l, _)| d == &dest && l == lang_key)
        .map(|(_, _, line)| line.as_str())
        .collect();
    let hotel_station = if want_zh {
        &plan.transit_hotel_station_zh
    } else {
        &plan.transit_hotel_station
    };

    if hotel_station.is_empty() && key_lines.is_empty() {
        return String::new();
    }

    let mut lines_html = String::new();
    for l in &key_lines {
        lines_html.push_str(&format!("<div>\u{2022} {}</div>", esc(l)));
    }

    format!(
        "<div class=\"transit-summary\">\
           <h2>{cheat}</h2>\
           <div style=\"font-size:13px;margin-bottom:10px\">\
             <strong>{home}:</strong> {station}\
           </div>\
           <div style=\"font-size:12px;color:var(--text-dim);margin-bottom:6px\">{lines}</div>\
           <div style=\"font-size:12px;color:var(--accent);font-weight:500;margin-top:8px\">{daily}</div>\
         </div>",
        cheat = t("transitCheat", lang),
        home = t("homeBase", lang),
        station = esc(hotel_station),
        lines = lines_html,
        daily = t("dailyTransit", lang),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Plan, Day, Session, Activity};

    fn pending(title: &str, book_by: &str, booking_url: &str) -> Activity {
        Activity {
            title: title.into(),
            booking_status: "pending".into(),
            book_by: book_by.into(),
            booking_url: booking_url.into(),
        }
    }
    /// A plan with a single day whose morning session holds the given activities.
    fn plan_with(acts: Vec<Activity>) -> Plan {
        Plan {
            plan_id: "okinawa-2026".into(),
            days: vec![Day {
                day_number: 1, date: "2026-06-12".into(), day_type: "arrival".into(),
                sessions: vec![Session { session_type: "morning".into(), activities: acts, ..Default::default() }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    // ---- is_meal_title ----
    #[test]
    fn meal_title_matches_cjk_and_ascii_prefixes() {
        assert!(is_meal_title("晚餐：ステーキ88"));
        assert!(is_meal_title("午餐 Makishi"));
        assert!(is_meal_title("Lunch at Makishi"));
        assert!(is_meal_title("DINNER reservation"));
        assert!(is_meal_title("  breakfast buffet")); // leading whitespace trimmed
        assert!(!is_meal_title("Churaumi Aquarium"));
        assert!(!is_meal_title("水族館"));
    }

    // ---- parse_iso_date_millis / urgency guard ----
    #[test]
    fn iso_date_parses_and_garbage_is_none() {
        assert!(parse_iso_date_millis("2026-05-01").is_some());
        assert_eq!(parse_iso_date_millis("1970-01-01"), Some(0.0));
        assert!(parse_iso_date_millis("not-a-date").is_none());
        assert!(parse_iso_date_millis("").is_none());
        assert!(parse_iso_date_millis("2026-13-01").is_none()); // bad month
        assert!(parse_iso_date_millis("2026-05").is_none());    // incomplete
    }
    #[test]
    fn urgency_past_deadline_is_urgent_future_is_not() {
        let now = parse_iso_date_millis("2026-06-22").unwrap();
        assert!(is_urgent("2026-05-01", now), "past deadline → urgent");
        assert!(!is_urgent("2026-12-01", now), "future deadline → not urgent");
        assert!(is_urgent("2026-06-22", now), "same-day deadline (<=) → urgent");
    }
    #[test]
    fn urgency_empty_or_garbage_book_by_is_not_urgent() {
        let now = parse_iso_date_millis("2026-06-22").unwrap();
        assert!(!is_urgent("", now));
        assert!(!is_urgent("   ", now));
        assert!(!is_urgent("garbage", now), "unparseable book_by must NOT be urgent");
    }

    // ---- strip_embedded_maps ----
    #[test]
    fn strip_embedded_maps_extracts_url_and_cleans_title() {
        let raw = "Churaumi Aquarium\nGoogle Maps：https://maps.google.com/?q=churaumi";
        let (title, url) = strip_embedded_maps(raw);
        assert_eq!(title, "Churaumi Aquarium");
        assert_eq!(url.as_deref(), Some("https://maps.google.com/?q=churaumi"));
    }
    #[test]
    fn strip_embedded_maps_ascii_colon_and_case_insensitive() {
        let raw = "Foo\ngoogle maps: https://www.google.com/maps/search/foo more";
        let (title, url) = strip_embedded_maps(raw);
        assert_eq!(title, "Foo");
        assert_eq!(url.as_deref(), Some("https://www.google.com/maps/search/foo"));
    }
    #[test]
    fn strip_embedded_maps_none_when_absent() {
        let (title, url) = strip_embedded_maps("Plain Title");
        assert_eq!(title, "Plain Title");
        assert!(url.is_none());
    }
    #[test]
    fn strip_embedded_maps_safe_with_length_changing_lowercase_chars() {
        // 'İ' (U+0130, 2 bytes) lowercases to 'i̇' (3 bytes); the old impl mapped a
        // lowercased-string byte offset onto `raw`, which misaligns/panics. The
        // CJK 'ẞ→ss' shrink is the opposite direction. Both must be safe now.
        for raw in [
            "İCAFE 那覇\nGoogle Maps：https://maps.google.com/?q=cafe",
            "ẞpot 東京\nGoogle Maps：https://maps.google.com/?q=spot",
            "中文標題（很長）\nGoogle Maps：https://maps.google.com/?q=x extra",
        ] {
            let (title, url) = strip_embedded_maps(raw); // must not panic
            assert!(!title.contains("Google Maps"), "embedded line stripped; got: {title}");
            assert!(url.as_deref().unwrap().starts_with("https://maps.google.com/?q="), "got: {url:?}");
        }
    }
    #[test]
    fn extract_maps_url_safe_with_non_ascii_before_scheme() {
        // Non-ASCII before the scheme must not misalign the slice.
        let url = extract_maps_url("：日本語 https://maps.example/x trailing");
        assert_eq!(url.as_deref(), Some("https://maps.example/x"));
    }

    // ---- parse_iso_date_millis: reject impossible day-of-month ----
    #[test]
    fn parse_iso_date_rejects_impossible_days() {
        assert!(parse_iso_date_millis("2026-04-31").is_none(), "Apr has 30 days");
        assert!(parse_iso_date_millis("2026-02-30").is_none(), "Feb never has 30");
        assert!(parse_iso_date_millis("2026-02-29").is_none(), "2026 is not a leap year");
        assert!(parse_iso_date_millis("2026-06-31").is_none(), "Jun has 30 days");
        // Valid dates still parse, incl. a real leap day.
        assert!(parse_iso_date_millis("2026-02-28").is_some());
        assert!(parse_iso_date_millis("2024-02-29").is_some(), "2024 IS a leap year");
        assert!(parse_iso_date_millis("2026-06-15").is_some());
    }

    // ---- render_pending_alerts_at: pending vs non-pending ----
    #[test]
    fn only_pending_activities_are_alerted() {
        let now = parse_iso_date_millis("2026-06-22").unwrap();
        let plan = plan_with(vec![
            pending("Churaumi Aquarium", "2026-12-01", ""),
            Activity { title: "Booked thing".into(), booking_status: "booked".into(), ..Default::default() },
        ]);
        let html = render_pending_alerts_with(&plan, "en", false, || now);
        assert!(html.contains("Churaumi Aquarium"), "got: {html}");
        assert!(!html.contains("Booked thing"), "non-pending must be excluded; got: {html}");
    }

    // ---- urgent past vs future class/icon ----
    #[test]
    fn urgent_past_deadline_gets_warning_icon_and_class() {
        let now = parse_iso_date_millis("2026-06-22").unwrap();
        let plan = plan_with(vec![pending("Churaumi Aquarium", "2026-05-01", "")]);
        let html = render_pending_alerts_with(&plan, "en", false, || now);
        assert!(html.contains("alert-urgent"), "got: {html}");
        assert!(html.contains("\u{26A0}\u{FE0F}"), "warning icon; got: {html}");
        assert!(html.contains("Book by 2026-05-01"), "got: {html}");
    }
    #[test]
    fn future_deadline_gets_hourglass_and_no_urgent_class() {
        let now = parse_iso_date_millis("2026-06-22").unwrap();
        let plan = plan_with(vec![pending("Churaumi Aquarium", "2026-12-01", "")]);
        let html = render_pending_alerts_with(&plan, "en", false, || now);
        assert!(!html.contains("alert-urgent"), "got: {html}");
        assert!(html.contains("\u{23F3}"), "hourglass icon; got: {html}");
    }

    // ---- null/empty book_by => no suffix + not urgent ----
    #[test]
    fn empty_book_by_omits_suffix_and_is_not_urgent() {
        let now = parse_iso_date_millis("2026-06-22").unwrap();
        let plan = plan_with(vec![pending("Churaumi Aquarium", "", "")]);
        let html = render_pending_alerts_with(&plan, "en", false, || now);
        assert!(!html.contains("Book by"), "no book-by suffix when empty; got: {html}");
        assert!(!html.contains("alert-urgent"), "empty book_by → not urgent; got: {html}");
        assert!(html.contains("\u{23F3}"), "hourglass (not urgent); got: {html}");
    }

    // ---- meal vs activity split ----
    #[test]
    fn meal_only_split_separates_meals_from_activities() {
        let now = parse_iso_date_millis("2026-06-22").unwrap();
        let plan = plan_with(vec![
            pending("Churaumi Aquarium", "2026-12-01", ""),
            pending("晚餐：ステーキ88", "2026-12-01", ""),
        ]);
        let non_meal = render_pending_alerts_with(&plan, "en", false, || now);
        assert!(non_meal.contains("Churaumi Aquarium"), "got: {non_meal}");
        assert!(!non_meal.contains("ステーキ88"), "meal must not appear in non-meal block; got: {non_meal}");

        let meal = render_pending_alerts_with(&plan, "en", true, || now);
        assert!(meal.contains("ステーキ88"), "got: {meal}");
        assert!(!meal.contains("Churaumi Aquarium"), "activity must not appear in meal block; got: {meal}");
    }

    // ---- embedded Google-Maps URL stripping (preferred over booking_url) ----
    #[test]
    fn embedded_maps_url_is_stripped_from_title_and_used_as_link() {
        let now = parse_iso_date_millis("2026-06-22").unwrap();
        let plan = plan_with(vec![pending(
            "Churaumi Aquarium\nGoogle Maps：https://maps.google.com/?q=churaumi",
            "2026-12-01",
            "https://booking.example/should-not-win",
        )]);
        let html = render_pending_alerts_with(&plan, "en", false, || now);
        // Visible title is cleaned (no embedded line / no raw URL dump / no %0A).
        assert!(html.contains("<strong>Churaumi Aquarium</strong>"), "got: {html}");
        assert!(!html.contains("Google Maps"), "embedded line stripped; got: {html}");
        // Embedded URL preferred over booking_url.
        assert!(html.contains("href=\"https://maps.google.com/?q=churaumi\""), "got: {html}");
        assert!(!html.contains("should-not-win"), "embedded URL must win; got: {html}");
    }
    #[test]
    fn booking_url_used_as_link_when_no_embedded_url() {
        let now = parse_iso_date_millis("2026-06-22").unwrap();
        let plan = plan_with(vec![pending("Churaumi Aquarium", "2026-12-01", "https://booking.example/x")]);
        let html = render_pending_alerts_with(&plan, "en", false, || now);
        assert!(html.contains("href=\"https://booking.example/x\""), "got: {html}");
    }

    // ---- transit assembly with / without station+lines ----
    #[test]
    fn transit_summary_renders_station_and_lines_en() {
        let plan = Plan {
            plan_id: "okinawa-2026".into(),
            transit_hotel_station: "Asato".into(),
            transit_hotel_station_zh: "安里".into(),
            transit_key_lines: vec![
                ("okinawa_2026".into(), "en".into(), "Yui Rail: airport - Asato".into()),
                ("okinawa_2026".into(), "zh".into(), "單軌電車：機場－安里".into()),
            ],
            ..Default::default()
        };
        let html = render_transit_summary(&plan, "en");
        assert!(html.contains("Transit Cheat Sheet"), "got: {html}");
        assert!(html.contains("Home base"), "got: {html}");
        assert!(html.contains("Asato"), "got: {html}");
        assert!(html.contains("Yui Rail: airport - Asato"), "got: {html}");
        // EN view must NOT pull the zh line.
        assert!(!html.contains("單軌電車"), "en view leaked zh line; got: {html}");
        assert!(html.contains("Daily transit"), "got: {html}");
    }

    // ---- zh column selection ----
    #[test]
    fn transit_summary_zh_uses_zh_station_and_lines() {
        let plan = Plan {
            plan_id: "okinawa-2026".into(),
            transit_hotel_station: "Asato".into(),
            transit_hotel_station_zh: "安里".into(),
            transit_key_lines: vec![
                ("okinawa_2026".into(), "en".into(), "Yui Rail line".into()),
                ("okinawa_2026".into(), "zh".into(), "單軌電車線".into()),
            ],
            ..Default::default()
        };
        let html = render_transit_summary(&plan, "zh");
        assert!(html.contains("交通速查表"), "got: {html}");
        assert!(html.contains("安里"), "zh station; got: {html}");
        assert!(html.contains("單軌電車線"), "zh line; got: {html}");
        assert!(!html.contains("Yui Rail line"), "zh view leaked en line; got: {html}");
        assert!(!html.contains("Asato"), "zh view leaked en station; got: {html}");
    }

    #[test]
    fn transit_summary_empty_when_no_station_and_no_lines() {
        let plan = Plan { plan_id: "okinawa-2026".into(), ..Default::default() };
        assert_eq!(render_transit_summary(&plan, "en"), "");
        assert_eq!(render_transit_summary(&plan, "zh"), "");
    }

    // station present but no key lines → still renders (non-empty).
    #[test]
    fn transit_summary_renders_with_station_only() {
        let plan = Plan {
            plan_id: "okinawa-2026".into(),
            transit_hotel_station: "Asato".into(),
            ..Default::default()
        };
        let html = render_transit_summary(&plan, "en");
        assert!(!html.is_empty());
        assert!(html.contains("Asato"), "got: {html}");
    }
}
