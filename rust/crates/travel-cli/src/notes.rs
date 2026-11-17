// `travel notes` / `travel note ...` / `travel note-share` — travel notes CLI
// (docs/plans/2026-11-16-travel-notes.md Task 4, spec §9).
//
// Notes are an independent entity: writes go through `travel_db::repo::notes` (bound parameters
// only) and are audited ONLY by `note_events` (actor always 'cli'). They never touch
// `plan_events` / `plans.version` / `operation_runs`.
//
// Photos: the bytes are scrubbed by `note_core::scrub` BEFORE anything leaves the machine
// (unsupported format / unsanitisable file -> refused). R2 writes always carry `--remote`
// (without it wrangler writes a local simulation bucket the site cannot read) and are built by
// the pure `r2_put_args` / `r2_delete_args`. `TRAVEL_NOTES_DRY_R2=1` skips wrangler entirely.
// R2 and DB ordering: delete = read key -> delete R2 (missing object counts as done) -> DB
// transaction (row + event); R2 failure leaves the DB untouched. Add = upload -> DB transaction,
// and a failed DB write deletes the just-uploaded object again.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use libsql::Connection;
use note_core::{is_image_id, is_note_id, is_note_token};
use travel_db::repo::notes::{self, ImageRow, NoteInput, NoteRow};

use crate::cascade::common::{begin_immediate, civil_from_unix, finish_tx, now_db_datetime};

const BUCKET: &str = "trip-dashboard-notes";
const ACTOR: &str = "cli";
const MISSING_NOTE: &str = "筆記不存在或已刪除";

pub const USAGE_NOTES: &str = "travel notes [--plan-id <id>] [--day <n>]\n  List live notes as a plain-text table (attached or not). --day needs --plan-id; the plan is NEVER inferred (no resolve_plan_id, $TRAVEL_PLAN_ID ignored).";
pub const USAGE_NOTE: &str = "travel note show <id>\n  travel note add --title \"<t>\" [--body \"<t>\" | --body-file -] [--purpose \"<t>\"] [--place \"<t>\"] [--taken-on YYYY-MM-DD] [--plan-id <id> --day <n> [--dest <slug>]]\n  travel note update <id> [same flags]   (only the given fields change; an empty --purpose/--place/--taken-on clears it)\n  travel note delete <id>                (soft delete)\n  travel note add-image <id> <path> [--caption \"<t>\"]   (JPEG/PNG only; metadata scrubbed before upload)\n  travel note delete-image <id> <image_id>\n  travel note purge-deleted --older-than-days <n>   (hard-delete notes soft-deleted more than n days ago, with their R2 photos)\n  (TRAVEL_NOTES_DRY_R2=1 skips the R2 upload/delete)";
pub const USAGE_SHARE: &str = "travel note-share <id> [--show | --show-full]\n  travel note-share deactivate <token-prefix>...   (each a unique >=6-char hex prefix)\n  travel note-share clear <id> | --all             (hard-delete tokens, active and inactive; --all = every note)\n  Mint (no flag) prints the share URL; the link opens ONLY that note.";

fn db_dry_run() -> bool {
    std::env::var("TRAVEL_NOTES_DRY_R2").as_deref() == Ok("1")
}

// ── pure helpers ───────────────────────────────────────────────────────────

/// Wrangler argv (after `npx`) that uploads one object. Always `--remote`.
pub(crate) fn r2_put_args(key: &str, file: &Path, content_type: &str) -> Vec<String> {
    let mut v: Vec<String> = ["wrangler", "r2", "object", "put"].iter().map(|s| s.to_string()).collect();
    v.push(format!("{BUCKET}/{key}"));
    v.push("--file".to_string());
    v.push(file.display().to_string());
    v.push("--content-type".to_string());
    v.push(content_type.to_string());
    v.push("--remote".to_string());
    v
}

/// Wrangler argv (after `npx`) that deletes one object. Always `--remote`.
pub(crate) fn r2_delete_args(key: &str) -> Vec<String> {
    let mut v: Vec<String> = ["wrangler", "r2", "object", "delete"].iter().map(|s| s.to_string()).collect();
    v.push(format!("{BUCKET}/{key}"));
    v.push("--remote".to_string());
    v
}

/// Does wrangler's failure text say the OBJECT KEY is missing (and nothing else)? Strict on purpose.
/// Source (wrangler 4.67 `wrangler-dist/cli.js`): `r2 object delete --remote` calls the Cloudflare
/// R2 REST API; HTTP 404 returns null (exit 0, no text), any other failure throws and exits
/// non-zero. The API's "no such object" error is code 10007 (`NO_SUCH_OBJECT_KEY`, S3 `NoSuchKey`);
/// a missing BUCKET is code 10006 ("The specified bucket does not exist") and must NOT count as
/// success, nor may auth/permission/network errors. Matches only the code `10007` as a whole
/// token or the `NoSuchKey` name.
pub(crate) fn is_r2_not_found(output: &str) -> bool {
    output.contains("NoSuchKey")
        || output
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| w == "10007")
}

