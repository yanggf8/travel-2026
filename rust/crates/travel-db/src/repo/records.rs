//! Trip-record DAL (docs/superpowers/specs/2026-11-16-trip-record-design.md §5).
//!
//! Three responsibilities, shared by the `record-*` / `clear-record` CLI commands
//! and by every delete/rebuild repo function's overwrite guard:
//!   1. UNIQUE matching — locate one activity / meal by (main-day, session, needle);
//!      0 or >1 matches fail loud with the candidate list. Matching is done in
//!      Rust (`to_lowercase().contains`) so `%` / `_` / quotes in titles are
//!      LITERAL text, never LIKE wildcards. This deliberately does NOT touch
//!      `set_activity::find_activity`'s legacy first-match behavior.
//!   2. MARKING — write `record_status` / `record_note` (activities, session_meals,
//!      day_route_segments) and `record_summary_zh` (timesofday). Legal values are
//!      validated in Rust by [`RecordStatus`] (the DB column has no CHECK). A
//!      same-value rerun returns `false` so the CLI skips the audit triad (no
//!      version bump). `note = None` leaves the note untouched;
//!      `Some(None)` clears it.
//!   3. OVERWRITE GUARD — [`ensure_unrecorded`] refuses any delete/rebuild that
//!      would wash away recorded rows (spec X3) or a written session summary.
//!
//! The audit triad stays in `travel-cli` (`cascade::common`) — this module never
//! touches it.

use libsql::Connection;

/// A row's actual-trip status. NULL (the column default) means unrecorded; the
/// legal values live only here (no DB CHECK, project rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordStatus {
    Done,
    Skipped,
}

impl RecordStatus {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "done" => Ok(RecordStatus::Done),
            "skipped" => Ok(RecordStatus::Skipped),
            other => Err(format!(
                "invalid record status \"{other}\" (legal values: done | skipped)"
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RecordStatus::Done => "done",
            RecordStatus::Skipped => "skipped",
        }
    }
}

/// Which table a record key addresses (used by `clear-record activity|meal|leg`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordTarget {
    Activity,
    Meal,
    Leg,
}

impl RecordTarget {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "activity" => Ok(RecordTarget::Activity),
            "meal" => Ok(RecordTarget::Meal),
            "leg" => Ok(RecordTarget::Leg),
            other => Err(format!(
                "invalid record target \"{other}\" (legal values: activity | meal | leg)"
            )),
        }
    }
}

