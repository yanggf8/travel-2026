//! End-to-end LOCK for the localization pair `set-poi-title` / `set-place-geocode`:
//! 1. A POI title renamed via the CLI (with --segments) also renames the
//!    day_route_segments endpoint labels that equal the OLD title — segment
//!    labels match POIs by exact normalized title keys, so leaving them behind
//!    would silently unpin those stops on the next snapshot.
//! 2. A manual place pin lands under the EXACT route_place_geocodes key
//!    snapshot-maps' resolve_place reads (same normalize_place formula) —
//!    a manual row that misses by construction would be useless.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard};

static LOCK: Mutex<()> = Mutex::new(());

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn run_cmd(args: &[&str]) -> Option<String> {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() && is_credless(&stderr) {
        return None;
    }
    assert!(out.status.success(), "travel {args:?} failed; stdout={stdout} stderr={stderr}");
    Some(stdout)
}

fn one_cell(sql: &str) -> Option<String> {
    db_exec(sql).and_then(|r| r.scalar())
}

#[test]
fn set_poi_title_renames_segments_and_set_place_geocode_keys_match() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let n = nanos();
    let plan = format!("zztest-poititle-{n}");
    let dest = format!("zz_poititle_{n}");

    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || {
            teardown_plan(&plan, &dest);
            let d = sql_lit(&dest);
            let _ = db_exec_teardown(&format!(
                "DELETE FROM destination_pois WHERE slug = {d}; \
                 DELETE FROM destination_config WHERE slug = {d};"
            ));
        }
    });
    teardown_plan(&plan, &dest);
    seed_plan(&plan, &dest, 0);

    let d = sql_lit(&dest);
    let p = sql_lit(&plan);
    let geo_key = format!("zz測試點{n}|Kyoto, Japan");
    if db_exec(&format!(
        "INSERT INTO destination_config (slug, display_name, timezone, currency, origin) \
           VALUES ({d}, 'ZZ Poititle Test', 'Asia/Tokyo', 'JPY', 'taiwan'); \
         INSERT INTO destination_pois (slug, poi_id, title, lat, lon, source_url, fetched_at, confidence) \
           VALUES ({d}, 'poi_x', 'Tenryuji Temple', 35.0143, 135.67, 'test', '2026-09-27', 'test'); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, status, updated_at) \
           VALUES ({p}, {d}, 1, '2026-11-01', 'full', 'draft', '2020-01-01 00:00:00'); \
         INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode, source) \
           VALUES ({p}, {d}, 1, 0, 'Tenryuji Temple', 'Bamboo Stop', 'walking', 'confirmed');"
    ))
    .is_none()
    {
        eprintln!("skipping (credless on seed)");
        return;
    }
    // Belt-and-braces: the geocode row is not plan-keyed, so it must not rely on
    // teardown_plan; drop any prior-run leftover before seeding it below.
    let _ = db_exec_teardown(&format!(
        "DELETE FROM route_place_geocodes WHERE query_key = '{}';",
        geo_key.replace('\'', "''")
    ));

    // 1. Rename via the CLI with --segments.
    let Some(out) = run_cmd(&["set-poi-title", &dest, "poi_x", "天龍寺", "--segments"]) else {
        eprintln!("skipping (credless on run)");
        return;
    };
    assert!(out.contains("天龍寺"), "stdout should show the new title: {out}");

    let title = one_cell(&format!(
        "SELECT title FROM destination_pois WHERE slug = {d} AND poi_id = 'poi_x'"
    ))
    .expect("poi row vanished");
    assert_eq!(title, "天龍寺");

    let seg_from = one_cell(&format!(
        "SELECT from_place FROM day_route_segments WHERE plan_id = {p} AND destination = {d} AND day_number = 1 AND sort_order = 0"
    ))
    .expect("segment row vanished");
    assert_eq!(seg_from, "天龍寺", "--segments must rename the matching label");
    let seg_to = one_cell(&format!(
        "SELECT to_place FROM day_route_segments WHERE plan_id = {p} AND destination = {d} AND day_number = 1 AND sort_order = 0"
    ))
    .expect("segment row vanished");
    assert_eq!(seg_to, "Bamboo Stop", "non-matching labels must stay untouched");

    // 2. Manual pin: the CLI-printed key must be the row snapshot-maps reads.
    let place = format!("zz測試點{n}");
    let Some(out) = run_cmd(&[
        "set-place-geocode",
        &place,
        "--lat",
        "35.0629",
        "--lon",
        "135.7852",
        "--context",
        "Kyoto, Japan",
    ]) else {
        eprintln!("skipping (credless on geocode run)");
        return;
    };
    assert!(out.contains(&geo_key), "stdout should print {geo_key}: {out}");
    let pinned = one_cell(&format!(
        "SELECT lat FROM route_place_geocodes WHERE query_key = '{}'",
        geo_key.replace('\'', "''")
    ))
    .expect("manual geocode row missing");
    assert_eq!(pinned.parse::<f64>().unwrap(), 35.0629);
}
