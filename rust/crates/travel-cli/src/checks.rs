//! checks — the single source of truth for the lint PREDICATES.
//!
//! These pure functions were extracted out of `validate_itinerary.rs` so that
//! WRITE commands (`set-route-segment`, transit writers, `set-activity-booking`,
//! `set-dates`, …) can call exactly the same logic the READ-ONLY lints call —
//! no drift between "what the lint flags" and "what the writer rejects".
//!
//! Nothing here touches the DB; every fn is a pure predicate over strings.
//! Behavior is identical to the copies that previously lived in
//! `validate_itinerary.rs` / `validate.rs` / `set_activity.rs` — this module just
//! relocated them and removed the duplicates.

use chrono::NaiveDate;

// ── map-link / transit-stop predicates ───────────────────────────────────

/// Strip the same noise the dashboard's cleanStopLabel removes, so lint sees the
/// same place string Maps will: drop note after （。、, clock times, ①②③ markers,
/// leading verbs/mode-nouns, "…至/到<place>", trailing 步行.
pub(crate) fn clean_stop(s: &str) -> String {
    let mut out = s
        .split(['\u{FF08}', '(', '\u{3002}', '\u{3001}', ','])
        .next()
        .unwrap_or("")
        .to_string();
    // drop clock times
    out = strip_clock(&out);
    // keep text after the last 至/到 (verb phrase → place)
    if let Some(idx) = out.rfind(['\u{81F3}', '\u{5230}']) {
        out = out[idx + '\u{81F3}'.len_utf8()..].to_string();
    }
    // strip leading ①②③ markers
    out = out
        .trim_start_matches(|c: char| {
            ('\u{2460}'..='\u{2473}').contains(&c) || c == '.' || c == '\u{FF0E}'
        })
        .to_string();
    // strip leading verbs / mode-nouns
    for p in [
        "\u{958B}\u{8ECA}", "\u{81EA}\u{99D5}", "\u{8F49}\u{4E58}", "\u{642D}\u{4E58}", "\u{642D}", "\u{4E58}",
        "\u{63A5}\u{99C1}\u{5DF4}\u{58EB}", "\u{5DF4}\u{58EB}", "\u{55AE}\u{8ECC}\u{96FB}\u{8ECA}", "\u{55AE}\u{8ECC}", "\u{96FB}\u{8ECA}",
    ] {
        if let Some(rest) = out.strip_prefix(p) {
            out = rest.trim_start_matches([':', '\u{FF1A}']).to_string();
        }
    }
    out.trim().trim_end_matches("\u{6B65}\u{884C}").trim().to_string()
}