fn share_url(note_id: &str, token: &str) -> String {
    format!("https://{}/n/{note_id}?t={token}", crate::share_token::dashboard_host())
}

/// `deleted_at < cutoff` string for "soft-deleted more than `days` days before `now_secs`",
/// in the `YYYY-MM-DD HH:MM:SS` format the notes tables store.
pub(crate) fn purge_cutoff(now_secs: i64, days: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_unix(now_secs - days * 86_400);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

pub(crate) fn parse_purge_days(raw: &str) -> Result<i64, String> {
    match raw.parse::<i64>() {
        Ok(n) if n >= 1 => Ok(n),
        _ => Err(format!("--older-than-days must be a positive integer, got {raw:?}")),
    }
}

/// Per-note token counts a `clear` will remove: one note (`only = Some`) or every note (`None`).
/// Pure so the `--all` selection is unit-tested without touching a shared database.
pub(crate) fn clear_counts(before: &[(String, String, String)], only: Option<&str>) -> BTreeMap<String, u64> {
    let mut per_note: BTreeMap<String, u64> = BTreeMap::new();
    for (note, _, _) in before {
        if only.is_none_or(|i| i == note) {
            *per_note.entry(note.clone()).or_default() += 1;
        }
    }
    per_note
}

fn check_date(s: &str) -> Result<(), String> {
    let ok = s.len() == 10 && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok();
    if ok { Ok(()) } else { Err(format!("--taken-on must be a real date YYYY-MM-DD, got {s:?}")) }
}

fn check_note_id(s: &str) -> Result<(), String> {
    if is_note_id(s) { Ok(()) } else { Err(format!("not a note id (expected note_ + 16 lowercase hex): {s:?}")) }
}

fn check_image_id(s: &str) -> Result<(), String> {
    if is_image_id(s) { Ok(()) } else { Err(format!("not an image id (expected 16 lowercase hex): {s:?}")) }
}

fn new_hex<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("CSPRNG (getrandom) failed");
    b
}

/// Parsed argv: positionals, `--flag value` pairs, bare boolean flags. Unknown flags are errors.
#[derive(Default, Debug)]
struct Args {
    pos: Vec<String>,
    vals: BTreeMap<String, String>,
    flags: BTreeSet<String>,
}

impl Args {
    fn parse(args: &[String], value_flags: &[&str], bool_flags: &[&str]) -> Result<Self, String> {
        crate::plan_resolver::reject_unknown_flags(args, value_flags, bool_flags)?;
        let mut out = Args::default();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if value_flags.contains(&a.as_str()) {
                let v = args.get(i + 1).ok_or_else(|| format!("{a} needs a value"))?;
                if out.vals.insert(a.clone(), v.clone()).is_some() {
                    return Err(format!("{a} given twice"));
                }
                i += 2;
            } else if bool_flags.contains(&a.as_str()) {
                out.flags.insert(a.clone());
                i += 1;
            } else {
                out.pos.push(a.clone());
                i += 1;
            }
        }
        Ok(out)
    }
    fn val(&self, k: &str) -> Option<&str> {
        self.vals.get(k).map(String::as_str)
    }
    fn has(&self, k: &str) -> bool {
        self.flags.contains(k)
    }
}

fn parse_day(s: &str) -> Result<i64, String> {
    match s.parse::<i64>() {
        Ok(n) if n >= 1 => Ok(n),
        _ => Err(format!("--day must be a positive integer, got {s:?}")),
    }
}

/// `--plan-id`/`--day`/`--dest` of add/update. `--day` and `--dest` need `--plan-id` and a plan
/// needs a day; nothing is inferred.
struct Attach {
    plan: String,
    day: i64,
    dest: Option<String>,
}

fn parse_attach(a: &Args) -> Result<Option<Attach>, String> {
    match (a.val("--plan-id"), a.val("--day"), a.val("--dest")) {
        (None, None, None) => Ok(None),
        (None, _, _) => Err("--day/--dest need --plan-id (a note is attached to a plan's day)".to_string()),
        (Some(_), None, _) => Err("--plan-id needs --day <n>".to_string()),
        (Some(p), Some(d), dest) => {
            if p.trim().is_empty() {
                return Err("--plan-id is empty".to_string());
            }
            Ok(Some(Attach { plan: p.to_string(), day: parse_day(d)?, dest: dest.map(str::to_string) }))
        }
    }
}

