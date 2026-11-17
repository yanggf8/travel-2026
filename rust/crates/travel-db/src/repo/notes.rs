//! Travel notes DAL — `notes`, `note_images`, `note_share_tokens`, `note_events`.
//!
//! Notes are an independent entity (spec 2026-11-15-travel-notes-design.md §7): they may hang on
//! a plan/day (`attached_*`, deliberately NOT named `plan_id`) or on nothing. Writes do NOT touch
//! `plan_events` / `plans.version` / `operation_runs`; the only audit surface is `note_events`
//! (`insert_event`), written by the caller with the shared action names
//! (`create update delete image_add image_delete image_move share_create share_deactivate
//! share_clear purge`).
//!
//! Every statement uses bound parameters — free text never reaches SQL by string building.
//! Repo functions do not open transactions (same convention as the other repos): a multi-step
//! caller (e.g. `purge_note` + its `purge` event) wraps them in `begin_immediate`/`finish_tx`.
//! Mutations that target one existing row return `bool` (false = 0 rows changed) so the caller
//! can refuse to write a `note_events` row for a no-op.
//!
//! R2 ordering rule (caller side): read `r2_key` (`get_image_key`), delete the R2 object, and only
//! then call `delete_image_row`; never delete the DB row first.

use libsql::{params, Connection};

use crate::ids::new_run_id;

/// Writable note fields. `update_note` writes the whole input (a `None` clears the column).
#[derive(Debug, Clone)]
pub struct NoteInput<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub purpose: Option<&'a str>,
    pub place_name: Option<&'a str>,
    pub taken_on: Option<&'a str>,
    pub attached_plan_id: Option<&'a str>,
    pub attached_destination: Option<&'a str>,
    pub attached_day_number: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRow {
    pub id: String,
    pub title: String,
    pub body: String,
    pub purpose: Option<String>,
    pub place_name: Option<String>,
    pub taken_on: Option<String>,
    pub attached_plan_id: Option<String>,
    pub attached_destination: Option<String>,
    pub attached_day_number: Option<i64>,
    pub created_by: String,
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRow {
    pub image_id: String,
    pub sort_order: i64,
    pub r2_key: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub caption: Option<String>,
}

const NOTE_COLS: &str = "id, title, body, purpose, place_name, taken_on, attached_plan_id, \
                         attached_destination, attached_day_number, created_by, deleted_at";

fn db_err(what: &str, e: libsql::Error) -> String {
    format!("notes {what} failed: {e}")
}

fn note_from_row(r: &libsql::Row) -> Result<NoteRow, String> {
    let g = |e: libsql::Error| db_err("row read", e);
    Ok(NoteRow {
        id: r.get::<String>(0).map_err(g)?,
        title: r.get::<Option<String>>(1).map_err(g)?.unwrap_or_default(),
        body: r.get::<Option<String>>(2).map_err(g)?.unwrap_or_default(),
        purpose: r.get::<Option<String>>(3).map_err(g)?,
        place_name: r.get::<Option<String>>(4).map_err(g)?,
        taken_on: r.get::<Option<String>>(5).map_err(g)?,
        attached_plan_id: r.get::<Option<String>>(6).map_err(g)?,
        attached_destination: r.get::<Option<String>>(7).map_err(g)?,
        attached_day_number: r.get::<Option<i64>>(8).map_err(g)?,
        created_by: r.get::<Option<String>>(9).map_err(g)?.unwrap_or_default(),
        deleted_at: r.get::<Option<String>>(10).map_err(g)?,
    })
}

fn opt(s: Option<&str>) -> libsql::Value {
    match s {
        Some(v) => libsql::Value::Text(v.to_string()),
        None => libsql::Value::Null,
    }
}

fn opt_i(n: Option<i64>) -> libsql::Value {
    match n {
        Some(v) => libsql::Value::Integer(v),
        None => libsql::Value::Null,
    }
}

// ── notes ──────────────────────────────────────────────────────────────────

pub async fn insert_note(
    conn: &Connection,
    id: &str,
    n: &NoteInput<'_>,
    actor: &str,
    now: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO notes (id, title, body, purpose, place_name, taken_on, attached_plan_id, \
            attached_destination, attached_day_number, created_at, created_by, updated_at, updated_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?10, ?11)",
        params![
            id.to_string(),
            n.title.to_string(),
            n.body.to_string(),
            opt(n.purpose),
            opt(n.place_name),
            opt(n.taken_on),
            opt(n.attached_plan_id),
            opt(n.attached_destination),
            opt_i(n.attached_day_number),
            now.to_string(),
            actor.to_string()
        ],
    )
    .await
    .map_err(|e| db_err("insert_note", e))?;
    Ok(())
}

