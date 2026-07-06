//! Plan-lifecycle domain writes (soft-delete, plan-creation seed) for
//! `mark-plan-deleted` and `shaping-adopt --create-plan`.
//!
//! DAL boundary: owns the domain-table SQL (the `plans` soft-delete UPDATE + the
//! create-plan seed). The audit triad (`plan_events`/`plan_event_data`/
//! `operation_runs`/`plans.version`) stays in `travel-cli` (`cascade::common`) —
//! this module never touches it. Event emission + record_operation + the shaping
//! pointer updates + the tour-group bridge stay in the CLI orchestration.

use libsql::Connection;

/// Soft-delete: set plans.deleted_at (domain write only; version bump is the audit
/// back-half, done by cascade::common::record_operation in the CLI).
pub async fn soft_delete(conn: &Connection, plan_id: &str, now_db: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE plans SET deleted_at = datetime('now'), updated_at = ?2 WHERE plan_id = ?1",
        libsql::params![plan_id.to_string(), now_db.to_string()],
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn list_destination_slugs(conn: &Connection, plan_id: &str) -> Result<Vec<String>, String> {
    let mut rows = conn
        .query(
            "SELECT slug FROM plan_destinations WHERE plan_id = ?1 ORDER BY slug",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_destinations query failed: {e}"))?;
    let mut slugs = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_destinations row read failed: {e}"))?
    {
        slugs.push(row.get::<String>(0).map_err(|e| e.to_string())?);
    }
    Ok(slugs)
}

pub async fn set_display_name(
    conn: &Connection,
    plan_id: &str,
    slug: &str,
    name: &str,
    now_db: &str,
) -> Result<u64, String> {
    conn.execute(
        "UPDATE plan_destinations SET display_name = ?3, updated_at = ?4 WHERE plan_id = ?1 AND slug = ?2",
        libsql::params![
            plan_id.to_string(),
            slug.to_string(),
            name.to_string(),
            now_db.to_string()
        ],
    )
    .await
    .map_err(|e| format!("plan_destinations update failed: {e}"))
}

pub async fn set_active_destination(
    conn: &Connection,
    plan_id: &str,
    slug: &str,
    now_db: &str,
) -> Result<u64, String> {
    conn.execute(
        "UPDATE plan_metadata SET active_destination = ?2, updated_at = ?3 WHERE plan_id = ?1",
        libsql::params![plan_id.to_string(), slug.to_string(), now_db.to_string()],
    )
    .await
    .map_err(|e| format!("plan_metadata update failed: {e}"))
}

/// The seed values for one `shaping-adopt --create-plan` (all owned; travel-db does
/// not depend on any travel-cli type). `ts` is the run's now_rfc3339 timestamp used
/// where the original bound it (plan_destinations created_at/updated_at).
#[derive(Debug, Clone)]
pub struct PlanSeed {
    pub plan_id: String,
    pub schema_version: String,
    pub dest_slug: String,
    pub display_name: String,
    pub origin_code: Option<String>,
    pub region: String,
    pub primary_airport: String,
    pub nights: i64,
    pub start_date: String,
    pub end_date: String,
    pub days: i64,
    pub session: String,
    pub ts: String,
}

/// The 6 process rows the create-plan seed writes (plain INSERT, in this order).
const SEED_PROCESS_ROWS: &[(&str, &str)] = &[
    ("process_1_date_anchor", "confirmed"),
    ("process_2_destination", "confirmed"),
    ("process_3_transportation", "pending"),
    ("process_3_4_packages", "pending"),
    ("process_4_accommodation", "pending"),
    ("process_5_daily_itinerary", "pending"),
];

/// Create-plan seed: the ordered plan/plan_metadata/plan_destinations/destination_details/
/// destination_cities/date_anchors inserts, then the 6 `process_statuses` rows (PLAIN INSERT —
/// NOT the ON CONFLICT upsert), then event_log_state + event_log_destinations. BYTE-IDENTICAL
/// to the inline seed in `shaping::adopt_candidate_to_new_plan` (same SQL text, `datetime('now')`
/// where used, same order). The caller emits plan_events/plan_event_data, calls record_operation
/// (version 0→1), updates the shaping pointers, and runs the bridge — none of that lives here.
pub async fn create_plan_seed(conn: &Connection, s: &PlanSeed) -> Result<(), String> {
    let stmts: Vec<(&str, Vec<libsql::Value>)> = vec![
        (
            "INSERT INTO plans (plan_id, schema_version, updated_at) VALUES (?1, ?2, datetime('now'))",
            vec![s.plan_id.clone().into(), s.schema_version.clone().into()],
        ),
        (
            "INSERT INTO plan_metadata (plan_id, schema_version, active_destination, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))",
            vec![
                s.plan_id.clone().into(),
                s.schema_version.clone().into(),
                s.dest_slug.clone().into(),
            ],
        ),
        (
            "INSERT INTO plan_destinations (plan_id, slug, display_name, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'active', ?4, ?5)",
            vec![
                s.plan_id.clone().into(),
                s.dest_slug.clone().into(),
                s.display_name.clone().into(),
                s.ts.clone().into(),
                s.ts.clone().into(),
            ],
        ),
        (
            "INSERT INTO destination_details (plan_id, destination, origin_city, region, primary_airport, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            vec![
                s.plan_id.clone().into(),
                s.dest_slug.clone().into(),
                s.origin_code
                    .clone()
                    .map(libsql::Value::from)
                    .unwrap_or(libsql::Value::Null),
                s.region.clone().into(),
                s.primary_airport.clone().into(),
            ],
        ),
        (
            "INSERT INTO destination_cities (plan_id, destination, city_slug, display_name, role, nights, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'primary', ?5, datetime('now'))",
            vec![
                s.plan_id.clone().into(),
                s.dest_slug.clone().into(),
                s.dest_slug.clone().into(),
                s.display_name.clone().into(),
                s.nights.into(),
            ],
        ),
        (
            "INSERT INTO date_anchors (plan_id, destination, start_date, end_date, days, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            vec![
                s.plan_id.clone().into(),
                s.dest_slug.clone().into(),
                s.start_date.clone().into(),
                s.end_date.clone().into(),
                s.days.into(),
            ],
        ),
    ];
    for (sql, p) in stmts {
        conn.execute(sql, libsql::params_from_iter(p))
            .await
            .map_err(|e| format!("adopt insert failed ({sql:.40}): {e}"))?;
    }

    // process_statuses — PLAIN INSERT (not upsert), the 6 rows in order.
    for (pid, st) in SEED_PROCESS_ROWS {
        conn.execute(
            "INSERT INTO process_statuses (plan_id, destination, process_id, status, updated_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            libsql::params![s.plan_id.clone(), s.dest_slug.clone(), pid.to_string(), st.to_string()],
        )
        .await
        .map_err(|e| format!("insert process_statuses failed: {e}"))?;
    }

    // event log
    conn.execute(
        "INSERT INTO event_log_state (plan_id, session, project, version, current_focus, active_destination)
         VALUES (?1, ?2, 'japan-travel', '3.0', '', ?3)",
        libsql::params![s.plan_id.clone(), s.session.clone(), s.dest_slug.clone()],
    )
    .await
    .map_err(|e| format!("insert event_log_state failed: {e}"))?;
    conn.execute(
        "INSERT INTO event_log_destinations (plan_id, destination, status)
         VALUES (?1, ?2, 'active')",
        libsql::params![s.plan_id.clone(), s.dest_slug.clone()],
    )
    .await
    .map_err(|e| format!("insert event_log_destinations failed: {e}"))?;

    Ok(())
}
