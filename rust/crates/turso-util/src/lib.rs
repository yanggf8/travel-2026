pub mod config;
pub mod credentials;
pub mod db;
pub mod error;
pub mod migrate;

pub use config::{RegistryConfig, TokenEnvPolicy};
pub use credentials::{CachedCredential, TokenTier};
pub use error::{Error, Result};
pub use migrate::{run_migrations, MigrateOptions, MigrateReport};

#[derive(Default)]
pub struct ResolveOpts {
    pub mint_allowed: bool,
    pub mint_fresh: bool,
    pub expiration: Option<String>,
    pub turso_bin: Option<String>,
}

pub fn resolve_token(
    cfg: &RegistryConfig,
    tier: TokenTier,
    opts: ResolveOpts,
) -> Result<credentials::CachedCredential> {
    if !cfg.supports(tier) {
        return Err(Error::unsupported_tier(tier.as_str(), &cfg.db_name));
    }

    let db = credentials::db_name(cfg);
    if let Some((token, source)) = cfg.token_from_env(tier) {
        let (url, url_source) = cfg.db_url_from_env().ok_or_else(|| {
            Error::auth(format!(
                "database URL is required; checked {}",
                cfg.db_url_envs.join(", ")
            ))
        })?;
        let path = credentials::token_cache_path(cfg, &db, tier)?;
        return Ok(credentials::CachedCredential {
            token,
            url,
            db,
            tier: tier.as_str().to_string(),
            issued_at: "env".to_string(),
            expires_at: "unknown".to_string(),
            expires_at_unix: None,
            path,
            source: format!("env:{source}; url:env:{url_source}"),
        });
    }

    if let Some(credential) = credentials::resolve_cached_or_mint(
        cfg,
        tier,
        opts.mint_allowed,
        opts.mint_fresh,
        opts.expiration.as_deref(),
        opts.turso_bin.as_deref(),
    )? {
        return Ok(credential);
    }

    Err(Error::auth(format!(
        "this command needs one of: {}",
        cfg.token_env_names(tier).join(", ")
    )))
}

pub async fn connect(cfg: &RegistryConfig, tier: TokenTier) -> Result<libsql::Database> {
    let credential = resolve_token(
        cfg,
        tier,
        ResolveOpts {
            mint_allowed: true,
            ..Default::default()
        },
    )?;
    db::database(&credential.url, Some(&credential.token)).await
}
