//! Itinerary-cluster domain writes (`days`, `timesofday`, `activities`, `activity_tags`,
//! `session_meals`, `session_activities_zh`, `booking_status`) for scaffold-itinerary /
//! set-tod / set-activity / populate-itinerary / swap-days.
//!
//! DAL boundary: owns the itinerary-table SQL. The audit triad
//! (`plan_events`/`plan_event_data`/`operation_runs`/`plans.version`) stays in `travel-cli`
//! (`cascade::common`) — this module never touches it. Cascade orchestration, validation,
//! and event emission stay in the command modules.
//!
//! Built incrementally, one migration step at a time (scaffold → set_tod → set_activity →
//! populate → swap), each byte-identical to the inline SQL it replaces and gated by a
//! committed behavior-lock test.

use libsql::Connection;

/// One day of the scaffolded skeleton. `morning_transit`/`evening_transit` are the derived
/// arrival/departure notes (all other sessions get no transit note).
#[derive(Debug, Clone)]
pub struct SkeletonDay {
    pub day_number: i64,
    pub date: String,
    pub day_type: String,
    pub morning_transit: Option<String>,
    pub evening_transit: Option<String>,
}

/// Replace the day skeleton (+ session/activity child rows) for a destination: delete the stale
/// rows in the fixed order, then insert the fresh `days` + four `timesofday` sessions per day.
/// SQL copied verbatim from `scaffold_itinerary::write_skeleton`. A single `now` is captured by
/// the caller and used for ALL `days` + `timesofday` inserts (do not call a per-row clock here).
pub async fn replace_skeleton(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    days: &[SkeletonDay],
    now: &str,
) -> Result<(), String> {
    // Stale child rows first (FK-free, so order is just for cleanliness).
    conn.execute(
        "DELETE FROM activity_tags WHERE activity_id IN \
            (SELECT id FROM activities WHERE plan_id = ?1 AND destination = ?2)",
        libsql::params![plan_id.to_string(), destination.to_string()],
    )
    .await
    .map_err(|e| format!("activity_tags delete failed: {e}"))?;
    for tbl in [
        "session_meals",
        "session_activities_zh",
        "activities",
        "timesofday",
        "days",
    ] {
        conn.execute(
            &format!("DELETE FROM {tbl} WHERE plan_id = ?1 AND destination = ?2"),
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("{tbl} delete failed: {e}"))?;
    }

    for d in days {
        conn.execute(
            "INSERT INTO days \
                (plan_id, destination, day_number, date, theme, day_type, status, updated_at) \
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'draft', ?6)",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                d.day_number,
                d.date.clone(),
                d.day_type.clone(),
                now.to_string()
            ],
        )
        .await
        .map_err(|e| format!("days INSERT failed: {e}"))?;

        for sess in ["morning", "noon", "afternoon", "evening"] {
            let transit: Option<String> = match sess {
                "morning" => d.morning_transit.clone(),
                "evening" => d.evening_transit.clone(),
                _ => None,
            };
            conn.execute(
                "INSERT INTO timesofday \
                    (plan_id, destination, day_number, session_type, focus, transit_notes, \
                     booking_notes, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5, NULL, ?6)",
                libsql::params![
                    plan_id.to_string(),
                    destination.to_string(),
                    d.day_number,
                    sess,
                    transit,
                    now.to_string()
                ],
            )
            .await
            .map_err(|e| format!("timesofday INSERT failed: {e}"))?;
        }
    }
    Ok(())
}

/// `INSERT OR REPLACE` one `process_statuses` row. NOTE: this is the INSERT OR REPLACE variant
/// used by `scaffold_itinerary::set_process_status` — distinct from the ON CONFLICT upsert in
/// [`crate::repo::process_statuses::upsert`]. SQL copied verbatim; caller passes `now`.
pub async fn upsert_process_status_replace(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    process_id: &str,
    status: &str,
    now: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO process_statuses \
            (plan_id, destination, process_id, status, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            process_id.to_string(),
            status.to_string(),
            now.to_string()
        ],
    )
    .await
    .map_err(|e| format!("process_statuses UPSERT failed: {e}"))?;
    Ok(())
}

/// `INSERT OR REPLACE` one `cascade_dirty_flags` row with `dirty = 0`. SQL copied verbatim from
/// `scaffold_itinerary::clear_dirty`; caller passes `now` for `last_changed`.
pub async fn clear_dirty(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    process_id: &str,
    now: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO cascade_dirty_flags \
            (plan_id, destination, process_id, dirty, last_changed) \
         VALUES (?1, ?2, ?3, 0, ?4)",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            process_id.to_string(),
            now.to_string()
        ],
    )
    .await
    .map_err(|e| format!("cascade_dirty_flags UPSERT failed: {e}"))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// set-tod family (set-tod-focus / set-tod-time-range / set-tod-zh / set-meals)