/// Rewrite all editable fields of a live note. `false` = no live row (missing or soft-deleted).
pub async fn update_note(
    conn: &Connection,
    id: &str,
    n: &NoteInput<'_>,
    actor: &str,
    now: &str,
) -> Result<bool, String> {
    let rows = conn
        .execute(
            "UPDATE notes SET title = ?2, body = ?3, purpose = ?4, place_name = ?5, taken_on = ?6, \
                attached_plan_id = ?7, attached_destination = ?8, attached_day_number = ?9, \
                updated_at = ?10, updated_by = ?11 \
             WHERE id = ?1 AND deleted_at IS NULL",
            params![
                id.to_string(),
                n.title.to_string(),
                n.body.to_string(),
                opt(n.purpose),
                opt(n.place_name),
                opt(n.taken_on),
                opt(n.attached_plan_id),
                opt(n.attached_destination),
                opt_i(n.attached_day_number),
                now.to_string(),
                actor.to_string()
            ],
        )
        .await
        .map_err(|e| db_err("update_note", e))?;
    Ok(rows > 0)
}

/// Soft delete. `false` = no live row (missing or already deleted; `deleted_at` is not moved).
pub async fn soft_delete_note(
    conn: &Connection,
    id: &str,
    actor: &str,
    now: &str,
) -> Result<bool, String> {
    let rows = conn
        .execute(
            "UPDATE notes SET deleted_at = ?2, deleted_by = ?3 WHERE id = ?1 AND deleted_at IS NULL",
            params![id.to_string(), now.to_string(), actor.to_string()],
        )
        .await
        .map_err(|e| db_err("soft_delete_note", e))?;
    Ok(rows > 0)
}

/// One note by id, INCLUDING soft-deleted rows (`deleted_at` tells; callers decide).
pub async fn get_note(conn: &Connection, id: &str) -> Result<Option<NoteRow>, String> {
    let mut rows = conn
        .query(
            &format!("SELECT {NOTE_COLS} FROM notes WHERE id = ?1"),
            params![id.to_string()],
        )
        .await
        .map_err(|e| db_err("get_note", e))?;
    match rows.next().await.map_err(|e| db_err("get_note", e))? {
        Some(r) => Ok(Some(note_from_row(&r)?)),
        None => Ok(None),
    }
}

/// Live (not soft-deleted) notes, newest first. No selector = every note, attached or not.
/// `day` without `plan` is rejected (a bare day number is meaningless across plans).
pub async fn list_notes(
    conn: &Connection,
    plan: Option<&str>,
    day: Option<i64>,
) -> Result<Vec<NoteRow>, String> {
    if day.is_some() && plan.is_none() {
        return Err("notes list: a day filter requires a plan".to_string());
    }
    let mut rows = conn
        .query(
            &format!(
                "SELECT {NOTE_COLS} FROM notes \
                 WHERE deleted_at IS NULL \
                   AND (?1 IS NULL OR attached_plan_id = ?1) \
                   AND (?2 IS NULL OR attached_day_number = ?2) \
                 ORDER BY created_at DESC, id"
            ),
            params![opt(plan), opt_i(day)],
        )
        .await
        .map_err(|e| db_err("list_notes", e))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| db_err("list_notes", e))? {
        out.push(note_from_row(&r)?);
    }
    Ok(out)
}

