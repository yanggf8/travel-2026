//! Integration tests for the audited OTA-catalog mutation commands
//! (DB-centric provider architecture, spec 2026-06-29). Real-Turso; skips if creds absent.
//! Panic-safe teardown via the shared Guard.

use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

static CATALOG_LOCK: Mutex<()> = Mutex::new(());

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
}

fn is_credless(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
}

fn run(args: &[&str]) -> (bool, String, String) {
    let out = bin().args(args).output().unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn scalar(sql: &str) -> Option<String> {
    let (ok, stdout, stderr) = run(&["db", "exec", sql]);
    if !ok {
        if is_credless(&stderr) {
            return None;
        }
        panic!("db exec failed: {}\nSQL: {sql}", stderr.trim());
    }
    stdout.lines().find_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
}

fn teardown(sid: &str) {
    let _ = run(&["db", "exec", &format!("DELETE FROM ota_source_coverage WHERE source_id='{sid}'")]);
    let _ = run(&["db", "exec", &format!("DELETE FROM ota_source_region_codes WHERE source_id='{sid}'")]);
    let _ = run(&["db", "exec", &format!("DELETE FROM ota_sources WHERE source_id='{sid}'")]);
    // catalog_runs is append-only audit; clean only this test's noise by command_summary match.
    let _ = run(&["db", "exec", &format!("DELETE FROM catalog_runs WHERE command_summary LIKE '{sid}%'")]);
}

#[tokio::test]
async fn set_coverage_proven_requires_date_and_method() {
    let _guard = CATALOG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }
    let sid = format!("zztest{}", nanos());
    teardown(&sid);
    let _g = Guard::new({
        let sid = sid.clone();
        move || teardown(&sid)
    });

    run(&["set-ota-source", &sid, "--name", "ZZ Test", "--status", "active"]);

    // --proven WITHOUT --proven-at/--method must FAIL and write nothing.
    let (ok, _o, _e) = run(&["set-ota-coverage", &sid, "fit", "--proven"]);
    assert!(!ok, "--proven without --proven-at/--method must fail");
    let Some(n) = scalar(&format!(
        "SELECT count(*) AS n FROM ota_source_coverage WHERE source_id='{sid}'"
    )) else {
        return;
    };
    assert_eq!(n, "0", "failed --proven must write nothing");

    // A bad product_type must fail loud.
    let (ok2, _o, _e) = run(&[
        "set-ota-coverage", &sid, "bogus_type", "--proven", "--proven-at", "2026-06-29",
        "--method", "agent_parse",
    ]);
    assert!(!ok2, "unknown product_type must be rejected");

    // Full valid coverage write succeeds and lands the fields.
    let (ok3, _o, e3) = run(&[
        "set-ota-coverage", &sid, "fit", "--proven", "--proven-at", "2026-06-29",
        "--method", "agent_parse", "--search-url", "http://x/search",
    ]);
    assert!(ok3, "valid coverage write should succeed; err={e3}");
    assert_eq!(
        scalar(&format!("SELECT proven FROM ota_source_coverage WHERE source_id='{sid}' AND product_type='fit'")).as_deref(),
        Some("1"),
        "proven landed"
    );
    assert_eq!(
        scalar(&format!("SELECT method FROM ota_source_coverage WHERE source_id='{sid}' AND product_type='fit'")).as_deref(),
        Some("agent_parse"),
        "method landed"
    );
    // catalog_runs audit row written.
    let Some(audit) = scalar(&format!(
        "SELECT count(*) AS n FROM catalog_runs WHERE command_summary LIKE '{sid}/fit%'"
    )) else {
        return;
    };
    assert!(audit.parse::<i64>().unwrap_or(0) >= 1, "a catalog_runs audit row was written");
}
