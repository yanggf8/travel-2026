//! Real-Turso behavior-LOCK integration test for `set-process-status`.
//!
//! Seeds a unique plan with process_statuses ladder rows, runs
//! `set-process-status process_3_transportation booked`, asserts the shortest
//! legal path (pending -> populated -> booking -> booked), dual-bucket events,
//! operation_runs audit, and version bump. Also locks idempotent re-run and
//! no-path failure on corrupted status.
//!
//! Status machine (2026-11-11): an off-ladder target is rejected with nothing
//! written; `p5 confirmed` is refused on a publish blocker and takes two hops
//! when the plan is clean; a failure injected between hops
//! (`TRAVEL_STATUS_FAIL_AT`) rolls the whole command back — which also proves
//! the hrana stream accepts `BEGIN IMMEDIATE` (every successful run here
//! commits through it).
//!
//! Skips cleanly if Turso creds are absent. Panic-safe teardown via Guard.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, is_credless, is_transient, nanos, seed_plan, teardown_plan, Guard};

static SET_PROCESS_STATUS_LOCK: Mutex<()> = Mutex::new(());

fn run_cmd(args: &[&str]) -> Option<(bool, String, String)> {
    for attempt in 0..6 {
        let out = Command::new(bin())
            .args(args)
            .env_remove("TRAVEL_PLAN_ID")
            .output()
            .expect("run set-process-status");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        if !out.status.success() && is_credless(&stderr) {
            eprintln!(
                "skipping set-process-status test (no Turso creds): {}",
                stderr.trim()
            );
            return None;
        }
        if !out.status.success() && is_transient(&stderr) && attempt < 5 {
            std::thread::sleep(std::time::Duration::from_millis(400 * (attempt + 1)));
            continue;
        }
        return Some((out.status.success(), stdout, stderr));
    }
    unreachable!()
}

fn seed_process_statuses(plan: &str, dest: &str) -> bool {
    seed_plan(plan, dest, 7);
    let sql = format!(
        "INSERT INTO process_statuses (plan_id, destination, process_id, status) VALUES \
         ('{plan}','{dest}','process_1_date_anchor','confirmed'), \
         ('{plan}','{dest}','process_2_destination','confirmed'), \
         ('{plan}','{dest}','process_3_transportation','pending'), \
         ('{plan}','{dest}','process_3_4_packages','pending'), \
         ('{plan}','{dest}','process_4_accommodation','pending'), \
         ('{plan}','{dest}','process_5_daily_itinerary','pending');"
    );
    db_exec(&sql).is_some()
}

fn run_set_status(plan: &str, dest: &str, process_id: &str, target: &str) -> Option<(bool, String, String)> {
    run_cmd(&[
        "set-process-status",
        process_id,
        target,
        "--dest",
        dest,
        "--plan-id",
        plan,
    ])
}

