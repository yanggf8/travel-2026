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

use crate::repo::records::{ensure_unrecorded, RecordScope};
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
    // Trip-record guard (spec X3): scaffold --force wipes the whole destination —
    // refuse while ANY row (or session summary) of it is recorded.
    ensure_unrecorded(conn, plan_id, destination, RecordScope::Destination).await?;
    // Stale child rows first (FK-free, so order is just for cleanliness).
    conn.execute(
        "DELETE FROM activity_tags WHERE activity_id IN \
            (SELECT id FROM activities WHERE plan_id = ?1 AND destination = ?2)",
        libsql::params![plan_id.to_string(), destination.to_string()],
    )
    .await
    .map_err(|e| format!("activity_tags delete failed: {e}"))?;
    // Day-N map legends carry no destination column: scope them by the day numbers this
    // destination is about to lose (must run before the `days` delete). Plan-level legends stay.
    conn.execute(
        "DELETE FROM map_legend_stops WHERE plan_id = ?1 AND map_key LIKE 'day-%' \
           AND CAST(substr(map_key, 5) AS INTEGER) IN \
               (SELECT day_number FROM days WHERE plan_id = ?1 AND destination = ?2)",
        libsql::params![plan_id.to_string(), destination.to_string()],
    )
    .await
    .map_err(|e| format!("map_legend_stops delete failed: {e}"))?;
    for tbl in [
        "session_meals",
        "session_activities_zh",
        "activities",
        "timesofday",
        "day_route_segments",
        "day_landmarks",
        "day_weather_sessions",
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

/// DELETE every `session_meals` row of a `(plan, dest, day, session)`; returns the number removed.
/// Refuses first when any of those meals carries a trip record (spec X3).
pub async fn delete_session_meals(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
) -> Result<u64, String> {
    ensure_unrecorded(
        conn,
        plan_id,
        destination,
        RecordScope::MealsOfSession { day, session },
    )
    .await?;
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
    .map_err(|e| format!("session_meals DELETE failed: {e}"))
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
    // Trip-record guard (spec X3): set-meals replaces the whole session — refuse
    // while any current meal of the session is recorded.
    ensure_unrecorded(
        conn,
        plan_id,
        destination,
        RecordScope::MealsOfSession { day, session },
    )
    .await?;
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
    // Column whitelist (trip-record Task 5 review finding): the `{field}` name is
    // interpolated into the SQL, so it must be one of these fixed names — never
    // a caller-supplied free string. Mirrors `update_tod_field`'s guard above.
    let column = match field {
        "start_time" => "start_time",
        "end_time" => "end_time",
        "is_fixed_time" => "is_fixed_time",
        "booking_status" => "booking_status",
        "booking_ref" => "booking_ref",
        "book_by" => "book_by",
        "booking_required" => "booking_required",
        "notes" => "notes",
        _ => return Err(format!("unsupported activities field: {field}")),
    };
    let sql = format!(
        "UPDATE activities SET {column} = ?1, updated_at = ?2 \
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

/// Column whitelist for [`update_activity_field`]: the `{field}` interpolated into
/// the SQL must be one of these fixed names (the audit finding behind Task 5 —
/// the caller's field name used to reach the SQL unvalidated).
#[cfg(test)]
mod update_activity_field_whitelist_tests {
    use super::*;

    async fn mem_conn() -> Connection {
        libsql::Builder::new_local(":memory:")
            .build()
            .await
            .expect("in-memory db")
            .connect()
            .expect("connect")
    }

    async fn seed_activity(conn: &Connection) {
        conn.execute(
            "CREATE TABLE activities (id TEXT PRIMARY KEY, plan_id TEXT NOT NULL, \
             destination TEXT NOT NULL, day_number INTEGER NOT NULL, session_type TEXT NOT NULL, \
             sort_order INTEGER NOT NULL DEFAULT 0, title TEXT NOT NULL, notes TEXT, \
             start_time TEXT, end_time TEXT, is_fixed_time INTEGER, booking_status TEXT, \
             booking_ref TEXT, book_by TEXT, booking_required INTEGER, \
             updated_at DATETIME DEFAULT CURRENT_TIMESTAMP)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) \
             VALUES ('a1', 'p', 'd', 1, 'noon', 0, '東寺')",
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn update_activity_field_rejects_unknown_column() {
        let conn = mem_conn().await;
        seed_activity(&conn).await;
        // An attacker-typo'd / invented field must be refused BEFORE any SQL runs.
        let err = update_activity_field(
            &conn, "p", "d", 1, "noon", "a1", "haxor; DROP TABLE activities", None, "2026-01-01 00:00:00",
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("unsupported activities field"),
            "must reject unknown fields: {err}"
        );
        // The row survives.
        let mut rows = conn
            .query("SELECT COUNT(*) FROM activities", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 1);
    }

    #[tokio::test]
    async fn update_activity_field_accepts_whitelisted_notes() {
        let conn = mem_conn().await;
        seed_activity(&conn).await;
        update_activity_field(
            &conn, "p", "d", 1, "noon", "a1", "notes", Some("新備註".to_string()), "2026-01-01 00:00:00",
        )
        .await
        .expect("notes is whitelisted");
        let mut rows = conn
            .query("SELECT notes FROM activities WHERE id = 'a1'", ())
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "新備註");
    }
}

/// APPEND one `session_meals` row at `MAX(sort_order)+1` (the `add-meal` path —
/// append, never replace). The MAX+1 read MUST run inside the caller's
/// transaction so a concurrent append can't take the same slot. Returns the
/// assigned `sort_order`. Caller passes `now` for `updated_at`.
#[allow(clippy::too_many_arguments)]
pub async fn append_session_meal(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    meal: &str,
    source: &str,
    now: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM session_meals \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("session_meals MAX(sort_order) query failed: {e}"))?;
    let row = rows
        .next()
        .await
        .map_err(|e| format!("session_meals MAX(sort_order) row read failed: {e}"))?
        .ok_or("session_meals MAX(sort_order) returned no row")?;
    let max_so: i64 = row
        .get(0)
        .map_err(|e| format!("session_meals MAX(sort_order) col read failed: {e}"))?;
    let sort_order: i64 = max_so + 1;
    conn.execute(
        "INSERT INTO session_meals \
            (plan_id, destination, day_number, session_type, sort_order, meal, source, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string(),
            sort_order,
            meal.to_string(),
            source.to_string(),
            now.to_string()
        ],
    )
    .await
    .map_err(|e| format!("session_meals APPEND failed: {e}"))?;
    Ok(sort_order)
}

/// UPDATE one meal's text IN PLACE (the `set-meal` path — keyed by
/// day/session/sort_order, so the row's identity/slot never changes). Fails
/// loud when the slot does not exist. Caller passes `now` for `updated_at`.
#[allow(clippy::too_many_arguments)]
pub async fn update_session_meal_text(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    sort_order: i64,
    meal: &str,
    now: &str,
) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE session_meals SET meal = ?1, updated_at = ?2 \
             WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 \
               AND session_type = ?6 AND sort_order = ?7",
            libsql::params![
                meal.to_string(),
                now.to_string(),
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                sort_order
            ],
        )
        .await
        .map_err(|e| format!("session_meals UPDATE failed: {e}"))?;
    if n != 1 {
        return Err(format!(
            "no meal at day {day} {session} sort_order {sort_order} of plan {plan_id} ({destination})"
        ));
    }
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
    require_unrecorded: bool,
    now: &str,
) -> Result<(), String> {
    // `require_unrecorded` makes the trip-record guard atomic with the write: the UPDATE itself
    // re-checks `record_status IS NULL`, so a record-activity landing between the caller's
    // check and this write cannot be bypassed (0 rows -> fail loud, nothing written).
    let guard = if require_unrecorded { " AND record_status IS NULL" } else { "" };
    let update_sql = if clear_poi {
        format!(
            "UPDATE activities SET title = ?1, poi_id = NULL, updated_at = ?2 \
             WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 \
               AND session_type = ?6 AND id = ?7{guard}"
        )
    } else {
        format!(
            "UPDATE activities SET title = ?1, updated_at = ?2 \
             WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 \
               AND session_type = ?6 AND id = ?7{guard}"
        )
    };
    let n = conn.execute(
        &update_sql,
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
    if require_unrecorded && n == 0 {
        return Err(format!(
            "activity {activity_id} now carries a trip record (or moved); refusing to change its \
             title — use `record-activity` to change the note, or `clear-record` first"
        ));
    }
    Ok(())
}

/// The editable columns of one `activities` row (`update-activity`, spec 2026-11-19 §1).
/// `id`, `sort_order`, `booking_*`, `record_*`, `source` and `cost_estimate` are deliberately
/// absent: this command never touches them. `poi_id` is carried read-only so the caller can
/// apply the title-change poi-clear rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityRow {
    pub title: String,
    pub area: Option<String>,
    pub nearest_station: Option<String>,
    pub duration_min: Option<i64>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub is_fixed_time: bool,
    pub priority: String,
    pub notes: Option<String>,
    pub poi_id: Option<String>,
    /// Who asked for this stop (1-16 chars, validated by the CLI); NULL = nobody.
    pub requested_by: Option<String>,
}

