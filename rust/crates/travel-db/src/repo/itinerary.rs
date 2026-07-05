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
    source: &str,
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
                (plan_id, destination, day_number, session_type, sort_order, meal, source) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                i as i64,
                meal.clone(),
                source.to_string()
            ],
        )
        .await
        .map_err(|e| format!("session_meals INSERT failed: {e}"))?;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// set-activity family (set-activity-time / -title / -booking, delete-activity,
// move-activity, add-activity, reorder-activities) domain writes on the
// `activities` + `activity_tags` tables. SQL copied verbatim from `set_activity.rs`;
// caller passes `now` for `updated_at` (fresh `now_db_datetime()` per write, matching
// the prior inline timing). The audit triad + event emission + activity-resolution
// stay in `travel-cli` — this module never touches them.
// ─────────────────────────────────────────────────────────────────

/// UPDATE a single `activities` scalar column (`start_time`/`end_time`/`is_fixed_time`/
/// `booking_status`/`booking_ref`/`book_by`/`booking_required`) for a
/// `(plan, dest, day, session, id)` row, bumping `updated_at`. SQL copied verbatim from
/// `set_activity::update_field` — the `{field}` is a fixed column name interpolated by the
/// caller (never user input). Caller passes `now`.
#[allow(clippy::too_many_arguments)]
pub async fn update_activity_field(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
    field: &str,
    value: Option<String>,
    now: &str,
) -> Result<(), String> {
    let sql = format!(
        "UPDATE activities SET {field} = ?1, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 \
           AND session_type = ?6 AND id = ?7"
    );
    conn.execute(
        &sql,
        libsql::params![
            value,
            now.to_string(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string(),
            activity_id.to_string()
        ],
    )
    .await
    .map_err(|e| format!("activities UPDATE {field} failed: {e}"))?;
    Ok(())
}

/// UPDATE an activity's `title` (bumping `updated_at`), optionally also clearing `poi_id` to NULL.
/// SQL copied verbatim from `set_activity::run_title` — `clear_poi` selects between the two branches
/// (clear-poi when the title actually changed AND the row had a poi_id). Caller passes `now`.
#[allow(clippy::too_many_arguments)]
pub async fn update_activity_title(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
    new_title: &str,
    clear_poi: bool,
    now: &str,
) -> Result<(), String> {
    let update_sql = if clear_poi {
        "UPDATE activities SET title = ?1, poi_id = NULL, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 \
           AND session_type = ?6 AND id = ?7"
    } else {
        "UPDATE activities SET title = ?1, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 \
           AND session_type = ?6 AND id = ?7"
    };
    conn.execute(
        update_sql,
        libsql::params![
            new_title.to_string(),
            now.to_string(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string(),
            activity_id.to_string()
        ],
    )
    .await
    .map_err(|e| format!("activities UPDATE title failed: {e}"))?;
    Ok(())
}

/// DELETE one `activities` row (by `(plan, dest, day, session, id)`) then DELETE its
/// `activity_tags` child rows (by `activity_id`), in that order. SQL copied verbatim from
/// `set_activity::run_delete`.
pub async fn delete_activity(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM activities \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
           AND session_type = ?4 AND id = ?5",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string(),
            activity_id.to_string()
        ],
    )
    .await
    .map_err(|e| format!("activities DELETE failed: {e}"))?;
    // Clean up any tag child rows for the removed activity.
    conn.execute(
        "DELETE FROM activity_tags WHERE activity_id = ?1",
        libsql::params![activity_id.to_string()],
    )
    .await
    .map_err(|e| format!("activity_tags DELETE failed: {e}"))?;
    Ok(())
}

/// Move an activity to another `(day, session)` by re-keying its `day_number`/`session_type`/
/// `sort_order` — every other column (id, poi_id, booking_*, updated_at) is preserved untouched
/// (this UPDATE deliberately does NOT bump `updated_at`). SQL copied verbatim from
/// `set_activity::run_move`.
#[allow(clippy::too_many_arguments)]
pub async fn move_activity(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    activity_id: &str,
    to_day: i64,
    to_session: &str,
    new_sort: i64,
) -> Result<(), String> {
    // Re-key the row by id; every other column (poi_id, booking_*, tags via the
    // separate activity_tags table keyed on activity_id) is preserved untouched.
    conn.execute(
        "UPDATE activities \
         SET day_number = ?1, session_type = ?2, sort_order = ?3 \
         WHERE plan_id = ?4 AND destination = ?5 AND id = ?6",
        libsql::params![
            to_day,
            to_session.to_string(),
            new_sort,
            plan_id.to_string(),
            destination.to_string(),
            activity_id.to_string()
        ],
    )
    .await
    .map_err(|e| format!("activities move UPDATE failed: {e}"))?;
    Ok(())
}

