mod common;
use common::{bin, db_exec, is_credless, nanos, db_exec_teardown, Guard, seed_plan, teardown_plan};

use std::process::Command;

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Dependency-order teardown for omiyage + seeded ref rows (non-plan-keyed).
fn omi_teardown(slug: &str) {
    let s = slug.replace('\'', "''");
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_omiyage_locations WHERE slug='{s}'; \
         DELETE FROM destination_omiyage_items WHERE slug='{s}'; \
         DELETE FROM destination_pois WHERE slug='{s}'; \
         DELETE FROM destination_config WHERE slug='{s}';"
    ));
}

/// Seed destination_config + one destination_pois seller.
/// Columns verified against scripts/schema.sql — NO invented `source` column.
fn seed_dest_and_poi(slug: &str, poi: &str) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    let s = slug.replace('\'', "''");
    let p = poi.replace('\'', "''");
    db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
           VALUES ('{s}', 'Omi Test', 'Asia/Tokyo', 'JPY', 'taiwan'); \
         INSERT INTO destination_pois \
           (slug, poi_id, title, area, nearest_station, hours, address, source_url, fetched_at, confidence) \
           VALUES ('{s}', '{p}', 'Test Depachika', 'namba', 'Namba', '10:00-20:00', NULL, \
                   'https://example.com/poi', '2026-07-12T00:00:00Z', 'verified');"
    ))
    .is_some()
}

#[test]
fn omiyage_tables_exist_after_migrate() {
    let Some(_) = db_exec("SELECT 1") else {
        eprintln!("credless");
        return;
    };
    let _ = run(&["db", "migrate"]);
    // both tables queryable with the specified columns (empty is fine)
    assert!(
        db_exec(
            "SELECT slug,item_id,name,category,notes,source_url,fetched_at,confidence \
             FROM destination_omiyage_items LIMIT 0"
        )
        .is_some(),
        "destination_omiyage_items must have the 8 columns"
    );
    assert!(
        db_exec(
            "SELECT slug,item_id,poi_id,purchase_note,source_url,fetched_at,confidence \
             FROM destination_omiyage_locations LIMIT 0"
        )
        .is_some(),
        "destination_omiyage_locations must have the 7 columns"
    );
}
