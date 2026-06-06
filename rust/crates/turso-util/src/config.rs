use crate::credentials::TokenTier;

#[derive(Debug, Clone)]
pub struct TokenEnvPolicy {
    pub read: Vec<String>,
    pub write: Vec<String>,
    pub secrets: Vec<String>,
    pub allow_generic_fallback: bool,
}

#[derive(Debug, Clone)]
pub struct RegistryConfig {
    pub db_name: String,
    pub db_name_envs: Vec<String>,
    pub db_url_envs: Vec<String>,
    pub operator_env: String,
    pub config_home_env: String,
    pub cache_namespace: String,
    pub token_envs: TokenEnvPolicy,
    pub supported_tiers: Vec<TokenTier>,
}

impl RegistryConfig {
    pub fn supports(&self, tier: TokenTier) -> bool {
        self.supported_tiers.contains(&tier)
    }

    /// The ordered list of env var names consulted for a tier's token.
    /// Project-specific names first; generic TURSO_*_TOKEN appended only
    /// when allow_generic_fallback is true.
    pub fn token_env_names(&self, tier: TokenTier) -> Vec<String> {
        let (mut names, generic) = match tier {
            TokenTier::Read => (self.token_envs.read.clone(), "TURSO_READ_TOKEN"),
            TokenTier::Write => (self.token_envs.write.clone(), "TURSO_WRITE_TOKEN"),
            TokenTier::Secrets => (self.token_envs.secrets.clone(), "TURSO_SECRETS_TOKEN"),
        };
        if self.token_envs.allow_generic_fallback {
            names.push(generic.to_string());
        }
        names
    }

    pub fn db_url_from_env(&self) -> Option<(String, String)> {
        for key in &self.db_url_envs {
            if let Ok(v) = std::env::var(key) {
                if !v.is_empty() {
                    return Some((v, key.clone()));
                }
            }
        }
        None
    }

    pub fn token_from_env(&self, tier: TokenTier) -> Option<(String, String)> {
        for key in self.token_env_names(tier) {
            if let Ok(v) = std::env::var(&key) {
                if !v.is_empty() {
                    return Some((v, key));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    static LOCK: Mutex<()> = Mutex::new(());

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
    fn finance_does_not_use_generic_token_vars() {
        let _g = LOCK.lock().unwrap();
        std::env::set_var("TURSO_READ_TOKEN", "persona-secret");
        std::env::remove_var("FINANCE_TURSO_READ_TOKEN");
        let cfg = finance();
        let names = cfg.token_env_names(TokenTier::Read);
        assert_eq!(names, vec!["FINANCE_TURSO_READ_TOKEN".to_string()]);
        std::env::remove_var("TURSO_READ_TOKEN");
    }

    #[test]
    fn generic_fallback_appends_tier_generic_when_enabled() {
        let mut cfg = finance();
        cfg.token_envs.allow_generic_fallback = true;
        let names = cfg.token_env_names(TokenTier::Read);
        assert_eq!(
            names,
            vec![
                "FINANCE_TURSO_READ_TOKEN".to_string(),
                "TURSO_READ_TOKEN".to_string()
            ]
        );
    }
}
