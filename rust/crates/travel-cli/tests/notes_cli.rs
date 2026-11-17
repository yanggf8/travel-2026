//! Real-Turso tests for the `notes` / `note ...` / `note-share` commands.
//!
//! Every binary run sets `TRAVEL_NOTES_DRY_R2=1`: R2 is never touched and wrangler never runs.
//! Fixtures are per-run unique (`zz_test_<nanos>` titles and plan ids); the RAII `Guard` is armed
//! right after the tag is bound and deletes only rows reachable from THIS run's tagged notes.
//! The table-wide forms (`note-share clear --all`, `note purge-deleted`) are NEVER executed here
//! (shared Turso); their selection logic is covered by pure unit tests in `src/notes.rs` and the
//! repo tests. Skips cleanly without credentials.

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard};
use std::io::Write;
use std::process::{Command, Stdio};

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn run_stdin(args: &[&str], stdin: Option<&str>) -> Out {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .env("TRAVEL_NOTES_DRY_R2", "1")
        .env_remove("TRAVEL_PLAN_ID")
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn travel");
    if let Some(s) = stdin {
        child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    }
    let o = child.wait_with_output().expect("wait travel");
    Out {
        ok: o.status.success(),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

fn run(args: &[&str]) -> Out {
    run_stdin(args, None)
}

/// `false` = credentials explicitly absent (skip). Any other migrate failure panics.
fn migrate() -> bool {
    static DONE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DONE.get_or_init(|| {
        let o = Command::new(bin()).args(["db", "migrate"]).output().expect("db migrate");
        let err = String::from_utf8_lossy(&o.stderr);
        if !o.status.success() && is_credless(&err) {
            eprintln!("skipping (no creds): {}", err.trim());
            return false;
        }
        assert!(o.status.success(), "db migrate failed: {err}");
        true
    })
}

/// Deletes everything reachable from notes whose title starts with this run's tag.
fn cleanup(tag: &str) {
    let sub = format!("SELECT id FROM notes WHERE title LIKE '{tag}%'");
    let _ = db_exec_teardown(&format!(
        "DELETE FROM note_events WHERE note_id IN ({sub}); \
         DELETE FROM note_share_tokens WHERE note_id IN ({sub}); \
         DELETE FROM note_images WHERE note_id IN ({sub}); \
         DELETE FROM notes WHERE title LIKE '{tag}%';"
    ));
}

fn tag() -> String {
    format!("zz_test_{}", nanos())
}

/// First `note_` + 16 hex word in `s`.
fn find_note_id(s: &str) -> String {
    s.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .find(|w| note_core::is_note_id(w))
        .unwrap_or_else(|| panic!("no note id in output: {s}"))
        .to_string()
}

fn count(sql: &str) -> i64 {
    db_exec(sql).expect("creds").scalar().expect("scalar").parse().expect("int")
}

fn events(id: &str, action: &str) -> i64 {
    count(&format!("SELECT COUNT(*) FROM note_events WHERE note_id = '{id}' AND action = '{action}'"))
}

fn fixture(name: &str) -> String {
    format!("{}/../note-core/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn add_show_update_delete_flow() {
    if !migrate() {
        return;
    }
    let t = tag();
    let plan = format!("{t}_p");
    let dest = format!("{t}_d");
    seed_plan(&plan, &dest, 7);
    let _g = Guard::new({
        let (t, plan, dest) = (t.clone(), plan.clone(), dest.clone());
        move || {
            cleanup(&t);
            teardown_plan(&plan, &dest);
        }
    });
    let version = || count(&format!("SELECT version FROM plans WHERE plan_id = '{plan}'"));
    let plan_events = || count(&format!("SELECT COUNT(*) FROM plan_events WHERE plan_id = '{plan}'"));
    let ops = || count(&format!("SELECT COUNT(*) FROM operation_runs WHERE plan_id = '{plan}'"));
    let (v0, e0, o0) = (version(), plan_events(), ops());

    let title = format!("{t}_title");
    let o = run(&[
        "note", "add", "--title", &title, "--body", "line one \"quoted\" 🍫", "--purpose", "for daughter",
        "--place", "GODIVA", "--taken-on", "2026-11-20", "--plan-id", &plan, "--day", "2",
    ]);
    assert!(o.ok, "add failed: {}", o.stderr);
    let id = find_note_id(&o.stdout);
    assert_eq!(events(&id, "create"), 1);
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) FROM notes WHERE id = '{id}' AND attached_plan_id = '{plan}' \
             AND attached_destination = '{dest}' AND attached_day_number = 2 AND created_by = 'cli'"
        )),
        1
    );

    let o = run(&["note", "show", &id]);
    assert!(o.ok, "show failed: {}", o.stderr);
    for want in [title.as_str(), "line one \"quoted\" 🍫", "for daughter", "GODIVA", "2026-11-20"] {
        assert!(o.stdout.contains(want), "show lacks {want:?}: {}", o.stdout);
    }
    assert!(!o.stdout.trim_start().starts_with('{'), "show must be plain text, not JSON");

    // update touches only the given field
    let new_title = format!("{t}_title2");
    let o = run(&["note", "update", &id, "--title", &new_title]);
    assert!(o.ok, "update failed: {}", o.stderr);
    assert_eq!(events(&id, "update"), 1);
    let o = run(&["note", "show", &id]);
    assert!(o.stdout.contains(&new_title) && o.stdout.contains("for daughter") && o.stdout.contains("line one"));

    let o = run(&["notes", "--plan-id", &plan, "--day", "2"]);
    assert!(o.ok && o.stdout.contains(&id), "notes list should show the note: {}{}", o.stdout, o.stderr);
    let o = run(&["notes", "--plan-id", &plan, "--day", "3"]);
    assert!(o.ok && !o.stdout.contains(&id), "day filter must exclude: {}", o.stdout);

    let o = run(&["note", "delete", &id]);
    assert!(o.ok, "delete failed: {}", o.stderr);
    assert_eq!(events(&id, "delete"), 1);
    let o = run(&["notes", "--plan-id", &plan]);
    assert!(!o.stdout.contains(&id), "deleted note must leave the list");
    let o = run(&["note", "show", &id]);
    assert!(o.ok && o.stdout.contains("deleted"), "show marks a soft-deleted note: {}", o.stdout);
    // a second delete / an update of a deleted note is a no-op error with no new event
    assert!(!run(&["note", "delete", &id]).ok);
    assert!(!run(&["note", "update", &id, "--title", "x"]).ok);
    assert_eq!(events(&id, "delete"), 1);
    assert_eq!(events(&id, "update"), 1);

    // the plan audit triad was never touched
    assert_eq!((version(), plan_events(), ops()), (v0, e0, o0));
}

