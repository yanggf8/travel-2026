use std::{
    env, fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Value};

use crate::{
    config::RegistryConfig,
    error::{Error, Result},
};

const DEFAULT_EXPIRATION: &str = "1d";
const SAFETY_WINDOW_SECONDS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenTier {
    Read,
    Write,
    Secrets,
}

impl TokenTier {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenTier::Read => "read",
            TokenTier::Write => "write",
            TokenTier::Secrets => "secrets",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CachedCredential {
    pub token: String,
    pub url: String,
    pub db: String,
    pub tier: String,
    pub issued_at: String,
    pub expires_at: String,
    pub expires_at_unix: Option<u64>,
    pub path: PathBuf,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct CacheProbe {
    pub path: PathBuf,
    pub status: String,
    pub expires_at: Option<String>,
}

pub fn db_name(cfg: &RegistryConfig) -> String {
    for key in &cfg.db_name_envs {
        if let Ok(value) = env::var(key) {
            if !value.is_empty() {
                return value;
            }
        }
    }
    cfg.db_name.clone()
}

pub fn config_home(cfg: &RegistryConfig) -> Result<PathBuf> {
    if let Ok(value) = env::var(&cfg.config_home_env) {
        return Ok(PathBuf::from(value));
    }
    let home = env::var("HOME").map_err(|_| {
        Error::environment(format!(
            "HOME is required to locate {} credential cache",
            cfg.cache_namespace
        ))
    })?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join(&cfg.cache_namespace))
}

pub fn token_cache_path(cfg: &RegistryConfig, db: &str, tier: TokenTier) -> Result<PathBuf> {
    Ok(config_home(cfg)?
        .join("tokens")
        .join(format!("{}-{}.json", db, tier.as_str())))
}

pub fn operator_file(cfg: &RegistryConfig) -> Result<PathBuf> {
    Ok(config_home(cfg)?.join("operator"))
}

pub fn load_operator_file(cfg: &RegistryConfig) -> Result<Option<String>> {
    let path = match operator_file(cfg) {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    if !path.exists() {
        return Ok(None);
    }
    require_safe_file(&path)?;
    let value = fs::read_to_string(&path)
        .map_err(Error::internal)?
        .trim()
        .to_string();
    if !email_shaped(&value) {
        return Err(Error::auth(format!(
            "operator file must contain one email address: {}",
            path.display()
        )));
    }
    Ok(Some(value))
}

pub fn write_operator_file(cfg: &RegistryConfig, email: &str, force: bool) -> Result<PathBuf> {
    let email = email.trim();
    if !email_shaped(email) {
        return Err(Error::auth("operator must be an email address"));
    }
    let path = operator_file(cfg)?;
    if path.exists() {
        let existing = fs::read_to_string(&path)
            .map_err(Error::internal)?
            .trim()
            .to_string();
        if existing != email && !force {
            return Err(Error::conflict(format!(
                "operator file already contains {existing}; rerun with --force to overwrite {}",
                path.display()
            )));
        }
    }
    atomic_write(&path, format!("{email}\n").as_bytes())?;
    Ok(path)
}

pub fn resolve_cached_or_mint(
    cfg: &RegistryConfig,
    tier: TokenTier,
    mint_allowed: bool,
    mint_fresh: bool,
    expiration: Option<&str>,
    turso_bin: Option<&str>,
) -> Result<Option<CachedCredential>> {
    let db = db_name(cfg);
    let path = token_cache_path(cfg, &db, tier)?;
    let tier_name = tier.as_str();

    if !mint_fresh {
        if let Some(cached) = read_cache(&path, &db, tier_name)? {
            if usable(&cached) {
                return Ok(Some(cached));
            }
        }
    }

    if !mint_allowed {
        return Ok(None);
    }

    let expiration = expiration.unwrap_or(DEFAULT_EXPIRATION);
    let turso_bin = turso_bin.unwrap_or("turso");
    let issued = now_unix();
    let url = run_turso(turso_bin, &["db", "show", &db, "--url"])?;
    if !url.starts_with("libsql://") {
        return Err(Error::environment(format!(
            "turso returned unexpected URL for db '{db}'"
        )));
    }
    let token = run_turso(
        turso_bin,
        &[
            "db",
            "tokens",
            "create",
            &db,
            "-e",
            &turso_expiration(expiration)?,
        ],
    )?;
    let (expires_at, expires_at_unix) = expires_at(issued, expiration);
    let credential = CachedCredential {
        token,
        url,
        db,
        tier: tier_name.to_string(),
        issued_at: iso_from_unix(issued),
        expires_at,
        expires_at_unix,
        path,
        source: "minted".to_string(),
    };
    write_cache(&credential)?;
    Ok(Some(credential))
}

pub fn refresh(
    cfg: &RegistryConfig,
    tier: TokenTier,
    expiration: Option<&str>,
    turso_bin: Option<&str>,
) -> Result<CachedCredential> {
    resolve_cached_or_mint(cfg, tier, true, true, expiration, turso_bin)?
        .ok_or_else(|| Error::environment("failed to refresh token cache"))
}

pub fn cache_probe(cfg: &RegistryConfig, tier: TokenTier) -> CacheProbe {
    let db = db_name(cfg);
    let path = token_cache_path(cfg, &db, tier).unwrap_or_else(|_| PathBuf::from("<unavailable>"));
    if !path.exists() {
        return CacheProbe {
            path,
            status: "missing".to_string(),
            expires_at: None,
        };
    }
    match read_cache(&path, &db, tier.as_str()) {
        Ok(Some(credential)) => CacheProbe {
            path,
            status: if usable(&credential) {
                "usable"
            } else {
                "expired"
            }
            .to_string(),
            expires_at: Some(credential.expires_at),
        },
        Ok(None) => CacheProbe {
            path,
            status: "corrupt".to_string(),
            expires_at: None,
        },
        Err(err) => CacheProbe {
            path,
            status: err.message().to_string(),
            expires_at: None,
        },
    }
}

fn read_cache(path: &Path, db: &str, tier: &str) -> Result<Option<CachedCredential>> {
    if !path.exists() {
        return Ok(None);
    }
    require_safe_file(path)?;
    let raw = fs::read_to_string(path).map_err(Error::internal)?;
    let value: Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let Some(token) = value.get("token").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(url) = value.get("url").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(cached_db) = value.get("db").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(cached_tier) = value.get("tier").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(issued_at) = value.get("issued_at").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(expires_at) = value.get("expires_at").and_then(Value::as_str) else {
        return Ok(None);
    };
    if token.is_empty() || url.is_empty() || cached_db != db || cached_tier != tier {
        return Ok(None);
    }
    Ok(Some(CachedCredential {
        token: token.to_string(),
        url: url.to_string(),
        db: cached_db.to_string(),
        tier: cached_tier.to_string(),
        issued_at: issued_at.to_string(),
        expires_at: expires_at.to_string(),
        expires_at_unix: value.get("expires_at_unix").and_then(Value::as_u64),
        path: path.to_path_buf(),
        source: "cache".to_string(),
    }))
}

fn write_cache(credential: &CachedCredential) -> Result<()> {
    let payload = json!({
        "token": credential.token,
        "url": credential.url,
        "db": credential.db,
        "tier": credential.tier,
        "issued_at": credential.issued_at,
        "expires_at": credential.expires_at,
        "expires_at_unix": credential.expires_at_unix,
    });
    atomic_write(
        &credential.path,
        format!(
            "{}\n",
            serde_json::to_string(&payload).map_err(Error::internal)?
        )
        .as_bytes(),
    )
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            Error::environment(format!(
                "cannot create credential cache at {}; fix permissions or export TURSO_<TIER>_TOKEN: {err}",
                parent.display()
            ))
        })?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).ok();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("cache"),
        std::process::id()
    ));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(Error::internal)?;
    file.write_all(content).map_err(Error::internal)?;
    file.sync_all().map_err(Error::internal)?;
    drop(file);
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).map_err(Error::internal)?;
    fs::rename(&tmp, path).map_err(Error::internal)?;
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