/// Refuse to operate on a VARIANT day: variant rows are alternative-plan
/// content and can never be recorded (spec §2). `Err` also when the day is
/// missing — fail loud, never a silent empty scope.
async fn require_main_day(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<(), String> {
    match crate::repo::days::variant_parent(conn, plan_id, destination, day).await? {
        None => Ok(()),
        Some(main) => Err(format!(
            "day {day} is a VARIANT of day {main} — variant rows cannot be recorded \
             (record on the main day {main})"
        )),
    }
}

/// One (id, title) candidate for the ambiguity error message.
fn list_candidates(rows: &[(String, String)]) -> String {
    rows.iter()
        .map(|(id, title)| format!("{id}: {title}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Unique activity match within one MAIN day + session (the RECORD path —
/// variant rows are never candidates). See [`match_activity_unique`] for the
/// matching rule.
pub async fn resolve_activity_unique(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    needle: &str,
) -> Result<String, String> {
    require_main_day(conn, plan_id, destination, day).await?;
    match_activity_unique(conn, plan_id, destination, day, session, needle).await
}

/// Unique activity match within one day + session, WITHOUT the record path's
/// main-day restriction — the shared matcher for EDIT commands
/// (set-activity-notes), which must keep working on variant days (variants are
/// alternative plans; only RECORDING is refused there, spec §2). `needle` is
/// either an exact activity id or a case-insensitive LITERAL substring of the
/// title. 0 or >1 matches → `Err` listing the candidates.
pub async fn match_activity_unique(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    needle: &str,
) -> Result<String, String> {
    let mut rows = conn
        .query(
            "SELECT id, title FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4 \
             ORDER BY sort_order, id",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("activities candidates query failed: {e}"))?;
    let mut all: Vec<(String, String)> = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let id: String = row.get(0).map_err(|e| e.to_string())?;
        let title: String = row.get(1).map_err(|e| e.to_string())?;
        all.push((id, title));
    }
    if all.is_empty() {
        return Err(format!(
            "no activities in day {day} {session} of plan {plan_id} ({destination}) \
             — scaffold the itinerary first"
        ));
    }
    // Exact id wins first (ids are unique, so at most one).
    if let Some((id, _)) = all.iter().find(|(id, _)| id == needle) {
        return Ok(id.clone());
    }
    let needle_lc = needle.to_lowercase();
    let hits: Vec<&(String, String)> = all
        .iter()
        .filter(|(_, title)| title.to_lowercase().contains(&needle_lc))
        .collect();
    match hits.as_slice() {
        [] => Err(format!(
            "no activity matches \"{needle}\" in day {day} {session}; candidates: {}",
            list_candidates(&all)
        )),
        [(id, _)] => Ok(id.clone()),
        many => Err(format!(
            "\"{needle}\" matches {} activities in day {day} {session}; pass the exact id \
             or a unique title substring: {}",
            many.len(),
            list_candidates(
                &many.iter().map(|(i, t)| (i.clone(), t.clone())).collect::<Vec<_>>()
            )
        )),
    }
}

/// Unique meal match within one MAIN day + session (the RECORD path). See
/// [`match_meal_unique`] for the matching rule.
pub async fn resolve_meal_unique(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    needle: &str,
) -> Result<i64, String> {
    require_main_day(conn, plan_id, destination, day).await?;
    match_meal_unique(conn, plan_id, destination, day, session, needle).await
}

/// Unique meal match within one day + session, WITHOUT the record path's
/// main-day restriction — the shared matcher for EDIT commands (set-meal).
/// Case-insensitive LITERAL substring of the meal text; returns `sort_order`.
/// Meals have no id column — text matching is the locator (spec §4).
pub async fn match_meal_unique(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    needle: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT sort_order, meal FROM session_meals \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4 \
             ORDER BY sort_order",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("session_meals candidates query failed: {e}"))?;
    let mut all: Vec<(i64, String)> = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let so: i64 = row.get(0).map_err(|e| e.to_string())?;
        let meal: String = row.get(1).map_err(|e| e.to_string())?;
        all.push((so, meal));
    }
    if all.is_empty() {
        return Err(format!(
            "no meals in day {day} {session} of plan {plan_id} ({destination})"
        ));
    }
    let needle_lc = needle.to_lowercase();
    let hits: Vec<&(i64, String)> = all
        .iter()
        .filter(|(_, meal)| meal.to_lowercase().contains(&needle_lc))
        .collect();
    let render = |hits: &[&(i64, String)]| {
        hits.iter()
            .map(|(so, meal)| format!("#{so}: {meal}"))
            .collect::<Vec<_>>()
            .join("; ")
    };
    match hits.as_slice() {
        [] => Err(format!(
            "no meal matches \"{needle}\" in day {day} {session}; candidates: {}",
            all.iter()
                .map(|(so, meal)| format!("#{so}: {meal}"))
                .collect::<Vec<_>>()
                .join("; ")
        )),
        [(so, _)] => Ok(*so),
        many => Err(format!(
            "\"{needle}\" matches {} meals in day {day} {session}; pass a unique substring: {}",
            many.len(),
            render(many)
        )),
    }
}

/// Read one activity's current `(record_status, record_note)` by id.
async fn read_activity_record(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    id: &str,
) -> Result<Option<(Option<String>, Option<String>)>, String> {
    let mut rows = conn
        .query(
            "SELECT record_status, record_note FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND id = ?3",
            vec![
                    libsql::Value::Text(plan_id.to_string()),
                    libsql::Value::Text(destination.to_string()),
                    libsql::Value::Text(id.to_string()),
                ],
        )
        .await
        .map_err(|e| format!("activities record read failed: {e}"))?;
    match rows.next().await.map_err(|e| e.to_string())? {
        Some(row) => Ok(Some((
            row.get::<Option<String>>(0).map_err(|e| e.to_string())?,
            row.get::<Option<String>>(1).map_err(|e| e.to_string())?,
        ))),
        None => Ok(None),
    }
}

/// Mark one activity. `st = None` leaves the status untouched (used by
/// note-only updates); `note = None` leaves the note untouched, `Some(None)`
/// clears it. Returns whether anything changed — a same-value rerun is `false`
/// so the caller skips the audit triad. A real change also bumps
/// `updated_at = datetime('now')` so `check-maps-fresh` sees the map inputs
/// moved (spec §5, Fable#11 / K3#5).
pub async fn set_activity_record(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    id: &str,
    st: Option<RecordStatus>,
    note: Option<Option<&str>>,
) -> Result<bool, String> {
    let Some((cur_status, cur_note)) = read_activity_record(conn, plan_id, destination, id).await?
    else {
        return Err(format!(
            "activity {id} not found in plan {plan_id} ({destination})"
        ));
    };
    let new_status: Option<String> = st
        .map(|s| s.as_str().to_string())
        .or_else(|| cur_status.clone());
    // note: None = leave untouched; Some(None) = clear to NULL; Some(Some(t)) = set.
    let new_note: Option<String> = match note {
        None => cur_note.clone(),
        Some(inner) => inner.map(|s| s.to_string()),
    };
    if new_status == cur_status && new_note == cur_note {
        return Ok(false);
    }
    conn.execute(
        "UPDATE activities SET record_status = ?1, record_note = ?2, updated_at = datetime('now') \
         WHERE plan_id = ?3 AND destination = ?4 AND id = ?5",
        libsql::params![
            new_status,
            new_note,
            plan_id.to_string(),
            destination.to_string(),
            id.to_string()
        ],
    )
    .await
    .map_err(|e| format!("activities record UPDATE failed: {e}"))?;
    Ok(true)
}

/// Mark one meal (keyed by day + session + sort_order). Same semantics as
/// [`set_activity_record`]; bumps `session_meals.updated_at` on change.
pub async fn set_meal_record(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    sort_order: i64,
    st: Option<RecordStatus>,
    note: Option<Option<&str>>,
) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT record_status, record_note FROM session_meals \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
               AND session_type = ?4 AND sort_order = ?5",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                sort_order
            ],
        )
        .await
        .map_err(|e| format!("session_meals record read failed: {e}"))?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Err(format!(
            "no meal at day {day} {session} sort_order {sort_order} of plan {plan_id} ({destination})"
        ));
    };
    let cur_status: Option<String> = row.get(0).map_err(|e| e.to_string())?;
    let cur_note: Option<String> = row.get(1).map_err(|e| e.to_string())?;
    let new_status: Option<String> = st
        .map(|s| s.as_str().to_string())
        .or_else(|| cur_status.clone());
    // note: None = leave untouched; Some(None) = clear to NULL; Some(Some(t)) = set.
    let new_note: Option<String> = match note {
        None => cur_note.clone(),
        Some(inner) => inner.map(|s| s.to_string()),
    };
    if new_status == cur_status && new_note == cur_note {
        return Ok(false);
    }
    conn.execute(
        "UPDATE session_meals SET record_status = ?1, record_note = ?2, updated_at = datetime('now') \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 \
           AND session_type = ?6 AND sort_order = ?7",
        libsql::params![
            new_status,
            new_note,
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string(),
            sort_order
        ],
    )
    .await
    .map_err(|e| format!("session_meals record UPDATE failed: {e}"))?;
    Ok(true)
}