#[test]
fn body_file_stdin_roundtrips_unicode() {
    if !migrate() {
        return;
    }
    let t = tag();
    let _g = Guard::new({
        let t = t.clone();
        move || cleanup(&t)
    });
    let o = run_stdin(
        &["note", "add", "--title", &format!("{t}_stdin"), "--body-file", "-"],
        Some("第一行\n<script>alert(1)</script> https://x.test 🎮\n"),
    );
    assert!(o.ok, "{}", o.stderr);
    let id = find_note_id(&o.stdout);
    let o = run(&["note", "show", &id]);
    assert!(o.stdout.contains("第一行") && o.stdout.contains("<script>alert(1)</script>") && o.stdout.contains("🎮"));
}

#[test]
fn add_image_rejects_heic() {
    if !migrate() {
        return;
    }
    let t = tag();
    let _g = Guard::new({
        let t = t.clone();
        move || cleanup(&t)
    });
    let o = run(&["note", "add", "--title", &format!("{t}_heic")]);
    let id = find_note_id(&o.stdout);
    let o = run(&["note", "add-image", &id, &fixture("fake.heic")]);
    assert!(!o.ok, "HEIC must be refused");
    assert!(!o.stderr.contains("would upload"), "nothing may reach the upload step: {}", o.stderr);
    assert_eq!(count(&format!("SELECT COUNT(*) FROM note_images WHERE note_id = '{id}'")), 0);
    assert_eq!(events(&id, "image_add"), 0);
}

