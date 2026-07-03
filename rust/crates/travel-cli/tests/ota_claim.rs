//! Integration test for `travel ota claim/heartbeat/finish/reap-stale`.

use std::process::Command;

mod common;
use common::{bin, db_exec_teardown, is_credless, nanos, Guard};
use std::sync::Mutex;

/// OTA claim tests mutate shared global queue state — serialize them.
static CLAIM_TEST_LOCK: Mutex<()> = Mutex::new(());

fn claim_test_lock() -> std::sync::MutexGuard<'static, ()> {
    CLAIM_TEST_LOCK
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

fn field(stdout: &str, key: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}\t")).map(|v| v.to_string()))
}

fn teardown_job(job_id: &str) {
    let _ = db_exec_teardown(&format!("DELETE FROM ota_attempts WHERE job_id='{job_id}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_job_params WHERE job_id='{job_id}'"));
    let _ = db_exec_teardown(&format!("DELETE FROM ota_jobs WHERE job_id='{job_id}'"));
}

fn seed_queued_job(job_id: &str) {
    let now = "2000-01-01T00:00:00Z";
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'queued', '{now}', '{now}')"
        ),
    ]);
}

fn insert_claimed_job(job_id: &str, token: &str, worker: &str, lease: &str) {
    let now = "2026-06-29T15:10:00Z";
    let (ok, _, stderr) = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claimed_by, claim_token, \
             claimed_at, heartbeat_at, lease_expires_at, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'claimed', '{worker}', '{token}', \
             '{now}', '{now}', '{lease}', '{now}', '{now}')"
        ),
    ]);
    assert!(ok, "insert_claimed_job failed: {stderr}");
}

fn insert_claimed_job_with_attempts(job_id: &str, lease: &str, attempts: i64, max_attempts: i64) {
    let now = "2026-06-29T15:10:00Z";
    let token = format!("zztok{}", nanos());
    let (ok, _, stderr) = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, claimed_by, claim_token, \
             claimed_at, heartbeat_at, lease_expires_at, attempts, max_attempts, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'claimed', 'w-att', '{token}', \
             '{now}', '{now}', '{lease}', {attempts}, {max_attempts}, '{now}', '{now}')"
        ),
    ]);
    assert!(ok, "insert_claimed_job_with_attempts failed: {stderr}");
}

fn job_status(job_id: &str) -> Option<String> {
    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!("SELECT status FROM ota_jobs WHERE job_id='{job_id}'"),
    ]);
    if !ok {
        return None;
    }
    // db exec prints `status: <value>`; just probe for the status keyword.
    for kw in ["queued", "failed", "claimed", "running", "succeeded", "blocked"] {
        if stdout.contains(kw) {
            return Some(kw.to_string());
        }
    }
    None
}

#[tokio::test]
async fn concurrent_claims_exactly_one_wins() {
    let _lock = claim_test_lock();
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let job_id = format!("zzclaim{}", nanos());
    teardown_job(&job_id);
    seed_queued_job(&job_id);
    let _g = Guard::new({
        let job_id = job_id.clone();
        move || teardown_job(&job_id)
    });

    let (ok1, out1, e1) = run(&["ota", "claim", "--worker", "w1"]);
    let (ok2, out2, e2) = run(&["ota", "claim", "--worker", "w2"]);
    if is_credless(&e1) || is_credless(&e2) {
        eprintln!("skipping (no creds mid-test)");
        return;
    }

    let wins = [(&ok1, &out1), (&ok2, &out2)]
        .iter()
        .filter(|(ok, out)| **ok && out.contains(&format!("job_id\t{job_id}")))
        .count();
    assert_eq!(wins, 1, "exactly one claim must win; out1={out1} out2={out2}");
}