/// Fields shared by `add` and `update` after validation (cred-free).
struct Fields {
    title: Option<String>,
    body: Option<String>,
    purpose: Option<String>,
    place: Option<String>,
    taken_on: Option<String>,
    attach: Option<Attach>,
}

const FIELD_VALUE_FLAGS: [&str; 9] =
    ["--title", "--body", "--body-file", "--purpose", "--place", "--taken-on", "--plan-id", "--day", "--dest"];

fn parse_fields(a: &Args) -> Result<Fields, String> {
    if a.val("--body").is_some() && a.val("--body-file").is_some() {
        return Err("give --body OR --body-file -, not both".to_string());
    }
    let body = match a.val("--body-file") {
        Some("-") => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).map_err(|e| format!("cannot read the body from stdin: {e}"))?;
            Some(s)
        }
        Some(other) => return Err(format!("--body-file only accepts '-' (stdin), got {other:?}")),
        None => a.val("--body").map(str::to_string),
    };
    if let Some(t) = a.val("--title") {
        if t.trim().is_empty() {
            return Err("--title must not be empty".to_string());
        }
    }
    if let Some(d) = a.val("--taken-on") {
        if !d.is_empty() {
            check_date(d)?;
        }
    }
    Ok(Fields {
        title: a.val("--title").map(str::to_string),
        body,
        purpose: a.val("--purpose").map(str::to_string),
        place: a.val("--place").map(str::to_string),
        taken_on: a.val("--taken-on").map(str::to_string),
        attach: parse_attach(a)?,
    })
}

fn clear_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.is_empty())
}

// ── R2 side effects ────────────────────────────────────────────────────────

