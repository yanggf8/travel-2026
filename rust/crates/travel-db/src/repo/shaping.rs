//! Shaping-research domain writes (`shaping_research_*`, `shaping_rules`,
//! `shaping_scrape_attempts`, `shaping_candidates`, `shaping_candidate_flights`).
//!
//! DAL boundary: owns the shaping-research table SQL. shaping-init/import write no audit
//! triad; shaping-adopt's plan-creation audit stays in `travel-cli` (`cascade::common`).
//!
//! Unit 1 (run_init): the five research-run seed writers below. SQL copied verbatim from
//! `cascade`-adjacent `shaping::run_init` so the migration is byte-identical.

use libsql::Connection;

/// One typed shaping rule (aspect/role/kind + at most one typed value + optional notes).
#[derive(Debug, Clone)]
pub struct ShapingRuleWrite {
    pub aspect: String,
    pub role: String,
    pub kind: String,
    pub value_text: Option<String>,
    pub value_date: Option<String>,
    pub value_integer: Option<i64>,
    pub notes: Option<String>,
}

/// INSERT the parent `shaping_research_runs` row (currency literal 'TWD', status literal
/// 'started'; created_at = updated_at = ts). Verbatim from run_init.
#[allow(clippy::too_many_arguments)]
pub async fn insert_run(
    conn: &Connection,
    run_id: &str,
    origin_code: &str,
    pax: i64,
    window_start: &str,
    window_end: &str,
    exchange_rate_usd_twd: f64,
    ts: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO shaping_research_runs
          (run_id, origin_code, pax, window_start, window_end, currency,
           exchange_rate_usd_twd, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'TWD', ?6, 'started', ?7, ?8)",
        libsql::params![
            run_id.to_string(),
            origin_code.to_string(),
            pax,
            window_start.to_string(),
            window_end.to_string(),
            exchange_rate_usd_twd,
            ts.to_string(),
            ts.to_string(),
        ],
    )
    .await
    .map_err(|e| format!("insert shaping_research_runs failed: {e}"))?;
    Ok(())
}

/// INSERT one `shaping_research_destinations` row (caller passes the sort_order). Verbatim.
pub async fn insert_destination(
    conn: &Connection,
    run_id: &str,
    dest_code: &str,
    dest_label: &str,
    sort_order: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO shaping_research_destinations (run_id, dest_code, dest_label, sort_order)
         VALUES (?1, ?2, ?3, ?4)",
        libsql::params![
            run_id.to_string(),
            dest_code.to_string(),
            dest_label.to_string(),
            sort_order
        ],
    )
    .await
    .map_err(|e| format!("insert shaping_research_destinations failed: {e}"))?;
    Ok(())
}

/// INSERT one `shaping_research_durations` row (duration_days computed by the caller as
/// nights + 1, matching run_init). Verbatim.
pub async fn insert_duration(
    conn: &Connection,
    run_id: &str,
    nights: i64,
    duration_days: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO shaping_research_durations (run_id, nights, duration_days)
         VALUES (?1, ?2, ?3)",
        libsql::params![run_id.to_string(), nights, duration_days],
    )
    .await
    .map_err(|e| format!("insert shaping_research_durations failed: {e}"))?;
    Ok(())
}

/// INSERT one `shaping_rules` row. Verbatim.
pub async fn insert_rule(
    conn: &Connection,
    run_id: &str,
    rule: &ShapingRuleWrite,
    created_at: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO shaping_rules
           (run_id, aspect, role, kind, value_text, value_date, value_integer, notes, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        libsql::params![
            run_id.to_string(),
            rule.aspect.clone(),
            rule.role.clone(),
            rule.kind.clone(),
            rule.value_text.clone(),
            rule.value_date.clone(),
            rule.value_integer,
            rule.notes.clone(),
            created_at.to_string(),
        ],
    )
    .await
    .map_err(|e| format!("insert shaping_rules failed: {e}"))?;
    Ok(())
}

/// INSERT one 'pending' `shaping_scrape_attempts` row (candidate_count/error/attempted_at
/// NULL). Verbatim from run_init's per-(dest × duration) seed loop.
pub async fn insert_pending_attempt(
    conn: &Connection,
    run_id: &str,
    dest_code: &str,
    nights: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO shaping_scrape_attempts
          (run_id, dest_code, nights, status, candidate_count, error, attempted_at)
         VALUES (?1, ?2, ?3, 'pending', NULL, NULL, NULL)",
        libsql::params![run_id.to_string(), dest_code.to_string(), nights],
    )
    .await
    .map_err(|e| format!("insert shaping_scrape_attempts failed: {e}"))?;
    Ok(())
}