// domain writes. SQL copied verbatim from `set_tod.rs`; caller passes `now`.
// The audit triad stays in `cascade::common` — this module never touches it.
// ─────────────────────────────────────────────────────────────────

/// UPDATE a single `timesofday` scalar column (`focus`/`focus_zh`/`transit_notes_zh`) for a
/// `(plan, dest, day, session)` row, bumping `updated_at`, then touch the day. SQL copied verbatim
/// from `set_tod::apply_tod_field` (which trailed with a `touch_day`) — preserve BOTH writes. The
/// `field` guard is retained exactly: an unsupported field returns `Err`.
#[allow(clippy::too_many_arguments)]
pub async fn update_tod_field(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    field: &str,
    value: Option<String>,
    now: &str,
) -> Result<(), String> {
    // Mirror the TS sync INSERT OR REPLACE pattern. The Rust
    // equivalent for an UPDATE of a single column.
    let column = match field {
        "focus" => "focus",
        "focus_zh" => "focus_zh",
        "transit_notes_zh" => "transit_notes_zh",
        _ => return Err(format!("unsupported timesofday field: {field}")),
    };
    let sql = format!(
        "UPDATE timesofday SET {column} = ?1, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 AND session_type = ?6"
    );
    conn.execute(
        &sql,
        libsql::params![
            value,
            now.to_string(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string()
        ],
    )
    .await
    .map_err(|e| format!("timesofday UPDATE {field} failed: {e}"))?;
    touch_day(conn, plan_id, destination, day, now).await?;
    Ok(())
}

/// UPDATE the `timesofday.time_range_start`/`time_range_end` for a `(plan, dest, day, session)` row,
/// bumping `updated_at`. SQL copied verbatim from `set_tod::apply_time_range` — it does NOT touch
/// the day (the caller `run_time_range` touches separately); preserve that difference.
#[allow(clippy::too_many_arguments)]
pub async fn update_tod_time_range(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    start: &str,
    end: &str,
    now: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE timesofday SET time_range_start = ?1, time_range_end = ?2, updated_at = ?3 \
         WHERE plan_id = ?4 AND destination = ?5 AND day_number = ?6 AND session_type = ?7",
        libsql::params![
            start.to_string(),
            end.to_string(),
            now.to_string(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string()
        ],
    )
    .await
    .map_err(|e| format!("timesofday time_range UPDATE failed: {e}"))?;
    Ok(())
}

/// Touch a `days` row's `updated_at`. SQL copied verbatim from `set_tod::touch_day`; caller passes
/// `now`.
pub async fn touch_day(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    now: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET updated_at = ?1 WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4",
        libsql::params![now.to_string(), plan_id.to_string(), destination.to_string(), day],
    )
    .await
    .map_err(|e| format!("days touch UPDATE failed: {e}"))?;
    Ok(())
}

/// Ordered replace of `session_activities_zh` for a `(plan, dest, day, session)`: DELETE all rows
/// then re-INSERT one per activity with `sort_order` = index. An empty slice (`--clear-activities`)
/// does just the DELETE. SQL copied verbatim from the inline block in `set_tod::run_zh`.
pub async fn replace_session_activities_zh(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activities: &[String],
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM session_activities_zh \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string()
        ],
    )
    .await
    .map_err(|e| format!("session_activities_zh DELETE failed: {e}"))?;
    for (i, a) in activities.iter().enumerate() {
        conn.execute(
            "INSERT INTO session_activities_zh \
                (plan_id, destination, day_number, session_type, sort_order, activity) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                i as i64,
                a.clone()
            ],
        )
        .await
        .map_err(|e| format!("session_activities_zh INSERT failed: {e}"))?;
    }
    Ok(())
}

/// Ordered replace of `session_meals` for a `(plan, dest, day, session)`: DELETE all rows then
/// re-INSERT one per meal with `sort_order` = index. SQL copied verbatim from the inline block in
/// `set_tod::run_meals`. Caller keeps its per-meal stdout echo.
pub async fn replace_session_meals(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    meals: &[String],
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM session_meals \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string()
        ],
    )
    .await
    .map_err(|e| format!("session_meals DELETE failed: {e}"))?;
    for (i, meal) in meals.iter().enumerate() {
        conn.execute(
            "INSERT INTO session_meals \
                (plan_id, destination, day_number, session_type, sort_order, meal) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                i as i64,
                meal.clone()
            ],
        )
        .await
        .map_err(|e| format!("session_meals INSERT failed: {e}"))?;
    }
    Ok(())
}
