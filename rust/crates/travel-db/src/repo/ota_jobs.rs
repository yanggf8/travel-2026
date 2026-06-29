use crate::ids::new_run_id;
use libsql::Connection;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct OtaJobRow {
    pub job_id: String,
    pub source_id: String,
    pub product_type: String,
    pub status: String,
    pub claimed_by: Option<String>,
    pub claimed_at: Option<String>,
    pub claim_token: Option<String>,
    pub lease_expires_at: Option<String>,
    pub heartbeat_at: Option<String>,
    pub attempts: i64,
    pub max_attempts: i64,
    pub next_retry_at: Option<String>,
    pub blocked_reason_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct EnqueueInput {
    pub job_id: String,
    pub source_id: String,
    pub product_type: String,
    pub params: HashMap<String, String>,
    pub now: String,
}

/// Insert a queued job + param rows.
pub async fn enqueue(conn: &Connection, input: &EnqueueInput) -> Result<(), String> {
    conn.execute(
        "INSERT INTO ota_jobs (job_id, source_id, product_type, status, created_at, updated_at) \
         VALUES (?1, ?2, ?3, 'queued', ?4, ?4)",
        libsql::params![
            input.job_id.clone(),
            input.source_id.clone(),
            input.product_type.clone(),
            input.now.clone(),
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    for (key, value) in &input.params {
        conn.execute(
            "INSERT INTO ota_job_params (job_id, param_key, param_value) VALUES (?1, ?2, ?3)",
            libsql::params![input.job_id.clone(), key.clone(), value.clone()],
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Load a job by id.
pub async fn get(conn: &Connection, job_id: &str) -> Result<Option<OtaJobRow>, String> {
    let mut rows = conn
        .query(
            "SELECT job_id, source_id, product_type, status, claimed_by, claimed_at, claim_token, \
             lease_expires_at, heartbeat_at, attempts, max_attempts, next_retry_at, \
             blocked_reason_code, created_at, updated_at \
             FROM ota_jobs WHERE job_id = ?1",
            libsql::params![job_id.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    Ok(Some(read_job_row(&row)?))
}

fn read_job_row(row: &libsql::Row) -> Result<OtaJobRow, String> {
    Ok(OtaJobRow {
        job_id: row.get(0).map_err(|e| e.to_string())?,
        source_id: row.get(1).map_err(|e| e.to_string())?,
        product_type: row.get(2).map_err(|e| e.to_string())?,
        status: row.get(3).map_err(|e| e.to_string())?,
        claimed_by: row.get(4).ok(),
        claimed_at: row.get(5).ok(),
        claim_token: row.get(6).ok(),
        lease_expires_at: row.get(7).ok(),
        heartbeat_at: row.get(8).ok(),
        attempts: row.get(9).unwrap_or(0),
        max_attempts: row.get(10).unwrap_or(3),
        next_retry_at: row.get(11).ok(),
        blocked_reason_code: row.get(12).ok(),
        created_at: row.get(13).map_err(|e| e.to_string())?,
        updated_at: row.get(14).map_err(|e| e.to_string())?,
    })
}

#[derive(Debug, Clone)]
pub struct ClaimResult {
    pub job_id: String,
    pub claim_token: String,
    pub source_id: String,
    pub product_type: String,
}

/// Claim the oldest queued job. Returns `None` when no job is available or lost the race.
pub async fn claim(
    conn: &Connection,
    worker_id: &str,
    now: &str,
    lease_expires_at: &str,
) -> Result<Option<ClaimResult>, String> {
    loop {
        // Only claim jobs that still have attempts left; a poison job at its ceiling is parked
        // as 'failed' by reap_stale, so max_attempts is a real give-up bound.
        let mut rows = conn
            .query(
                "SELECT job_id, source_id, product_type FROM ota_jobs \
                 WHERE status = 'queued' AND attempts < max_attempts \
                 ORDER BY created_at ASC LIMIT 1",
                (),
            )
            .await
            .map_err(|e| e.to_string())?;
        let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let job_id: String = row.get(0).map_err(|e| e.to_string())?;
        let source_id: String = row.get(1).map_err(|e| e.to_string())?;
        let product_type: String = row.get(2).map_err(|e| e.to_string())?;
        let claim_token = new_run_id();

        let affected = conn
            .execute(
                "UPDATE ota_jobs SET status = 'claimed', claimed_by = ?1, claim_token = ?2, \
                 claimed_at = ?3, heartbeat_at = ?3, lease_expires_at = ?4, updated_at = ?3 \
                 WHERE job_id = ?5 AND status = 'queued' AND attempts < max_attempts",
                libsql::params![
                    worker_id.to_string(),
                    claim_token.clone(),
                    now.to_string(),
                    lease_expires_at.to_string(),
                    job_id.clone(),
                ],
            )
            .await
            .map_err(|e| e.to_string())?;
        if affected == 1 {
            return Ok(Some(ClaimResult {
                job_id,
                claim_token,
                source_id,
                product_type,
            }));
        }
    }
}

/// Extend lease when token matches. Returns affected row count.
pub async fn heartbeat(
    conn: &Connection,
    job_id: &str,
    claim_token: &str,
    now: &str,
    lease_expires_at: &str,
) -> Result<u64, String> {
    conn.execute(
        "UPDATE ota_jobs SET heartbeat_at = ?1, lease_expires_at = ?2, updated_at = ?1 \
         WHERE job_id = ?3 AND claim_token = ?4 AND status IN ('claimed', 'running')",
        libsql::params![
            now.to_string(),
            lease_expires_at.to_string(),
            job_id.to_string(),
            claim_token.to_string(),
        ],
    )
    .await
    .map_err(|e| e.to_string())
}

/// Flip claimed → running when token matches.
pub async fn mark_running(
    conn: &Connection,
    job_id: &str,
    claim_token: &str,
    now: &str,
) -> Result<u64, String> {
    conn.execute(
        "UPDATE ota_jobs SET status = 'running', updated_at = ?1 \
         WHERE job_id = ?2 AND claim_token = ?3 AND status IN ('claimed', 'running')",
        libsql::params![now.to_string(), job_id.to_string(), claim_token.to_string()],
    )
    .await
    .map_err(|e| e.to_string())
}

/// Increment a job's `attempts` counter (token-guarded). Call once per attempt, after a
/// successful `mark_running`, so `max_attempts` is enforceable. Returns affected row count.
pub async fn bump_attempts(
    conn: &Connection,
    job_id: &str,
    claim_token: &str,
    now: &str,
) -> Result<u64, String> {
    conn.execute(
        "UPDATE ota_jobs SET attempts = attempts + 1, updated_at = ?1 \
         WHERE job_id = ?2 AND claim_token = ?3 AND status IN ('claimed', 'running')",
        libsql::params![now.to_string(), job_id.to_string(), claim_token.to_string()],
    )
    .await
    .map_err(|e| e.to_string())
}

/// Token-guarded terminal update. Returns affected row count.
pub async fn finish(
    conn: &Connection,
    job_id: &str,
    claim_token: &str,
    status: &str,
    blocked_reason_code: Option<&str>,
    now: &str,
) -> Result<u64, String> {
    conn.execute(
        "UPDATE ota_jobs SET status = ?1, blocked_reason_code = ?2, updated_at = ?3 \
         WHERE job_id = ?4 AND claim_token = ?5 AND status IN ('claimed', 'running')",
        libsql::params![
            status.to_string(),
            blocked_reason_code.map(|s| s.to_string()),
            now.to_string(),
            job_id.to_string(),
            claim_token.to_string(),
        ],
    )
    .await
    .map_err(|e| e.to_string())
}

/// Outcome of a reap pass: jobs requeued for retry vs. jobs parked as `failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReapResult {
    pub requeued: u64,
    pub failed: u64,
}

/// Reap jobs with expired leases. A job that still has attempts left is requeued (token/lease
/// cleared); a job that has reached `max_attempts` is parked as `failed` so it stops
/// re-occupying the front of the queue. Lexical comparison of `lease_expires_at < now` is safe
/// because callers validate `now` as a canonical `%Y-%m-%dT%H:%M:%SZ` timestamp.
pub async fn reap_stale(conn: &Connection, now: &str) -> Result<ReapResult, String> {
    // Park exhausted jobs first so the requeue pass doesn't re-arm them.
    let failed = conn
        .execute(
            "UPDATE ota_jobs SET status = 'failed', claimed_by = NULL, claim_token = NULL, \
             claimed_at = NULL, lease_expires_at = NULL, updated_at = ?1 \
             WHERE status IN ('claimed', 'running') AND lease_expires_at < ?1 \
             AND attempts >= max_attempts",
            libsql::params![now.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let requeued = conn
        .execute(
            "UPDATE ota_jobs SET status = 'queued', claimed_by = NULL, claim_token = NULL, \
             claimed_at = NULL, lease_expires_at = NULL, updated_at = ?1 \
             WHERE status IN ('claimed', 'running') AND lease_expires_at < ?1 \
             AND attempts < max_attempts",
            libsql::params![now.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(ReapResult { requeued, failed })
}

/// Next 1-based attempt number for a job.
pub async fn next_attempt_no(conn: &Connection, job_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(attempt_no), 0) + 1 FROM ota_attempts WHERE job_id = ?1",
            libsql::params![job_id.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Ok(1);
    };
    row.get(0).map_err(|e| e.to_string())
}

/// Count jobs matching a status (test helper).
pub async fn count_by_status(conn: &Connection, status: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM ota_jobs WHERE status = ?1",
            libsql::params![status.to_string()],
        )
        .await
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Ok(0);
    };
    row.get(0).map_err(|e| e.to_string())
}