/// Shift `sort_order` up by one for every activity in a `(plan, dest, day, session)` positioned
/// AFTER `anchor_sort_order` — opens the `anchor_sort_order + 1` gap for an `add-activity --after`
/// insert. SQL copied verbatim from `set_activity::run_add`.
pub async fn shift_activities_after(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    anchor_sort_order: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE activities SET sort_order = sort_order + 1 \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
           AND session_type = ?4 AND sort_order > ?5",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string(),
            anchor_sort_order
        ],
    )
    .await
    .map_err(|e| format!("activities sort_order shift failed: {e}"))?;
    Ok(())
}

/// INSERT a brand-new `activities` row. SQL copied verbatim from `set_activity::run_add`
/// (`booking_required` is hard-coded to 0 in the SQL; `is_fixed_time` is bound as 1/0 by the
/// caller). Caller passes `now` for `updated_at`.
#[allow(clippy::too_many_arguments)]
pub async fn insert_activity(
    conn: &Connection,
    activity_id: &str,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    sort_order: i64,
    title: &str,
    area: Option<String>,
    nearest_station: Option<String>,
    duration_min: Option<i64>,
    start_time: Option<String>,
    end_time: Option<String>,
    is_fixed_time: bool,
    priority: &str,
    notes: Option<String>,
    source: &str,
    now: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO activities \
            (id, plan_id, destination, day_number, session_type, sort_order, \
             title, area, nearest_station, duration_min, start_time, end_time, \
             is_fixed_time, priority, notes, source, booking_required, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 0, ?17)",
        libsql::params![
            activity_id.to_string(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string(),
            sort_order,
            title.to_string(),
            area,
            nearest_station,
            duration_min,
            start_time,
            end_time,
            if is_fixed_time { 1_i64 } else { 0_i64 },
            priority.to_string(),
            notes,
            source.to_string(),
            now.to_string()
        ],
    )
    .await
    .map_err(|e| format!("activities INSERT failed: {e}"))?;
    Ok(())
}

/// UPDATE one activity's `sort_order` (bumping `updated_at`) for a `(plan, dest, day, session, id)`
/// row — the per-id primitive `reorder-activities` calls once per id in the requested order. SQL
/// copied verbatim from `set_activity::run_reorder`. Caller passes `now`.
#[allow(clippy::too_many_arguments)]
pub async fn update_activity_sort_order(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
    sort_order: i64,
    now: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE activities SET sort_order = ?1, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 \
           AND session_type = ?6 AND id = ?7",
        libsql::params![
            sort_order,
            now.to_string(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string(),
            activity_id.to_string()
        ],
    )
    .await
    .map_err(|e| format!("activities reorder UPDATE failed: {e}"))?;
    Ok(())
}

/// MAX(sort_order)+1 for a `(plan, dest, day, session)` — the next append slot (returns 0 for an
/// empty session, via `COALESCE(MAX(...), -1) + 1`). SQL copied verbatim from
/// `set_activity::next_activity_sort_order`.
pub async fn next_activity_sort_order(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities MAX(sort_order) query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities MAX(sort_order) row read failed: {e}"))?
    {
        let m: i64 = row
            .get(0)
            .map_err(|e| format!("activities MAX(sort_order) col read failed: {e}"))?;
        return Ok(m + 1);
    }
    Ok(0)
}

// ─────────────────────────────────────────────────────────────────
// set-activity-poi domain read + write on the `destination_pois` (validation)
// and `activities` (poi_id link) tables. SQL copied verbatim from
// `set_activity_poi.rs`; caller passes `now` for `updated_at`. The audit triad
// + event emission + activity-resolution stay in `travel-cli` — this module
// never touches them.
// ─────────────────────────────────────────────────────────────────

/// True iff `poi_id` exists in `destination_pois` for `slug` (`destination`).
/// A pure existence SELECT — SQL copied verbatim from `set_activity_poi::assert_poi_exists`;
/// the caller raises the fail-loud error when this returns `false`.
pub async fn poi_exists(
    conn: &Connection,
    destination: &str,
    poi_id: &str,
) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM destination_pois WHERE slug = ?1 AND poi_id = ?2",
            libsql::params![destination.to_string(), poi_id.to_string()],
        )
        .await
        .map_err(|e| format!("destination_pois existence query failed: {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("destination_pois existence row read failed: {e}"))?
        .is_some())
}