#[tokio::test]
async fn stale_token_finish_after_reap_affects_zero_rows() {
    let _lock = claim_test_lock();
    let job_id = format!("zzclaim{}", nanos());
    let token = format!("zztok{}", nanos());
    teardown_job(&job_id);
    insert_claimed_job(&job_id, &token, "w-stale", "2026-06-29T15:10:01Z");
    let _g = Guard::new({
        let job_id = job_id.clone();
        move || teardown_job(&job_id)
    });

    let (ok, stdout, stderr) = run(&[
        "ota",
        "reap-stale",
        "--now",
        "2026-06-29T15:10:02Z",
    ]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "reap failed: {stderr}");
    let reaped: i64 = field(&stdout, "reaped")
        .unwrap_or_default()
        .parse()
        .unwrap_or(0);
    assert!(reaped >= 1, "reap should include our job; stdout={stdout}");

    let (ok, _out, stderr) = run(&[
        "ota",
        "finish",
        &job_id,
        &token,
        "--status",
        "succeeded",
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(!ok, "stale token finish must fail; ok={ok} stderr={stderr}");

    let (ok, _out, stderr) = run(&[
        "ota",
        "heartbeat",
        &job_id,
        &token,
    ]);
    if is_credless(&stderr) {
        return;
    }
    assert!(!ok, "stale token heartbeat must fail");
}

#[tokio::test]
async fn live_heartbeat_prevents_reap() {
    let _lock = claim_test_lock();
    let job_id = format!("zzclaim{}", nanos());
    let token = format!("zztok{}", nanos());
    teardown_job(&job_id);
    // Expired lease; heartbeat will extend it to ~now+600.
    insert_claimed_job(&job_id, &token, "w-live", "2000-01-01T00:00:00Z");
    let _g = Guard::new({
        let job_id = job_id.clone();
        move || teardown_job(&job_id)
    });

    let (ok, _out, stderr) = run(&[
        "ota",
        "heartbeat",
        &job_id,
        &token,
        "--lease-seconds",
        "600",
    ]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "heartbeat failed: {stderr}");

    // Reap time after the old lease but before the heartbeat-extended lease (~now+600).
    let (ok, stdout, stderr) = run(&[
        "ota",
        "reap-stale",
        "--now",
        "2000-01-02T00:00:00Z",
    ]);
    assert!(ok, "reap failed: {stderr}");
    assert_eq!(field(&stdout, "reaped").as_deref(), Some("0"));

    let (ok, _out, stderr) = run(&[
        "ota",
        "finish",
        &job_id,
        &token,
        "--status",
        "succeeded",
    ]);
    assert!(ok, "finish with live token should succeed: {stderr}");
}

#[tokio::test]
async fn blocked_finish_requires_valid_blocked_code() {
    let _lock = claim_test_lock();
    let job_id = format!("zzclaim{}", nanos());
    let token = format!("zztok{}", nanos());
    teardown_job(&job_id);
    insert_claimed_job(&job_id, &token, "w-block", "2099-01-01T00:00:00Z");
    let _g = Guard::new({
        let job_id = job_id.clone();
        move || teardown_job(&job_id)
    });

    let (ok, _out, stderr) = run(&[
        "ota",
        "finish",
        &job_id,
        &token,
        "--status",
        "blocked",
    ]);
    if is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(!ok, "blocked without code must fail");
    assert!(stderr.contains("blocked") || stderr.contains("Error"));

    let (ok, _out, stderr) = run(&[
        "ota",
        "finish",
        &job_id,
        &token,
        "--status",
        "blocked",
        "--blocked",
        "captcha",
    ]);
    assert!(ok, "blocked with valid code should succeed: {stderr}");
}

#[tokio::test]
async fn reap_parks_exhausted_job_and_requeues_retriable() {
    let _lock = claim_test_lock();
    let exhausted = format!("zzclaimx{}", nanos());
    let retriable = format!("zzclaimr{}", nanos());
    teardown_job(&exhausted);
    teardown_job(&retriable);
    // Both leases already expired at the reap time below.
    insert_claimed_job_with_attempts(&exhausted, "2026-06-29T15:09:00Z", 3, 3);
    insert_claimed_job_with_attempts(&retriable, "2026-06-29T15:09:00Z", 1, 3);
    let _g = Guard::new({
        let (a, b) = (exhausted.clone(), retriable.clone());
        move || {
            teardown_job(&a);
            teardown_job(&b);
        }
    });

    let (ok, stdout, stderr) = run(&["ota", "reap-stale", "--now", "2026-06-29T15:10:02Z"]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "reap failed: {stderr}");
    // At least our two jobs are acted on; the exhausted one is parked, the retriable requeued.
    let failed: i64 = field(&stdout, "failed").unwrap_or_default().parse().unwrap_or(0);
    let requeued: i64 = field(&stdout, "requeued").unwrap_or_default().parse().unwrap_or(0);
    assert!(failed >= 1, "expected >=1 parked-as-failed; stdout={stdout}");
    assert!(requeued >= 1, "expected >=1 requeued; stdout={stdout}");

    assert_eq!(job_status(&exhausted).as_deref(), Some("failed"), "ceiling job must be parked");
    assert_eq!(job_status(&retriable).as_deref(), Some("queued"), "retriable job must requeue");
}

#[tokio::test]
async fn reap_stale_rejects_loose_now_format() {
    // --now is validated BEFORE connecting to Turso, so this needs no creds.
    let (ok, _stdout, stderr) = run(&["ota", "reap-stale", "--now", "2026-6-29 10:00"]);
    assert!(!ok, "loose --now must be rejected; stderr={stderr}");
    assert!(
        stderr.contains("invalid timestamp") || stderr.contains("expected YYYY"),
        "stderr={stderr}"
    );
}