/// Mark one route leg (keyed by day + sort_order — `day_route_segments` has no
/// id and no `updated_at` column, so only the record columns move).
pub async fn set_leg_record(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    sort_order: i64,
    st: Option<RecordStatus>,
    note: Option<Option<&str>>,
) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT record_status, record_note FROM day_route_segments \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND sort_order = ?4",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                sort_order
            ],
        )
        .await
        .map_err(|e| format!("day_route_segments record read failed: {e}"))?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Err(format!(
            "no route segment at day {day} sort_order {sort_order} of plan {plan_id} ({destination})"
        ));
    };
    let cur_status: Option<String> = row.get(0).map_err(|e| e.to_string())?;
    let cur_note: Option<String> = row.get(1).map_err(|e| e.to_string())?;
    let new_status: Option<String> = st
        .map(|s| s.as_str().to_string())
        .or_else(|| cur_status.clone());
    // note: None = leave untouched; Some(None) = clear to NULL; Some(Some(t)) = set.
    let new_note: Option<String> = match note {
        None => cur_note.clone(),
        Some(inner) => inner.map(|s| s.to_string()),
    };
    if new_status == cur_status && new_note == cur_note {
        return Ok(false);
    }
    conn.execute(
        "UPDATE day_route_segments SET record_status = ?1, record_note = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 AND sort_order = ?6",
        libsql::params![
            new_status,
            new_note,
            plan_id.to_string(),
            destination.to_string(),
            day,
            sort_order
        ],
    )
    .await
    .map_err(|e| format!("day_route_segments record UPDATE failed: {e}"))?;
    Ok(true)
}

