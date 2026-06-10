//! Access scoping. A request carries an optional token (query param `token` or
//! the owner secret). Owner sees everything; a per-plan token sees exactly one plan.

use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq)]
pub enum AccessScope {
    Owner,
    Plan(String), // plan slug, e.g. "okinawa-2026"
    Denied,
}

/// Resolve scope. `token` is the value from `?token=`; `owner_token` is the secret;
/// `share_tokens` maps token -> plan_id (loaded from plan_share_tokens).
pub fn resolve(token: Option<&str>, owner_token: &str, share_tokens: &HashMap<String, String>) -> AccessScope {
    match token {
        Some(t) if !owner_token.is_empty() && t == owner_token => AccessScope::Owner,
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
    fn owner_token_is_owner() {
        assert_eq!(resolve(Some("OWNER"), "OWNER", &shares()), AccessScope::Owner);
    }
    #[test]
    fn share_token_scopes_to_one_plan() {
        assert_eq!(resolve(Some("share-oki-abc"), "OWNER", &shares()), AccessScope::Plan("okinawa-2026".into()));
    }
    #[test]
    fn unknown_token_denied() {
        assert_eq!(resolve(Some("nope"), "OWNER", &shares()), AccessScope::Denied);
    }
    #[test]
    fn no_token_denied() {
        assert_eq!(resolve(None, "OWNER", &shares()), AccessScope::Denied);
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
}
