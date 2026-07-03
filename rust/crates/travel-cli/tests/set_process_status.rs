//! Real-Turso behavior-LOCK integration test for `set-process-status`.
//!
//! Seeds a unique plan with process_statuses ladder rows, runs
//! `set-process-status process_3_transportation booked`, asserts the shortest
//! legal path (pending -> populated -> booking -> booked), dual-bucket events,
//! operation_runs audit, and version bump. Also locks idempotent re-run and
//! no-path failure on corrupted status.
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