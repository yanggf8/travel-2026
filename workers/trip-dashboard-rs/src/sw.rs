//! Same-origin service worker for offline copies of the plan page.
//!
//! The SW body lives in [`sw.js`] as a compiled-in string (same pattern as
//! `styles.rs`) and is served at `/sw.js` with no auth and no Turso read. The
//! constants below are the single source of truth — the literals inside
//! `sw.js` are locked to them by `script_constants_match_rust`. UI text (the
//! offline-miss page) is filled from `i18n.rs` at serve time, so no CJK ever
//! lives in the JS source. Rollback is NOT a flag: `router.rs` serves
//! [`KILL_SWITCH`] at `/sw.js` and the deploy is redone (plan Task 7 Step 4).

use crate::i18n;

const SW_TEMPLATE: &str = include_str!("sw.js");

// These constants are the Rust-side source of truth that sw.js's literals are
// locked to (script_constants_match_rust); production code reads them only via
// tests, and KILL_SWITCH is served exclusively during a rollback deploy (plan
// Task 7 Step 4) — hence the allow.
#[allow(dead_code)]
mod consts {
    /// The only URL parameters a cache key keeps, in canonical order. `lang` is
    /// defaulted to `zh` and always written; the owner storage key drops `token`.
    pub const KEY_PARAMS: [&str; 3] = ["plan", "lang", "token"];

    /// Fixed cache names — deliberately NOT versioned with the SW version, so a
    /// SW update never drops offline copies. `activate` must never delete these.
    pub const CACHE_NAMES: [&str; 3] = ["td-viewer", "td-owner", "td-index"];

    /// Network-first timeout for plan-page HTML (spec §6 資源策略).
    pub const NET_TIMEOUT_MS: u32 = 4000;

    /// The rollback script served at `/sw.js` during a kill-switch deploy.
    pub const KILL_SWITCH: &str = include_str!("sw_kill.js");
}
#[allow(unused_imports)] // re-export consumed by tests only (see module note)
pub use consts::{CACHE_NAMES, KEY_PARAMS, KILL_SWITCH, NET_TIMEOUT_MS};

/// The SW script with the offline-miss text filled in for both languages (the
/// SW picks by URL `lang` at runtime). Escaping happens BEFORE the fill, so
/// i18n text can never break out of the JS string literal.
pub fn script() -> String {
    i18n::fill(
        SW_TEMPLATE,
        &[
            ("tdMissZh", &js_str_escape(i18n::t("offlineMiss", "zh"))),
            ("tdMissEn", &js_str_escape(i18n::t("offlineMiss", "en"))),
        ],
    )
}

/// Escape text for a JS single-quoted string literal: backslash, quote,
/// newline (a raw one is a syntax error) and `<` (so a `</script>` inside the
/// text can never terminate the script tag when the file is served inline).
pub fn js_str_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '<' => out.push_str("\\u003c"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

/// Response headers for `/sw.js`: JS content type, and `no-cache` so browsers
/// revalidate the script on every load (spec §6 資源策略 — the browser owns
/// this file's lifecycle; the SW never caches it itself).
pub fn sw_headers() -> [(&'static str, &'static str); 2] {
    [
        ("Content-Type", "application/javascript; charset=utf-8"),
        ("Cache-Control", "no-cache"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// script() is the raw sw.js template with the offlineMiss text filled in
    /// for BOTH languages (the SW picks by URL lang at runtime), JS-escaped so
    /// i18n text can never break out of the string literal. Post-T2-merge the
    /// filled text is the real ZH/EN copy; until then t() returns "" and this
    /// still locks the fill+escape pipeline and the no-placeholder-left rule.
    #[test]
    fn script_fills_miss_text_both_langs_escaped() {
        let s = script();
        assert_eq!(
            s,
            i18n::fill(
                SW_TEMPLATE,
                &[
                    ("tdMissZh", &js_str_escape(i18n::t("offlineMiss", "zh"))),
                    ("tdMissEn", &js_str_escape(i18n::t("offlineMiss", "en"))),
                ],
            )
        );
        assert!(!s.contains("{tdMiss"), "placeholders must be filled");
    }

    #[test]
    fn js_str_escape_escapes_quote_backslash_lt_newline() {
        assert_eq!(
            js_str_escape("a'b\\c<d\ne'"),
            "a\\'b\\\\c\\u003cd\\ne\\'"
        );
    }

    /// The constants sw.js hardcodes as literals must be assembled from the Rust
    /// constants, so the two can never drift.
    #[test]
    fn script_constants_match_rust() {
        let s = script();
        assert!(s.contains(&format!("var KEY_PARAMS=['{}'];", KEY_PARAMS.join("','"))));
        assert!(s.contains(&format!("var NET_TIMEOUT_MS={};", NET_TIMEOUT_MS)));
        assert!(s.contains(&format!("var CACHE_NAMES=['{}'];", CACHE_NAMES.join("','"))));
    }

    /// UI text is filled from i18n at serve time — the sw.js source itself never
    /// carries a CJK literal.
    #[test]
    fn sw_js_has_no_cjk() {
        assert!(!SW_TEMPLATE.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)));
    }

    #[test]
    fn kill_switch_unregisters_and_deletes_td_caches() {
        assert!(KILL_SWITCH.contains("unregister()"));
        assert!(KILL_SWITCH.contains("indexOf('td-')===0"));
    }

    #[test]
    fn sw_headers_no_cache_js() {
        assert_eq!(
            sw_headers(),
            [
                ("Content-Type", "application/javascript; charset=utf-8"),
                ("Cache-Control", "no-cache"),
            ]
        );
    }
}
