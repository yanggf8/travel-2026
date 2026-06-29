//! Integration test for `travel ota enqueue`. Real-Turso; skips if creds absent.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

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

fn scalar(sql: &str) -> Option<String> {
    let (ok, stdout, stderr) = run(&["db", "exec", sql]);
    if !ok {
        if is_credless(&stderr) {
            return None;
        }
        panic!("db exec failed: {}\nSQL: {sql}", stderr.trim());
    }
    stdout
        .lines()
        .find_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
}

fn teardown_job(job_id: &str) {
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM ota_job_params WHERE job_id='{job_id}'"),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM ota_jobs WHERE job_id='{job_id}'"),
    ]);
}

fn ensure_zztest_source() {
    let now = "2026-06-29T15:00:00Z";
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
             VALUES ('zztest', 'fit', 'active', 'regex', '{now}')"
        ),
    ]);
}

fn teardown_zztest_source() {
    let _ = run(&[
        "db",
        "exec",
        "DELETE FROM ota_source_coverage WHERE source_id='zztest'",
    ]);
    let _ = run(&[
        "db",
        "exec",
        "DELETE FROM ota_sources WHERE source_id='zztest'",
    ]);
}

#[tokio::test]
async fn enqueue_creates_job_and_params() {
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }
    ensure_zztest_source();
    let _g_src = Guard::new(teardown_zztest_source);

    let (ok, stdout, stderr) = run(&[
        "ota",
        "enqueue",
        "zztest",
        "fit",
        "--depart",
        "2026-09-01",
        "--nights",
        "4",
    ]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds mid-test): {}", stderr.trim());
        return;
    }
    assert!(ok, "enqueue should succeed; stderr={stderr}");
    let job_id = stdout
        .lines()
        .find_map(|l| l.strip_prefix("job_id\t"))
        .expect("stdout must include job_id");
    assert!(job_id.contains('-'), "job_id={job_id}");
    let _g = Guard::new({
        let job_id = job_id.to_string();
        move || teardown_job(&job_id)
    });

    assert!(stdout.contains("status\tqueued"));
    let Some(n) = scalar(&format!(
        "SELECT count(*) AS n FROM ota_jobs WHERE job_id='{job_id}'"
    )) else {
        return;
    };
    assert_eq!(n, "1");
    let Some(p) = scalar(&format!(
        "SELECT count(*) AS n FROM ota_job_params WHERE job_id='{job_id}'"
    )) else {
        return;
    };
    assert_eq!(p, "2");
}

#[tokio::test]
async fn enqueue_rejects_bad_source_and_writes_nothing() {
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let before = scalar("SELECT count(*) AS n FROM ota_jobs WHERE source_id='zznonexistent999'")
        .unwrap_or_else(|| "0".to_string());
    let (ok, _stdout, stderr) = run(&[
        "ota",
        "enqueue",
        "zznonexistent999",
        "fit",
        "--nights",
        "4",
    ]);
    if is_credless(&stderr) {
        eprintln!("skipping (no creds mid-test): {}", stderr.trim());
        return;
    }
    assert!(!ok, "bad source must fail; ok={ok}");
    assert!(
        stderr.contains("not found") || stderr.contains("Error"),
        "stderr={stderr}"
    );
    let after = scalar("SELECT count(*) AS n FROM ota_jobs WHERE source_id='zznonexistent999'")
        .unwrap_or_else(|| before.clone());
    assert_eq!(before, after);
}

#[tokio::test]
async fn enqueue_rejects_bad_product_type() {
    ensure_zztest_source();
    let _g_src = Guard::new(teardown_zztest_source);
    let (ok, _stdout, stderr) = run(&[
        "ota",
        "enqueue",
        "zztest",
        "bogus_type",
    ]);
    if is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(!ok, "bad product_type must fail");
    assert!(stderr.contains("product_type") || stderr.contains("Error"));
}