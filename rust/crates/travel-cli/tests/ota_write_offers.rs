//! Integration test for `travel ota write-offers`.

use std::path::PathBuf;
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

fn field(stdout: &str, key: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}\t")).map(|v| v.to_string()))
}

fn fixture_tsv() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zz_ota_offers.tsv")
}

fn teardown(job_id: &str, capture_id: &str) {
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM offers WHERE produced_by_job_id='{job_id}'"),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM ota_attempts WHERE job_id='{job_id}'"),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM ota_jobs WHERE job_id='{job_id}'"),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM captures WHERE capture_id='{capture_id}'"),
    ]);
}

#[tokio::test]
async fn write_offers_from_fixture_tsv() {
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

fn bad_header_tsv() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zz_bad_header.tsv")
}

#[tokio::test]
async fn write_offers_bad_header_fails_loud() {
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
        &format!(
            "SELECT count(*) AS n FROM offers WHERE produced_by_job_id='{job_id}'"
        ),
    ]);
    if ok {
        assert!(stdout.contains(": 0") || stdout.contains(":0"));
    }
}