fn r2_put(key: &str, bytes: &[u8], content_type: &str) -> Result<(), String> {
    if db_dry_run() {
        eprintln!("dry-run: would upload {key}");
        return Ok(());
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("cannot timestamp the upload: {e}"))?
        .as_nanos();
    let tmp: PathBuf = std::env::temp_dir().join(format!("travel-note-{}-{nanos}", std::process::id()));
    // Owner-only and never an existing path; removed on every exit path below.
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("cannot create temporary file {}: {e}", tmp.display()))?
    };
    let written = std::io::Write::write_all(&mut file, bytes);
    drop(file);
    let result = match written {
        Ok(()) => {
            let status = crate::snapshot_maps::wrangler_command()
                .args(r2_put_args(key, &tmp, content_type))
                .status()
                .map_err(|e| format!("failed to run Wrangler upload for {key}: {e}"));
            match status {
                Ok(s) if s.success() => Ok(()),
                Ok(s) => Err(format!("Wrangler upload failed for {key} (exit {s}); no DB row was written")),
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(format!("cannot write temporary file {}: {e}", tmp.display())),
    };
    let _ = std::fs::remove_file(&tmp);
    result
}

fn r2_delete(key: &str) -> Result<(), String> {
    if db_dry_run() {
        eprintln!("dry-run: would delete {key}");
        return Ok(());
    }
    let out = crate::snapshot_maps::wrangler_command()
        .args(r2_delete_args(key))
        .output()
        .map_err(|e| format!("failed to run Wrangler delete for {key}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if is_r2_not_found(&text) {
        return Ok(());
    }
    Err(format!("R2 delete failed for {key} (exit {}): {}", out.status, text.trim()))
}

// ── shared DB helpers ──────────────────────────────────────────────────────

async fn live_note(conn: &Connection, id: &str) -> Result<NoteRow, String> {
    match notes::get_note(conn, id).await? {
        Some(n) if n.deleted_at.is_none() => Ok(n),
        _ => Err(format!("{MISSING_NOTE}: {id}")),
    }
}

async fn resolve_attach(conn: &Connection, at: &Attach) -> Result<(String, String, i64), String> {
    if !travel_db::repo::plan_documents::plan_exists(conn, &at.plan).await? {
        return Err(format!("plan {} does not exist", at.plan));
    }
    let dest = crate::cascade::common::resolve_active_destination(conn, &at.plan, at.dest.as_deref()).await?;
    Ok((at.plan.clone(), dest, at.day))
}

// ── notes (list) ───────────────────────────────────────────────────────────

pub async fn run_notes(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &["--plan-id", "--day"], &[])?;
    if !a.pos.is_empty() {
        return Err(format!("notes takes no positional argument: {}", a.pos.join(" ")));
    }
    let day = a.val("--day").map(parse_day).transpose()?;
    if day.is_some() && a.val("--plan-id").is_none() {
        return Err("notes: --day requires --plan-id (the plan is never inferred)".to_string());
    }
    let conn = crate::db::connect_read().await?;
    let rows = notes::list_notes(&conn, a.val("--plan-id"), day).await?;
    if rows.is_empty() {
        println!("no notes");
        return Ok(());
    }
    println!("{:<21} {:<12} {:<28} {:<10} {}", "id", "taken_on", "attached", "place", "title");
    for n in rows {
        let attached = match (&n.attached_plan_id, n.attached_day_number) {
            (Some(p), Some(d)) => format!("{p} day {d}"),
            (Some(p), None) => p.clone(),
            _ => "-".to_string(),
        };
        println!(
            "{:<21} {:<12} {:<28} {:<10} {}",
            n.id,
            n.taken_on.as_deref().unwrap_or("-"),
            attached,
            n.place_name.as_deref().unwrap_or("-"),
            n.title
        );
    }
    Ok(())
}

// ── note <sub> ─────────────────────────────────────────────────────────────

pub async fn run_note(args: &[String]) -> Result<(), String> {
    let Some((sub, rest)) = args.split_first() else {
        return Err(format!("note needs a subcommand\n  {USAGE_NOTE}"));
    };
    match sub.as_str() {
        "show" => run_show(rest).await,
        "add" => run_add(rest).await,
        "update" => run_update(rest).await,
        "delete" => run_delete(rest).await,
        "add-image" => run_add_image(rest).await,
        "delete-image" => run_delete_image(rest).await,
        "purge-deleted" => run_purge(rest).await,
        other => Err(format!("unknown note subcommand: {other}\n  {USAGE_NOTE}")),
    }
}

fn one_id(a: &Args, what: &str) -> Result<String, String> {
    match a.pos.as_slice() {
        [id] => {
            check_note_id(id)?;
            Ok(id.clone())
        }
        _ => Err(format!("{what} takes exactly one note id")),
    }
}

async fn run_show(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &[], &[])?;
    let id = one_id(&a, "note show")?;
    let conn = crate::db::connect_read().await?;
    let n = notes::get_note(&conn, &id).await?.ok_or_else(|| format!("note not found: {id}"))?;
    println!("note:       {}", n.id);
    println!("title:      {}", n.title);
    match &n.deleted_at {
        Some(at) => println!("status:     deleted at {at}"),
        None => println!("status:     live"),
    }
    println!("purpose:    {}", n.purpose.as_deref().unwrap_or("-"));
    println!("place:      {}", n.place_name.as_deref().unwrap_or("-"));
    println!("taken_on:   {}", n.taken_on.as_deref().unwrap_or("-"));
    match &n.attached_plan_id {
        Some(p) => println!(
            "attached:   {p} / {} / day {}",
            n.attached_destination.as_deref().unwrap_or("-"),
            n.attached_day_number.map(|d| d.to_string()).unwrap_or_else(|| "-".into())
        ),
        None => println!("attached:   -"),
    }
    println!("created_by: {}", n.created_by);
    println!("body:\n{}", n.body);
    let images = notes::list_images(&conn, &id).await?;
    println!("images: {}", images.len());
    for i in images {
        println!(
            "  {:>2}  {}  {}  {} bytes  {}  {}",
            i.sort_order,
            i.image_id,
            i.content_type,
            i.size_bytes,
            i.r2_key,
            i.caption.as_deref().unwrap_or("")
        );
    }
    let tokens = notes::list_share_tokens(&conn, &id).await?;
    println!("share tokens: {}", tokens.len());
    for (t, status, created) in tokens {
        println!("  {}  status={status}  created {created}", crate::share_token::token_fingerprint(&t));
    }
    Ok(())
}

async fn run_add(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &FIELD_VALUE_FLAGS, &[])?;
    if !a.pos.is_empty() {
        return Err(format!("note add takes no positional argument: {}", a.pos.join(" ")));
    }
    let f = parse_fields(&a)?;
    let title = f.title.ok_or("note add: --title is required")?;
    let body = f.body.unwrap_or_default();
    let conn = crate::db::connect_write().await?;
    let attach = match &f.attach {
        Some(at) => Some(resolve_attach(&conn, at).await?),
        None => None,
    };
    let id = note_core::note_id_from_bytes(new_hex::<8>());
    let (purpose, place, taken_on) = (clear_empty(f.purpose), clear_empty(f.place), clear_empty(f.taken_on));
    let input = NoteInput {
        title: &title,
        body: &body,
        purpose: purpose.as_deref(),
        place_name: place.as_deref(),
        taken_on: taken_on.as_deref(),
        attached_plan_id: attach.as_ref().map(|t| t.0.as_str()),
        attached_destination: attach.as_ref().map(|t| t.1.as_str()),
        attached_day_number: attach.as_ref().map(|t| t.2),
    };
    let now = now_db_datetime();
    begin_immediate(&conn, "note add").await?;
    let outcome = async {
        notes::insert_note(&conn, &id, &input, ACTOR, &now).await?;
        notes::insert_event(&conn, &id, "create", ACTOR, &format!("title={title}"), &now).await
    }
    .await;
    finish_tx(&conn, "note add", outcome).await?;
    println!("created {id}");
    Ok(())
}