fn strip_clock(s: &str) -> String {
    // remove HH:MM occurrences
    let bytes: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        // try to match D?D:DD
        let rest: String = bytes[i..].iter().take(5).collect();
        if regex_lite_clock(&rest) {
            // skip the matched clock (4 or 5 chars)
            let len = if bytes.get(i + 2) == Some(&':') { 5 } else { 4 };
            i += len;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

// matches H:MM or HH:MM at start
fn regex_lite_clock(s: &str) -> bool {
    let c: Vec<char> = s.chars().collect();
    if c.len() >= 5 && c[0].is_ascii_digit() && c[1].is_ascii_digit() && c[2] == ':' && c[3].is_ascii_digit() && c[4].is_ascii_digit() {
        return true;
    }
    if c.len() >= 4 && c[0].is_ascii_digit() && c[1] == ':' && c[2].is_ascii_digit() && c[3].is_ascii_digit() {
        return true;
    }
    false
}

/// Decide whether a transit/route stop will produce a BROKEN Google Maps link.
/// `cleaned` is the output of [`clean_stop`]; `raw` is the original stop text.
/// Returns `Some(reason)` when the link is broken, `None` when it's fine.
///
/// This is the guard that stops broken map links reaching the dashboard. The
/// dashboard geocodes the cleaned stop name; if cleaning leaves nothing, or
/// leaves residual junk (a stray paren, a `+步行` tail, a clock time, a pure
/// mode word with no place), the resulting Maps query is wrong or empty.
pub(crate) fn stop_link_problem(cleaned: &str, raw: &str) -> Option<String> {
    let c = cleaned.trim();

    // 1. Cleans to nothing → the stop was pure junk/mode text, no place at all.
    //    (e.g. "（單軌約2分）+步行", "步行", "單軌" with nothing after it)
    if c.is_empty() {
        // Only flag if the raw text actually carried something (an empty raw
        // stop is a malformed chain we report once via the empty-leg path).
        if raw.trim().is_empty() {
            return None;
        }
        return Some("has no usable place name — cleans to empty, so the map link can't geocode".to_string());
    }

    // 2. Residual structural junk survived cleaning → poisons the Maps query.
    const JUNK: [char; 6] = ['\u{FF08}', '\u{FF09}', '(', ')', '+', '\u{FF0B}']; // （ ） ( ) + ＋
    if let Some(bad) = c.chars().find(|ch| JUNK.contains(ch)) {
        return Some(format!("contains stray '{bad}' after cleaning — malformed map query"));
    }

    // 3. A leftover mode tail/word with no real place (e.g. "步行", "單軌電車").
    const MODE_ONLY: [&str; 6] = [
        "\u{6B65}\u{884C}", "\u{55AE}\u{8ECC}", "\u{55AE}\u{8ECC}\u{96FB}\u{8ECA}",
        "\u{5DF4}\u{58EB}", "\u{96FB}\u{8ECA}", "\u{63A5}\u{99C1}\u{5DF4}\u{58EB}",
    ]; // 步行 單軌 單軌電車 巴士 電車 接駁巴士
    if MODE_ONLY.contains(&c) {
        return Some("is only a transport word, not a place — map link can't geocode".to_string());
    }

    None
}

/// Emit (to stderr) the standard "the dashboard renders ZH by default, but you
/// only updated the English field" warning. The trip dashboard defaults to
/// Traditional Chinese and reads separate `*_zh` columns, so an English-only
/// edit silently does NOT change what the default page shows. Call this from an
/// English-side mutation when a corresponding `*_zh` value already exists and
/// the command did not also set it. `zh_hint` is the copy-pasteable fix command.
pub(crate) fn warn_zh_stale(field: &str, zh_hint: &str) {
    eprintln!(
        "⚠ note: updated the English {field}, but the dashboard shows Traditional Chinese by \
         default and its {field}_zh still holds the OLD text — the change won't appear on the \
         default page until you also run: {zh_hint}"
    );
}

/// Normalize a user-typed route mode to one of the three canonical Google Maps
/// travel modes: `transit` | `walking` | `driving`. Accepts the common natural
/// aliases a user (or agent) reaches for — `walk`, `monorail`, `bus`, `train`,
/// `rail`, `taxi`, `car`, `ferry`, etc. — so the route writers stop rejecting
/// obvious synonyms. Returns `Err` (listing the canonical set) only for a value
/// with no sensible mapping. Single source of truth: both `set-route-segment`
/// and `set-route-segments-bulk` validate through here.
pub(crate) fn normalize_mode(mode: &str) -> Result<&'static str, String> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "transit" | "monorail" | "rail" | "train" | "bus" | "subway" | "metro" | "tram"
        | "ferry" | "boat" | "single" | "單軌" | "電車" | "巴士" | "公車" => Ok("transit"),
        "walking" | "walk" | "foot" | "on foot" | "步行" | "走路" => Ok("walking"),
        "driving" | "drive" | "car" | "taxi" | "cab" | "計程車" | "開車" | "自駕" => Ok("driving"),
        other => Err(format!(
            "mode must be transit|walking|driving (or an alias like walk/monorail/bus/taxi); got {other:?}"
        )),
    }
}

/// Validate that a place string will produce a working Google Maps link as a
/// transit/route stop. Returns `Err(reason)` for a broken stop, `Ok(())` when
/// fine. This is the WRITE-TIME guard: `set-route-segment` / transit writers
/// call it so a bad stop is rejected at creation, never reaching the dashboard.
pub(crate) fn check_stop_linkable(stop: &str) -> Result<(), String> {
    match stop_link_problem(&clean_stop(stop), stop.trim()) {
        Some(reason) => Err(reason),
        None => Ok(()),
    }
}

