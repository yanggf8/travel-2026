//! Logged-in owner manages viewer grant tokens for others. Recipients open the
//! copied link with no login — just `?plan=` + per-plan grant token.

use std::collections::HashMap;

use super::esc;
use crate::i18n::t;
use crate::turso::Row;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantStatus {
    Active,
    Inactive,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantToken {
    pub token: String,
    pub plan_slug: String,
    pub status: GrantStatus,
    pub created_at: String,
    pub created_by: Option<String>,
    pub deactivated_at: Option<String>,
    pub deactivated_by: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GrantMaps {
    /// Active token -> hyphenated plan slug. Used for viewer auth.
    pub token_to_plan: HashMap<String, String>,
    /// Hyphenated plan slug -> newest active grant token.
    pub plan_to_current: HashMap<String, GrantToken>,
    /// Hyphenated plan slug -> all tokens, newest first.
    pub plan_to_history: HashMap<String, Vec<GrantToken>>,
}

/// Build grant maps from rows ordered newest first.
pub fn build_grant_maps(rows: &[Row]) -> GrantMaps {
    let mut maps = GrantMaps::default();
    for r in rows {
        let token = rs(r, "token");
        let plan_id = rs(r, "plan_id");
        if token.is_empty() || plan_id.is_empty() {
            continue;
        }
        let slug = plan_id.replace('_', "-");
        let status = match rs(r, "status").as_str() {
            "" | "active" => GrantStatus::Active,
            "inactive" => GrantStatus::Inactive,
            _ => continue,
        };
        let grant = GrantToken {
            token: token.clone(),
            plan_slug: slug.clone(),
            status: status.clone(),
            created_at: rs(r, "created_at"),
            created_by: opt(r, "created_by"),
            deactivated_at: opt(r, "deactivated_at"),
            deactivated_by: opt(r, "deactivated_by"),
        };
        if status == GrantStatus::Active {
            maps.token_to_plan.insert(token, slug.clone());
            maps.plan_to_current
                .entry(slug.clone())
                .or_insert_with(|| grant.clone());
        }
        maps.plan_to_history.entry(slug).or_default().push(grant);
    }
    maps
}

fn rs(row: &Row, k: &str) -> String {
    row.get(k)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn opt(row: &Row, k: &str) -> Option<String> {
    let s = rs(row, k);
    if s.is_empty() { None } else { Some(s) }
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

pub fn create_grant_form(plan_slug: &str, csrf: &str, lang: &str) -> String {
    format!(
        r#"<form class="grant-form" method="post" action="/grants/create"><input type="hidden" name="plan" value="{}"><input type="hidden" name="csrf" value="{}"><button type="submit" class="grant-create-btn">{}</button></form>"#,
        esc(plan_slug),
        esc(csrf),
        esc(t("createGrantToken", lang)),
    )
}

/// Owner chrome when logged in: signed-in label + copy (or missing hint) + logout.
pub fn owner_plan_chrome(
    plan_slug: &str,
    grant_token: Option<&GrantToken>,
    history: &[GrantToken],
    public_origin: &str,
    owner_login: &str,
    create_csrf: &str,
    batch_csrf: &str,
    lang: &str,
) -> String {
    let mut h = String::from(r#"<div class="owner-chrome">"#);
    h.push_str(&format!(
        r#"<span class="owner-chrome-user">{} <strong>{}</strong></span>"#,
        esc(t("signedInAs", lang)),
        esc(owner_login),
    ));
    match grant_token {
        Some(grant) => h.push_str(&copy_button(
            &share_url(public_origin, plan_slug, &grant.token),
            lang,
        )),
        None => h.push_str(&create_grant_form(plan_slug, create_csrf, lang)),
    }
    h.push_str(&format!(
        r#" <a class="owner-chrome-logout" href="/auth/logout">{}</a>"#,
        esc(t("logout", lang)),
    ));
    h.push_str("</div>");
    h.push_str(&grant_manager(plan_slug, history, public_origin, batch_csrf, lang));
    h
}

/// Share-link management panel (owner only): active links with checkboxes for
/// batch deactivation, deactivated links folded into a nested <details> —
/// folded by default, same judgment as the 其他海景參考 section. Pure HTML
/// form (checkbox + submit); no client JS beyond the existing copy button.
fn grant_manager(
    plan_slug: &str,
    history: &[GrantToken],
    public_origin: &str,
    batch_csrf: &str,
    lang: &str,
) -> String {
    let actives: Vec<&GrantToken> = history
        .iter()
        .filter(|g| g.status == GrantStatus::Active)
        .collect();
    let inactives: Vec<&GrantToken> = history
        .iter()
        .filter(|g| g.status == GrantStatus::Inactive)
        .collect();
    let (title, none_txt, active_txt, deact_txt, deact_on, btn) = if lang == "en" {
        (
            "Share links",
            "No active share links.",
            "active",
            "deactivated",
            "deactivated at",
            "Deactivate selected",
        )
    } else {
        (
            "分享連結管理",
            "目前沒有現用連結。",
            "現用",
            "已停用",
            "停用於",
            "停用所選",
        )
    };
    let mut h = format!(
        "<details class=\"grant-manager\"><summary>{} · {} {}</summary>",
        esc(title),
        actives.len(),
        active_txt
    );
    if actives.is_empty() {
        h.push_str(&format!("<p class=\"grant-none\">{}</p>", esc(none_txt)));
    } else {
        h.push_str(&format!(
            "<form method=\"post\" action=\"/grants/deactivate-batch\">\
             <input type=\"hidden\" name=\"plan\" value=\"{}\">\
             <input type=\"hidden\" name=\"csrf\" value=\"{}\">",
            esc(plan_slug),
            esc(batch_csrf)
        ));
        for g in &actives {
            h.push_str(&format!(
                "<label class=\"grant-row\"><input type=\"checkbox\" name=\"token\" value=\"{}\"> \
                 <code>{}</code> <span class=\"grant-created\">{}</span> {}</label>",
                esc(&g.token),
                esc(&token_fingerprint(&g.token)),
                esc(&g.created_at),
                copy_button(&share_url(public_origin, plan_slug, &g.token), lang)
            ));
        }
        h.push_str(&format!(
            "<button type=\"submit\" class=\"grant-deactivate-btn\">{}</button></form>",
            esc(btn)
        ));
    }
    if !inactives.is_empty() {
        h.push_str(&format!(
            "<details class=\"grant-inactive\"><summary>{} ({})</summary>",
            deact_txt,
            inactives.len()
        ));
        for g in &inactives {
            h.push_str(&format!(
                "<div class=\"grant-row grant-row--inactive\"><code>{}</code>                  <span class=\"grant-created\">{} · {} {}</span></div>",
                esc(&token_fingerprint(&g.token)),
                esc(&g.created_at),
                deact_on,
                esc(g.deactivated_at.as_deref().unwrap_or(""))
            ));
        }
        h.push_str("</details>");
    }
    h.push_str("</details>");
    h
}

pub fn token_fingerprint(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 12 {
        return token.to_string();
    }
    let head: String = chars.iter().take(6).collect();
    let tail: String = chars[chars.len() - 6..].iter().collect();
    format!("{head}...{tail}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn grant_manager_folds_inactive_and_checks_actives() {
        use super::{grant_manager, GrantStatus, GrantToken};
        let history = vec![
            GrantToken {
                token: "0123456789abcdef0123456789abcdef".into(),
                plan_slug: "jiufen-2026".into(),
                status: GrantStatus::Active,
                created_at: "2026-10-07".into(),
                created_by: None,
                deactivated_at: None,
                deactivated_by: None,
            },
            GrantToken {
                token: "fedcba9876543210fedcba9876543210".into(),
                plan_slug: "jiufen-2026".into(),
                status: GrantStatus::Inactive,
                created_at: "2026-10-01".into(),
                created_by: None,
                deactivated_at: Some("2026-10-05".into()),
                deactivated_by: None,
            },
        ];
        let html = grant_manager("jiufen-2026", &history, "https://x", "csrf-val", "zh");
        // Folded by default: <details> without open.
        assert!(html.contains("<details class=\"grant-manager\">"), "{html}");
        assert!(!html.contains("<details class=\"grant-manager\" open"), "{html}");
        // Active row: checkbox for batch deactivate + copy affordance.
        assert!(html.contains("type=\"checkbox\" name=\"token\""), "{html}");
        assert!(html.contains("012345...abcdef"), "{html}");
        // Inactive block folds INSIDE, labelled, not checkboxed.
        assert!(html.contains("grant-inactive"), "{html}");
        assert!(html.contains("停用 (1)"), "{html}");
        assert!(html.contains("fedcba...543210"), "{html}");
        // Batch form wiring: action + csrf hidden field + submit.
        assert!(html.contains("/grants/deactivate-batch"), "{html}");
        assert!(html.contains("csrf-val"), "{html}");
        assert!(html.contains("停用所選"), "{html}");
    }

    use super::*;
    use crate::turso::Row;

    #[test]
    fn share_url_uses_public_origin_and_slug() {
        let u = share_url(
            "https://trip-dashboard-rs.yanggf.workers.dev",
            "okinawa-2026",
            "abc123",
        );
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
        let grant = GrantToken {
            token: "deadbeef".into(),
            plan_slug: "okinawa-2026".into(),
            status: GrantStatus::Active,
            created_at: "2026-06-25".into(),
            created_by: None,
            deactivated_at: None,
            deactivated_by: None,
        };
        let html = owner_plan_chrome(
            "okinawa-2026",
            Some(&grant),
            &[],
            "https://example.dev",
            "yanggf8",
            "csrf",
            "batch-csrf",
            "zh",
        );
        assert!(html.contains("copy-share-btn"));
        assert!(html.contains("token=deadbeef"));
        assert!(html.contains("複製分享連結"));
        assert!(html.contains("/auth/logout"));
    }

    #[test]
    fn owner_chrome_shows_missing_hint_when_no_token() {
        let html = owner_plan_chrome(
            "okinawa-2026",
            None,
            &[],
            "https://example.dev",
            "yanggf8",
            "csrf",
            "batch-csrf",
            "en",
        );
        assert!(html.contains("Create grant token"));
        assert!(html.contains("/grants/create"));
        assert!(!html.contains("copy-share-btn"));
    }

    #[test]
    fn build_grant_maps_auth_resolves_active_tokens_only() {
        let mut a = Row::new();
        a.insert("token".into(), serde_json::json!("tok-new"));
        a.insert("plan_id".into(), serde_json::json!("okinawa-2026"));
        a.insert("status".into(), serde_json::json!("active"));
        let mut b = Row::new();
        b.insert("token".into(), serde_json::json!("tok-old"));
        b.insert("plan_id".into(), serde_json::json!("okinawa-2026"));
        b.insert("status".into(), serde_json::json!("inactive"));
        let maps = build_grant_maps(&[a, b]);
        assert_eq!(
            maps.token_to_plan.get("tok-new").map(|s| s.as_str()),
            Some("okinawa-2026")
        );
        assert!(maps.token_to_plan.get("tok-old").is_none());
        assert_eq!(maps.plan_to_current["okinawa-2026"].token, "tok-new");
        assert_eq!(maps.plan_to_history["okinawa-2026"].len(), 2);
    }

    #[test]
    fn build_grant_maps_hyphenates_underscore_plan_ids() {
        let mut row = Row::new();
        row.insert("token".into(), serde_json::json!("abc"));
        row.insert("plan_id".into(), serde_json::json!("okinawa_2026"));
        row.insert("status".into(), serde_json::json!("active"));
        let maps = build_grant_maps(&[row]);
        assert_eq!(
            maps.token_to_plan.get("abc").map(|s| s.as_str()),
            Some("okinawa-2026")
        );
        assert_eq!(maps.plan_to_current["okinawa-2026"].token, "abc");
    }

    #[test]
    fn token_fingerprint_is_unicode_safe() {
        assert_eq!(token_fingerprint("abcdef0123456789"), "abcdef...456789");
        assert_eq!(
            token_fingerprint("短短短短短短短短短短短短短"),
            "短短短短短短...短短短短短短"
        );
    }
}
