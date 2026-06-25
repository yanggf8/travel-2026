//! Logged-in owner copies a viewer share URL for others. Recipients open the
//! copied link with no login — just `?plan=` + per-plan share token.

use std::collections::HashMap;

use super::esc;
use crate::i18n::t;

/// One-shot clipboard handler for every `.copy-share-btn` on the page.
pub const COPY_SCRIPT: &str = r#"<script>
(function(){
  function label(key,zh){return zh?(key==='ok'?'已複製！':'複製失敗'):(key==='ok'?'Copied!':'Copy failed');}
  document.addEventListener('click',function(e){
    var btn=e.target.closest('.copy-share-btn');
    if(!btn) return;
    e.preventDefault();
    e.stopPropagation();
    var url=btn.getAttribute('data-copy-url');
    if(!url) return;
    var zh=document.documentElement.lang!=='en';
    var orig=btn.textContent;
    function flash(ok){btn.textContent=label(ok?'ok':'fail',zh);btn.classList.toggle('copy-share-ok',ok);setTimeout(function(){btn.textContent=orig;btn.classList.remove('copy-share-ok');},2000);}
    function fallback(){try{var ta=document.createElement('textarea');ta.value=url;ta.style.position='fixed';ta.style.left='-9999px';document.body.appendChild(ta);ta.select();document.execCommand('copy');document.body.removeChild(ta);flash(true);}catch(_){flash(false);}}
    if(navigator.clipboard&&navigator.clipboard.writeText){navigator.clipboard.writeText(url).then(function(){flash(true);}).catch(fallback);}else{fallback();}
  },true);
})();
</script>"#;

/// Build maps from share-token rows (query must be `ORDER BY created_at DESC`).
/// - `token_to_plan`: every token → hyphenated plan slug (auth; order-independent `insert`)
/// - `plan_slug_to_token`: hyphenated slug → a valid share token (first seen = newest by created_at, second granularity)
pub fn build_share_maps(rows: &[(String, String)]) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut token_to_plan = HashMap::new();
    let mut plan_slug_to_token = HashMap::new();
    for (token, plan_id) in rows {
        let slug = plan_id.replace('_', "-");
        token_to_plan.insert(token.clone(), slug.clone());
        plan_slug_to_token.entry(slug).or_insert_with(|| token.clone());
    }
    (token_to_plan, plan_slug_to_token)
}

/// Shareable viewer URL (view-scope token only — never owner secret or session).
pub fn share_url(public_origin: &str, plan_slug: &str, token: &str) -> String {
    let origin = public_origin.trim_end_matches('/');
    format!("{origin}/?plan={plan_slug}&token={token}")
}

pub fn copy_button(share_url: &str, lang: &str) -> String {
    format!(
        r#"<button type="button" class="copy-share-btn" data-copy-url="{}">{}</button>"#,
        esc(share_url),
        esc(t("copyShareLink", lang)),
    )
}

/// Owner chrome when logged in: signed-in label + copy (or missing hint) + logout.
pub fn owner_plan_chrome(
    plan_slug: &str,
    share_token: Option<&str>,
    public_origin: &str,
    owner_login: &str,
    lang: &str,
) -> String {
    let mut h = String::from(r#"<div class="owner-chrome">"#);
    h.push_str(&format!(
        r#"<span class="owner-chrome-user">{} <strong>{}</strong></span>"#,
        esc(t("signedInAs", lang)),
        esc(owner_login),
    ));
    match share_token {
        Some(tok) => h.push_str(&copy_button(&share_url(public_origin, plan_slug, tok), lang)),
        None => h.push_str(&format!(
            r#"<span class="copy-share-missing">{}</span>"#,
            esc(t("noShareLink", lang)),
        )),
    }
    h.push_str(&format!(
        r#" <a class="owner-chrome-logout" href="/auth/logout">{}</a>"#,
        esc(t("logout", lang)),
    ));
    h.push_str("</div>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_url_uses_public_origin_and_slug() {
        let u = share_url("https://trip-dashboard-rs.yanggf.workers.dev", "okinawa-2026", "abc123");
        assert_eq!(
            u,
            "https://trip-dashboard-rs.yanggf.workers.dev/?plan=okinawa-2026&token=abc123"
        );
    }

    #[test]
    fn share_url_trims_trailing_slash_on_origin() {
        let u = share_url("https://example.dev/", "tokyo-2026", "tok");
        assert_eq!(u, "https://example.dev/?plan=tokyo-2026&token=tok");
    }

    #[test]
    fn copy_button_escapes_url_for_html_attribute() {
        let html = copy_button("https://x/?plan=a&token=b", "en");
        assert!(html.contains("data-copy-url=\"https://x/?plan=a&amp;token=b\""));
        assert!(html.contains("Copy share link"));
    }

    #[test]
    fn owner_chrome_includes_share_token_url() {
        let html = owner_plan_chrome(
            "okinawa-2026",
            Some("deadbeef"),
            "https://example.dev",
            "yanggf8",
            "zh",
        );
        assert!(html.contains("copy-share-btn"));
        assert!(html.contains("token=deadbeef"));
        assert!(html.contains("複製分享連結"));
        assert!(html.contains("/auth/logout"));
    }

    #[test]
    fn owner_chrome_shows_missing_hint_when_no_token() {
        let html = owner_plan_chrome("okinawa-2026", None, "https://example.dev", "yanggf8", "en");
        assert!(html.contains("No share link yet"));
        assert!(!html.contains("copy-share-btn"));
    }

    #[test]
    fn build_share_maps_auth_resolves_every_token() {
        let rows = vec![
            ("tok-new".into(), "okinawa-2026".into()),
            ("tok-old".into(), "okinawa-2026".into()),
            ("tok-tokyo".into(), "tokyo-2026".into()),
        ];
        let (auth, copy) = build_share_maps(&rows);
        assert_eq!(auth.get("tok-new").map(|s| s.as_str()), Some("okinawa-2026"));
        assert_eq!(auth.get("tok-old").map(|s| s.as_str()), Some("okinawa-2026"));
        assert_eq!(auth.get("tok-tokyo").map(|s| s.as_str()), Some("tokyo-2026"));
        assert_eq!(copy.get("okinawa-2026").map(|s| s.as_str()), Some("tok-new"));
        assert_eq!(copy.get("tokyo-2026").map(|s| s.as_str()), Some("tok-tokyo"));
    }

    #[test]
    fn build_share_maps_hyphenates_underscore_plan_ids() {
        let rows = vec![("abc".into(), "okinawa_2026".into())];
        let (auth, copy) = build_share_maps(&rows);
        assert_eq!(auth.get("abc").map(|s| s.as_str()), Some("okinawa-2026"));
        assert_eq!(copy.get("okinawa-2026").map(|s| s.as_str()), Some("abc"));
    }
}