async fn run_update(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &FIELD_VALUE_FLAGS, &[])?;
    let id = one_id(&a, "note update")?;
    let f = parse_fields(&a)?;
    let mut changed: Vec<&str> = Vec::new();
    for (name, given) in [
        ("title", f.title.is_some()),
        ("body", f.body.is_some()),
        ("purpose", f.purpose.is_some()),
        ("place", f.place.is_some()),
        ("taken_on", f.taken_on.is_some()),
        ("attached", f.attach.is_some()),
    ] {
        if given {
            changed.push(name);
        }
    }
    if changed.is_empty() {
        return Err("note update: give at least one field to change".to_string());
    }
    let conn = crate::db::connect_write().await?;
    let cur = live_note(&conn, &id).await?;
    let attach = match &f.attach {
        Some(at) => Some(resolve_attach(&conn, at).await?),
        None => None,
    };
    let title = f.title.unwrap_or(cur.title);
    let body = f.body.unwrap_or(cur.body);
    let purpose = f.purpose.map(|v| clear_empty(Some(v))).unwrap_or(cur.purpose);
    let place = f.place.map(|v| clear_empty(Some(v))).unwrap_or(cur.place_name);
    let taken = f.taken_on.map(|v| clear_empty(Some(v))).unwrap_or(cur.taken_on);
    let (a_plan, a_dest, a_day) = match attach {
        Some((p, d, n)) => (Some(p), Some(d), Some(n)),
        None => (cur.attached_plan_id, cur.attached_destination, cur.attached_day_number),
    };
    let input = NoteInput {
        title: &title,
        body: &body,
        purpose: purpose.as_deref(),
        place_name: place.as_deref(),
        taken_on: taken.as_deref(),
        attached_plan_id: a_plan.as_deref(),
        attached_destination: a_dest.as_deref(),
        attached_day_number: a_day,
    };
    let now = now_db_datetime();
    begin_immediate(&conn, "note update").await?;
    let outcome = async {
        if !notes::update_note(&conn, &id, &input, ACTOR, &now).await? {
            return Err(format!("{MISSING_NOTE}: {id}"));
        }
        notes::insert_event(&conn, &id, "update", ACTOR, &format!("fields={}", changed.join(",")), &now).await
    }
    .await;
    finish_tx(&conn, "note update", outcome).await?;
    println!("updated {id} ({})", changed.join(", "));
    Ok(())
}

async fn run_delete(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &[], &[])?;
    let id = one_id(&a, "note delete")?;
    let conn = crate::db::connect_write().await?;
    let now = now_db_datetime();
    begin_immediate(&conn, "note delete").await?;
    let outcome = async {
        if !notes::soft_delete_note(&conn, &id, ACTOR, &now).await? {
            return Err(format!("{MISSING_NOTE}: {id}"));
        }
        notes::insert_event(&conn, &id, "delete", ACTOR, "", &now).await
    }
    .await;
    finish_tx(&conn, "note delete", outcome).await?;
    println!("deleted {id} (soft; purge-deleted removes it for good)");
    Ok(())
}

async fn run_add_image(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &["--caption"], &[])?;
    let [id, path] = a.pos.as_slice() else {
        return Err("note add-image takes <id> <path> [--caption \"<t>\"]".to_string());
    };
    check_note_id(id)?;
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read image {path}: {e}"))?;
    // Scrub BEFORE any network or DB step; refusal is final.
    let (kind, clean) = note_core::scrub(&bytes)
        .map_err(|e| format!("image refused ({e:?}); only JPEG/PNG whose metadata can be fully stripped are accepted"))?;
    let conn = crate::db::connect_write().await?;
    // Fail before touching R2 when the note is not there.
    live_note(&conn, id).await?;
    let image_id = note_core::image_id_from_bytes(new_hex::<8>());
    let key = note_core::r2_key(id, &image_id, kind);
    let img = ImageRow {
        image_id: image_id.clone(),
        sort_order: 0, // ignored by the repo (MAX+1)
        r2_key: key.clone(),
        content_type: kind.content_type().to_string(),
        size_bytes: clean.len() as i64,
        caption: a.val("--caption").filter(|c| !c.is_empty()).map(str::to_string),
    };
    r2_put(&key, &clean, kind.content_type())?;
    let now = now_db_datetime();
    let db = async {
        begin_immediate(&conn, "note add-image").await?;
        let outcome = async {
            if !notes::insert_image(&conn, id, &img, ACTOR, &now).await? {
                return Err(format!("{MISSING_NOTE}: {id}"));
            }
            notes::insert_event(&conn, id, "image_add", ACTOR, &format!("image_id={image_id} key={key}"), &now).await
        }
        .await;
        finish_tx(&conn, "note add-image", outcome).await
    }
    .await;
    if let Err(e) = db {
        // Compensate: do not leave an object no DB row points at.
        return Err(match r2_delete(&key) {
            Ok(()) => format!("{e}; the uploaded object was removed again"),
            Err(ce) => format!("{e}; ALSO could not remove the uploaded object ({ce}); remove it by hand: npx wrangler r2 object delete {BUCKET}/{key} --remote"),
        });
    }
    println!("added image {image_id} to {id} ({key}, {} bytes)", clean.len());
    Ok(())
}

