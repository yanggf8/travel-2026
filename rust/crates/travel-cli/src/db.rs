use crate::leave::{HolidayCalendar, HolidayEntry, MakeupWorkday};
use libsql::params;
use std::{collections::HashMap, env, path::PathBuf};
use turso_util::{RegistryConfig, ResolveOpts, TokenEnvPolicy, TokenTier, resolve_token};

pub async fn load_holiday_calendar(
    country_or_market: &str,
    year: i32,
) -> Result<HolidayCalendar, String> {
    let country = normalize_country(country_or_market);
    let conn = connect_read().await?;
    let start = format!("{year}-01-01");
    let end = format!("{year}-12-31");
    let mut rows = conn
        .query(
            "SELECT date, name, is_holiday
             FROM holidays
             WHERE country = ?1 AND date >= ?2 AND date <= ?3
             ORDER BY date",
            params![country.clone(), start, end],
        )
        .await
        .map_err(|err| format!("failed to query holidays from Turso: {err}"))?;

    let mut holidays = HashMap::new();
    let mut makeup_workdays = HashMap::new();
    let mut count = 0usize;

    while let Some(row) = rows
        .next()
        .await
        .map_err(|err| format!("failed to read holiday row: {err}"))?
    {
        count += 1;
        let date: String = row.get(0).map_err(|err| format!("holiday date: {err}"))?;
        let name: String = row.get(1).unwrap_or_default();
        let flag: i64 = row.get(2).unwrap_or_default();
        let month_day = date
            .get(5..)
            .ok_or_else(|| format!("invalid holiday date from Turso: {date}"))?
            .to_string();
        if flag == 2 && !name.is_empty() {
            holidays.insert(
                month_day,
                HolidayEntry {
                    name,
                    name_en: String::new(),
                    kind: "national".to_string(),
                },
            );
        } else if flag == 1 {
            makeup_workdays.insert(
                month_day,
                MakeupWorkday {
                    name: if name.is_empty() {
                        "補班日".to_string()
                    } else {
                        name
                    },
                    name_en: "Makeup workday".to_string(),
                    for_holiday: String::new(),
                },
            );
        }
    }

    if count == 0 {
        return Err(format!(
            "Missing Turso data for holidays country={country} year={year}. Run the holiday fetch/seed path before using leave-aware commands."
        ));
    }

    Ok(HolidayCalendar {
        country,
        year,
        description: format!("Turso holidays for {country_or_market} {year}"),
        holidays,
        makeup_workdays,
    })
}

pub(crate) async fn connect_read() -> Result<libsql::Connection, String> {
    let credential = resolve_travel_token(TokenTier::Read)?;
    let db = libsql::Builder::new_remote(credential.url, credential.token)
        .build()
        .await
        .map_err(|err| format!("failed to connect to Turso: {err}"))?;
    db.connect()
        .map_err(|err| format!("failed to open Turso connection: {err}"))
}

fn normalize_country(country_or_market: &str) -> String {
    match country_or_market.to_ascii_lowercase().as_str() {
        "tw" => "taiwan".to_string(),
        "jp" => "japan".to_string(),
        other => other.to_string(),
    }
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
                    mint_err.formatted("travel"),
                    tier.as_str().to_ascii_uppercase()
                )
            })
        }
        Err(err) => Err(format!(
            "{}; run `turso auth login` or set TRAVEL_TURSO_{}_TOKEN plus TRAVEL_TURSO_URL",
            err.formatted("travel"),
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
