//! Real-Turso test for `set-fit-offer`: add → list → remove on a seeded plan and
//! a per-run package offer, the destination/type guard, and the audit triad;
//! plus the `--booked` / `--not-booked` marker (one booked FIT per plan+dest).

use std::process::Command;

mod common;
use common::{bin, db_exec, is_credless, nanos, seed_plan, teardown_offers, teardown_plan, Guard};

/// The writes run in a transaction; two tests writing the shared DB at once get
/// SQLITE_BUSY (`database is locked`), so this file's tests run one at a time.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
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

#[test]
fn set_fit_offer_booked_marker_is_unique_and_audited() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }
    let tag = nanos();
    let plan = format!("zztest-fitb-{tag}");
    let dest = format!("zztest_fitb_{tag}");
    let a = format!("zz_test_{tag}_a");
    let b = format!("zz_test_{tag}_b");
    let unlisted = format!("zz_test_{tag}_u");
    let _g = Guard::new({
        let (plan, dest, a, b, unlisted) =
            (plan.clone(), dest.clone(), a.clone(), b.clone(), unlisted.clone());
        move || {
            teardown_offers(&[&a, &b, &unlisted]);
            teardown_plan(&plan, &dest);
        }
    });

    seed_plan(&plan, &dest, 10);
    let offer = |id: &str, price: i64| {
        format!(
            "('{id}', 'zzfitb', 'package', 'ZZ PKG', {price}, 'TWD', '{dest}', '2026-11-12', \
              '2026-11-16', 4, '2026-09-30T00:00:00Z', 'ZZ HOTEL')"
        )
    };
    db_exec(&format!(
        "INSERT INTO offers (id, source_id, type, name, price_per_person, currency, destination, \
            departure_date, return_date, nights, scraped_at, hotel_name) VALUES {}, {}, {}",
        offer(&a, 20000),
        offer(&b, 21000),
        offer(&unlisted, 22000)
    ))
    .expect("seed offers");

    let (ok, stdout, stderr) = run(&plan, &[&a]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping set-fit-offer booked test: {}", stderr.trim());
        return;
    }
    assert!(ok, "list a; stdout={stdout}\nstderr={stderr}");
    let (ok, stdout, stderr) = run(&plan, &[&b]);
    assert!(ok, "list b; stdout={stdout}\nstderr={stderr}");
    let version = || scalar(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'"));
    let booked = |id: &str| {
        scalar(&format!(
            "SELECT booked AS b FROM plan_fit_offers WHERE plan_id = '{plan}' AND offer_id = '{id}'"
        ))
    };
    assert_eq!(version(), "12");

    // Marking an offer that is not in the FIT list fails loud; nothing is written.
    let (ok, _, stderr) = run(&plan, &[&unlisted, "--booked"]);
    assert!(!ok && stderr.contains("not listed"), "{stderr}");
    assert_eq!(version(), "12");

    let (ok, stdout, stderr) = run(&plan, &[&a, "--booked"]);
    assert!(ok, "mark a; stdout={stdout}\nstderr={stderr}");
    assert!(stdout.contains(&format!("FIT offer marked booked: {a}")), "{stdout}");
    assert!(stdout.contains(&format!("{a}  [booked]")), "{stdout}");
    assert_eq!((booked(&a), booked(&b)), ("1".to_string(), "0".to_string()));
    assert_eq!(version(), "13");
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}' AND event = 'fit_offer_booked'"
        )),
        "1"
    );

    // Marking b unmarks a in the same transaction and says so.
    let (ok, stdout, stderr) = run(&plan, &[&b, "--booked"]);
    assert!(ok, "mark b; stdout={stdout}\nstderr={stderr}");
    assert!(
        stdout.contains(&format!("unmarked previously booked FIT offer: {a}")),
        "{stdout}"
    );
    assert_eq!((booked(&a), booked(&b)), ("0".to_string(), "1".to_string()));
    assert_eq!(version(), "14");
    assert_eq!(
        scalar(&format!(
            "SELECT value AS v FROM plan_event_data WHERE plan_id = '{plan}' AND key = 'unmarked_offer_id'"
        )),
        a
    );

    // Re-marking is a no-op: no audit row, no version bump.
    let (ok, stdout, stderr) = run(&plan, &[&b, "--booked"]);
    assert!(ok && stdout.contains("already marked booked"), "{stdout}\n{stderr}");
    assert_eq!(version(), "14");

    let (ok, stdout, stderr) = run(&plan, &[&b, "--not-booked"]);
    assert!(ok, "unmark b; stdout={stdout}\nstderr={stderr}");
    assert_eq!(booked(&b), "0");
    assert_eq!(version(), "15");
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = '{plan}' AND command_type = 'set-fit-offer'"
        )),
        "5"
    );
}
