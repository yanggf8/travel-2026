//! Real-Turso integration tests for the trip-record CLI commands
//! (`record-activity` / `record-meal` / `record-leg` / `record-session` /
//! `clear-record`, docs/superpowers/specs/2026-11-16-trip-record-design.md §5).
//!
//! Fixtures are per-run unique (`zz_test_<nanos>`); the RAII `Guard` is armed
//! right after the plan id is bound. `TRAVEL_TODAY` pins the destination-local
//! "today" so the future-day guard is deterministic. Skips cleanly without
//! Turso creds.

use std::process::Command;

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A fixed "today" (the plan/seed dates are all in the past relative to it).
const TODAY: &str = "2026-11-17";

fn exec_ok(sql: &str) -> common::Rows {
    db_exec(sql).unwrap_or_else(|| panic!("db exec skipped unexpectedly for SQL: {sql}"))
}

fn n(sql: &str) -> String {
    exec_ok(sql).scalar().unwrap_or_default()
}

/// Run the travel binary with `TRAVEL_TODAY` pinned and `TRAVEL_PLAN_ID` removed.
/// Returns (ok, stdout, stderr).
fn run_cmd(args: &[&str], today: Option<&str>) -> (bool, String, String) {
    let mut cmd = Command::new(bin());
    cmd.args(args).env_remove("TRAVEL_PLAN_ID");
    match today {
        Some(t) => cmd.env("TRAVEL_TODAY", t),
        None => cmd.env_remove("TRAVEL_TODAY"),
    };
    let out = cmd.output().expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Seed a main day + noon session + one activity titled 東寺. Returns its id.
fn seed_day_with_activity(plan: &str, dest: &str, day: i64, date: &str, id_suffix: &str) -> String {
    let id = format!("{plan}_{id_suffix}");
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan}', '{dest}', {day}, '{date}', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', {day}, 'noon'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) \
           VALUES ('{id}', '{plan}', '{dest}', {day}, 'noon', 0, '東寺');"
    ));
    id
}

fn creds() -> bool {
    db_exec("SELECT 1 AS n").is_some()
}

fn version(plan: &str) -> String {
    n(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'"))
}

fn events(plan: &str, event: &str) -> String {
    n(&format!(
        "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}' AND event = '{event}'"
    ))
}

fn ops(plan: &str, command_type: &str) -> String {
    n(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = '{plan}' AND command_type = '{command_type}'"
    ))
}

#[test]
fn record_activity_writes_fields_and_triad() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_cli test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    let id = seed_day_with_activity(&plan, &dest, 1, "2026-11-10", "a");

    let (ok, stdout, stderr) = run_cmd(
        &[
            "record-activity", "1", "noon", "東寺", "done",
            "--note", "人少，多待了一小時",
            "--plan-id", &plan, "--dest", &dest,
        ],
        Some(TODAY),
    );
    assert!(ok, "record-activity failed; stdout={stdout} stderr={stderr}");

    // Fields written.
    assert_eq!(
        n(&format!(
            "SELECT record_status AS v FROM activities WHERE id = '{id}'"
        )),
        "done"
    );
    assert_eq!(
        n(&format!(
            "SELECT record_note AS v FROM activities WHERE id = '{id}'"
        )),
        "人少，多待了一小時"
    );
    // Audit triad: version 0 → 1, one record_marked event pair, one operation_runs row.
    assert_eq!(version(&plan), "1");
    assert_eq!(events(&plan, "record_marked"), "2"); // dest_process + timeline
    assert_eq!(ops(&plan, "record-activity"), "1");
}

#[test]
fn rerun_same_value_no_bump() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_cli test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    seed_day_with_activity(&plan, &dest, 1, "2026-11-10", "a");
    for args in [
        vec!["record-activity", "1", "noon", "東寺", "done", "--plan-id", &plan, "--dest", &dest],
        vec!["record-activity", "1", "noon", "東寺", "done", "--plan-id", &plan, "--dest", &dest],
    ] {
        let (ok, _, stderr) = run_cmd(&args, Some(TODAY));
        assert!(ok, "same-value rerun must still exit 0; stderr={stderr}");
    }
    assert_eq!(version(&plan), "1", "same-value rerun must NOT bump version");
    assert_eq!(events(&plan, "record_marked"), "2");
    assert_eq!(ops(&plan, "record-activity"), "1");
}

