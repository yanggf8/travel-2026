//! Real-Turso test for `set-fit-offer`: add → list → remove on a seeded plan and
//! a per-run package offer, the destination/type guard, and the audit triad.

use std::process::Command;

mod common;
use common::{bin, db_exec, is_credless, nanos, seed_plan, teardown_offers, teardown_plan, Guard};

fn run(plan: &str, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .arg("set-fit-offer")
        .args(args)
        .args(["--plan-id", plan])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run travel set-fit-offer");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn scalar(sql: &str) -> String {
    db_exec(sql)
        .and_then(|r| r.scalar())
        .unwrap_or_else(|| panic!("no scalar for {sql}"))
}

#[test]
fn set_fit_offer_adds_lists_removes_and_audits() {
    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }
    let tag = nanos();
    let plan = format!("zztest-fit-{tag}");
    let dest = format!("zztest_fit_{tag}");
    let pkg = format!("zz_test_{tag}_pkg");
    let hotel = format!("zz_test_{tag}_hotel");
    let _g = Guard::new({
        let (plan, dest, pkg, hotel) = (plan.clone(), dest.clone(), pkg.clone(), hotel.clone());
        move || {
            teardown_offers(&[&pkg, &hotel]);
            teardown_plan(&plan, &dest);
        }
    });

    seed_plan(&plan, &dest, 10);
    db_exec(&format!(
        "INSERT INTO offers (id, source_id, type, name, price_per_person, currency, destination, \
            departure_date, return_date, nights, scraped_at, hotel_name) VALUES \
         ('{pkg}', 'zzfit', 'package', 'ZZ PKG', 20000, 'TWD', '{dest}', '2026-11-12', '2026-11-16', 4, \
            '2026-09-30T00:00:00Z', 'ZZ HOTEL'), \
         ('{hotel}', 'zzfit', 'hotel', 'ZZ HOTEL ONLY', 5000, 'TWD', '{dest}', '2026-11-12', '2026-11-16', 4, \
            '2026-09-30T00:00:00Z', 'ZZ HOTEL')"
    ))
    .expect("seed offers");

    let (ok, stdout, stderr) = run(&plan, &[&pkg]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping set-fit-offer test: {}", stderr.trim());
        return;
    }
    assert!(ok, "add should succeed; stdout={stdout}\nstderr={stderr}");
    assert!(stdout.contains(&format!("FIT offers for {plan} / {dest}: 1")), "{stdout}");
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_fit_offers WHERE plan_id = '{plan}' AND offer_id = '{pkg}'"
        )),
        "1"
    );
    assert_eq!(scalar(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'")), "11");
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = '{plan}' AND command_type = 'set-fit-offer'"
        )),
        "1"
    );

    // A hotel-only offer is refused; nothing is written.
    let (ok, _, stderr) = run(&plan, &[&hotel]);
    assert!(!ok, "hotel offer must be refused");
    assert!(stderr.contains("package offers"), "{stderr}");
    assert_eq!(scalar(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'")), "11");

    let (ok, stdout, stderr) = run(&plan, &[&pkg, "--remove"]);
    assert!(ok, "remove should succeed; stdout={stdout}\nstderr={stderr}");
    assert!(stdout.contains(": 0"), "{stdout}");
    assert_eq!(scalar(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'")), "12");

    // Removing again fails loud.
    let (ok, _, stderr) = run(&plan, &[&pkg, "--remove"]);
    assert!(!ok && stderr.contains("not listed"), "{stderr}");
}