/// Partial update for [`ActivityRow`]. Outer `Option` = was the field given; inner `Option`
/// (nullable columns only) = set a value (`Some`) or clear to NULL (`None`).
/// `title`, `is_fixed_time` and `priority` are NOT NULL, so they cannot be cleared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityPatch {
    pub title: Option<String>,
    pub area: Option<Option<String>>,
    pub nearest_station: Option<Option<String>>,
    pub duration_min: Option<Option<i64>>,
    pub start_time: Option<Option<String>>,
    pub end_time: Option<Option<String>>,
    pub is_fixed_time: Option<bool>,
    pub priority: Option<String>,
    pub notes: Option<Option<String>>,
    /// `--append-notes`: merged into the CURRENT stored notes inside the caller's
    /// transaction (see [`ActivityPatch::apply`]); mutually exclusive with `notes`.
    pub append_notes: Option<String>,
    pub requested_by: Option<Option<String>>,
}

impl ActivityPatch {
    /// True when no field was given.
    pub fn is_empty(&self) -> bool {
        *self == ActivityPatch::default()
    }

    /// The row as it would look after the patch (pure; `poi_id` is left as-is).
    /// `Err` only for an empty `--append-notes` text. An append whose lines are
    /// already present leaves `notes` unchanged (= "No change" via the diff).
    pub fn apply(&self, row: &ActivityRow) -> Result<ActivityRow, String> {
        let notes = match &self.append_notes {
            Some(add) => crate::repo::text_merge::append_note(row.notes.as_deref(), add, "\n")?
                .or_else(|| row.notes.clone()),
            None => self.notes.clone().unwrap_or_else(|| row.notes.clone()),
        };
        Ok(ActivityRow {
            title: self.title.clone().unwrap_or_else(|| row.title.clone()),
            area: self.area.clone().unwrap_or_else(|| row.area.clone()),
            nearest_station: self
                .nearest_station
                .clone()
                .unwrap_or_else(|| row.nearest_station.clone()),
            duration_min: self.duration_min.unwrap_or(row.duration_min),
            start_time: self.start_time.clone().unwrap_or_else(|| row.start_time.clone()),
            end_time: self.end_time.clone().unwrap_or_else(|| row.end_time.clone()),
            is_fixed_time: self.is_fixed_time.unwrap_or(row.is_fixed_time),
            priority: self.priority.clone().unwrap_or_else(|| row.priority.clone()),
            notes,
            poi_id: row.poi_id.clone(),
            requested_by: self
                .requested_by
                .clone()
                .unwrap_or_else(|| row.requested_by.clone()),
        })
    }
}

