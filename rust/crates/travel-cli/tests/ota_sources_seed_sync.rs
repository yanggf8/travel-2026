//! Regression: `db migrate` must NOT overwrite live OTA catalog edits.
//! The seed data is cold-start bootstrap only; once rows exist, Turso is authoritative.
//! Real-Turso integration test; skips cleanly if creds absent and restores real rows via Guard.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, nanos, Guard};

static SEED_LOCK: Mutex<()> = Mutex::new(());

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn restore_besttour(original_status: &str, test_region: &str) {
    let _ = db_exec_teardown(&format!(
        "UPDATE ota_sources SET status='{original_status}' WHERE source_id='besttour'"
    ));
    let _ = db_exec_teardown(&format!(
        "DELETE FROM ota_source_regions WHERE source_id='besttour' AND region='{test_region}'"
    ));
}

#[tokio::test]
async fn db_migrate_preserves_live_ota_source_edits() {
    let _lock = SEED_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let Some(original_status) =
        db_exec("SELECT status FROM ota_sources WHERE source_id='besttour'").and_then(|r| r.scalar())
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
        db_exec("SELECT status FROM ota_sources WHERE source_id='besttour'")
            .and_then(|r| r.scalar())
            .as_deref(),
        Some(live_status),
        "db migrate must not overwrite a live-edited ota_sources status"
    );
    assert_eq!(
        db_exec(&format!(
            "SELECT count(*) AS n FROM ota_source_regions \
             WHERE source_id='besttour' AND region='{test_region}'"
        ))
        .and_then(|r| r.scalar())
        .as_deref(),
        Some("1"),
        "db migrate must not delete and reinsert child region rows"
    );
}
