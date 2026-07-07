//! Integration tests for the audited OTA-catalog mutation commands
//! (DB-centric provider architecture, spec 2026-06-29). Real-Turso; skips if creds absent.
//! Panic-safe teardown via the shared Guard.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, Guard};

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
    let _ = db_exec_teardown(&format!("DELETE FROM ota_source_coverage WHERE source_id='{sid}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_source_region_codes WHERE source_id='{sid}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_sources WHERE source_id='{sid}'"));
    // catalog_runs is append-only audit; clean only this test's noise by command_summary match.
    let _ = db_exec_teardown(&format!("DELETE FROM catalog_runs WHERE command_summary LIKE '{sid}%'"));
}

fn teardown_tier1(sid: &str) {
    let _ = db_exec_teardown(&format!("DELETE FROM ota_source_url_param WHERE source_id='{sid}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_source_workflow WHERE source_id='{sid}'"));
    teardown(sid);
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
    let Some(n) = db_exec(&format!(
        "SELECT count(*) AS n FROM ota_source_coverage WHERE source_id='{sid}'"
    ))
    .and_then(|r| r.scalar()) else {
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
        db_exec(&format!("SELECT proven FROM ota_source_coverage WHERE source_id='{sid}' AND product_type='fit'"))
            .and_then(|r| r.scalar())
            .as_deref(),
        Some("1"),
        "proven landed"
    );
    assert_eq!(
        db_exec(&format!("SELECT method FROM ota_source_coverage WHERE source_id='{sid}' AND product_type='fit'"))
            .and_then(|r| r.scalar())
            .as_deref(),
        Some("agent_parse"),
        "method landed"
    );
    // catalog_runs audit row written.
    let Some(audit) = db_exec(&format!(
        "SELECT count(*) AS n FROM catalog_runs WHERE command_summary LIKE '{sid}/fit%'"
    ))
    .and_then(|r| r.scalar()) else {
        return;
    };
    assert!(audit.parse::<i64>().unwrap_or(0) >= 1, "a catalog_runs audit row was written");
}

#[tokio::test]
async fn set_ota_workflow_round_trip_writes_row_and_audit() {
    let _guard = CATALOG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let sid = format!("zztest{}", nanos());
    teardown_tier1(&sid);
    let _g = Guard::new({
        let sid = sid.clone();
        move || teardown_tier1(&sid)
    });

    let (ok, _o, err) = run(&["set-ota-source", &sid, "--name", "ZZ Test", "--status", "active"]);
    assert!(ok, "set-ota-source should succeed; err={err}");

    let template = "https://example.com/search?dest={dest_code}&depart={depart}&return={return}";
    let (ok, stdout, stderr) = run(&[
        "set-ota-workflow",
        &sid,
        "fit",
        "--nav",
        "get",
        "--url-template",
        template,
        "--capture-url-contains",
        "example.com/search",
        "--settle-ms",
        "25000",
        "--settle-marker",
        "ready",
        "--note",
        "zz workflow note",
    ]);
    assert!(
        ok,
        "set-ota-workflow should succeed; stdout={stdout} stderr={stderr}"
    );

    assert_eq!(
        db_exec(&format!(
            "SELECT nav_kind FROM ota_source_workflow WHERE source_id='{sid}' AND product_type='fit'"
        ))
        .and_then(|r| r.scalar())
        .as_deref(),
        Some("get"),
        "nav_kind landed"
    );
    assert_eq!(
        db_exec(&format!(
            "SELECT url_template FROM ota_source_workflow WHERE source_id='{sid}' AND product_type='fit'"
        ))
        .and_then(|r| r.scalar())
        .as_deref(),
        Some(template),
        "url_template landed"
    );
    assert_eq!(
        db_exec(&format!(
            "SELECT capture_url_contains FROM ota_source_workflow WHERE source_id='{sid}' AND product_type='fit'"
        ))
        .and_then(|r| r.scalar())
        .as_deref(),
        Some("example.com/search"),
        "capture_url_contains landed"
    );
    assert_eq!(
        db_exec(&format!(
            "SELECT settle_marker FROM ota_source_workflow WHERE source_id='{sid}' AND product_type='fit'"
        ))
        .and_then(|r| r.scalar())
        .as_deref(),
        Some("ready"),
        "settle_marker landed"
    );
    assert_eq!(
        db_exec(&format!(
            "SELECT settle_ms FROM ota_source_workflow WHERE source_id='{sid}' AND product_type='fit'"
        ))
        .and_then(|r| r.scalar())
        .as_deref(),
        Some("25000"),
        "settle_ms landed"
    );
    assert_eq!(
        db_exec(&format!(
            "SELECT agent_extraction_note FROM ota_source_workflow WHERE source_id='{sid}' AND product_type='fit'"
        ))
        .and_then(|r| r.scalar())
        .as_deref(),
        Some("zz workflow note"),
        "agent_extraction_note landed"
    );

    let Some(audit) = db_exec(&format!(
        "SELECT count(*) AS n FROM catalog_runs \
         WHERE command_type='set-ota-workflow' AND command_summary LIKE '{sid}/fit%'"
    ))
    .and_then(|r| r.scalar()) else {
        return;
    };
    assert!(
        audit.parse::<i64>().unwrap_or(0) >= 1,
        "set-ota-workflow wrote a catalog_runs audit row"
    );
}

