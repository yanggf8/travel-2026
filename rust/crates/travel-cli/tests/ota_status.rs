//! Integration tests for the DB-native ota-status coverage view.
//! Real-Turso; skips if creds absent. Panic-safe teardown via Guard.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec_teardown, is_credless, nanos, Guard};

static CATALOG_LOCK: Mutex<()> = Mutex::new(());

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn teardown(sid: &str) {
    let _ = db_exec_teardown(&format!("DELETE FROM ota_source_region_codes WHERE source_id='{sid}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_source_coverage WHERE source_id='{sid}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_sources WHERE source_id='{sid}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM catalog_runs WHERE command_summary LIKE '{sid}%'"));
}

#[tokio::test]
async fn ota_status_shows_and_filters_coverage_rows() {
    let _guard = CATALOG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }
    assert!(ok, "db migrate should succeed; err={err}");

    let sid = format!("zzview{}", nanos());
    teardown(&sid);
    let _g = Guard::new({
        let sid = sid.clone();
        move || teardown(&sid)
    });

    let (ok, _out, err) = run(&[
        "set-ota-source",
        &sid,
        "--name",
        "ZZ View",
        "--status",
        "active",
    ]);
    assert!(ok, "set-ota-source should succeed; err={err}");
    let (ok, _out, err) = run(&[
        "set-ota-coverage",
        &sid,
        "group_tour",
        "--proven",
        "--proven-at",
        "2026-06-29",
        "--method",
        "agent_parse",
    ]);
    assert!(ok, "set-ota-coverage should succeed; err={err}");

    let (ok, out, err) = run(&["ota-status"]);
    assert!(ok, "ota-status should succeed; err={err}");
    assert!(
        out.contains(&sid),
        "ota-status should list {sid}; got:\n{out}"
    );
    assert!(
        out.contains("group_tour"),
        "ota-status should show the product type; got:\n{out}"
    );
    assert!(
        out.contains("agent_parse"),
        "ota-status should show the method; got:\n{out}"
    );

    let (ok, out_gt, err) = run(&["ota-status", "--type", "group_tour"]);
    assert!(ok, "ota-status --type group_tour should succeed; err={err}");
    assert!(
        out_gt.contains(&sid),
        "--type group_tour should include {sid}"
    );

    let (ok, out_fl, err) = run(&["ota-status", "--type", "flight"]);
    assert!(ok, "ota-status --type flight should succeed; err={err}");
    assert!(
        !out_fl.contains(&sid),
        "--type flight must not include the group_tour-only {sid}"
    );
}