/// Country of a place by keyword. `None` = unknown (don't flag). Used by the
/// cross-country map-leg lint (a TW place + a JP place in one leg draws an
/// ocean-spanning route).
pub(crate) fn place_country(s: &str) -> Option<&'static str> {
    // ONLY unambiguous Taiwan markers. A cross-country guard's worst failure is a
    // FALSE POSITIVE (blocking a legit same-country edit), so any substring that
    // also appears in common Japanese place names is DISQUALIFIED. Notably 橋
    // (bridge) was removed: it is everywhere in Japan (日本橋/心斎橋/京橋/新橋/
    // 渚橋…) and never reliably means Taiwan — it caused real JP→JP legs (e.g.
    // 渚橋 next to iias豊崎 in Okinawa, or any 日本橋 edit in Tokyo) to be wrongly
    // rejected as ocean-spanning. The remaining markers are place names that do
    // not occur in Japanese toponyms. Traditional/simplified 台/臺 variants both
    // listed so 臺北/臺灣 also classify.
    const TW: [&str; 10] = [
        "\u{7D05}\u{6A39}\u{6797}", "\u{6DE1}\u{6C34}", "\u{65B0}\u{5317}", "\u{53F0}\u{5317}", "\u{81FA}\u{5317}",
        "\u{6843}\u{5712}", "\u{5927}\u{5712}", "TPE", "\u{53F0}\u{7063}", "\u{81FA}\u{7063}",
    ]; // 紅樹林 淡水 新北 台北 臺北 桃園 大園 TPE 台灣 臺灣
    const JP: [&str; 10] = [
        "\u{90A3}\u{8987}", "\u{5B89}\u{91CC}", "\u{6C96}\u{7E04}", "\u{9996}\u{91CC}", "\u{570B}\u{969B}\u{901A}",
        "\u{725F}\u{6587}", "OKA", "\u{6771}\u{4EAC}", "\u{4EAC}\u{90FD}", "\u{5927}\u{962A}",
    ]; // 那覇 安里 沖繩 首里 國際通 牧志 OKA 東京 京都 大阪
    if TW.iter().any(|k| s.contains(k)) {
        return Some("TW");
    }
    if JP.iter().any(|k| s.contains(k)) {
        return Some("JP");
    }
    None
}

/// True when the text names a rail/bus mode — used to flag a leg stored as
/// `mode=walking` that actually mentions rail/bus.
pub(crate) fn mentions_rail_or_bus(s: &str) -> bool {
    ["\u{7DDA}", "\u{5DF4}\u{58EB}", "\u{55AE}\u{8ECC}", "\u{96FB}\u{8ECA}", "\u{5730}\u{9435}", "\u{6377}\u{904B}", "\u{706B}\u{8ECA}", "JR", "monorail", "rail", "bus", "train"]
        .iter()
        .any(|k| s.contains(k))
}

/// Fail-loud guard bundle for a single (from, to, mode) route segment. Reuses
/// the lint's OWN predicates verbatim so the WRITE guard and the READ-ONLY lint
/// in `validate_itinerary.rs` agree exactly — no drift.
///
/// Mirrors `validate_itinerary::lint`'s per-leg logic, in the same order:
///   (#3) each of from/to must form a usable Maps stop: reject if it cleans to
///        empty, retains stray （）()＋+, carries a clock time, or is a
///        mode-only word (步行/單軌/巴士…). The segment's `mode` column already
///        carries the travel mode, so a mode word standing as a PLACE is a bug.
///   (#4) cross-country ground leg: compute place_country on the CLEANED stops
///        (matching the lint, which cleans before comparing) and reject when
///        both are known AND differ (an ocean-spanning route, e.g. TW↔JP).
///   (#5) walking-over-rail: if mode=="walking" and `"<from> <to>"` mentions a
///        rail/bus mode → reject (the lint uses the raw from/to as context).
///
/// Returns `Err(reason)` naming the offending stop/rule; `Ok(())` when clean.
/// Pure: no DB, no I/O — so it's unit-tested directly and called before any write.
pub(crate) fn guard_segment(from: &str, to: &str, mode: &str) -> Result<(), String> {
    // (#3) per-stop map-link integrity — use the lint's check_stop_linkable,
    // which is exactly clean_stop → stop_link_problem.
    for (label, stop) in [("from", from), ("to", to)] {
        if let Err(reason) = check_stop_linkable(stop) {
            return Err(format!("<{label}> \"{stop}\" {reason} [rule: map-link/stop]"));
        }
    }

    // The lint compares place_country on the CLEANED stop strings; do the same
    // so the guard never disagrees with what the lint would flag.
    let from_clean = clean_stop(from);
    let to_clean = clean_stop(to);

    // (#4) cross-country ground leg — both known AND different.
    if let (Some(a), Some(b)) = (place_country(&from_clean), place_country(&to_clean)) {
        if a != b {
            return Err(format!(
                "cross-country ground leg: \"{from}\" ({a}) → \"{to}\" ({b}) would draw an ocean-spanning route [rule: cross-country]"
            ));
        }
    }

    // (#5) walking leg that actually rides rail/bus — context is the raw pair
    // (matches the lint's `format!("{from} {to}")` ctx for route legs).
    if mode == "walking" && mentions_rail_or_bus(&format!("{from} {to}")) {
        return Err(format!(
            "mode=walking but \"{from} {to}\" names a rail/bus leg — set mode=transit [rule: walking-over-rail]"
        ));
    }

    Ok(())
}