/// Field-by-field differences `(column, from, to)` between two rows, with NULL rendered as
/// `None`. Order is fixed (title, area, nearest_station, duration_min, start_time, end_time,
/// is_fixed_time, priority, notes, poi_id, requested_by). Empty = the patch is a no-op.
pub fn activity_row_diff(
    before: &ActivityRow,
    after: &ActivityRow,
) -> Vec<(&'static str, Option<String>, Option<String>)> {
    let mut out = Vec::new();
    let mut push = |col: &'static str, a: Option<String>, b: Option<String>| {
        if a != b {
            out.push((col, a, b));
        }
    };
    push("title", Some(before.title.clone()), Some(after.title.clone()));
    push("area", before.area.clone(), after.area.clone());
    push(
        "nearest_station",
        before.nearest_station.clone(),
        after.nearest_station.clone(),
    );
    push(
        "duration_min",
        before.duration_min.map(|d| d.to_string()),
        after.duration_min.map(|d| d.to_string()),
    );
    push("start_time", before.start_time.clone(), after.start_time.clone());
    push("end_time", before.end_time.clone(), after.end_time.clone());
    push(
        "is_fixed_time",
        Some(before.is_fixed_time.to_string()),
        Some(after.is_fixed_time.to_string()),
    );
    push("priority", Some(before.priority.clone()), Some(after.priority.clone()));
    push("notes", before.notes.clone(), after.notes.clone());
    push("poi_id", before.poi_id.clone(), after.poi_id.clone());
    push(
        "requested_by",
        before.requested_by.clone(),
        after.requested_by.clone(),
    );
    out
}