#[test]
fn set_process_status_advances_shortest_path_and_audits() {
    let _lock = SET_PROCESS_STATUS_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }

    let tag = nanos();
    let plan = format!("test-setstatus-{tag}");
    let dest = format!("zz_setstatus_{tag}");

    teardown_plan(&plan, &dest);
    let _g = Guard::new({
        let plan = plan.clone();
        let dest = dest.clone();
        move || teardown_plan(&plan, &dest)
    });

    assert!(seed_process_statuses(&plan, &dest), "seed plan");

    let (ok, stdout, stderr) = run_set_status(&plan, &dest, "process_3_transportation", "booked")
        .expect("run set-process-status");
    assert!(
        ok,
        "set-process-status should succeed; stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("Path:") && stdout.contains("pending") && stdout.contains("booked"),
        "stdout should show path; stdout={stdout}"
    );

    // Final status
    let status = db_exec(&format!(
        "SELECT status FROM process_statuses WHERE plan_id='{plan}' AND destination='{dest}' AND process_id='process_3_transportation';"
    ))
    .unwrap();
    assert_eq!(status.scalar().as_deref(), Some("booked"), "final status; out={status}");

    // Dest-process events
    let dest_events = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM plan_events \
         WHERE plan_id='{plan}' AND scope='dest_process' AND destination='{dest}' \
           AND process_id='process_3_transportation' AND event='status_changed';"
    ))
    .unwrap();
    assert_eq!(
        dest_events.scalar().as_deref(),
        Some("3"),
        "dest_process status_changed count; out={dest_events}"
    );

    // Timeline events
    let tl_events = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM plan_events \
         WHERE plan_id='{plan}' AND scope='timeline' AND destination='' \
           AND process_id='process_3_transportation' AND event='status_changed';"
    ))
    .unwrap();
    assert_eq!(
        tl_events.scalar().as_deref(),
        Some("3"),
        "timeline status_changed count; out={tl_events}"
    );

    // Hop order
    let hops = db_exec(&format!(
        "SELECT from_state || '->' || to_state AS hop FROM plan_events \
         WHERE plan_id='{plan}' AND scope='dest_process' AND destination='{dest}' \
           AND process_id='process_3_transportation' AND event='status_changed' \
         ORDER BY sort_order;"
    ))
    .unwrap();
    assert_eq!(
        hops.column(),
        vec![
            "pending->populated".to_string(),
            "populated->booking".to_string(),
            "booking->booked".to_string(),
        ],
        "hop order; out={hops}"
    );

    // Operation/version
    let op = db_exec(&format!(
        "SELECT command_type || '|' || status || '|' || version_before || '|' || version_after || '|' || command_summary \
         FROM operation_runs WHERE plan_id='{plan}' ORDER BY started_at DESC LIMIT 1;"
    ))
    .unwrap();
    let op_line = op.scalar().unwrap_or_default();
    assert!(
        op_line.starts_with("set-process-status|completed|7|8|"),
        "operation_runs row; got={op_line}"
    );
    assert!(
        op_line.contains("process_3_transportation") && op_line.contains("pending->booked"),
        "command_summary; got={op_line}"
    );

    let version = db_exec(&format!(
        "SELECT version FROM plans WHERE plan_id='{plan}';"
    ))
    .unwrap();
    assert_eq!(version.scalar().as_deref(), Some("8"), "plan version; out={version}");

    // Idempotent re-run
    let (ok2, stdout2, stderr2) = run_set_status(&plan, &dest, "process_3_transportation", "booked")
        .expect("idempotent re-run");
    assert!(
        ok2,
        "idempotent re-run should succeed; stdout={stdout2} stderr={stderr2}"
    );
    assert!(
        stdout2.contains("No change") || stdout2.contains("already at"),
        "idempotent stdout; stdout={stdout2}"
    );

    let dest_events2 = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM plan_events \
         WHERE plan_id='{plan}' AND scope='dest_process' AND destination='{dest}' \
           AND process_id='process_3_transportation' AND event='status_changed';"
    ))
    .unwrap();
    assert_eq!(dest_events2.scalar().as_deref(), Some("3"), "dest events unchanged");

    let tl_events2 = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM plan_events \
         WHERE plan_id='{plan}' AND scope='timeline' AND destination='' \
           AND process_id='process_3_transportation' AND event='status_changed';"
    ))
    .unwrap();
    assert_eq!(tl_events2.scalar().as_deref(), Some("3"), "timeline events unchanged");

    let op_count = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id='{plan}' AND command_type='set-process-status';"
    ))
    .unwrap();
    assert_eq!(
        op_count.scalar().as_deref(),
        Some("1"),
        "still one set-process-status operation; out={op_count}"
    );

    let version2 = db_exec(&format!(
        "SELECT version FROM plans WHERE plan_id='{plan}';"
    ))
    .unwrap();
    assert_eq!(version2.scalar().as_deref(), Some("8"), "version unchanged");

    // No-path failure: corrupted current value
    db_exec(&format!(
        "UPDATE process_statuses SET status='archived' \
         WHERE plan_id='{plan}' AND destination='{dest}' AND process_id='process_4_accommodation';"
    ))
    .unwrap();

    let events_before = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM plan_events \
         WHERE plan_id='{plan}' AND scope='dest_process' AND destination='{dest}' \
           AND process_id='process_4_accommodation' AND event='status_changed';"
    ))
    .unwrap();
    let ops_before = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id='{plan}';"
    ))
    .unwrap();

    let (ok_fail, _stdout_fail, stderr_fail) =
        run_set_status(&plan, &dest, "process_4_accommodation", "booked").expect("failure run");
    assert!(!ok_fail, "corrupted status should fail");
    assert!(
        stderr_fail.contains("unknown current status")
            || stderr_fail.contains("no legal status path"),
        "stderr should explain failure; stderr={stderr_fail}"
    );

    let events_after = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM plan_events \
         WHERE plan_id='{plan}' AND scope='dest_process' AND destination='{dest}' \
           AND process_id='process_4_accommodation' AND event='status_changed';"
    ))
    .unwrap();
    assert_eq!(
        events_after.scalar(),
        events_before.scalar(),
        "no P4 status_changed events added"
    );

    let ops_after = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id='{plan}';"
    ))
    .unwrap();
    assert_eq!(
        ops_after.scalar(),
        ops_before.scalar(),
        "no extra operation_runs"
    );
}