#[test]
fn future_day_rejected() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_cli test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    let id = seed_day_with_activity(&plan, &dest, 3, "2026-12-25", "a"); // after TODAY

    let (ok, stdout, stderr) = run_cmd(
        &["record-activity", "3", "noon", "東寺", "done", "--plan-id", &plan, "--dest", &dest],
        Some(TODAY),
    );
    assert!(!ok, "future day must be rejected");
    assert!(stderr.contains("future"), "stderr must say why: {stderr}");
    // Nothing moved: no version bump, no mark, no events.
    assert_eq!(version(&plan), "0");
    assert_eq!(
        n(&format!("SELECT COUNT(*) AS n FROM activities WHERE id = '{id}' AND record_status IS NOT NULL")),
        "0"
    );
    assert_eq!(events(&plan, "record_marked"), "0");
    assert_eq!(ops(&plan, "record-activity"), "0");
    let _ = stdout;
}

#[test]
fn variant_row_rejected() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_cli test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    let id = format!("{plan}_v");
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, variant_of) \
           VALUES ('{plan}', '{dest}', 9, '2026-11-10', 'full', 1); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 9, 'noon'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) \
           VALUES ('{id}', '{plan}', '{dest}', 9, 'noon', 0, '東寺');"
    ));

    let (ok, _, stderr) = run_cmd(
        &["record-activity", "9", "noon", "東寺", "done", "--plan-id", &plan, "--dest", &dest],
        Some(TODAY),
    );
    assert!(!ok, "variant row must be rejected");
    assert!(
        stderr.contains("variant"),
        "stderr must name the variant refusal: {stderr}"
    );
    assert_eq!(
        n(&format!("SELECT COUNT(*) AS n FROM activities WHERE id = '{id}' AND record_status IS NOT NULL")),
        "0"
    );
    assert_eq!(version(&plan), "0");
}

#[test]
fn missing_timezone_fails_loud() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_cli test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    // No destination_config row for this dest (timezone NOT NULL in schema — the
    // "NULL timezone" case in practice is a missing registration row).
    seed_day_with_activity(&plan, &dest, 1, "2026-11-10", "a");

    // No TRAVEL_TODAY, timezone NULL → must fail loud naming destination_config.timezone.
    let (ok, _, stderr) = run_cmd(
        &["record-activity", "1", "noon", "東寺", "done", "--plan-id", &plan, "--dest", &dest],
        None,
    );
    assert!(!ok, "NULL timezone must fail");
    assert!(
        stderr.contains("destination_config.timezone"),
        "stderr must name destination_config.timezone: {stderr}"
    );
    assert_eq!(version(&plan), "0");
}

#[test]
fn clear_record_resets_both() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_cli test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    let id = seed_day_with_activity(&plan, &dest, 1, "2026-11-10", "a");
    let (ok, _, stderr) = run_cmd(
        &[
            "record-activity", "1", "noon", "東寺", "skipped",
            "--note", "排太滿跳過",
            "--plan-id", &plan, "--dest", &dest,
        ],
        Some(TODAY),
    );
    assert!(ok, "mark failed: {stderr}");
    assert_eq!(version(&plan), "1");

    let (ok, stdout, stderr) = run_cmd(
        &["clear-record", "activity", "1", "noon", "東寺", "--plan-id", &plan, "--dest", &dest],
        Some(TODAY),
    );
    assert!(ok, "clear-record failed; stdout={stdout} stderr={stderr}");
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM activities WHERE id = '{id}' \
             AND record_status IS NULL AND record_note IS NULL"
        )),
        "1",
        "status AND note must both be NULL again"
    );
    assert_eq!(events(&plan, "record_cleared"), "2");
    assert_eq!(version(&plan), "2", "clear is a real change → one bump");
}

