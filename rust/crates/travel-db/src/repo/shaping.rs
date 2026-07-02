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

/// UPSERT one `shaping_scrape_attempts` row from an import payload. Verbatim from run_import
/// (INSERT OR REPLACE, all 7 columns bound). Distinct from `insert_pending_attempt` (which is
/// the run_init seed with literal 'pending' + NULLs).
#[allow(clippy::too_many_arguments)]
pub async fn upsert_scrape_attempt(
    conn: &Connection,
    run_id: &str,
    dest_code: &str,
    nights: i64,
    status: &str,
    candidate_count: Option<i64>,
    error: Option<&str>,
    attempted_at: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO shaping_scrape_attempts
          (run_id, dest_code, nights, status, candidate_count, error, attempted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        libsql::params![
            run_id.to_string(),
            dest_code.to_string(),
            nights,
            status.to_string(),
            candidate_count,
            error.map(|s| s.to_string()),
            attempted_at.to_string(),
        ],
    )
    .await
    .map_err(|e| format!("upsert scrape attempt failed: {e}"))?;
    Ok(())
}

/// One imported candidate row (rank + adopted_plan_id are written NULL; rank is set later by
/// `set_rank`). Verbatim column set from run_import.
#[derive(Debug, Clone)]
pub struct CandidateWrite {
    pub candidate_id: String,
    pub run_id: String,
    pub dest_code: String,
    pub depart_date: String,
    pub return_date: String,
    pub nights: i64,
    pub flight_total_twd: Option<i64>,
    pub leave_days: Option<i64>,
    pub verdict: Option<String>,
}

/// INSERT one `shaping_candidates` row (rank=NULL, adopted_plan_id=NULL). Verbatim.
pub async fn insert_candidate(conn: &Connection, c: &CandidateWrite) -> Result<(), String> {
    conn.execute(
        "INSERT INTO shaping_candidates
          (candidate_id, run_id, dest_code, depart_date, return_date, nights,
           flight_total_twd, leave_days, rank, verdict, adopted_plan_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, NULL)",
        libsql::params![
            c.candidate_id.clone(),
            c.run_id.clone(),
            c.dest_code.clone(),
            c.depart_date.clone(),
            c.return_date.clone(),
            c.nights,
            c.flight_total_twd,
            c.leave_days,
            c.verdict.clone(),
        ],
    )
    .await
    .map_err(|e| format!("insert shaping_candidates failed: {e}"))?;
    Ok(())
}

/// One imported candidate flight leg. Verbatim column set from run_import.
#[derive(Debug, Clone)]
pub struct CandidateFlightWrite {
    pub candidate_id: String,
    pub direction: String,
    pub airline: Option<String>,
    pub depart_time: Option<String>,
    pub arrive_time: Option<String>,
    pub duration: Option<String>,
    pub nonstop: Option<i64>,
    pub price_total_twd: Option<i64>,
}

/// INSERT one `shaping_candidate_flights` row. Verbatim.
pub async fn insert_candidate_flight(
    conn: &Connection,
    f: &CandidateFlightWrite,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO shaping_candidate_flights
          (candidate_id, direction, airline, depart_time, arrive_time, duration, nonstop, price_total_twd)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        libsql::params![
            f.candidate_id.clone(),
            f.direction.clone(),
            f.airline.clone(),
            f.depart_time.clone(),
            f.arrive_time.clone(),
            f.duration.clone(),
            f.nonstop,
            f.price_total_twd,
        ],
    )
    .await
    .map_err(|e| format!("insert shaping_candidate_flights failed: {e}"))?;
    Ok(())
}

/// DELETE a (run_id, dest_code, nights) pair's candidate flights then candidates (in that
/// order). Verbatim from `delete_candidates_for_pair` — scoped to the pair, NOT the whole run.
pub async fn delete_pair(
    conn: &Connection,
    run_id: &str,
    dest_code: &str,
    nights: i64,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM shaping_candidate_flights WHERE candidate_id IN
          (SELECT candidate_id FROM shaping_candidates
           WHERE run_id = ?1 AND dest_code = ?2 AND nights = ?3)",
        libsql::params![run_id.to_string(), dest_code.to_string(), nights],
    )
    .await
    .map_err(|e| format!("delete candidate flights failed: {e}"))?;
    conn.execute(
        "DELETE FROM shaping_candidates WHERE run_id = ?1 AND dest_code = ?2 AND nights = ?3",
        libsql::params![run_id.to_string(), dest_code.to_string(), nights],
    )
    .await
    .map_err(|e| format!("delete candidates failed: {e}"))?;
    Ok(())
}

/// Set a candidate's 1-based rank. Verbatim from rank_run's per-candidate UPDATE.
pub async fn set_rank(conn: &Connection, candidate_id: &str, rank: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE shaping_candidates SET rank = ?1 WHERE candidate_id = ?2",
        libsql::params![rank, candidate_id.to_string()],
    )
    .await
    .map_err(|e| format!("update rank failed: {e}"))?;
    Ok(())
}

/// Set a run's status (e.g. 'ranked' or 'failed') + updated_at. Verbatim from rank_run /
/// run_import's empty-candidates branch.
pub async fn set_run_status(
    conn: &Connection,
    run_id: &str,
    status: &str,
    updated_at: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE shaping_research_runs SET status = ?1, updated_at = ?2 WHERE run_id = ?3",
        libsql::params![status.to_string(), updated_at.to_string(), run_id.to_string()],
    )
    .await
    .map_err(|e| format!("update run status failed: {e}"))?;
    Ok(())
}

/// Simple adopt: set `adopted_plan_id` on the candidate and mark the run adopted.
/// Verbatim from `shaping::run_adopt` (without `--create-plan`).
pub async fn set_adopted(
    conn: &Connection,
    candidate_id: &str,
    plan_id: &str,
    run_id: &str,
    ts: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE shaping_candidates SET adopted_plan_id = ?1 WHERE candidate_id = ?2",
        libsql::params![plan_id.to_string(), candidate_id.to_string()],
    )
    .await
    .map_err(|e| format!("update shaping_candidates failed: {e}"))?;
    conn.execute(
        "UPDATE shaping_research_runs SET status = 'adopted', updated_at = ?1 WHERE run_id = ?2",
        libsql::params![ts.to_string(), run_id.to_string()],
    )
    .await
    .map_err(|e| format!("update shaping_research_runs failed: {e}"))?;
    Ok(())
}
