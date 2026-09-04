//! domestic_accommodations CRUD integration test (real Turso, Guard panic-safe).
//!
//! Covers the slug-keyed reference-data CLI family (no --plan-id):
//!   add-accommodation → list-accommodations → update-accommodation
//!   (image_url + booking_url) → validate data flags → delete-accommodation.
//!
//! domestic_accommodations rows are NOT plan-keyed, so teardown_plan does NOT
//! cover them — the Guard deletes by the unique per-run hotel name.

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
fn accommodation_crud_roundtrip() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    // Ensures the booking_url column exists (CREATE + back-compat ALTER).
    let (ok, _, stderr) = run(&["db", "migrate"]);
    if is_credless(&stderr) {
        eprintln!("credless on db migrate — skip");
        return;
    }
    assert!(ok, "db migrate failed: {stderr}");

    let n = common::nanos();
    let dest = "jiufen";
    // Keep the name short: the list table truncates hotel_name at 16 chars, so a
    // long unique suffix would break the full-name `contains` assertion below.
    let hotel = format!("ZZ測試旅宿{:06}", n % 1_000_000);
    let room = "測試雙人房";

    // Guard: non-plan-keyed rows MUST be torn down locally (teardown_plan only
    // covers plan_id-column tables). Delete by the unique per-run hotel name.
    let _g = Guard::new({
        let hotel = hotel.clone();
        move || {
            let _ = db_exec_teardown(&format!(
                "DELETE FROM domestic_accommodations WHERE hotel_name = '{}'",
                hotel.replace('\'', "''")
            ));
        }
    });

    // 1. add-accommodation
    let (ok, stdout, stderr) = run(&[
        "add-accommodation",
        "--dest", dest,
        "--hotel", &hotel,
        "--room-type", room,
        "--price", "3900",
        "--sea-view",
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on add-accommodation — skip");
        return;
    }
    assert!(ok, "add-accommodation should succeed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("Added accommodation"), "stdout: {stdout}");
    let id = stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("id: "))
        .map(str::trim)
        .expect("add-accommodation must print the new id")
        .to_string();
    assert!(id.starts_with("jiufen_"), "id should be dest-scoped: {id}");

    // add again → idempotent dedup (exit 0, "already exists", still one row)
    let (ok2, stdout2, stderr2) = run(&[
        "add-accommodation",
        "--dest", dest,
        "--hotel", &hotel,
        "--room-type", room,
        "--price", "3900",
    ]);
    assert!(ok2, "re-add should be a no-op success; stderr={stderr2}");
    assert!(stdout2.contains("already exists"), "stdout2: {stdout2}");

    // 2. list-accommodations shows the row with no image/booking yet
    let (ok, stdout, stderr) = run(&["list-accommodations", "--dest", dest, "--limit", "100"]);
    assert!(ok, "list-accommodations should succeed; stderr={stderr}");
    assert!(stdout.contains(&hotel), "list should include the new row: {stdout}");
    assert!(stdout.contains(&id), "list should include the id: {stdout}");

    // 3. validate data flags the missing image_url (WARN) + booking_url (INFO)
    let (ok, stdout, stderr) = run(&["validate", "data"]);
    assert!(ok, "validate data should exit 0 (warnings only); stderr={stderr}");
    assert!(
        stdout.contains("[domestic-accommodations]") && stdout.contains(&id),
        "validate data should flag the new row missing links: {stdout}"
    );

    // 4. update-accommodation sets both URLs
    let (ok, stdout, stderr) = run(&[
        "update-accommodation",
        "--id", &id,
        "--image-url", "https://example.com/img.webp",
        "--booking-url", "https://agoda.com/example",
    ]);
    assert!(ok, "update-accommodation should succeed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("Updated accommodation"), "stdout: {stdout}");

    let img = scalar(&format!(
        "SELECT image_url AS v FROM domestic_accommodations WHERE id = '{id}'"
    ));
    assert_eq!(img.as_deref(), Some("https://example.com/img.webp"));
    let book = scalar(&format!(
        "SELECT booking_url AS v FROM domestic_accommodations WHERE id = '{id}'"
    ));
    assert_eq!(book.as_deref(), Some("https://agoda.com/example"));

    // 5. update/delete on an unknown id fail loud (exit 1)
    let (ok, _, stderr) = run(&[
        "update-accommodation",
        "--id", "nonexistent_id_zz",
        "--booking-url", "https://x",
    ]);
    assert!(!ok, "update with unknown id must fail");
    assert!(stderr.contains("no domestic_accommodations row"), "stderr: {stderr}");

    let (ok, _, stderr) = run(&["delete-accommodation", "--id", "nonexistent_id_zz"]);
    assert!(!ok, "delete with unknown id must fail");
    assert!(stderr.contains("no domestic_accommodations row"), "stderr: {stderr}");

    // 6. delete-accommodation removes the row
    let (ok, stdout, stderr) = run(&["delete-accommodation", "--id", &id]);
    assert!(ok, "delete-accommodation should succeed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("Deleted accommodation"), "stdout: {stdout}");

    let count = scalar(&format!(
        "SELECT COUNT(*) AS v FROM domestic_accommodations WHERE id = '{id}'"
    ));
    assert_eq!(count.as_deref(), Some("0"), "row must be gone after delete");
}
