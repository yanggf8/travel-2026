// Unknown-flag rejection locks for the two connect-before-parse commands
// sync-bookings + fetch-weather. Both resolve the plan (open Turso) in the
// main.rs dispatch arm, so the reject is preflighted there — which also makes
// these tests hermetic (they fail loud before any DB touch, no creds needed).
mod common;
use common::bin;
use std::process::Command;

#[test]
fn sync_bookings_rejects_unknown_flag() {
    let out = Command::new(bin())
        .args(["sync-bookings", "--dry-runn"])
        .env("TRAVEL_PLAN_ID", "zz-no-db")
        .output()
        .expect("run sync-bookings");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "must reject; stderr={stderr}");
    assert!(
        stderr.contains("unknown argument: --dry-runn"),
        "stderr should name the bad flag; stderr={stderr}"
    );
}

#[test]
fn fetch_weather_rejects_unknown_flag() {
    let out = Command::new(bin())
        .args(["fetch-weather", "--al"])
        .env("TRAVEL_PLAN_ID", "zz-no-db")
        .output()
        .expect("run fetch-weather");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "must reject; stderr={stderr}");
    assert!(
        stderr.contains("unknown argument: --al"),
        "stderr should name the bad flag; stderr={stderr}"
    );
}