/// Clear one activity's record (status AND note back to NULL) — the
/// `clear-record activity` path. Returns whether anything was actually set.
pub async fn clear_activity_record(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    id: &str,
) -> Result<bool, String> {
    let Some((cur_status, cur_note)) = read_activity_record(conn, plan_id, destination, id).await?
    else {
        return Err(format!(
            "activity {id} not found in plan {plan_id} ({destination})"
        ));
    };
    if cur_status.is_none() && cur_note.is_none() {
        return Ok(false);
    }
    conn.execute(
        "UPDATE activities SET record_status = NULL, record_note = NULL, updated_at = datetime('now') \
         WHERE plan_id = ?1 AND destination = ?2 AND id = ?3",
        libsql::params![plan_id.to_string(), destination.to_string(), id.to_string()],
    )
    .await
    .map_err(|e| format!("activities record CLEAR failed: {e}"))?;
    Ok(true)
}

/// Clear one meal's record — the `clear-record meal` path.
pub async fn clear_meal_record(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    sort_order: i64,
) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT record_status, record_note FROM session_meals \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
               AND session_type = ?4 AND sort_order = ?5",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string(),
                sort_order
            ],
        )
        .await
        .map_err(|e| format!("session_meals record read failed: {e}"))?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Err(format!(
            "no meal at day {day} {session} sort_order {sort_order} of plan {plan_id} ({destination})"
        ));
    };
    let cur_status: Option<String> = row.get(0).map_err(|e| e.to_string())?;
    let cur_note: Option<String> = row.get(1).map_err(|e| e.to_string())?;
    if cur_status.is_none() && cur_note.is_none() {
        return Ok(false);
    }
    conn.execute(
        "UPDATE session_meals SET record_status = NULL, record_note = NULL, updated_at = datetime('now') \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
           AND session_type = ?4 AND sort_order = ?5",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string(),
            sort_order
        ],
    )
    .await
    .map_err(|e| format!("session_meals record CLEAR failed: {e}"))?;
    Ok(true)
}

