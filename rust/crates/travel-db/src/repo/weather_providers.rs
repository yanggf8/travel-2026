//! Weather data providers (`weather_providers`): GLOBAL reference data naming the
//! source of stored forecasts and the licence it requires us to credit. Keyed by
//! `provider_id` (the value stored in `days.weather_source_id` /
//! `day_weather_sessions.source`). NO audit triad — same family as `clothing_tips`.
//! The dashboard worker renders the attribution line from these rows.

use libsql::Connection;

#[derive(Debug, Clone, PartialEq)]
pub struct WeatherProvider {
    pub provider_id: String,
    pub name_zh: String,
    pub name_en: String,
    pub attribution_url: String,
    pub license: String,
}

/// Insert or replace one provider (keyed by `provider_id`). Returns affected rows.
pub async fn upsert(conn: &Connection, p: &WeatherProvider) -> Result<u64, String> {
    conn.execute(
        "INSERT INTO weather_providers (provider_id, name_zh, name_en, attribution_url, license) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(provider_id) DO UPDATE SET name_zh = excluded.name_zh, \
            name_en = excluded.name_en, attribution_url = excluded.attribution_url, \
            license = excluded.license",
        libsql::params![
            p.provider_id.clone(),
            p.name_zh.clone(),
            p.name_en.clone(),
            p.attribution_url.clone(),
            p.license.clone()
        ],
    )
    .await
    .map_err(|e| format!("weather_providers upsert failed: {e}"))
}

/// Every provider, ordered by provider_id.
pub async fn list(conn: &Connection) -> Result<Vec<WeatherProvider>, String> {
    let mut rows = conn
        .query(
            "SELECT provider_id, name_zh, name_en, attribution_url, license \
             FROM weather_providers ORDER BY provider_id",
            (),
        )
        .await
        .map_err(|e| format!("weather_providers query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("weather_providers row read failed: {e}"))?
    {
        out.push(WeatherProvider {
            provider_id: r.get(0).unwrap_or_default(),
            name_zh: r.get(1).unwrap_or_default(),
            name_en: r.get(2).unwrap_or_default(),
            attribution_url: r.get(3).unwrap_or_default(),
            license: r.get(4).unwrap_or_default(),
        });
    }
    Ok(out)
}

/// Delete one provider (test teardown / retiring a provider). Returns affected rows.
pub async fn delete(conn: &Connection, provider_id: &str) -> Result<u64, String> {
    conn.execute(
        "DELETE FROM weather_providers WHERE provider_id = ?1",
        libsql::params![provider_id.to_string()],
    )
    .await
    .map_err(|e| format!("weather_providers delete failed: {e}"))
}
