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

/// One row the `validate data` record-column lint (audit H2) sees. Two kinds:
/// - a row-level record: `record_status IS NOT NULL` on `activities` /
///   `session_meals` / `day_route_segments` (`status` is `Some`, `is_summary`
///   false);
/// - a session summary record: `timesofday.record_summary_zh` non-empty
///   (`status` is `None`, `is_summary` true, `row_text` is the summary text —
///   a summary alone is a complete record, spec §5 `record-session`).
/// Both carry the day's `date` and `variant_of` (NULL when the `days` row is
/// missing). The rule decisions (legal status values, future dates, variant
/// days) live in the validator's pure functions, not here.
#[derive(Debug, Clone)]
pub struct RecordLintRow {
    /// Source table: 'activities' | 'session_meals' | 'day_route_segments' |
    /// 'timesofday' (summary rows).
    pub table: String,
    pub plan_id: String,
    pub destination: String,
    pub day: i64,
    /// Session type; legs have none.
    pub session: Option<String>,
    /// Identifying text: title / meal / from_place → to_place / the summary.
    pub row_text: String,
    /// The row's `record_status`; `None` on summary rows.
    pub status: Option<String>,
    /// True when this row is a `timesofday.record_summary_zh` record.
    pub is_summary: bool,
    /// The row's day date (`days.date`), NULL when the day row is missing.
    pub date: Option<String>,
    /// The day's `days.variant_of`; Some = a variant (alternative) day.
    pub variant_of: Option<i64>,
}

/// The `validate data` record-column reader (audit H2): every row-level record
/// (`record_status IS NOT NULL` on `activities` / `session_meals` /
/// `day_route_segments`) plus every session summary record (non-empty
/// `timesofday.record_summary_zh`), LEFT JOINed with the day's date and
/// `variant_of` — ONE targeted SELECT (UNION ALL) whose result set is exactly
/// what the audit-H2 rules (illegal status values; future-dated records;
/// records on variant days) evaluate. The ORDER BY carries each branch's
/// `sort_order` and (for activities) `id`, so same-day same-table rows keep a
/// stable issue order.
pub async fn recorded_rows(conn: &Connection) -> Result<Vec<RecordLintRow>, String> {
    const QUERY: &str = "\
        SELECT 'activities', a.plan_id, a.destination, a.day_number, a.session_type, \
               a.title, a.record_status, 0, d.date, d.variant_of, a.sort_order, a.id \
        FROM activities a LEFT JOIN days d \
          ON d.plan_id = a.plan_id AND d.destination = a.destination \
             AND d.day_number = a.day_number \
        WHERE a.record_status IS NOT NULL \
        UNION ALL \
        SELECT 'session_meals', m.plan_id, m.destination, m.day_number, m.session_type, \
               m.meal, m.record_status, 0, d.date, d.variant_of, m.sort_order, '' \
        FROM session_meals m LEFT JOIN days d \
          ON d.plan_id = m.plan_id AND d.destination = m.destination \
             AND d.day_number = m.day_number \
        WHERE m.record_status IS NOT NULL \
        UNION ALL \
        SELECT 'day_route_segments', r.plan_id, r.destination, r.day_number, NULL, \
               r.from_place || ' → ' || r.to_place, r.record_status, 0, \
               d.date, d.variant_of, r.sort_order, '' \
        FROM day_route_segments r LEFT JOIN days d \
          ON d.plan_id = r.plan_id AND d.destination = r.destination \
             AND d.day_number = r.day_number \
        WHERE r.record_status IS NOT NULL \
        UNION ALL \
        SELECT 'timesofday', t.plan_id, t.destination, t.day_number, t.session_type, \
               t.record_summary_zh, NULL, 1, d.date, d.variant_of, 0, '' \
        FROM timesofday t LEFT JOIN days d \
          ON d.plan_id = t.plan_id AND d.destination = t.destination \
             AND d.day_number = t.day_number \
        WHERE t.record_summary_zh IS NOT NULL AND TRIM(t.record_summary_zh) <> '' \
        ORDER BY 2, 3, 4, 1, 5, 11, 12";
    let mut rows = conn
        .query(QUERY, ())
        .await
        .map_err(|e| format!("record lint rows query failed: {e}"))?;
    let mut out: Vec<RecordLintRow> = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let g = |e: libsql::Error| format!("record lint row decode failed: {e}");
        out.push(RecordLintRow {
            table: row.get(0).map_err(g)?,
            plan_id: row.get(1).map_err(g)?,
            destination: row.get(2).map_err(g)?,
            day: row.get(3).map_err(g)?,
            session: row.get::<Option<String>>(4).map_err(g)?,
            row_text: row.get(5).map_err(g)?,
            status: row.get(6).map_err(g)?,
            is_summary: row.get::<i64>(7).map_err(g)? != 0,
            date: row.get(8).map_err(g)?,
            variant_of: row.get(9).map_err(g)?,
        });
    }
    Ok(out)
}

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