/// Clear one leg's record — the `clear-record leg` path.
pub async fn clear_leg_record(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    sort_order: i64,
) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT record_status, record_note FROM day_route_segments \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND sort_order = ?4",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                sort_order
            ],
        )
        .await
        .map_err(|e| format!("day_route_segments record read failed: {e}"))?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Err(format!(
            "no route segment at day {day} sort_order {sort_order} of plan {plan_id} ({destination})"
        ));
    };
    let cur_status: Option<String> = row.get(0).map_err(|e| e.to_string())?;
    let cur_note: Option<String> = row.get(1).map_err(|e| e.to_string())?;
    if cur_status.is_none() && cur_note.is_none() {
        return Ok(false);
    }
    conn.execute(
        "UPDATE day_route_segments SET record_status = NULL, record_note = NULL \
         WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND sort_order = ?4",
        libsql::params![
            plan_id.to_string(),
            destination.to_string(),
            day,
            sort_order
        ],
    )
    .await
    .map_err(|e| format!("day_route_segments record CLEAR failed: {e}"))?;
    Ok(true)
}

/// Write (or, with `None`, clear) one session's `record_summary_zh`. Returns
/// whether the value changed (same text rerun → `false`); bumps
/// `timesofday.updated_at` on change.
pub async fn set_session_summary(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    zh: Option<&str>,
) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT record_summary_zh FROM timesofday \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
            libsql::params![
                plan_id.to_string(),
                destination.to_string(),
                day,
                session.to_string()
            ],
        )
        .await
        .map_err(|e| format!("timesofday summary read failed: {e}"))?;
    let Some(row) = rows.next().await.map_err(|e| e.to_string())? else {
        return Err(format!(
            "no timesofday row for day {day} {session} of plan {plan_id} ({destination})"
        ));
    };
    let cur: Option<String> = row.get(0).map_err(|e| e.to_string())?;
    let new = zh.map(|s| s.to_string());
    if new == cur {
        return Ok(false);
    }
    conn.execute(
        "UPDATE timesofday SET record_summary_zh = ?1, updated_at = datetime('now') \
         WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4 AND session_type = ?5",
        libsql::params![
            new,
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string()
        ],
    )
    .await
    .map_err(|e| format!("timesofday summary UPDATE failed: {e}"))?;
    Ok(true)
}

/// The delete/rebuild scope for [`ensure_unrecorded`]. Narrow scopes cover only
/// the rows they will touch; `Day` (whole-day delete / swap-days) also covers
/// that day's session summaries, and `Destination` (scaffold --force) covers
/// every summary of the destination.
#[derive(Debug)]
pub enum RecordScope<'a> {
    /// One activity row (delete-activity, cross-day move-activity).
    Activity { id: &'a str },
    /// Whole day: main-day row statuses (activities + meals + legs) PLUS that
    /// day's `timesofday.record_summary_zh` (delete_day_content, swap-days).
    Day { day: i64 },
    /// All activities of a day.
    ActivitiesOfDay { day: i64 },
    /// All meals of one session.
    MealsOfSession { day: i64, session: &'a str },
    /// All legs of a day.
    LegsOfDay { day: i64 },
    /// One leg slot (set-route-segment's delete-then-insert).
    LegSlot { day: i64, sort_order: i64 },
    /// The `ai_recommended` legs of a day (derive-routes' rebuild set).
    LegsAiRecommendedOfDay { day: i64 },
    /// Everything of one (plan, destination): rows + session summaries
    /// (scaffold-itinerary --force → replace_skeleton).
    Destination,
}

/// Push `AND record_status IS NOT NULL` rows of `sql` into `out` as
/// `"<label>"` lines. `label` receives (day, session-or-""), the identifying
/// text (title / meal / from→to) and the status.
async fn collect_status_rows(
    conn: &Connection,
    out: &mut Vec<String>,
    sql: &str,
    binds: Vec<libsql::Value>,
    table: &str,
) -> Result<(), String> {
    let mut rows = conn
        .query(sql, binds)
        .await
        .map_err(|e| format!("{table} guard query failed: {e}"))?;
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let day: i64 = row.get(0).map_err(|e| e.to_string())?;
        let session: Option<String> = row.get::<Option<String>>(1).ok().flatten();
        let text: String = row.get(2).map_err(|e| e.to_string())?;
        let status: String = row.get(3).map_err(|e| e.to_string())?;
        out.push(format!(
            "{table} day {day}{} 「{text}」 is recorded ({status})",
            session.map(|s| format!(" {s}")).unwrap_or_default()
        ));
    }
    Ok(())
}

