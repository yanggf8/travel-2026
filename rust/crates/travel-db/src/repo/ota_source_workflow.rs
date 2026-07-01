use libsql::Connection;

#[derive(Debug, Clone)]
pub struct WorkflowRow {
    pub source_id: String,
    pub product_type: String,
    pub nav_kind: String,
    pub url_template: String,
    pub capture_url_contains: Option<String>,
    pub settle_marker: Option<String>,
    pub settle_ms: i64,
    pub agent_extraction_note: Option<String>,
}

/// Load a workflow config row for a (source_id, product_type) pair.
pub async fn get(
    conn: &Connection,
    source_id: &str,
    product_type: &str,
) -> Result<Option<WorkflowRow>, String> {
    let mut rows = conn
        .query(
            "SELECT source_id, product_type, nav_kind, url_template, capture_url_contains, \
             settle_marker, settle_ms, agent_extraction_note \
             FROM ota_source_workflow WHERE source_id = ?1 AND product_type = ?2",
            libsql::params![source_id.to_string(), product_type.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    Ok(Some(WorkflowRow {
        source_id: row.get(0).map_err(|e| e.to_string())?,
        product_type: row.get(1).map_err(|e| e.to_string())?,
        nav_kind: row.get(2).map_err(|e| e.to_string())?,
        url_template: row.get(3).map_err(|e| e.to_string())?,
        capture_url_contains: row.get(4).ok(),
        settle_marker: row.get(5).ok(),
        settle_ms: row.get(6).unwrap_or(0),
        agent_extraction_note: row.get(7).ok(),
    }))
}

/// Look up a provider-specific region id from a human region label.
pub async fn region_id(
    conn: &Connection,
    source_id: &str,
    product_type: &str,
    region_label: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT region_code FROM ota_source_region_codes \
             WHERE source_id = ?1 AND product_type = ?2 AND region_label = ?3",
            libsql::params![
                source_id.to_string(),
                product_type.to_string(),
                region_label.to_string(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    Ok(Some(row.get(0).map_err(|e| e.to_string())?))
}

/// Look up the actual URL value for a URL parameter, keyed by an internal input name/value pair.
pub async fn url_param_value(
    conn: &Connection,
    source_id: &str,
    product_type: &str,
    url_param_name: &str,
    input_name: &str,
    input_value: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT url_value FROM ota_source_url_param \
             WHERE source_id = ?1 AND product_type = ?2 AND url_param_name = ?3 \
               AND input_name = ?4 AND input_value = ?5",
            libsql::params![
                source_id.to_string(),
                product_type.to_string(),
                url_param_name.to_string(),
                input_name.to_string(),
                input_value.to_string(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    Ok(Some(row.get(0).map_err(|e| e.to_string())?))
}

/// Distinct internal `input_name` values registered for a URL parameter on a source/product_type pair.
pub async fn url_param_input_names(
    conn: &Connection,
    source_id: &str,
    product_type: &str,
    url_param_name: &str,
) -> Result<Vec<String>, String> {
    let mut rows = conn
        .query(
            "SELECT DISTINCT input_name FROM ota_source_url_param \
             WHERE source_id = ?1 AND product_type = ?2 AND url_param_name = ?3 \
             ORDER BY input_name",
            libsql::params![
                source_id.to_string(),
                product_type.to_string(),
                url_param_name.to_string(),
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(row.get(0).map_err(|e| e.to_string())?);
    }
    Ok(out)
}