// ── images ─────────────────────────────────────────────────────────────────

/// Append an image row to a LIVE note; `img.sort_order` is ignored — the new row takes
/// MAX(sort_order)+1 for the note (1 for the first), computed in the same statement. The parent
/// note's existence and `deleted_at IS NULL` are checked by that same statement; `false` = nothing
/// written (note missing or soft-deleted), so the caller must not write an `image_add` event.
pub async fn insert_image(
    conn: &Connection,
    note_id: &str,
    img: &ImageRow,
    actor: &str,
    now: &str,
) -> Result<bool, String> {
    let rows = conn
        .execute(
            "INSERT INTO note_images (note_id, image_id, sort_order, r2_key, content_type, size_bytes, \
                caption, created_at, created_by) \
             SELECT ?1, ?2, \
                (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM note_images WHERE note_id = ?1), \
                ?3, ?4, ?5, ?6, ?7, ?8 \
             FROM notes WHERE id = ?1 AND deleted_at IS NULL",
            params![
                note_id.to_string(),
                img.image_id.clone(),
                img.r2_key.clone(),
                img.content_type.clone(),
                img.size_bytes,
                opt(img.caption.as_deref()),
                now.to_string(),
                actor.to_string()
            ],
        )
        .await
        .map_err(|e| db_err("insert_image", e))?;
    Ok(rows > 0)
}

/// The stored `r2_key` of `(note_id, image_id)`; the only legal source of an R2 key.
pub async fn get_image_key(
    conn: &Connection,
    note_id: &str,
    image_id: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT r2_key FROM note_images WHERE note_id = ?1 AND image_id = ?2",
            params![note_id.to_string(), image_id.to_string()],
        )
        .await
        .map_err(|e| db_err("get_image_key", e))?;
    match rows.next().await.map_err(|e| db_err("get_image_key", e))? {
        Some(r) => Ok(r.get::<Option<String>>(0).map_err(|e| db_err("get_image_key", e))?),
        None => Ok(None),
    }
}

/// Delete one image row of a LIVE note (a soft-deleted note keeps its photos until the 30-day
/// purge; `false` also covers that). Caller deletes the R2 object FIRST and calls this only on success.
pub async fn delete_image_row(
    conn: &Connection,
    note_id: &str,
    image_id: &str,
) -> Result<bool, String> {
    let rows = conn
        .execute(
            "DELETE FROM note_images WHERE note_id = ?1 AND image_id = ?2 \
               AND EXISTS (SELECT 1 FROM notes WHERE id = ?1 AND deleted_at IS NULL)",
            params![note_id.to_string(), image_id.to_string()],
        )
        .await
        .map_err(|e| db_err("delete_image_row", e))?;
    Ok(rows > 0)
}

pub async fn list_images(conn: &Connection, note_id: &str) -> Result<Vec<ImageRow>, String> {
    let mut rows = conn
        .query(
            "SELECT image_id, sort_order, r2_key, content_type, size_bytes, caption \
             FROM note_images WHERE note_id = ?1 ORDER BY sort_order, image_id",
            params![note_id.to_string()],
        )
        .await
        .map_err(|e| db_err("list_images", e))?;
    let g = |e: libsql::Error| db_err("list_images", e);
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(g)? {
        out.push(ImageRow {
            image_id: r.get::<String>(0).map_err(g)?,
            sort_order: r.get::<Option<i64>>(1).map_err(g)?.unwrap_or(0),
            r2_key: r.get::<Option<String>>(2).map_err(g)?.unwrap_or_default(),
            content_type: r.get::<Option<String>>(3).map_err(g)?.unwrap_or_default(),
            size_bytes: r.get::<Option<i64>>(4).map_err(g)?.unwrap_or(0),
            caption: r.get::<Option<String>>(5).map_err(g)?,
        });
    }
    Ok(out)
}

