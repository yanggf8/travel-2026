//! Real-Turso tests for the `notes` schema (db migrate) and the `travel_db::repo::notes` DAL.
//! Fixtures are per-run unique (`note_` + 16 hex derived from nanos; plan `zz_test_<nanos>_*`);
//! the RAII `Guard` is armed right after the ids are bound. Skips cleanly without creds.

mod common;
use common::{bin, db_exec, is_credless, nanos, seed_plan, teardown_notes, teardown_plan, Guard};
use std::process::Command;
use travel_db::repo::notes::{self, ImageRow, NoteInput};

const NOW: &str = "2026-11-16 10:00:00";

/// `None` only when the credentials are explicitly absent (skip). Any connect/auth failure with
/// credentials present PANICS: a broken connection must never read as a green test.
async fn connect_write() -> Option<libsql::Connection> {
    let url = std::env::var("TRAVEL_TURSO_URL").or_else(|_| std::env::var("TURSO_URL")).ok()?;
    let token = std::env::var("TRAVEL_TURSO_WRITE_TOKEN").or_else(|_| std::env::var("TURSO_TOKEN")).ok()?;
    let db = libsql::Builder::new_remote(url, token)
        .build()
        .await
        .unwrap_or_else(|e| panic!("Turso creds present but connect failed: {e}"));
    Some(db.connect().unwrap_or_else(|e| panic!("Turso creds present but open failed: {e}")))
}

/// `note_` + 16 lowercase hex, unique per run and per `salt`.
fn note_id(salt: u64) -> String {
    format!("note_{:016x}", (nanos() as u64).wrapping_mul(31).wrapping_add(salt))
}

/// Runs `db migrate` once per test binary (idempotent anyway). `false` = credentials explicitly
/// absent (skip); any other failure panics.
fn migrate() -> bool {
    static DONE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DONE.get_or_init(|| {
        let out = Command::new(bin()).args(["db", "migrate"]).output().expect("run db migrate");
        let err = String::from_utf8_lossy(&out.stderr);
        if !out.status.success() && is_credless(&err) {
            eprintln!("skipping (no creds): {}", err.trim());
            return false;
        }
        assert!(out.status.success(), "db migrate failed: {err}");
        true
    })
}

/// 32-hex token: `<16-hex per-run base><16-hex tail>` so tests can share a prefix; unique per run.
fn token(base: &str, tail: &str) -> String {
    format!("{base}{tail}")
}

fn input<'a>(title: &'a str, body: &'a str) -> NoteInput<'a> {
    NoteInput {
        title,
        body,
        purpose: None,
        place_name: None,
        taken_on: None,
        attached_plan_id: None,
        attached_destination: None,
        attached_day_number: None,
    }
}

fn img(image_id: &str, note: &str) -> ImageRow {
    ImageRow {
        image_id: image_id.to_string(),
        sort_order: 0, // ignored by insert_image (appends)
        r2_key: format!("{note}/{image_id}.jpg"),
        content_type: "image/jpeg".into(),
        size_bytes: 1234,
        caption: None,
    }
}

macro_rules! setup {
    ($ids:expr) => {{
        let ids: Vec<String> = $ids;
        let _g = Guard::new({
            let ids = ids.clone();
            move || {
                let refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
                teardown_notes(&refs)
            }
        });
        if !migrate() {
            return;
        }
        let Some(conn) = connect_write().await else {
            eprintln!("skipping (no Turso creds in env)");
            return;
        };
        (ids, _g, conn)
    }};
}