async fn run_delete_image(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &[], &[])?;
    let [id, image_id] = a.pos.as_slice() else {
        return Err("note delete-image takes <id> <image_id>".to_string());
    };
    check_note_id(id)?;
    check_image_id(image_id)?;
    let conn = crate::db::connect_write().await?;
    live_note(&conn, id).await?;
    let key = notes::get_image_key(&conn, id, image_id)
        .await?
        .ok_or_else(|| format!("image not found: {id} {image_id}"))?;
    // R2 first; on failure the DB is untouched and the key stays for a retry.
    r2_delete(&key)?;
    let now = now_db_datetime();
    begin_immediate(&conn, "note delete-image").await?;
    let outcome = async {
        if !notes::delete_image_row(&conn, id, image_id).await? {
            return Err(format!("image row already gone or note deleted: {id} {image_id}"));
        }
        notes::insert_event(&conn, id, "image_delete", ACTOR, &format!("image_id={image_id} key={key}"), &now).await
    }
    .await;
    finish_tx(&conn, "note delete-image", outcome).await?;
    println!("deleted image {image_id} of {id}");
    Ok(())
}

async fn run_purge(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &["--older-than-days"], &[])?;
    if !a.pos.is_empty() {
        return Err(format!("note purge-deleted takes no positional argument: {}", a.pos.join(" ")));
    }
    let raw = a.val("--older-than-days").ok_or("note purge-deleted needs an explicit --older-than-days <n> (Yang's rule: 30)")?;
    let days = parse_purge_days(raw)?;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .map_err(|e| format!("clock error: {e}"))?;
    let cutoff = purge_cutoff(now_secs, days);
    let conn = crate::db::connect_write().await?;
    let candidates = notes::deleted_before(&conn, &cutoff).await?;
    let (mut purged, mut skipped) = (0usize, Vec::<String>::new());
    for (id, keys) in candidates {
        // All R2 objects first; one failure keeps the whole note (row + remaining keys) for a retry.
        if let Some(err) = keys.iter().find_map(|k| r2_delete(k).err()) {
            eprintln!("skip {id}: {err}");
            skipped.push(id);
            continue;
        }
        let now = now_db_datetime();
        begin_immediate(&conn, "note purge-deleted").await?;
        let outcome = async {
            if notes::purge_note(&conn, &id).await? == 0 {
                return Err(format!("note {id} was already gone; no purge event written"));
            }
            notes::insert_event(&conn, &id, "purge", ACTOR, &format!("images={}", keys.len()), &now).await
        }
        .await;
        finish_tx(&conn, "note purge-deleted", outcome).await?;
        println!("purged {id} ({} images)", keys.len());
        purged += 1;
    }
    println!("purged {purged} note(s) soft-deleted before {cutoff}");
    if skipped.is_empty() {
        Ok(())
    } else {
        Err(format!("{} note(s) skipped (R2 delete failed; rows kept, retry later): {}", skipped.len(), skipped.join(", ")))
    }
}

// ── note-share ─────────────────────────────────────────────────────────────

pub async fn run_share(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("deactivate") => share_deactivate(&args[1..]).await,
        Some("clear") => share_clear(&args[1..]).await,
        _ => share_mint_or_show(args).await,
    }
}