// ── share tokens ───────────────────────────────────────────────────────────

/// New token for a LIVE note (status defaults to `active`). `false` = nothing written (note
/// missing or soft-deleted). Format validation is the caller's job.
pub async fn insert_share_token(
    conn: &Connection,
    note_id: &str,
    token: &str,
    actor: &str,
    now: &str,
) -> Result<bool, String> {
    let rows = conn
        .execute(
            "INSERT INTO note_share_tokens (token, note_id, created_at, created_by) \
             SELECT ?1, ?2, ?3, ?4 FROM notes WHERE id = ?2 AND deleted_at IS NULL",
            params![token.to_string(), note_id.to_string(), now.to_string(), actor.to_string()],
        )
        .await
        .map_err(|e| db_err("insert_share_token", e))?;
    Ok(rows > 0)
}

/// Mark ACTIVE tokens whose text starts with any prefix as `inactive` (rows stay; `clear_*`
/// deletes). Prefix match is literal (no LIKE wildcards). An empty prefix is rejected — it would
/// match every token. Returns the number of rows deactivated.
pub async fn deactivate_share_tokens(
    conn: &Connection,
    prefixes: &[&str],
    actor: &str,
    now: &str,
) -> Result<u64, String> {
    if prefixes.iter().any(|p| p.is_empty()) {
        return Err("deactivate_share_tokens: empty prefix would match every token".to_string());
    }
    let mut total = 0u64;
    for p in prefixes {
        total += conn
            .execute(
                "UPDATE note_share_tokens \
                 SET status = 'inactive', deactivated_at = ?2, deactivated_by = ?3 \
                 WHERE status = 'active' AND substr(token, 1, length(?1)) = ?1",
                params![p.to_string(), now.to_string(), actor.to_string()],
            )
            .await
            .map_err(|e| db_err("deactivate_share_tokens", e))?;
    }
    Ok(total)
}

/// The DELETE statement `clear_share_tokens` runs: scoped to one note (`?1`) or, with `None`,
/// the whole table (`clear --all`). Pure so the WHERE shape is unit-tested without touching a DB.
pub fn clear_share_tokens_sql(note_id: Option<&str>) -> &'static str {
    match note_id {
        Some(_) => "DELETE FROM note_share_tokens WHERE note_id = ?1",
        None => "DELETE FROM note_share_tokens",
    }
}

/// Hard-delete tokens (active AND inactive) of one note, or of every note with `None`.
pub async fn clear_share_tokens(conn: &Connection, note_id: Option<&str>) -> Result<u64, String> {
    let sql = clear_share_tokens_sql(note_id);
    let res = match note_id {
        Some(id) => conn.execute(sql, params![id.to_string()]).await,
        None => conn.execute(sql, ()).await,
    };
    res.map_err(|e| db_err("clear_share_tokens", e))
}

/// `(token, status, created_at)` of a note, newest first.
pub async fn list_share_tokens(
    conn: &Connection,
    note_id: &str,
) -> Result<Vec<(String, String, String)>, String> {
    let mut rows = conn
        .query(
            "SELECT token, status, created_at FROM note_share_tokens \
             WHERE note_id = ?1 ORDER BY created_at DESC, token",
            params![note_id.to_string()],
        )
        .await
        .map_err(|e| db_err("list_share_tokens", e))?;
    let g = |e: libsql::Error| db_err("list_share_tokens", e);
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(g)? {
        out.push((
            r.get::<String>(0).map_err(g)?,
            r.get::<Option<String>>(1).map_err(g)?.unwrap_or_default(),
            r.get::<Option<String>>(2).map_err(g)?.unwrap_or_default(),
        ));
    }
    Ok(out)
}