#[tokio::test]
async fn insert_get_roundtrip_unicode() {
    let id = note_id(1);
    let (_ids, _g, conn) = setup!(vec![id.clone()]);
    let title = "GODIVA ✈️ 'quote'; --";
    let body = "line1\nline2 \"dq\" 🍫; DROP TABLE notes; --";
    let mut n = input(title, body);
    n.purpose = Some("給女兒（DQ 參考）");
    n.place_name = Some("イオンモールKYOTO 櫻館");
    n.taken_on = Some("2026-11-15");
    n.attached_destination = Some("kyoto_nov_2026");
    n.attached_day_number = Some(4);
    notes::insert_note(&conn, &id, &n, "yanggf8", NOW).await.expect("insert");
    let got = notes::get_note(&conn, &id).await.expect("get").expect("row");
    assert_eq!(got.id, id);
    assert_eq!(got.title, title);
    assert_eq!(got.body, body);
    assert_eq!(got.purpose.as_deref(), Some("給女兒（DQ 參考）"));
    assert_eq!(got.place_name.as_deref(), Some("イオンモールKYOTO 櫻館"));
    assert_eq!(got.taken_on.as_deref(), Some("2026-11-15"));
    assert_eq!(got.attached_plan_id, None);
    assert_eq!(got.attached_destination.as_deref(), Some("kyoto_nov_2026"));
    assert_eq!(got.attached_day_number, Some(4));
    assert_eq!(got.created_by, "yanggf8");
    assert_eq!(got.deleted_at, None);
    // The table survived the injection-shaped body.
    assert!(notes::get_note(&conn, &id).await.unwrap().is_some());
    assert!(notes::get_note(&conn, "note_0000000000000000").await.unwrap().is_none());
}

#[tokio::test]
async fn update_note_changes_fields_and_clears_optionals() {
    let id = note_id(2);
    let (_ids, _g, conn) = setup!(vec![id.clone()]);
    let mut n = input("t1", "b1");
    n.place_name = Some("p1");
    notes::insert_note(&conn, &id, &n, "yanggf8", NOW).await.unwrap();
    let n2 = input("t2", "b2 😀");
    assert!(notes::update_note(&conn, &id, &n2, "lisyanggf", "2026-11-17 09:00:00").await.unwrap());
    let got = notes::get_note(&conn, &id).await.unwrap().unwrap();
    assert_eq!((got.title.as_str(), got.body.as_str()), ("t2", "b2 😀"));
    assert_eq!(got.place_name, None, "update writes the full input; None clears");
    assert_eq!(got.created_by, "yanggf8");
}

#[tokio::test]
async fn list_filters_plan_day_and_excludes_deleted() {
    let tag = nanos();
    let plan = format!("zz_test_{tag}_np");
    let other = format!("zz_test_{tag}_np2");
    let (a, b, c, d) = (note_id(10), note_id(11), note_id(12), note_id(13));
    let (_ids, _g, conn) = setup!(vec![a.clone(), b.clone(), c.clone(), d.clone()]);
    let mut na = input("a", "x");
    na.attached_plan_id = Some(&plan);
    na.attached_day_number = Some(1);
    let mut nb = input("b", "x");
    nb.attached_plan_id = Some(&plan);
    nb.attached_day_number = Some(2);
    let mut nc = input("c", "x");
    nc.attached_plan_id = Some(&other);
    nc.attached_day_number = Some(1);
    let nd = input("d-unattached", "x");
    for (id, n) in [(&a, &na), (&b, &nb), (&c, &nc), (&d, &nd)] {
        notes::insert_note(&conn, id, n, "t", NOW).await.unwrap();
    }
    let ids = |v: Vec<notes::NoteRow>| {
        let mut v: Vec<String> = v.into_iter().map(|r| r.id).collect();
        v.sort();
        v
    };
    let mut want = vec![a.clone(), b.clone()];
    want.sort();
    assert_eq!(ids(notes::list_notes(&conn, Some(&plan), None).await.unwrap()), want);
    assert_eq!(ids(notes::list_notes(&conn, Some(&plan), Some(2)).await.unwrap()), vec![b.clone()]);
    // No selector = everything, including the unattached note.
    let all = ids(notes::list_notes(&conn, None, None).await.unwrap());
    for id in [&a, &b, &c, &d] {
        assert!(all.contains(id), "unfiltered list must contain {id}");
    }
    // A day without a plan is a caller error, never a silent cross-plan filter.
    assert!(notes::list_notes(&conn, None, Some(1)).await.is_err());
    // Soft-deleted notes drop out of every list.
    assert!(notes::soft_delete_note(&conn, &a, "t", NOW).await.unwrap());
    assert_eq!(ids(notes::list_notes(&conn, Some(&plan), None).await.unwrap()), vec![b.clone()]);
    assert!(!ids(notes::list_notes(&conn, None, None).await.unwrap()).contains(&a));
    // get_note still returns the soft-deleted row (callers decide), with deleted_at/by set.
    let got = notes::get_note(&conn, &a).await.unwrap().unwrap();
    assert!(got.deleted_at.is_some());
}

