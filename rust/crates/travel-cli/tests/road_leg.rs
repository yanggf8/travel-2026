//! Integration tests for `travel road-leg` — the OSRM road-geometry cache
//! (`route_road_legs` + `route_road_leg_points`) that `snapshot-maps` reads.
//!
//! The cache is GLOBAL (coords-keyed, not plan-keyed), so teardown is LOCAL
//! delete-by-leg_key — NOT `teardown_plan` (no plan rows are ever touched).
//!
//! Invariants exercised:
//!   1. `road-leg list` prints the cached legs in plain text (uses the
//!      read path only; runs against whatever legs exist, asserts its own
//!      seeded row appears and the run does not fail).
//!   2. `road-leg refetch` with literal lat,lon endpoints re-fetches from
//!      OSRM, upserts the leg + points under the exact 5-dp key, and reports
//!      the geometry in plain text.
//!   3. `road-leg refetch` with `--via` waypoints stores the FORCED route:
//!      the polyline must pass near the via point (the coastal-route
//!      regression guard — a refetch without vias returns the fastest path,
//!      which ignores the via).
//!   4. Missing --to fails loud with the usage reminder.
//!
//! OSRM network flake: if the router is unreachable the refetch tests print a
//! skip note instead of failing (the demo router has no SLA; the projection
//! invariants are covered by snapshot_maps unit tests).

mod common;
use common::{bin, db_exec, db_exec_teardown, nanos, Guard, Rows};

use std::process::Command;

fn leg_key(from: (f64, f64), to: (f64, f64)) -> String {
    format!("{:.5},{:.5}>{:.5},{:.5}|osrm-demo|driving", from.0, from.1, to.0, to.1)
}

fn seed_leg(key: &str) -> bool {
    if db_exec("SELECT 1").is_none() {
        return false;
    }
    db_exec(&format!(
        "INSERT INTO route_road_legs (leg_key, from_lat, from_lon, to_lat, to_lon, provider, profile, status, point_count, distance_m, failure_reason, fetched_at) \
         VALUES ('{key}', 35.0, 139.0, 35.5, 139.5, 'osrm-demo', 'driving', 'ok', 2, 1000.0, NULL, '2026-01-01T00:00:00Z'); \
         INSERT INTO route_road_leg_points (leg_key, point_order, lat, lon) \
         VALUES ('{key}', 0, 35.0, 139.0), ('{key}', 1, 35.5, 139.5);"
    ))
    .is_some()
}

fn teardown_leg(key: &str) {
    let _ = db_exec_teardown(&format!(
        "DELETE FROM route_road_leg_points WHERE leg_key = '{key}'; \
         DELETE FROM route_road_legs WHERE leg_key = '{key}';"
    ));
}