/// A meal "has a pin" if it carries the ｜map:/|map: marker or an embedded URL.
pub(crate) fn meal_has_pin(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("\u{FF5C}map:") // ｜map:
        || lower.contains("|map:")
        || lower.contains("http://")
        || lower.contains("https://")
}

/// Extract the Google-Maps URLs embedded in a free-text string (activity title).
/// Returns every `http…` token that looks like a Maps link.
pub(crate) fn extract_map_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find("http") {
        let tail = &rest[pos..];
        // take until whitespace
        let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        let url = &tail[..end];
        if url.contains("google.com/maps") || url.contains("maps.google") || url.contains("/maps/") {
            out.push(url.to_string());
        }
        rest = &tail[end.min(tail.len())..];
        if rest.is_empty() {
            break;
        }
    }
    out
}

/// WRITE-TIME guard for an activity title that may carry an embedded Google-Maps
/// link. The dashboard's activity-text linkifier takes the URL token verbatim and
/// the `/maps/dir/?...&...` query form breaks: the linkifier truncates at the first
/// `&`, producing a dead link. The path form (`/maps/search/<place>`) and the
/// `?q=lat,lon` form have no `&`, so they survive.
///
/// Returns `Err(reason)` naming the offending URL (and suggesting the path form)
/// when any embedded Maps URL contains `&`; `Ok(())` when the title has no embedded
/// URL or only clean path-form URLs. Same predicate the read-only map-link lint
/// (`validate_itinerary::validate_map_links`) flags — no drift between lint + writer.
pub(crate) fn check_title_map_url(title: &str) -> Result<(), String> {
    for url in extract_map_urls(title) {
        if url.contains('&') {
            return Err(format!(
                "embedded Map URL contains '&' and will be truncated by the dashboard linkifier (broken link): {url}\n  Use the path form https://www.google.com/maps/search/<place> (or ?q=lat,lon) — no '&' query params"
            ));
        }
    }
    Ok(())
}

// ── date / time predicates ───────────────────────────────────────────────

/// Port of validateIsoDate(input, fieldName): required → format (YYYY-MM-DD) →
/// real-date validity. Error strings embed `field` verbatim and are byte-for-byte
/// identical to the TS originals (verified against live `set-dates` /
/// `set-activity-booking` output). This is the SINGLE canonical ISO-date check;
/// the chrono-backed real-date validity (rejects 2026-13-99 / 2026-02-30) makes
/// it stricter and more correct than the old hand-rolled days_in_month copy.
pub(crate) fn validate_iso_date(input: &str, field: &str) -> Result<(), String> {
    if input.is_empty() {
        return Err(format!("{field} is required"));
    }
    // ^(\d{4})-(\d{2})-(\d{2})$
    let bytes = input.as_bytes();
    let well_formed = input.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && input[0..4].bytes().all(|b| b.is_ascii_digit())
        && input[5..7].bytes().all(|b| b.is_ascii_digit())
        && input[8..10].bytes().all(|b| b.is_ascii_digit());
    if !well_formed {
        return Err(format!("{field} must be YYYY-MM-DD format (got: \"{input}\")"));
    }
    // Real calendar date? (e.g. 2026-13-99 is well-formed but invalid.)
    if NaiveDate::parse_from_str(input, "%Y-%m-%d").is_err() {
        return Err(format!("{field} is not a valid date: \"{input}\""));
    }
    Ok(())
}