/// Refuse a TITLE edit on an activity that already carries a trip record
/// (`record_status IS NOT NULL`): `done 東寺` followed by a retitle to another place would
/// make the record lie (spec 2026-11-19 §1.4). Shared by `update-activity --title` and
/// `set-activity-title`. Callers invoke it only when the title would actually change.
/// A missing row is an `Err` too.
pub async fn ensure_title_editable(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    activity_id: &str,
) -> Result<(), String> {
    let Some((status, _)) = read_activity_record(conn, plan_id, destination, activity_id).await?
    else {
        return Err(format!(
            "activity {activity_id} not found in plan {plan_id} ({destination})"
        ));
    };
    match status {
        Some(st) => Err(format!(
            "activity {activity_id} already has a trip record ({st}); refusing to change its title \
             (the record would describe a different place). Use `record-activity` to change the \
             note, or `clear-record` first, then retitle"
        )),
        None => Ok(()),
    }
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

// ---------------------------------------------------------------------------
// Reading a plan's record rows (verify-site's record layer, audit H1)
// ---------------------------------------------------------------------------

/// One recorded `activities` row (see [`list_plan_records`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityRecord {
    /// `activities.id` — the row's stable identity (`data-activity-id`).
    pub id: String,
    pub day: i64,
    pub session: String,
    pub title: String,
    pub status: Option<String>,
    pub note: Option<String>,
}

/// One recorded `session_meals` row; `sort_order` is the row's identity
/// (`data-meal-order`) — the table has no id column.
#[derive(Debug, Clone, PartialEq)]
pub struct MealRecord {
    pub day: i64,
    pub session: String,
    pub sort_order: i64,
    pub text: String,
    pub status: Option<String>,
    pub note: Option<String>,
}

/// One recorded `day_route_segments` row; `sort_order` is the row's identity
/// (`data-seg-order`) — the table has no id column.
#[derive(Debug, Clone, PartialEq)]
pub struct LegRecord {
    pub day: i64,
    pub sort_order: i64,
    pub from_place: String,
    pub to_place: String,
    pub status: Option<String>,
    pub note: Option<String>,
}

/// One non-empty `timesofday.record_summary_zh`.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummaryRecord {
    pub day: i64,
    pub session: String,
    pub summary_zh: String,
}

/// A plan's recorded rows, in table render order (day, then sort_order). Only
/// rows with something recorded are listed: a row counts when its status or
/// note is a non-empty value (the worker renders empty strings as unset), and
/// a summary when its text is non-empty.
#[derive(Debug, Default, PartialEq)]
pub struct PlanRecords {
    pub activities: Vec<ActivityRecord>,
    pub meals: Vec<MealRecord>,
    pub legs: Vec<LegRecord>,
    pub summaries: Vec<SessionSummaryRecord>,
}

impl PlanRecords {
    pub fn is_empty(&self) -> bool {
        self.activities.is_empty()
            && self.meals.is_empty()
            && self.legs.is_empty()
            && self.summaries.is_empty()
    }
}