fn require_safe_file(path: &Path) -> Result<()> {
    let mode = fs::metadata(path)
        .map_err(Error::internal)?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        return Err(Error::auth(format!(
            "refusing token cache with unsafe permissions: {}; chmod 600 or mint fresh",
            path.display()
        )));
    }
    Ok(())
}

fn usable(credential: &CachedCredential) -> bool {
    match credential.expires_at_unix {
        Some(value) => value.saturating_sub(now_unix()) > SAFETY_WINDOW_SECONDS,
        None => true,
    }
}

fn run_turso(bin: &str, args: &[&str]) -> Result<String> {
    let output = ProcessCommand::new(bin)
        .args(args)
        .output()
        .map_err(|err| {
            Error::environment(format!(
                "cannot run Turso CLI '{bin}'; install turso or set TURSO_<TIER>_TOKEN: {err}"
            ))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let message = if args.contains(&"auth") {
            "turso auth not available; run: turso auth login".to_string()
        } else if stderr.is_empty() {
            "cannot refresh token; network unavailable or Turso unreachable".to_string()
        } else {
            format!("turso command failed: {stderr}")
        };
        return Err(Error::environment(message));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err(Error::environment(format!(
            "turso command returned empty output: turso {}",
            args.join(" ")
        )));
    }
    if stdout.to_ascii_lowercase().contains("not logged in") {
        return Err(Error::environment(
            "turso auth not available; run: turso auth login",
        ));
    }
    Ok(stdout)
}

fn turso_expiration(expiration: &str) -> Result<String> {
    if expiration == "never" || expiration.ends_with('d') {
        return Ok(expiration.to_string());
    }
    let Some(hours) = expiration.strip_suffix('h') else {
        return Ok(expiration.to_string());
    };
    let hours = hours.parse::<u64>().map_err(|_| {
        Error::validation(format!(
            "invalid expiration '{expiration}'; expected Nh, Nd, or never"
        ))
    })?;
    if hours == 0 {
        return Err(Error::validation(
            "invalid expiration '0h'; expected a positive duration",
        ));
    }
    Ok(format!("{}d", hours.div_ceil(24)))
}

fn expires_at(issued: u64, expiration: &str) -> (String, Option<u64>) {
    if expiration == "never" {
        return ("never".to_string(), None);
    }
    let seconds = if let Some(days) = expiration.strip_suffix('d') {
        days.parse::<u64>().unwrap_or(1) * 86_400
    } else if let Some(hours) = expiration.strip_suffix('h') {
        hours.parse::<u64>().unwrap_or(24) * 3_600
    } else {
        86_400
    };
    let value = issued + seconds;
    (iso_from_unix(value), Some(value))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

fn iso_from_unix(value: u64) -> String {
    let days = (value / 86_400) as i64;
    let seconds = value % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

fn email_shaped(value: &str) -> bool {
    value
        .split_once('@')
        .map(|(_, domain)| domain.contains('.'))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, fs, os::unix::fs::PermissionsExt, sync::Mutex};
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn formats_unix_timestamp_as_utc() {
        assert_eq!(iso_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_unix(1_777_909_973), "2026-05-04T15:52:53Z");
    }

    #[test]
    fn reads_usable_cache_without_minting() {
        let _g = LOCK.lock().unwrap();
        let dir = env::temp_dir().join(format!("turso-util-cache-{}", std::process::id()));
        let tokens = dir.join("tokens");
        fs::create_dir_all(&tokens).unwrap();
        let path = tokens.join("finance-registry-read.json");
        fs::write(&path, r#"{"token":"t","url":"libsql://x","db":"finance-registry","tier":"read","issued_at":"1","expires_at":"9999999999","expires_at_unix":9999999999}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let cached = read_cache(&path, "finance-registry", "read")
            .unwrap()
            .unwrap();
        assert_eq!(cached.token, "t");
        assert!(usable(&cached));
        let _ = fs::remove_dir_all(dir);
    }
}
