//! Integration test for `travel ota observations` read-only view.

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

fn teardown(obs_id: &str) {
    let _ = run(&[
        "db",
        "exec",
        &format!("DELETE FROM ota_observations WHERE observation_id='{obs_id}'"),
    ]);
}

#[tokio::test]
async fn observations_view_filters_and_shows_typed_facts() {
    let (ok, _o, err) = run(&["db", "migrate"]);
    if !ok && is_credless(&err) {
        eprintln!("skipping (no creds): {}", err.trim());
        return;
    }

    let obs_id = format!("zzobs{}", nanos());
    let now = "2026-06-29T15:40:00Z";
    teardown(&obs_id);
    let _g = Guard::new({
        let obs_id = obs_id.clone();
        move || teardown(&obs_id)
    });

    let insert = format!(
        "INSERT INTO ota_observations \
         (observation_id, source_id, product_type, observation_type, severity, field_name, \
          expected_value, observed_value, detail, observed_at) \
         VALUES ('{obs_id}', 'zztest', 'fit', 'parse_warning', 'warn', 'price', '10000', '9999', \
          'price mismatch on row 1', '{now}')"
    );
    let (ok, _, stderr) = run(&["db", "exec", &insert]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping (no creds mid-test): {}", stderr.trim());
        return;
    }
    assert!(ok, "seed observation failed: {stderr}");

    let (ok, stdout, stderr) = run(&[
        "ota",
        "observations",
        "--source",
        "zztest",
        "--type",
        "parse_warning",
        "--severity",
        "warn",
    ]);
    assert!(ok, "observations view failed: {stderr}");
    assert!(stdout.contains(&obs_id));
    assert!(stdout.contains("field_name"));
    assert!(stdout.contains("price"));
    assert!(stdout.contains("expected_value"));
    assert!(stdout.contains("10000"));
    assert!(stdout.contains("observed_value"));
    assert!(stdout.contains("9999"));
    assert!(stdout.contains("price mismatch"));
    assert!(stdout.contains("observations\t"));

    let (ok, stdout, stderr) = run(&[
        "ota",
        "observations",
        "--source",
        "zznonexistent-source-999",
    ]);
    assert!(ok, "empty filter view failed: {stderr}");
    assert!(
        stdout.contains("observations\t0") || !stdout.contains(&obs_id),
        "unrelated source filter should not show our row"
    );
}