#[test]
fn add_image_dry_run_writes_row_with_db_key() {
    if !migrate() {
        return;
    }
    let t = tag();
    let _g = Guard::new({
        let t = t.clone();
        move || cleanup(&t)
    });
    let o = run(&["note", "add", "--title", &format!("{t}_img")]);
    let id = find_note_id(&o.stdout);
    let o = run(&["note", "add-image", &id, &fixture("late_exif.png"), "--caption", "front 🍫"]);
    assert!(o.ok, "add-image failed: {}", o.stderr);
    assert!(o.stderr.contains("dry-run: would upload"), "{}", o.stderr);
    let key = db_exec(&format!("SELECT r2_key FROM note_images WHERE note_id = '{id}'"))
        .unwrap()
        .scalar()
        .unwrap();
    let (prefix, file) = key.split_once('/').expect("key is <note>/<image>.<ext>");
    assert_eq!(prefix, id);
    let (image_id, ext) = file.split_once('.').expect("extension");
    assert!(note_core::is_image_id(image_id), "image id format: {image_id}");
    assert_eq!(key, note_core::r2_key(&id, image_id, note_core::ImageKind::Png));
    assert_eq!(ext, "png");
    assert!(o.stderr.contains(&key), "dry-run line names the DB key: {}", o.stderr);
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) FROM note_images WHERE note_id = '{id}' AND content_type = 'image/png' \
             AND sort_order = 1 AND caption = 'front 🍫'"
        )),
        1
    );
    assert_eq!(events(&id, "image_add"), 1);

    // a second photo appends
    let o = run(&["note", "add-image", &id, &fixture("iphone_portrait_gps.jpg")]);
    assert!(o.ok, "{}", o.stderr);
    assert_eq!(count(&format!("SELECT MAX(sort_order) FROM note_images WHERE note_id = '{id}'")), 2);

    // delete-image: unknown image fails, real one removes the row and logs
    let bogus = "0123456789abcdef";
    assert!(!run(&["note", "delete-image", &id, bogus]).ok);
    assert_eq!(events(&id, "image_delete"), 0);
    let o = run(&["note", "delete-image", &id, image_id]);
    assert!(o.ok, "delete-image failed: {}", o.stderr);
    assert!(o.stderr.contains("dry-run: would delete"), "{}", o.stderr);
    assert_eq!(count(&format!("SELECT COUNT(*) FROM note_images WHERE note_id = '{id}'")), 1);
    assert_eq!(events(&id, "image_delete"), 1);
}

#[test]
fn add_image_to_missing_or_deleted_note_fails_before_upload() {
    if !migrate() {
        return;
    }
    let t = tag();
    let _g = Guard::new({
        let t = t.clone();
        move || cleanup(&t)
    });
    let missing = format!("note_{:016x}", (nanos() as u64) ^ 0x5a5a);
    let o = run(&["note", "add-image", &missing, &fixture("late_exif.png")]);
    assert!(!o.ok && !o.stderr.contains("would upload"), "{}", o.stderr);
    assert_eq!(count(&format!("SELECT COUNT(*) FROM note_images WHERE note_id = '{missing}'")), 0);
    assert_eq!(events(&missing, "image_add"), 0);

    let o = run(&["note", "add", "--title", &format!("{t}_del")]);
    let id = find_note_id(&o.stdout);
    assert!(run(&["note", "delete", &id]).ok);
    let o = run(&["note", "add-image", &id, &fixture("late_exif.png")]);
    assert!(!o.ok && !o.stderr.contains("would upload"), "{}", o.stderr);
    assert_eq!(events(&id, "image_add"), 0);
}

#[test]
fn notes_day_without_plan_errors() {
    // Fails before any DB access: no credentials needed, never skipped.
    let o = run(&["notes", "--day", "2"]);
    assert!(!o.ok);
    assert!(o.stderr.contains("--plan-id"), "{}", o.stderr);
    // $TRAVEL_PLAN_ID is NOT a selector for `notes`
    let mut cmd = Command::new(bin());
    let out = cmd.args(["notes", "--day", "2"]).env("TRAVEL_PLAN_ID", "whatever").output().unwrap();
    assert!(!out.status.success());
}

#[test]
fn argument_validation_fails_before_db() {
    for args in [
        vec!["note", "add"],                                                        // no title
        vec!["note", "add", "--title", "x", "--plan-id", "p"],                      // plan without day
        vec!["note", "add", "--title", "x", "--day", "2"],                          // day without plan
        vec!["note", "add", "--title", "x", "--taken-on", "2026-13-40"],            // bad date
        vec!["note", "add", "--title", "x", "--body", "a", "--body-file", "-"],     // both bodies
        vec!["note", "add", "--title", "x", "--bogus", "1"],                        // unknown flag
        vec!["note", "show", "not-an-id"],
        vec!["note", "update", "note_0123456789abcdef"],                            // nothing to update
        vec!["note", "delete", "../etc"],
        vec!["note-share", "deactivate"],                                           // no prefix
        vec!["note-share", "deactivate", "abc"],                                    // prefix too short
        vec!["note-share", "clear"],                                                // no scope
        vec!["note-share", "clear", "note_0123456789abcdef", "--all"],              // two scopes
    ] {
        let o = run(&args);
        assert!(!o.ok, "{args:?} must fail; stdout={}", o.stdout);
        assert!(!o.stderr.trim().is_empty(), "{args:?} must explain itself");
    }
}

