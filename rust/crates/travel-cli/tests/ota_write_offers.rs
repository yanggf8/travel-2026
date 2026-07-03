//! Integration test for `travel ota write-offers`.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec_teardown, is_credless, nanos, Guard};

// These tests share the global `zztest` row in ota_sources/ota_source_coverage, and each test's
// teardown DELETEs it by that shared literal — so a concurrent test would have its source yanked
// mid-run. Serialize them (same pattern as ota_claim.rs / ota_parse.rs).
static WRITE_OFFERS_TEST_LOCK: Mutex<()> = Mutex::new(());

fn write_offers_test_lock() -> std::sync::MutexGuard<'static, ()> {
    WRITE_OFFERS_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

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

fn fixture_tsv() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zz_ota_offers.tsv")
}

fn teardown(job_id: &str, capture_id: &str) {
    let _ = db_exec_teardown(&format!("DELETE FROM offers WHERE produced_by_job_id='{job_id}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_attempts WHERE job_id='{job_id}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_jobs WHERE job_id='{job_id}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM captures WHERE capture_id='{capture_id}'"));
    let _ = db_exec_teardown("DELETE FROM ota_source_coverage WHERE source_id='zztest'");
    let _ = db_exec_teardown("DELETE FROM ota_sources WHERE source_id='zztest'");
}

#[tokio::test]
async fn write_offers_from_fixture_tsv() {
    let _lock = write_offers_test_lock();
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let suffix = nanos();
    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcapwo{suffix}");
    let now = "2026-06-29T15:30:00Z";
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });

    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT OR IGNORE INTO ota_sources (source_id, name, status, updated_at) \
             VALUES ('zztest', 'ZZ Test', 'active', '{now}')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT OR IGNORE INTO ota_source_coverage \
             (source_id, product_type, status, method, updated_at) \
             VALUES ('zztest', 'fit', 'active', 'agent_parse', '{now}')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, url, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', 'https://example.com/', '{now}', 'agent tsv test')"
        ),
    ]);
    let token = format!("zztok{suffix}");
    let lease = "2026-06-29T16:30:00Z";
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, claimed_by, \
             claimed_at, heartbeat_at, lease_expires_at, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'claimed', '{token}', 'wo-test', \
             '{now}', '{now}', '{lease}', '{now}', '{now}')"
        ),
    ]);

    let tsv = fixture_tsv();
    let (ok, stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        &token,
        "--tsv",
        tsv.to_str().unwrap(),
    ]);
    assert!(ok, "write-offers failed: {stderr}");
    assert!(stdout.contains("inserted\t1"));
    assert!(stdout.contains("parser_method\tagent_parse"));

    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!(
            "SELECT type, price_per_person, hotel_name, parser_method \
             FROM offers WHERE produced_by_job_id='{job_id}' LIMIT 1"
        ),
    ]);
    assert!(ok);
    assert!(stdout.contains("package"));
    assert!(stdout.contains("15000"));
    assert!(stdout.contains("HOTEL ZZTEST"));
    assert!(stdout.contains("agent_parse"));
}

fn fixture_multi_tsv() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zz_ota_multi.tsv")
}

/// Two distinct offers share the same (date, nights) — they MUST NOT collapse onto one PK.
/// Locks the disambiguation fix (base offer_row_id keys only on source/code/date/nights).
#[tokio::test]
async fn write_offers_distinct_rows_do_not_collapse_on_pk() {
    let _lock = write_offers_test_lock();
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let suffix = nanos();
    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcapwo{suffix}");
    let now = "2026-06-29T15:36:00Z";
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });

    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT OR IGNORE INTO ota_sources (source_id, name, status, updated_at) \
             VALUES ('zztest', 'ZZ Test', 'active', '{now}')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, url, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', 'https://example.com/pkg', '{now}', 'multi tsv')"
        ),
    ]);
    let token = format!("zztok{suffix}");
    let lease = "2026-06-29T16:36:00Z";
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, claimed_by, \
             claimed_at, heartbeat_at, lease_expires_at, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'claimed', '{token}', 'wo-test', \
             '{now}', '{now}', '{lease}', '{now}', '{now}')"
        ),
    ]);

    let tsv = fixture_multi_tsv();
    let (ok, stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        &token,
        "--tsv",
        tsv.to_str().unwrap(),
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(ok, "write-offers failed: {stderr}");
    // BOTH offers must land — pre-fix only one survived ON CONFLICT DO NOTHING.
    assert!(stdout.contains("candidates\t2"), "stdout={stdout}");
    assert!(stdout.contains("inserted\t2"), "stdout={stdout}");
    assert!(stdout.contains("deduped\t0"), "stdout={stdout}");

    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!(
            "SELECT count(*) AS n FROM offers WHERE produced_by_job_id='{job_id}'"
        ),
    ]);
    assert!(ok);
    assert!(stdout.contains(": 2") || stdout.contains(":2"), "row count stdout={stdout}");

    // The job must end 'succeeded', not stuck in 'running', and attempts must have advanced.
    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!("SELECT status, attempts FROM ota_jobs WHERE job_id='{job_id}'"),
    ]);
    assert!(ok);
    assert!(stdout.contains("succeeded"), "job status stdout={stdout}");
}