async fn collect_summaries(
    conn: &Connection,
    out: &mut Vec<String>,
    sql: &str,
    binds: Vec<libsql::Value>,
) -> Result<(), String> {
    let mut rows = conn
        .query(sql, binds)
        .await
        .map_err(|e| format!("timesofday summary guard query failed: {e}"))?;
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let day: i64 = row.get(0).map_err(|e| e.to_string())?;
        let session: String = row.get(1).map_err(|e| e.to_string())?;
        let summary: String = row.get(2).map_err(|e| e.to_string())?;
        out.push(format!(
            "timesofday day {day} {session} has record_summary_zh 「{summary}」"
        ));
    }
    Ok(())
}

const SUMMARY_NOT_EMPTY: &str = "record_summary_zh IS NOT NULL AND record_summary_zh <> ''";

/// The common `(plan_id, destination)` bind pair for the guard queries.
fn binds_pd(plan_id: &str, destination: &str) -> Vec<libsql::Value> {
    vec![
        libsql::Value::Text(plan_id.to_string()),
        libsql::Value::Text(destination.to_string()),
    ]
}

/// The day-scoped guard SQL suffix: the day is a BIND (`?3`, via
/// [`binds_pd_day`]), never interpolated into the SQL — same rule as every
/// other value in this module.
fn day_scoped_sql(sql: &str) -> String {
    format!("{sql} AND day_number = ?3")
}

/// `(plan_id, destination, day)` binds for [`day_scoped_sql`].
fn binds_pd_day(plan_id: &str, destination: &str, day: i64) -> Vec<libsql::Value> {
    let mut b = binds_pd(plan_id, destination);
    b.push(libsql::Value::Integer(day));
    b
}