/// Validate a 24-hour `HH:MM` clock string. Accepts ONLY exactly two digits,
/// a colon, two digits, with hour 00–23 and minute 00–59. Rejects `9am`,
/// `25:00`, `9:0`, `08:00:00`, empty, leading space, non-ASCII, etc.
///
/// WRITE-TIME guard for activity start/end times — keeps the stored clock in the
/// one shape `parse_time` and the dashboard expect. Wired into the time-writing
/// commands via [`validate_time_flag`] (see set_activity / set_tod / set_flight).
pub(crate) fn validate_hhmm(s: &str) -> Result<(), String> {
    let bytes = s.as_bytes();
    let well_formed = s.len() == 5
        && bytes[2] == b':'
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit();
    if !well_formed {
        return Err(format!("time must be HH:MM (24h) format (got: \"{s}\")"));
    }
    let hh: u32 = s[0..2].parse().unwrap();
    let mm: u32 = s[3..5].parse().unwrap();
    if hh > 23 || mm > 59 {
        return Err(format!("time is not a valid 24h clock: \"{s}\""));
    }
    Ok(())
}

/// Agent-first flag wrapper around [`validate_hhmm`]: validates the value of a
/// named time flag (e.g. `--start`, `--end`, `--dep`, `--arr`) and, on failure,
/// returns ONE actionable error naming both the flag AND the bad value verbatim:
///
///   `error: --start "9am" is not a valid HH:MM time (expected 00:00–23:59)`
///
/// This is the WRITE-TIME guard the time-writing commands call in their
/// arg-parse path, BEFORE any DB connection/write — so an invalid time fails
/// loud and writes NOTHING.
pub(crate) fn validate_time_flag(flag: &str, value: &str) -> Result<(), String> {
    if validate_hhmm(value).is_err() {
        return Err(format!(
            "error: {flag} \"{value}\" is not a valid HH:MM time (expected 00:00\u{2013}23:59)"
        ));
    }
    Ok(())
}

