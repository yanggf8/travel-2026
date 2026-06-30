//! End-to-end test for `travel ota run --capture-only`. Real-Turso + browser;
//! skips cleanly if Turso creds or Chrome remote debugging are absent.

use std::net::{SocketAddr, TcpStream};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

static RUN_CAPTURE_ONLY_TEST_LOCK: Mutex<()> = Mutex::new(());

fn run_capture_only_test_lock() -> std::sync::MutexGuard<'static, ()> {
    RUN_CAPTURE_ONLY_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn is_credless(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
}

fn chrome_available() -> bool {
    let addr: SocketAddr = "127.0.0.1:9222".parse().expect("valid chrome addr");
    TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
}

fn run(args: &[&str]) -> (bool, String, String) {
    let out = bin()
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

fn count(sql: &str) -> Option<i64> {
    let (ok, stdout, stderr) = run(&["db", "exec", sql]);
    if !ok {
        if is_credless(&stderr) {
            eprintln!("skipping (no creds mid-test): {}", stderr.trim());
            return None;
        }
        panic!("db exec failed: {}\nSQL: {sql}", stderr.trim());
    }
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("n: ").map(|s| s.trim().parse::<i64>().unwrap_or(-1)))
}

fn scalar(sql: &str) -> Option<String> {
    let (ok, stdout, stderr) = run(&["db", "exec", sql]);
    if !ok {
        if is_credless(&stderr) {
            eprintln!("skipping (no creds mid-test): {}", stderr.trim());
            return None;
        }
        panic!("db exec failed: {}\nSQL: {sql}", stderr.trim());
    }
    stdout
        .lines()
        .find_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
}

fn teardown() {
    let _ = run(&[
        "db",
        "exec",
        "DELETE FROM offers WHERE produced_by_job_id IN \
         (SELECT job_id FROM ota_jobs WHERE source_id='zztest')",
    ]);
    let _ = run(&[
        "db",
        "exec",
        "DELETE FROM ota_observations WHERE job_id IN \
         (SELECT job_id FROM ota_jobs WHERE source_id='zztest')",
    ]);
    let _ = run(&[
        "db",
        "exec",
        "DELETE FROM ota_attempts WHERE job_id IN \
         (SELECT job_id FROM ota_jobs WHERE source_id='zztest')",
    ]);
    let _ = run(&[
        "db",
        "exec",
        "DELETE FROM ota_job_params WHERE job_id IN \
         (SELECT job_id FROM ota_jobs WHERE source_id='zztest')",
    ]);
    let _ = run(&["db", "exec", "DELETE FROM ota_jobs WHERE source_id='zztest'"]);
    let _ = run(&[
        "db",
        "exec",
        "DELETE FROM captures WHERE source_id='zztest' AND url LIKE 'https://example.com%'",
    ]);
    let _ = run(&[
        "db",
        "exec",
        "DELETE FROM ota_source_workflow WHERE source_id='zztest'",
    ]);
    let _ = run(&[
        "db",
        "exec",
        "DELETE FROM ota_source_coverage WHERE source_id='zztest'",
    ]);
    let _ = run(&["db", "exec", "DELETE FROM ota_sources WHERE source_id='zztest'"]);
}

fn seed_workflow() {
    let now = "2026-06-30T10:30:00Z";
    let (ok, _stdout, stderr) = run(&[
        "db",
        "exec",
        &format!(
            "INSERT OR IGNORE INTO ota_sources (source_id, name, status, updated_at) \
             VALUES ('zztest', 'ZZ Test', 'active', '{now}')"
        ),
    ]);
    assert!(ok, "seed ota_sources failed: {stderr}");

    let (ok, _stdout, stderr) = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_source_workflow \
             (source_id, product_type, nav_kind, url_template, capture_url_contains, \
              settle_ms, agent_extraction_note, updated_at) \
             VALUES ('zztest', 'fit', 'get', 'https://example.com/', 'example.com', \
              0, 'zztest capture-only simple page', '{now}')"
        ),
    ]);
    assert!(ok, "seed ota_source_workflow failed: {stderr}");
}

#[tokio::test]
async fn run_capture_only_claims_job_captures_page_and_writes_no_offers() {
    let _lock = run_capture_only_test_lock();

    let (ok, _stdout, stderr) = run(&["db", "migrate"]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "db migrate should succeed; stderr={stderr}");

    if !chrome_available() {
        eprintln!("skipping (Chrome remote debugging not available on 127.0.0.1:9222)");
        return;
    }

    teardown();
    let _g = Guard::new(teardown);
    seed_workflow();

    let (ok, stdout, stderr) = run(&["ota", "run", "--capture-only", "zztest", "fit"]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds mid-test): {}", stderr.trim());
        return;
    }
    assert!(ok, "ota run --capture-only failed: stdout={stdout} stderr={stderr}");

    let job_id = field(&stdout, "job_id").expect("stdout must include job_id");
    let claim_token = field(&stdout, "claim_token").expect("stdout must include claim_token");
    let capture_id = field(&stdout, "capture_id").expect("stdout must include capture_id");

    assert!(!job_id.trim().is_empty(), "job_id must be non-empty");
    assert!(!claim_token.trim().is_empty(), "claim_token must be non-empty");
    assert!(!capture_id.trim().is_empty(), "capture_id must be non-empty");
    assert!(stdout.contains("source_id\tzztest"), "stdout={stdout}");
    assert!(stdout.contains("product_type\tfit"), "stdout={stdout}");
    assert!(
        stdout.contains("agent_extraction_note\tzztest capture-only simple page"),
        "stdout={stdout}"
    );

    let Some(captures) = count(&format!(
        "SELECT count(*) AS n FROM captures WHERE capture_id='{capture_id}' AND source_id='zztest'"
    )) else {
        return;
    };
    assert_eq!(captures, 1, "capture row must exist for capture_id={capture_id}");

    let Some(status) = scalar(&format!(
        "SELECT status FROM ota_jobs WHERE job_id='{job_id}'"
    )) else {
        return;
    };
    assert_eq!(status, "claimed", "capture-only job should remain claimed");

    let Some(db_token) = scalar(&format!(
        "SELECT claim_token FROM ota_jobs WHERE job_id='{job_id}'"
    )) else {
        return;
    };
    assert_eq!(db_token, claim_token, "printed token must match job row token");

    let Some(offers) = count(&format!(
        "SELECT count(*) AS n FROM offers WHERE produced_by_job_id='{job_id}'"
    )) else {
        return;
    };
    assert_eq!(offers, 0, "capture-only must not write offers");
}