/// `(note_id, token, status)` of every note share token, any note, any status. Lets the CLI find
/// the owning note of a token prefix (to write its `share_deactivate` event) and the notes a
/// `clear --all` touches (one `share_clear` event each).
pub async fn list_all_share_tokens(
    conn: &Connection,
) -> Result<Vec<(String, String, String)>, String> {
    let mut rows = conn
        .query("SELECT note_id, token, status FROM note_share_tokens ORDER BY note_id, token", ())
        .await
        .map_err(|e| db_err("list_all_share_tokens", e))?;
    let g = |e: libsql::Error| db_err("list_all_share_tokens", e);
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(g)? {
        out.push((
            r.get::<String>(0).map_err(g)?,
            r.get::<String>(1).map_err(g)?,
            r.get::<Option<String>>(2).map_err(g)?.unwrap_or_default(),
        ));
    }
    Ok(out)
}

// ── purge / events ─────────────────────────────────────────────────────────

/// Soft-deleted notes with `deleted_at < cutoff` and their R2 keys (for the 30-day purge).
pub async fn deleted_before(
    conn: &Connection,
    cutoff: &str,
) -> Result<Vec<(String, Vec<String>)>, String> {
    let mut rows = conn
        .query(
            "SELECT n.id, i.r2_key FROM notes n \
             LEFT JOIN note_images i ON i.note_id = n.id \
             WHERE n.deleted_at IS NOT NULL AND n.deleted_at < ?1 \
             ORDER BY n.id, i.sort_order, i.image_id",
            params![cutoff.to_string()],
        )
        .await
        .map_err(|e| db_err("deleted_before", e))?;
    let g = |e: libsql::Error| db_err("deleted_before", e);
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    while let Some(r) = rows.next().await.map_err(g)? {
        let id = r.get::<String>(0).map_err(g)?;
        let key = r.get::<Option<String>>(1).map_err(g)?;
        if out.last().map(|(last, _)| last != &id).unwrap_or(true) {
            out.push((id, Vec::new()));
        }
        if let Some(k) = key {
            out.last_mut().expect("pushed above").1.push(k);
        }
    }
    Ok(out)
}

/// Hard-delete a note with its image rows and share tokens. `note_events` is kept; the caller
/// writes the `purge` event. Call only after the R2 objects are gone; wrap in a transaction
/// together with that event. Returns how many `notes` rows were deleted (0 = no such note:
/// the caller must then write NO purge event).
pub async fn purge_note(conn: &Connection, id: &str) -> Result<u64, String> {
    let mut notes_deleted = 0;
    for sql in [
        "DELETE FROM note_images WHERE note_id = ?1",
        "DELETE FROM note_share_tokens WHERE note_id = ?1",
        "DELETE FROM notes WHERE id = ?1",
    ] {
        let n = conn
            .execute(sql, params![id.to_string()])
            .await
            .map_err(|e| db_err("purge_note", e))?;
        if sql.starts_with("DELETE FROM notes ") {
            notes_deleted = n;
        }
    }
    Ok(notes_deleted)
}

/// Append one `note_events` row (random UUID-shaped `event_id`).
pub async fn insert_event(
    conn: &Connection,
    note_id: &str,
    action: &str,
    actor: &str,
    detail: &str,
    now: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO note_events (event_id, note_id, action, actor, detail, at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            new_run_id(),
            note_id.to_string(),
            action.to_string(),
            actor.to_string(),
            detail.to_string(),
            now.to_string()
        ],
    )
    .await
    .map_err(|e| db_err("insert_event", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_sql_scoped_vs_all() {
        let one = clear_share_tokens_sql(Some("note_0123456789abcdef"));
        assert!(one.contains("WHERE note_id = ?1"), "{one}");
        let all = clear_share_tokens_sql(None);
        assert_eq!(all, "DELETE FROM note_share_tokens");
        assert!(!all.contains("WHERE") && !all.contains("?1"));
    }
}