/// Enforce that a start time is not after an end time. Both values must already
/// be valid zero-padded `HH:MM` (validate them with [`validate_time_flag`]
/// first), so a plain string compare orders them correctly. On violation returns
/// an actionable error naming both flags and both values:
///
///   `error: --start "14:00" is after --end "09:00" (start must be ≤ end)`
pub(crate) fn validate_start_le_end(
    start_flag: &str,
    start: &str,
    end_flag: &str,
    end: &str,
) -> Result<(), String> {
    if start > end {
        return Err(format!(
            "error: {start_flag} \"{start}\" is after {end_flag} \"{end}\" (start must be \u{2264} end)"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── clean_stop ────────────────────────────────────────────────────
    #[test]
    fn clean_stop_clean_case() {
        // already a clean place name → unchanged
        assert_eq!(clean_stop("\u{8D64}\u{5DBA}\u{99C5}"), "\u{8D64}\u{5DBA}\u{99C5}"); // 赤嶺駅
    }

    #[test]
    fn clean_stop_malformed_case() {
        // verb + 至 + place（note）+ clock → place only
        assert_eq!(
            clean_stop("\u{8F49}\u{4E58}\u{63A5}\u{99C1}\u{5DF4}\u{58EB}\u{81F3}\u{6843}\u{6A5F}T2"),
            "\u{6843}\u{6A5F}T2"
        ); // 轉乘接駁巴士至桃機T2 → 桃機T2
        assert_eq!(
            clean_stop("\u{5B89}\u{91CC}\u{99C5}\u{FF08}\u{55AE}\u{8ECC}\u{FF09}"),
            "\u{5B89}\u{91CC}\u{99C5}"
        ); // 安里駅（單軌）→ 安里駅
        assert_eq!(clean_stop("05:00 \u{7D05}\u{6A39}\u{6797}").trim(), "\u{7D05}\u{6A39}\u{6797}");
    }

    // ── stop_link_problem / check_stop_linkable ───────────────────────
    #[test]
    fn stop_link_clean_case() {
        assert!(stop_link_problem("\u{8D64}\u{5DBA}\u{99C5}", "\u{8D64}\u{5DBA}\u{99C5}").is_none()); // 赤嶺駅
        assert!(check_stop_linkable("iias \u{6C96}\u{7E04}\u{8C4A}\u{5D0E}").is_ok()); // iias 沖縄豊崎
        // empty stop is intentionally Ok (caught upstream by required-arg checks)
        assert!(check_stop_linkable("").is_ok());
    }

    #[test]
    fn stop_link_malformed_case() {
        // mode-word-only and junk-only stops are rejected
        assert!(check_stop_linkable("\u{6B65}\u{884C}").is_err()); // 步行 (mode word only)
        assert!(check_stop_linkable("\u{55AE}\u{8ECC}").is_err()); // 單軌 (mode word only)
        assert!(check_stop_linkable("\u{FF08}\u{55AE}\u{8ECC}\u{7D04}2\u{5206}\u{FF09}+\u{6B65}\u{884C}").is_err()); // （單軌約2分）+步行
        // residual stray paren after cleaning
        assert!(stop_link_problem("\u{5B89}\u{91CC}(", "\u{5B89}\u{91CC}(").is_some());
    }

    // ── place_country ─────────────────────────────────────────────────
    #[test]
    fn place_country_clean_case() {
        assert_eq!(place_country("Somewhere"), None); // unknown → don't flag
    }

    #[test]
    fn place_country_detects() {
        // The real okinawa day-1 Taiwan-departure stops MUST still classify TW so
        // the genuine Taiwan↔Okinawa ocean-spanning guard keeps working.
        assert_eq!(place_country("\u{7D05}\u{6A39}\u{6797}"), Some("TW")); // 紅樹林
        assert_eq!(place_country("\u{5927}\u{5712}\u{51FA}\u{570B}\u{505C}\u{8ECA}\u{5834}"), Some("TW")); // 大園…
        // Traditional 臺 variants classify TW too.
        assert_eq!(place_country("\u{81FA}\u{5317}"), Some("TW")); // 臺北
        assert_eq!(place_country("\u{81FA}\u{7063}"), Some("TW")); // 臺灣
        // JP markers — incl. 大阪, which must NOT collide with the TW 大園.
        assert_eq!(place_country("\u{5B89}\u{91CC}"), Some("JP")); // 安里
        assert_eq!(place_country("\u{90A3}\u{8987}\u{6A5F}\u{5834}"), Some("JP")); // 那覇機場
        assert_eq!(place_country("\u{5927}\u{962A}"), Some("JP")); // 大阪 (not TW 大園)
    }

    // ── place_country: the 橋(bridge) false-positive class is FIXED ────────
    // 橋 is hopelessly ambiguous: it is everywhere in Japanese place names
    // (日本橋/心斎橋/渚橋…), so it must NEVER classify a stop as Taiwan. These
    // legit JAPAN stops previously got mis-tagged TW and a same-country (JP→JP)
    // edit was wrongly rejected as an ocean-spanning cross-country leg.
    #[test]
    fn place_country_bridge_is_not_taiwan() {
        // 日本橋 (Nihonbashi, Tokyo/Osaka) — was the bug, must not be TW.
        assert_eq!(place_country("\u{65E5}\u{672C}\u{6A4B}"), None);
        // 心斎橋 (Shinsaibashi, Osaka) — must not be TW.
        assert_eq!(place_country("\u{5FC3}\u{658E}\u{6A4B}"), None);
        // 渚橋 (Nagisa Bridge, next to iias豊崎, Okinawa) — the real itinerary case.
        assert_eq!(place_country("\u{6E1A}\u{6A4B}"), None);
        // A bridge stop that ALSO carries a real JP marker → JP (never TW).
        assert_eq!(
            place_country("iias \u{6C96}\u{7E04}\u{8C4A}\u{5D0E}"), // iias 沖縄豊崎
            Some("JP")
        );
        // And the leg that triggered the report: 渚橋(iias豊崎) — the parenthetical
        // carries 豊崎 but the cleaned stop is 渚橋; either way it is NOT TW.
        assert_eq!(
            place_country("\u{6E1A}\u{6A4B}(iias\u{8C4A}\u{5D0E})"), // 渚橋(iias豊崎)
            None
        );
    }

    // ── cross-country guard predicate (#4) ────────────────────────────
    // Exact replica of the reject condition `guard_segment` (set_route_segment.rs)
    // and the validate-itinerary lint apply: reject ONLY when both cleaned stops
    // are KNOWN and DIFFERENT. Proves the guard still catches the real
    // ocean-spanning leg while no longer blocking the legit same-country (JP)
    // bridge leg that the 橋-marker bug used to reject.
    fn cross_country_rejects(from: &str, to: &str) -> bool {
        let from_c = clean_stop(from);
        let to_c = clean_stop(to);
        matches!(
            (place_country(&from_c), place_country(&to_c)),
            (Some(a), Some(b)) if a != b
        )
    }

    #[test]
    fn cross_country_guard_allows_same_country_bridge_leg() {
        // 安里 → 渚橋 — BOTH Japan (Okinawa). Was wrongly rejected because 渚橋
        // matched the 橋 marker → "TW", making it look cross-country. Now: 安里=JP,
        // 渚橋=unknown(None) → NOT rejected (a None side never rejects).
        assert!(
            !cross_country_rejects("\u{5B89}\u{91CC}", "\u{6E1A}\u{6A4B}"), // 安里 → 渚橋
            "a same-country (JP) bridge leg must NOT be rejected"
        );
        // The full reported leg form 安里 → 渚橋(iias豊崎) likewise passes.
        assert!(!cross_country_rejects(
            "\u{5B89}\u{91CC}",                                   // 安里
            "\u{6E1A}\u{6A4B}(iias\u{8C4A}\u{5D0E})"              // 渚橋(iias豊崎)
        ));
    }

    #[test]
    fn cross_country_guard_still_rejects_real_ocean_leg() {
        // 紅樹林 (TW) → 那覇機場 (JP) — the GENUINE ocean-spanning leg. Still rejected.
        assert!(
            cross_country_rejects("\u{7D05}\u{6A39}\u{6797}", "\u{90A3}\u{8987}\u{6A5F}\u{5834}"),
            "the real TW->JP ocean-spanning leg must still be rejected"
        );
        // 大園出國停車場 (TW) → 安里 (JP) — the day-1 departure direction, still caught.
        assert!(cross_country_rejects(
            "\u{5927}\u{5712}\u{51FA}\u{570B}\u{505C}\u{8ECA}\u{5834}", // 大園出國停車場
            "\u{5B89}\u{91CC}"                                          // 安里
        ));
    }

    // ── mentions_rail_or_bus ──────────────────────────────────────────
    #[test]
    fn mentions_rail_or_bus_clean_case() {
        // pure walking text → false
        assert!(!mentions_rail_or_bus("\u{6B65}\u{884C}5\u{5206}")); // 步行5分
    }

    #[test]
    fn mentions_rail_or_bus_positive_case() {
        assert!(mentions_rail_or_bus("\u{55AE}\u{8ECC}\u{7D04}2\u{5206}")); // 單軌約2分
        assert!(mentions_rail_or_bus("take the JR line"));
        assert!(mentions_rail_or_bus("\u{5DF4}\u{58EB}")); // 巴士
    }

    // ── meal_has_pin ──────────────────────────────────────────────────
    #[test]
    fn meal_has_pin_clean_case() {
        // no marker / url → no pin
        assert!(!meal_has_pin("Lunch: near Omoromachi / Shuri"));
    }

    #[test]
    fn meal_has_pin_positive_case() {
        assert!(meal_has_pin("\u{5348}\u{9910}\u{FF1A}X\u{FF5C}map:\u{9996}\u{91CC}\u{6BBF}\u{5167}")); // ｜map:首里殿内
        assert!(meal_has_pin("Dinner |map: somewhere"));
        assert!(meal_has_pin("see https://maps.google.com/x"));
    }

    // ── extract_map_urls ──────────────────────────────────────────────
    #[test]
    fn extract_map_urls_clean_case() {
        // no maps URL present → empty
        assert!(extract_map_urls("just a plain title, no link").is_empty());
        // an http URL that isn't a maps link → not collected
        assert!(extract_map_urls("see https://example.com/foo").is_empty());
    }

    #[test]
    fn extract_map_urls_finds_maps() {
        let t = "drive\nGoogle: https://www.google.com/maps/dir/A/B then walk";
        let u = extract_map_urls(t);
        assert_eq!(u.len(), 1);
        assert!(u[0].contains("/maps/dir/A/B"));
    }

    // ── check_title_map_url ───────────────────────────────────────────
    #[test]
    fn check_title_map_url_passes_clean() {
        // no embedded URL at all → ok
        assert!(check_title_map_url("\u{9996}\u{91CC}\u{57CE}").is_ok()); // 首里城
        // clean path-form /maps/search/ URL (no '&') → ok
        assert!(check_title_map_url(
            "\u{9996}\u{91CC}\u{57CE} Google Maps\u{FF1A}https://www.google.com/maps/search/Shuri+Castle"
        )
        .is_ok());
        // ?q=lat,lon form (no '&') → ok
        assert!(check_title_map_url(
            "Pin https://www.google.com/maps?q=26.2,127.7"
        )
        .is_ok());
    }

    #[test]
    fn check_title_map_url_rejects_ampersand_dir_form() {
        // the /maps/dir/?...&... query form the dashboard linkifier truncates
        let err = check_title_map_url(
            "Google Maps\u{FF1A}https://www.google.com/maps/dir/?api=1&destination=Naha"
        )
        .unwrap_err();
        assert!(err.contains("destination=Naha"), "error must name the offending URL: {err}");
        assert!(err.contains("path form"), "error must suggest the path form: {err}");
    }

    // ── validate_iso_date ─────────────────────────────────────────────
    #[test]
    fn iso_date_clean_case() {
        assert!(validate_iso_date("2026-06-12", "start date").is_ok());
    }

    #[test]
    fn iso_date_malformed_case() {
        assert_eq!(
            validate_iso_date("", "start date"),
            Err("start date is required".to_string())
        );
        assert_eq!(
            validate_iso_date("2026/03/01", "start date"),
            Err("start date must be YYYY-MM-DD format (got: \"2026/03/01\")".to_string())
        );
        assert_eq!(
            validate_iso_date("2026-13-99", "start date"),
            Err("start date is not a valid date: \"2026-13-99\"".to_string())
        );
        // field name is embedded verbatim — --book-by caller contract preserved
        assert_eq!(
            validate_iso_date("2026/03/01", "--book-by"),
            Err("--book-by must be YYYY-MM-DD format (got: \"2026/03/01\")".to_string())
        );
        assert_eq!(
            validate_iso_date("2026-02-30", "--book-by"),
            Err("--book-by is not a valid date: \"2026-02-30\"".to_string())
        );
    }

    // ── validate_hhmm ─────────────────────────────────────────────────
    #[test]
    fn hhmm_accepts_valid() {
        assert!(validate_hhmm("08:00").is_ok());
        assert!(validate_hhmm("23:59").is_ok());
        assert!(validate_hhmm("00:00").is_ok());
    }

    #[test]
    fn hhmm_rejects_invalid() {
        assert!(validate_hhmm("9am").is_err());
        assert!(validate_hhmm("24:00").is_err());
        assert!(validate_hhmm("25:00").is_err());
        assert!(validate_hhmm("8:0").is_err());
        assert!(validate_hhmm("9:0").is_err());
        assert!(validate_hhmm("08:00:00").is_err());
        assert!(validate_hhmm("").is_err());
        assert!(validate_hhmm(" 8:00").is_err());
        assert!(validate_hhmm("\u{516B}\u{9EDE}").is_err()); // 八點
        assert!(validate_hhmm("08:60").is_err()); // bad minute
    }

    // ── validate_time_flag ────────────────────────────────────────────
    #[test]
    fn time_flag_accepts_valid() {
        assert!(validate_time_flag("--start", "08:00").is_ok());
        assert!(validate_time_flag("--end", "23:59").is_ok());
        assert!(validate_time_flag("--dep", "00:00").is_ok());
    }

    #[test]
    fn time_flag_rejects_invalid_and_names_flag_and_value() {
        // Exact actionable message contract: names BOTH the flag and the bad value.
        let err = validate_time_flag("--start", "9am").unwrap_err();
        assert_eq!(
            err,
            "error: --start \"9am\" is not a valid HH:MM time (expected 00:00\u{2013}23:59)"
        );
        // Out-of-range clock is rejected too, naming the flag passed in.
        let err = validate_time_flag("--arr", "25:00").unwrap_err();
        assert!(err.contains("--arr") && err.contains("\"25:00\""), "got: {err}");
    }

    // ── validate_start_le_end ─────────────────────────────────────────
    #[test]
    fn start_le_end_ok_when_ordered_or_equal() {
        assert!(validate_start_le_end("--start", "09:00", "--end", "11:30").is_ok());
        assert!(validate_start_le_end("--start", "09:00", "--end", "09:00").is_ok()); // equal is fine
    }

    #[test]
    fn start_le_end_rejects_inverted_range() {
        let err = validate_start_le_end("--start", "14:00", "--end", "09:00").unwrap_err();
        assert!(err.contains("--start") && err.contains("\"14:00\""), "got: {err}");
        assert!(err.contains("--end") && err.contains("\"09:00\""), "got: {err}");
        assert!(err.contains("start must be"), "must explain the rule: {err}");
    }
}
