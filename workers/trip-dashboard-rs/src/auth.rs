//! Access scoping. GitHub OAuth grants owner scope; query tokens are per-plan
//! viewer share tokens only.

use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq)]
pub enum AccessScope {
    Owner,
    Plan(String), // plan slug, e.g. "okinawa-2026"
    Denied,
}

/// Resolve viewer scope. `token` is the value from `?token=`;
/// `share_tokens` maps token -> plan_id (loaded from plan_share_tokens).
pub fn resolve(token: Option<&str>, share_tokens: &HashMap<String, String>) -> AccessScope {
    match token {
        Some(t) => match share_tokens.get(t) {
            Some(plan) => AccessScope::Plan(plan.clone()),
            None => AccessScope::Denied,
        },
        None => AccessScope::Denied,
    }
}

/// Can this scope view the given plan slug?
pub fn can_view_plan(scope: &AccessScope, slug: &str) -> bool {
    match scope {
        AccessScope::Owner => true,
        AccessScope::Plan(p) => p == slug,
        AccessScope::Denied => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shares() -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("share-oki-abc".into(), "okinawa-2026".into());
        m
    }

    #[test]
    fn share_token_scopes_to_one_plan() {
        assert_eq!(
            resolve(Some("share-oki-abc"), &shares()),
            AccessScope::Plan("okinawa-2026".into())
        );
    }
    #[test]
    fn unknown_token_denied() {
        assert_eq!(resolve(Some("nope"), &shares()), AccessScope::Denied);
    }
    #[test]
    fn no_token_denied() {
        assert_eq!(resolve(None, &shares()), AccessScope::Denied);
    }
    #[test]
    fn plan_scope_cannot_view_other_plan() {
        let s = AccessScope::Plan("okinawa-2026".into());
        assert!(can_view_plan(&s, "okinawa-2026"));
        assert!(!can_view_plan(&s, "tokyo-2026"));
    }
    #[test]
    fn owner_views_any() {
        assert!(can_view_plan(&AccessScope::Owner, "anything"));
    }
    #[test]
    fn empty_token_is_denied() {
        let m = std::collections::HashMap::new();
        assert_eq!(resolve(Some(""), &m), AccessScope::Denied);
    }
}
