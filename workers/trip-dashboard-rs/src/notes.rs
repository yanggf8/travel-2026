use crate::{render, turso};
use note_core::{is_image_id, is_note_id, is_note_token, ImageKind, NOTE_PUBLIC_COLUMNS};
use std::collections::{BTreeMap, HashMap};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use worker::*;

const TEXT_FORM_MAX: u64 = 65_536;
/// Single-photo upload cap (12 MiB, plan Global Constraints). Checked against
/// the request's declared Content-Length BEFORE `form_data()` reads the body,
/// and again on the actual decoded bytes (a lying header must not smuggle a
/// larger file in).
const IMAGE_FORM_MAX: u64 = 12_582_912;
/// The multipart framing the upload request cap tolerates on top of the photo
/// bytes: boundary lines, part headers, the CSRF field and the filename. Named
/// allowance so the request cap stays "photo limit + allowance" — a photo of
/// exactly IMAGE_FORM_MAX (plus its framing) must still be accepted.
const MULTIPART_ALLOWANCE: u64 = 16_384;
/// The Content-Length cap for one photo-upload request.
const UPLOAD_REQUEST_MAX: u64 = IMAGE_FORM_MAX + MULTIPART_ALLOWANCE;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoteForm {
    pub id: String,
    pub title: String,
    pub body: String,
    pub purpose: String,
    pub place_name: String,
    pub taken_on: String,
    pub attached_plan_id: String,
    pub attached_destination: String,
    pub attached_day_number: String,
    pub form_nonce: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Precheck { Owner, ExpiredRefill, Reject(u16) }

pub fn precheck(is_owner: bool, has_session_cookie: bool, action: &str, method_is_post: bool, content_length: Option<u64>, max: u64) -> Precheck {
    if !is_owner {
        return if has_session_cookie && method_is_post && matches!(action, "create" | "update") && content_length.is_some_and(|n| n <= max) {
            Precheck::ExpiredRefill
        } else { Precheck::Reject(404) };
    }
    if !method_is_post { return Precheck::Reject(405); }
    if content_length.is_some_and(|n| n > max) { return Precheck::Reject(413); }
    Precheck::Owner
}

/// Content-Length gate for the photo-upload route — runs BEFORE `form_data()`
/// reads a single byte. The RAW header string reaches the gate so a missing or
/// unparsable Content-Length is rejected (400) instead of letting an unbounded
/// multipart body into memory; a declared length over `UPLOAD_REQUEST_MAX` is
/// a 413. The post-read check on the actual decoded bytes stays as the second
/// gate (a lying-but-plausible header must not smuggle a larger file in).
pub fn upload_precheck(content_length: Option<&str>) -> Precheck {
    match content_length.and_then(|s| s.parse::<u64>().ok()) {
        Some(n) if n > UPLOAD_REQUEST_MAX => Precheck::Reject(413),
        Some(_) => Precheck::Owner,
        None => Precheck::Reject(400),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PostOutcome { Redirect(String), Refill { form: NoteForm, error_key: &'static str }, Status(u16) }

pub fn decide_note_post(action: &str, form: &NoteForm, csrf_ok: bool) -> PostOutcome {
    if !csrf_ok { return PostOutcome::Status(403); }
    if !matches!(action, "create" | "update" | "delete") { return PostOutcome::Status(404); }
    if action != "delete" && (form.title.trim().is_empty() || form.form_nonce.is_empty()) {
        return PostOutcome::Refill { form: refill_form(action, form), error_key: "noteRequired" };
    }
    if action == "create" { PostOutcome::Redirect(format!("/notes/{}/edit", form.id)) }
    else if action == "update" { PostOutcome::Redirect(format!("/notes/{}/edit", form.id)) }
    else { PostOutcome::Redirect("/notes".into()) }
}

/// Refill form for a rejected POST. A failed `create` never inserted a row, so
/// the re-rendered form must post back to /notes/new with the create-bound
/// CSRF and the original nonce — posting to /notes/<id>/edit for the
/// not-yet-existing id would bind a different CSRF token and the corrected
/// resubmission would be rejected. `update`/`delete` keep the id.
pub fn refill_form(action: &str, form: &NoteForm) -> NoteForm {
    let mut refill = form.clone();
    if action == "create" { refill.id.clear(); }
    refill
}

pub struct NoteCsrf { secret: String, session_cookie: String }
impl NoteCsrf {
    pub fn new(secret: &str, session_cookie: &str) -> Self { Self { secret: secret.into(), session_cookie: session_cookie.into() } }
    fn sign(&self, msg: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.secret.as_bytes()).expect("HMAC key");
        mac.update(msg.as_bytes());
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    }
    pub fn create(&self) -> String { self.sign(&format!("note:create:{}", self.session_cookie)) }
    pub fn action(&self, action: &str, note_id: &str) -> String { self.sign(&format!("note:{action}:{note_id}:{}", self.session_cookie)) }
    pub fn image(&self, action: &str, note_id: &str, image_id: &str) -> String { self.sign(&format!("note:{action}:{note_id}:{image_id}:{}", self.session_cookie)) }
    /// Stop-all has no note id to bind: the action + session alone scope it.
    pub fn stop_all(&self) -> String { self.sign(&format!("note:share-stop-all:{}", self.session_cookie)) }
    pub fn verify(&self, expected: &str, got: &str) -> bool {
        let Ok(expected) = URL_SAFE_NO_PAD.decode(expected) else { return false; };
        let Ok(got) = URL_SAFE_NO_PAD.decode(got) else { return false; };
        if expected.len() != got.len() { return false; }
        let mut diff = 0u8;
        for (a,b) in expected.iter().zip(got.iter()) { diff |= a ^ b; }
        diff == 0
    }
}

pub fn render_expired_refill(form: &NoteForm, lang: &str) -> String {
    render::notes::render_expired_refill(&form.title, &form.body, lang)
}

#[derive(Debug, PartialEq, Eq)]
pub enum NoteRoute {
    OwnerList,
    OwnerNew,
    OwnerEdit(String),
    OwnerPost(String),
    /// POST /notes/share-stop-all — revoke every note's active tokens at once.
    OwnerStopAll,
    /// POST /notes/<id>/image — one multipart photo upload.
    OwnerUpload(String),
    /// POST /notes/<id>/image/<image_id>/{delete,up,down}.
    OwnerImagePost(String, String, ImageOp),
    /// POST /notes/<id>/{share-preview,share-create,share-deactivate}.
    OwnerSharePost(String, ShareAction),
    SharePage(String),
    ShareImage(String, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageOp {
    Delete,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareAction {
    Preview,
    Create,
    Deactivate,
}

#[derive(Debug, PartialEq, Eq)]
pub enum NoteAccess {
    Allow,
    NotFound,
}

pub struct AccessRow {
    pub token_note_id: Option<String>,
    pub token_status: Option<String>,
    pub note_deleted: bool,
    pub image_exists: bool,
}

pub fn is_note_path(path: &str) -> bool {
    path == "/notes" || path.starts_with("/notes/") || path.starts_with("/n/")
}

pub fn parse_note_route(path: &str) -> Option<NoteRoute> {
    if path == "/notes" {
        return Some(NoteRoute::OwnerList);
    }
    if path == "/notes/new" {
        return Some(NoteRoute::OwnerNew);
    }
    if path == "/notes/share-stop-all" {
        return Some(NoteRoute::OwnerStopAll);
    }
    if let Some(rest) = path.strip_prefix("/notes/") {
        let segments: Vec<_> = rest.split('/').collect();
        match segments.as_slice() {
            [id, "edit"] if is_note_id(id) => return Some(NoteRoute::OwnerEdit((*id).into())),
            [id, "delete"] if is_note_id(id) => return Some(NoteRoute::OwnerPost((*id).into())),
            [id, "image"] if is_note_id(id) => return Some(NoteRoute::OwnerUpload((*id).into())),
            [id, "image", image, op]
                if is_note_id(id) && is_image_id(image) =>
            {
                let op = match *op {
                    "delete" => ImageOp::Delete,
                    "up" => ImageOp::Up,
                    "down" => ImageOp::Down,
                    _ => return None,
                };
                return Some(NoteRoute::OwnerImagePost((*id).into(), (*image).into(), op));
            }
            [id, action] if is_note_id(id) => {
                let action = match *action {
                    "share-preview" => ShareAction::Preview,
                    "share-create" => ShareAction::Create,
                    "share-deactivate" => ShareAction::Deactivate,
                    _ => return None,
                };
                return Some(NoteRoute::OwnerSharePost((*id).into(), action));
            }
            _ => {}
        }
        return None;
    }
    let rest = path.strip_prefix("/n/")?;
    let segments: Vec<_> = rest.split('/').collect();
    match segments.as_slice() {
        [id] if is_note_id(id) => Some(NoteRoute::SharePage((*id).into())),
        [id, "img", image] if is_note_id(id) && is_image_id(image) => {
            Some(NoteRoute::ShareImage((*id).into(), (*image).into()))
        }
        _ => None,
    }
}

pub fn note_token(query: &HashMap<String, String>) -> Option<&str> {
    query.get("t").map(String::as_str)
}

pub fn decide_note_access(
    route: &NoteRoute,
    is_owner: bool,
    token: Option<&str>,
    row: Option<&AccessRow>,
) -> NoteAccess {
    if !matches!(route, NoteRoute::SharePage(_) | NoteRoute::ShareImage(..)) {
        return if is_owner {
            NoteAccess::Allow
        } else {
            NoteAccess::NotFound
        };
    }
    let Some(row) = row else {
        return NoteAccess::NotFound;
    };
    if row.note_deleted || matches!(route, NoteRoute::ShareImage(_, _) if !row.image_exists) {
        return NoteAccess::NotFound;
    }
    if is_owner {
        return NoteAccess::Allow;
    }
    let id = match route {
        NoteRoute::SharePage(id) | NoteRoute::ShareImage(id, _) => id,
        _ => unreachable!(),
    };
    if token.is_some_and(is_note_token)
        && row.token_status.as_deref() == Some("active")
        && row.token_note_id.as_deref() == Some(id)
    {
        NoteAccess::Allow
    } else {
        NoteAccess::NotFound
    }
}

#[derive(Clone, Copy)]
pub enum NoteResponseKind<'a> {
    SharePage,
    Image(&'a str),
    NotFound,
}

/// Complete final headers: `with_headers` replaces the response's existing set.
pub fn note_header_values(kind: NoteResponseKind<'_>) -> Vec<(&'static str, &str)> {
    let mut headers = NOTE_HEADERS.to_vec();
    match kind {
        NoteResponseKind::SharePage => headers.push(("Content-Type", "text/html; charset=utf-8")),
        NoteResponseKind::Image(content_type) => {
            headers.push(("Content-Type", content_type));
            headers.push(("Content-Disposition", "inline"));
        }
        NoteResponseKind::NotFound => headers.push(("Content-Type", "text/plain; charset=utf-8")),
    }
    headers
}

pub fn note_response_headers(h: &Headers, kind: NoteResponseKind<'_>) -> Result<()> {
    for (key, value) in note_header_values(kind) {
        h.set(key, value)?;
    }
    Ok(())
}

const NOTE_HEADERS: [(&str, &str); 4] = [
    ("Cache-Control", "private, no-store"),
    ("X-Robots-Tag", "noindex"),
    ("Referrer-Policy", "no-referrer"),
    ("X-Content-Type-Options", "nosniff"),
];

fn value<'a>(row: &'a turso::Row, key: &str) -> Option<&'a str> {
    row.get(key).and_then(|v| v.as_str())
}

fn not_found() -> Result<Response> {
    let h = Headers::new();
    note_response_headers(&h, NoteResponseKind::NotFound)?;
    Ok(Response::error("Not found", 404)?.with_headers(h))
}

fn html_response(html: String, status: u16) -> Result<Response> {
    let h = Headers::new();
    note_response_headers(&h, NoteResponseKind::SharePage)?;
    Ok(Response::from_html(html)?.with_status(status).with_headers(h))
}

fn random_hex<const N: usize>() -> Result<String> {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).map_err(|e| Error::RustError(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn form_from_post(data: &FormData, id: &str) -> NoteForm {
    let field = |key| data.get_field(key).unwrap_or_default();
    NoteForm {
        id: id.into(), title: field("title"), body: field("body"), purpose: field("purpose"),
        place_name: field("place_name"), taken_on: field("taken_on"),
        attached_plan_id: field("attached_plan_id"), attached_destination: field("attached_destination"),
        attached_day_number: field("attached_day_number"), form_nonce: field("form_nonce"),
    }
}

fn optional_text(s: &str) -> turso::Arg {
    if s.trim().is_empty() { turso::Arg::Null } else { turso::Arg::Text(s.trim().into()) }
}

/// The content statement, its expected affected-row count, and the `note_events`
/// tail statement for one owner mutation. Pure: no DB, no RNG — the caller
/// supplies ids/nonce, so the SQL shape and the bound arguments are testable.
pub struct BuiltMutation {
    pub content: turso::Stmt,
    pub expected: u64,
    pub event: turso::Stmt,
}

/// The day-attachment existence guard, INSIDE the content statement: a note may
/// only be written when its attachment either is NULL (unattached) or names a
/// row of `days`. Bound to the same parameters the statement writes, so a
/// pre-check SELECT race cannot admit a dangling attachment.
const DAYS_GUARD_SQL: &str = "(? IS NULL OR EXISTS (SELECT 1 FROM days WHERE plan_id = ? AND destination = ? AND day_number = ?))";

/// Build the two-statement mutation sent through `turso::pipeline_txn`:
/// content statement first (expected exactly 1 affected row), `note_events`
/// INSERT as the transaction tail. All form text stays in bound `Arg`s; the
/// SQL never interpolates a field value. The duplicate-submit nonce is
/// recorded in `note_events.detail` and checked in the content WHERE.
pub fn build_note_mutation(
    action: &str,
    id: &str,
    form: &NoteForm,
    actor: &str,
    nonce: &str,
    event_id: &str,
) -> Option<BuiltMutation> {
    let guard_args = |args: &mut Vec<turso::Arg>| {
        // Re-bind the attachment triple for `? IS NULL` and the three guard
        // comparisons; `optional_text` already turned "no attachment" into NULL.
        args.push(optional_text(&form.attached_plan_id));
        args.push(optional_text(&form.attached_plan_id));
        args.push(optional_text(&form.attached_destination));
        args.push(if form.attached_day_number.is_empty() { turso::Arg::Null } else { turso::Arg::Int(form.attached_day_number.parse().unwrap_or(0)) });
    };
    let content = match action {
        "create" => {
            let mut args = vec![turso::Arg::Text(id.into())];
            args.push(optional_text(&form.attached_plan_id));
            args.push(optional_text(&form.attached_destination));
            args.push(if form.attached_day_number.is_empty() { turso::Arg::Null } else { turso::Arg::Int(form.attached_day_number.parse().unwrap_or(0)) });
            args.push(turso::Arg::Text(form.title.trim().into()));
            args.push(turso::Arg::Text(form.body.clone()));
            args.push(optional_text(&form.purpose));
            args.push(optional_text(&form.place_name));
            args.push(optional_text(&form.taken_on));
            args.push(turso::Arg::Text(actor.into())); // created_by
            args.push(turso::Arg::Text(actor.into())); // updated_by
            args.push(turso::Arg::Text(nonce.into())); // duplicate-submit guard
            guard_args(&mut args);
            turso::Stmt {
                sql: format!(
                    "INSERT INTO notes (id, attached_plan_id, attached_destination, attached_day_number, title, body, purpose, place_name, taken_on, created_at, created_by, updated_at, updated_by) \
                     SELECT ?,?,?,?,?,?,?,?,?,CURRENT_TIMESTAMP,?,CURRENT_TIMESTAMP,? \
                     WHERE NOT EXISTS (SELECT 1 FROM note_events WHERE detail = ?) AND {DAYS_GUARD_SQL}"
                ),
                args,
            }
        }
        "update" => {
            let mut args = vec![];
            args.push(optional_text(&form.attached_plan_id));
            args.push(optional_text(&form.attached_destination));
            args.push(if form.attached_day_number.is_empty() { turso::Arg::Null } else { turso::Arg::Int(form.attached_day_number.parse().unwrap_or(0)) });
            args.push(turso::Arg::Text(form.title.trim().into()));
            args.push(turso::Arg::Text(form.body.clone()));
            args.push(optional_text(&form.purpose));
            args.push(optional_text(&form.place_name));
            args.push(optional_text(&form.taken_on));
            args.push(turso::Arg::Text(actor.into())); // updated_by
            args.push(turso::Arg::Text(id.into()));    // WHERE id = ?
            args.push(turso::Arg::Text(id.into()));    // duplicate-submit guard note_id
            args.push(turso::Arg::Text(nonce.into())); // duplicate-submit guard detail
            guard_args(&mut args);
            turso::Stmt {
                sql: format!(
                    "UPDATE notes SET attached_plan_id=?, attached_destination=?, attached_day_number=?, title=?, body=?, purpose=?, place_name=?, taken_on=?, updated_at=CURRENT_TIMESTAMP, updated_by=? \
                     WHERE id=? AND deleted_at IS NULL \
                     AND NOT EXISTS (SELECT 1 FROM note_events WHERE note_id=? AND detail=?) \
                     AND {DAYS_GUARD_SQL}"
                ),
                args,
            }
        }
        "delete" => turso::Stmt {
            sql: "UPDATE notes SET deleted_at=CURRENT_TIMESTAMP, deleted_by=? WHERE id=? AND deleted_at IS NULL".into(),
            args: vec![turso::Arg::Text(actor.into()), turso::Arg::Text(id.into())],
        },
        _ => return None,
    };
    let event = turso::Stmt {
        sql: "INSERT INTO note_events (event_id, note_id, action, actor, detail, at) VALUES (?,?,?,?,?,CURRENT_TIMESTAMP)".into(),
        args: vec![
            turso::Arg::Text(event_id.into()),
            turso::Arg::Text(id.into()),
            turso::Arg::Text(action.into()),
            turso::Arg::Text(actor.into()),
            turso::Arg::Text(nonce.into()),
        ],
    };
    Some(BuiltMutation { content, expected: 1, event })
}

// ── T7: photo upload/delete/reorder, share flow (pure builders) ──

/// The note-must-exist-and-not-be-soft-deleted guard carried INSIDE every
/// note_images write, so a note deleted between the read and the write yields
/// 0 affected rows (→ RowMismatch → 404) instead of an orphan image row.
const NOTE_LIVE_GUARD_SQL: &str = "EXISTS (SELECT 1 FROM notes WHERE id = ? AND deleted_at IS NULL)";

/// A scrubbed upload, ready to store. Pure: no R2, no RNG — scrubbing happens
/// (and fails closed) before any I/O, so a rejected photo never reaches a put.
#[derive(Debug)]
pub struct UploadPlan {
    pub kind: ImageKind,
    pub bytes: Vec<u8>,
}

/// Scrub an uploaded photo's bytes through the shared note-core whitelist.
/// Errors are i18n keys the refill page renders.
pub fn plan_upload(bytes: &[u8]) -> Result<UploadPlan, &'static str> {
    match note_core::scrub(bytes) {
        Ok((kind, bytes)) => Ok(UploadPlan { kind, bytes }),
        Err(note_core::ScrubError::UnsupportedFormat) => Err("noteUnsupportedImage"),
        Err(note_core::ScrubError::Malformed(_)) => Err("noteImageMalformed"),
    }
}

/// The R2 object key — assembled only from validated ids via note-core, never
/// from request path parts.
pub fn upload_key(note_id: &str, image_id: &str, kind: ImageKind) -> String {
    note_core::r2_key(note_id, image_id, kind)
}

/// Same shape as the CLI's `share_token.rs:574` fingerprint so both surfaces
/// log identical `note_events.detail` text for a token.
pub fn token_fingerprint(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 12 {
        return token.to_string();
    }
    let head: String = chars.iter().take(6).collect();
    let tail: String = chars[chars.len() - 6..].iter().collect();
    format!("{head}...{tail}")
}

/// One image row as read for the reorder UI (already ordered).
#[derive(Debug, PartialEq, Eq)]
pub struct ImageOrder {
    pub image_id: String,
    pub sort_order: i64,
}

/// The neighbor the target swaps with, and the two sort_orders to exchange.
#[derive(Debug, PartialEq, Eq)]
pub struct ImageSwap {
    pub neighbor_id: String,
    pub target_order: i64,
    pub neighbor_order: i64,
}

/// Pure: which row does `target` swap with when moved `up`? `None` = no-op
/// (first row up / last row down / unknown id) — the caller redirects without
/// touching the DB.
pub fn plan_image_move(images: &[ImageOrder], target: &str, up: bool) -> Option<ImageSwap> {
    let idx = images.iter().position(|i| i.image_id == target)?;
    let neighbor = if up { idx.checked_sub(1)? } else { idx.checked_add(1)? };
    let neighbor = images.get(neighbor)?;
    Some(ImageSwap {
        neighbor_id: neighbor.image_id.clone(),
        target_order: images[idx].sort_order,
        neighbor_order: neighbor.sort_order,
    })
}

/// A multi-statement note write for `pipeline_txn`: content statements first
/// (each with its own expected affected-row count), `note_events` inserts as
/// the transaction tail.
pub struct BuiltWrite {
    pub content: Vec<turso::Stmt>,
    pub expected: Vec<u64>,
    pub events: Vec<turso::Stmt>,
}

fn event_stmt(event_id: &str, note_id: &str, action: &str, actor: &str, detail: &str) -> turso::Stmt {
    turso::Stmt {
        sql: "INSERT INTO note_events (event_id, note_id, action, actor, detail, at) VALUES (?,?,?,?,?,CURRENT_TIMESTAMP)".into(),
        args: vec![
            turso::Arg::Text(event_id.into()),
            turso::Arg::Text(note_id.into()),
            turso::Arg::Text(action.into()),
            turso::Arg::Text(actor.into()),
            turso::Arg::Text(detail.into()),
        ],
    }
}

/// Upload: insert the image row with the next sort_order, guarded by the note
/// still existing and not being soft-deleted.
pub fn build_image_add(
    note_id: &str,
    image_id: &str,
    r2_key: &str,
    content_type: &str,
    size_bytes: i64,
    actor: &str,
    event_id: &str,
) -> BuiltWrite {
    BuiltWrite {
        content: vec![turso::Stmt {
            sql: format!(
                "INSERT INTO note_images (note_id, image_id, sort_order, r2_key, content_type, size_bytes, caption, created_at, created_by) \
                 SELECT ?, ?, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM note_images WHERE note_id = ?), ?, ?, ?, NULL, CURRENT_TIMESTAMP, ? \
                 WHERE {NOTE_LIVE_GUARD_SQL}"
            ),
            args: vec![
                turso::Arg::Text(note_id.into()),
                turso::Arg::Text(image_id.into()),
                turso::Arg::Text(note_id.into()),
                turso::Arg::Text(r2_key.into()),
                turso::Arg::Text(content_type.into()),
                turso::Arg::Int(size_bytes),
                turso::Arg::Text(actor.into()),
                turso::Arg::Text(note_id.into()),
            ],
        }],
        expected: vec![1],
        events: vec![event_stmt(event_id, note_id, "image_add", actor, &format!("image_id={image_id} key={r2_key}"))],
    }
}

/// Delete one image row. The caller deletes the R2 object FIRST (Global
/// Constraints: read key → delete R2 → only then remove the row + event).
pub fn build_image_delete(
    note_id: &str,
    image_id: &str,
    r2_key: &str,
    actor: &str,
    event_id: &str,
) -> BuiltWrite {
    BuiltWrite {
        content: vec![turso::Stmt {
            sql: format!("DELETE FROM note_images WHERE note_id = ? AND image_id = ? AND {NOTE_LIVE_GUARD_SQL}"),
            args: vec![
                turso::Arg::Text(note_id.into()),
                turso::Arg::Text(image_id.into()),
                turso::Arg::Text(note_id.into()),
            ],
        }],
        expected: vec![1],
        events: vec![event_stmt(event_id, note_id, "image_delete", actor, &format!("image_id={image_id} key={r2_key}"))],
    }
}

/// Swap the target row with its neighbor: two UPDATEs, each expected to touch
/// exactly 1 row. Any 0-row statement (note/image gone mid-flight) rolls the
/// whole swap back — no half-applied ordering.
pub fn build_image_move(
    note_id: &str,
    image_id: &str,
    swap: &ImageSwap,
    up: bool,
    actor: &str,
    event_id: &str,
) -> BuiltWrite {
    let move_sql = format!("UPDATE note_images SET sort_order = ? WHERE note_id = ? AND image_id = ? AND {NOTE_LIVE_GUARD_SQL}");
    BuiltWrite {
        content: vec![
            turso::Stmt {
                sql: move_sql.clone(),
                args: vec![
                    turso::Arg::Int(swap.neighbor_order),
                    turso::Arg::Text(note_id.into()),
                    turso::Arg::Text(image_id.into()),
                    turso::Arg::Text(note_id.into()),
                ],
            },
            turso::Stmt {
                sql: move_sql,
                args: vec![
                    turso::Arg::Int(swap.target_order),
                    turso::Arg::Text(note_id.into()),
                    turso::Arg::Text(swap.neighbor_id.clone()),
                    turso::Arg::Text(note_id.into()),
                ],
            },
        ],
        expected: vec![1, 1],
        events: vec![event_stmt(
            event_id,
            note_id,
            "image_move",
            actor,
            &format!("image_id={image_id} dir={}", if up { "up" } else { "down" }),
        )],
    }
}

/// Mint one share token for a live note.
pub fn build_share_create(note_id: &str, token: &str, actor: &str, event_id: &str) -> BuiltWrite {
    BuiltWrite {
        content: vec![turso::Stmt {
            sql: format!(
                "INSERT INTO note_share_tokens (token, note_id, created_at, created_by, status) \
                 SELECT ?, ?, CURRENT_TIMESTAMP, ?, 'active' \
                 WHERE {NOTE_LIVE_GUARD_SQL}"
            ),
            args: vec![
                turso::Arg::Text(token.into()),
                turso::Arg::Text(note_id.into()),
                turso::Arg::Text(actor.into()),
                turso::Arg::Text(note_id.into()),
            ],
        }],
        expected: vec![1],
        events: vec![event_stmt(event_id, note_id, "share_create", actor, &format!("token={}", token_fingerprint(token)))],
    }
}

/// Deactivate THIS note's active tokens. `expected_count` comes from the
/// caller's pre-read SELECT, so a concurrent change fails loudly (RowMismatch)
/// instead of half-claiming success.
pub fn build_share_deactivate(note_id: &str, expected_count: u64, actor: &str, event_id: &str) -> BuiltWrite {
    BuiltWrite {
        content: vec![turso::Stmt {
            sql: "UPDATE note_share_tokens SET status='inactive', deactivated_at=CURRENT_TIMESTAMP, deactivated_by=? WHERE note_id=? AND status='active'".into(),
            args: vec![turso::Arg::Text(actor.into()), turso::Arg::Text(note_id.into())],
        }],
        expected: vec![expected_count],
        events: vec![event_stmt(event_id, note_id, "share_deactivate", actor, &format!("tokens={expected_count}"))],
    }
}

/// Stop-all: one UPDATE flips every active token; each affected note still
/// gets its own audit event (detail `tokens=<its count>`).
pub fn build_share_stop_all(
    per_note: &[(String, u64)],
    actor: &str,
    event_ids: &[String],
) -> BuiltWrite {
    let total: u64 = per_note.iter().map(|(_, c)| *c).sum();
    BuiltWrite {
        content: vec![turso::Stmt {
            sql: "UPDATE note_share_tokens SET status='inactive', deactivated_at=CURRENT_TIMESTAMP, deactivated_by=? WHERE status='active'".into(),
            args: vec![turso::Arg::Text(actor.into())],
        }],
        expected: vec![total],
        events: per_note
            .iter()
            .zip(event_ids)
            .map(|((note_id, count), event_id)| {
                event_stmt(event_id, note_id, "share_deactivate", actor, &format!("tokens={count}"))
            })
            .collect(),
    }
}

fn form_valid(form: &NoteForm) -> bool {
    if form.title.trim().is_empty() || form.form_nonce.len() != 32 || !form.form_nonce.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) { return false; }
    if !form.taken_on.is_empty() && (form.taken_on.len() != 10 || !form.taken_on.bytes().enumerate().all(|(i,b)| if i==4 || i==7 { b==b'-' } else { b.is_ascii_digit() })) { return false; }
    let attachment = [&form.attached_plan_id, &form.attached_destination, &form.attached_day_number];
    if attachment.iter().any(|s| !s.is_empty()) && attachment.iter().any(|s| s.is_empty()) { return false; }
    form.attached_day_number.is_empty() || form.attached_day_number.parse::<i64>().is_ok_and(|n| n > 0)
}

fn note_form_row(id: &str, row: &turso::Row, nonce: String) -> NoteForm {
    let s = |key| value(row, key).unwrap_or("").to_owned();
    NoteForm { id: id.into(), title: s("title"), body: s("body"), purpose: s("purpose"), place_name: s("place_name"), taken_on: s("taken_on"), attached_plan_id: s("attached_plan_id"), attached_destination: s("attached_destination"), attached_day_number: row.get("attached_day_number").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default(), form_nonce: nonce }
}

/// The precheck action name for a route — text actions get the 64 KiB text-form
/// cap in the generic precheck; the photo upload is gated by `upload_precheck`
/// (photo limit + multipart allowance) on its declared Content-Length.
fn route_action(route: &NoteRoute) -> &'static str {
    match route {
        NoteRoute::OwnerNew => "create",
        NoteRoute::OwnerEdit(_) => "update",
        NoteRoute::OwnerPost(_) => "delete",
        NoteRoute::OwnerList => "list",
        NoteRoute::OwnerStopAll => "share-stop-all",
        NoteRoute::OwnerUpload(_) => "image_add",
        NoteRoute::OwnerImagePost(_, _, op) => match op {
            ImageOp::Delete => "image_delete",
            ImageOp::Up | ImageOp::Down => "image_move",
        },
        NoteRoute::OwnerSharePost(_, action) => match action {
            ShareAction::Preview => "share-preview",
            ShareAction::Create => "share-create",
            ShareAction::Deactivate => "share-deactivate",
        },
        _ => "list",
    }
}

async fn handle_owner(req: &mut Request, env: &Env, route: &NoteRoute, is_owner: bool, login: Option<&str>, lang: &str) -> Result<Response> {
    let is_post = req.method() == Method::Post;
    let action = route_action(route);
    let max = TEXT_FORM_MAX;
    let cookie = req.headers().get("Cookie")?.unwrap_or_default();
    let session_cookie = cookie.split(';').map(str::trim).find_map(|p| p.strip_prefix("__Host-td_session=")).unwrap_or("");
    let content_length_header = req.headers().get("Content-Length")?;
    let content_length = content_length_header.as_deref().and_then(|s| s.parse::<u64>().ok());
    if is_post {
        if let NoteRoute::OwnerUpload(id) = route {
            // The photo upload must DECLARE a parsable size at or under the
            // request cap BEFORE `form_data()` reads a byte — a missing or
            // unparsable Content-Length would otherwise read an unbounded
            // multipart body into memory (Codex review HIGH). The per-file
            // photo cap re-checks the decoded bytes after the read.
            if !is_owner { return not_found(); }
            match upload_precheck(content_length_header.as_deref()) {
                Precheck::Owner => {}
                Precheck::Reject(status) => {
                    let key = if status == 413 { "notePhotoTooLarge" } else { "noteBadRequest" };
                    return html_response(render::notes::render_owner_error(&format!("/notes/{id}/edit"), key, lang), status);
                }
                Precheck::ExpiredRefill => unreachable!("upload_precheck never returns ExpiredRefill"),
            }
        } else {
            match precheck(is_owner, !session_cookie.is_empty(), action, true, content_length, max) {
                Precheck::Reject(404) => return not_found(),
                Precheck::Reject(status) => {
                    // Over-limit POST (413): an owner error page in the request
                    // language with a way back — not bare English text.
                    let back = match route {
                        NoteRoute::OwnerNew => "/notes/new".into(),
                        NoteRoute::OwnerEdit(id) | NoteRoute::OwnerPost(id) => format!("/notes/{id}/edit"),
                        _ => "/notes".into(),
                    };
                    return html_response(render::notes::render_owner_error(&back, "noteTooLarge", lang), status);
                }
                Precheck::ExpiredRefill => {
                    let data = req.form_data().await?;
                    let form = form_from_post(&data, "");
                    return html_response(render_expired_refill(&form, lang), 401);
                }
                Precheck::Owner => {}
            }
        }
    } else if !is_owner { return not_found(); }
    if !is_post && req.method() != Method::Get { return Response::error("Method not allowed", 405); }
    let origin = req.url()?.origin().ascii_serialization();
    let url = env.secret("TURSO_URL")?.to_string();
    let read_token = env.secret("TURSO_TOKEN")?.to_string();
    let csrf = NoteCsrf::new(&env.secret("SESSION_SECRET")?.to_string(), session_cookie);
    if !is_post {
        match route {
            NoteRoute::OwnerList => {
                let rows = turso::pipeline_args(&url, &read_token, &[turso::Stmt { sql: "SELECT id, title, purpose FROM notes WHERE deleted_at IS NULL ORDER BY created_at DESC, id DESC".into(), args: vec![] }]).await?;
                let notes = rows[0].rows.iter().filter_map(|r| Some((value(r,"id")?.to_owned(), value(r,"title").unwrap_or("").to_owned(), value(r,"purpose").unwrap_or("").to_owned()))).collect::<Vec<_>>();
                return html_response(render::notes::render_owner_list(&notes, &csrf.stop_all(), lang), 200);
            }
            NoteRoute::OwnerNew => {
                let form = NoteForm { form_nonce: random_hex::<16>()?, ..Default::default() };
                return html_response(render::notes::render_owner_form(&form, &csrf.create(), None, None, lang), 200);
            }
            NoteRoute::OwnerEdit(id) => {
                return owner_edit_page(&url, &read_token, id, &csrf, None, lang).await;
            }
            // Every other route is POST-only.
            _ => return Response::error("Method not allowed", 405),
        }
    }
    if action == "list" { return Response::error("Method not allowed", 405); }
    let write_token = env.secret("TURSO_WRITE_TOKEN")?.to_string();
    // New-in-T7 POST routes dispatch to their own flows; the text note forms
    // (create/update/delete) keep the T6 path below.
    match route {
        NoteRoute::OwnerStopAll => return stop_all_flow(req, &url, &write_token, &csrf, login, lang).await,
        NoteRoute::OwnerUpload(id) => return upload_flow(req, env, &url, &write_token, &csrf, id, login, lang).await,
        NoteRoute::OwnerImagePost(id, image, op) => return image_flow(req, env, &url, &read_token, &write_token, &csrf, id, image, *op, login, lang).await,
        NoteRoute::OwnerSharePost(id, share) => return share_flow(req, &url, &read_token, &write_token, &csrf, id, *share, login, lang, &origin).await,
        _ => {}
    }
    let id = match route { NoteRoute::OwnerEdit(id) | NoteRoute::OwnerPost(id) => id.clone(), NoteRoute::OwnerNew => format!("note_{}", random_hex::<8>()?), _ => unreachable!() };
    let data = req.form_data().await?;
    let form = form_from_post(&data, &id);
    let expected = if action == "create" { csrf.create() } else { csrf.action(action, &id) };
    let csrf_ok = csrf.verify(&expected, &data.get_field("csrf").unwrap_or_default());
    match decide_note_post(action, &form, csrf_ok) {
        PostOutcome::Status(status) => {
            let back = if action == "create" { "/notes/new".to_string() } else { format!("/notes/{id}/edit") };
            return html_response(render::notes::render_owner_error(&back, "noteFormExpired", lang), status);
        }
        PostOutcome::Refill { form, error_key } => return html_response(render::notes::render_owner_form(&form, &expected, None, Some(error_key), lang), 400),
        PostOutcome::Redirect(_) => {}
    }
    if action != "delete" && !form_valid(&form) {
        return html_response(render::notes::render_owner_form(&refill_form(action, &form), &expected, None, Some("noteRequired"), lang), 400);
    }
    if !form.attached_plan_id.is_empty() {
        let day = form.attached_day_number.parse::<i64>().unwrap_or(0);
        let found = turso::pipeline_args(&url, &read_token, &[turso::Stmt {
            sql: "SELECT 1 FROM days WHERE plan_id=? AND destination=? AND day_number=? LIMIT 1".into(),
            args: vec![turso::Arg::Text(form.attached_plan_id.clone()), turso::Arg::Text(form.attached_destination.clone()), turso::Arg::Int(day)],
        }]).await?;
        if found[0].rows.is_empty() {
            return html_response(render::notes::render_owner_form(&refill_form(action, &form), &expected, None, Some("noteInvalidDay"), lang), 400);
        }
    }
    let nonce = if action == "delete" { random_hex::<16>()? } else { form.form_nonce.clone() };
    let event_id = random_hex::<16>()?;
    let actor = login.unwrap_or("");
    let Some(built) = build_note_mutation(action, &id, &form, actor, &nonce, &event_id) else {
        return Response::error("Invalid request", 404);
    };
    match turso::pipeline_txn(&url, &write_token, &[built.content], &[built.expected], &[built.event]).await {
        Ok(_) => {
            let location = if action == "delete" { "/notes".into() } else { format!("/notes/{id}/edit") };
            redirect_to(&location)
        }
        Err(turso::TxnError::RowMismatch { .. }) => not_found(),
        Err(e) => Err(Error::RustError(e.to_string())),
    }
}

fn redirect_to(location: &str) -> Result<Response> {
    let h = Headers::new();
    h.set("Location", location)?;
    h.set("Cache-Control", "private, no-store")?;
    Ok(Response::empty()?.with_status(303).with_headers(h))
}

/// The note's integer column read back from Turso (string or number JSON).
fn int_value(row: &turso::Row, key: &str) -> Option<i64> {
    row.get(key).and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
}

/// The full edit page: text form + photo section + share controls. Re-served
/// (with the error banner) when an upload fails scrubbing or the form is empty.
async fn owner_edit_page(
    url: &str,
    read_token: &str,
    id: &str,
    csrf: &NoteCsrf,
    error: Option<&'static str>,
    lang: &str,
) -> Result<Response> {
    let rows = turso::pipeline_args(url, read_token, &[turso::Stmt { sql: "SELECT title, body, purpose, place_name, taken_on, attached_plan_id, attached_destination, attached_day_number FROM notes WHERE id = ? AND deleted_at IS NULL".into(), args: vec![turso::Arg::Text(id.into())] }]).await?;
    let Some(row) = rows.first().and_then(|r| r.rows.first()) else { return not_found(); };
    let form = note_form_row(id, row, random_hex::<16>()?);
    let images = load_image_orders(url, read_token, id).await?;
    let image_uis = images
        .iter()
        .map(|img| render::notes::ImageUi {
            image_id: img.image_id.clone(),
            csrf_up: csrf.image("image_move_up", id, &img.image_id),
            csrf_down: csrf.image("image_move_down", id, &img.image_id),
            csrf_delete: csrf.image("image_delete", id, &img.image_id),
        })
        .collect::<Vec<_>>();
    let active = active_share_count(url, read_token, id).await?;
    let images_html = render::notes::render_owner_images(id, &image_uis, &csrf.action("image_add", id), lang);
    let share_html = render::notes::render_owner_share(id, active, &csrf.action("share-preview", id), &csrf.action("share-deactivate", id), lang);
    let status = if error.is_some() { 400 } else { 200 };
    html_response(
        render::notes::render_owner_edit(&form, &csrf.action("update", id), Some(&csrf.action("delete", id)), &images_html, &share_html, error, lang),
        status,
    )
}

/// One image row per line, ordered — the reorder plan's input.
async fn load_image_orders(url: &str, read_token: &str, note_id: &str) -> Result<Vec<ImageOrder>> {
    let rows = turso::pipeline_args(url, read_token, &[turso::Stmt {
        sql: format!("SELECT i.image_id, i.sort_order FROM note_images i WHERE i.note_id = ? AND {NOTE_LIVE_GUARD_SQL} ORDER BY i.sort_order, i.image_id"),
        args: vec![turso::Arg::Text(note_id.into()), turso::Arg::Text(note_id.into())],
    }]).await?;
    Ok(rows
        .first()
        .map(|r| {
            r.rows
                .iter()
                .filter_map(|row| {
                    let image_id = value(row, "image_id")?;
                    if !is_image_id(image_id) { return None; }
                    Some(ImageOrder { image_id: image_id.to_owned(), sort_order: int_value(row, "sort_order").unwrap_or(0) })
                })
                .collect()
        })
        .unwrap_or_default())
}

async fn active_share_count(url: &str, read_token: &str, note_id: &str) -> Result<u64> {
    let rows = turso::pipeline_args(url, read_token, &[turso::Stmt {
        sql: "SELECT COUNT(*) AS n FROM note_share_tokens WHERE note_id = ? AND status = 'active'".into(),
        args: vec![turso::Arg::Text(note_id.into())],
    }]).await?;
    Ok(rows.first().and_then(|r| r.rows.first()).and_then(|row| int_value(row, "n")).map(|n| n.max(0) as u64).unwrap_or(0))
}

/// Upload one photo: CSRF → read the file → scrub (fail closed) → R2 put →
/// DB insert + event, compensated by an R2 delete when the DB write fails.
async fn upload_flow(
    req: &mut Request,
    env: &Env,
    url: &str,
    write_token: &str,
    csrf: &NoteCsrf,
    id: &str,
    login: Option<&str>,
    lang: &str,
) -> Result<Response> {
    let data = req.form_data().await?;
    let expected = csrf.action("image_add", id);
    if !csrf.verify(&expected, &data.get_field("csrf").unwrap_or_default()) {
        return html_response(render::notes::render_owner_error(&format!("/notes/{id}/edit"), "noteFormExpired", lang), 403);
    }
    let Some(worker::FormEntry::File(file)) = data.get("image") else {
        let read_token = env.secret("TURSO_TOKEN")?.to_string();
        return owner_edit_page(url, &read_token, id, csrf, Some("notePhotoRequired"), lang).await;
    };
    let bytes = file.bytes().await?;
    // The Content-Length precheck already capped the declared size; this caps
    // what actually arrived (absent/lying headers must not smuggle more in).
    if bytes.len() as u64 > IMAGE_FORM_MAX {
        return html_response(render::notes::render_owner_error(&format!("/notes/{id}/edit"), "notePhotoTooLarge", lang), 413);
    }
    let plan = match plan_upload(&bytes) {
        Ok(plan) => plan,
        Err(key) => {
            let read_token = env.secret("TURSO_TOKEN")?.to_string();
            return owner_edit_page(url, &read_token, id, csrf, Some(key), lang).await;
        }
    };
    // One targeted SELECT before touching R2: the note must still be live.
    let live = turso::pipeline_args(url, write_token, &[turso::Stmt {
        sql: "SELECT 1 FROM notes WHERE id = ? AND deleted_at IS NULL".into(),
        args: vec![turso::Arg::Text(id.into())],
    }]).await?;
    if live.first().map(|r| r.rows.is_empty()).unwrap_or(true) {
        return not_found();
    }
    let image_id = random_hex::<8>()?;
    let key = upload_key(id, &image_id, plan.kind);
    let bucket = env.bucket("NOTES")?;
    bucket
        .put(&key, worker::Data::Bytes(plan.bytes.clone()))
        .http_metadata(worker::HttpMetadata {
            content_type: Some(plan.kind.content_type().into()),
            ..Default::default()
        })
        .execute()
        .await?;
    let actor = login.unwrap_or("");
    let event_id = random_hex::<16>()?;
    let built = build_image_add(id, &image_id, &key, plan.kind.content_type(), plan.bytes.len() as i64, actor, &event_id);
    match turso::pipeline_txn(url, write_token, &built.content, &built.expected, &built.events).await {
        Ok(_) => redirect_to(&format!("/notes/{id}/edit")),
        Err(turso::TxnError::RowMismatch { .. }) => {
            // DB row did not land (note gone mid-flight) — remove the orphan object.
            let _ = bucket.delete(&key).await;
            not_found()
        }
        Err(e) => {
            let _ = bucket.delete(&key).await;
            Err(Error::RustError(e.to_string()))
        }
    }
}

/// Delete / move one photo. Delete order is fixed (Global Constraints): read
/// the key → R2 delete first (missing object = success) → txn removes the row
/// and writes the event. R2 failure keeps the row for a later retry.
async fn image_flow(
    req: &mut Request,
    env: &Env,
    url: &str,
    read_token: &str,
    write_token: &str,
    csrf: &NoteCsrf,
    note_id: &str,
    image_id: &str,
    op: ImageOp,
    login: Option<&str>,
    lang: &str,
) -> Result<Response> {
    let data = req.form_data().await?;
    let expected = match op {
        ImageOp::Delete => csrf.image("image_delete", note_id, image_id),
        ImageOp::Up => csrf.image("image_move_up", note_id, image_id),
        ImageOp::Down => csrf.image("image_move_down", note_id, image_id),
    };
    if !csrf.verify(&expected, &data.get_field("csrf").unwrap_or_default()) {
        return html_response(render::notes::render_owner_error(&format!("/notes/{note_id}/edit"), "noteFormExpired", lang), 403);
    }
    let images = load_image_orders(url, read_token, note_id).await?;
    if !images.iter().any(|i| i.image_id == image_id) {
        return not_found();
    }
    let actor = login.unwrap_or("");
    let event_id = random_hex::<16>()?;
    let edit_location = format!("/notes/{note_id}/edit");
    match op {
        ImageOp::Delete => {
            // The R2 key comes from the DB row alone — never assembled from the URL.
            let key_row = turso::pipeline_args(url, read_token, &[turso::Stmt {
                sql: "SELECT r2_key FROM note_images WHERE note_id = ? AND image_id = ?".into(),
                args: vec![turso::Arg::Text(note_id.into()), turso::Arg::Text(image_id.into())],
            }]).await?;
            let Some(key) = key_row.first().and_then(|r| r.rows.first()).and_then(|r| value(r, "r2_key").map(str::to_owned)) else {
                return not_found();
            };
            let bucket = env.bucket("NOTES")?;
            // R2 delete failure → error page, row stays, key retried later.
            bucket.delete(&key).await?;
            let built = build_image_delete(note_id, image_id, &key, actor, &event_id);
            match turso::pipeline_txn(url, write_token, &built.content, &built.expected, &built.events).await {
                Ok(_) => redirect_to(&edit_location),
                Err(turso::TxnError::RowMismatch { .. }) => not_found(),
                Err(e) => Err(Error::RustError(e.to_string())),
            }
        }
        up_down => {
            let up = up_down == ImageOp::Up;
            // First row up / last row down / unknown id: redirect without a write.
            let Some(swap) = plan_image_move(&images, image_id, up) else {
                return redirect_to(&edit_location);
            };
            let built = build_image_move(note_id, image_id, &swap, up, actor, &event_id);
            match turso::pipeline_txn(url, write_token, &built.content, &built.expected, &built.events).await {
                Ok(_) => redirect_to(&edit_location),
                Err(turso::TxnError::RowMismatch { .. }) => not_found(),
                Err(e) => Err(Error::RustError(e.to_string())),
            }
        }
    }
}

/// The share flow: preview (the recipient's exact page), create (mint + show
/// the copyable link), deactivate (revoke this note's links).
async fn share_flow(
    req: &mut Request,
    url: &str,
    read_token: &str,
    write_token: &str,
    csrf: &NoteCsrf,
    id: &str,
    action: ShareAction,
    login: Option<&str>,
    lang: &str,
    origin: &str,
) -> Result<Response> {
    let data = req.form_data().await?;
    let expected = csrf.action(
        match action {
            ShareAction::Preview => "share-preview",
            ShareAction::Create => "share-create",
            ShareAction::Deactivate => "share-deactivate",
        },
        id,
    );
    if !csrf.verify(&expected, &data.get_field("csrf").unwrap_or_default()) {
        return html_response(render::notes::render_owner_error(&format!("/notes/{id}/edit"), "noteFormExpired", lang), 403);
    }
    let actor = login.unwrap_or("");
    let event_id = random_hex::<16>()?;
    match action {
        ShareAction::Preview => {
            // The owner preview = the recipient's page: the same public columns
            // and the same share_article body.
            let note = match load_public_note(url, read_token, id).await? {
                Some(note) => note,
                None => return not_found(),
            };
            let images = load_image_orders(url, read_token, id).await?;
            let image_ids = images.iter().map(|i| i.image_id.clone()).collect::<Vec<_>>();
            html_response(render::notes::render_share_preview(&note, &image_ids, &csrf.action("share-create", id), lang), 200)
        }
        ShareAction::Create => {
            let token = random_hex::<16>()?;
            debug_assert!(is_note_token(&token));
            let built = build_share_create(id, &token, actor, &event_id);
            match turso::pipeline_txn(url, write_token, &built.content, &built.expected, &built.events).await {
                Ok(_) => {
                    let link = format!("{origin}/n/{id}?t={token}");
                    html_response(render::notes::render_share_created(&link, lang), 200)
                }
                Err(turso::TxnError::RowMismatch { .. }) => not_found(),
                Err(e) => Err(Error::RustError(e.to_string())),
            }
        }
        ShareAction::Deactivate => {
            let active = active_share_count(url, read_token, id).await?;
            if active == 0 {
                // Nothing to revoke: no write, no event — back to the editor.
                return redirect_to(&format!("/notes/{id}/edit"));
            }
            let built = build_share_deactivate(id, active, actor, &event_id);
            match turso::pipeline_txn(url, write_token, &built.content, &built.expected, &built.events).await {
                Ok(_) => redirect_to(&format!("/notes/{id}/edit")),
                Err(turso::TxnError::RowMismatch { .. }) => not_found(),
                Err(e) => Err(Error::RustError(e.to_string())),
            }
        }
    }
}

/// One-click revoke of every note's active tokens.
async fn stop_all_flow(
    req: &mut Request,
    url: &str,
    write_token: &str,
    csrf: &NoteCsrf,
    login: Option<&str>,
    lang: &str,
) -> Result<Response> {
    let data = req.form_data().await?;
    if !csrf.verify(&csrf.stop_all(), &data.get_field("csrf").unwrap_or_default()) {
        return html_response(render::notes::render_owner_error(&"/notes".to_string(), "noteFormExpired", lang), 403);
    }
    // Per-note active counts before the write: the UPDATE's expected row count
    // and one audit event per affected note both come from this read.
    let rows = turso::pipeline_args(url, write_token, &[turso::Stmt {
        sql: "SELECT note_id, COUNT(*) AS c FROM note_share_tokens WHERE status = 'active' GROUP BY note_id".into(),
        args: vec![],
    }]).await?;
    let per_note: Vec<(String, u64)> = rows
        .first()
        .map(|r| {
            r.rows
                .iter()
                .filter_map(|row| {
                    let note_id = value(row, "note_id")?;
                    if !is_note_id(note_id) { return None; }
                    Some((note_id.to_owned(), int_value(row, "c").map(|n| n.max(0) as u64).unwrap_or(0)))
                })
                .collect()
        })
        .unwrap_or_default();
    if per_note.is_empty() {
        return redirect_to("/notes");
    }
    let event_ids: Vec<String> = (0..per_note.len()).map(|_| random_hex::<16>()).collect::<Result<Vec<_>>>()?;
    let actor = login.unwrap_or("");
    let built = build_share_stop_all(&per_note, actor, &event_ids);
    match turso::pipeline_txn(url, write_token, &built.content, &built.expected, &built.events).await {
        Ok(_) => redirect_to("/notes"),
        Err(turso::TxnError::RowMismatch { .. }) => not_found(),
        Err(e) => Err(Error::RustError(e.to_string())),
    }
}

/// The public-column note for the owner preview (owner view: live note).
async fn load_public_note(url: &str, read_token: &str, id: &str) -> Result<Option<render::notes::PublicNote>> {
    let results = turso::pipeline_args(url, read_token, &[turso::Stmt {
        sql: format!("SELECT {NOTE_PUBLIC_COLUMNS} FROM notes WHERE id = ? AND deleted_at IS NULL"),
        args: vec![turso::Arg::Text(id.into())],
    }]).await?;
    let Some(row) = results.first().and_then(|r| r.rows.first()) else { return Ok(None); };
    Ok(Some(render::notes::PublicNote {
        id: id.into(),
        title: value(row, "title").unwrap_or("").into(),
        body: value(row, "body").unwrap_or("").into(),
        place_name: value(row, "place_name").map(str::to_owned),
        taken_on: value(row, "taken_on").map(str::to_owned),
    }))
}

/// The notes attached to a plan's days, for the owner-only plan-page entry.
/// Queried SEPARATELY from load_plan (never in the viewer pipeline, spec §8.1):
/// day_number → (note id, title) pairs.
pub async fn load_day_notes(turso_url: &str, token: &str, plan_id: &str) -> Result<BTreeMap<i64, Vec<(String, String)>>> {
    let rows = turso::pipeline_args(turso_url, token, &[turso::Stmt {
        sql: "SELECT id, title, attached_day_number FROM notes WHERE attached_plan_id = ? AND deleted_at IS NULL ORDER BY created_at DESC, id DESC".into(),
        args: vec![turso::Arg::Text(plan_id.into())],
    }]).await?;
    let mut out: BTreeMap<i64, Vec<(String, String)>> = BTreeMap::new();
    for row in rows.first().map(|r| r.rows.as_slice()).unwrap_or(&[]) {
        let (Some(id), day) = (value(row, "id"), int_value(row, "attached_day_number")) else { continue };
        if !is_note_id(id) { continue; }
        let Some(day) = day else { continue };
        out.entry(day).or_default().push((id.to_owned(), value(row, "title").unwrap_or("").to_owned()));
    }
    Ok(out)
}

pub async fn handle(
    mut req: Request,
    env: Env,
    path: &str,
    query: &HashMap<String, String>,
    is_owner: bool,
    session_login: Option<&str>,
    lang: &str,
) -> Result<Response> {
    let Some(route) = parse_note_route(path) else {
        return not_found();
    };
    // Every /notes/* owner route goes through handle_owner; only /n/* share
    // routes reach the token-checked branch below.
    if !matches!(route, NoteRoute::SharePage(_) | NoteRoute::ShareImage(..)) {
        return handle_owner(&mut req, &env, &route, is_owner, session_login, lang).await;
    }
    if req.method() != Method::Get {
        return not_found();
    }
    let token = note_token(query);
    if !is_owner && !token.is_some_and(is_note_token) {
        return not_found();
    }
    let url = env.secret("TURSO_URL")?.to_string();
    let db_token = env.secret("TURSO_TOKEN")?.to_string();
    match &route {
        NoteRoute::SharePage(id) => {
            // Only the public column list is selected. The correlated EXISTS does not
            // expose purpose, author, or attachment to the rendering layer.
            let sql = if is_owner {
                format!(
                    "SELECT {NOTE_PUBLIC_COLUMNS} FROM notes WHERE id = ? AND deleted_at IS NULL"
                )
            } else {
                format!("SELECT {NOTE_PUBLIC_COLUMNS} FROM notes WHERE id = ? AND deleted_at IS NULL AND EXISTS (SELECT 1 FROM note_share_tokens WHERE note_id = notes.id AND token = ? AND status = 'active')")
            };
            let mut args = vec![turso::Arg::Text(id.clone())];
            if !is_owner {
                args.push(turso::Arg::Text(token.unwrap().into()));
            }
            let results =
                turso::pipeline_args(&url, &db_token, &[turso::Stmt { sql, args }]).await?;
            let Some(row) = results.first().and_then(|r| r.rows.first()) else {
                return not_found();
            };
            let access = AccessRow {
                token_note_id: Some(id.clone()),
                token_status: Some("active".into()),
                note_deleted: false,
                image_exists: false,
            };
            if decide_note_access(&route, is_owner, token, Some(&access)) != NoteAccess::Allow {
                return not_found();
            }
            let note = render::notes::PublicNote {
                id: id.clone(),
                title: value(row, "title").unwrap_or("").into(),
                body: value(row, "body").unwrap_or("").into(),
                place_name: value(row, "place_name").map(str::to_owned),
                taken_on: value(row, "taken_on").map(str::to_owned),
            };
            let images = turso::pipeline_args(&url, &db_token, &[turso::Stmt { sql: "SELECT image_id FROM note_images WHERE note_id = ? ORDER BY sort_order, image_id".into(), args: vec![turso::Arg::Text(id.clone())] }]).await?;
            let ids: Vec<String> = images
                .first()
                .map(|r| {
                    r.rows
                        .iter()
                        .filter_map(|r| value(r, "image_id").map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let html = render::notes::render_share_page(&note, &ids, token.unwrap_or(""), lang);
            let h = Headers::new();
            note_response_headers(&h, NoteResponseKind::SharePage)?;
            Ok(Response::from_html(html)?.with_headers(h))
        }
        NoteRoute::ShareImage(id, image) => {
            // One query checks note, token and image membership; the R2 key comes
            // solely from the returned image row.
            let results = turso::pipeline_args(&url, &db_token, &[turso::Stmt {
                sql: "SELECT n.deleted_at, i.r2_key, i.content_type, s.note_id AS token_note_id, s.status AS token_status FROM notes n LEFT JOIN note_images i ON i.note_id = n.id AND i.image_id = ? LEFT JOIN note_share_tokens s ON s.note_id = n.id AND s.token = ? WHERE n.id = ?".into(),
                args: vec![turso::Arg::Text(image.clone()), token.map(|s| turso::Arg::Text(s.into())).unwrap_or(turso::Arg::Null), turso::Arg::Text(id.clone())],
            }]).await?;
            let Some(row) = results.first().and_then(|r| r.rows.first()) else {
                return not_found();
            };
            let access = AccessRow {
                token_note_id: value(row, "token_note_id").map(str::to_owned),
                token_status: value(row, "token_status").map(str::to_owned),
                note_deleted: value(row, "deleted_at").is_some(),
                image_exists: value(row, "r2_key").is_some(),
            };
            if decide_note_access(&route, is_owner, token, Some(&access)) != NoteAccess::Allow {
                return not_found();
            }
            let Some(key) = value(row, "r2_key") else {
                return not_found();
            };
            let Some(content_type) = value(row, "content_type") else {
                return not_found();
            };
            if content_type != "image/jpeg" && content_type != "image/png" {
                return not_found();
            }
            let bucket = env.bucket("NOTES")?;
            let Some(object) = bucket.get(key).execute().await? else {
                return not_found();
            };
            let Some(body) = object.body() else {
                return not_found();
            };
            let h = Headers::new();
            note_response_headers(&h, NoteResponseKind::Image(content_type))?;
            Ok(Response::from_bytes(body.bytes().await?)?.with_headers(h))
        }
        _ => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    const ID: &str = "note_0123456789abcdef";
    const OTHER: &str = "note_1111111111111111";
    const IMG: &str = "0123456789abcdef";
    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    #[test]
    fn precheck_order() {
        assert_eq!(precheck(false, false, "create", true, Some(70000), 65536), Precheck::Reject(404));
        assert_eq!(precheck(true, true, "create", false, Some(70000), 65536), Precheck::Reject(405));
        assert_eq!(precheck(true, true, "create", true, Some(70000), 65536), Precheck::Reject(413));
    }
    #[test]
    fn precheck_expired_refill_only_for_text_post_with_cookie() {
        assert_eq!(precheck(false, false, "create", true, Some(10), 65536), Precheck::Reject(404));
        assert_eq!(precheck(false, true, "image_add", true, Some(10), 65536), Precheck::Reject(404));
        assert_eq!(precheck(false, true, "update", true, Some(70000), 65536), Precheck::Reject(404));
        assert_eq!(precheck(false, true, "create", true, Some(10), 65536), Precheck::ExpiredRefill);
    }
    #[test]
    fn csrf_bound_to_action_note_session() {
        let a = NoteCsrf::new("secret", "session-a");
        let b = NoteCsrf::new("secret", "session-b");
        assert!(!a.verify(&a.action("update", ID), &a.create()));
        assert!(!a.verify(&a.action("update", ID), &a.action("update", OTHER)));
        assert!(!a.verify(&a.action("update", ID), &b.action("update", ID)));
        assert!(a.verify(&a.action("update", ID), &a.action("update", ID)));
        // image tokens are additionally bound to the image_id and the action
        assert!(!a.verify(&a.image("image_add", ID, IMG), &a.action("update", ID)));
        assert!(!a.verify(&a.image("image_add", ID, IMG), &a.image("image_add", ID, "ffffffffffffffff")));
        assert!(a.verify(&a.image("image_add", ID, IMG), &a.image("image_add", ID, IMG)));
    }
    #[test]
    fn expired_refill_echoes_only_submitted_text() {
        let form = NoteForm { title: "<script>".into(), body: "submitted body".into(), ..Default::default() };
        let html = render_expired_refill(&form, "en");
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("submitted body"));
        assert!(!html.contains("<script>"));
    }
    #[test]
    fn create_redirects_to_edit() {
        let form = NoteForm { id: ID.into(), title: "title".into(), body: "body".into(), form_nonce: "nonce".into(), ..Default::default() };
        assert_eq!(decide_note_post("create", &form, true), PostOutcome::Redirect(format!("/notes/{ID}/edit")));
    }

    #[test]
    fn precheck_expired_refill_uses_caller_max() {
        assert_eq!(precheck(false, true, "create", true, Some(60), 50), Precheck::Reject(404));
        assert_eq!(precheck(false, true, "create", true, Some(50), 50), Precheck::ExpiredRefill);
        assert_eq!(precheck(false, true, "update", true, Some(10), 50), Precheck::ExpiredRefill);
    }

    #[test]
    fn upload_precheck_rejects_missing_unparsable_and_over_cap_content_length() {
        // Missing Content-Length: reject BEFORE `form_data()` reads a byte
        // (an unbounded multipart body must never reach memory).
        assert_eq!(upload_precheck(None), Precheck::Reject(400));
        // Unparsable header: the raw header string reaches the gate, so a
        // garbage value is rejected the same way — never read on trust.
        assert_eq!(upload_precheck(Some("twelve megabytes")), Precheck::Reject(400));
        assert_eq!(upload_precheck(Some("-1")), Precheck::Reject(400));
        assert_eq!(upload_precheck(Some("")), Precheck::Reject(400));
        // Declared over the request cap: 413.
        assert_eq!(upload_precheck(Some(&(UPLOAD_REQUEST_MAX + 1).to_string())), Precheck::Reject(413));
        // Exactly at the cap still reads.
        assert_eq!(upload_precheck(Some(&UPLOAD_REQUEST_MAX.to_string())), Precheck::Owner);
        // A photo of exactly the photo limit is accepted: the request cap is
        // photo limit + a NAMED multipart/fields allowance, not the photo limit
        // alone (photo + framing would otherwise be over).
        assert_eq!(upload_precheck(Some(&IMAGE_FORM_MAX.to_string())), Precheck::Owner);
        assert_eq!(upload_precheck(Some(&(IMAGE_FORM_MAX + 1).to_string())), Precheck::Owner);
        // The cap really is photo limit + allowance, and the allowance is fixed.
        assert_eq!(UPLOAD_REQUEST_MAX, IMAGE_FORM_MAX + MULTIPART_ALLOWANCE);
        assert!(MULTIPART_ALLOWANCE > 0);
    }

    fn attached_form() -> NoteForm {
        NoteForm {
            id: ID.into(),
            title: "標題 'quote'; --".into(),
            body: "內文 ✈️".into(),
            purpose: "給女兒".into(),
            place_name: "台北".into(),
            taken_on: "2026-11-17".into(),
            attached_plan_id: "tokyo-2026".into(),
            attached_destination: "tokyo_2026".into(),
            attached_day_number: "4".into(),
            form_nonce: "0123456789abcdef0123456789abcdef".into(),
        }
    }

    fn placeholders(sql: &str) -> usize {
        sql.matches('?').count()
    }

    const DAYS_GUARD: &str = "EXISTS (SELECT 1 FROM days WHERE plan_id = ? AND destination = ? AND day_number = ?)";

    #[test]
    fn mutation_create_checks_days_existence_inside_the_write() {
        let form = attached_form();
        let built = build_note_mutation("create", ID, &form, "yanggf8", &form.form_nonce, "eeeeeeeeeeeeeeee").unwrap();
        assert_eq!(built.expected, 1);
        // The attachment target must exist at write time, not only in a
        // pre-check SELECT that can race: the guard is part of the INSERT.
        assert!(built.content.sql.contains(DAYS_GUARD));
        assert!(built.content.sql.contains("? IS NULL OR"));
        assert_eq!(placeholders(&built.content.sql), built.content.args.len());
        // The guard re-binds the attachment triple, so it appears twice in the args.
        let plans: Vec<&turso::Arg> = built.content.args.iter().filter(|a| matches!(a, turso::Arg::Text(t) if t == "tokyo-2026")).collect();
        assert_eq!(plans.len(), 3, "plan bound once for the row plus twice for the guard");
    }

    #[test]
    fn mutation_unattached_note_guard_passes_with_null_plan() {
        let mut form = attached_form();
        form.attached_plan_id.clear();
        form.attached_destination.clear();
        form.attached_day_number.clear();
        let built = build_note_mutation("create", ID, &form, "yanggf8", &form.form_nonce, "eeeeeeeeeeeeeeee").unwrap();
        assert!(built.content.sql.contains(DAYS_GUARD));
        // `? IS NULL OR ...` must short-circuit: attachment params are NULL.
        assert!(matches!(built.content.args[1], turso::Arg::Null));
        assert!(matches!(built.content.args[12], turso::Arg::Null));
    }

    #[test]
    fn mutation_update_guards_deleted_duplicate_and_days() {
        let form = attached_form();
        let built = build_note_mutation("update", ID, &form, "yanggf8", &form.form_nonce, "eeeeeeeeeeeeeeee").unwrap();
        assert_eq!(built.expected, 1);
        let sql = &built.content.sql;
        assert!(sql.contains("deleted_at IS NULL"));
        assert!(sql.contains("NOT EXISTS (SELECT 1 FROM note_events WHERE note_id=? AND detail=?)"));
        assert!(sql.contains(DAYS_GUARD));
        assert_eq!(placeholders(sql), built.content.args.len());
        // Detaching an update (all attachment fields cleared) still passes: `? IS NULL`.
        let mut form = attached_form();
        form.attached_plan_id.clear();
        form.attached_destination.clear();
        form.attached_day_number.clear();
        let built = build_note_mutation("update", ID, &form, "yanggf8", &form.form_nonce, "eeeeeeeeeeeeeeee").unwrap();
        assert!(matches!(built.content.args[0], turso::Arg::Null));
    }

    #[test]
    fn mutation_form_text_never_reaches_sql() {
        let mut form = attached_form();
        form.title = "x'; DROP TABLE notes; --".into();
        form.body = "<script>https://x 😀</script>".into();
        for action in ["create", "update"] {
            let built = build_note_mutation(action, ID, &form, "yanggf8", &form.form_nonce, "eeeeeeeeeeeeeeee").unwrap();
            assert!(!built.content.sql.contains("DROP TABLE"), "{action} SQL must not embed form text");
            assert!(!built.content.sql.contains("x';"));
            assert!(!built.content.sql.contains("<script>"));
            assert!(!built.event.sql.contains("<script>"));
            assert!(built.content.args.iter().any(|a| matches!(a, turso::Arg::Text(t) if t.contains("DROP TABLE notes"))));
            assert_eq!(placeholders(&built.content.sql), built.content.args.len());
        }
    }

    #[test]
    fn mutation_delete_is_soft_delete_only() {
        let form = attached_form();
        let built = build_note_mutation("delete", ID, &form, "yanggf8", "ffffffffffffffff", "eeeeeeeeeeeeeeee").unwrap();
        assert_eq!(built.expected, 1);
        assert!(built.content.sql.contains("SET deleted_at=CURRENT_TIMESTAMP"));
        assert!(built.content.sql.contains("deleted_at IS NULL"));
        assert!(!built.content.sql.contains("DELETE FROM"));
        assert_eq!(placeholders(&built.content.sql), built.content.args.len());
        // Delete binds actor and id only — no form text at all.
        assert_eq!(built.content.args.len(), 2);
    }

    #[test]
    fn mutation_event_records_action_and_nonce_detail() {
        let form = attached_form();
        let nonce = "0123456789abcdef0123456789abcdef";
        let built = build_note_mutation("update", ID, &form, "yanggf8", nonce, "eeeeeeeeeeeeeeee").unwrap();
        assert!(built.event.sql.starts_with("INSERT INTO note_events"));
        assert_eq!(placeholders(&built.event.sql), built.event.args.len());
        let texts: Vec<&str> = built.event.args.iter().filter_map(|a| if let turso::Arg::Text(t) = a { Some(t.as_str()) } else { None }).collect();
        assert_eq!(texts, vec!["eeeeeeeeeeeeeeee", ID, "update", "yanggf8", nonce]);
    }

    #[test]
    fn mutation_unknown_action_rejected() {
        let form = attached_form();
        assert!(build_note_mutation("purge", ID, &form, "a", "b", "c").is_none());
    }

    #[test]
    fn refill_after_failed_create_reposts_to_new_not_missing_edit() {
        let form = attached_form();
        // A failed create never inserted a row, so the refill form must post back
        // to /notes/new (the create-bound CSRF and the fresh form_nonce match
        // that route); posting to /notes/<id>/edit would bind a different CSRF
        // and the corrected submission would be rejected.
        let refill = refill_form("create", &form);
        assert!(refill.id.is_empty());
        assert_eq!(refill.title, form.title);
        assert_eq!(refill.body, form.body);
        assert_eq!(refill.form_nonce, form.form_nonce);
        let refill = refill_form("update", &form);
        assert_eq!(refill.id, form.id);
        let html = render::notes::render_owner_form(&refill_form("create", &form), "tok", None, Some("noteRequired"), "zh");
        assert!(html.contains("action=\"/notes/new\""));
        let html = render::notes::render_owner_form(&refill_form("update", &form), "tok", None, Some("noteRequired"), "zh");
        assert!(html.contains(&format!("action=\"/notes/{ID}/edit\"")));
    }
    fn headers(kind: NoteResponseKind<'_>) -> BTreeMap<&'static str, String> {
        note_header_values(kind)
            .into_iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect()
    }

    fn assert_private_headers(h: &BTreeMap<&str, String>) {
        assert_eq!(
            h.get("Cache-Control").map(String::as_str),
            Some("private, no-store")
        );
        assert_eq!(h.get("X-Robots-Tag").map(String::as_str), Some("noindex"));
        assert_eq!(
            h.get("Referrer-Policy").map(String::as_str),
            Some("no-referrer")
        );
        assert_eq!(
            h.get("X-Content-Type-Options").map(String::as_str),
            Some("nosniff")
        );
    }

    #[test]
    fn share_page_final_headers_include_html_content_type() {
        let h = headers(NoteResponseKind::SharePage);
        assert_private_headers(&h);
        assert_eq!(
            h.get("Content-Type").map(String::as_str),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(h.len(), 5);
    }

    #[test]
    fn image_final_headers_use_db_content_type() {
        for content_type in ["image/jpeg", "image/png"] {
            let h = headers(NoteResponseKind::Image(content_type));
            assert_private_headers(&h);
            assert_eq!(
                h.get("Content-Type").map(String::as_str),
                Some(content_type)
            );
            assert_eq!(
                h.get("Content-Disposition").map(String::as_str),
                Some("inline")
            );
            assert_eq!(h.len(), 6);
        }
    }

    #[test]
    fn not_found_final_headers_are_private() {
        let h = headers(NoteResponseKind::NotFound);
        assert_private_headers(&h);
        assert_eq!(
            h.get("Content-Type").map(String::as_str),
            Some("text/plain; charset=utf-8")
        );
        assert_eq!(h.len(), 5);
    }
    fn row() -> AccessRow {
        AccessRow {
            token_note_id: Some(ID.into()),
            token_status: Some("active".into()),
            note_deleted: false,
            image_exists: true,
        }
    }

    #[test]
    fn uniform_404() {
        let page = NoteRoute::SharePage(ID.into());
        let img = NoteRoute::ShareImage(ID.into(), IMG.into());
        assert_eq!(
            decide_note_access(&page, false, None, Some(&row())),
            NoteAccess::NotFound
        );
        assert_eq!(
            decide_note_access(&page, false, Some("bad"), Some(&row())),
            NoteAccess::NotFound
        );
        assert_eq!(
            decide_note_access(&page, false, Some(TOKEN), None),
            NoteAccess::NotFound
        );
        let mut r = row();
        r.token_status = Some("inactive".into());
        assert_eq!(
            decide_note_access(&page, false, Some(TOKEN), Some(&r)),
            NoteAccess::NotFound
        );
        let mut r = row();
        r.token_note_id = Some(OTHER.into());
        assert_eq!(
            decide_note_access(&page, false, Some(TOKEN), Some(&r)),
            NoteAccess::NotFound
        );
        let mut r = row();
        r.note_deleted = true;
        assert_eq!(
            decide_note_access(&page, false, Some(TOKEN), Some(&r)),
            NoteAccess::NotFound
        );
        let mut r = row();
        r.image_exists = false;
        assert_eq!(
            decide_note_access(&img, false, Some(TOKEN), Some(&r)),
            NoteAccess::NotFound
        );
        assert!(parse_note_route("/n/note_xxx").is_none());
    }

    #[test]
    fn owner_without_token_allowed() {
        assert_eq!(
            decide_note_access(&NoteRoute::SharePage(ID.into()), true, None, Some(&row())),
            NoteAccess::Allow
        );
        assert_eq!(
            decide_note_access(
                &NoteRoute::ShareImage(ID.into(), IMG.into()),
                true,
                None,
                Some(&row())
            ),
            NoteAccess::Allow
        );
        assert_eq!(
            decide_note_access(&NoteRoute::OwnerList, true, None, None),
            NoteAccess::Allow
        );
        assert_eq!(
            decide_note_access(&NoteRoute::OwnerList, false, None, None),
            NoteAccess::NotFound
        );
    }

    #[test]
    fn plan_query_token_ignored() {
        let query = std::collections::HashMap::from([("token".to_string(), TOKEN.to_string())]);
        assert_eq!(note_token(&query), None);
        assert_eq!(
            decide_note_access(
                &NoteRoute::SharePage(ID.into()),
                false,
                note_token(&query),
                Some(&row())
            ),
            NoteAccess::NotFound
        );
    }

    #[test]
    fn parse_route_rejects_traversal() {
        assert!(parse_note_route("/n/../x").is_none());
        assert!(parse_note_route("/n/note_xxx/img/../../a").is_none());
        assert!(parse_note_route(&format!("/n/{ID}/img/{IMG}/more")).is_none());
        assert!(is_note_path("/notes"));
        assert!(is_note_path(&format!("/n/{ID}")));
        assert!(!is_note_path("/notesfoo"));
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn headers_set() {
        let h = Headers::new();
        note_response_headers(&h, NoteResponseKind::SharePage).unwrap();
        for (key, value) in [
            ("Cache-Control", "private, no-store"),
            ("X-Robots-Tag", "noindex"),
            ("Referrer-Policy", "no-referrer"),
            ("X-Content-Type-Options", "nosniff"),
        ] {
            assert_eq!(h.get(key).unwrap().as_deref(), Some(value));
        }
        assert_eq!(
            h.get("Content-Type").unwrap().as_deref(),
            Some("text/html; charset=utf-8")
        );
        note_response_headers(&h, NoteResponseKind::Image("image/jpeg")).unwrap();
        assert_eq!(
            h.get("Content-Type").unwrap().as_deref(),
            Some("image/jpeg")
        );
        assert_eq!(
            h.get("Content-Disposition").unwrap().as_deref(),
            Some("inline")
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn headers_set() {
        assert_eq!(
            NOTE_HEADERS,
            [
                ("Cache-Control", "private, no-store"),
                ("X-Robots-Tag", "noindex"),
                ("Referrer-Policy", "no-referrer"),
                ("X-Content-Type-Options", "nosniff")
            ]
        );
    }

    // ── T7: photo upload/delete/reorder, share flow, plan-page entry ──

    /// A real, structurally valid 1×1 PNG (the same byte-for-byte placeholder the
    /// map route serves) — passes note_core::scrub's chunk whitelist.
    const PNG_BYTES: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78, 0x01, 0x01, 0x05,
        0x00, 0xfa, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x01, 0x64, 0x78, 0x95,
        0x38, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn image_texts(args: &[turso::Arg]) -> Vec<String> {
        args.iter()
            .filter_map(|a| if let turso::Arg::Text(t) = a { Some(t.clone()) } else { None })
            .collect()
    }

    #[test]
    fn upload_rejects_unscrubbable() {
        // HTML and HEIC fail the magic-byte sniff; a JPEG whose APP0 length field
        // overruns the buffer passes the sniff but fails the segment walk. Both
        // scrub failures map to an i18n error key — and because `plan_upload` is
        // pure and returns Err before any I/O, a rejected upload can never reach
        // an R2 put.
        assert_eq!(plan_upload(b"<html><script>").unwrap_err(), "noteUnsupportedImage");
        let heic = b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00mif1heic";
        assert_eq!(plan_upload(heic).unwrap_err(), "noteUnsupportedImage");
        let truncated = [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F'];
        assert_eq!(plan_upload(&truncated).unwrap_err(), "noteImageMalformed");
        let ok = plan_upload(PNG_BYTES).unwrap();
        assert_eq!(ok.kind, note_core::ImageKind::Png);
        assert!(!ok.bytes.is_empty());
    }

    #[test]
    fn upload_plan_key_from_ids() {
        assert_eq!(upload_key(ID, IMG, note_core::ImageKind::Jpeg), format!("{ID}/{IMG}.jpg"));
        assert_eq!(upload_key(ID, IMG, note_core::ImageKind::Png), format!("{ID}/{IMG}.png"));
    }

    fn io(image_id: &str, sort_order: i64) -> ImageOrder {
        ImageOrder { image_id: image_id.into(), sort_order }
    }

    fn three_images() -> Vec<ImageOrder> {
        vec![io("0123456789abcdef", 1), io("1111111111111111", 2), io("2222222222222222", 3)]
    }

    #[test]
    fn move_up_first_is_noop() {
        let imgs = three_images();
        // First image moved up / last moved down / unknown id: nothing to swap.
        assert!(plan_image_move(&imgs, "0123456789abcdef", true).is_none());
        assert!(plan_image_move(&imgs, "2222222222222222", false).is_none());
        assert!(plan_image_move(&imgs, "9999999999999999", true).is_none());
        // Middle image: up swaps with the row above, down with the row below.
        let swap = plan_image_move(&imgs, "1111111111111111", true).unwrap();
        assert_eq!(swap.neighbor_id, "0123456789abcdef");
        assert_eq!((swap.target_order, swap.neighbor_order), (2, 1));
        let swap = plan_image_move(&imgs, "1111111111111111", false).unwrap();
        assert_eq!(swap.neighbor_id, "2222222222222222");
        assert_eq!((swap.target_order, swap.neighbor_order), (2, 3));
    }

    #[test]
    fn reorder_expects_one_row_each() {
        // The photo move is TWO UPDATEs — `changes()` only reflects the last one,
        // so each statement carries its own expected count of exactly 1: any 0-row
        // statement (note gone, image gone) rolls the whole swap back.
        let imgs = three_images();
        let swap = plan_image_move(&imgs, "1111111111111111", true).unwrap();
        let built = build_image_move(ID, "1111111111111111", &swap, true, "yanggf8", "eeeeeeeeeeeeeeee");
        assert_eq!(built.content.len(), 2);
        assert_eq!(built.expected, vec![1, 1]);
        // Photo delete / photo upload / note update: one statement, one row each.
        let del = build_image_delete(ID, IMG, &format!("{ID}/{IMG}.jpg"), "yanggf8", "eeeeeeeeeeeeeeee");
        assert_eq!(del.expected, vec![1]);
        assert_eq!(del.content.len(), 1);
        let add = build_image_add(ID, IMG, &format!("{ID}/{IMG}.png"), "image/png", 1234, "yanggf8", "eeeeeeeeeeeeeeee");
        assert_eq!(add.expected, vec![1]);
        let upd = build_note_mutation("update", ID, &attached_form(), "yanggf8", "0123456789abcdef0123456789abcdef", "eeeeeeeeeeeeeeee").unwrap();
        assert_eq!(upd.expected, 1);
    }

    #[test]
    fn image_add_sql_guards_deleted_note_and_binds_all_text() {
        let key = format!("{ID}/{IMG}.png");
        let built = build_image_add(ID, IMG, &key, "image/png", 1234, "yanggf8", "eeeeeeeeeeeeeeee");
        let sql = &built.content[0].sql;
        assert!(sql.contains("INSERT INTO note_images"));
        // The image row only lands when the note exists and is not soft-deleted,
        // and the next sort_order is read inside the statement (no TOCTOU).
        assert!(sql.contains("deleted_at IS NULL"));
        assert!(sql.contains("COALESCE(MAX(sort_order)"));
        assert_eq!(placeholders(sql), built.content[0].args.len());
        assert!(built.content[0].args.iter().any(|a| matches!(a, turso::Arg::Int(1234))));
        // Event detail mirrors the CLI shape (image_id=… key=…).
        let ev = &built.events[0];
        assert_eq!(placeholders(&ev.sql), ev.args.len());
        assert_eq!(
            image_texts(&ev.args),
            vec![
                "eeeeeeeeeeeeeeee".to_string(),
                ID.to_string(),
                "image_add".to_string(),
                "yanggf8".to_string(),
                format!("image_id={IMG} key={key}")
            ]
        );
    }

    #[test]
    fn image_delete_sql_guards_note_and_logs_key() {
        let key = format!("{ID}/{IMG}.jpg");
        let built = build_image_delete(ID, IMG, &key, "yanggf8", "eeeeeeeeeeeeeeee");
        let sql = &built.content[0].sql;
        assert!(sql.contains("DELETE FROM note_images"));
        assert!(sql.contains("deleted_at IS NULL"));
        assert_eq!(placeholders(sql), built.content[0].args.len());
        let ev = &built.events[0];
        assert_eq!(
            image_texts(&ev.args),
            vec![
                "eeeeeeeeeeeeeeee".to_string(),
                ID.to_string(),
                "image_delete".to_string(),
                "yanggf8".to_string(),
                format!("image_id={IMG} key={key}")
            ]
        );
    }

    #[test]
    fn image_move_sql_swaps_two_rows_with_guard() {
        let imgs = three_images();
        let swap = plan_image_move(&imgs, "1111111111111111", true).unwrap();
        let built = build_image_move(ID, "1111111111111111", &swap, true, "yanggf8", "eeeeeeeeeeeeeeee");
        assert_eq!(built.content.len(), 2);
        // Statement 1 sets the TARGET row to the NEIGHBOR's order, statement 2
        // the neighbor to the target's — that is what "swap" means.
        for (stmt, order) in built.content.iter().zip([swap.neighbor_order, swap.target_order]) {
            assert!(stmt.sql.contains("UPDATE note_images SET sort_order = ?"));
            assert!(stmt.sql.contains("deleted_at IS NULL"));
            assert_eq!(placeholders(&stmt.sql), stmt.args.len());
            assert!(stmt.args.iter().any(|a| matches!(a, turso::Arg::Int(n) if *n == order)), "{order}");
        }
        let ev = &built.events[0];
        assert_eq!(
            image_texts(&ev.args),
            vec![
                "eeeeeeeeeeeeeeee".to_string(),
                ID.to_string(),
                "image_move".to_string(),
                "yanggf8".to_string(),
                "image_id=1111111111111111 dir=up".to_string()
            ]
        );
    }

    #[test]
    fn share_create_sql_binds_token_and_guards_note() {
        let token = TOKEN;
        let built = build_share_create(ID, token, "yanggf8", "eeeeeeeeeeeeeeee");
        let sql = &built.content[0].sql;
        assert!(sql.contains("INSERT INTO note_share_tokens"));
        // Token only lands when the note exists and is not soft-deleted.
        assert!(sql.contains("deleted_at IS NULL"));
        assert!(sql.contains("'active'"));
        assert_eq!(placeholders(sql), built.content[0].args.len());
        assert!(built.content[0].args.iter().any(|a| matches!(a, turso::Arg::Text(t) if t == token)));
        let ev = &built.events[0];
        assert_eq!(
            image_texts(&ev.args),
            vec![
                "eeeeeeeeeeeeeeee".to_string(),
                ID.to_string(),
                "share_create".to_string(),
                "yanggf8".to_string(),
                format!("token={}", token_fingerprint(token))
            ]
        );
    }

    #[test]
    fn share_deactivate_and_stop_all_expect_actual_counts() {
        // Deactivating THIS note's links: the expected count comes from the
        // pre-read SELECT, so a concurrent change fails loudly instead of
        // half-claiming success.
        let built = build_share_deactivate(ID, 2, "yanggf8", "eeeeeeeeeeeeeeee");
        assert_eq!(built.expected, vec![2]);
        assert!(built.content[0].sql.contains("SET status='inactive'"));
        assert!(built.content[0].sql.contains("WHERE note_id=? AND status='active'"));
        assert_eq!(image_texts(&built.events[0].args)[4], "tokens=2");

        // Stop-all flips every active token in one UPDATE; each affected note
        // still gets its own audit event.
        let per_note = vec![(ID.to_string(), 1u64), (OTHER.to_string(), 2u64)];
        let built = build_share_stop_all(&per_note, "yanggf8", &["eeeeeeeeeeeeeeee".to_string(), "ffffffffffffffff".to_string()]);
        assert_eq!(built.expected, vec![3]);
        assert!(built.content[0].sql.contains("WHERE status='active'"));
        assert!(!built.content[0].sql.contains("note_id=?"), "stop-all must not be scoped to one note");
        assert_eq!(built.events.len(), 2);
        assert_eq!(image_texts(&built.events[0].args)[2], "share_deactivate");
        assert_eq!(image_texts(&built.events[0].args)[4], "tokens=1");
        assert_eq!(image_texts(&built.events[1].args)[1], OTHER);
        assert_eq!(image_texts(&built.events[1].args)[4], "tokens=2");
    }

    #[test]
    fn token_fingerprint_hides_middle() {
        // Same shape as share_token.rs:574 (CLI) so both surfaces log the same detail.
        assert_eq!(token_fingerprint(TOKEN), "012345...abcdef");
        assert_eq!(token_fingerprint("0123456789ab"), "0123456789ab");
    }

    #[test]
    fn share_and_image_csrf_are_action_scoped() {
        let csrf = NoteCsrf::new("secret", "session-a");
        // Share actions bind action + note + session: a create token cannot mint
        // a share link, and one share action cannot fire another.
        assert!(!csrf.verify(&csrf.action("share-create", ID), &csrf.create()));
        assert!(!csrf.verify(&csrf.action("share-create", ID), &csrf.action("share-preview", ID)));
        assert!(!csrf.verify(&csrf.action("share-deactivate", ID), &csrf.action("share-create", ID)));
        assert!(csrf.verify(&csrf.action("share-create", ID), &csrf.action("share-create", ID)));
        // Image actions bind the image_id on top of action + note + session.
        assert!(!csrf.verify(&csrf.image("image_delete", ID, IMG), &csrf.action("share-create", ID)));
        assert!(!csrf.verify(&csrf.image("image_delete", ID, IMG), &csrf.image("image_delete", OTHER, IMG)));
        assert!(!csrf.verify(&csrf.image("image_delete", ID, IMG), &csrf.image("image_move", ID, IMG)));
        assert!(csrf.verify(&csrf.image("image_delete", ID, IMG), &csrf.image("image_delete", ID, IMG)));
        // Stop-all has no note id: it binds the action + session alone.
        assert!(!csrf.verify(&csrf.stop_all(), &csrf.create()));
        assert!(csrf.verify(&csrf.stop_all(), &csrf.stop_all()));
    }

    #[test]
    fn new_owner_routes_parse_and_stay_owner_only() {
        assert_eq!(parse_note_route("/notes/share-stop-all"), Some(NoteRoute::OwnerStopAll));
        assert_eq!(parse_note_route(&format!("/notes/{ID}/image")), Some(NoteRoute::OwnerUpload(ID.into())));
        assert_eq!(
            parse_note_route(&format!("/notes/{ID}/image/{IMG}/delete")),
            Some(NoteRoute::OwnerImagePost(ID.into(), IMG.into(), ImageOp::Delete))
        );
        assert_eq!(
            parse_note_route(&format!("/notes/{ID}/image/{IMG}/up")),
            Some(NoteRoute::OwnerImagePost(ID.into(), IMG.into(), ImageOp::Up))
        );
        assert_eq!(
            parse_note_route(&format!("/notes/{ID}/image/{IMG}/down")),
            Some(NoteRoute::OwnerImagePost(ID.into(), IMG.into(), ImageOp::Down))
        );
        assert_eq!(
            parse_note_route(&format!("/notes/{ID}/share-preview")),
            Some(NoteRoute::OwnerSharePost(ID.into(), ShareAction::Preview))
        );
        assert_eq!(
            parse_note_route(&format!("/notes/{ID}/share-create")),
            Some(NoteRoute::OwnerSharePost(ID.into(), ShareAction::Create))
        );
        assert_eq!(
            parse_note_route(&format!("/notes/{ID}/share-deactivate")),
            Some(NoteRoute::OwnerSharePost(ID.into(), ShareAction::Deactivate))
        );
        // Bad ids and traversal stay None.
        assert!(parse_note_route("/notes/note_xxx/image").is_none());
        assert!(parse_note_route(&format!("/notes/{ID}/image/{IMG}/../../a")).is_none());
        assert!(parse_note_route(&format!("/notes/{ID}/image/ffffffffffffffff/nope")).is_none());
        assert!(parse_note_route("/notes/share-stop-all/x").is_none());
        // Every owner route stays owner-only for a stranger: uniform 404.
        for route in [
            NoteRoute::OwnerStopAll,
            NoteRoute::OwnerUpload(ID.into()),
            NoteRoute::OwnerImagePost(ID.into(), IMG.into(), ImageOp::Up),
            NoteRoute::OwnerSharePost(ID.into(), ShareAction::Create),
        ] {
            assert_eq!(decide_note_access(&route, false, None, None), NoteAccess::NotFound);
            assert_eq!(decide_note_access(&route, true, None, None), NoteAccess::Allow);
        }
    }
}
