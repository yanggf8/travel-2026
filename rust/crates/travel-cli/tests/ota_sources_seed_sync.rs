//! Regression: `db migrate` must NOT overwrite live OTA catalog edits.
//! The seed data is cold-start bootstrap only; once rows exist, Turso is authoritative.
//! Real-Turso integration test; skips cleanly if creds absent and restores real rows via Guard.

use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

static SEED_LOCK: Mutex<()> = Mutex::new(());

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

fn cell(sql: &str) -> Option<String> {
    let out = bin().args(["db", "exec", sql]).output().expect("db exec");
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if is_credless(&stderr) {
            eprintln!("skipping ota-sources-seed-sync test: {}", stderr.trim());
            return None;
        }
        panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .find_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
}

fn restore_besttour(original_status: &str, test_region: &str) {
    let _ = run(&[
        "db",
        "exec",
        &format!("UPDATE ota_sources SET status='{original_status}' WHERE source_id='besttour'"),
    ]);
    let _ = run(&[
        "db",
        "exec",
        &format!(
            "DELETE FROM ota_source_regions WHERE source_id='besttour' AND region='{test_region}'"
        ),
    ]);
}

#[tokio::test]
async fn db_migrate_preserves_live_ota_source_edits() {
    let _lock = SEED_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let Some(original_status) = cell("SELECT status FROM ota_sources WHERE source_id='besttour'")
    else {
        return;
    };
    let test_region = format!("zzregion{}", nanos());
    let _g = Guard::new({
        let original_status = original_status.clone();
        let test_region = test_region.clone();
        move || restore_besttour(&original_status, &test_region)
    });

    let live_status = if original_status == "active" {
        "inactive"
    } else {
        "active"
    };
    let (ok, _stdout, stderr) = run(&["set-ota-source", "besttour", "--status", live_status]);
    assert!(
        ok,
        "set-ota-source should live-edit besttour status; err={stderr}"
    );
    let (ok, _stdout, stderr) = run(&[
        "db",
        "exec",
        &format!(
            "INSERT INTO ota_source_regions (source_id, region) VALUES ('besttour', '{test_region}')"
        ),
    ]);
    assert!(ok, "test child region insert should succeed; err={stderr}");

    let (ok, stdout, stderr) = run(&["db", "migrate"]);
    assert!(
        ok,
        "db migrate should succeed; stdout={stdout} stderr={stderr}"
    );

    assert_eq!(
        cell("SELECT status FROM ota_sources WHERE source_id='besttour'").as_deref(),
        Some(live_status),
        "db migrate must not overwrite a live-edited ota_sources status"
    );
    assert_eq!(
        cell(&format!(
            "SELECT count(*) AS n FROM ota_source_regions \
             WHERE source_id='besttour' AND region='{test_region}'"
        ))
        .as_deref(),
        Some("1"),
        "db migrate must not delete and reinsert child region rows"
    );
}
