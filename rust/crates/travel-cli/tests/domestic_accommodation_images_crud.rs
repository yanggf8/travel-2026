//! domestic_accommodation_images CRUD integration test (real Turso, Guard panic-safe).
//!
//! Covers the candidate-gallery CLI family (slug-keyed reference data, no --plan-id):
//!   add-accommodation → add-accommodation-image (append + explicit --sort + dedup)
//!   → list-accommodation-images (--id and --dest, per-hotel counts)
//!   → validate data gallery-depth INFO → delete-accommodation-image (--url and --all).
//!
//! Neither table is plan-keyed, so teardown_plan does NOT cover them — the Guard
//! deletes the gallery rows first, then the stay, by the unique per-run hotel name.

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
fn accommodation_gallery_crud_roundtrip() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless — skip");
        return;
    };
    // Ensures domestic_accommodation_images exists.
    let (ok, _, stderr) = run(&["db", "migrate"]);
    if is_credless(&stderr) {
        eprintln!("credless on db migrate — skip");
        return;
    }
    assert!(ok, "db migrate failed: {stderr}");

    let n = common::nanos();
    let dest = "jiufen";
    let hotel = format!("ZZ相簿測試{:06}", n % 1_000_000);

    // Guard: children first (FK), then the parent stay.
    let _g = Guard::new({
        let hotel = hotel.clone();
        move || {
            let esc = hotel.replace('\'', "''");
            let _ = db_exec_teardown(&format!(
                "DELETE FROM domestic_accommodation_images WHERE accommodation_id IN \
                 (SELECT id FROM domestic_accommodations WHERE hotel_name = '{esc}')"
            ));
            let _ = db_exec_teardown(&format!(
                "DELETE FROM domestic_accommodations WHERE hotel_name = '{esc}'"
            ));
        }
    });

    let (ok, stdout, stderr) = run(&[
        "add-accommodation",
        "--dest", dest,
        "--hotel", &hotel,
        "--room-type", "測試海景房",
        "--price", "4100",
        "--sea-view",
        "--image-url", "https://example.com/hero.webp",
        "--booking-url", "https://example.com/book",
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

    // 1. an unknown accommodation id fails loud — never a silent orphan row.
    let (ok, _, stderr) = run(&[
        "add-accommodation-image",
        "--id", "nonexistent_id_zz",
        "--url", "https://example.com/x.webp",
    ]);
    assert!(!ok, "add-accommodation-image with unknown id must fail");
    assert!(stderr.contains("no domestic_accommodations row"), "stderr: {stderr}");

    // 2. two appended photos get sort_order 1 then 2.
    for (url, label, want_sort) in [
        ("https://example.com/g1.webp", "海景雙人房", "1"),
        ("https://example.com/g2.webp", "公區", "2"),
    ] {
        let (ok, stdout, stderr) =
            run(&["add-accommodation-image", "--id", &id, "--url", url, "--label", label]);
        assert!(ok, "add-accommodation-image failed; stdout={stdout} stderr={stderr}");
        assert!(stdout.contains("Added gallery photo"), "stdout: {stdout}");
        assert!(stdout.contains(&format!("sort:  {want_sort}")), "sort should append: {stdout}");
    }

    // 3. re-adding the same url is a dedup no-op (PK), not a duplicate row.
    let (ok, stdout, _) = run(&[
        "add-accommodation-image", "--id", &id, "--url", "https://example.com/g1.webp",
    ]);
    assert!(ok, "re-add should be a no-op success");
    assert!(stdout.contains("already exists"), "stdout: {stdout}");

    // 4. an explicit --sort is honoured.
    let (ok, stdout, stderr) = run(&[
        "add-accommodation-image", "--id", &id,
        "--url", "https://example.com/g0.webp", "--label", "封面", "--sort", "0",
    ]);
    assert!(ok, "explicit --sort failed; stderr={stderr}");
    assert!(stdout.contains("sort:  0"), "stdout: {stdout}");

    let count = scalar(&format!(
        "SELECT COUNT(*) AS v FROM domestic_accommodation_images WHERE accommodation_id = '{id}'"
    ));
    assert_eq!(count.as_deref(), Some("3"), "3 distinct photos after the dedup");

    // 5. list --id renders in sort_order; list --dest carries per-hotel counts.
    let (ok, stdout, stderr) = run(&["list-accommodation-images", "--id", &id]);
    assert!(ok, "list --id failed; stderr={stderr}");
    assert!(stdout.contains("3 photo(s)"), "stdout: {stdout}");
    let p0 = stdout.find("g0.webp").expect("g0 listed");
    let p1 = stdout.find("g1.webp").expect("g1 listed");
    assert!(p0 < p1, "sort 0 must render before sort 1: {stdout}");

    let (ok, stdout, stderr) = run(&["list-accommodation-images", "--dest", dest]);
    assert!(ok, "list --dest failed; stderr={stderr}");
    assert!(stdout.contains(&format!("{hotel}: 3 photo(s)")), "per-hotel count: {stdout}");

    // 6. validate data: 3 photos meets the gallery-depth floor, so this row is
    //    NOT flagged (it would be at 2 — see the delete below).
    let (ok, stdout, stderr) = run(&["validate", "data"]);
    assert!(ok, "validate data should exit 0; stderr={stderr}");
    assert!(
        !stdout.contains(&format!("{id}) has only")),
        "a 3-photo gallery must not be flagged: {stdout}"
    );

    // 7. deleting one photo drops below the floor → validate flags it.
    let (ok, stdout, stderr) = run(&[
        "delete-accommodation-image", "--id", &id, "--url", "https://example.com/g0.webp",
    ]);
    assert!(ok, "delete one photo failed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("Deleted gallery photo"), "stdout: {stdout}");

    let (ok, stdout, _) = run(&["validate", "data"]);
    assert!(ok, "validate data should still exit 0");
    assert!(
        stdout.contains(&format!("{id}) has only 2 gallery photo(s)")),
        "a thin gallery must be flagged: {stdout}"
    );

    // 8. deleting an unknown url fails loud.
    let (ok, _, stderr) = run(&[
        "delete-accommodation-image", "--id", &id, "--url", "https://example.com/nope.webp",
    ]);
    assert!(!ok, "unknown url delete must fail");
    assert!(stderr.contains("no gallery photo"), "stderr: {stderr}");

    // 9. --all clears the gallery; a second --all then fails loud.
    let (ok, stdout, stderr) = run(&["delete-accommodation-image", "--id", &id, "--all"]);
    assert!(ok, "--all delete failed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("Deleted all 2 gallery photo(s)"), "stdout: {stdout}");

    let count = scalar(&format!(
        "SELECT COUNT(*) AS v FROM domestic_accommodation_images WHERE accommodation_id = '{id}'"
    ));
    assert_eq!(count.as_deref(), Some("0"), "gallery must be empty");

    let (ok, _, stderr) = run(&["delete-accommodation-image", "--id", &id, "--all"]);
    assert!(!ok, "clearing an empty gallery must fail loud");
    assert!(stderr.contains("no gallery photos"), "stderr: {stderr}");
}
