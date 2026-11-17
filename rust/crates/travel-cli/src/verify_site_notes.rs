//! Temporary live note checks for `verify-site` (Task 8). All fixture rows are
//! marked `created_by='verify-site'`; cleanup deletes R2 first, then DB rows.
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use libsql::{Connection, params};
use note_core::{note_id_from_bytes, token_from_bytes};
use scraper::{Html, Selector};
use travel_db::repo::notes::{self, NoteInput};

use crate::verify_site::Check;

const ACTOR: &str = "verify-site";

fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0; N];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("note fixture random: {e}"))?;
    Ok(bytes)
}

/// Sweep an interrupted run before minting any new fixture. A failed R2 delete
/// leaves its DB key intact so a later run can retry it.
pub async fn sweep(conn: &Connection) -> Result<(), String> {
    let mut rows = conn
        .query("SELECT id FROM notes WHERE created_by = ?1", params![ACTOR])
        .await
        .map_err(|e| e.to_string())?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        ids.push(row.get::<String>(0).map_err(|e| e.to_string())?);
    }
    for id in ids {
        cleanup(conn, &id, None).await?;
    }
    Ok(())
}

async fn cleanup(conn: &Connection, id: &str, prefix: Option<&str>) -> Result<(), String> {
    // An unexpected POST may have created a note with the run's title prefix
    // under the owner login. Never touch rows outside these two markers.
    let mut matched = conn
        .query(
            "SELECT 1 FROM notes WHERE id = ?1 AND (created_by = ?2 OR (?3 IS NOT NULL AND substr(title, 1, length(?3)) = ?3)) LIMIT 1",
            params![id, ACTOR, prefix.map(str::to_owned)],
        )
        .await
        .map_err(|e| e.to_string())?;
    if matched.next().await.map_err(|e| e.to_string())?.is_none() {
        return Ok(());
    }
    // The list comes from Turso, never from an URL or a computed R2 path.
    for image in notes::list_images(conn, id).await? {
        r2_delete(&image.r2_key)?;
    }
    let tx = conn.transaction().await.map_err(|e| e.to_string())?;
    for table in ["note_events", "note_share_tokens", "note_images", "notes"] {
        // Table names are fixed code constants; values remain bound.
        if table == "notes" {
            let changed = tx.execute(
                "DELETE FROM notes WHERE id = ?1 AND (created_by = ?2 OR (?3 IS NOT NULL AND substr(title, 1, length(?3)) = ?3))",
                params![id, ACTOR, prefix.map(str::to_owned)],
            ).await.map_err(|e| e.to_string())?;
            if changed != 1 {
                return Err(format!(
                    "cleanup {id}: expected one note row, removed {changed}"
                ));
            }
        } else {
            // Table names are fixed code constants; values remain bound.
            tx.execute(
                &format!("DELETE FROM {table} WHERE note_id = ?1"),
                params![id],
            )
            .await
            .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().await.map_err(|e| e.to_string())
}

async fn run_note_ids(conn: &Connection, prefix: &str) -> Result<Vec<String>, String> {
    let mut rows = conn.query(
        "SELECT id FROM notes WHERE created_by = ?1 OR (substr(title, 1, length(?2)) = ?2) ORDER BY id",
        params![ACTOR, prefix],
    ).await.map_err(|e| e.to_string())?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        ids.push(row.get::<String>(0).map_err(|e| e.to_string())?);
    }
    Ok(ids)
}

async fn count_run_notes(conn: &Connection, prefix: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM notes WHERE created_by = ?1 OR substr(title, 1, length(?2)) = ?2",
            params![ACTOR, prefix],
        )
        .await
        .map_err(|e| e.to_string())?;
    rows.next()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "note count row missing".to_string())?
        .get(0)
        .map_err(|e| e.to_string())
}

