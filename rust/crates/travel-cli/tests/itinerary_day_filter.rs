//! `travel itinerary --day N` renders ONLY that day (the full-trip header kept,
//! the PENDING BOOKINGS section scoped to the day), and unknown flags fail
//! loud instead of being silently ignored. Locks the gap found in self-review:
//! `itinerary --day 2 --plan-id X` used to ignore --day and print every day.
//!
//! Real-Turso integration test; unique zztest fixtures; skips cleanly without
//! creds. Serialized per file via a static mutex.

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

use std::process::Command;
use std::sync::Mutex;

static DAY_LOCK: Mutex<()> = Mutex::new(());

fn run_cmd(plan: &str, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env("TRAVEL_PLAN_ID", plan)
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Seed two days, each with a pending-booked morning activity, so both the day
/// blocks and the PENDING BOOKINGS section have day-scoped content.
fn seed_two_days(plan: &str, dest: &str) {
    for day in 1..=2 {
        db_exec(&format!(
            "INSERT OR REPLACE INTO days \
               (plan_id, destination, day_number, date, theme, day_type, status, excursion) \
             VALUES ('{plan}', '{dest}', {day}, '2026-11-1{day}', 'ZZ theme', 'full', \
                     'confirmed', 0); \
             INSERT OR REPLACE INTO timesofday \
               (plan_id, destination, day_number, session_type) \
             VALUES ('{plan}', '{dest}', {day}, 'morning'); \
             INSERT OR REPLACE INTO activities \
               (id, plan_id, destination, day_number, session_type, sort_order, title, \
                booking_required, booking_status, book_by, is_fixed_time, priority, source) \
             VALUES ('act{day}-{plan}', '{plan}', '{dest}', {day}, 'morning', 0, \
                     'ZZ Day {day} Activity', 1, 'pending', '2026-11-0{day}', 0, 'must', 'seed')"
        ))
        .expect("seed day");
    }
}

#[test]
fn itinerary_day_flag_renders_only_that_day() {
    let _g = DAY_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zztest-idf-{tag}");
    let dest = format!("zzidf{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping (no Turso creds)");
        return;
    }
    seed_plan(&plan, &dest, 0);
    seed_two_days(&plan, &dest);

    let (ok, stdout, stderr) = run_cmd(&plan, &["itinerary", "--day", "2"]);
    assert!(ok, "--day 2 should succeed; stderr={stderr}");
    assert!(stdout.contains("ITINERARY"), "trip header must stay");
    assert!(stdout.contains("Day 2"), "Day 2 block must render");
    assert!(
        !stdout.contains("Day 1"),
        "--day 2 must not render Day 1 (blocks AND pending-bookings): got:\n{stdout}"
    );
    // The pending-bookings section, when present, must be day-scoped.
    if stdout.contains("PENDING BOOKINGS") {
        assert!(
            !stdout.contains("Day 1:"),
            "PENDING BOOKINGS must be scoped to day 2"
        );
    }
}

#[test]
fn itinerary_day_flag_rejects_nonexistent_day() {
    let _g = DAY_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zztest-idf-{tag}");
    let dest = format!("zzidf{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping (no Turso creds)");
        return;
    }
    seed_plan(&plan, &dest, 0);
    seed_two_days(&plan, &dest);

    let (ok, stdout, stderr) = run_cmd(&plan, &["itinerary", "--day", "9"]);
    assert!(
        !ok,
        "a nonexistent day must fail loud, not render an empty view; stdout={stdout}"
    );
    assert!(
        stderr.to_lowercase().contains("day"),
        "the error must name the day problem: {stderr}"
    );
}

#[test]
fn itinerary_rejects_unknown_flag() {
    let _g = DAY_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zztest-idf-{tag}");
    let dest = format!("zzidf{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping (no Turso creds)");
        return;
    }
    seed_plan(&plan, &dest, 0);

    let (ok, _stdout, stderr) = run_cmd(&plan, &["itinerary", "--wat"]);
    assert!(
        !ok,
        "an unknown flag must fail loud (project rule), not be silently ignored"
    );
    assert!(stderr.contains("--wat"), "the error must name the flag: {stderr}");
}

/// Flag-surface rejections fire BEFORE any DB access (reject_unknown_flags is
/// the first thing after --help), so these need no Turso credentials at all
/// (codex review: add credential-free malformed-argument coverage).
#[test]
fn itinerary_rejects_malformed_arguments_without_db() {
    let _g = DAY_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zztest-idf-{tag}");
    let cases: Vec<Vec<&str>> = vec![
        vec!["itinerary", "--day"],                       // missing value
        vec!["itinerary", "--dest"],                      // missing value
        vec!["itinerary", "--dest", "--wat"],             // flag swallowed as a value
        vec!["itinerary", "--day", "1", "--day", "2"],    // duplicate --day
        vec!["itinerary", "tokyo"],                       // stray positional
    ];
    for args in cases {
        let (ok, _stdout, stderr) = run_cmd(&plan, &args);
        assert!(
            !ok,
            "{args:?} must fail loud before any DB access (value flags need a value, \
             no duplicate --day, no stray positionals)"
        );
        assert!(
            !stderr.is_empty(),
            "{args:?} rejection must explain itself on stderr"
        );
    }
}
