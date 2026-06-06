use turso_util::{RegistryConfig, TokenEnvPolicy, TokenTier};

fn finance() -> RegistryConfig {
    RegistryConfig {
        db_name: "finance-registry".into(),
        db_name_envs: vec!["FINANCE_TURSO_DB".into()],
        db_url_envs: vec!["FINANCE_TURSO_URL".into()],
        operator_env: "FINANCE_OPERATOR".into(),
        config_home_env: "GWEBCDB_CONFIG_HOME".into(),
        cache_namespace: "gwebcdb".into(),
        token_envs: TokenEnvPolicy {
            read: vec!["FINANCE_TURSO_READ_TOKEN".into()],
            write: vec!["FINANCE_TURSO_WRITE_TOKEN".into()],
            secrets: vec![],
            allow_generic_fallback: false,
        },
        supported_tiers: vec![TokenTier::Read, TokenTier::Write],
    }
}

#[test]
fn finance_secrets_tier_is_unsupported_before_any_lookup() {
    let cfg = finance();
    let err = turso_util::resolve_token(&cfg, TokenTier::Secrets, Default::default()).unwrap_err();
    assert_eq!(err.kind_str(), "unsupported-tier");
}

#[test]
fn finance_missing_token_names_finance_var_not_generic() {
    let dir = std::env::temp_dir().join(format!("tu-tier-policy-{}", std::process::id()));
    std::env::set_var("GWEBCDB_CONFIG_HOME", &dir);
    std::env::remove_var("FINANCE_TURSO_READ_TOKEN");
    std::env::set_var("TURSO_READ_TOKEN", "persona-secret");
    let cfg = finance();
    let err = turso_util::resolve_token(
        &cfg,
        TokenTier::Read,
        turso_util::ResolveOpts {
            mint_allowed: false,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(err.message().contains("FINANCE_TURSO_READ_TOKEN"));
    assert!(!err.message().contains(", TURSO_READ_TOKEN"));
    std::env::remove_var("TURSO_READ_TOKEN");
    std::env::remove_var("GWEBCDB_CONFIG_HOME");
    let _ = std::fs::remove_dir_all(dir);
}