/// UPDATE one activity's `poi_id` (bumping `updated_at`) for a
/// `(plan, dest, day, session, id)` row. Returns the rows_affected so the caller
/// keeps its `== 1` fail-loud check. SQL copied verbatim from the inline block in
/// `set_activity_poi::execute`. Caller passes `now`.
#[allow(clippy::too_many_arguments)]
pub async fn set_activity_poi(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
    poi_id: &str,
    now: &str,
) -> Result<u64, String> {
    conn.execute(
        "UPDATE activities SET poi_id = ?1, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 \
           AND session_type = ?6 AND id = ?7",
        libsql::params![
            poi_id.to_string(),
            now.to_string(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string(),
            activity_id.to_string()
        ],
    )
    .await
    .map_err(|e| format!("activities UPDATE poi_id failed: {e}"))
}

fn push_text_filter(sql: &mut String, binds: &mut Vec<libsql::Value>, col: &str, val: &str) {
    binds.push(libsql::Value::Text(val.to_string()));
    sql.push_str(&format!(" AND {col} = ?{}", binds.len()));
}

fn push_i64_filter(sql: &mut String, binds: &mut Vec<libsql::Value>, col: &str, val: i64) {
    binds.push(libsql::Value::Integer(val));
    sql.push_str(&format!(" AND {col} = ?{}", binds.len()));
}

/// Flip `ai_recommended` activities to `confirmed`, scoped by optional day/session.
pub async fn confirm_activities(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: Option<i64>,
    session: Option<&str>,
    now: &str,
) -> Result<u64, String> {
    let mut sql =
        String::from("UPDATE activities SET source = 'confirmed', updated_at = ?1 WHERE 1=1");
    let mut binds = vec![libsql::Value::Text(now.to_string())];
    push_text_filter(&mut sql, &mut binds, "plan_id", plan_id);
    push_text_filter(&mut sql, &mut binds, "destination", destination);
    sql.push_str(" AND source = 'ai_recommended'");
    if let Some(day) = day {
        push_i64_filter(&mut sql, &mut binds, "day_number", day);
    }
    if let Some(session) = session {
        push_text_filter(&mut sql, &mut binds, "session_type", session);
    }
    conn.execute(&sql, binds)
        .await
        .map_err(|e| format!("activities confirm recommendations failed: {e}"))
}

/// Flip `ai_recommended` session_meals to `confirmed`, scoped by optional day/session.
pub async fn confirm_meals(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: Option<i64>,
    session: Option<&str>,
    now: &str,
) -> Result<u64, String> {
    let mut sql =
        String::from("UPDATE session_meals SET source = 'confirmed', updated_at = ?1 WHERE 1=1");
    let mut binds = vec![libsql::Value::Text(now.to_string())];
    push_text_filter(&mut sql, &mut binds, "plan_id", plan_id);
    push_text_filter(&mut sql, &mut binds, "destination", destination);
    sql.push_str(" AND source = 'ai_recommended'");
    if let Some(day) = day {
        push_i64_filter(&mut sql, &mut binds, "day_number", day);
    }
    if let Some(session) = session {
        push_text_filter(&mut sql, &mut binds, "session_type", session);
    }
    conn.execute(&sql, binds)
        .await
        .map_err(|e| format!("session_meals confirm recommendations failed: {e}"))
}

// ─────────────────────────────────────────────────────────────────
// populate-itinerary domain writes on the `days`, `timesofday`, `activities`,
// and `activity_tags` tables. SQL copied verbatim from `populate_itinerary.rs`;
// caller passes `now` for `updated_at` (fresh `now_db_datetime()` per write,
// matching the prior inline timing). NOTE: populate's `activities` INSERT and
// `next_*_sort_order` SELECT have a DISTINCT column set / SQL text from the
// set-activity family above — they are their own functions, not shared. The
// audit triad + event emission + allocation/pacing stay in `travel-cli` —
// this module never touches them.
// ─────────────────────────────────────────────────────────────────

/// UPDATE a `days.theme` (bumping `updated_at`) for a `(plan, dest, day)` row —
/// populate-itinerary's day-theme setter (sets `updated_at`, unlike swap-days'
/// theme swap). SQL copied verbatim from `populate_itinerary::set_day_theme`.
/// Caller passes `now`.
pub async fn set_day_theme_populate(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    theme: &str,
    now: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET theme = ?1, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5",
        libsql::params![
            theme.to_string(),
            now.to_string(),
            plan_id.to_string(),
            destination.to_string(),
            day
        ],
    )
    .await
    .map_err(|e| format!("days theme UPDATE failed: {e}"))?;
    Ok(())
}