fn run_cmd_env(args: &[&str], envs: &[(&str, &str)]) -> Option<(bool, String, String)> {
    for attempt in 0..6 {
        let mut cmd = Command::new(bin());
        cmd.args(args).env_remove("TRAVEL_PLAN_ID");
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("run travel");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        if !out.status.success() && is_credless(&stderr) {
            eprintln!("skipping (no Turso creds): {}", stderr.trim());
            return None;
        }
        if !out.status.success() && is_transient(&stderr) && attempt < 5 {
            std::thread::sleep(std::time::Duration::from_millis(400 * (attempt + 1)));
            continue;
        }
        return Some((out.status.success(), stdout, stderr));
    }
    unreachable!()
}

fn scalar(sql: &str) -> Option<String> {
    db_exec(sql).and_then(|r| r.scalar())
}

/// Per-plan write surface the status machine touches: (P-status, status_changed
/// count for the process, operation_runs count, plans.version).
fn surface(plan: &str, dest: &str, process_id: &str) -> (Option<String>, String, String, String) {
    (
        scalar(&format!(
            "SELECT status AS v FROM process_statuses \
             WHERE plan_id='{plan}' AND destination='{dest}' AND process_id='{process_id}'"
        )),
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id='{plan}' \
               AND process_id='{process_id}' AND event='status_changed'"
        ))
        .unwrap_or_default(),
        scalar(&format!("SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id='{plan}'"))
            .unwrap_or_default(),
        scalar(&format!("SELECT version AS v FROM plans WHERE plan_id='{plan}'"))
            .unwrap_or_default(),
    )
}

fn seed_ladder(plan: &str, dest: &str, p5: &str) {
    seed_plan(plan, dest, 3);
    db_exec(&format!(
        "INSERT INTO process_statuses (plan_id, destination, process_id, status) VALUES \
         ('{plan}','{dest}','process_1_date_anchor','confirmed'), \
         ('{plan}','{dest}','process_2_destination','confirmed'), \
         ('{plan}','{dest}','process_3_transportation','pending'), \
         ('{plan}','{dest}','process_3_4_packages','pending'), \
         ('{plan}','{dest}','process_4_accommodation','pending'), \
         ('{plan}','{dest}','process_5_daily_itinerary','{p5}');"
    ))
    .expect("seed ladder");
}

#[test]
fn off_ladder_target_is_rejected_and_writes_nothing() {
    let _lock = SET_PROCESS_STATUS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}_sps_off");
    let dest = format!("zz_test_{tag}_d");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_ladder(&plan, &dest, "researching");
    let before = surface(&plan, &dest, "process_5_daily_itinerary");

    let Some((ok, stdout, stderr)) =
        run_cmd_env(&["set-process-status", "p5", "booked", "--dest", &dest, "--plan-id", &plan], &[])
    else {
        return;
    };
    assert!(!ok, "p5 booked must be refused; stdout={stdout} stderr={stderr}");
    assert!(
        stderr.contains("status booked is not on the itinerary ladder"),
        "stderr={stderr}"
    );
    assert_eq!(surface(&plan, &dest, "process_5_daily_itinerary"), before, "nothing written");
}

#[test]
fn p5_confirm_is_refused_on_a_publish_blocker() {
    let _lock = SET_PROCESS_STATUS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}_sps_gate");
    let dest = format!("zz_test_{tag}_d");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    // No itinerary days at all → the publish gate's [p5-days] blocker.
    seed_ladder(&plan, &dest, "researching");
    let before = surface(&plan, &dest, "process_5_daily_itinerary");
    assert_eq!(before.0.as_deref(), Some("researching"));

    let Some((ok, stdout, stderr)) = run_cmd_env(
        &["set-process-status", "p5", "confirmed", "--dest", &dest, "--plan-id", &plan],
        &[("TRAVEL_TODAY", "2026-07-05")],
    ) else {
        return;
    };
    assert!(!ok, "confirm must be refused; stdout={stdout} stderr={stderr}");
    assert!(stderr.contains("P5 confirm refused"), "stderr={stderr}");
    assert!(stderr.contains("[p5-days]"), "blockers listed; stderr={stderr}");
    assert_eq!(
        surface(&plan, &dest, "process_5_daily_itinerary"),
        before,
        "a refused confirm writes nothing"
    );
}

