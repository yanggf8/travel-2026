//! Schema/seed test for `ota_source_workflow`. Real-Turso; skips if creds absent.

use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

static WORKFLOW_SCHEMA_TEST_LOCK: Mutex<()> = Mutex::new(());

fn workflow_schema_test_lock() -> std::sync::MutexGuard<'static, ()> {
    WORKFLOW_SCHEMA_TEST_LOCK
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
            eprintln!("skipping workflow schema test: {}", stderr.trim());
            return None;
        }
        panic!("db schema {table} failed: {}", stderr.trim());
    }
    Some(
        stdout
            .lines()
            .filter(|l| l.starts_with("  ") && !l.contains("columns)"))
            .filter_map(|l| l.split_whitespace().next().map(|s| s.to_string()))
            .collect(),
    )
}

fn scalar(sql: &str) -> Option<String> {
    let (ok, stdout, stderr) = run(&["db", "exec", sql]);
    if !ok {
        if is_credless(&stderr) {
            eprintln!("skipping workflow schema test: {}", stderr.trim());
            return None;
        }
        panic!("db exec failed: {}\nSQL: {sql}", stderr.trim());
    }
    stdout
        .lines()
        .find_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
}

fn teardown(source_id: &str) {
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM ota_source_workflow WHERE source_id='{source_id}'"),
    ]);
}

#[tokio::test]
async fn workflow_table_exists_with_seed_rows_and_get_only_check() {
    let _lock = workflow_schema_test_lock();
    let (ok, _stdout, stderr) = run(&["db", "migrate"]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "db migrate should succeed; stderr={stderr}");

    let Some(cols) = columns_of("ota_source_workflow") else {
        return;
    };
    for col in [
        "source_id",
        "product_type",
        "nav_kind",
        "url_template",
        "capture_url_contains",
        "settle_marker",
        "settle_ms",
        "agent_extraction_note",
        "updated_at",
    ] {
        assert!(
            cols.iter().any(|c| c == col),
            "ota_source_workflow must have column {col}; got {cols:?}"
        );
    }

    let expected = [
        (
            "settour",
            "fit",
            "https://fit.settour.com.tw/product/v2?tripType=RT&directFlightOnly=true&roomQty=1&depAirportCode=TPE&arrAirportCode={dest_code}&depDate={depart},{return}&hotelCheckInDate={depart}&hotelCheckOutDate={return}&adtCount={pax}&chdCount=0&regionId={region_id}",
            "product/v2",
            "25000",
        ),
        (
            "eztravel",
            "fit",
            "https://packages.eztravel.com.tw/roundtrip-TPE-{dest_code}?checkin={depart}&checkout={return}&adult={pax}&child=0",
            "roundtrip-TPE",
            "25000",
        ),
        (
            "besttour",
            "group_tour",
            "https://www.besttour.com.tw/e_web/search?v=//////{region_id}///////",
            "e_web/search",
            "25000",
        ),
    ];

    for (source_id, product_type, url_template, capture_url_contains, settle_ms) in expected {
        let Some(got_url) = scalar(&format!(
            "SELECT url_template FROM ota_source_workflow \
             WHERE source_id='{source_id}' AND product_type='{product_type}'"
        )) else {
            return;
        };
        assert_eq!(got_url, url_template, "{source_id}/{product_type} url_template");

        let Some(got_contains) = scalar(&format!(
            "SELECT capture_url_contains FROM ota_source_workflow \
             WHERE source_id='{source_id}' AND product_type='{product_type}'"
        )) else {
            return;
        };
        assert_eq!(
            got_contains, capture_url_contains,
            "{source_id}/{product_type} capture_url_contains"
        );

        let Some(got_settle_ms) = scalar(&format!(
            "SELECT settle_ms FROM ota_source_workflow \
             WHERE source_id='{source_id}' AND product_type='{product_type}'"
        )) else {
            return;
        };
        assert_eq!(got_settle_ms, settle_ms, "{source_id}/{product_type} settle_ms");
    }

    let bad_source_id = format!("zzwf{}", nanos());
    teardown(&bad_source_id);
    let _g = Guard::new({
        let bad_source_id = bad_source_id.clone();
        move || teardown(&bad_source_id)
    });

    let (ok, stdout, stderr) = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_source_workflow \
             (source_id, product_type, nav_kind, url_template, settle_ms) \
             VALUES ('{bad_source_id}', 'fit', 'form', 'https://example.com/', 0)"
        ),
    ]);
    if is_credless(&stderr) {
        eprintln!("skipping (no creds mid-test): {}", stderr.trim());
        return;
    }
    let combined = format!("{stdout}{stderr}").to_lowercase();
    assert!(
        !ok && (combined.contains("constraint") || combined.contains("check")),
        "nav_kind='form' must be rejected by CHECK; ok={ok} stdout={stdout} stderr={stderr}"
    );
}

