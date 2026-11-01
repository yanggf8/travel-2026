//! `travel set-clothing-tip` against real Turso: the CLI inserts a band, a second
//! call with the same tip_id REPLACES it (one row, new values), and a rejected call
//! writes nothing. `clothing_tips` is GLOBAL (not plan-keyed), so the rows this test
//! seeds are torn down locally by tip_id — no plan is created.
//!
//! The fixture bands use kinds whose thresholds can never match (rain >= 1000 %,
//! swing >= 1000 °C) and a huge sort_order, so the live dashboard — which reads this
//! same table — renders nothing different while the test runs.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, Guard};

static LOCK: Mutex<()> = Mutex::new(());

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Run the binary; None when the sandbox has no Turso creds.
fn run(args: &[&str]) -> Option<(bool, String, String)> {
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
    Some((out.status.success(), stdout, stderr))
}

fn row(tip_id: &str) -> Vec<String> {
    db_exec(&format!(
        "SELECT kind || '|' || COALESCE(CAST(threshold AS TEXT), '') || '|' || sort_order \
         || '|' || text_zh || '|' || text_en AS r FROM clothing_tips WHERE tip_id = {}",
        sql_lit(tip_id)
    ))
    .expect("creds present (checked by the first CLI call)")
    .column()
}

#[test]
fn set_clothing_tip_inserts_replaces_and_rejects_without_writing() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let n = nanos();
    let rain = format!("zz_test_{n}_rain");
    let swing = format!("zz_test_{n}_swing");
    let bad = format!("zz_test_{n}_bad");

    let _g = Guard::new({
        let ids = [rain.clone(), swing.clone(), bad.clone()];
        move || {
            let list = ids.iter().map(|i| sql_lit(i)).collect::<Vec<_>>().join(", ");
            let _ = db_exec_teardown(&format!("DELETE FROM clothing_tips WHERE tip_id IN ({list})"));
        }
    });

    // Insert.
    let Some((ok, stdout, stderr)) = run(&[
        "set-clothing-tip", &rain, "--kind", "rain_min", "--threshold", "1000", "--sort",
        "2000000001", "--zh", "zz 帶傘 {hi}°C", "--en", "zz umbrella {lo}°C",
    ]) else {
        eprintln!("skipping (credless)");
        return;
    };
    assert!(ok, "insert failed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains(&format!("clothing_tips upserted: {rain}")), "{stdout}");
    // The listing after the write shows the new band in read order.
    assert!(stdout.contains(&format!("2000000001\t{rain}\train_min\t1000\tzz 帶傘 {{hi}}°C")), "{stdout}");
    assert_eq!(row(&rain), vec!["rain_min|1000.0|2000000001|zz 帶傘 {hi}°C|zz umbrella {lo}°C"]);

    // Same tip_id again → full replace, still one row.
    let (ok, stdout, stderr) = run(&[
        "set-clothing-tip", &rain, "--kind", "swing_min", "--threshold", "999.5", "--sort",
        "2000000002", "--zh", "zz 改", "--en", "zz changed",
    ])
    .expect("creds");
    assert!(ok, "replace failed; stdout={stdout} stderr={stderr}");
    assert_eq!(row(&rain), vec!["swing_min|999.5|2000000002|zz 改|zz changed"]);

    // A second band is its own row.
    let (ok, stdout, stderr) = run(&[
        "set-clothing-tip", &swing, "--kind", "swing_min", "--threshold", "1000", "--sort",
        "2000000003", "--zh", "zz 溫差", "--en", "zz swing",
    ])
    .expect("creds");
    assert!(ok, "second insert failed; stdout={stdout} stderr={stderr}");
    assert_eq!(row(&swing).len(), 1);

    // Rejected calls fail loud and write nothing.
    for (args, needle) in [
        (vec!["--kind", "rain_min", "--sort", "1", "--zh", "z", "--en", "e"], "--threshold N is required"),
        (vec!["--kind", "rain_min", "--threshold", "5", "--sort", "1", "--zh", "{low}", "--en", "e"], "unknown placeholder"),
        (vec!["--kind", "rain_min", "--threshold", "5", "--sort", "1", "--zh", "z", "--en", "e", "--bogus", "x"], "unknown argument: --bogus"),
    ] {
        let mut full = vec!["set-clothing-tip", bad.as_str()];
        full.extend(args);
        let (ok, stdout, stderr) = run(&full).expect("creds");
        assert!(!ok, "expected failure for {full:?}; stdout={stdout}");
        assert!(stderr.contains(needle) || stdout.contains(needle), "{full:?}: stderr={stderr}");
    }
    assert!(row(&bad).is_empty(), "a rejected call wrote a row");
}