#[tokio::test]
async fn update_note_missing_or_deleted_returns_false() {
    let id = note_id(20);
    let (_ids, _g, conn) = setup!(vec![id.clone()]);
    let n = input("t", "b");
    assert!(!notes::update_note(&conn, "note_ffffffffffffffff", &n, "t", NOW).await.unwrap());
    assert!(!notes::soft_delete_note(&conn, "note_ffffffffffffffff", "t", NOW).await.unwrap());
    notes::insert_note(&conn, &id, &n, "t", NOW).await.unwrap();
    assert!(notes::soft_delete_note(&conn, &id, "t", NOW).await.unwrap());
    // Already deleted: neither op touches it again.
    assert!(!notes::soft_delete_note(&conn, &id, "t", "2027-01-01 00:00:00").await.unwrap());
    assert!(!notes::update_note(&conn, &id, &input("zz", "zz"), "t", NOW).await.unwrap());
    let got = notes::get_note(&conn, &id).await.unwrap().unwrap();
    assert_eq!(got.title, "t");
    assert_eq!(got.deleted_at.as_deref(), Some(NOW), "second delete must not move deleted_at");
}

#[tokio::test]
async fn image_sort_order_appends() {
    let id = note_id(30);
    let (_ids, _g, conn) = setup!(vec![id.clone()]);
    notes::insert_note(&conn, &id, &input("t", "b"), "t", NOW).await.unwrap();
    for iid in ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb", "cccccccccccccccc"] {
        assert!(notes::insert_image(&conn, &id, &img(iid, &id), "t", NOW).await.unwrap());
    }
    let imgs = notes::list_images(&conn, &id).await.unwrap();
    let order: Vec<(&str, i64)> = imgs.iter().map(|i| (i.image_id.as_str(), i.sort_order)).collect();
    assert_eq!(
        order,
        vec![("aaaaaaaaaaaaaaaa", 1), ("bbbbbbbbbbbbbbbb", 2), ("cccccccccccccccc", 3)]
    );
    // Deleting the middle one and appending again never reuses a taken position.
    assert!(notes::delete_image_row(&conn, &id, "bbbbbbbbbbbbbbbb").await.unwrap());
    assert!(notes::insert_image(&conn, &id, &img("dddddddddddddddd", &id), "t", NOW).await.unwrap());
    let imgs = notes::list_images(&conn, &id).await.unwrap();
    let order: Vec<(&str, i64)> = imgs.iter().map(|i| (i.image_id.as_str(), i.sort_order)).collect();
    assert_eq!(
        order,
        vec![("aaaaaaaaaaaaaaaa", 1), ("cccccccccccccccc", 3), ("dddddddddddddddd", 4)]
    );
    assert_eq!(imgs[0].content_type, "image/jpeg");
    assert_eq!(imgs[0].size_bytes, 1234);
}

