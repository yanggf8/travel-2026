//! domestic_accommodation_ratings integration test (real Turso, Guard panic-safe).
//!
//! Covers `set-accommodation-rating`: per-source upsert with the right default
//! scale, re-running a source overwriting rather than duplicating, --clear, and
//! the fail-loud paths (unknown accommodation, clearing a rating that isn't there).
//!
//! Neither table is plan-keyed, so teardown_plan does NOT cover them — the Guard
//! deletes the ratings first, then the stay, by the unique per-run hotel name.

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, Guard};
use std::process::Command;

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin()).args(args).output().expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn scalar(sql: &str) -> Option<String> {
    db_exec(sql).and_then(|r| r.scalar())
}

#[test]
fn accommodation_rating_roundtrip() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    let (ok, _, stderr) = run(&["db", "migrate"]);
    if is_credless(&stderr) {
        eprintln!("credless on db migrate — skip");
        return;
    }
    assert!(ok, "db migrate failed: {stderr}");

    let n = common::nanos();
    let hotel = format!("ZZ評分測試{:06}", n % 1_000_000);

    let _g = Guard::new({
        let hotel = hotel.clone();
        move || {
            let esc = hotel.replace('\'', "''");
            let _ = db_exec_teardown(&format!(
                "DELETE FROM domestic_accommodation_ratings WHERE accommodation_id IN \
                 (SELECT id FROM domestic_accommodations WHERE hotel_name = '{esc}')"
            ));
            let _ = db_exec_teardown(&format!(
                "DELETE FROM domestic_accommodations WHERE hotel_name = '{esc}'"
            ));
        }
    });

    let (ok, stdout, stderr) = run(&[
        "add-accommodation",
        "--dest", "jiufen",
        "--hotel", &hotel,
        "--room-type", "測試海景房",
        "--price", "4400",
        "--sea-view",
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on add-accommodation — skip");
        return;
    }
    assert!(ok, "add-accommodation failed; stdout={stdout} stderr={stderr}");
    let id = stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("id: "))
        .map(str::trim)
        .expect("add-accommodation must print the new id")
        .to_string();

    // 1. unknown accommodation id fails loud — no orphan rating rows.
    let (ok, _, stderr) = run(&[
        "set-accommodation-rating", "--id", "nonexistent_id_zz",
        "--source", "Booking.com", "--score", "9.0",
    ]);
    assert!(!ok, "unknown id must fail");
    assert!(stderr.contains("no domestic_accommodations row"), "stderr: {stderr}");

    // 2. booking defaults to /10, google to /5 — two sources coexist.
    let (ok, stdout, stderr) = run(&[
        "set-accommodation-rating", "--id", &id,
        "--source", "Booking.com", "--score", "9.0", "--reviews", "266",
    ]);
    assert!(ok, "booking rating failed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("Booking.com 9/10"), "stdout: {stdout}");

    let (ok, stdout, _) = run(&[
        "set-accommodation-rating", "--id", &id,
        "--source", "Google", "--score", "4.6", "--reviews", "119",
    ]);
    assert!(ok, "google rating failed");
    assert!(stdout.contains("Google 4.6/5"), "stdout: {stdout}");

    let count = scalar(&format!(
        "SELECT COUNT(*) AS v FROM domestic_accommodation_ratings WHERE accommodation_id = '{id}'"
    ));
    assert_eq!(count.as_deref(), Some("2"), "one row per source");

    let scale = scalar(&format!(
        "SELECT scale AS v FROM domestic_accommodation_ratings WHERE accommodation_id = '{id}' AND source = 'Google'"
    ));
    assert!(
        scale.as_deref().is_some_and(|s| s.starts_with('5')),
        "google scale must default to 5, got {scale:?}"
    );

    // 3. re-running a source OVERWRITES it (upsert), never duplicates.
    let (ok, _, stderr) = run(&[
        "set-accommodation-rating", "--id", &id,
        "--source", "Booking.com", "--score", "9.2", "--reviews", "300",
    ]);
    assert!(ok, "re-rating failed: {stderr}");
    let count = scalar(&format!(
        "SELECT COUNT(*) AS v FROM domestic_accommodation_ratings WHERE accommodation_id = '{id}'"
    ));
    assert_eq!(count.as_deref(), Some("2"), "upsert must not add a row");
    let score = scalar(&format!(
        "SELECT score AS v FROM domestic_accommodation_ratings WHERE accommodation_id = '{id}' AND source = 'Booking.com'"
    ));
    assert!(
        score.as_deref().is_some_and(|s| s.starts_with("9.2")),
        "score must be overwritten, got {score:?}"
    );

    // 4. a score above its scale is rejected before it reaches the DB.
    let (ok, _, stderr) = run(&[
        "set-accommodation-rating", "--id", &id,
        "--source", "Google", "--score", "9.0",
    ]);
    assert!(!ok, "9.0 on a /5 scale must fail");
    assert!(stderr.contains("cannot exceed"), "stderr: {stderr}");

    // 5. --clear removes one source; clearing it again fails loud.
    let (ok, stdout, stderr) = run(&[
        "set-accommodation-rating", "--id", &id, "--source", "Google", "--clear",
    ]);
    assert!(ok, "clear failed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("Cleared Google rating"), "stdout: {stdout}");

    let count = scalar(&format!(
        "SELECT COUNT(*) AS v FROM domestic_accommodation_ratings WHERE accommodation_id = '{id}'"
    ));
    assert_eq!(count.as_deref(), Some("1"), "only the cleared source is gone");

    let (ok, _, stderr) = run(&[
        "set-accommodation-rating", "--id", &id, "--source", "Google", "--clear",
    ]);
    assert!(!ok, "clearing a missing rating must fail loud");
    assert!(stderr.contains("no 'Google' rating"), "stderr: {stderr}");

    // 6. update-accommodation --price-source stamps a read date automatically.
    let (ok, stdout, stderr) = run(&[
        "update-accommodation", "--id", &id, "--price-source", "Booking.com",
    ]);
    assert!(ok, "price-source update failed; stdout={stdout} stderr={stderr}");
    let checked = scalar(&format!(
        "SELECT price_checked_at AS v FROM domestic_accommodations WHERE id = '{id}'"
    ));
    assert!(
        checked.as_deref().is_some_and(|s| s.len() >= 10 && s.contains('-')),
        "price_checked_at must be stamped, got {checked:?}"
    );
}