async fn cleanup_run(conn: &Connection, prefix: &str) -> Result<(), String> {
    let ids = run_note_ids(conn, prefix).await?;
    let mut errors = Vec::new();
    for id in ids {
        if let Err(e) = cleanup(conn, &id, Some(prefix)).await {
            errors.push(format!("{id}: {e}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn r2_delete(key: &str) -> Result<(), String> {
    let out = crate::snapshot_maps::wrangler_command()
        .args(crate::notes::r2_delete_args(key))
        .output()
        .map_err(|e| format!("R2 delete {key}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if crate::notes::is_r2_not_found(&message) {
        return Ok(());
    }
    Err(format!(
        "R2 delete {key} failed ({}): {}",
        out.status,
        message.trim()
    ))
}

struct Http {
    status: u16,
    headers: String,
    body: Vec<u8>,
}

struct TempFiles(std::path::PathBuf);
impl TempFiles {
    fn new() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("verify-site-notes-{}", hex(&random::<8>()?)));
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|e| e.to_string())?;
        Ok(Self(path))
    }
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
}
impl Drop for TempFiles {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn read_response_body(path: &Path) -> Result<Vec<u8>, String> {
    match fs::read(path) {
        Ok(body) => Ok(body),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

fn fetch(
    url: &str,
    temp: &TempFiles,
    cookie: Option<&str>,
    form: Option<&str>,
) -> Result<Http, String> {
    let body = temp.path("body");
    let _ = fs::remove_file(&body);
    let mut config = crate::verify_site::curl_config_url(url)?;
    if let Some(data) = form {
        if data.contains(['\r', '\n', '"', '\\']) {
            return Err("invalid form data".into());
        }
        config.push_str(&format!("data = \"{data}\"\n"));
    }
    if let Some(value) = cookie {
        if value.contains(['\r', '\n', '"']) {
            return Err("invalid owner cookie".into());
        }
        config.push_str(&format!("header = \"Cookie: __Host-td_session={value}\"\n"));
    }
    let mut cmd = Command::new("curl");
    cmd.args([
        "-q",
        "-sS",
        "--max-time",
        "30",
        "--proto",
        "=https",
        "-K",
        "-",
        "-D",
        "-",
    ])
    .arg("-o")
    .arg(&body)
    .arg("-w")
    .arg("\n__VERIFY_STATUS__%{http_code}");
    if form.is_some() {
        cmd.args(["-X", "POST"]);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("curl stdin missing")?
        .write_all(config.as_bytes())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "curl failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let output = String::from_utf8_lossy(&out.stdout);
    let (headers, status) = output
        .rsplit_once("\n__VERIFY_STATUS__")
        .ok_or("curl status missing")?;
    let status = status.trim().parse().map_err(|_| "curl status missing")?;
    Ok(Http {
        status,
        headers: headers.to_ascii_lowercase(),
        body: read_response_body(&body)?,
    })
}

fn expected_headers(response: &Http, image: bool) -> bool {
    let h = &response.headers;
    [
        "cache-control: private, no-store",
        "x-robots-tag: noindex",
        "referrer-policy: no-referrer",
        "x-content-type-options: nosniff",
    ]
    .iter()
    .all(|line| h.contains(line))
        && (!image
            || (h.contains("content-disposition: inline") && h.contains("content-type: image/png")))
}

fn verdict(key: &str, ok: bool, detail: &str) -> (String, Check) {
    (
        key.into(),
        if ok {
            Check::pass(detail)
        } else {
            Check::fail(detail, None)
        },
    )
}

fn upload_by_cli(id: &str, fixture: &Path) -> Result<(), String> {
    // T4 is the only CLI path that scrubs then writes the live R2 bucket.
    let out = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(["note", "add-image", id])
        .arg(fixture)
        .env_remove("TRAVEL_NOTES_DRY_R2")
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "CLI note add-image failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

async fn count_events(conn: &Connection, id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM note_events WHERE note_id = ?1",
            params![id],
        )
        .await
        .map_err(|e| e.to_string())?;
    rows.next()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "count row missing".to_string())?
        .get(0)
        .map_err(|e| e.to_string())
}

fn abnormal_post_check(
    response: &Http,
    expected_status: u16,
    escaped_title: Option<&str>,
    before: i64,
    after: i64,
) -> Check {
    let echoed =
        escaped_title.is_none_or(|text| String::from_utf8_lossy(&response.body).contains(text));
    if response.status == expected_status && echoed && after == before {
        Check::pass("rejected POST returned the expected response and created no note")
    } else {
        Check::fail(
            format!(
                "rejected POST: HTTP {} (expected {expected_status}), title echoed={echoed}, matching note rows {before} → {after}",
                response.status
            ),
            None,
        )
    }
}

/// Read the token actually issued by the worker for one exact form action.
/// Form-scoped lookup avoids accidentally taking the delete or share token.
fn hidden_field_for_action(html: &str, action: &str, name: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let forms = Selector::parse("form").ok()?;
    let inputs = Selector::parse("input").ok()?;
    doc.select(&forms)
        .find(|form| form.value().attr("action") == Some(action))?
        .select(&inputs)
        .find(|input| input.value().attr("name") == Some(name))?
        .value()
        .attr("value")
        .map(str::to_owned)
}

fn owner_availability(cookie: Option<&str>) -> Check {
    match cookie.filter(|value| !value.is_empty()) {
        Some(_) => Check::pass("owner cookie supplied"),
        None => Check::warn(
            "owner checks skipped: set TRAVEL_VERIFY_SITE_OWNER_COOKIE (one-time, copy from a signed-in browser; never store it)",
            None,
        ),
    }
}

fn owner_form_field(page: &Http, action: &str, name: &str) -> Result<String, String> {
    if page.status != 200 {
        return Err("owner edit page did not return 200".into());
    }
    hidden_field_for_action(&String::from_utf8_lossy(&page.body), action, name)
        .ok_or_else(|| format!("owner edit page has no {name} for {action}"))
}

/// Each result is one plain-text verdict. The outer function always cleans up,
/// even if fixture creation or an HTTP/DB step fails.
pub async fn check(
    conn: &Connection,
    plan: &str,
    host: &str,
) -> Result<Vec<(String, Check)>, String> {
    let first = note_id_from_bytes(random()?);
    let second = note_id_from_bytes(random()?);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("note fixture clock: {e}"))?
        .as_nanos();
    let prefix = format!("zz_verify_site_{nanos}_{}_", hex(&random::<4>()?));
    let mut result = run_checks(conn, plan, host, &first, &second, &prefix)
        .await
        .unwrap_or_default();
    if let Err(e) = cleanup_run(conn, &prefix).await {
        result.push(verdict("cleanup", false, &e));
    }
    Ok(result)
}

async fn run_checks(
    conn: &Connection,
    plan: &str,
    host: &str,
    first: &str,
    second: &str,
    prefix: &str,
) -> Option<Vec<(String, Check)>> {
    match run_checks_inner(conn, plan, host, first, second, prefix).await {
        Ok(rows) => Some(rows),
        Err(e) => Some(vec![verdict("setup", false, &e)]),
    }
}

async fn run_checks_inner(
    conn: &Connection,
    plan: &str,
    host: &str,
    first: &str,
    second: &str,
    prefix: &str,
) -> Result<Vec<(String, Check)>, String> {
    let temp = TempFiles::new()?;
    let title = format!("{prefix}fixture <script> ✈️");
    let input = NoteInput {
        title: &title,
        body: "https://example.invalid ✈️",
        purpose: None,
        place_name: None,
        taken_on: None,
        attached_plan_id: None,
        attached_destination: None,
        attached_day_number: None,
    };
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    notes::insert_note(conn, first, &input, ACTOR, &now).await?;
    notes::insert_note(conn, second, &input, ACTOR, &now).await?;
    let token = token_from_bytes(random()?);
    if !notes::insert_share_token(conn, first, &token, ACTOR, &now).await? {
        return Err("note token insert changed zero rows".into());
    }
    let plan_token = crate::share_token::mint_for_plan(conn, plan, ACTOR).await?;
    let checked = run_live(
        conn,
        plan,
        host,
        first,
        second,
        &token,
        &plan_token,
        &temp,
        prefix,
    )
    .await;
    let deleted = conn
        .execute(
            "DELETE FROM plan_share_tokens WHERE token = ?1 AND created_by = ?2",
            params![plan_token, ACTOR],
        )
        .await
        .map_err(|e| e.to_string());
    match (checked, deleted) {
        (Ok(rows), Ok(1)) => Ok(rows),
        (Ok(mut rows), Ok(n)) => {
            rows.push(verdict(
                "cleanup-plan-token",
                false,
                &format!("deleted {n} rows, expected 1"),
            ));
            Ok(rows)
        }
        (Ok(mut rows), Err(e)) => {
            rows.push(verdict("cleanup-plan-token", false, &e));
            Ok(rows)
        }
        (Err(e), Ok(1)) => Err(e),
        (Err(e), other) => Err(format!("{e}; plan token cleanup: {other:?}")),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_live(
    conn: &Connection,
    plan: &str,
    host: &str,
    first: &str,
    second: &str,
    token: &str,
    plan_token: &str,
    temp: &TempFiles,
    prefix: &str,
) -> Result<Vec<(String, Check)>, String> {
    let mut checks = Vec::new();
    let root = format!("https://{host}");
    let page = format!("{root}/n/{first}?t={token}");
    let p = fetch(
        &format!("{root}/?plan={plan}&token={token}"),
        temp,
        None,
        None,
    )?;
    let plan_doc = crate::verify_site_page::parse_page(&String::from_utf8_lossy(&p.body));
    checks.push(verdict(
        "note-token-plan",
        !crate::verify_site_page::page_usable(p.status, &plan_doc),
        "note token must not open a usable plan page",
    ));
    let plan_page = fetch(
        &format!("{root}/?plan={plan}&token={plan_token}"),
        temp,
        None,
        None,
    )?;
    let plan_markup =
        crate::verify_site_page::parse_page(&String::from_utf8_lossy(&plan_page.body));
    let mut voucher_url =
        url::Url::parse(&format!("{root}/voucher/{plan}/__verify_site_absent__.pdf"))
            .map_err(|e| e.to_string())?;
    if let Some(href) = plan_markup.voucher_hrefs.first() {
        voucher_url = voucher_url.join(href).map_err(|e| e.to_string())?;
    }
    voucher_url.set_query(Some(&format!("token={token}")));
    let v = fetch(voucher_url.as_str(), temp, None, None)?;
    checks.push(verdict(
        "note-token-voucher",
        v.status != 200,
        "note token must not open a voucher",
    ));
    let other = fetch(&format!("{root}/n/{second}?t={token}"), temp, None, None)?;
    checks.push(verdict(
        "other-note",
        other.status == 404,
        "other note must return 404",
    ));
    let plan_as_note = fetch(
        &format!("{root}/n/{first}?t={plan_token}"),
        temp,
        None,
        None,
    )?;
    checks.push(verdict(
        "plan-token-note",
        plan_as_note.status == 404,
        "plan token must not open a note",
    ));
    let note = fetch(&page, temp, None, None)?;
    checks.push(verdict(
        "page-headers",
        note.status == 200 && expected_headers(&note, false),
        "note page requires all four privacy headers",
    ));
    let html = String::from_utf8_lossy(&note.body);
    checks.push(verdict(
        "page-escaping",
        html.contains("&lt;script&gt;")
            && html.contains("✈️")
            && !html.contains("<a href=\"https://example.invalid"),
        "HTML must escape markup, preserve emoji, and leave URLs plain",
    ));

    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../note-core/tests/fixtures/late_exif.png");
    upload_by_cli(first, &fixture)?;
    upload_by_cli(first, &fixture)?;
    upload_by_cli(second, &fixture)?;
    let images = notes::list_images(conn, first).await?;
    let other_images = notes::list_images(conn, second).await?;
    if images.len() != 2 || other_images.len() != 1 {
        return Err("CLI uploads did not create expected image rows".into());
    }
    let image = &images[0];
    let image_url = format!("{root}/n/{first}/img/{}?t={token}", image.image_id);
    let photo = fetch(&image_url, temp, None, None)?;
    let scrubbed = note_core::scrub(&fs::read(&fixture).map_err(|e| e.to_string())?)
        .map_err(|e| format!("fixture scrub: {e:?}"))?
        .1;
    checks.push(verdict(
        "image-headers",
        photo.status == 200 && expected_headers(&photo, true),
        "photo requires privacy, nosniff, inline, and DB MIME headers",
    ));
    checks.push(verdict(
        "cli-remote-upload",
        photo.status == 200 && photo.body == scrubbed,
        "CLI scrubbed bytes must be served by the live worker",
    ));
    let cross = fetch(
        &format!(
            "{root}/n/{first}/img/{}?t={token}",
            other_images[0].image_id
        ),
        temp,
        None,
        None,
    )?;
    checks.push(verdict(
        "other-image",
        cross.status == 404,
        "other note image must return 404",
    ));

    let data = format!("title={prefix}post-%3Cscript%3E&body=copy-me");
    let before = count_run_notes(conn, prefix).await?;
    let expired = fetch(
        &format!("{root}/notes/new"),
        temp,
        Some("forged-invalid-session"),
        Some(&data),
    )?;
    let after = count_run_notes(conn, prefix).await?;
    checks.push((
        "expired-refill".into(),
        abnormal_post_check(&expired, 401, Some("&lt;script&gt;"), before, after),
    ));
    let before = count_run_notes(conn, prefix).await?;
    let no_cookie = fetch(&format!("{root}/notes/new"), temp, None, Some(&data))?;
    let after = count_run_notes(conn, prefix).await?;
    checks.push((
        "no-cookie-post".into(),
        abnormal_post_check(&no_cookie, 404, None, before, after),
    ));

    // Read the real form tokens while both notes still exist. This also lets
    // us post those forms after one note or photo has been removed.
    let owner = std::env::var("TRAVEL_VERIFY_SITE_OWNER_COOKIE").ok();
    let mut first_forms = None;
    match owner.as_deref().filter(|value| !value.is_empty()) {
        Some(cookie) => {
            let first_edit = fetch(
                &format!("{root}/notes/{first}/edit"),
                temp,
                Some(cookie),
                None,
            )?;
            let second_edit = fetch(
                &format!("{root}/notes/{second}/edit"),
                temp,
                Some(cookie),
                None,
            )?;
            let first_action = format!("/notes/{first}/edit");
            let second_action = format!("/notes/{second}/edit");
            let first_update = owner_form_field(&first_edit, &first_action, "csrf")?;
            let first_nonce = owner_form_field(&first_edit, &first_action, "form_nonce")?;
            let first_bad_image = owner_form_field(
                &first_edit,
                &format!("/notes/{first}/image/{}/up", images[0].image_id),
                "csrf",
            )?;
            let first_deleted_image = owner_form_field(
                &first_edit,
                &format!("/notes/{first}/image/{}/up", images[1].image_id),
                "csrf",
            )?;
            let second_update = owner_form_field(&second_edit, &second_action, "csrf")?;
            let second_nonce = owner_form_field(&second_edit, &second_action, "form_nonce")?;
            let second_move = owner_form_field(
                &second_edit,
                &format!("/notes/{second}/image/{}/up", other_images[0].image_id),
                "csrf",
            )?;
            let owner_photo = fetch(
                &format!("{root}/n/{first}/img/{}", image.image_id),
                temp,
                Some(cookie),
                None,
            )?;
            checks.push(verdict(
                "owner-image",
                owner_photo.status == 200 && owner_photo.body == scrubbed,
                "owner must read CLI-uploaded photo",
            ));
            let first_orders: Vec<_> = notes::list_images(conn, first)
                .await?
                .iter()
                .map(|i| i.sort_order)
                .collect();
            cleanup(conn, second, Some(prefix)).await?;
            let event_before = count_events(conn, second).await?;
            let form = format!(
                "title=missing&body=missing&form_nonce={second_nonce}&csrf={second_update}"
            );
            let response = fetch(
                &format!("{root}{second_action}"),
                temp,
                Some(cookie),
                Some(&form),
            )?;
            checks.push(verdict(
                "missing-edit",
                response.status == 404 && count_events(conn, second).await? == event_before,
                "missing note edit must return 404 without an event",
            ));
            let move_form = format!("csrf={second_move}");
            let response = fetch(
                &format!(
                    "{root}/notes/{second}/image/{}/up",
                    other_images[0].image_id
                ),
                temp,
                Some(cookie),
                Some(&move_form),
            )?;
            let first_orders_after: Vec<_> = notes::list_images(conn, first)
                .await?
                .iter()
                .map(|i| i.sort_order)
                .collect();
            checks.push(verdict(
                "missing-note-move",
                response.status == 404
                    && count_events(conn, second).await? == event_before
                    && first_orders_after == first_orders,
                "bad note image move must return 404 without an event or reorder",
            ));
            first_forms = Some((
                first_update,
                first_nonce,
                first_bad_image,
                first_deleted_image,
            ));
        }
        None => checks.push(("owner".into(), owner_availability(None))),
    }

    let changed = notes::deactivate_share_tokens(
        conn,
        &[token],
        ACTOR,
        &chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    )
    .await?;
    let revoked = fetch(&image_url, temp, None, None)?;
    checks.push(verdict(
        "revoke-image",
        changed == 1 && revoked.status == 404,
        "deactivated token must immediately lose photo access",
    ));
    let fresh = token_from_bytes(random()?);
    if !notes::insert_share_token(
        conn,
        first,
        &fresh,
        ACTOR,
        &chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    )
    .await?
    {
        return Err("fresh token insert changed zero rows".into());
    }
    if let (Some(cookie), Some((_, _, bad_image_csrf, _))) =
        (owner.as_deref(), first_forms.as_ref())
    {
        let before_events = count_events(conn, first).await?;
        r2_delete(&image.r2_key)?;
        if !notes::delete_image_row(conn, first, &image.image_id).await? {
            return Err("temporary image delete changed zero rows".into());
        }
        let before_orders: Vec<_> = notes::list_images(conn, first)
            .await?
            .iter()
            .map(|i| i.sort_order)
            .collect();
        let form = format!("csrf={bad_image_csrf}");
        let moved = fetch(
            &format!("{root}/notes/{first}/image/{}/up", image.image_id),
            temp,
            Some(cookie),
            Some(&form),
        )?;
        let after_orders: Vec<_> = notes::list_images(conn, first)
            .await?
            .iter()
            .map(|i| i.sort_order)
            .collect();
        checks.push(verdict(
            "missing-image-move",
            moved.status == 404
                && count_events(conn, first).await? == before_events
                && after_orders == before_orders,
            "bad photo move must return 404 without an event or reorder",
        ));
    }
    let deleted_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    if !notes::soft_delete_note(conn, first, ACTOR, &deleted_at).await? {
        return Err("soft delete changed zero rows".into());
    }
    let deleted = fetch(
        &format!("{root}/n/{first}/img/{}?t={fresh}", images[1].image_id),
        temp,
        None,
        None,
    )?;
    checks.push(verdict(
        "deleted-image",
        deleted.status == 404,
        "soft-deleted note must immediately lose photo access",
    ));
    if let (Some(cookie), Some((update_csrf, nonce, _, move_csrf))) =
        (owner.as_deref(), first_forms.as_ref())
    {
        let before_events = count_events(conn, first).await?;
        let before_orders: Vec<_> = notes::list_images(conn, first)
            .await?
            .iter()
            .map(|i| i.sort_order)
            .collect();
        let form = format!("title=deleted&body=deleted&form_nonce={nonce}&csrf={update_csrf}");
        let edit = fetch(
            &format!("{root}/notes/{first}/edit"),
            temp,
            Some(cookie),
            Some(&form),
        )?;
        checks.push(verdict(
            "deleted-edit",
            edit.status == 404 && count_events(conn, first).await? == before_events,
            "deleted edit must return 404 without an event",
        ));
        let move_form = format!("csrf={move_csrf}");
        let moved = fetch(
            &format!("{root}/notes/{first}/image/{}/up", images[1].image_id),
            temp,
            Some(cookie),
            Some(&move_form),
        )?;
        let after_orders: Vec<_> = notes::list_images(conn, first)
            .await?
            .iter()
            .map(|i| i.sort_order)
            .collect();
        checks.push(verdict(
            "deleted-note-move",
            moved.status == 404
                && count_events(conn, first).await? == before_events
                && after_orders == before_orders,
            "deleted note image move must return 404 without an event or reorder",
        ));
    }
    Ok(checks)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejected_post_with_a_new_note_is_a_failure() {
        let no_cookie = Http {
            status: 404,
            headers: String::new(),
            body: Vec::new(),
        };
        assert_eq!(
            abnormal_post_check(&no_cookie, 404, None, 2, 2).level,
            crate::verify_site::Level::Pass
        );
        assert_eq!(
            abnormal_post_check(&no_cookie, 404, None, 2, 3).level,
            crate::verify_site::Level::Fail
        );
        let forged = Http {
            status: 401,
            headers: String::new(),
            body: b"&lt;script&gt;".to_vec(),
        };
        assert_eq!(
            abnormal_post_check(&forged, 401, Some("&lt;script&gt;"), 2, 3).level,
            crate::verify_site::Level::Fail
        );
    }
    #[tokio::test]
    async fn cleanup_run_includes_unexpected_prefix_notes_and_children() {
        let db = libsql::Builder::new_local(":memory:")
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        for sql in [
            "CREATE TABLE notes (id TEXT PRIMARY KEY, title TEXT, created_by TEXT)",
            "CREATE TABLE note_images (note_id TEXT, image_id TEXT, sort_order INTEGER, r2_key TEXT, content_type TEXT, size_bytes INTEGER, caption TEXT)",
            "CREATE TABLE note_share_tokens (token TEXT, note_id TEXT)",
            "CREATE TABLE note_events (event_id TEXT, note_id TEXT)",
            "INSERT INTO notes VALUES ('owned', 'other title', 'verify-site')",
            "INSERT INTO notes VALUES ('unexpected', 'zz_verify_site_123_post', 'owner')",
            "INSERT INTO notes VALUES ('unrelated', 'ordinary', 'owner')",
            "INSERT INTO note_share_tokens VALUES ('token', 'unexpected')",
            "INSERT INTO note_events VALUES ('event', 'unexpected')",
        ] {
            conn.execute(sql, ()).await.unwrap();
        }
        cleanup_run(&conn, "zz_verify_site_123_").await.unwrap();
        let mut rows = conn
            .query("SELECT id FROM notes ORDER BY id", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next()
                .await
                .unwrap()
                .unwrap()
                .get::<String>(0)
                .unwrap(),
            "unrelated"
        );
        assert!(rows.next().await.unwrap().is_none());
        for table in ["note_share_tokens", "note_events"] {
            let mut rows = conn
                .query(&format!("SELECT COUNT(*) FROM {table}"), ())
                .await
                .unwrap();
            assert_eq!(
                rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
                0
            );
        }
    }
    #[test]
    fn missing_body_is_empty_but_other_read_errors_fail() {
        let temp = TempFiles::new().unwrap();
        assert_eq!(
            read_response_body(&temp.path("absent")).unwrap(),
            Vec::<u8>::new()
        );
        assert!(read_response_body(&temp.0).is_err());
    }
    #[test]
    fn csrf_is_read_from_the_exact_owner_form() {
        let html = r#"<form action="/notes/note_0123456789abcdef/edit"><input name="csrf" value="edit-token"><input name="form_nonce" value="one-time"></form><form action="/notes/note_0123456789abcdef/image/1111111111111111/up"><input name="csrf" value="move-token"></form>"#;
        assert_eq!(
            hidden_field_for_action(html, "/notes/note_0123456789abcdef/edit", "csrf"),
            Some("edit-token".into())
        );
        assert_eq!(
            hidden_field_for_action(html, "/notes/note_0123456789abcdef/edit", "form_nonce"),
            Some("one-time".into())
        );
        assert_eq!(
            hidden_field_for_action(
                html,
                "/notes/note_0123456789abcdef/image/1111111111111111/up",
                "csrf"
            ),
            Some("move-token".into())
        );
        assert_eq!(
            hidden_field_for_action(
                html,
                "/notes/note_0123456789abcdef/image/2222222222222222/up",
                "csrf"
            ),
            None
        );
    }
    #[test]
    fn missing_owner_cookie_is_a_warning() {
        let check = owner_availability(None);
        assert_eq!(check.level, crate::verify_site::Level::Warn);
        assert!(
            check
                .detail
                .contains("owner checks skipped: set TRAVEL_VERIFY_SITE_OWNER_COOKIE")
        );
        assert_eq!(
            owner_availability(Some("session")).level,
            crate::verify_site::Level::Pass
        );
    }
    #[test]
    fn privacy_headers_require_every_header() {
        let response = Http { status: 200, headers: "cache-control: private, no-store\r\nx-robots-tag: noindex\r\nreferrer-policy: no-referrer\r\nx-content-type-options: nosniff\r\ncontent-disposition: inline\r\ncontent-type: image/png\r\n".into(), body: vec![] };
        assert!(expected_headers(&response, true));
        let missing = Http {
            headers: response
                .headers
                .replace("referrer-policy: no-referrer\r\n", ""),
            ..response
        };
        assert!(!expected_headers(&missing, true));
    }
}