/// Read every record row of one (plan, destination) — the single Turso read
/// verify-site's record layer runs per plan (one targeted SELECT per table).
pub async fn list_plan_records(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
) -> Result<PlanRecords, String> {
    let recorded = "(record_status IS NOT NULL AND record_status <> '') \
                    OR (record_note IS NOT NULL AND record_note <> '')";
    let mut out = PlanRecords::default();

    let mut rows = conn
        .query(
            &format!(
                "SELECT id, day_number, session_type, title, record_status, record_note \
                 FROM activities \
                 WHERE plan_id = ?1 AND destination = ?2 AND ({recorded}) \
                 ORDER BY day_number, sort_order"
            ),
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("activities record-list query failed: {e}"))?;
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.activities.push(ActivityRecord {
            id: row.get(0).map_err(|e| e.to_string())?,
            day: row.get(1).map_err(|e| e.to_string())?,
            session: row.get(2).map_err(|e| e.to_string())?,
            title: row.get(3).map_err(|e| e.to_string())?,
            status: row.get(4).map_err(|e| e.to_string())?,
            note: row.get(5).map_err(|e| e.to_string())?,
        });
    }

    let mut rows = conn
        .query(
            &format!(
                "SELECT day_number, session_type, sort_order, meal, record_status, record_note \
                 FROM session_meals \
                 WHERE plan_id = ?1 AND destination = ?2 AND ({recorded}) \
                 ORDER BY day_number, session_type, sort_order"
            ),
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("session_meals record-list query failed: {e}"))?;
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.meals.push(MealRecord {
            day: row.get(0).map_err(|e| e.to_string())?,
            session: row.get(1).map_err(|e| e.to_string())?,
            sort_order: row.get(2).map_err(|e| e.to_string())?,
            text: row.get(3).map_err(|e| e.to_string())?,
            status: row.get(4).map_err(|e| e.to_string())?,
            note: row.get(5).map_err(|e| e.to_string())?,
        });
    }

    let mut rows = conn
        .query(
            &format!(
                "SELECT day_number, sort_order, from_place, to_place, record_status, record_note \
                 FROM day_route_segments \
                 WHERE plan_id = ?1 AND destination = ?2 AND ({recorded}) \
                 ORDER BY day_number, sort_order"
            ),
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("day_route_segments record-list query failed: {e}"))?;
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.legs.push(LegRecord {
            day: row.get(0).map_err(|e| e.to_string())?,
            sort_order: row.get(1).map_err(|e| e.to_string())?,
            from_place: row.get(2).map_err(|e| e.to_string())?,
            to_place: row.get(3).map_err(|e| e.to_string())?,
            status: row.get(4).map_err(|e| e.to_string())?,
            note: row.get(5).map_err(|e| e.to_string())?,
        });
    }

    let mut rows = conn
        .query(
            "SELECT day_number, session_type, record_summary_zh FROM timesofday \
             WHERE plan_id = ?1 AND destination = ?2 \
               AND record_summary_zh IS NOT NULL AND record_summary_zh <> '' \
             ORDER BY day_number, session_type",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("timesofday summary-list query failed: {e}"))?;
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.summaries.push(SessionSummaryRecord {
            day: row.get(0).map_err(|e| e.to_string())?,
            session: row.get(1).map_err(|e| e.to_string())?,
            summary_zh: row.get(2).map_err(|e| e.to_string())?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal in-memory schema of the four recorded tables (only the columns
    /// `list_plan_records` reads).
    async fn db() -> Connection {
        let db = libsql::Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        for sql in [
            "CREATE TABLE activities (id TEXT, plan_id TEXT, destination TEXT, day_number INTEGER, \
             session_type TEXT, sort_order INTEGER, title TEXT, record_status TEXT, record_note TEXT)",
            "CREATE TABLE session_meals (plan_id TEXT, destination TEXT, day_number INTEGER, \
             session_type TEXT, sort_order INTEGER, meal TEXT, record_status TEXT, record_note TEXT)",
            "CREATE TABLE day_route_segments (plan_id TEXT, destination TEXT, day_number INTEGER, \
             sort_order INTEGER, from_place TEXT, to_place TEXT, record_status TEXT, record_note TEXT)",
            "CREATE TABLE timesofday (plan_id TEXT, destination TEXT, day_number INTEGER, \
             session_type TEXT, record_summary_zh TEXT)",
        ] {
            conn.execute(sql, ()).await.unwrap();
        }
        conn
    }

    #[tokio::test]
    async fn list_plan_records_returns_only_recorded_rows_in_order() {
        let conn = db().await;
        for sql in [
            // recorded (done, skipped, note-only) + unrecorded (NULL, empty strings) rows,
            // deliberately inserted OUT of sort_order so the ordering is the query's.
            "INSERT INTO activities VALUES \
             ('a2','p','kyoto',2,'noon',1,'未前往店','skipped',NULL), \
             ('a4','p','kyoto',2,'noon',3,'空字串列','',''), \
             ('a1','p','kyoto',2,'noon',0,'東寺','done','很棒'), \
             ('a3','p','kyoto',2,'noon',2,'未標記店',NULL,NULL), \
             ('a5','p','kyoto',2,'noon',4,'只有備註',NULL,'備註一句'), \
             ('a9','p','other',1,'noon',0,'別的目的地','done',NULL)",
        ] {
            conn.execute(sql, ()).await.unwrap();
        }
        conn.execute(
            "INSERT INTO session_meals VALUES \
             ('p','kyoto',2,'noon',1,'備選店','skipped','排隊太長'), \
             ('p','kyoto',2,'noon',0,'主選店','done',NULL), \
             ('p','kyoto',2,'noon',2,'未標記餐',NULL,NULL)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO day_route_segments VALUES \
             ('p','kyoto',2,1,'B','C','skipped',NULL), \
             ('p','kyoto',2,0,'A','B','done','走路10分'), \
             ('p','kyoto',2,2,'C','D',NULL,NULL)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO timesofday VALUES \
             ('p','kyoto',2,'noon','實際去了東寺，改吃隔壁'), \
             ('p','kyoto',2,'evening',''), \
             ('p','kyoto',3,'noon',NULL)",
            (),
        )
        .await
        .unwrap();

        let records = list_plan_records(&conn, "p", "kyoto").await.unwrap();
        assert!(!records.is_empty());
        // Activities: only the recorded ones, in (day, sort_order); note-only rows count.
        let ids: Vec<&str> = records.activities.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["a1", "a2", "a5"], "{:?}", records.activities);
        assert_eq!(
            records.activities[0],
            ActivityRecord {
                id: "a1".into(),
                day: 2,
                session: "noon".into(),
                title: "東寺".into(),
                status: Some("done".into()),
                note: Some("很棒".into()),
            }
        );
        // Meals and legs: same rule, keyed by sort_order.
        assert_eq!(records.meals.len(), 2);
        assert_eq!((records.meals[0].sort_order, records.meals[0].text.as_str()), (0, "主選店"));
        assert_eq!(records.meals[1].status.as_deref(), Some("skipped"));
        assert_eq!(records.meals[1].note.as_deref(), Some("排隊太長"));
        assert_eq!(records.legs.len(), 2);
        assert_eq!((records.legs[0].sort_order, records.legs[0].from_place.as_str()), (0, "A"));
        assert_eq!(records.legs[1].status.as_deref(), Some("skipped"));
        // Summaries: empty strings and NULL stay out.
        assert_eq!(records.summaries.len(), 1);
        assert_eq!(records.summaries[0].summary_zh, "實際去了東寺，改吃隔壁");

        // Destination scoping: the 'other' destination sees only its own row.
        let other = list_plan_records(&conn, "p", "other").await.unwrap();
        assert_eq!(other.activities.len(), 1);
        assert_eq!(other.activities[0].id, "a9");
        // Nothing recorded anywhere → empty (the layer's one-PASS case).
        assert!(list_plan_records(&conn, "p", "nowhere").await.unwrap().is_empty());
        let fresh = db().await;
        assert!(list_plan_records(&fresh, "p", "kyoto").await.unwrap().is_empty());
    }
}

#[cfg(test)]
mod recorded_rows_tests {
    use super::recorded_rows;

    // In-memory tables with only the columns `recorded_rows` reads (no Turso).
    async fn fixture() -> libsql::Connection {
        let db = libsql::Builder::new_local(":memory:").build().await.unwrap();
        let c = db.connect().unwrap();
        c.execute_batch(
            "CREATE TABLE days (plan_id TEXT, destination TEXT, day_number INTEGER, date TEXT, variant_of INTEGER);
             CREATE TABLE activities (id TEXT, plan_id TEXT, destination TEXT, day_number INTEGER, session_type TEXT, title TEXT, record_status TEXT, sort_order INTEGER);
             CREATE TABLE session_meals (plan_id TEXT, destination TEXT, day_number INTEGER, session_type TEXT, meal TEXT, record_status TEXT, sort_order INTEGER);
             CREATE TABLE day_route_segments (plan_id TEXT, destination TEXT, day_number INTEGER, from_place TEXT, to_place TEXT, record_status TEXT, sort_order INTEGER);
             CREATE TABLE timesofday (plan_id TEXT, destination TEXT, day_number INTEGER, session_type TEXT, record_summary_zh TEXT);
             INSERT INTO days VALUES ('p','d',1,'2026-11-01',NULL),('p','d',2,'2026-11-02',3);
             INSERT INTO activities VALUES ('a-z','p','d',1,'morning','B','done',2),('a-a','p','d',1,'morning','A','done',1),('a-n','p','d',1,'morning','none',NULL,3);
             INSERT INTO timesofday VALUES ('p','d',1,'morning','摘要'),('p','d',1,'noon','  '),('p','d',2,'noon','變體摘要');",
        )
        .await
        .unwrap();
        c
    }

    #[tokio::test]
    async fn rows_include_summaries_and_variant_of_and_skip_unrecorded() {
        let rows = recorded_rows(&fixture().await).await.unwrap();
        let texts: Vec<_> = rows.iter().map(|r| r.row_text.as_str()).collect();
        assert_eq!(texts, ["A", "B", "摘要", "變體摘要"], "{texts:?}");
        assert!(rows[2].is_summary && rows[2].status.is_none());
        assert_eq!(rows[3].variant_of, Some(3));
        assert_eq!(rows[0].variant_of, None);
    }

    #[tokio::test]
    async fn same_day_same_table_rows_order_by_sort_order_not_insert_order() {
        let rows = recorded_rows(&fixture().await).await.unwrap();
        // inserted B (sort 2) before A (sort 1): stable order puts A first
        assert_eq!(rows[0].row_text, "A");
        assert_eq!(rows[1].row_text, "B");
    }
}

#[cfg(test)]
mod title_editable_tests {
    use super::*;

    async fn db() -> Connection {
        let db = libsql::Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE activities (id TEXT, plan_id TEXT, destination TEXT, title TEXT, \
             record_status TEXT, record_note TEXT)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO activities VALUES \
             ('a_rec','p','d','東寺','done','x'), ('a_plain','p','d','鼻頭角',NULL,NULL)",
            (),
        )
        .await
        .unwrap();
        conn
    }

    #[tokio::test]
    async fn ensure_title_editable_refuses_recorded_row_only() {
        let conn = db().await;
        let err = ensure_title_editable(&conn, "p", "d", "a_rec").await.unwrap_err();
        assert!(err.contains("record-activity") && err.contains("clear-record"), "{err}");
        assert!(ensure_title_editable(&conn, "p", "d", "a_plain").await.is_ok());
        assert!(ensure_title_editable(&conn, "p", "d", "nope").await.unwrap_err().contains("not found"));
    }
}
