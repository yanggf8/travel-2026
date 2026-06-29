use libsql::{Connection, Value, params};
use std::{env, path::PathBuf};
use turso_util::{RegistryConfig, ResolveOpts, TokenEnvPolicy, TokenTier, resolve_token};

pub struct TravelDb {
    conn: Connection,
}

impl TravelDb {
    pub async fn connect_read() -> Result<Self, String> {
        Self::connect_with_tier(TokenTier::Read).await
    }

    pub async fn connect_write() -> Result<Self, String> {
        Self::connect_with_tier(TokenTier::Write).await
    }

    async fn connect_with_tier(tier: TokenTier) -> Result<Self, String> {
        let credential = resolve_travel_token(tier)?;
        let db = libsql::Builder::new_remote(credential.url, credential.token)
            .build()
            .await
            .map_err(|err| format!("failed to connect to Turso: {err}"))?;
        let conn = db
            .connect()
            .map_err(|err| format!("failed to open Turso connection: {err}"))?;
        Ok(Self { conn })
    }

    pub fn token_status(tier: TokenTier) -> Result<TokenStatus, String> {
        let credential = resolve_travel_token(tier)?;
        Ok(TokenStatus {
            db: credential.db,
            tier: credential.tier,
            source: credential.source,
            issued_at: credential.issued_at,
            expires_at: credential.expires_at,
            cache_path: credential.path.display().to_string(),
        })
    }

    pub async fn exec(&self, sql: &str) -> Result<u64, String> {
        self.conn
            .execute(sql, ())
            .await
            .map_err(|err| format!("Turso exec failed: {err}"))
    }

    /// Query returning column names + rows of plain string cells, for table
    /// rendering in the CLI (no JSON output).
    pub async fn query_table(&self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
        let mut rows = self
            .conn
            .query(sql, ())
            .await
            .map_err(|err| format!("Turso query failed: {err}"))?;
        let mut cols: Vec<String> = Vec::new();
        let mut out: Vec<Vec<String>> = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|err| format!("failed to read Turso row: {err}"))?
        {
            if cols.is_empty() {
                for idx in 0..row.column_count() {
                    cols.push(
                        row.column_name(idx)
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("col_{idx}")),
                    );
                }
            }
            let mut cells = Vec::with_capacity(cols.len());
            for idx in 0..row.column_count() {
                let value = row
                    .get_value(idx)
                    .map_err(|err| format!("failed to read column {idx}: {err}"))?;
                cells.push(libsql_value_to_plain(value));
            }
            out.push(cells);
        }
        Ok((cols, out))
    }

    /// Ensure the captures table exists (plain-text raw_text, no JSON files).
    pub async fn ensure_captures_table(&self) -> Result<(), String> {
        self.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS captures (
                    capture_id TEXT PRIMARY KEY,
                    source_id TEXT NOT NULL,
                    url TEXT,
                    title TEXT,
                    captured_at TEXT NOT NULL,
                    raw_text TEXT NOT NULL
                 )",
                (),
            )
            .await
            .map_err(|err| format!("failed to create captures table: {err}"))?;
        Ok(())
    }

    /// Store a capture as a row (raw_text is plain text — never a JSON file).
    pub async fn insert_capture(
        &self,
        capture_id: &str,
        source_id: &str,
        url: &str,
        title: &str,
        captured_at: &str,
        raw_text: &str,
    ) -> Result<(), String> {
        self.ensure_captures_table().await?;
        self.conn
            .execute(
                "INSERT INTO captures (capture_id, source_id, url, title, captured_at, raw_text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(capture_id) DO UPDATE SET
                    source_id=excluded.source_id, url=excluded.url, title=excluded.title,
                    captured_at=excluded.captured_at, raw_text=excluded.raw_text",
                params![
                    capture_id.to_string(),
                    source_id.to_string(),
                    url.to_string(),
                    title.to_string(),
                    captured_at.to_string(),
                    raw_text.to_string(),
                ],
            )
            .await
            .map_err(|err| format!("failed to insert capture: {err}"))?;
        Ok(())
    }
}

pub struct TokenStatus {
    pub db: String,
    pub tier: String,
    pub source: String,
    pub issued_at: String,
    pub expires_at: String,
    pub cache_path: String,
}

fn resolve_travel_token(tier: TokenTier) -> Result<turso_util::CachedCredential, String> {
    let cfg = travel_registry_config();
    let opts = || ResolveOpts {
        mint_allowed: true,
        turso_bin: Some(turso_bin()),
        ..Default::default()
    };
    match resolve_token(&cfg, tier, opts()) {
        Ok(credential) => Ok(credential),
        Err(err) if err.message().contains("database URL is required") => {
            let mut mint_cfg = cfg.clone();
            mint_cfg.token_envs.read.clear();
            mint_cfg.token_envs.write.clear();
            mint_cfg.token_envs.secrets.clear();
            mint_cfg.token_envs.allow_generic_fallback = false;
            resolve_token(&mint_cfg, tier, opts()).map_err(|mint_err| {
                format!(
                    "{}; run `turso auth login` or set TRAVEL_TURSO_{}_TOKEN plus TRAVEL_TURSO_URL",
                    mint_err.formatted("chromeport"),
                    tier.as_str().to_ascii_uppercase()
                )
            })
        }
        Err(err) => Err(format!(
            "{}; run `turso auth login` or set TRAVEL_TURSO_{}_TOKEN plus TRAVEL_TURSO_URL",
            err.formatted("chromeport"),
            tier.as_str().to_ascii_uppercase()
        )),
    }
}

fn travel_registry_config() -> RegistryConfig {
    RegistryConfig {
        db_name: "travel-2026".to_string(),
        db_name_envs: vec!["TRAVEL_TURSO_DB".to_string()],
        db_url_envs: vec!["TRAVEL_TURSO_URL".to_string(), "TURSO_URL".to_string()],
        operator_env: "TRAVEL_TURSO_OPERATOR".to_string(),
        config_home_env: "TRAVEL_TURSO_CONFIG_HOME".to_string(),
        cache_namespace: "travel-2026".to_string(),
        token_envs: TokenEnvPolicy {
            read: vec!["TRAVEL_TURSO_READ_TOKEN".to_string()],
            write: vec!["TRAVEL_TURSO_WRITE_TOKEN".to_string()],
            secrets: vec!["TRAVEL_TURSO_SECRETS_TOKEN".to_string()],
            allow_generic_fallback: false,
        },
        supported_tiers: vec![TokenTier::Read, TokenTier::Write, TokenTier::Secrets],
    }
}

fn turso_bin() -> String {
    if let Ok(value) = env::var("TRAVEL_TURSO_BIN") {
        if !value.trim().is_empty() {
            return value;
        }
    }
    if let Ok(home) = env::var("HOME") {
        let candidate = PathBuf::from(home).join(".turso").join("turso");
        if candidate.exists() {
            return candidate.display().to_string();
        }
    }
    "turso".to_string()
}

/// Render a libsql cell as a plain string for table output (no JSON).
fn libsql_value_to_plain(value: Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Integer(value) => value.to_string(),
        Value::Real(value) => value.to_string(),
        Value::Text(value) => value,
        Value::Blob(value) => format!("<blob:{} bytes>", value.len()),
    }
}
