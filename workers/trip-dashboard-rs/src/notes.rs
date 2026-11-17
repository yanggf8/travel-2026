use crate::{render, turso};
use note_core::{is_image_id, is_note_id, is_note_token, NOTE_PUBLIC_COLUMNS};
use std::collections::HashMap;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use worker::*;

const TEXT_FORM_MAX: u64 = 65_536;

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
    // No callers until the photo routes land (plan T7); the binding format is
    // covered by csrf_bound_to_action_note_session.
    #[allow(dead_code)]
    pub fn image(&self, action: &str, note_id: &str, image_id: &str) -> String { self.sign(&format!("note:{action}:{note_id}:{image_id}:{}", self.session_cookie)) }
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
    SharePage(String),
    ShareImage(String, String),
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
    if let Some(rest) = path.strip_prefix("/notes/") {
        let segments: Vec<_> = rest.split('/').collect();
        if segments.len() == 2 && is_note_id(segments[0]) && segments[1] == "edit" {
            return Some(NoteRoute::OwnerEdit(segments[0].into()));
        }
        if segments.len() == 2 && is_note_id(segments[0]) && segments[1] == "delete" {
            return Some(NoteRoute::OwnerPost(segments[0].into()));
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
    if matches!(
        route,
        NoteRoute::OwnerList
            | NoteRoute::OwnerNew
            | NoteRoute::OwnerEdit(_)
            | NoteRoute::OwnerPost(_)
    ) {
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

async fn handle_owner(req: &mut Request, env: &Env, route: &NoteRoute, is_owner: bool, login: Option<&str>, lang: &str) -> Result<Response> {
    let is_post = req.method() == Method::Post;
    let action = match route {
        NoteRoute::OwnerNew => "create",
        NoteRoute::OwnerEdit(_) => "update",
        NoteRoute::OwnerPost(_) => "delete",
        NoteRoute::OwnerList => "list",
        _ => return not_found(),
    };
    let cookie = req.headers().get("Cookie")?.unwrap_or_default();
    let session_cookie = cookie.split(';').map(str::trim).find_map(|p| p.strip_prefix("__Host-td_session=")).unwrap_or("");
    let content_length = req.headers().get("Content-Length")?.and_then(|s| s.parse::<u64>().ok());
    if is_post {
        match precheck(is_owner, !session_cookie.is_empty(), action, true, content_length, TEXT_FORM_MAX) {
            Precheck::Reject(404) => return not_found(),
            Precheck::Reject(status) => return Response::error("Invalid request", status),
            Precheck::ExpiredRefill => {
                let data = req.form_data().await?;
                let form = form_from_post(&data, "");
                return html_response(render_expired_refill(&form, lang), 401);
            }
            Precheck::Owner => {}
        }
    } else if !is_owner { return not_found(); }
    if !is_post && req.method() != Method::Get { return Response::error("Method not allowed", 405); }
    let url = env.secret("TURSO_URL")?.to_string();
    let read_token = env.secret("TURSO_TOKEN")?.to_string();
    let csrf = NoteCsrf::new(&env.secret("SESSION_SECRET")?.to_string(), session_cookie);
    if !is_post {
        match route {
            NoteRoute::OwnerList => {
                let rows = turso::pipeline_args(&url, &read_token, &[turso::Stmt { sql: "SELECT id, title, purpose FROM notes WHERE deleted_at IS NULL ORDER BY created_at DESC, id DESC".into(), args: vec![] }]).await?;
                let notes = rows[0].rows.iter().filter_map(|r| Some((value(r,"id")?.to_owned(), value(r,"title").unwrap_or("").to_owned(), value(r,"purpose").unwrap_or("").to_owned()))).collect::<Vec<_>>();
                return html_response(render::notes::render_owner_list(&notes, lang), 200);
            }
            NoteRoute::OwnerNew => {
                let form = NoteForm { form_nonce: random_hex::<16>()?, ..Default::default() };
                return html_response(render::notes::render_owner_form(&form, &csrf.create(), None, None, lang), 200);
            }
            NoteRoute::OwnerEdit(id) => {
                let rows = turso::pipeline_args(&url, &read_token, &[turso::Stmt { sql: "SELECT title, body, purpose, place_name, taken_on, attached_plan_id, attached_destination, attached_day_number FROM notes WHERE id = ? AND deleted_at IS NULL".into(), args: vec![turso::Arg::Text(id.clone())] }]).await?;
                let Some(row) = rows[0].rows.first() else { return not_found(); };
                let form = note_form_row(id, row, random_hex::<16>()?);
                return html_response(render::notes::render_owner_form(&form, &csrf.action("update", id), Some(&csrf.action("delete", id)), None, lang), 200);
            }
            _ => return Response::error("Method not allowed", 405),
        }
    }
    if action == "list" { return Response::error("Method not allowed", 405); }
    let id = match route { NoteRoute::OwnerEdit(id) | NoteRoute::OwnerPost(id) => id.clone(), NoteRoute::OwnerNew => format!("note_{}", random_hex::<8>()?), _ => unreachable!() };
    let data = req.form_data().await?;
    let form = form_from_post(&data, &id);
    let expected = if action == "create" { csrf.create() } else { csrf.action(action, &id) };
    let csrf_ok = csrf.verify(&expected, &data.get_field("csrf").unwrap_or_default());
    match decide_note_post(action, &form, csrf_ok) {
        PostOutcome::Status(status) => return Response::error("Invalid request", status),
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
    let write_token = env.secret("TURSO_WRITE_TOKEN")?.to_string();
    let nonce = if action == "delete" { random_hex::<16>()? } else { form.form_nonce.clone() };
    let event_id = random_hex::<16>()?;
    let actor = login.unwrap_or("");
    let Some(built) = build_note_mutation(action, &id, &form, actor, &nonce, &event_id) else {
        return Response::error("Invalid request", 404);
    };
    match turso::pipeline_txn(&url, &write_token, &[built.content], &[built.expected], &[built.event]).await {
        Ok(_) => {
            let location = if action == "delete" { "/notes".into() } else { format!("/notes/{id}/edit") };
            let h = Headers::new(); h.set("Location", &location)?; h.set("Cache-Control", "private, no-store")?;
            Ok(Response::empty()?.with_status(303).with_headers(h))
        }
        Err(turso::TxnError::RowMismatch { .. }) => not_found(),
        Err(e) => Err(Error::RustError(e.to_string())),
    }
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
    if matches!(
        route,
        NoteRoute::OwnerList
            | NoteRoute::OwnerNew
            | NoteRoute::OwnerEdit(_)
            | NoteRoute::OwnerPost(_)
    ) {
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
}