#[tokio::test]
async fn get_image_key_reads_db_and_delete_false_when_missing() {
    let id = note_id(40);
    let other = note_id(41);
    let (_ids, _g, conn) = setup!(vec![id.clone(), other.clone()]);
    notes::insert_note(&conn, &id, &input("t", "b"), "t", NOW).await.unwrap();
    notes::insert_note(&conn, &other, &input("t", "b"), "t", NOW).await.unwrap();
    let mut im = img("1111111111111111", &id);
    im.r2_key = format!("{id}/1111111111111111.png"); // key comes from the DB row, not rebuilt
    assert!(notes::insert_image(&conn, &id, &im, "t", NOW).await.unwrap());
    assert_eq!(
        notes::get_image_key(&conn, &id, "1111111111111111").await.unwrap(),
        Some(format!("{id}/1111111111111111.png"))
    );
    // Same image id under another note, or a missing id: no key.
    assert_eq!(notes::get_image_key(&conn, &other, "1111111111111111").await.unwrap(), None);
    assert_eq!(notes::get_image_key(&conn, &id, "9999999999999999").await.unwrap(), None);
    assert!(!notes::delete_image_row(&conn, &id, "9999999999999999").await.unwrap());
    assert!(!notes::delete_image_row(&conn, &other, "1111111111111111").await.unwrap());
    assert!(notes::delete_image_row(&conn, &id, "1111111111111111").await.unwrap());
    assert!(!notes::delete_image_row(&conn, &id, "1111111111111111").await.unwrap());
}

#[tokio::test]
async fn share_tokens_deactivate_list_and_clear_scoped() {
    let (a, b) = (note_id(50), note_id(51));
    let (_ids, _g, conn) = setup!(vec![a.clone(), b.clone()]);
    // Per-run unique tokens (global PK): shared 16-hex base so one prefix hits two of them.
    let base = format!("{:016x}", nanos() as u64);
    let other = format!("{:016x}", (nanos() as u64).wrapping_add(0x9e37_79b9_7f4a_7c15));
    let (t1, t2, t3) = (token(&base, "aaaaaaaaaaaaaaaa"), token(&base, "bbbbbbbbbbbbbbbb"), token(&other, "cccccccccccccccc"));
    for id in [&a, &b] {
        notes::insert_note(&conn, id, &input("t", "b"), "t", NOW).await.unwrap();
    }
    assert!(notes::insert_share_token(&conn, &a, &t1, "yanggf8", NOW).await.unwrap());
    assert!(notes::insert_share_token(&conn, &a, &t2, "yanggf8", NOW).await.unwrap());
    assert!(notes::insert_share_token(&conn, &b, &t3, "yanggf8", NOW).await.unwrap());
    let listed = notes::list_share_tokens(&conn, &a).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|(_, status, at)| status == "active" && at == NOW));

    assert_eq!(notes::deactivate_share_tokens(&conn, &[&base], "lisyanggf", NOW).await.unwrap(), 2);
    assert_eq!(notes::deactivate_share_tokens(&conn, &["zzzzzzzzzzzz"], "x", NOW).await.unwrap(), 0);
    // Already inactive rows are not counted again; wildcard characters are literal.
    assert_eq!(notes::deactivate_share_tokens(&conn, &[&base], "x", NOW).await.unwrap(), 0);
    assert_eq!(notes::deactivate_share_tokens(&conn, &["%"], "x", NOW).await.unwrap(), 0);
    assert!(notes::deactivate_share_tokens(&conn, &[""], "x", NOW).await.is_err(), "empty prefix would match everything");
    let listed = notes::list_share_tokens(&conn, &a).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|(_, status, _)| status == "inactive"));
    assert_eq!(notes::list_share_tokens(&conn, &b).await.unwrap()[0].1, "active", "other note untouched");

    // clear ONE note: hard-deletes active AND inactive of that note only. The whole-table form
    // (`None`) is never executed against the shared DB; its SQL shape is a unit test in notes.rs.
    assert_eq!(notes::clear_share_tokens(&conn, Some(&a)).await.unwrap(), 2);
    assert!(notes::list_share_tokens(&conn, &a).await.unwrap().is_empty());
    assert_eq!(notes::list_share_tokens(&conn, &b).await.unwrap().len(), 1);
    assert_eq!(notes::clear_share_tokens(&conn, Some(&b)).await.unwrap(), 1);
}