/// Refuse to wash away trip records (spec X3/X4): `Err` (naming every offending
/// row) when the scope contains any row with `record_status IS NOT NULL`, or —
/// for the `Day` / `Destination` scopes only — any non-empty session summary.
/// `Ok(())` means the delete/rebuild may proceed.
pub async fn ensure_unrecorded(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    scope: RecordScope<'_>,
) -> Result<(), String> {
    let mut hits: Vec<String> = Vec::new();
    match scope {
        RecordScope::Activity { id } => {
            collect_status_rows(
                conn,
                &mut hits,
                "SELECT day_number, session_type, title, record_status FROM activities \
                 WHERE plan_id = ?1 AND destination = ?2 AND id = ?3 AND record_status IS NOT NULL",
                vec![
                    libsql::Value::Text(plan_id.to_string()),
                    libsql::Value::Text(destination.to_string()),
                    libsql::Value::Text(id.to_string()),
                ],
                "activities",
            )
            .await?;
        }
        RecordScope::Day { day } => {
            collect_status_rows(
                conn,
                &mut hits,
                &day_scoped_sql(
                    "SELECT day_number, session_type, title, record_status FROM activities \
                     WHERE plan_id = ?1 AND destination = ?2 AND record_status IS NOT NULL",
                ),
                binds_pd_day(plan_id, destination, day),
                "activities",
            )
            .await?;
            collect_status_rows(
                conn,
                &mut hits,
                &day_scoped_sql(
                    "SELECT day_number, session_type, meal, record_status FROM session_meals \
                     WHERE plan_id = ?1 AND destination = ?2 AND record_status IS NOT NULL",
                ),
                binds_pd_day(plan_id, destination, day),
                "session_meals",
            )
            .await?;
            collect_status_rows(
                conn,
                &mut hits,
                &day_scoped_sql(
                    "SELECT day_number, NULL, from_place || ' → ' || to_place, record_status \
                     FROM day_route_segments \
                     WHERE plan_id = ?1 AND destination = ?2 AND record_status IS NOT NULL",
                ),
                binds_pd_day(plan_id, destination, day),
                "day_route_segments",
            )
            .await?;
            collect_summaries(
                conn,
                &mut hits,
                &day_scoped_sql(&format!(
                    "SELECT day_number, session_type, record_summary_zh FROM timesofday \
                     WHERE plan_id = ?1 AND destination = ?2 AND {SUMMARY_NOT_EMPTY}"
                )),
                binds_pd_day(plan_id, destination, day),
            )
            .await?;
        }
        RecordScope::ActivitiesOfDay { day } => {
            collect_status_rows(
                conn,
                &mut hits,
                &day_scoped_sql(
                    "SELECT day_number, session_type, title, record_status FROM activities \
                     WHERE plan_id = ?1 AND destination = ?2 AND record_status IS NOT NULL",
                ),
                binds_pd_day(plan_id, destination, day),
                "activities",
            )
            .await?;
        }
        RecordScope::MealsOfSession { day, session } => {
            let mut binds = binds_pd_day(plan_id, destination, day);
            binds.push(libsql::Value::Text(session.to_string()));
            collect_status_rows(
                conn,
                &mut hits,
                "SELECT day_number, session_type, meal, record_status FROM session_meals \
                 WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
                   AND session_type = ?4 AND record_status IS NOT NULL",
                binds,
                "session_meals",
            )
            .await?;
        }
        RecordScope::LegsOfDay { day } | RecordScope::LegsAiRecommendedOfDay { day } => {
            let ai_filter = match scope {
                RecordScope::LegsAiRecommendedOfDay { .. } => " AND source = 'ai_recommended'",
                _ => "",
            };
            collect_status_rows(
                conn,
                &mut hits,
                &day_scoped_sql(
                    &format!(
                        "SELECT day_number, NULL, from_place || ' → ' || to_place, record_status \
                         FROM day_route_segments \
                         WHERE plan_id = ?1 AND destination = ?2 AND record_status IS NOT NULL{ai_filter}"
                    ),
                ),
                binds_pd_day(plan_id, destination, day),
                "day_route_segments",
            )
            .await?;
        }
        RecordScope::LegSlot { day, sort_order } => {
            let mut binds = binds_pd_day(plan_id, destination, day);
            binds.push(libsql::Value::Integer(sort_order));
            collect_status_rows(
                conn,
                &mut hits,
                "SELECT day_number, NULL, from_place || ' → ' || to_place, record_status \
                 FROM day_route_segments \
                 WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 \
                   AND sort_order = ?4 AND record_status IS NOT NULL",
                binds,
                "day_route_segments",
            )
            .await?;
        }
        RecordScope::Destination => {
            collect_status_rows(
                conn,
                &mut hits,
                "SELECT day_number, session_type, title, record_status FROM activities \
                 WHERE plan_id = ?1 AND destination = ?2 AND record_status IS NOT NULL",
                binds_pd(plan_id, destination),
                "activities",
            )
            .await?;
            collect_status_rows(
                conn,
                &mut hits,
                "SELECT day_number, session_type, meal, record_status FROM session_meals \
                 WHERE plan_id = ?1 AND destination = ?2 AND record_status IS NOT NULL",
                binds_pd(plan_id, destination),
                "session_meals",
            )
            .await?;
            collect_status_rows(
                conn,
                &mut hits,
                "SELECT day_number, NULL, from_place || ' → ' || to_place, record_status \
                 FROM day_route_segments \
                 WHERE plan_id = ?1 AND destination = ?2 AND record_status IS NOT NULL",
                binds_pd(plan_id, destination),
                "day_route_segments",
            )
            .await?;
            collect_summaries(
                conn,
                &mut hits,
                &format!(
                    "SELECT day_number, session_type, record_summary_zh FROM timesofday \
                     WHERE plan_id = ?1 AND destination = ?2 AND {SUMMARY_NOT_EMPTY}"
                ),
                binds_pd(plan_id, destination),
            )
            .await?;
        }
    }
    if hits.is_empty() {
        return Ok(());
    }
    Err(format!(
        "refusing: this would delete recorded trip content — {}",
        hits.join("; ")
    ))
}