/// UPDATE a `timesofday.focus` (bumping `updated_at`) for a
/// `(plan, dest, day, session)` row — populate-itinerary's session-focus setter.
/// SQL copied verbatim from `populate_itinerary::set_session_focus`. Caller
/// passes `now`.
#[allow(clippy::too_many_arguments)]
pub async fn set_session_focus_populate(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    focus: &str,
    now: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE timesofday SET focus = ?1, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 AND session_type = ?6",
        libsql::params![
            focus.to_string(),
            now.to_string(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string()
        ],
    )
    .await
    .map_err(|e| format!("timesofday focus UPDATE failed: {e}"))?;
    Ok(())
}

/// MAX(sort_order)+1 for a `(plan, dest, day, session)` — populate-itinerary's
/// next append slot (returns 0 for an empty session, via
/// `COALESCE(MAX(...), -1) + 1`). SQL copied verbatim from
/// `populate_itinerary::next_sort_order` — note this SELECT has NO `AS m` alias
/// (distinct SQL text from [`next_activity_sort_order`] above), so it is its own
/// function rather than a reuse.
pub async fn next_sort_order(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities MAX(sort_order) query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities MAX(sort_order) row read failed: {e}"))?
    {
        let m: i64 = row.get(0).unwrap_or(-1);
        return Ok(m + 1);
    }
    Ok(0)
}

/// INSERT a brand-new `activities` row for populate-itinerary. SQL copied
/// verbatim from `populate_itinerary::add_activity` — this is a DISTINCT column
/// set from [`insert_activity`] above: it carries `booking_url` + `cost_estimate`,
/// has NO `start_time`/`end_time`, binds `booking_required` (1/0) as a param, and
/// hard-codes `is_fixed_time` to 0 in the SQL. Caller passes `now` for
/// `updated_at`.
#[allow(clippy::too_many_arguments)]
pub async fn insert_populated_activity(
    conn: &Connection,
    activity_id: &str,
    plan_id: &str,
    destination: &str,
    poi_id: &str,
    day: i64,
    session: &str,
    sort_order: i64,
    title: &str,
    area: &str,
    nearest_station: Option<&str>,
    duration_min: Option<i64>,
    booking_required: bool,
    booking_url: Option<&str>,
    cost_estimate: Option<i64>,
    notes: Option<&str>,
    priority: &str,
    source: &str,
    now: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO activities \
            (id, plan_id, destination, poi_id, day_number, session_type, sort_order, title, area, \
             nearest_station, duration_min, booking_required, booking_url, cost_estimate, \
             notes, priority, source, is_fixed_time, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, 0, ?18)",
        libsql::params![
            activity_id.to_string(),
            plan_id.to_string(),
            destination.to_string(),
            poi_id.to_string(),
            day,
            session.to_string(),
            sort_order,
            title.to_string(),
            area.to_string(),
            nearest_station.map(|s| s.to_string()),
            duration_min,
            if booking_required { 1i64 } else { 0i64 },
            booking_url.map(|s| s.to_string()),
            cost_estimate,
            notes.map(|s| s.to_string()),
            priority.to_string(),
            source.to_string(),
            now.to_string()
        ],
    )
    .await
    .map_err(|e| format!("activities INSERT failed: {e}"))?;
    Ok(())
}

/// `INSERT OR IGNORE` one `activity_tags` child row (2 columns: `activity_id`,
/// `tag` — NO `updated_at`). SQL copied verbatim from the inline tag loop in
/// `populate_itinerary::add_activity`.
pub async fn insert_activity_tag_ignore(
    conn: &Connection,
    activity_id: &str,
    tag: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR IGNORE INTO activity_tags (activity_id, tag) VALUES (?1, ?2)",
        libsql::params![activity_id.to_string(), tag.to_string()],
    )
    .await
    .map_err(|e| format!("activity_tags INSERT failed: {e}"))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// swap-days domain writes on the `days` (theme/theme_zh only) and the four
// session-scoped tables (day_number re-point). SQL copied verbatim from
// `swap_days.rs`. NOTE: [`swap_day_theme`] writes theme/theme_zh ONLY with NO
// `updated_at` column (distinct from [`set_day_theme_populate`] above) — the
// observable `updated_at` change comes ONLY from the separate [`touch_day`] the
// caller invokes on both days. The audit triad stays in `cascade::common`.
// ─────────────────────────────────────────────────────────────────