async fn share_mint_or_show(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &[], &["--show", "--show-full"])?;
    let id = one_id(&a, "note-share")?;
    if a.has("--show") && a.has("--show-full") {
        return Err("give --show OR --show-full, not both".to_string());
    }
    if a.has("--show") || a.has("--show-full") {
        let conn = crate::db::connect_read().await?;
        notes::get_note(&conn, &id).await?.ok_or_else(|| format!("note not found: {id}"))?;
        let tokens = notes::list_share_tokens(&conn, &id).await?;
        if tokens.is_empty() {
            println!("no share tokens for {id}");
        }
        for (t, status, created) in tokens {
            println!("token: {}  status={status}  created {created}", crate::share_token::token_fingerprint(&t));
            if a.has("--show-full") {
                println!("url:   {}", share_url(&id, &t));
            }
        }
        return Ok(());
    }
    let conn = crate::db::connect_write().await?;
    let token = note_core::token_from_bytes(new_hex::<16>());
    debug_assert!(is_note_token(&token));
    let now = now_db_datetime();
    begin_immediate(&conn, "note-share").await?;
    let outcome = async {
        if !notes::insert_share_token(&conn, &id, &token, ACTOR, &now).await? {
            return Err(format!("{MISSING_NOTE}: {id}"));
        }
        let fp = crate::share_token::token_fingerprint(&token);
        notes::insert_event(&conn, &id, "share_create", ACTOR, &format!("token={fp}"), &now).await
    }
    .await;
    finish_tx(&conn, "note-share", outcome).await?;
    println!("{}", share_url(&id, &token));
    Ok(())
}

async fn share_deactivate(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &[], &[])?;
    if a.pos.is_empty() {
        return Err("note-share deactivate needs at least one token prefix".to_string());
    }
    for p in &a.pos {
        let hex = p.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if p.len() < 6 || p.len() > 32 || !hex {
            return Err(format!("token prefix must be 6-32 lowercase hex chars: {p:?}"));
        }
    }
    let conn = crate::db::connect_write().await?;
    let all = notes::list_all_share_tokens(&conn).await?;
    // prefix -> exactly one ACTIVE token; ambiguity or no match is an error before any write.
    let mut targets: BTreeMap<String, String> = BTreeMap::new(); // token -> note_id
    for p in &a.pos {
        let hits: Vec<_> = all.iter().filter(|(_, t, s)| s == "active" && t.starts_with(p.as_str())).collect();
        match hits.as_slice() {
            [] => return Err(format!("no active share token starts with {p}")),
            [(note, tok, _)] => {
                targets.insert(tok.clone(), note.clone());
            }
            _ => return Err(format!("prefix {p} matches several active tokens; give more characters")),
        }
    }
    let now = now_db_datetime();
    let full: Vec<&str> = targets.keys().map(String::as_str).collect();
    begin_immediate(&conn, "note-share deactivate").await?;
    let outcome = async {
        let n = notes::deactivate_share_tokens(&conn, &full, ACTOR, &now).await?;
        if n != full.len() as u64 {
            return Err(format!("expected to deactivate {} token(s), changed {n}", full.len()));
        }
        for (tok, note) in &targets {
            let fp = crate::share_token::token_fingerprint(tok);
            notes::insert_event(&conn, note, "share_deactivate", ACTOR, &format!("token={fp}"), &now).await?;
        }
        Ok(n)
    }
    .await;
    let n = finish_tx(&conn, "note-share deactivate", outcome).await?;
    println!("deactivated {n} token(s)");
    Ok(())
}

