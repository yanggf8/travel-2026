//! Integration test for OTA execution schema tables (ota_jobs, ota_job_params,
//! ota_attempts, ota_observations). Real-Turso; skips if creds absent.
//! Test-owned rows use zz* prefixes and Guard teardown.

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
            eprintln!("skipping ota-execution-schema test: {}", stderr.trim());
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

fn assert_has_columns(table: &str, want: &[&str], have: &[String]) {
    for col in want {
        assert!(
            have.iter().any(|c| c == col),
            "table {table} must have column {col}; got {have:?}"
        );
    }
}

fn exec_must_fail(sql: &str, label: &str) {
    let (ok, stdout, stderr) = run(&["db", "exec", sql]);
    if is_credless(&stderr) {
        eprintln!("skipping (no creds mid-test): {}", stderr.trim());
        return;
    }
    let combined = format!("{stdout}{stderr}").to_lowercase();
    assert!(
        !ok && (combined.contains("constraint") || combined.contains("check")),
        "{label} must be rejected by CHECK; ok={ok} stdout={stdout} stderr={stderr}"
    );
}

fn teardown(job_id: &str) {
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM ota_observations WHERE job_id='{job_id}'"),
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
}

#[tokio::test]
async fn execution_tables_exist_with_documented_columns() {
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }
    assert!(ok, "db migrate should succeed; err={err}");

    let expected: &[(&str, &[&str])] = &[
        (
            "ota_jobs",
            &[
                "job_id",
                "source_id",
                "product_type",
                "status",
                "claimed_by",
                "claimed_at",
                "claim_token",
                "lease_expires_at",
                "heartbeat_at",
                "attempts",
                "max_attempts",
                "next_retry_at",
                "blocked_reason_code",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "ota_job_params",
            &["job_id", "param_key", "param_value"],
        ),
        (
            "ota_attempts",
            &[
                "attempt_id",
                "job_id",
                "attempt_no",
                "claim_token",
                "outcome",
                "capture_id",
                "candidate_count",
                "inserted_count",
                "deduped_count",
                "error_detail",
                "started_at",
                "finished_at",
            ],
        ),
        (
            "ota_observations",
            &[
                "observation_id",
                "source_id",
                "product_type",
                "job_id",
                "attempt_id",
                "observation_type",
                "block_reason_code",
                "severity",
                "http_status",
                "field_name",
                "selector",
                "expected_value",
                "observed_value",
                "duration_ms",
                "freshness_reference_at",
                "detail",
                "observed_at",
            ],
        ),
    ];

    for (table, want_cols) in expected {
        let Some(have) = columns_of(table) else {
            return;
        };
        assert_has_columns(table, want_cols, &have);
    }
}

#[tokio::test]
async fn execution_check_constraints_reject_invalid_rows() {
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let job_id = format!("zzjob{}", nanos());
    let now = "2026-06-29T12:00:00Z";
    teardown(&job_id);
    let _g = Guard::new({
        let job_id = job_id.clone();
        move || teardown(&job_id)
    });

    // Bad ota_jobs.status
    exec_must_fail(
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', 'bogus', '{now}', '{now}')"
        ),
        "invalid ota_jobs.status",
    );

    // Valid job row for child-table checks
    let (ok, _out, err) = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, created_at, updated_at) \
             VALUES ('{job_id}', 'zztest', 'fit', '{now}', '{now}')"
        ),
    ]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds mid-test): {}", err.trim());
        return;
    }
    assert!(ok, "seed ota_jobs row should succeed; err={err}");

    for (param_key, param_value) in [
        ("origin", "TPE"),
        ("currency", "TWD"),
        ("rooms", "1"),
        ("hotel", "my-hotel"),
    ] {
        let (ok, stdout, stderr) = run(&[
            "db",
            "exec",
            &format!(
                "INSERT INTO ota_job_params (job_id, param_key, param_value) \
                 VALUES ('{job_id}', '{param_key}', '{param_value}')"
            ),
        ]);
        if !ok && is_credless(&stderr) {
            eprintln!("skipping (no creds mid-test): {}", stderr.trim());
            return;
        }
        assert!(
            ok,
            "ota_job_params.param_key='{param_key}' should be accepted; stdout={stdout} stderr={stderr}"
        );
    }

    // Bad ota_job_params.param_key
    exec_must_fail(
        &format!(
            "INSERT INTO ota_job_params (job_id, param_key, param_value) \
             VALUES ('{job_id}', 'bogus_key', 'x')"
        ),
        "invalid ota_job_params.param_key",
    );

    // Blocked without blocked_reason_code
    exec_must_fail(
        &format!(
            "INSERT INTO ota_jobs (job_id, source_id, product_type, status, created_at, updated_at) \
             VALUES ('{job_id}-blocked', 'zztest', 'fit', 'blocked', '{now}', '{now}')"
        ),
        "blocked ota_jobs without blocked_reason_code",
    );

    // Bad ota_observations.observation_type
    exec_must_fail(
        &format!(
            "INSERT INTO ota_observations \
             (observation_id, source_id, observation_type, observed_at) \
             VALUES ('zzobs{}', 'zztest', 'bogus_type', '{now}')",
            nanos()
        ),
        "invalid ota_observations.observation_type",
    );
}