#[test]
fn p5_confirm_takes_two_hops_when_the_plan_is_clean() {
    let _lock = SET_PROCESS_STATUS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}_sps_ok");
    let dest = format!("zz_test_{tag}_d");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_ladder(&plan, &dest, "researching");
    // A past plan with days, a session and a route segment: no publish blocker
    // (past plans skip the ZH/weather/content-depth checks).
    db_exec(&format!(
        "INSERT INTO date_anchors (plan_id, destination, start_date, end_date, days) \
           VALUES ('{plan}', '{dest}', '2026-02-01', '2026-02-03', 3); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, theme) \
           VALUES ('{plan}', '{dest}', 1, '2026-02-01', 'arrival', 'Arrival'), \
                  ('{plan}', '{dest}', 2, '2026-02-02', 'full', 'Explore'), \
                  ('{plan}', '{dest}', 3, '2026-02-03', 'departure', 'Departure'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, focus) \
           VALUES ('{plan}', '{dest}', 2, 'morning', 'Museum'); \
         INSERT INTO day_route_segments \
           (plan_id, destination, day_number, sort_order, from_place, to_place, mode) \
           VALUES ('{plan}', '{dest}', 2, 0, 'Place A', 'Place B', 'walking');"
    ))
    .expect("seed clean itinerary");

    let Some((ok, stdout, stderr)) = run_cmd_env(
        &["set-process-status", "p5", "confirmed", "--dest", &dest, "--plan-id", &plan],
        &[("TRAVEL_TODAY", "2026-07-05")],
    ) else {
        return;
    };
    assert!(ok, "clean confirm should succeed; stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("researching -> researched -> confirmed"),
        "path printed; stdout={stdout}"
    );
    let hops = db_exec(&format!(
        "SELECT scope || ':' || from_state || '->' || to_state AS hop FROM plan_events \
         WHERE plan_id='{plan}' AND process_id='process_5_daily_itinerary' \
           AND event='status_changed' ORDER BY scope, sort_order;"
    ))
    .unwrap();
    assert_eq!(
        hops.column(),
        vec![
            "dest_process:researching->researched".to_string(),
            "dest_process:researched->confirmed".to_string(),
            "timeline:researching->researched".to_string(),
            "timeline:researched->confirmed".to_string(),
        ],
        "two hops in both buckets; out={hops}"
    );
    let (status, _, ops, version) = surface(&plan, &dest, "process_5_daily_itinerary");
    assert_eq!(status.as_deref(), Some("confirmed"));
    assert_eq!(ops, "1", "one audit row");
    assert_eq!(version, "4", "version 3 -> 4");
}

#[test]
fn failure_between_hops_rolls_back_the_whole_command() {
    let _lock = SET_PROCESS_STATUS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}_sps_fail");
    let dest = format!("zz_test_{tag}_d");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_ladder(&plan, &dest, "pending");
    let before = surface(&plan, &dest, "process_3_transportation");
    assert_eq!(before.0.as_deref(), Some("pending"));

    // pending -> populated -> booking are written, then the booking -> booked hop fails.
    let Some((ok, stdout, stderr)) = run_cmd_env(
        &["set-process-status", "p3", "booked", "--dest", &dest, "--plan-id", &plan],
        &[("TRAVEL_STATUS_FAIL_AT", "booked")],
    ) else {
        return;
    };
    assert!(!ok, "injected failure must fail the command; stdout={stdout}");
    assert!(stderr.contains("injected failure"), "stderr={stderr}");
    assert!(stderr.contains("rolled back"), "stderr={stderr}");
    assert_eq!(
        surface(&plan, &dest, "process_3_transportation"),
        before,
        "the two hops written before the failure must be rolled back"
    );
    let events = scalar(&format!(
        "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id='{plan}'"
    ));
    assert_eq!(events.as_deref(), Some("0"), "no plan_events at all");
}
