//! Behavior LOCK for B5: `resolve_active_destination` rejects a `--dest` override
//! that is not a registered destination of the plan — so `set-* --dest bogus_slug`
//! fails loud instead of writing orphaned rows under a phantom destination (the
//! write-side analogue of the read-side assert_dest_matches bug). Exercised via
//! set-day-theme (a simple mutation that routes --dest through the resolver).

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, is_credless, nanos, seed_plan, teardown_plan, Guard};

static LOCK: Mutex<()> = Mutex::new(());

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[test]
fn set_day_theme_rejects_bogus_dest_override() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let n = nanos();
    let plan = format!("zztest-destval-{n}");
    let dest = format!("zz_destval_{n}");

    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    teardown_plan(&plan, &dest);
    seed_plan(&plan, &dest, 0);

    let d = sql_lit(&dest);
    let p = sql_lit(&plan);
    // seed_plan already registers the plan's real destination in plan_destinations;
    // add a day so a VALID --dest write has a row to touch.
    if db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, status, updated_at) \
           VALUES ({p}, {d}, 1, '2026-11-01', 'full', 'draft', '2020-01-01 00:00:00');"
    ))
    .is_none()
    {
        eprintln!("skipping (credless on seed)");
        return;
    }

    // Bogus --dest must fail loud BEFORE any write, naming the real one.
    let bogus = Command::new(bin())
        .args(["set-day-theme", "1", "X", "--plan-id", &plan, "--dest", "zz_not_a_dest"])
        .output()
        .expect("run set-day-theme");
    let berr = String::from_utf8_lossy(&bogus.stderr);
    if is_credless(&berr) {
        return;
    }
    assert!(!bogus.status.success(), "bogus --dest must fail; stderr={berr}");
    assert!(
        berr.contains("is not a destination of plan"),
        "error should name the phantom-dest problem; stderr={berr}"
    );

    // A VALID --dest (the plan's real destination) must still succeed — no false rejection.
    let ok = Command::new(bin())
        .args(["set-day-theme", "1", "Real Theme", "--plan-id", &plan, "--dest", &dest])
        .output()
        .expect("run set-day-theme");
    let oerr = String::from_utf8_lossy(&ok.stderr);
    let ostdout = String::from_utf8_lossy(&ok.stdout);
    assert!(
        ok.status.success(),
        "valid --dest must not be falsely rejected; stdout={ostdout} stderr={oerr}"
    );
}
