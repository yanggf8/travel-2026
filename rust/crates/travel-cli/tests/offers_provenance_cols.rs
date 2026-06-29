//! Integration test for nullable OTA provenance columns on global `offers`.
//! Real-Turso; skips if creds absent. Test-owned rows use zz* and Guard teardown.

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

fn columns_of(table: &str) -> Option<Vec<String>> {
    let (ok, stdout, stderr) = run(&["db", "schema", table]);
    if !ok {
        if is_credless(&stderr) {
            eprintln!("skipping offers-provenance-cols test: {}", stderr.trim());
            return None;
        }
        panic!("db schema {table} failed: {}", stderr.trim());
    }
    let cols: Vec<String> = stdout
        .lines()
        .filter(|l| l.starts_with("  ") && !l.contains("columns)"))
        .filter_map(|l| l.split_whitespace().next().map(|s| s.to_string()))
        .collect();
    Some(cols)
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

fn teardown(offer_id: &str, scraped_at: &str) {
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM offers WHERE id='{offer_id}' AND scraped_at='{scraped_at}'"),
    ]);
}

#[tokio::test]
async fn offers_has_provenance_columns_after_migrate() {
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }
    assert!(ok, "db migrate should succeed; err={err}");

    let want = [
        "capture_id",
        "produced_by_job_id",
        "produced_by_attempt_id",
        "parser_method",
        "capture_checksum",
        "parser_rule_checksum",
        "normalizer_version",
    ];
    let Some(have) = columns_of("offers") else {
        return;
    };
    for col in want {
        assert!(
            have.iter().any(|c| c == col),
            "offers must have column {col}; got {have:?}"
        );
    }
}

#[tokio::test]
async fn legacy_offer_insert_omitting_provenance_still_succeeds() {
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let offer_id = format!("zzlegacy{}", nanos());
    let scraped_at = "2026-06-29T12:00:00Z";
    teardown(&offer_id, scraped_at);
    let _g = Guard::new({
        let (offer_id, scraped_at) = (offer_id.clone(), scraped_at.to_string());
        move || teardown(&offer_id, &scraped_at)
    });

    let (ok, _out, err) = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO offers (id, source_id, type, scraped_at) \
             VALUES ('{offer_id}', 'zztest', 'package', '{scraped_at}')"
        ),
    ]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds mid-test): {}", err.trim());
        return;
    }
    assert!(ok, "legacy insert without provenance cols should succeed; err={err}");

    let Some(n) = scalar(&format!(
        "SELECT count(*) AS n FROM offers WHERE id='{offer_id}' AND scraped_at='{scraped_at}'"
    )) else {
        return;
    };
    assert_eq!(n, "1");
}

#[tokio::test]
async fn provenance_bearing_insert_round_trips_all_columns() {
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let offer_id = format!("zzprov{}", nanos());
    let scraped_at = "2026-06-29T12:01:00Z";
    let capture_id = format!("zzcap{}", nanos());
    let job_id = format!("zzjob{}", nanos());
    let attempt_id = format!("zzatt{}", nanos());
    teardown(&offer_id, scraped_at);
    let _g = Guard::new({
        let (offer_id, scraped_at) = (offer_id.clone(), scraped_at.to_string());
        move || teardown(&offer_id, &scraped_at)
    });

    let insert = format!(
        "INSERT INTO offers \
         (id, source_id, type, scraped_at, capture_id, produced_by_job_id, produced_by_attempt_id, \
          parser_method, capture_checksum, parser_rule_checksum, normalizer_version) \
         VALUES ('{offer_id}', 'zztest', 'package', '{scraped_at}', '{capture_id}', '{job_id}', \
          '{attempt_id}', 'regex', 'capsha1', 'rulesha1', 'norm-v1')"
    );
    let (ok, _out, err) = run(&["db", "exec", &insert]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds mid-test): {}", err.trim());
        return;
    }
    assert!(ok, "provenance insert should succeed; err={err}");

    let select = format!(
        "SELECT capture_id, produced_by_job_id, produced_by_attempt_id, parser_method, \
         capture_checksum, parser_rule_checksum, normalizer_version \
         FROM offers WHERE id='{offer_id}' AND scraped_at='{scraped_at}'"
    );
    let (ok, stdout, stderr) = run(&["db", "exec", &select]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds mid-test): {}", stderr.trim());
        return;
    }
    assert!(ok, "provenance select should succeed; err={stderr}");

    let values: Vec<String> = stdout
        .lines()
        .next()
        .unwrap_or("")
        .split(", ")
        .filter_map(|part| part.split_once(':').map(|(_, v)| v.trim().to_string()))
        .collect();
    assert_eq!(
        values,
        vec![
            capture_id,
            job_id,
            attempt_id,
            "regex".to_string(),
            "capsha1".to_string(),
            "rulesha1".to_string(),
            "norm-v1".to_string(),
        ],
        "provenance columns must round-trip; stdout={stdout}"
    );
}

#[tokio::test]
async fn offers_rejects_invalid_parser_method() {
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let offer_id = format!("zzbad{}", nanos());
    let scraped_at = "2026-06-29T12:02:00Z";
    teardown(&offer_id, scraped_at);
    let _g = Guard::new({
        let (offer_id, scraped_at) = (offer_id.clone(), scraped_at.to_string());
        move || teardown(&offer_id, &scraped_at)
    });

    let (ok, stdout, stderr) = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO offers (id, source_id, type, scraped_at, parser_method) \
             VALUES ('{offer_id}', 'zztest', 'package', '{scraped_at}', 'bad')"
        ),
    ]);
    if is_credless(&stderr) {
        eprintln!("skipping (no creds mid-test): {}", stderr.trim());
        return;
    }
    let combined = format!("{stdout}{stderr}").to_lowercase();
    assert!(
        !ok && (combined.contains("constraint") || combined.contains("check")),
        "parser_method='bad' must be rejected; ok={ok} stdout={stdout} stderr={stderr}"
    );
}