#[test]
fn record_marks_maps_stale() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_cli test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    // POI-linked activity (map-bearing input for check-maps-fresh).
    let id = format!("{plan}_a");
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, updated_at) \
           VALUES ('{plan}', '{dest}', 1, '2026-11-10', 'full', datetime('now','-1 hour')); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 1, 'noon'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title, poi_id, updated_at) \
           VALUES ('{id}', '{plan}', '{dest}', 1, 'noon', 0, '東寺', '{plan}_poi', datetime('now','-1 hour'));"
    ));

    // Snapshot (stamps snapshotted_at = now), then push the stamp back so the
    // mark's updated_at = now is deterministically newer.
    let (ok, _, stderr) = run_cmd(&["mark-maps-snapshotted", &plan], Some(TODAY));
    assert!(ok, "mark-maps-snapshotted failed: {stderr}");
    exec_ok(&format!(
        "UPDATE plan_map_snapshots SET snapshotted_at = datetime('now','-1 hour') WHERE plan_id = '{plan}';"
    ));
    let (_, stdout, _) = run_cmd(&["check-maps-fresh", "--plan-id", &plan], Some(TODAY));
    assert!(stdout.contains("maps fresh"), "fresh before marking; got:\n{stdout}");

    // Mark the POI-linked activity → its updated_at moves to now → STALE.
    let (ok, _, stderr) = run_cmd(
        &["record-activity", "1", "noon", "東寺", "done", "--plan-id", &plan, "--dest", &dest],
        Some(TODAY),
    );
    assert!(ok, "record-activity failed: {stderr}");
    let (ok, stdout, _) = run_cmd(&["check-maps-fresh", "--plan-id", &plan], Some(TODAY));
    assert!(ok, "check-maps-fresh always exits 0");
    assert!(
        stdout.contains("STALE") && stdout.contains(&plan),
        "marking a map-bearing activity must make the maps stale; got:\n{stdout}"
    );
}

/// Minimal happy-path proof for the three commands T3 adds beyond
/// record-activity (each new command gets at least one test).
#[test]
fn record_meal_leg_session_write_fields_and_triad() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_cli test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan}', '{dest}', 1, '2026-11-10', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 1, 'noon'); \
         INSERT INTO session_meals (plan_id, destination, day_number, session_type, sort_order, meal) \
           VALUES ('{plan}', '{dest}', 1, 'noon', 0, 'ラーメン'), \
                  ('{plan}', '{dest}', 1, 'noon', 1, '牛カツ'); \
         INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode) \
           VALUES ('{plan}', '{dest}', 1, 0, '京都駅', '東寺', 'walk');"
    ));

    let (ok, _, stderr) = run_cmd(
        &["record-meal", "1", "noon", "ラーメン", "done", "--plan-id", &plan, "--dest", &dest],
        Some(TODAY),
    );
    assert!(ok, "record-meal failed: {stderr}");
    assert_eq!(
        n(&format!(
            "SELECT record_status AS v FROM session_meals \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 \
               AND session_type = 'noon' AND sort_order = 0"
        )),
        "done"
    );

    let (ok, _, stderr) = run_cmd(
        &["record-leg", "1", "0", "skipped", "--note", "改搭公車", "--plan-id", &plan, "--dest", &dest],
        Some(TODAY),
    );
    assert!(ok, "record-leg failed: {stderr}");
    assert_eq!(
        n(&format!(
            "SELECT record_status AS v FROM day_route_segments \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 AND sort_order = 0"
        )),
        "skipped"
    );

    let (ok, _, stderr) = run_cmd(
        &[
            "record-session", "1", "noon", "--zh", "中午在東寺周邊吃拉麵",
            "--plan-id", &plan, "--dest", &dest,
        ],
        Some(TODAY),
    );
    assert!(ok, "record-session failed: {stderr}");
    assert_eq!(
        n(&format!(
            "SELECT record_summary_zh AS v FROM timesofday \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 \
               AND session_type = 'noon'"
        )),
        "中午在東寺周邊吃拉麵"
    );

    // Each command wrote its own audit row.
    assert_eq!(ops(&plan, "record-meal"), "1");
    assert_eq!(ops(&plan, "record-leg"), "1");
    assert_eq!(ops(&plan, "record-session"), "1");
    assert_eq!(version(&plan), "3");
}