#[tokio::test]
async fn set_ota_url_param_round_trip_writes_row_and_audit() {
    let _guard = CATALOG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let sid = format!("zztest{}", nanos());
    teardown_tier1(&sid);
    let _g = Guard::new({
        let sid = sid.clone();
        move || teardown_tier1(&sid)
    });

    let (ok, _o, err) = run(&["set-ota-source", &sid, "--name", "ZZ Test", "--status", "active"]);
    assert!(ok, "set-ota-source should succeed; err={err}");

    let (ok, stdout, stderr) = run(&[
        "set-ota-url-param",
        &sid,
        "fit",
        "dest_code",
        "destination",
        "tokyo",
        "TYO",
    ]);
    assert!(
        ok,
        "set-ota-url-param should succeed; stdout={stdout} stderr={stderr}"
    );

    assert_eq!(
        db_exec(&format!(
            "SELECT url_value FROM ota_source_url_param \
             WHERE source_id='{sid}' AND product_type='fit' AND url_param_name='dest_code' \
               AND input_name='destination' AND input_value='tokyo'"
        ))
        .and_then(|r| r.scalar())
        .as_deref(),
        Some("TYO"),
        "token row landed"
    );

    let Some(audit) = db_exec(&format!(
        "SELECT count(*) AS n FROM catalog_runs \
         WHERE command_type='set-ota-url-param' AND command_summary LIKE '{sid}/fit%'"
    ))
    .and_then(|r| r.scalar()) else {
        return;
    };
    assert!(
        audit.parse::<i64>().unwrap_or(0) >= 1,
        "set-ota-url-param wrote a catalog_runs audit row"
    );
}

#[tokio::test]
async fn set_ota_url_param_hotel_round_trip_writes_row_and_audit() {
    let _guard = CATALOG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let sid = format!("zztest{}", nanos());
    teardown_tier1(&sid);
    let _g = Guard::new({
        let sid = sid.clone();
        move || teardown_tier1(&sid)
    });

    let (ok, _o, err) = run(&["set-ota-source", &sid, "--name", "ZZ Test", "--status", "active"]);
    assert!(ok, "set-ota-source should succeed; err={err}");

    let (ok, stdout, stderr) = run(&[
        "set-ota-url-param",
        &sid,
        "hotel",
        "hotel_slug",
        "hotel",
        "my-hotel",
        "tok",
    ]);
    assert!(
        ok,
        "set-ota-url-param hotel should succeed; stdout={stdout} stderr={stderr}"
    );

    assert_eq!(
        db_exec(&format!(
            "SELECT url_value FROM ota_source_url_param \
             WHERE source_id='{sid}' AND product_type='hotel' AND url_param_name='hotel_slug' \
               AND input_name='hotel' AND input_value='my-hotel'"
        ))
        .and_then(|r| r.scalar())
        .as_deref(),
        Some("tok"),
        "hotel token row landed"
    );

    let Some(audit) = db_exec(&format!(
        "SELECT count(*) AS n FROM catalog_runs \
         WHERE command_type='set-ota-url-param' AND command_summary LIKE '{sid}/hotel%'"
    ))
    .and_then(|r| r.scalar()) else {
        return;
    };
    assert!(
        audit.parse::<i64>().unwrap_or(0) >= 1,
        "set-ota-url-param hotel wrote a catalog_runs audit row"
    );
}