#[test]
fn update_missing_note_fails_no_event() {
    if !migrate() {
        return;
    }
    let missing = format!("note_{:016x}", (nanos() as u64) ^ 0xa5a5);
    let o = run(&["note", "update", &missing, "--title", "zz_test_never"]);
    assert!(!o.ok, "update of a missing note must fail");
    assert!(o.stderr.contains("不存在或已刪除"), "{}", o.stderr);
    let o = run(&["note", "delete", &missing]);
    assert!(!o.ok);
    assert_eq!(count(&format!("SELECT COUNT(*) FROM note_events WHERE note_id = '{missing}'")), 0);
}

#[test]
fn note_share_mint_deactivate_clear_one() {
    if !migrate() {
        return;
    }
    let t = tag();
    let _g = Guard::new({
        let t = t.clone();
        move || cleanup(&t)
    });
    let a = find_note_id(&run(&["note", "add", "--title", &format!("{t}_a")]).stdout);
    let b = find_note_id(&run(&["note", "add", "--title", &format!("{t}_b")]).stdout);

    let o = run(&["note-share", &a]);
    assert!(o.ok, "{}", o.stderr);
    let url = o.stdout.lines().find(|l| l.contains("/n/")).unwrap_or_else(|| panic!("no url: {}", o.stdout));
    let token_a = url.split("?t=").nth(1).unwrap().trim().to_string();
    assert!(note_core::is_note_token(&token_a), "{token_a}");
    assert!(url.contains(&format!("/n/{a}?t=")), "{url}");
    assert_eq!(events(&a, "share_create"), 1);
    // the full token must not be stored in the audit trail
    assert_eq!(
        count(&format!("SELECT COUNT(*) FROM note_events WHERE note_id = '{a}' AND detail LIKE '%{token_a}%'")),
        0
    );
    let tok_b = find_token(&run(&["note-share", &b]).stdout);

    // --show hides the middle, --show-full prints the URL
    let o = run(&["note-share", &a, "--show"]);
    assert!(o.ok && !o.stdout.contains(&token_a) && o.stdout.contains(&token_a[..6]), "{}", o.stdout);
    let o = run(&["note-share", &a, "--show-full"]);
    assert!(o.ok && o.stdout.contains(&token_a), "{}", o.stdout);

    // deactivate by prefix flips the status and logs on the owning note
    let o = run(&["note-share", "deactivate", &token_a[..8]]);
    assert!(o.ok, "{}", o.stderr);
    assert_eq!(
        count(&format!("SELECT COUNT(*) FROM note_share_tokens WHERE token = '{token_a}' AND status = 'inactive'")),
        1
    );
    assert_eq!(events(&a, "share_deactivate"), 1);
    assert!(!run(&["note-share", "deactivate", &token_a[..8]]).ok, "already inactive: no match");
    assert_eq!(events(&a, "share_deactivate"), 1);

    // clear <id> removes only that note's tokens
    let o = run(&["note-share", "clear", &a]);
    assert!(o.ok, "{}", o.stderr);
    assert_eq!(count(&format!("SELECT COUNT(*) FROM note_share_tokens WHERE note_id = '{a}'")), 0);
    assert_eq!(count(&format!("SELECT COUNT(*) FROM note_share_tokens WHERE token = '{tok_b}'")), 1);
    assert_eq!(events(&a, "share_clear"), 1);

    // minting for a deleted note fails and writes nothing
    assert!(run(&["note", "delete", &b]).ok);
    let o = run(&["note-share", &b]);
    assert!(!o.ok, "share of a deleted note must fail");
    assert_eq!(events(&b, "share_create"), 1, "only the earlier mint is logged");
}

fn find_token(s: &str) -> String {
    let line = s.lines().find(|l| l.contains("?t=")).unwrap_or_else(|| panic!("no url: {s}"));
    line.split("?t=").nth(1).unwrap().trim().to_string()
}

#[test]
fn purge_requires_days_flag() {
    // Fails before any DB access.
    let o = run(&["note", "purge-deleted"]);
    assert!(!o.ok);
    assert!(o.stderr.contains("--older-than-days"), "{}", o.stderr);
    for bad in ["abc", "-1", "0", ""] {
        let o = run(&["note", "purge-deleted", "--older-than-days", bad]);
        assert!(!o.ok, "--older-than-days {bad:?} must be refused");
    }
}