async fn share_clear(args: &[String]) -> Result<(), String> {
    let a = Args::parse(args, &[], &["--all"])?;
    let id = match (a.pos.as_slice(), a.has("--all")) {
        ([], true) => None,
        ([id], false) => {
            check_note_id(id)?;
            Some(id.clone())
        }
        _ => return Err("note-share clear needs exactly one scope: <note id> OR --all".to_string()),
    };
    let conn = crate::db::connect_write().await?;
    if let Some(id) = &id {
        notes::get_note(&conn, id).await?.ok_or_else(|| format!("note not found: {id}"))?;
    }
    // Per-note counts so each affected note gets its `share_clear` event.
    let before = notes::list_all_share_tokens(&conn).await?;
    let per_note = clear_counts(&before, id.as_deref());
    if per_note.is_empty() {
        println!("no share tokens to clear");
        return Ok(());
    }
    let expected: u64 = per_note.values().sum();
    let now = now_db_datetime();
    begin_immediate(&conn, "note-share clear").await?;
    let outcome = async {
        let n = notes::clear_share_tokens(&conn, id.as_deref()).await?;
        if n != expected {
            return Err(format!("expected to remove {expected} token(s), removed {n} (concurrent change?)"));
        }
        for (note, c) in &per_note {
            notes::insert_event(&conn, note, "share_clear", ACTOR, &format!("removed={c}"), &now).await?;
        }
        Ok(n)
    }
    .await;
    let n = finish_tx(&conn, "note-share clear", outcome).await?;
    println!("removed {n} share token(s) from {} note(s)", per_note.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrangler_args_include_remote() {
        let put = r2_put_args("note_0123456789abcdef/0011223344556677.jpg", Path::new("/tmp/x"), "image/jpeg");
        assert_eq!(&put[..4], ["wrangler", "r2", "object", "put"]);
        assert_eq!(put[4], "trip-dashboard-notes/note_0123456789abcdef/0011223344556677.jpg");
        assert!(put.contains(&"--remote".to_string()), "{put:?}");
        let ct = put.iter().position(|a| a == "--content-type").unwrap();
        assert_eq!(put[ct + 1], "image/jpeg");
        let del = r2_delete_args("note_0123456789abcdef/0011223344556677.png");
        assert_eq!(&del[..4], ["wrangler", "r2", "object", "delete"]);
        assert_eq!(del[4], "trip-dashboard-notes/note_0123456789abcdef/0011223344556677.png");
        assert!(del.contains(&"--remote".to_string()), "{del:?}");
    }

    #[test]
    fn not_found_classifier() {
        // missing KEY -> success
        assert!(is_r2_not_found("X [ERROR] The specified key does not exist. [code: 10007]"));
        assert!(is_r2_not_found("NoSuchKey"));
        // missing BUCKET, auth, permission, network, other codes -> failure
        assert!(!is_r2_not_found("X [ERROR] The specified bucket does not exist. [code: 10006]"));
        assert!(!is_r2_not_found("NoSuchBucket: The specified bucket does not exist"));
        assert!(!is_r2_not_found("Authentication error [code: 10000]"));
        assert!(!is_r2_not_found("Access Denied (403) Forbidden"));
        assert!(!is_r2_not_found("Failed to fetch /accounts/a/r2/buckets/b/objects/k - 500: Internal Server Error"));
        assert!(!is_r2_not_found("network timeout"));
        assert!(!is_r2_not_found("the page was not found"));
        assert!(!is_r2_not_found("request id 1000712 failed")); // 10007 only as a whole token
    }

    #[test]
    fn share_url_opens_only_the_note_route() {
        let u = share_url("note_0123456789abcdef", "00112233445566778899aabbccddeeff");
        assert!(u.ends_with("/n/note_0123456789abcdef?t=00112233445566778899aabbccddeeff"), "{u}");
        assert!(!u.contains("token="), "?t= not ?token=");
    }

    #[test]
    fn purge_cutoff_is_days_before_now() {
        // 2026-11-17 00:00:00 UTC
        let now = 1_794_873_600;
        assert_eq!(purge_cutoff(now, 30), "2026-10-18 00:00:00");
        assert_eq!(purge_cutoff(now, 1), "2026-11-16 00:00:00");
    }

    #[test]
    fn purge_days_must_be_explicit_positive() {
        assert_eq!(parse_purge_days("30"), Ok(30));
        for bad in ["", "0", "-1", "abc", "1.5"] {
            assert!(parse_purge_days(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn clear_counts_scoped_vs_all() {
        let t = |n: &str, k: &str, st: &str| (n.to_string(), k.to_string(), st.to_string());
        let rows = vec![t("note_a", "t1", "active"), t("note_a", "t2", "inactive"), t("note_b", "t3", "active")];
        let one = clear_counts(&rows, Some("note_a"));
        assert_eq!(one.len(), 1);
        assert_eq!(one["note_a"], 2, "active and inactive both counted");
        let all = clear_counts(&rows, None);
        assert_eq!((all["note_a"], all["note_b"], all.len()), (2, 1, 2));
        assert!(clear_counts(&rows, Some("note_zzz")).is_empty());
        assert!(clear_counts(&[], None).is_empty());
    }

    #[test]
    fn taken_on_must_be_a_real_date() {
        assert!(check_date("2026-11-20").is_ok());
        assert!(check_date("2026-13-40").is_err());
        assert!(check_date("2026-2-3").is_err());
        assert!(check_date("tomorrow").is_err());
    }

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn attach_rules() {
        let p = |a: &[&str]| Args::parse(&v(a), &FIELD_VALUE_FLAGS, &[]).and_then(|a| parse_attach(&a).map(|o| o.is_some()));
        assert_eq!(p(&[]), Ok(false));
        assert_eq!(p(&["--plan-id", "x", "--day", "2"]), Ok(true));
        assert!(p(&["--plan-id", "x"]).is_err());
        assert!(p(&["--day", "2"]).is_err());
        assert!(p(&["--dest", "d"]).is_err());
        assert!(p(&["--plan-id", "x", "--day", "0"]).is_err());
    }

    #[test]
    fn args_reject_unknown_and_valueless_flags() {
        assert!(Args::parse(&v(&["--bogus"]), &[], &[]).is_err());
        assert!(Args::parse(&v(&["--title"]), &["--title"], &[]).is_err());
        assert!(Args::parse(&v(&["--title", "a", "--title", "b"]), &["--title"], &[]).is_err());
        let a = Args::parse(&v(&["note_x", "--title", "--weird"]), &["--title"], &[]).unwrap();
        assert_eq!(a.val("--title"), Some("--weird"));
        assert_eq!(a.pos, ["note_x"]);
    }
}
