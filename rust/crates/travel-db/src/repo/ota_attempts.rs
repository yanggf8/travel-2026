use libsql::Connection;

#[derive(Debug, Clone)]
pub struct AttemptInput {
    pub attempt_id: String,
    pub job_id: String,
    pub attempt_no: i64,
    pub claim_token: String,
    pub outcome: String,
    pub capture_id: Option<String>,
    pub candidate_count: i64,
    pub inserted_count: i64,
    pub deduped_count: i64,
    pub error_detail: Option<String>,
    pub started_at: String,
    pub finished_at: String,
}

/// Record one `ota_attempts` row.
pub async fn record(conn: &Connection, input: &AttemptInput) -> Result<(), String> {
    conn.execute(
        "INSERT INTO ota_attempts \
         (attempt_id, job_id, attempt_no, claim_token, outcome, capture_id, candidate_count, \
          inserted_count, deduped_count, error_detail, started_at, finished_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        libsql::params![
            input.attempt_id.clone(),
            input.job_id.clone(),
            input.attempt_no,
            input.claim_token.clone(),
            input.outcome.clone(),
            input.capture_id.clone(),
            input.candidate_count,
            input.inserted_count,
            input.deduped_count,
            input.error_detail.clone(),
            input.started_at.clone(),
            input.finished_at.clone(),
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}