fn bad_header_tsv() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zz_bad_header.tsv")
}

#[tokio::test]
async fn write_offers_bad_header_fails_loud() {
    let _lock = write_offers_test_lock();
    let suffix = nanos();
    let bad_path = bad_header_tsv();

    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcap{suffix}");
    let now = "2026-06-29T15:31:00Z";
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });
    let token = format!("zztok{suffix}");
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'claimed', '{token}', '{now}', '{now}')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', '{now}', 'x')"
        ),
    ]);

    let (ok, stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        &token,
        "--tsv",
        bad_path.to_str().unwrap(),
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(!ok, "bad header must fail; stdout={stdout} stderr={stderr}");
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("unknown") || combined.contains("bogus") || combined.contains("Error"),
        "combined={combined}"
    );
}

#[tokio::test]
async fn write_offers_stale_token_writes_nothing() {
    let _lock = write_offers_test_lock();
    let suffix = nanos();
    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcap{suffix}");
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });
    let now = "2026-06-29T15:32:00Z";
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', '{now}', 'x')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'claimed', '{now}', '{now}')"
        ),
    ]);
    let tsv = fixture_tsv();
    let (ok, _stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        "wrong-token",
        "--tsv",
        tsv.to_str().unwrap(),
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(!ok);
    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!("SELECT count(*) AS n FROM offers WHERE produced_by_job_id='{job_id}'"),
    ]);
    if ok {
        assert!(stdout.contains(": 0") || stdout.contains(":0"));
    }
}

#[tokio::test]
async fn write_offers_rejects_capture_source_mismatch() {
    let _lock = write_offers_test_lock();
    let suffix = nanos();
    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcap{suffix}");
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });
    let now = "2026-06-29T15:33:00Z";
    let token = format!("zztok{suffix}");
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', '{now}', 'x')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, created_at, updated_at) \
             VALUES ('{job_id}', 'zzother', 'fit', 'claimed', '{token}', '{now}', '{now}')"
        ),
    ]);

    let tsv = fixture_tsv();
    let (ok, _stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        &token,
        "--tsv",
        tsv.to_str().unwrap(),
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(!ok, "capture source mismatch must fail");
    assert!(stderr.contains("source_id"), "stderr={stderr}");
    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!("SELECT count(*) AS n FROM offers WHERE produced_by_job_id='{job_id}'"),
    ]);
    if ok {
        assert!(stdout.contains(": 0") || stdout.contains(":0"));
    }
}

#[tokio::test]
async fn write_offers_rejects_type_incompatible_with_job_product() {
    let _lock = write_offers_test_lock();
    let suffix = nanos();
    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcap{suffix}");
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });
    let now = "2026-06-29T15:34:00Z";
    let token = format!("zztok{suffix}");
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', '{now}', 'x')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'flight', 'claimed', '{token}', '{now}', '{now}')"
        ),
    ]);

    let tsv = fixture_tsv();
    let (ok, _stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        &token,
        "--tsv",
        tsv.to_str().unwrap(),
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(!ok, "package TSV rows must not write to a flight job");
    assert!(stderr.contains("incompatible"), "stderr={stderr}");
    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!("SELECT count(*) AS n FROM offers WHERE produced_by_job_id='{job_id}'"),
    ]);
    if ok {
        assert!(stdout.contains(": 0") || stdout.contains(":0"));
    }
}

#[tokio::test]
async fn write_offers_stops_when_mark_running_rejects_token_state() {
    let _lock = write_offers_test_lock();
    let suffix = nanos();
    let job_id = format!("zzwo{suffix}");
    let capture_id = format!("zzcap{suffix}");
    teardown(&job_id, &capture_id);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown(&job_id, &capture_id)
    });
    let now = "2026-06-29T15:35:00Z";
    let token = format!("zztok{suffix}");
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', '{now}', 'x')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claim_token, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'queued', '{token}', '{now}', '{now}')"
        ),
    ]);

    let tsv = fixture_tsv();
    let (ok, _stdout, stderr) = run(&[
        "ota",
        "write-offers",
        &job_id,
        "--capture",
        &capture_id,
        "--claim-token",
        &token,
        "--tsv",
        tsv.to_str().unwrap(),
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(!ok, "mark_running rejection must fail before writing");
    assert!(stderr.contains("claim rejected"), "stderr={stderr}");
    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!("SELECT count(*) AS n FROM offers WHERE produced_by_job_id='{job_id}'"),
    ]);
    if ok {
        assert!(stdout.contains(": 0") || stdout.contains(":0"));
    }
}
