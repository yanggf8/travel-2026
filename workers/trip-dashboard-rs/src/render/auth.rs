//! Auth-related HTML pages and fragments.

use super::{esc, page};
use crate::i18n::t;

pub fn sign_in_page(lang: &str) -> String {
    let body = format!(
        r#"<div class="auth-page"><h1>{}</h1><a class="gh-btn" href="/auth/login">{}</a></div>"#,
        esc(t("ownerSignIn", lang)),
        esc(t("signIn", lang)),
    );
    page(t("signIn", lang), &body, lang)
}

pub fn signed_in_banner(login: &str, lang: &str) -> String {
    format!(
        r#"<div class="signin-banner">{} <strong>{}</strong> · <a href="/auth/logout">{}</a></div>"#,
        esc(t("signedInAs", lang)),
        esc(login),
        esc(t("logout", lang)),
    )
}

pub fn not_authorized_page(login: &str, lang: &str) -> String {
    let body = format!(
        r#"<div class="auth-page"><h1>{}</h1><p>{} <strong>{}</strong></p><p>{}</p><p><a href="/auth/logout">{}</a> · <a href="/auth/login">{}</a></p></div>"#,
        esc(t("notAuthorized", lang)),
        esc(t("signedInAs", lang)),
        esc(login),
        esc(t("notAuthorizedBody", lang)),
        esc(t("logout", lang)),
        esc(t("ownerSignIn", lang)),
    );
    page(t("notAuthorized", lang), &body, lang)
}

pub fn bad_share_page(lang: &str) -> String {
    let body = format!(
        r#"<div class="auth-page"><h1>{}</h1><p>{}</p><p class="auth-hint"><a href="/auth/login">{}</a></p></div>"#,
        esc(t("badShare", lang)),
        esc(t("badShareBody", lang)),
        esc(t("ownerSignIn", lang)),
    );
    page(t("badShare", lang), &body, lang)
}

#[allow(dead_code)]
pub fn logged_out_page(lang: &str) -> String {
    let body = format!(
        r#"<div class="auth-page"><h1>{}</h1><p><a class="gh-btn" href="/auth/login">{}</a></p></div>"#,
        esc(t("logout", lang)),
        esc(t("signIn", lang)),
    );
    page(t("signIn", lang), &body, lang)
}

pub fn oauth_error_page(message: &str, lang: &str) -> String {
    let body = format!(
        r#"<div class="auth-page"><h1>{}</h1><p>{}</p><p><a href="/auth/login">{}</a></p></div>"#,
        esc(t("notAuthorized", lang)),
        esc(message),
        esc(t("signIn", lang)),
    );
    page(t("notAuthorized", lang), &body, lang)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_authorized_escapes_script_in_login() {
        let html = not_authorized_page("<script>alert(1)</script>", "en");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn auth_pages_use_auth_paths_not_bare_login() {
        for html in [
            sign_in_page("en"),
            not_authorized_page("other", "en"),
            bad_share_page("en"),
            logged_out_page("en"),
            oauth_error_page("oops", "en"),
        ] {
            assert!(html.contains("/auth/login"), "missing /auth/login in auth page");
            assert!(!html.contains(r#"href="/login""#), "bare /login href found");
        }
        let banner = signed_in_banner("yanggf8", "en");
        assert!(banner.contains("/auth/logout"));
        assert!(!banner.contains(r#"href="/logout""#));
        assert!(!banner.contains(r#"href="/login""#));
    }
}