/// UPDATE a `days.theme` + `days.theme_zh` (2 columns, NO `updated_at`) for a
/// `(plan, dest, day)` row — swap-days' theme swapper. SQL copied verbatim from
/// `swap_days::set_day_theme`. Do NOT confuse with [`set_day_theme_populate`]
/// (which DOES bump `updated_at`): swap-days deliberately leaves `updated_at`
/// untouched here, relying on the separate [`touch_day`] calls.
pub async fn swap_day_theme(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    theme: &Option<String>,
    theme_zh: &Option<String>,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET theme = ?1, theme_zh = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5",
        libsql::params![
            theme.clone(),
            theme_zh.clone(),
            plan_id.to_string(),
            destination.to_string(),
            day
        ],
    )
    .await
    .map_err(|e| format!("days theme UPDATE failed: {e}"))?;
    Ok(())
}

/// Swap the `day_number` ownership of every session-scoped table between
/// `day_a` and `day_b`, using the out-of-range TMP day_number dance to avoid PK
/// collisions during the flip. Owns the fixed `SESSION_TABLES` list, the
/// `TMP = -999_999` constant, and the 3-step (a→TMP, b→a, TMP→b) sequence per
/// table — all copied verbatim from `swap_days::swap_content`'s session-repoint
/// loop. The `{table}` values are fixed literals (never user input).
pub async fn swap_session_day_numbers(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day_a: i64,
    day_b: i64,
) -> Result<(), String> {
    const SESSION_TABLES: &[&str] = &[
        "timesofday",
        "activities",
        "session_meals",
        "session_activities_zh",
    ];
    const TMP: i64 = -999_999;
    for table in SESSION_TABLES {
        repoint_day_number(conn, table, plan_id, destination, day_a, TMP).await?;
        repoint_day_number(conn, table, plan_id, destination, day_b, day_a).await?;
        repoint_day_number(conn, table, plan_id, destination, TMP, day_b).await?;
    }
    Ok(())
}

/// Re-point `day_number` from `from_day` to `to_day` for a `(plan, dest)` in one
/// session-scoped `table`. SQL copied verbatim from `swap_days::repoint`; the
/// `{table}` is a fixed `SESSION_TABLES` literal (never user input).
async fn repoint_day_number(
    conn: &Connection,
    table: &str,
    plan_id: &str,
    destination: &str,
    from_day: i64,
    to_day: i64,
) -> Result<(), String> {
    let sql = format!(
        "UPDATE {table} SET day_number = ?1 \
         WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4"
    );
    conn.execute(
        &sql,
        libsql::params![to_day, plan_id.to_string(), destination.to_string(), from_day],
    )
    .await
    .map_err(|e| format!("{table} re-point day_number failed: {e}"))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// set_day_weather domain write (migrated from weather.rs::write_day_weather).
// The LAST inline-SQL domain write. SQL is verbatim; weather_source_id='open_meteo'
// is a SQL LITERAL (not bound); updated_at binds the caller-supplied now_db;
// weather_sourced_at binds a param. The audit triad (events + record_operation)
// and the open-meteo fetch remain untouched in weather.rs.
#[allow(clippy::too_many_arguments)]
pub async fn set_day_weather(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day_number: i64,
    weather_label: &str,
    temp_low_c: f64,
    temp_high_c: f64,
    feels_like_low_c: Option<f64>,
    feels_like_high_c: Option<f64>,
    precipitation_pct: Option<f64>,
    weather_code: i64,
    sourced_at: &str,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET \
            weather_label = ?1, temp_low_c = ?2, temp_high_c = ?3, \
            feels_like_low_c = ?4, feels_like_high_c = ?5, precipitation_pct = ?6, \
            weather_code = ?7, weather_source_id = 'open_meteo', weather_sourced_at = ?8, \
            updated_at = ?9 \
         WHERE plan_id = ?10 AND destination = ?11 AND day_number = ?12",
        libsql::params![
            weather_label.to_string(),
            temp_low_c,
            temp_high_c,
            feels_like_low_c,
            feels_like_high_c,
            precipitation_pct,
            weather_code,
            sourced_at.to_string(),
            now_db.to_string(),
            plan_id.to_string(),
            destination.to_string(),
            day_number
        ],
    )
    .await
    .map_err(|e| format!("days weather UPDATE failed: {e}"))?;
    Ok(())
}
