use crate::ids::new_run_id;
use libsql::Connection;

#[derive(Debug, Clone, Default)]
pub struct ObservationInput {
    pub source_id: String,
    pub product_type: Option<String>,
    pub job_id: Option<String>,
    pub attempt_id: Option<String>,
    pub observation_type: String,
    pub block_reason_code: Option<String>,
    pub severity: String,
    pub http_status: Option<i64>,
    pub field_name: Option<String>,
    pub selector: Option<String>,
    pub expected_value: Option<String>,
    pub observed_value: Option<String>,
    pub duration_ms: Option<i64>,
    pub freshness_reference_at: Option<String>,
    pub detail: Option<String>,
    pub observed_at: String,
}

/// Insert one `ota_observations` row. Returns the new `observation_id`.
pub async fn record(conn: &Connection, input: &ObservationInput) -> Result<String, String> {
    let observation_id = new_run_id();
    conn.execute(
        "INSERT INTO ota_observations \
         (observation_id, source_id, product_type, job_id, attempt_id, observation_type, \
          block_reason_code, severity, http_status, field_name, selector, expected_value, \
          observed_value, duration_ms, freshness_reference_at, detail, observed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        libsql::params![
            observation_id.clone(),
            input.source_id.clone(),
            input.product_type.clone(),
            input.job_id.clone(),
            input.attempt_id.clone(),
            input.observation_type.clone(),
            input.block_reason_code.clone(),
            input.severity.clone(),
            input.http_status,
            input.field_name.clone(),
            input.selector.clone(),
            input.expected_value.clone(),
            input.observed_value.clone(),
            input.duration_ms,
            input.freshness_reference_at.clone(),
            input.detail.clone(),
            input.observed_at.clone(),
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(observation_id)
}

#[derive(Debug, Clone)]
pub struct ObservationRow {
    pub observation_id: String,
    pub source_id: String,
    pub product_type: Option<String>,
    pub job_id: Option<String>,
    pub attempt_id: Option<String>,
    pub observation_type: String,
    pub block_reason_code: Option<String>,
    pub severity: String,
    pub http_status: Option<i64>,
    pub field_name: Option<String>,
    pub selector: Option<String>,
    pub expected_value: Option<String>,
    pub observed_value: Option<String>,
    pub duration_ms: Option<i64>,
    pub freshness_reference_at: Option<String>,
    pub detail: Option<String>,
    pub observed_at: String,
}

pub struct ObservationFilter<'a> {
    pub source_id: Option<&'a str>,
    pub observation_type: Option<&'a str>,
    pub severity: Option<&'a str>,
}

/// List observations matching optional filters, newest first.
pub async fn list(
    conn: &Connection,
    filter: &ObservationFilter<'_>,
) -> Result<Vec<ObservationRow>, String> {
    let mut sql = String::from(
        "SELECT observation_id, source_id, product_type, job_id, attempt_id, observation_type, \
         block_reason_code, severity, http_status, field_name, selector, expected_value, \
         observed_value, duration_ms, freshness_reference_at, detail, observed_at \
         FROM ota_observations WHERE 1=1",
    );
    let mut binds: Vec<String> = Vec::new();
    if let Some(s) = filter.source_id {
        binds.push(s.to_string());
        sql.push_str(&format!(" AND source_id = ?{}", binds.len()));
    }
    if let Some(t) = filter.observation_type {
        binds.push(t.to_string());
        sql.push_str(&format!(" AND observation_type = ?{}", binds.len()));
    }
    if let Some(sev) = filter.severity {
        binds.push(sev.to_string());
        sql.push_str(&format!(" AND severity = ?{}", binds.len()));
    }
    sql.push_str(" ORDER BY observed_at DESC LIMIT 200");

    let params: Vec<libsql::Value> = binds.into_iter().map(libsql::Value::Text).collect();
    let mut rows = conn
        .query(&sql, params)
        .await
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(ObservationRow {
            observation_id: row.get(0).map_err(|e| e.to_string())?,
            source_id: row.get(1).map_err(|e| e.to_string())?,
            product_type: row.get(2).ok(),
            job_id: row.get(3).ok(),
            attempt_id: row.get(4).ok(),
            observation_type: row.get(5).map_err(|e| e.to_string())?,
            block_reason_code: row.get(6).ok(),
            severity: row.get(7).map_err(|e| e.to_string())?,
            http_status: row.get(8).ok(),
            field_name: row.get(9).ok(),
            selector: row.get(10).ok(),
            expected_value: row.get(11).ok(),
            observed_value: row.get(12).ok(),
            duration_ms: row.get(13).ok(),
            freshness_reference_at: row.get(14).ok(),
            detail: row.get(15).ok(),
            observed_at: row.get(16).map_err(|e| e.to_string())?,
        });
    }
    Ok(out)
}