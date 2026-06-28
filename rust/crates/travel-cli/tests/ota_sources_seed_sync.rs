//! Regression: `db migrate` must SYNC the committed OTA_SOURCES seed metadata
//! (status/scraper_script/url_template/notes) onto existing live `ota_sources` rows,
//! not just create missing ones. The original `seed_ota_sources` used INSERT OR IGNORE,
//! so once a row existed its notes/status drifted forever from the committed seed — every
//! live row kept its pre-sweep notes while the seed array carried the authoritative
//! "PROVEN REAL"/"DEFERRED" decisions. Turso is the source of truth, so the live rows
//! must reflect the committed seed after a migrate.
//!
//! Real-Turso integration test; skips cleanly if creds absent.

use std::process::Command;
use std::sync::Mutex;

static SEED_LOCK: Mutex<()> = Mutex::new(());

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn is_credless(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
}

fn run(args: &[&str]) -> (bool, String, String) {
    let out = bin().args(args).output().unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Single-cell scalar from `db exec` ("col: value" lines).
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
    stdout.lines().find_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
}

#[tokio::test]
async fn db_migrate_syncs_ota_source_notes() {
    let _guard = SEED_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    // Skip cleanly if no creds.
    if cell("SELECT COUNT(*) AS n FROM ota_sources WHERE source_id='besttour'").is_none() {
        return;
    }

    // Corrupt the live besttour note (simulate a stale pre-sweep row).
    let (ok, _o, e) = run(&[
        "db",
        "exec",
        "UPDATE ota_sources SET notes='STALE_TEST_SENTINEL' WHERE source_id='besttour'",
    ]);
    assert!(ok, "setup UPDATE should succeed; err={e}");
    assert_eq!(
        cell("SELECT notes FROM ota_sources WHERE source_id='besttour'").as_deref(),
        Some("STALE_TEST_SENTINEL"),
        "sentinel set"
    );

    // Run migrate — must resync the seed metadata onto the existing row.
    let (ok, stdout, stderr) = run(&["db", "migrate"]);
    assert!(ok, "db migrate should succeed; stdout={stdout} stderr={stderr}");

    // The committed besttour seed note begins with "PROVEN REAL"; the stale sentinel must
    // be gone (proving the upsert updated the existing row, not just ignored it).
    let notes = cell("SELECT notes FROM ota_sources WHERE source_id='besttour'");
    assert_ne!(
        notes.as_deref(),
        Some("STALE_TEST_SENTINEL"),
        "db migrate must overwrite a stale ota_sources note with the committed seed (was INSERT OR IGNORE)"
    );
    assert!(
        notes.as_deref().map(|n| n.contains("PROVEN REAL")).unwrap_or(false),
        "besttour note should be resynced to the committed 'PROVEN REAL' seed; got {notes:?}"
    );
}
