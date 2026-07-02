//! OTA provider-catalog domain writes for `set-ota-source`/`set-ota-coverage`/
//! `set-ota-region-code`/`set-ota-url-param` (the globally-scoped `ota_sources` /
//! `ota_source_coverage` / `ota_source_region_codes` / `ota_source_url_param`
//! tables; `ota_source_workflow` already has its own repo module).
//!
//! DAL boundary: owns the domain-table SQL. The `catalog_runs` audit row stays in
//! `travel-cli` — this module never touches it.

use libsql::Connection;

/// UPSERT `ota_sources` identity (name/status). COALESCE keeps an existing value
/// when a flag is omitted. SQL verbatim from set_ota_catalog.rs::run_set_source.
pub async fn upsert_source(
    conn: &Connection,
    source_id: &str,
    name: Option<&str>,
    status: Option<&str>,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO ota_sources (source_id, name, status, updated_at) \
         VALUES (?1, COALESCE(?2, ?1), COALESCE(?3, 'active'), ?4) \
         ON CONFLICT(source_id) DO UPDATE SET \
            name = COALESCE(?2, ota_sources.name), \
            status = COALESCE(?3, ota_sources.status), \
            updated_at = ?4",
        libsql::params![
            source_id.to_string(),
            name.map(|s| s.to_string()),
            status.map(|s| s.to_string()),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// UPSERT one `ota_source_coverage` row. SQL verbatim from run_set_coverage.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_coverage(
    conn: &Connection,
    source_id: &str,
    product_type: &str,
    proven_int: i64,
    proven_at: Option<&str>,
    method: Option<&str>,
    search_url: Option<&str>,
    blocked: Option<&str>,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO ota_source_coverage \
            (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(source_id, product_type) DO UPDATE SET \
            proven = ?3, \
            proven_at = COALESCE(?4, ota_source_coverage.proven_at), \
            method = COALESCE(?5, ota_source_coverage.method), \
            search_url = COALESCE(?6, ota_source_coverage.search_url), \
            blocked_reason_code = ?7, \
            updated_at = ?8",
        libsql::params![
            source_id.to_string(),
            product_type.to_string(),
            proven_int,
            proven_at.map(|s| s.to_string()),
            method.map(|s| s.to_string()),
            search_url.map(|s| s.to_string()),
            blocked.map(|s| s.to_string()),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// UPSERT one `ota_source_region_codes` row. SQL verbatim from run_set_region.
pub async fn upsert_region_code(
    conn: &Connection,
    source_id: &str,
    product_type: &str,
    region_label: &str,
    region_code: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO ota_source_region_codes (source_id, product_type, region_label, region_code) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(source_id, product_type, region_label) DO UPDATE SET region_code = ?4",
        libsql::params![
            source_id.to_string(),
            product_type.to_string(),
            region_label.to_string(),
            region_code.to_string()
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// UPSERT one `ota_source_url_param` row. SQL verbatim from run_set_url_param.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_url_param(
    conn: &Connection,
    source_id: &str,
    product_type: &str,
    url_param_name: &str,
    input_name: &str,
    input_value: &str,
    url_value: &str,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO ota_source_url_param \
            (source_id, product_type, url_param_name, input_name, input_value, url_value, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(source_id, product_type, url_param_name, input_name, input_value) DO UPDATE SET \
            url_value = ?6, updated_at = ?7",
        libsql::params![
            source_id.to_string(),
            product_type.to_string(),
            url_param_name.to_string(),
            input_name.to_string(),
            input_value.to_string(),
            url_value.to_string(),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}
