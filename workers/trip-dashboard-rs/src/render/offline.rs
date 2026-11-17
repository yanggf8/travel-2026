//! Offline-support markers on the plan page (offline plan Task 2):
//!
//! - [`view_of`] — the single view classifier ("viewer" / "owner") behind BOTH
//!   the body's `data-view` marker and the `X-TD-View` response header (the
//!   service worker's save gate reads the header and cross-checks the body,
//!   spec §5).
//! - [`plan_page_headers`] — the plan page's response headers (no-store + view).
//! - [`status_bar`] — the hidden save-status / offline-banner elements. The
//!   server pre-fills every caption from `i18n.rs`; the `{maps}`/`{pdfs}`/`{n}`/
//!   `{time}` placeholders stay literal — the inline script (Task 4) fills them
//!   at runtime. Everything is served `hidden`, so the page is byte-identical
//!   in structure with JS off (SSR-first; the Global Constraints).

use super::esc;
use crate::i18n::t;

/// "" (no owner chrome — a viewer/token page) → "viewer"; anything else → "owner".
/// The input is the owner chrome fragment the router already computed.
pub fn view_of(owner_chrome: &str) -> &'static str {
    if owner_chrome.is_empty() {
        "viewer"
    } else {
        "owner"
    }
}

/// The plan page's response headers: `private, no-store` (unchanged — the
/// Cache API is NOT bound by it; the SW stores by explicit rule, spec §4) plus
/// `X-TD-View`, same source as the body's `data-view`.
pub fn plan_page_headers(view: &str) -> [(&'static str, String); 2] {
    [
        ("Cache-Control", "private, no-store".to_string()),
        ("X-TD-View", view.to_string()),
    ]
}

/// The hidden offline status bar + banner, placed before the 目錄. The DOM
/// contract Task 4's script reads and Task 6's probe checks:
///
/// ```html
/// <div class="offline-status" id="offline-status" hidden data-state=""
///      data-l-saving="…" data-l-saved="…{maps}…{pdfs}…" data-l-partial="…{n}…"
///      data-l-kept="…{time}…" data-l-failed="…" data-l-unsupported="…">
///   <span class="offline-msg"></span><ul class="offline-failed" hidden></ul>
///   <button type="button" class="offline-retry" hidden>重試/Retry</button></div>
/// <div class="offline-banner" id="offline-banner" hidden data-l-copy="…{time}…" data-l-relogin="…"></div>
/// ```
pub fn status_bar(lang: &str) -> String {
    format!(
        "<div class=\"offline-status\" id=\"offline-status\" hidden data-state=\"\" \
         data-l-saving=\"{}\" data-l-saved=\"{}\" data-l-partial=\"{}\" data-l-kept=\"{}\" \
         data-l-failed=\"{}\" data-l-unsupported=\"{}\">\
         <span class=\"offline-msg\"></span><ul class=\"offline-failed\" hidden></ul>\
         <button type=\"button\" class=\"offline-retry\" hidden>{}</button></div>\
         <div class=\"offline-banner\" id=\"offline-banner\" hidden data-l-copy=\"{}\" data-l-relogin=\"{}\"></div>",
        esc(t("offlineSaving", lang)),
        esc(t("offlineSaved", lang)),
        esc(t("offlinePartial", lang)),
        esc(t("offlineKeptOld", lang)),
        esc(t("offlineFailed", lang)),
        esc(t("offlineUnsupported", lang)),
        esc(t("offlineRetry", lang)),
        esc(t("offlineCopy", lang)),
        esc(t("offlineRelogin", lang)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extract one double-quoted attribute's value (escaped entities stay
    /// encoded — the assertions only look at ASCII/placeholder text).
    fn attr_value(html: &str, attr: &str) -> String {
        let needle = format!("{attr}=\"");
        let i = html.find(&needle).unwrap_or_else(|| panic!("{attr} missing: {html}"));
        let rest = &html[i + needle.len()..];
        rest[..rest.find('"').expect("unterminated attribute")].to_string()
    }

    #[test]
    fn view_of_maps_empty_chrome_to_viewer() {
        assert_eq!(view_of(""), "viewer");
        assert_eq!(view_of("<div class=\"owner-chrome\"></div>"), "owner");
        assert_eq!(view_of("owner-chrome"), "owner");
    }

    #[test]
    fn plan_page_headers_are_no_store_plus_view() {
        assert_eq!(
            plan_page_headers("viewer"),
            [
                ("Cache-Control", "private, no-store".to_string()),
                ("X-TD-View", "viewer".to_string())
            ]
        );
        assert_eq!(plan_page_headers("owner")[1].1, "owner");
    }

    /// The 10 offline keys (plan's list; `offlineKeptOld` comes from spec §6
    /// step 5) exist as non-empty ZH/EN pairs, EN ASCII per house convention.
    #[test]
    fn offline_i18n_keys_have_zh_and_en() {
        for key in [
            "offlineSaving",
            "offlineSaved",
            "offlinePartial",
            "offlineRetry",
            "offlineFailed",
            "offlineCopy",
            "offlineRelogin",
            "offlineUnsupported",
            "offlineMiss",
            "offlineKeptOld",
        ] {
            let (en, zh) = (t(key, "en"), t(key, "zh"));
            assert!(!en.is_empty() && !zh.is_empty(), "{key} missing a language");
            assert!(en.is_ascii(), "{key} EN must be English: {en}");
            assert!(!zh.is_ascii(), "{key} ZH must be Chinese: {zh}");
        }
    }

    #[test]
    fn status_bar_served_hidden_with_templates() {
        for lang in ["zh", "en"] {
            let bar = status_bar(lang);
            assert!(
                bar.contains("<div class=\"offline-status\" id=\"offline-status\" hidden data-state=\"\""),
                "{lang}: {bar}"
            );
            for attr in [
                "data-l-saving",
                "data-l-saved",
                "data-l-partial",
                "data-l-kept",
                "data-l-failed",
                "data-l-unsupported",
            ] {
                assert!(!attr_value(&bar, attr).is_empty(), "{lang} {attr}: {bar}");
            }
            assert!(
                bar.contains("<span class=\"offline-msg\"></span><ul class=\"offline-failed\" hidden></ul>"),
                "{lang}: {bar}"
            );
            // The retry button carries its label and starts hidden.
            assert!(
                bar.contains(&format!(
                    "<button type=\"button\" class=\"offline-retry\" hidden>{}</button>",
                    esc(t("offlineRetry", lang))
                )),
                "{lang}: {bar}"
            );
            assert!(
                bar.contains("<div class=\"offline-banner\" id=\"offline-banner\" hidden data-l-copy=\""),
                "{lang}: {bar}"
            );
            assert!(bar.contains(" data-l-relogin=\""), "{lang}: {bar}");
            // Templates stay placeholders — the inline script (Task 4) fills them.
            let saved = attr_value(&bar, "data-l-saved");
            assert!(saved.contains("{maps}") && saved.contains("{pdfs}"), "{lang}: {saved}");
            assert!(attr_value(&bar, "data-l-partial").contains("{n}"), "{lang}: {bar}");
            for attr in ["data-l-kept", "data-l-copy"] {
                assert!(attr_value(&bar, attr).contains("{time}"), "{lang} {attr}: {bar}");
            }
            // No script inside the status bar (one-inline-script rule).
            assert!(!bar.contains("<script"), "{lang}: {bar}");
        }
        // Spec §7 ZH copy verbatim.
        assert!(status_bar("zh").contains("已存好，可離線查看"), "{}", status_bar("zh"));
        // EN copy carries no CJK.
        assert!(status_bar("en").chars().all(|c| !('\u{4E00}'..='\u{9FFF}').contains(&c)));
    }
}