/// Read the editable columns of one activity, pinned to the (day, session) it was matched in.
pub async fn read_activity_row(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
) -> Result<Option<ActivityRow>, String> {
    let mut rows = conn
        .query(
            "SELECT title, area, nearest_station, duration_min, start_time, end_time, \
                    is_fixed_time, priority, notes, poi_id, requested_by \
             FROM activities WHERE plan_id = ?1 AND destination = ?2 AND id = ?3 \
               AND day_number = ?4 AND session_type = ?5",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                activity_id.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities row read failed: {e}"))?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let g = |e: libsql::Error| e.to_string();
    Ok(Some(ActivityRow {
        title: row.get::<String>(0).map_err(g)?,
        area: row.get::<Option<String>>(1).map_err(g)?,
        nearest_station: row.get::<Option<String>>(2).map_err(g)?,
        duration_min: row.get::<Option<i64>>(3).map_err(g)?,
        start_time: row.get::<Option<String>>(4).map_err(g)?,
        end_time: row.get::<Option<String>>(5).map_err(g)?,
        is_fixed_time: row.get::<i64>(6).map_err(g)? != 0,
        priority: row.get::<String>(7).map_err(g)?,
        notes: row.get::<Option<String>>(8).map_err(g)?,
        poi_id: row.get::<Option<String>>(9).map_err(g)?,
        requested_by: row.get::<Option<String>>(10).map_err(g)?,
    }))
}

/// Write the merged `row` back with ONE UPDATE (fixed column names, every value bound,
/// `poi_id` included so a title-change poi clear is part of the same write), bumping
/// `updated_at`. Pinned to `(day, session, id)`: if the row moved since it was matched,
/// 0 rows change and this fails loud instead of updating under a stale position.
#[allow(clippy::too_many_arguments)]
pub async fn update_activity_fields(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
    row: &ActivityRow,
    now: &str,
) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE activities SET title = ?1, area = ?2, nearest_station = ?3, duration_min = ?4, \
               start_time = ?5, end_time = ?6, is_fixed_time = ?7, priority = ?8, notes = ?9, \
               poi_id = ?10, updated_at = ?11, requested_by = ?12 \
             WHERE plan_id = ?13 AND destination = ?14 AND day_number = ?15 \
               AND session_type = ?16 AND id = ?17",
            libsql::params![
                row.title.clone(),
                row.area.clone(),
                row.nearest_station.clone(),
                row.duration_min,
                row.start_time.clone(),
                row.end_time.clone(),
                row.is_fixed_time as i64,
                row.priority.clone(),
                row.notes.clone(),
                row.poi_id.clone(),
                now.to_string(),
                row.requested_by.clone(),
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                activity_id.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities UPDATE failed: {e}"))?;
    if n == 0 {
        return Err(format!(
            "activity {activity_id} not found in D{day}/{session} of plan {plan_id} ({destination}) \
             (moved or deleted meanwhile?)"
        ));
    }
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
    // Trip-record guard (spec X3): a recorded activity is history — refuse.
    ensure_unrecorded(
        conn,
        plan_id,
        destination,
        RecordScope::Activity { id: activity_id },
    )
    .await?;
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
    // Trip-record guard (spec X4): a recorded activity may move WITHIN its day
    // (re-sessioning keeps the record with its day), but never ACROSS days —
    // the record belongs to the day it happened on.
    let current_day: i64 = {
        let mut rows = conn
            .query(
                "SELECT day_number FROM activities \
                 WHERE plan_id = ?1 AND destination = ?2 AND id = ?3",
                libsql::params![
                    plan_id.to_string(),
                    destination.to_string(),
                    activity_id.to_string()
                ],
            )
            .await
            .map_err(|e| format!("activities day read failed: {e}"))?;
        let row = rows
            .next()
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                format!("activity {activity_id} not found in plan {plan_id} ({destination})")
            })?;
        row.get(0).map_err(|e| format!("activities day col read failed: {e}"))?
    };
    if current_day != to_day {
        ensure_unrecorded(
            conn,
            plan_id,
            destination,
            RecordScope::Activity { id: activity_id },
        )
        .await?;
    }
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
    requested_by: Option<String>,
    geo: Option<ActivityGeo>,
) -> Result<(), String> {
    let (lat, lon, geo_source) = match geo {
        Some(g) => (Some(g.lat), Some(g.lon), g.source),
        None => (None, None, None),
    };
    conn.execute(
        "INSERT INTO activities \
            (id, plan_id, destination, day_number, session_type, sort_order, \
             title, area, nearest_station, duration_min, start_time, end_time, \
             is_fixed_time, priority, notes, source, booking_required, updated_at, \
             requested_by, lat, lon, geo_source) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 0, ?17, \
                 ?18, ?19, ?20, ?21)",
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
            now.to_string(),
            requested_by,
            lat,
            lon,
            geo_source
        ],
    )
    .await
    .map_err(|e| format!("activities INSERT failed: {e}"))?;
    Ok(())
}

