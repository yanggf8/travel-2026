//! Real-Turso test for `set-weather-provider` / `weather-providers`: upsert is
//! idempotent (second call replaces the row), the list shows it, and the command
//! writes NO audit rows (global reference data). zz_test_ id; Guard deletes the row.

use std::process::Command;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, Guard};

fn run_cmd(args: &[&str]) -> Option<String> {
    let out = Command::new(bin()).args(args).env_remove("TRAVEL_PLAN_ID").output().expect("run travel");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() && is_credless(&stderr) {
        eprintln!("skipping set-weather-provider test: {}", stderr.trim());
        return None;
    }
    assert!(out.status.success(), "travel {args:?} failed; stdout={stdout} stderr={stderr}");
    Some(stdout)
}

#[test]
fn set_weather_provider_upserts_and_lists() {
    let id = format!("zz_test_{}_wp", nanos());
    let _g = Guard::new({
        let id = id.clone();
        move || {
            let _ = db_exec_teardown(&format!("DELETE FROM weather_providers WHERE provider_id = '{id}'"));
        }
    });
    let Some(out) = run_cmd(&[
        "set-weather-provider", &id, "--name-zh", "測試甲", "--name-en", "Test A",
        "--url", "https://example.test/a", "--license", "CC BY 4.0",
    ]) else { return };
    assert!(out.contains("upserted") && out.contains(&id), "{out}");

    // second call replaces, not duplicates
    let Some(_) = run_cmd(&[
        "set-weather-provider", &id, "--name-zh", "測試乙", "--name-en", "Test B",
        "--url", "https://example.test/b", "--license", "CC0",
    ]) else { return };
    let rows = db_exec(&format!(
        "SELECT name_zh || '|' || name_en || '|' || attribution_url || '|' || license FROM weather_providers WHERE provider_id = '{id}'"
    )).expect("select");
    assert_eq!(rows.column().len(), 1, "exactly one row after two upserts");
    assert_eq!(rows.scalar().unwrap(), "測試乙|Test B|https://example.test/b|CC0");

    let Some(list) = run_cmd(&["weather-providers"]) else { return };
    assert!(list.contains(&id) && list.contains("CC0"), "{list}");

    // global reference data: no audit row mentions the test id
    let audit = db_exec(&format!("SELECT COUNT(*) FROM operation_runs WHERE command_summary LIKE '%{id}%' OR plan_id LIKE '%{id}%'")).expect("audit");
    assert_eq!(audit.scalar().unwrap(), "0", "set-weather-provider must not write operation_runs");

    // bad input fails loud
    let bad = Command::new(bin())
        .args(["set-weather-provider", &id, "--name-zh", "x", "--name-en", "x", "--url", "nope", "--license", "x"])
        .output().expect("run");
    assert!(!bad.status.success());
}