#[tokio::test]
async fn set_ota_url_param_rejects_origin_input_name_and_writes_nothing() {
    let _guard = CATALOG_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let sid = format!("zztest{}", nanos());
    teardown_tier1(&sid);
    let _g = Guard::new({
        let sid = sid.clone();
        move || teardown_tier1(&sid)
    });

    let (ok, _o, err) = run(&["set-ota-source", &sid, "--name", "ZZ Test", "--status", "active"]);
    assert!(ok, "set-ota-source should succeed; err={err}");

    let (ok, stdout, stderr) = run(&[
        "set-ota-url-param",
        &sid,
        "fit",
        "dest_code",
        "origin",
        "tokyo",
        "TYO",
    ]);
    assert!(
        !ok,
        "non-destination input_name must fail; stdout={stdout} stderr={stderr}"
    );

    let Some(rows) = db_exec(&format!(
        "SELECT count(*) AS n FROM ota_source_url_param WHERE source_id='{sid}'"
    ))
    .and_then(|r| r.scalar()) else {
        return;
    };
    assert_eq!(rows, "0", "failed set-ota-url-param must write no url_param row");

    let Some(audit) = db_exec(&format!(
        "SELECT count(*) AS n FROM catalog_runs WHERE command_type='set-ota-url-param' AND command_summary LIKE '{sid}%'"
    ))
    .and_then(|r| r.scalar()) else {
        return;
    };
    assert_eq!(audit, "0", "failed set-ota-url-param must write no audit row");
}

// ── Unknown-flag rejection (parse-before-connect → hermetic, no DB needed) ──
// Each subcommand rejects an unknown --flag with its OWN flag list. A flag valid
// for one subcommand (e.g. --proven on coverage) is unknown for another (source).

#[test]
fn set_ota_source_rejects_unknown_flag() {
    let (ok, _o, err) = run(&["set-ota-source", "zzsrc", "--name", "ZZ", "--stat", "active"]);
    assert!(!ok, "should reject --stat; err={err}");
    assert!(err.contains("unknown argument: --stat"), "err={err}");
}

#[test]
fn set_ota_coverage_rejects_typoed_proven() {
    // The provenance bug: --provven → silently proven=0. Now fails loud.
    let (ok, _o, err) = run(&["set-ota-coverage", "zzsrc", "fit", "--provven"]);
    assert!(!ok, "should reject --provven; err={err}");
    assert!(err.contains("unknown argument: --provven"), "err={err}");
}

#[test]
fn set_ota_region_rejects_any_flag() {
    // Positionals only — any --flag is misuse.
    let (ok, _o, err) = run(&["set-ota-region", "zzsrc", "fit", "Tokyo", "TYO", "--dry-run"]);
    assert!(!ok, "should reject --dry-run; err={err}");
    assert!(err.contains("unknown argument: --dry-run"), "err={err}");
}

#[test]
fn set_ota_workflow_rejects_unknown_flag() {
    let (ok, _o, err) = run(&["set-ota-workflow", "zzsrc", "fit", "--nav", "get", "--url-templat", "x"]);
    assert!(!ok, "should reject --url-templat; err={err}");
    assert!(err.contains("unknown argument: --url-templat"), "err={err}");
}

#[test]
fn set_ota_url_param_rejects_any_flag() {
    let (ok, _o, err) = run(&["set-ota-url-param", "zzsrc", "fit", "dest", "destination", "tokyo", "TYO", "--dry-run"]);
    assert!(!ok, "should reject --dry-run; err={err}");
    assert!(err.contains("unknown argument: --dry-run"), "err={err}");
}
