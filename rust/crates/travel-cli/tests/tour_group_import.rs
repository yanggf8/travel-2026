//! Port of tests/integration/tour-group-import.regression.test.ts.
//!
//! Drives `travel import-tour-group-offers --run <id> --file <fixture>` against
//! real Turso and asserts the imported shaping_tour_group_offers rows + the
//! scrape-attempt status (ok / partial / reject-on-identity-mismatch).
//!
//! The fixtures hardcode run_id test-run-tg-001/002/003, so this suite uses
//! those exact ids (serialized by a process mutex) and cleans them up.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

static IMPORT_LOCK: Mutex<()> = Mutex::new(());

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_travel")
}

fn repo_root() -> PathBuf {
    // tests run with CWD = crate dir (rust/crates/travel-cli); go up to repo root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("resolve repo root")
}

fn fixture(name: &str) -> String {
    repo_root()
        .join("tests/fixtures/tour-group")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// Run `db exec <sql>`; returns (success, stdout, stderr). Used for seed/assert/teardown.
fn db_exec(sql: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["db", "exec", sql])
        .output()
        .expect("run db exec");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// True if the error looks like missing Turso creds/connection — skip rather than fail.
fn is_skip(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("failed to connect to Turso")
}

fn clear_run(run_id: &str) {
    let _ = db_exec(&format!(
        "DELETE FROM shaping_tour_group_offers WHERE run_id = '{run_id}'"
    ));
    let _ = db_exec(&format!(
        "DELETE FROM shaping_tour_group_scrape_attempts WHERE run_id = '{run_id}'"
    ));
}

fn seed_pending_attempt(run_id: &str, source_id: &str, dest_region: &str, nights: i64) {
    db_exec(&format!(
        "INSERT OR REPLACE INTO shaping_tour_group_scrape_attempts \
         (run_id, source_id, dest_region, nights, status) \
         VALUES ('{run_id}', '{source_id}', '{dest_region}', {nights}, 'pending')"
    ));
}

fn import(run_id: &str, fixture_name: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args([
            "import-tour-group-offers",
            "--run",
            run_id,
            "--file",
            &fixture(fixture_name),
        ])
        .output()
        .expect("run import-tour-group-offers");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// SELECT a single scalar cell from a `db exec` "col: value" line.
fn scalar(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
}

#[tokio::test]
async fn import_happy_partial_reject() {
    let _guard = IMPORT_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    // Creds probe via a harmless read; skip cleanly if absent.
    let (ok, _o, err) = db_exec("SELECT 1");
    if !ok && is_skip(&err) {
        eprintln!("skipping tour-group-import test (no Turso creds): {}", err.trim());
        return;
    }

    // ── happy path: all rows imported, attempt status=ok ──
    clear_run("test-run-tg-001");
    seed_pending_attempt("test-run-tg-001", "besttour", "kansai", 5);
    let (ok, _out, err) = import("test-run-tg-001", "besttour-kansai-5n-ok.json");
    assert!(ok, "happy-path import should exit 0; stderr={err}");

    let (_, n, _) = db_exec(
        "SELECT COUNT(*) FROM shaping_tour_group_offers WHERE run_id = 'test-run-tg-001'",
    );
    assert_eq!(scalar(&n).as_deref(), Some("2"), "happy path imports 2 offers");
    let (_, cheapest, _) = db_exec(
        "SELECT price_per_person_twd FROM shaping_tour_group_offers \
         WHERE run_id = 'test-run-tg-001' ORDER BY price_per_person_twd LIMIT 1",
    );
    assert_eq!(scalar(&cheapest).as_deref(), Some("32500"), "cheapest = 32500");
    let (_, att, _) = db_exec(
        "SELECT status FROM shaping_tour_group_scrape_attempts WHERE run_id = 'test-run-tg-001'",
    );
    assert_eq!(scalar(&att).as_deref(), Some("ok"), "attempt status = ok");
    clear_run("test-run-tg-001");

    // ── partial: one row skipped for missing required field ──
    clear_run("test-run-tg-002");
    seed_pending_attempt("test-run-tg-002", "besttour", "kansai", 5);
    let (ok, _o, err) = import("test-run-tg-002", "besttour-kansai-5n-partial.json");
    assert!(ok, "partial import should exit 0; stderr={err}");
    let (_, n, _) = db_exec(
        "SELECT COUNT(*) FROM shaping_tour_group_offers WHERE run_id = 'test-run-tg-002'",
    );
    assert_eq!(scalar(&n).as_deref(), Some("1"), "partial imports 1 offer");
    let (_, att, _) = db_exec(
        "SELECT status FROM shaping_tour_group_scrape_attempts WHERE run_id = 'test-run-tg-002'",
    );
    assert_eq!(scalar(&att).as_deref(), Some("partial"), "attempt status = partial");
    clear_run("test-run-tg-002");

    // ── reject: attempt-identity mismatch (file declares a different source_id) ──
    clear_run("test-run-tg-003");
    seed_pending_attempt("test-run-tg-003", "besttour", "kansai", 5);
    let (ok, _o, err) = import("test-run-tg-003", "besttour-kansai-5n-mismatch.json");
    assert!(!ok, "identity-mismatch import should fail (non-zero exit)");
    let lc = err.to_lowercase();
    assert!(
        lc.contains("no pending attempt")
            || lc.contains("attempt not found")
            || lc.contains("does not match"),
        "expected identity-mismatch error; got stderr={err}"
    );
    let (_, n, _) = db_exec(
        "SELECT COUNT(*) FROM shaping_tour_group_offers WHERE run_id = 'test-run-tg-003'",
    );
    assert_eq!(scalar(&n).as_deref(), Some("0"), "rejected import writes 0 offers");
    clear_run("test-run-tg-003");
}