/// `record-session --clear` is a CLEAR, not a mark: it must emit the
/// `record_cleared` event pair (never `record_marked`) and bump the version
/// once. The summary is seeded raw so the clear is the plan's only CLI write.
#[test]
fn record_session_clear_emits_record_cleared() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_cli test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan}', '{dest}', 1, '2026-11-10', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, record_summary_zh) \
           VALUES ('{plan}', '{dest}', 1, 'noon', '中午在東寺周邊吃拉麵');"
    ));

    let (ok, stdout, stderr) = run_cmd(
        &["record-session", "1", "noon", "--clear", "--plan-id", &plan, "--dest", &dest],
        Some(TODAY),
    );
    assert!(ok, "record-session --clear failed; stdout={stdout} stderr={stderr}");

    // The summary is gone.
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM timesofday \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 \
               AND session_type = 'noon' AND record_summary_zh IS NULL"
        )),
        "1"
    );
    // The event pair is record_cleared (dest_process + timeline)...
    assert_eq!(
        events(&plan, "record_cleared"),
        "2",
        "--clear must emit record_cleared (dest_process + timeline)"
    );
    // ...and NOT record_marked.
    assert_eq!(
        events(&plan, "record_marked"),
        "0",
        "--clear must never emit record_marked"
    );
    assert_eq!(version(&plan), "1", "the clear is a real change → one bump");
    assert_eq!(ops(&plan, "record-session"), "1");
}

/// Clearing a skipped leg changes the drawn map (day_route_segments has no
/// updated_at column), so `clear-record leg` must touch the owning day — the
/// check-maps-fresh staleness hint has to fire. The record is seeded raw
/// because `record-leg` itself touches the day.
#[test]
fn clear_record_leg_marks_maps_stale() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_cli test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, updated_at) \
           VALUES ('{plan}', '{dest}', 1, '2026-11-10', 'full', datetime('now','-2 hour')); \
         INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode, record_status) \
           VALUES ('{plan}', '{dest}', 1, 0, '京都駅', '東寺', 'walk', 'skipped');"
    ));

    // Snapshot (stamps now), then push the stamp back 1h: days.updated_at
    // (-2h) is older than the stamp (-1h) → maps read fresh before the clear.
    let (ok, _, stderr) = run_cmd(&["mark-maps-snapshotted", &plan], Some(TODAY));
    assert!(ok, "mark-maps-snapshotted failed: {stderr}");
    exec_ok(&format!(
        "UPDATE plan_map_snapshots SET snapshotted_at = datetime('now','-1 hour') WHERE plan_id = '{plan}';"
    ));
    let (_, stdout, _) = run_cmd(&["check-maps-fresh", "--plan-id", &plan], Some(TODAY));
    assert!(stdout.contains("maps fresh"), "fresh before clearing; got:\n{stdout}");

    let (ok, stdout, stderr) = run_cmd(
        &["clear-record", "leg", "1", "0", "--plan-id", &plan, "--dest", &dest],
        Some(TODAY),
    );
    assert!(ok, "clear-record leg failed; stdout={stdout} stderr={stderr}");
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM day_route_segments \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 \
               AND sort_order = 0 AND record_status IS NULL"
        )),
        "1",
        "the leg record must be cleared"
    );

    // The day touch must have moved days.updated_at past the snapshot → STALE.
    let (ok, stdout, _) = run_cmd(&["check-maps-fresh", "--plan-id", &plan], Some(TODAY));
    assert!(ok, "check-maps-fresh always exits 0");
    assert!(
        stdout.contains("STALE") && stdout.contains(&plan),
        "clearing a skipped leg changes the drawn map — the staleness hint must fire; got:\n{stdout}"
    );
}
