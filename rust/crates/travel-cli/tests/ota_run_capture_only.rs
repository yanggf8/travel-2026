//! End-to-end test for `travel ota run --capture-only`. Real-Turso + browser;
//! skips cleanly if Turso creds or Chrome remote debugging are absent.

use std::net::{SocketAddr, TcpStream};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

fn teardown_job(job_id: &str, capture_id: &str) {
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM offers WHERE produced_by_job_id='{job_id}'"),
    ]);
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
    let _ = run(&["db", "exec", &format!("DELETE FROM ota_jobs WHERE job_id='{job_id}'")]);
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM captures WHERE capture_id='{capture_id}'"),
    ]);
}

#[tokio::test]
async fn run_capture_only_resolves_destination_tokens_for_verified_sources() {
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

    // The capture step shells out to gwebcdb's bridge/ota_capture.py, which reads TURSO_URL /
    // TURSO_TOKEN directly from the environment (it has no .env loader — see CLAUDE.md "OTA
    // scraping"). The Rust CLI uses TRAVEL_TURSO_* and can be credentialed without those, so gate
    // on them explicitly and skip cleanly rather than fail when only the CLI creds are present.
    if std::env::var("TURSO_URL").is_err() || std::env::var("TURSO_TOKEN").is_err() {
        eprintln!("skipping (gwebcdb needs TURSO_URL/TURSO_TOKEN in env for ota_capture.py)");
        return;
    }

    let produced: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let _g = Guard::new({
        let produced = Arc::clone(&produced);
        move || {
            let rows = produced
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            for (job_id, capture_id) in rows {
                teardown_job(&job_id, &capture_id);
            }
        }
    });

    let cases: &[(&str, &str, &[&str])] = &[
        ("besttour", "group_tour", &["295"]),
        ("settour", "fit", &["NRT", "179900"]),
        ("eztravel", "fit", &["TYO"]),
    ];

    for (source_id, product_type, expected_url_fragments) in cases {
        let (ok, stdout, stderr) = run(&[
            "ota",
            "run",
            "--capture-only",
            source_id,
            product_type,
            "--destination",
            "tokyo",
            "--depart",
            "2026-09-01",
            "--return",
            "2026-09-05",
            "--pax",
            "2",
        ]);
        if !ok && is_credless(&stderr) {
            eprintln!("skipping (no creds mid-test): {}", stderr.trim());
            return;
        }

        let combined = format!("{stdout}{stderr}");
        let combined_lower = combined.to_lowercase();
        assert!(
            !combined_lower.contains("missing placeholder"),
            "{source_id}/{product_type} must not fail URL interpolation; output={combined}"
        );
        assert!(
            !combined_lower.contains("no token"),
            "{source_id}/{product_type} must not fail URL-token lookup; output={combined}"
        );
        assert!(
            !combined_lower.contains("missing --destination"),
            "{source_id}/{product_type} received --destination tokyo; output={combined}"
        );
        assert!(
            ok,
            "ota run --capture-only {source_id} {product_type} failed: stdout={stdout} stderr={stderr}"
        );

        let job_id = field(&stdout, "job_id").expect("stdout must include job_id");
        let claim_token = field(&stdout, "claim_token").expect("stdout must include claim_token");
        let capture_id = field(&stdout, "capture_id").expect("stdout must include capture_id");

        produced
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((job_id.clone(), capture_id.clone()));

        assert!(!job_id.trim().is_empty(), "job_id must be non-empty");
        assert!(!claim_token.trim().is_empty(), "claim_token must be non-empty");
        assert!(!capture_id.trim().is_empty(), "capture_id must be non-empty");
        assert!(
            stdout.contains(&format!("source_id\t{source_id}")),
            "stdout={stdout}"
        );
        assert!(
            stdout.contains(&format!("product_type\t{product_type}")),
            "stdout={stdout}"
        );

        let Some(captures) = count(&format!(
            "SELECT count(*) AS n FROM captures WHERE capture_id='{capture_id}' AND source_id='{source_id}'"
        )) else {
            return;
        };
        assert_eq!(captures, 1, "capture row must exist for capture_id={capture_id}");

        let Some(capture_url) = scalar(&format!(
            "SELECT url FROM captures WHERE capture_id='{capture_id}'"
        )) else {
            return;
        };
        assert!(
            !capture_url.contains('{') && !capture_url.contains('}'),
            "resolved capture URL must not contain unresolved placeholders; url={capture_url}"
        );
        for fragment in *expected_url_fragments {
            assert!(
                capture_url.contains(fragment),
                "{source_id}/{product_type} capture URL should include token fragment {fragment}; url={capture_url}"
            );
        }

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
}