/// A map pin carried by an activity row itself (spec 2026-11-20 §2.3).
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityGeo {
    pub lat: f64,
    pub lon: f64,
    /// Free-text provenance (e.g. "OSM node 123"); no constraint.
    pub source: Option<String>,
}

/// Read one activity's pin (None = no coordinates), pinned to its `(day, session)`.
/// `Err` when the activity is not there.
pub async fn read_activity_geo(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
) -> Result<Option<ActivityGeo>, String> {
    let mut rows = conn
        .query(
            "SELECT lat, lon, geo_source FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND id = ?3 \
               AND day_number = ?4 AND session_type = ?5",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                activity_id.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities geo read failed: {e}"))?;
    let row = rows
        .next()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("activity {activity_id} not found in D{day}/{session}"))?;
    let g = |e: libsql::Error| e.to_string();
    let lat = row.get::<Option<f64>>(0).map_err(g)?;
    let lon = row.get::<Option<f64>>(1).map_err(g)?;
    let source = row.get::<Option<String>>(2).map_err(g)?;
    Ok(match (lat, lon) {
        (Some(lat), Some(lon)) => Some(ActivityGeo { lat, lon, source }),
        _ => None,
    })
}

/// Set (`Some`) or clear (`None`) one activity's pin with ONE UPDATE (fixed column
/// names, every value bound). Does not touch `updated_at` — a pin is map metadata.
/// Pinned to `(day, session, id)`: 0 rows changed fails loud.
pub async fn set_activity_geocode(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    activity_id: &str,
    geo: Option<&ActivityGeo>,
) -> Result<(), String> {
    let (lat, lon, source) = match geo {
        Some(g) => (Some(g.lat), Some(g.lon), g.source.clone()),
        None => (None, None, None),
    };
    let n = conn
        .execute(
            "UPDATE activities SET lat = ?1, lon = ?2, geo_source = ?3 \
             WHERE plan_id = ?4 AND destination = ?5 AND day_number = ?6 \
               AND session_type = ?7 AND id = ?8",
            libsql::params![
                lat,
                lon,
                source,
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                activity_id.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities geocode UPDATE failed: {e}"))?;
    if n == 0 {
        return Err(format!(
            "activity {activity_id} not found in D{day}/{session} of plan {plan_id} ({destination})"
        ));
    }
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

#[derive(Debug, Clone)]
pub struct RecommendedActivity {
    pub day_number: i64,
    pub session_type: String,
    pub sort_order: i64,
    pub title: String,
    pub poi_id: Option<String>,
}

pub async fn list_recommended_activities(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: Option<i64>,
    session: Option<&str>,
) -> Result<Vec<RecommendedActivity>, String> {
    let mut sql = String::from(
        "SELECT day_number, session_type, sort_order, title, poi_id FROM activities WHERE 1=1",
    );
    let mut binds: Vec<libsql::Value> = Vec::new();
    push_text_filter(&mut sql, &mut binds, "plan_id", plan_id);
    push_text_filter(&mut sql, &mut binds, "destination", destination);
    sql.push_str(" AND source = 'ai_recommended'");
    if let Some(day) = day {
        push_i64_filter(&mut sql, &mut binds, "day_number", day);
    }
    if let Some(session) = session {
        push_text_filter(&mut sql, &mut binds, "session_type", session);
    }
    sql.push_str(" ORDER BY day_number, session_type, sort_order");
    let mut rows = conn
        .query(&sql, binds)
        .await
        .map_err(|e| format!("list_recommended_activities failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(RecommendedActivity {
            day_number: row.get(0).map_err(|e| e.to_string())?,
            session_type: row.get(1).map_err(|e| e.to_string())?,
            sort_order: row.get(2).map_err(|e| e.to_string())?,
            title: row.get(3).map_err(|e| e.to_string())?,
            poi_id: row.get(4).ok(),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct RecommendedMeal {
    pub day_number: i64,
    pub session_type: String,
    pub sort_order: i64,
    pub meal: String,
}

pub async fn list_recommended_meals(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: Option<i64>,
    session: Option<&str>,
) -> Result<Vec<RecommendedMeal>, String> {
    let mut sql = String::from(
        "SELECT day_number, session_type, sort_order, meal FROM session_meals WHERE 1=1",
    );
    let mut binds: Vec<libsql::Value> = Vec::new();
    push_text_filter(&mut sql, &mut binds, "plan_id", plan_id);
    push_text_filter(&mut sql, &mut binds, "destination", destination);
    sql.push_str(" AND source = 'ai_recommended'");
    if let Some(day) = day {
        push_i64_filter(&mut sql, &mut binds, "day_number", day);
    }
    if let Some(session) = session {
        push_text_filter(&mut sql, &mut binds, "session_type", session);
    }
    sql.push_str(" ORDER BY day_number, session_type, sort_order");
    let mut rows = conn
        .query(&sql, binds)
        .await
        .map_err(|e| format!("list_recommended_meals failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(RecommendedMeal {
            day_number: row.get(0).map_err(|e| e.to_string())?,
            session_type: row.get(1).map_err(|e| e.to_string())?,
            sort_order: row.get(2).map_err(|e| e.to_string())?,
            meal: row.get(3).map_err(|e| e.to_string())?,
        });
    }
    Ok(out)
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

/// `days.excursion` for one day (a far-off day trip kept off the sightseeing
/// overview map); `None` when the day row does not exist.
pub async fn day_excursion(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<Option<bool>, String> {
    let mut rows = conn
        .query(
            "SELECT excursion FROM days WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("days excursion query failed: {e}"))?;
    Ok(match rows.next().await.map_err(|e| e.to_string())? {
        Some(r) => Some(r.get::<i64>(0).unwrap_or(0) != 0),
        None => None,
    })
}

/// Set `days.excursion` (no `updated_at` bump — callers touch the day themselves).
pub async fn set_day_excursion(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    on: bool,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET excursion = ?1 WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4",
        libsql::params![on as i64, plan_id.to_string(), destination.to_string(), day],
    )
    .await
    .map_err(|e| format!("days excursion UPDATE failed: {e}"))?;
    Ok(())
}

/// Whether any trip day (a day variant never counts) requires a separate excursion map for this plan.
pub async fn has_plan_excursion(conn: &Connection, plan_id: &str) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT EXISTS(SELECT 1 FROM days WHERE plan_id = ?1 AND excursion = 1 AND variant_of IS NULL)",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan excursion query failed: {e}"))?;
    let row = rows
        .next()
        .await
        .map_err(|e| format!("plan excursion row: {e}"))?
        .ok_or("plan excursion query returned no row")?;
    Ok(row.get::<i64>(0).map_err(|e| format!("plan excursion value: {e}"))? == 1)
}

/// Trip-day numbers flagged `excursion = 1` for a plan+destination, ascending (variants excluded).
pub async fn excursion_days(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<i64>, String> {
    let mut rows = conn
        .query(
            "SELECT day_number FROM days WHERE plan_id = ?1 AND destination = ?2 AND excursion = 1 \
             AND variant_of IS NULL ORDER BY day_number",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("excursion days query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(r.get::<i64>(0).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Swap the `day_number` ownership of every session- and day-scoped content table between
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
    // Trip-record guard (spec X4): swapping re-keys BOTH days' content — refuse
    // while either day holds any record (rows or session summaries).
    ensure_unrecorded(conn, plan_id, destination, RecordScope::Day { day: day_a }).await?;
    ensure_unrecorded(conn, plan_id, destination, RecordScope::Day { day: day_b }).await?;
    const SESSION_TABLES: &[&str] = &[
        "timesofday",
        "activities",
        "session_meals",
        "session_activities_zh",
        // Day-scoped (no session) — a day's route legs and map landmarks travel with its
        // content; leaving them behind strands the old day's route on the swapped day.
        "day_route_segments",
        "day_landmarks",
        // Per-segment via points are keyed by (day_number, sort_order) of their
        // segment: they must move with the segment, or the swapped day inherits the
        // other day's via geometry (spec §5, review H4).
        "day_route_segment_via",
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
// The LAST inline-SQL domain write. SQL is verbatim; weather_source_id is bound
// (?9, the caller's provider id); updated_at binds the caller-supplied now_db;
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
    source_id: &str,
    sourced_at: &str,
    now_db: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET \
            weather_label = ?1, temp_low_c = ?2, temp_high_c = ?3, \
            feels_like_low_c = ?4, feels_like_high_c = ?5, precipitation_pct = ?6, \
            weather_code = ?7, weather_source_id = ?8, weather_sourced_at = ?9, \
            updated_at = ?10 \
         WHERE plan_id = ?11 AND destination = ?12 AND day_number = ?13",
        libsql::params![
            weather_label.to_string(),
            temp_low_c,
            temp_high_c,
            feels_like_low_c,
            feels_like_high_c,
            precipitation_pct,
            weather_code,
            source_id.to_string(),
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

#[cfg(test)]
mod update_activity_fields_tests {
    use super::*;

    fn row() -> ActivityRow {
        ActivityRow {
            title: "鼻頭角".into(),
            area: None,
            nearest_station: Some("龍洞".into()),
            duration_min: Some(60),
            start_time: Some("09:00".into()),
            end_time: None,
            is_fixed_time: false,
            priority: "want".into(),
            notes: Some("n".into()),
            poi_id: Some("poi1".into()),
            requested_by: Some("Ping".into()),
        }
    }

    #[test]
    fn apply_only_changes_given_fields() {
        let r = row();
        let p = ActivityPatch {
            duration_min: Some(Some(45)),
            area: Some(Some("鼻頭".into())),
            ..Default::default()
        };
        let a = p.apply(&r).unwrap();
        assert_eq!(a.duration_min, Some(45));
        assert_eq!(a.area.as_deref(), Some("鼻頭"));
        assert_eq!(a.nearest_station, r.nearest_station);
        assert_eq!(a.start_time, r.start_time);
        assert_eq!(a.poi_id, r.poi_id);
    }

    #[test]
    fn apply_clear_sets_null_and_empty_patch_is_noop() {
        let r = row();
        let p = ActivityPatch {
            start_time: Some(None),
            notes: Some(None),
            ..Default::default()
        };
        let a = p.apply(&r).unwrap();
        assert_eq!(a.start_time, None);
        assert_eq!(a.notes, None);
        assert!(ActivityPatch::default().is_empty());
        assert!(!p.is_empty());
        assert!(activity_row_diff(&r, &ActivityPatch::default().apply(&r).unwrap()).is_empty());
    }

    #[test]
    fn diff_reports_changed_columns_in_fixed_order() {
        let r = row();
        let a = ActivityPatch {
            notes: Some(None),
            duration_min: Some(Some(45)),
            is_fixed_time: Some(true),
            ..Default::default()
        }
        .apply(&r)
        .unwrap();
        let d = activity_row_diff(&r, &a);
        let cols: Vec<&str> = d.iter().map(|(c, _, _)| *c).collect();
        assert_eq!(cols, ["duration_min", "is_fixed_time", "notes"]);
        let mut with_poi = a.clone();
        with_poi.poi_id = None;
        let d2 = activity_row_diff(&r, &with_poi);
        assert_eq!(d2.last().unwrap().0, "poi_id");
        assert_eq!(d2.last().unwrap().1.as_deref(), Some("poi1"));
        assert_eq!(d2.last().unwrap().2, None);
        assert_eq!(d[0].1.as_deref(), Some("60"));
        assert_eq!(d[0].2.as_deref(), Some("45"));
        assert_eq!(d[2].2, None);
    }

    #[tokio::test]
    async fn read_and_update_roundtrip_keeps_untouched_columns() {
        let conn = libsql::Builder::new_local(":memory:")
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap();
        conn.execute(
            "CREATE TABLE activities (id TEXT, plan_id TEXT, destination TEXT, title TEXT, area TEXT, \
             nearest_station TEXT, duration_min INTEGER, start_time TEXT, end_time TEXT, \
             is_fixed_time INTEGER, priority TEXT, notes TEXT, poi_id TEXT, sort_order INTEGER, \
             record_status TEXT, updated_at TEXT, day_number INTEGER, session_type TEXT, \
             requested_by TEXT, lat REAL, lon REAL, geo_source TEXT)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO activities VALUES ('a','p','d','鼻頭角',NULL,'龍洞',60,'09:00',NULL,0,'want','n','poi1',7,'done','old',1,'morning','Ping',NULL,NULL,NULL)",
            (),
        )
        .await
        .unwrap();
        let before = read_activity_row(&conn, "p", "d", 1, "morning", "a").await.unwrap().unwrap();
        assert_eq!(before, row());
        let after = ActivityPatch {
            duration_min: Some(Some(45)),
            start_time: Some(None),
            ..Default::default()
        }
        .apply(&before)
        .unwrap();
        update_activity_fields(&conn, "p", "d", 1, "morning", "a", &after, "now").await.unwrap();
        let got = read_activity_row(&conn, "p", "d", 1, "morning", "a").await.unwrap().unwrap();
        assert_eq!(got.duration_min, Some(45));
        assert_eq!(got.start_time, None);
        assert_eq!(got.poi_id.as_deref(), Some("poi1"));
        // untouched columns outside the editable set
        let mut r = conn.query("SELECT sort_order, record_status, updated_at FROM activities", ()).await.unwrap();
        let x = r.next().await.unwrap().unwrap();
        assert_eq!(x.get::<i64>(0).unwrap(), 7);
        assert_eq!(x.get::<String>(1).unwrap(), "done");
        assert_eq!(x.get::<String>(2).unwrap(), "now");
        // poi_id is part of the same write
        let mut no_poi = got.clone();
        no_poi.poi_id = None;
        update_activity_fields(&conn, "p", "d", 1, "morning", "a", &no_poi, "now2").await.unwrap();
        assert_eq!(read_activity_row(&conn, "p", "d", 1, "morning", "a").await.unwrap().unwrap().poi_id, None);
        // stale position (row moved to another session) or unknown id: fail loud, nothing written
        assert!(update_activity_fields(&conn, "p", "d", 1, "noon", "a", &got, "n").await.is_err());
        assert!(update_activity_fields(&conn, "p", "d", 2, "morning", "a", &got, "n").await.is_err());
        assert!(update_activity_fields(&conn, "p", "d", 1, "morning", "zz", &got, "n").await.is_err());
        assert!(read_activity_row(&conn, "p", "d", 1, "noon", "a").await.unwrap().is_none());
        let after_fail = read_activity_row(&conn, "p", "d", 1, "morning", "a").await.unwrap().unwrap();
        assert_eq!(after_fail.poi_id, None);
        assert_eq!(after_fail.duration_min, Some(45));
    }
}

#[cfg(test)]
mod update_activity_title_guard_tests {
    use super::*;

    #[tokio::test]
    async fn require_unrecorded_makes_the_record_guard_atomic_with_the_write() {
        let conn = libsql::Builder::new_local(":memory:").build().await.unwrap().connect().unwrap();
        conn.execute(
            "CREATE TABLE activities (id TEXT, plan_id TEXT, destination TEXT, day_number INTEGER, \
             session_type TEXT, title TEXT, poi_id TEXT, record_status TEXT, updated_at TEXT)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO activities VALUES ('rec','p','d',1,'noon','東寺','poi','done','x'), \
             ('plain','p','d',1,'noon','鼻頭角','poi',NULL,'x')",
            (),
        )
        .await
        .unwrap();
        // guard on a recorded row: Err and nothing changes (title AND poi_id)
        let e = update_activity_title(&conn, "p", "d", 1, "noon", "rec", "別處", true, true, "n")
            .await
            .unwrap_err();
        assert!(e.contains("record-activity"), "{e}");
        let mut r = conn.query("SELECT title, poi_id FROM activities WHERE id='rec'", ()).await.unwrap();
        let row = r.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "東寺");
        assert_eq!(row.get::<String>(1).unwrap(), "poi");
        // unrecorded row passes; unguarded same-title rewrite of a recorded row still works
        update_activity_title(&conn, "p", "d", 1, "noon", "plain", "野柳", true, true, "n").await.unwrap();
        update_activity_title(&conn, "p", "d", 1, "noon", "rec", "東寺", false, false, "n").await.unwrap();
    }
}
