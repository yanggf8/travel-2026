//! domestic_accommodations CRUD integration test (real Turso, Guard panic-safe).
//!
//! Covers the slug-keyed reference-data CLI family (no --plan-id):
//!   add-accommodation → list-accommodations → update-accommodation
//!   (image_url + booking_url) → validate data flags → delete-accommodation,
//!   plus the price_basis (tax-basis) flag family (spec 2026-11-19 §3).
//!
//! domestic_accommodations rows are NOT plan-keyed, so teardown_plan does NOT
//! cover them — the Guard deletes by the unique row id recorded right after each add.

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
    // The main list table truncates hotel_name at 16 chars; the decision-facts
    // block prints it in full, which is what the `contains` check reads.
    let hotel = format!("zz_test_{n}_crud");
    let room = "測試雙人房";

    // Guard: non-plan-keyed rows MUST be torn down locally (teardown_plan only
    // covers plan_id-column tables). Delete by the unique row id.
    let ids: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let _g = Guard::new({
        let ids = ids.clone();
        move || {
            for id in ids.lock().unwrap().iter() {
                let _ = db_exec_teardown(&format!(
                    "DELETE FROM domestic_accommodations WHERE id = '{id}'"
                ));
            }
        }
    });

    // 1. add-accommodation — with the full decision-fact set, so a scraped
    //    candidate is ONE mutation: price+read date, size, cancel deadline, stock.
    let (ok, stdout, stderr) = run(&[
        "add-accommodation",
        "--dest", dest,
        "--hotel", &hotel,
        "--room-type", room,
        "--price", "3900",
        "--sea-view",
        "--room-size", "42",
        "--rooms-left", "2",
        "--free-cancel-until", "2026-10-05",
        "--price-source", "Booking.com",
        "--bathtub", "no",
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
    ids.lock().unwrap().push(id.clone());
    assert!(id.starts_with("jiufen_"), "id should be dest-scoped: {id}");

    // The add-time facts must land in the row (no follow-up update needed).
    let facts = scalar(&format!(
        "SELECT room_size_sqm || '/' || rooms_left || '/' || free_cancel_until || '/' || price_source \
           || '/' || length(COALESCE(price_checked_at,'')) || '/' || COALESCE(has_bathtub, 'null') AS v \
         FROM domestic_accommodations WHERE id = '{id}'"
    ));
    assert_eq!(facts.as_deref(), Some("42/2/2026-10-05/Booking.com/10/0"), "add-time decision facts: {facts:?}");

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

    // 4b. bathtub is tri-state on update: --bathtub yes flips the verified fact.
    let (ok, stdout, stderr) = run(&["update-accommodation", "--id", &id, "--bathtub", "yes"]);
    assert!(ok, "update-accommodation --bathtub should succeed; stdout={stdout} stderr={stderr}");
    let tub = scalar(&format!(
        "SELECT has_bathtub AS v FROM domestic_accommodations WHERE id = '{id}'"
    ));
    assert_eq!(tub.as_deref(), Some("1"), "verified bathtub flips to 1: {tub:?}");

    // 4c. --lat/--lon write the minimap coordinates (pair-only; clear via `clear`).
    let (ok, stdout, stderr) = run(&[
        "update-accommodation", "--id", &id, "--lat", "25.1330327", "--lon", "121.807191",
    ]);
    assert!(ok, "update-accommodation --lat/--lon should succeed; stdout={stdout} stderr={stderr}");
    let ll = scalar(&format!(
        "SELECT latitude || ',' || longitude AS v FROM domestic_accommodations WHERE id = '{id}'"
    ));
    assert!(
        ll.as_deref().unwrap_or_default().starts_with("25.1330327,121.807191"),
        "coords persisted: {ll:?}"
    );
    // Half a pair fails loud — a minimap centered on one axis alone is wrong data.
    let (ok, _, stderr) = run(&["update-accommodation", "--id", &id, "--lat", "25.1"]);
    assert!(!ok, "--lat without --lon must fail");
    assert!(stderr.contains("together"), "stderr: {stderr}");
    // clear returns both columns to NULL.
    let (ok, _, stderr) = run(&[
        "update-accommodation", "--id", &id, "--lat", "clear", "--lon", "clear",
    ]);
    assert!(ok, "coord clear should succeed; stderr={stderr}");
    let nulled = scalar(&format!(
        "SELECT (latitude IS NULL) || ',' || (longitude IS NULL) AS v FROM domestic_accommodations WHERE id = '{id}'"
    ));
    assert_eq!(nulled.as_deref(), Some("1,1"), "coords cleared to NULL: {nulled:?}");

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

/// price_basis (tax basis) flag family, spec 2026-11-19 §3: add/update/clear,
/// price_checked_at is NOT touched by a basis-only update, a real source change
/// clears a recorded basis (with the note) while a same-value re-set does not,
/// and `validate data` flags a dirty basis value written outside the CLI.
#[test]
fn accommodation_price_basis_roundtrip() {
    // No `db migrate` here: the integrator owns migrations. The column must
    // already exist; fail with a clear message instead of migrating Turso.
    let Some(col) = db_exec(
        "SELECT COUNT(*) AS v FROM pragma_table_info('domestic_accommodations') WHERE name = 'price_basis'",
    ) else {
        eprintln!("credless - skip");
        return;
    };
    assert_eq!(
        col.scalar().as_deref(),
        Some("1"),
        "domestic_accommodations.price_basis is missing - run `./bin/travel db migrate` (integrator step) before this test"
    );

    let n = common::nanos();
    let dest = "jiufen";
    let hotel = format!("zz_test_{n}_basis");
    let room = "測試雙人房";

    // Guard: non-plan-keyed rows are torn down locally, by unique row id.
    let ids: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let _g = Guard::new({
        let ids = ids.clone();
        move || {
            for id in ids.lock().unwrap().iter() {
                let _ = db_exec_teardown(&format!(
                    "DELETE FROM domestic_accommodations WHERE id = '{id}'"
                ));
            }
        }
    });

    // 1. add with --price-source but NO --price-basis → tip on stdout, column
    //    stays NULL (未確認 — the pre-existing-rows case: no default data).
    let (ok, stdout, stderr) = run(&[
        "add-accommodation",
        "--dest", dest,
        "--hotel", &hotel,
        "--room-type", room,
        "--price", "3900",
        "--price-source", "Booking.com",
    ]);
    if is_credless(&stderr) {
        eprintln!("credless on add-accommodation — skip");
        return;
    }
    assert!(ok, "add-accommodation should succeed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("tip: --price-basis incl|excl"), "tip expected: {stdout}");
    let id = stdout
        .lines()
        .find_map(|l| l.trim().strip_prefix("id: "))
        .map(str::trim)
        .expect("add-accommodation must print the new id")
        .to_string();
    ids.lock().unwrap().push(id.clone());
    assert_eq!(
        scalar(&format!(
            "SELECT (price_basis IS NULL) AS v FROM domestic_accommodations WHERE id = '{id}'"
        ))
        .as_deref(),
        Some("1"),
        "no --price-basis → NULL"
    );
    let checked_before = scalar(&format!(
        "SELECT price_checked_at AS v FROM domestic_accommodations WHERE id = '{id}'"
    ))
    .expect("price_checked_at stamped at add time");

    // 2. update --price-basis incl → stored; price_checked_at UNTOUCHED (a
    //    basis change is not a re-quote).
    let (ok, stdout, stderr) = run(&["update-accommodation", "--id", &id, "--price-basis", "incl"]);
    assert!(ok, "update --price-basis should succeed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("price_basis: incl"), "stdout: {stdout}");
    assert_eq!(
        scalar(&format!(
            "SELECT price_basis AS v FROM domestic_accommodations WHERE id = '{id}'"
        ))
        .as_deref(),
        Some("incl")
    );
    assert_eq!(
        scalar(&format!(
            "SELECT price_checked_at AS v FROM domestic_accommodations WHERE id = '{id}'"
        )),
        Some(checked_before),
        "basis-only update must not re-stamp the read date"
    );

    // 3. --price-basis clear → back to NULL.
    let (ok, _, stderr) = run(&["update-accommodation", "--id", &id, "--price-basis", "clear"]);
    assert!(ok, "clear should succeed; stderr={stderr}");
    assert_eq!(
        scalar(&format!(
            "SELECT (price_basis IS NULL) AS v FROM domestic_accommodations WHERE id = '{id}'"
        ))
        .as_deref(),
        Some("1")
    );

    // 4. REAL source change without a basis flag → recorded basis is cleared
    //    (old basis belongs to the old source) and the note says so.
    let (ok, _, stderr) = run(&["update-accommodation", "--id", &id, "--price-basis", "incl"]);
    assert!(ok, "re-set basis to incl should succeed; stderr={stderr}");
    let (ok, stdout, stderr) = run(&["update-accommodation", "--id", &id, "--price-source", "Agoda"]);
    assert!(ok, "source-only update should succeed; stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("note: price_basis cleared (price source changed)"),
        "note expected: {stdout}"
    );
    assert_eq!(
        scalar(&format!(
            "SELECT (price_basis IS NULL) AS v FROM domestic_accommodations WHERE id = '{id}'"
        ))
        .as_deref(),
        Some("1"),
        "source change clears the recorded basis"
    );

    // 5. Same-value source re-set → basis kept, no note.
    let (ok, _, stderr) = run(&["update-accommodation", "--id", &id, "--price-basis", "excl"]);
    assert!(ok, "re-set basis to excl should succeed; stderr={stderr}");
    let (ok, stdout, stderr) = run(&["update-accommodation", "--id", &id, "--price-source", "Agoda"]);
    assert!(ok, "same-source re-set should succeed; stderr={stderr}");
    assert!(!stdout.contains("price_basis cleared"), "no note on same-source: {stdout}");
    assert_eq!(
        scalar(&format!(
            "SELECT price_basis AS v FROM domestic_accommodations WHERE id = '{id}'"
        ))
        .as_deref(),
        Some("excl"),
        "same-source re-set keeps the basis"
    );

    // 6. Source + basis together → both land.
    let (ok, stdout, stderr) = run(&[
        "update-accommodation", "--id", &id, "--price-source", "Booking.com", "--price-basis", "incl",
    ]);
    assert!(ok, "source+basis should succeed; stdout={stdout} stderr={stderr}");
    assert_eq!(
        scalar(&format!(
            "SELECT price_source || '/' || price_basis AS v FROM domestic_accommodations WHERE id = '{id}'"
        ))
        .as_deref(),
        Some("Booking.com/incl")
    );

    // 7. Junk flag value fails loud, writes nothing.
    let (ok, _, stderr) = run(&["update-accommodation", "--id", &id, "--price-basis", "taxable"]);
    assert!(!ok, "junk --price-basis must fail");
    assert!(stderr.contains("incl or excl"), "stderr: {stderr}");
    assert_eq!(
        scalar(&format!(
            "SELECT price_basis AS v FROM domestic_accommodations WHERE id = '{id}'"
        ))
        .as_deref(),
        Some("incl"),
        "failed update writes nothing"
    );

    // 8. Dirty value written OUTSIDE the CLI (raw db exec) → validate data WARN
    //    naming the id and the value (the column has no CHECK — this lint is
    //    the net under it).
    db_exec(&format!(
        "UPDATE domestic_accommodations SET price_basis = '含稅價' WHERE id = '{id}'"
    ))
    .expect("seed dirty basis");
    let (ok, stdout, stderr) = run(&["validate", "data"]);
    assert!(ok, "dirty basis is a warning, not a failure; stderr={stderr}");
    let hit = stdout.lines().any(|l| {
        l.contains("[domestic-accommodations]") && l.contains(&id) && l.contains("not incl or excl")
    });
    assert!(hit, "validate data must WARN on the dirty basis; got:\n{}", stdout);

    // 9. list-accommodations shows the basis in the decision-facts line.
    let (ok, stdout, stderr) = run(&["list-accommodations", "--dest", dest, "--limit", "100"]);
    assert!(ok, "list should succeed; stderr={stderr}");
    let line = stdout
        .lines()
        .find(|l| l.starts_with(&format!("  {hotel}")))
        .expect("decision-facts line for the fixture row");
    assert!(line.contains("price_basis="), "line: {line}");
}