/// Run `travel road-leg ...`. Returns (ok, stdout, stderr).
fn run_road_leg(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel road-leg {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Skip on OSRM unreachability — the demo router has no SLA.
fn osrm_down(stderr: &str) -> bool {
    stderr.contains("OSRM request failed") || stderr.contains("OSRM response parse failed")
}

fn points(key: &str) -> Vec<(f64, f64)> {
    let rows: Option<Rows> = db_exec(&format!(
        "SELECT lat, lon FROM route_road_leg_points WHERE leg_key = '{key}' ORDER BY point_order"
    ));
    let raw = rows.map(|r| r.raw().to_string()).unwrap_or_default();
    let mut pts = Vec::new();
    for line in raw.lines() {
        // One row per line: "lat: 35.0, lon: 139.0" (db exec plain-text rows).
        let cols: Vec<Option<f64>> = line
            .split(", ")
            .filter_map(|col| col.split_once(':'))
            .map(|(_, v)| v.trim().parse::<f64>().ok())
            .collect();
        if let (Some(Some(a)), Some(Some(b))) = (cols.first(), cols.get(1)) {
            pts.push((*a, *b));
        }
    }
    pts
}

// ── list prints cached legs in plain text ────────────────────────────────────
#[test]
fn road_leg_list_prints_seeded_leg() {
    let tag = nanos();
    // A deterministic key: real coords so the row shape matches production.
    let key = leg_key((35.0 + 1e-5 * (tag % 100) as f64, 139.0), (35.5, 139.5));
    let _g = Guard::new({
        let key = key.clone();
        move || teardown_leg(&key)
    });
    if !seed_leg(&key) {
        return;
    }

    let (ok, stdout, stderr) = run_road_leg(&["road-leg", "list"]);
    assert!(ok, "road-leg list should succeed; stderr={stderr}");
    assert!(stdout.contains(&key), "list output must contain the leg key");
    assert!(
        stdout.contains("leg(s)"),
        "list output ends with a leg count line"
    );
}

// ── refetch with via stores the FORCED route through the via point ──────────
#[test]
fn road_leg_refetch_via_forces_route() {
    let tag = nanos();
    // Distinct endpoints far from any production leg (Sea of Japan coast).
    let from = (35.500, 133.050 + 1e-4 * (tag % 100) as f64);
    let to = (35.550, 133.100);
    let via = (35.525, 133.075); // between the endpoints; the route must pass nearby
    let key = leg_key(from, to);
    let _g = Guard::new({
        let key = key.clone();
        move || teardown_leg(&key)
    });
    if db_exec("SELECT 1").is_none() {
        return;
    }

    let (ok, stdout, stderr) = run_road_leg(&[
        "road-leg",
        "refetch",
        "--from",
        &format!("{:.5},{:.5}", from.0, from.1),
        "--to",
        &format!("{:.5},{:.5}", to.0, to.1),
        "--via",
        &format!("{:.5},{:.5}", via.0, via.1),
    ]);
    if !ok && osrm_down(&stderr) {
        eprintln!("SKIP: OSRM demo router unreachable (stderr={stderr})");
        return;
    }
    assert!(
        ok,
        "road-leg refetch should succeed; stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("road leg cached"),
        "refetch reports the cached leg key in plain text; stdout={stdout}"
    );

    let leg = db_exec(&format!(
        "SELECT status || '|' || point_count FROM route_road_legs WHERE leg_key = '{key}'"
    ))
    .and_then(|r| r.scalar());
    let Some(leg) = leg else {
        panic!("leg row must exist after refetch; stdout={stdout}");
    };
    let (status, count) = leg.split_once('|').expect("status|point_count");
    assert_eq!(status, "ok");
    assert!(
        count.parse::<usize>().unwrap() >= 2,
        "cached geometry must have at least 2 points, got {count}"
    );

    // The FORCED-route guard: OSRM snaps the via waypoint onto the nearest
    // road and routes through it, so some polyline point must sit near the via
    // (2 km tolerance covers snap distance in rural terrain). Without --via
    // the fastest path may bypass the area entirely — that is the coastal 台2
    // regression this assertion exists to catch.
    let pts = points(&key);
    assert!(pts.len() >= 2, "geometry points readable from the cache");
    let nearest = pts
        .iter()
        .map(|&(lat, lon)| {
            let (dlat, dlon) = (lat - via.0, (lon - via.1) * via.0.to_radians().cos());
            (dlat * dlat + dlon * dlon).sqrt() * 111_320.0
        })
        .fold(f64::INFINITY, f64::min);
    assert!(
        nearest <= 2_000.0,
        "route must pass near the via point (nearest {nearest:.0} m); \
         a refetch without --via would draw the fastest path instead"
    );
}

// ── refetch without --to fails loud ─────────────────────────────────────────
#[test]
fn road_leg_refetch_requires_to() {
    let (ok, _stdout, stderr) = run_road_leg(&[
        "road-leg",
        "refetch",
        "--from",
        "35.50000,133.05000",
    ]);
    assert!(!ok, "refetch without --to must exit non-zero");
    assert!(
        stderr.contains("missing --to"),
        "stderr names the missing argument; stderr={stderr}"
    );
}
