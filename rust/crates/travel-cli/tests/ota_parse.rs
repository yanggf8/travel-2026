//! Integration test for `travel ota parse` under a token-guarded job.

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

fn teardown_all(job_id: &str, capture_id: &str, offer_prefix: &str) {
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM offers WHERE produced_by_job_id='{job_id}'"),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM offers WHERE id LIKE '{offer_prefix}%'"),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM ota_attempts WHERE job_id='{job_id}'"),
    ]);
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
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM captures WHERE capture_id='{capture_id}'"),
    ]);
    let _ = run(&[
        "db",
        "exec",
        "DELETE FROM parser_rules WHERE source_id='zztest' AND product_type='fit'",
    ]);
}

fn seed_parse_fixtures(capture_id: &str) {
    let now = "2026-06-29T15:20:00Z";
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT OR IGNORE INTO ota_sources (source_id, name, status, updated_at) \
             VALUES ('zztest', 'ZZ Test', 'active', '{now}')"
        ),
    ]);
    let raw = "出發 2026/09/01 回程 2026/09/05\n5天4夜\n總價 NT$20000\nCI100 CI101\n飯店\nHOTEL ZZTEST";
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO captures (capture_id, source_id, url, captured_at, raw_text) \
             VALUES ('{capture_id}', 'zztest', 'https://example.com/pkg/ZZ01', '{now}', '{raw}')"
        ),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO parser_rules \
             (source_id, product_type, date_range_rx, nights_rx, nights_is_days, price_marker, \
              price_amount_rx, price_basis, pax_divisor, flight_rx, hotel_anchor_rx, currency, \
              has_custom_parser) \
             VALUES ('zztest', 'fit', \
              '出發\\s+(\\d{{4}}/\\d{{2}}/\\d{{2}}).*回程\\s+(\\d{{4}}/\\d{{2}}/\\d{{2}})', \
              '(\\d+)天(\\d+)夜', 0, '總價', 'NT\\$?(\\d+)', 'total', 2, '([A-Z]{{2}}\\d+)', \
              '飯店', 'TWD', 0)"
        ),
    ]);
}

#[tokio::test]
async fn parse_under_job_writes_provenance() {
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let suffix = nanos();
    let capture_id = format!("zzcap{suffix}");
    let job_id = format!("zzjob{suffix}");
    teardown_all(&job_id, &capture_id, "zztest");
    seed_parse_fixtures(&capture_id);

    let now = "2026-06-29T15:20:00Z";
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'queued', '{now}', '{now}')"
        ),
    ]);

    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown_all(&job_id, &capture_id, "zztest")
    });

    let claim_token = format!("zztoken{suffix}");
    let lease = "2026-06-29T16:00:00Z";
    let (ok, _out, stderr) = run(&[
        "db",
        "exec",
        &format!(
            "UPDATE ota_jobs SET status='claimed', claimed_by='parse-w', claim_token='{claim_token}', \
             claimed_at='{now}', heartbeat_at='{now}', lease_expires_at='{lease}', updated_at='{now}' \
             WHERE job_id='{job_id}' AND status='queued'"
        ),
    ]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "force claim failed: {stderr}");

    let (ok, stdout, stderr) = run(&[
        "ota",
        "parse",
        &job_id,
        &capture_id,
        "--source",
        "zztest",
        "--product-type",
        "fit",
        "--claim-token",
        &claim_token,
    ]);
    assert!(ok, "parse failed: {stderr}");
    assert!(stdout.contains("inserted\t1") || stdout.contains("inserted\t0"));
    assert!(stdout.contains("capture_checksum\t"));
    assert!(stdout.contains("parser_rule_checksum\t"));

    let (ok, stdout, _stderr) = run(&[
        "db",
        "exec",
        &format!(
            "SELECT parser_method, capture_checksum, produced_by_job_id \
             FROM offers WHERE produced_by_job_id='{job_id}' LIMIT 1"
        ),
    ]);
    assert!(ok, "offer select failed");
    assert!(stdout.contains("regex"));
    assert!(stdout.contains(&job_id));
}

#[tokio::test]
async fn parse_dedup_on_repeat_insert_same_pk() {
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }
    let suffix = nanos();
    let offer_id = format!("zzdedup{suffix}");
    let scraped_at = "2026-06-29T15:22:00Z";
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM offers WHERE id='{offer_id}' AND scraped_at='{scraped_at}'"),
    ]);
    let _g = Guard::new({
        let offer_id = offer_id.clone();
        let scraped_at = scraped_at.to_string();
        move || {
            let _ = run(&[
                "db",
                "exec",
                &format!(
                    "DELETE FROM offers WHERE id='{offer_id}' AND scraped_at='{scraped_at}'"
                ),
            ]);
        }
    });
    let insert = format!(
        "INSERT INTO offers (id, source_id, type, scraped_at, parser_method) \
         VALUES ('{offer_id}', 'zztest', 'package', '{scraped_at}', 'regex')"
    );
    let (ok, _, stderr) = run(&["db", "exec", &insert]);
    assert!(ok, "seed offer failed: {stderr}");
    let dedup = format!(
        "INSERT OR IGNORE INTO offers (id, source_id, type, scraped_at, parser_method) \
         VALUES ('{offer_id}', 'zztest', 'package', '{scraped_at}', 'regex')"
    );
    let (ok, _, stderr) = run(&["db", "exec", &dedup]);
    if is_credless(&stderr) {
        return;
    }
    assert!(ok, "dedup insert should not error");
    let (ok, stdout, _) = run(&[
        "db",
        "exec",
        &format!(
            "SELECT count(*) AS n FROM offers WHERE id='{offer_id}' AND scraped_at='{scraped_at}'"
        ),
    ]);
    assert!(ok);
    assert!(stdout.contains("1"));
}

#[tokio::test]
async fn stale_token_parse_fails_before_writing_offers() {
    let suffix = nanos();
    let capture_id = format!("zzcapst{suffix}");
    let job_id = format!("zzjobst{suffix}");
    teardown_all(&job_id, &capture_id, "zztest");
    seed_parse_fixtures(&capture_id);
    let now = "2026-06-29T15:21:00Z";
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'queued', '{now}', '{now}')"
        ),
    ]);
    let _g = Guard::new({
        let (job_id, capture_id) = (job_id.clone(), capture_id.clone());
        move || teardown_all(&job_id, &capture_id, "zztest")
    });

    let (ok, _stdout, stderr) = run(&[
        "ota",
        "parse",
        &job_id,
        &capture_id,
        "--source",
        "zztest",
        "--product-type",
        "fit",
        "--claim-token",
        "stale-wrong-token",
    ]);
    if is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(!ok, "stale token parse must fail");
    let (ok, stdout, _stderr) = run(&[
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