#[tokio::test]
async fn insert_image_and_token_refuse_missing_or_deleted_note_and_delete_image_keeps_deleted() {
    let (live, gone) = (note_id(55), note_id(56));
    let missing = "note_ffffffffffffffff";
    let (_ids, _g, conn) = setup!(vec![live.clone(), gone.clone(), missing.to_string()]);
    for id in [&live, &gone] {
        notes::insert_note(&conn, id, &input("t", "b"), "t", NOW).await.unwrap();
    }
    let ntok = |n: u64| format!("{:032x}", (nanos() as u64 as u128) << 32 | n as u128);
    // Live note accepts both.
    assert!(notes::insert_image(&conn, &gone, &img("1111111111111111", &gone), "t", NOW).await.unwrap());
    assert!(notes::insert_share_token(&conn, &live, &ntok(1), "t", NOW).await.unwrap());
    notes::soft_delete_note(&conn, &gone, "t", NOW).await.unwrap();
    // Missing and soft-deleted notes: nothing written, false.
    for id in [missing, gone.as_str()] {
        assert!(!notes::insert_image(&conn, id, &img("2222222222222222", id), "t", NOW).await.unwrap(), "image into {id}");
        assert!(!notes::insert_share_token(&conn, id, &ntok(2), "t", NOW).await.unwrap(), "token for {id}");
        assert!(notes::list_images(&conn, id).await.unwrap().iter().all(|i| i.image_id != "2222222222222222"));
        assert!(notes::list_share_tokens(&conn, id).await.unwrap().is_empty());
    }
    // Soft-deleted note keeps its photo (30-day retention): delete_image_row refuses.
    assert!(!notes::delete_image_row(&conn, &gone, "1111111111111111").await.unwrap());
    assert_eq!(notes::list_images(&conn, &gone).await.unwrap().len(), 1);
    assert!(notes::get_image_key(&conn, &gone, "1111111111111111").await.unwrap().is_some());
}

#[tokio::test]
async fn deleted_before_cutoff_and_purge() {
    let (old, fresh, live) = (note_id(60), note_id(61), note_id(62));
    let (_ids, _g, conn) = setup!(vec![old.clone(), fresh.clone(), live.clone()]);
    for id in [&old, &fresh, &live] {
        notes::insert_note(&conn, id, &input("t", "b"), "t", NOW).await.unwrap();
    }
    assert!(notes::insert_image(&conn, &old, &img("aaaaaaaaaaaaaaaa", &old), "t", NOW).await.unwrap());
    assert!(notes::insert_image(&conn, &old, &img("bbbbbbbbbbbbbbbb", &old), "t", NOW).await.unwrap());
    assert!(notes::insert_share_token(&conn, &old, &format!("{:032x}", nanos()), "t", NOW).await.unwrap());
    notes::soft_delete_note(&conn, &old, "t", "2026-01-01 00:00:00").await.unwrap();
    notes::soft_delete_note(&conn, &fresh, "t", "2026-11-10 00:00:00").await.unwrap();

    let got = notes::deleted_before(&conn, "2026-06-01 00:00:00").await.unwrap();
    let mine: Vec<&(String, Vec<String>)> = got.iter().filter(|(id, _)| id == &old || id == &fresh || id == &live).collect();
    assert_eq!(mine.len(), 1, "only the note deleted before the cutoff: {got:?}");
    assert_eq!(mine[0].0, old);
    let mut keys = mine[0].1.clone();
    keys.sort();
    assert_eq!(keys, vec![format!("{old}/aaaaaaaaaaaaaaaa.jpg"), format!("{old}/bbbbbbbbbbbbbbbb.jpg")]);

    // purge hard-deletes notes + images + tokens, but keeps note_events history.
    notes::insert_event(&conn, &old, "purge", "cli", "purged", NOW).await.unwrap();
    notes::purge_note(&conn, &old).await.unwrap();
    assert!(notes::get_note(&conn, &old).await.unwrap().is_none());
    assert!(notes::list_images(&conn, &old).await.unwrap().is_empty());
    assert!(notes::list_share_tokens(&conn, &old).await.unwrap().is_empty());
    let n = db_exec(&format!("SELECT count(*) AS n FROM note_events WHERE note_id='{old}' AND action='purge'"))
        .and_then(|r| r.scalar());
    assert_eq!(n.as_deref(), Some("1"), "note_events must survive purge");
    // The fresh and live notes are untouched.
    assert!(notes::get_note(&conn, &fresh).await.unwrap().is_some());
    assert!(notes::get_note(&conn, &live).await.unwrap().is_some());
}

