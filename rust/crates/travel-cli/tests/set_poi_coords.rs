//! Integration tests for `travel set-poi-coords` — sets geocoded lat/lon on a
//! `destination_pois` row (map pins for the dashboard).
//!
//! `destination_pois` is slug-keyed GLOBAL reference data, NOT plan-keyed, so
//! teardown here is LOCAL by slug via `db_exec_teardown` — NOT `teardown_plan`
//! (there is no plan_id involved in this command at all).
//!
//! Invariants exercised:
//!   1. Setting coords on a seeded (slug, poi_id) persists lat/lon/source_url/
//!      confidence (a SELECT confirms).
//!   2. A missing (slug, poi_id) fails loud (non-zero) and writes no update.
//!   3. `validate data` WARNS on the ungeocoded slug BEFORE coords are set, and
//!      that warning clears for the slug AFTER coords are set (global validate
//!      exit code is NOT asserted — other slugs/data may still warn/error).

mod common;
use common::{bin, db_exec, db_exec_teardown, nanos, Guard};

use std::process::Command;

/// Seed a destination_config row + one destination_pois row with NULL lat/lon.
/// Returns false on a credless skip.
fn seed(slug: &str) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    let sql = format!(
        "INSERT INTO destination_config (slug, display_name, ref_id, ref_path, timezone, currency, language, origin) \
           VALUES ('{slug}', 'ZZ Coord Test', 'zz-ref', 'turso:destination-ref/zz', 'Asia/Tokyo', 'JPY', 'ja', 'taiwan'); \
         INSERT INTO destination_pois (slug, poi_id, title, source_url, fetched_at, confidence, lat, lon) \
           VALUES ('{slug}', 'poi_one', 'POI One', 'test', '2026-07-05', 'test', NULL, NULL);"
    );
    db_exec(&sql).is_some()
}

fn teardown(slug: &str) {
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_pois WHERE slug = '{slug}'; \
         DELETE FROM destination_config WHERE slug = '{slug}';"
    ));
}

/// Run a `travel` subcommand. Returns (ok, stdout, stderr).
fn run_cmd(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// ── Sets lat/lon/source_url/confidence on a seeded POI ───────────────────────
#[test]
fn set_poi_coords_persists_lat_lon_source_confidence() {
    let tag = nanos();
    let slug = format!("zzcoord_{tag}");
    let _g = Guard::new({
        let slug = slug.clone();
        move || teardown(&slug)
    });
    if !seed(&slug) {
        return;
    }

    let (ok, stdout, stderr) = run_cmd(&[
        "set-poi-coords",
        &slug,
        "poi_one",
        "35.6812",
        "139.7671",
        "--source",
        "test-provider",
        "--confidence",
        "verified",
    ]);

    let row = db_exec(&format!(
        "SELECT lat || '|' || lon || '|' || source_url || '|' || confidence AS row_text \
         FROM destination_pois WHERE slug = '{slug}' AND poi_id = 'poi_one'"
    ))
    .and_then(|rows| rows.scalar());

    assert!(ok, "set-poi-coords should succeed; stdout={stdout} stderr={stderr}");
    let row_text = row.expect("row_text should be present");
    assert_eq!(
        row_text, "35.6812|139.7671|test-provider|verified",
        "lat/lon/source_url/confidence must all be persisted"
    );
}

// ── Missing (slug, poi_id) → fail loud, no update ────────────────────────────
#[test]
fn set_poi_coords_fails_loud_on_missing_row() {
    let tag = nanos();
    let slug = format!("zzcoordmiss_{tag}");
    let _g = Guard::new({
        let slug = slug.clone();
        move || teardown(&slug)
    });
    if !seed(&slug) {
        return;
    }

    let (ok, _stdout, stderr) = run_cmd(&[
        "set-poi-coords",
        &slug,
        "no_such_poi",
        "35.6812",
        "139.7671",
    ]);

    let row = db_exec(&format!(
        "SELECT lat FROM destination_pois WHERE slug = '{slug}' AND poi_id = 'no_such_poi'"
    ))
    .map(|rows| rows.raw().to_string());

    assert!(!ok, "missing (slug, poi_id) must exit non-zero; stderr={stderr}");
    assert!(
        row.unwrap_or_default().trim().is_empty(),
        "no row should exist for the unknown poi_id (no update happened)"
    );

    // The originally-seeded poi_one row must remain untouched (still NULL lat/lon).
    // NULL || '|' || NULL is NULL in SQLite, so use COALESCE to observe each column:
    // both NULL → 'NULL|NULL'.
    let seeded_row = db_exec(&format!(
        "SELECT COALESCE(CAST(lat AS TEXT), 'NULL') || '|' || COALESCE(CAST(lon AS TEXT), 'NULL') AS row_text \
         FROM destination_pois WHERE slug = '{slug}' AND poi_id = 'poi_one'"
    ))
    .and_then(|rows| rows.scalar());
    assert_eq!(
        seeded_row.as_deref(),
        Some("NULL|NULL"),
        "the seeded poi_one row must remain NULL/NULL (untouched by the failed call)"
    );
}

// ── validate data warns on the ungeocoded slug, then clears once coords set ──
#[test]
fn validate_data_warns_on_ungeocoded_poi_then_clears() {
    let tag = nanos();
    let slug = format!("zzcoordval_{tag}");
    let _g = Guard::new({
        let slug = slug.clone();
        move || teardown(&slug)
    });
    if !seed(&slug) {
        return;
    }

    // BEFORE coords: validate data stdout must warn about this slug.
    let before = Command::new(bin())
        .args(["validate", "data"])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run validate data (before)");
    let before_stdout = String::from_utf8_lossy(&before.stdout).into_owned();

    assert!(
        before_stdout.contains(&slug) && before_stdout.contains("missing lat/lon"),
        "validate data must warn about ungeocoded slug {slug} before coords are set; stdout=\n{before_stdout}"
    );

    // Set coords.
    let (ok, stdout, stderr) = run_cmd(&[
        "set-poi-coords",
        &slug,
        "poi_one",
        "35.6812",
        "139.7671",
    ]);
    assert!(ok, "set-poi-coords should succeed; stdout={stdout} stderr={stderr}");

    // AFTER coords: the warning for this slug must clear (global exit code not asserted).
    let after = Command::new(bin())
        .args(["validate", "data"])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run validate data (after)");
    let after_stdout = String::from_utf8_lossy(&after.stdout).into_owned();

    let slug_warning_line = format!("{slug}: ");
    assert!(
        !after_stdout
            .lines()
            .any(|l| l.contains(&slug_warning_line) && l.contains("missing lat/lon")),
        "validate data must NOT warn about slug {slug} after coords are set; stdout=\n{after_stdout}"
    );
}