#[tokio::test]
async fn insert_event_writes_row_with_unique_ids() {
    let id = note_id(70);
    let (_ids, _g, conn) = setup!(vec![id.clone()]);
    notes::insert_event(&conn, &id, "create", "yanggf8", "title='x' 😀", NOW).await.unwrap();
    notes::insert_event(&conn, &id, "update", "yanggf8", "", NOW).await.unwrap();
    let mut rows = conn
        .query(
            "SELECT action, actor, detail, at, event_id FROM note_events WHERE note_id = ?1 ORDER BY action",
            libsql::params![id.clone()],
        )
        .await
        .unwrap();
    let mut got = Vec::new();
    while let Some(r) = rows.next().await.unwrap() {
        got.push((
            r.get::<String>(0).unwrap(),
            r.get::<String>(1).unwrap(),
            r.get::<String>(2).unwrap(),
            r.get::<String>(3).unwrap(),
            r.get::<String>(4).unwrap(),
        ));
    }
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].0, "create");
    assert_eq!(got[0].2, "title='x' 😀");
    assert_eq!(got[0].3, NOW);
    assert_ne!(got[0].4, got[1].4, "event ids must differ");
}

#[test]
fn migrate_idempotent() {
    let _g = Guard::new(|| {});
    if !migrate() {
        return;
    }
    // Idempotency: a SECOND explicit run on top of the once-per-binary one must also succeed.
    let out = Command::new(bin()).args(["db", "migrate"]).output().expect("run db migrate");
    assert!(out.status.success(), "second migrate failed: {}", String::from_utf8_lossy(&out.stderr));
    // The four tables exist, and the contract columns are there.
    for (table, col) in [
        ("notes", "attached_plan_id"),
        ("note_images", "r2_key"),
        ("note_share_tokens", "deactivated_by"),
        ("note_events", "detail"),
    ] {
        let n = db_exec(&format!(
            "SELECT count(*) AS n FROM pragma_table_info('{table}') WHERE name='{col}'"
        ))
        .and_then(|r| r.scalar());
        assert_eq!(n.as_deref(), Some("1"), "{table}.{col} missing");
    }
    // No plan_id column anywhere in the notes family (copy-plan / cleanup-deleted /
    // teardown_plan scan by that name), and no constraints.
    for table in ["notes", "note_images", "note_share_tokens", "note_events"] {
        let n = db_exec(&format!("SELECT count(*) AS n FROM pragma_table_info('{table}') WHERE name='plan_id'"))
            .and_then(|r| r.scalar());
        assert_eq!(n.as_deref(), Some("0"), "{table} must not have a plan_id column");
        let ddl = db_exec(&format!("SELECT sql AS s FROM sqlite_master WHERE name='{table}'"))
            .and_then(|r| r.scalar())
            .unwrap_or_default();
        assert!(!ddl.to_uppercase().contains("CHECK"), "{table} must carry no CHECK: {ddl}");
    }
    // Migration is schema-only: it never inserts note content.
}

#[tokio::test]
async fn teardown_plan_does_not_touch_notes() {
    let tag = nanos();
    let plan = format!("zz_test_{tag}_nt");
    let dest = format!("zz_test_{tag}_nd");
    let id = note_id(80);
    // Notes first in the guard order: its Drop runs LAST (declared first), after teardown_plan.
    let (_ids, _g, conn) = setup!(vec![id.clone()]);
    let _gp = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    let mut n = input("attached", "b");
    n.attached_plan_id = Some(&plan);
    n.attached_destination = Some(&dest);
    n.attached_day_number = Some(1);
    notes::insert_note(&conn, &id, &n, "t", NOW).await.unwrap();
    teardown_plan(&plan, &dest);
    let got = notes::get_note(&conn, &id).await.unwrap().expect("note must survive teardown_plan");
    assert_eq!(got.attached_plan_id.as_deref(), Some(plan.as_str()));
}
