//! Real-Turso integration tests for `update-activity` (spec
//! docs/superpowers/specs/2026-11-19-cli-activity-stay-price-design.md §1) and
//! the shared record guard on `set-activity-title`. Fixtures are per-run unique
//! (`zz_test_<nanos>`); the RAII `Guard` is armed right after the plan id is
//! bound. Skips cleanly without Turso creds.

use std::process::Command;

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn exec_ok(sql: &str) -> common::Rows {
    db_exec(sql).unwrap_or_else(|| panic!("db exec skipped unexpectedly for SQL: {sql}"))
}

fn n(sql: &str) -> String {
    exec_ok(sql).scalar().unwrap_or_default()
}

fn creds() -> bool {
    db_exec("SELECT 1 AS n").is_some()
}

fn run_cmd(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .env_remove("TRAVEL_TODAY")
        .output()
        .expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Day 1 morning: `{plan}_a` 鼻頭角 (poi, 60min, 09:00, notes, want),
/// `{plan}_b` 龍洞灣 (plain), `{plan}_c` 鼻頭角步道 (so "鼻頭角" is ambiguous in
/// afternoon), plus a recorded row `{plan}_r`. Day 9 is a variant day.
fn seed(plan: &str, dest: &str) {
    seed_plan(plan, dest, 5);
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan}', '{dest}', 1, '2026-01-01', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 1, 'morning'), ('{plan}', '{dest}', 1, 'afternoon'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, \
             title, nearest_station, duration_min, start_time, notes, priority, poi_id, updated_at) \
           VALUES ('{plan}_a', '{plan}', '{dest}', 1, 'morning', 3, '鼻頭角', '龍洞', 60, '09:00', \
                   '舊備註', 'want', '{plan}_poi', '2000-01-01 00:00:00'), \
                  ('{plan}_b', '{plan}', '{dest}', 1, 'morning', 4, '龍洞灣', NULL, NULL, NULL, NULL, 'want', NULL, '2000-01-01 00:00:00'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title, duration_min) \
           VALUES ('{plan}_c', '{plan}', '{dest}', 1, 'afternoon', 0, '鼻頭角步道', 30), \
                  ('{plan}_d', '{plan}', '{dest}', 1, 'afternoon', 1, '鼻頭角燈塔', 30); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, \
             title, duration_min, record_status, record_note, poi_id) \
           VALUES ('{plan}_r', '{plan}', '{dest}', 1, 'morning', 5, '東寺', 40, 'done', '去了', '{plan}_poi2');"
    ));
}

fn version(plan: &str) -> String {
    n(&format!("SELECT version AS n FROM plans WHERE plan_id = '{plan}'"))
}

fn events(plan: &str, ev: &str) -> String {
    n(&format!(
        "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}' AND event = '{ev}'"
    ))
}

macro_rules! fixture {
    ($plan:ident, $dest:ident) => {
        let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        if !creds() {
            eprintln!("skipping update_activity test (no Turso creds)");
            return;
        }
        let tag = nanos();
        let $plan = format!("zz_test_{tag}");
        let $dest = format!("zz_test_{tag}");
        let _g = Guard::new({
            let (plan, dest) = ($plan.clone(), $dest.clone());
            move || teardown_plan(&plan, &dest)
        });
        seed(&$plan, &$dest);
    };
}

#[test]
fn updates_given_fields_only_and_keeps_identity() {
    fixture!(plan, dest);
    let (ok, stdout, stderr) = run_cmd(&[
        "update-activity", "1", "morning", "鼻頭角", "--duration", "45", "--area", "鼻頭",
        "--station", "福隆", "--priority", "must", "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "update failed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("duration_min: 60 → 45"), "{stdout}");
    assert!(stdout.contains("area: (none) → 鼻頭"), "{stdout}");
    let row = |col: &str| n(&format!("SELECT {col} AS v FROM activities WHERE id = '{plan}_a'"));
    assert_eq!(row("duration_min"), "45");
    assert_eq!(row("area"), "鼻頭");
    assert_eq!(row("nearest_station"), "福隆");
    assert_eq!(row("priority"), "must");
    // untouched
    assert_eq!(row("title"), "鼻頭角");
    assert_eq!(row("start_time"), "09:00");
    assert_eq!(row("notes"), "舊備註");
    assert_eq!(row("sort_order"), "3");
    assert_eq!(row("poi_id"), format!("{plan}_poi"));
    assert_eq!(row("id"), format!("{plan}_a"));
    assert_ne!(row("updated_at"), "2000-01-01 00:00:00");
    // the sibling row is untouched
    assert_eq!(
        n(&format!("SELECT updated_at AS v FROM activities WHERE id = '{plan}_b'")),
        "2000-01-01 00:00:00"
    );
}

#[test]
fn clear_sets_null() {
    fixture!(plan, dest);
    let (ok, stdout, stderr) = run_cmd(&[
        "update-activity", "1", "morning", &format!("{plan}_a"), "--clear", "start,notes",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "clear failed; stdout={stdout} stderr={stderr}");
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM activities WHERE id = '{plan}_a' \
             AND start_time IS NULL AND notes IS NULL AND duration_min = 60 AND poi_id = '{plan}_poi'"
        )),
        "1"
    );
}

#[test]
fn audit_triad_is_complete() {
    fixture!(plan, dest);
    assert_eq!(version(&plan), "5");
    let (ok, stdout, stderr) = run_cmd(&[
        "update-activity", "1", "morning", "鼻頭角", "--duration", "45", "--clear", "notes",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "update failed; stdout={stdout} stderr={stderr}");
    assert_eq!(version(&plan), "6");
    // both event groups (dest_process + timeline)
    assert_eq!(events(&plan, "activity_updated"), "2");
    assert_eq!(
        n(&format!(
            "SELECT COUNT(DISTINCT scope) AS n FROM plan_events WHERE plan_id = '{plan}' AND event = 'activity_updated'"
        )),
        "2"
    );
    let kv = |k: &str| {
        n(&format!(
            "SELECT value AS v FROM plan_event_data WHERE plan_id = '{plan}' AND scope = 'timeline' AND key = '{k}'"
        ))
    };
    assert_eq!(kv("from.duration_min"), "60");
    assert_eq!(kv("to.duration_min"), "45");
    assert_eq!(kv("from.notes"), "舊備註");
    assert_eq!(kv("to.notes"), "undefined");
    assert_eq!(kv("changed"), "duration_min,notes");
    assert_eq!(kv("activity_id"), format!("{plan}_a"));
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = '{plan}' \
             AND command_type = 'update-activity' AND status = 'completed' \
             AND version_before = 5 AND version_after = 6"
        )),
        "1"
    );
}

#[test]
fn same_values_rerun_is_a_noop() {
    fixture!(plan, dest);
    let args = [
        "update-activity", "1", "morning", "鼻頭角", "--duration", "60", "--priority", "want",
        "--clear", "end", "--plan-id", &plan, "--dest", &dest,
    ];
    let (ok, stdout, stderr) = run_cmd(&args);
    assert!(ok, "no-op must succeed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("No change"), "{stdout}");
    assert_eq!(version(&plan), "5");
    assert_eq!(events(&plan, "activity_updated"), "0");
    assert_eq!(
        n(&format!("SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = '{plan}' AND command_type = 'update-activity'")),
        "0"
    );
    assert_eq!(
        n(&format!("SELECT updated_at AS v FROM activities WHERE id = '{plan}_a'")),
        "2000-01-01 00:00:00"
    );
}

#[test]
fn ambiguous_match_fails_with_zero_change() {
    fixture!(plan, dest);
    let (ok, _o, stderr) = run_cmd(&[
        "update-activity", "1", "afternoon", "鼻頭角", "--duration", "99",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(!ok, "ambiguous match must fail");
    assert!(stderr.contains("鼻頭角步道") && stderr.contains("鼻頭角燈塔"), "candidates listed: {stderr}");
    assert_eq!(
        n(&format!("SELECT COUNT(*) AS n FROM activities WHERE plan_id = '{plan}' AND duration_min = 99")),
        "0"
    );
    assert_eq!(version(&plan), "5");
    assert_eq!(events(&plan, "activity_updated"), "0");
}

#[test]
fn recorded_row_allows_fact_edits_but_refuses_title() {
    fixture!(plan, dest);
    let (ok, stdout, stderr) = run_cmd(&[
        "update-activity", "1", "morning", "東寺", "--duration", "25", "--start", "10:30",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "fact edit on a recorded row must work; stdout={stdout} stderr={stderr}");
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM activities WHERE id = '{plan}_r' AND duration_min = 25 \
             AND start_time = '10:30' AND record_status = 'done' AND record_note = '去了'"
        )),
        "1"
    );
    let v = version(&plan);
    let (ok, _o, stderr) = run_cmd(&[
        "update-activity", "1", "morning", "東寺", "--title", "別處", "--duration", "99",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(!ok, "title edit on a recorded row must be refused");
    assert!(stderr.contains("record-activity") && stderr.contains("clear-record"), "{stderr}");
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM activities WHERE id = '{plan}_r' AND title = '東寺' AND duration_min = 25 AND poi_id = '{plan}_poi2'"
        )),
        "1",
        "refused edit must change nothing (including the other flags)"
    );
    assert_eq!(version(&plan), v);
}

#[test]
fn set_activity_title_shares_the_record_guard() {
    fixture!(plan, dest);
    let (ok, _o, stderr) = run_cmd(&[
        "set-activity-title", "1", "morning", &format!("{plan}_r"), "別處",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(!ok, "set-activity-title on a recorded row must fail");
    assert!(stderr.contains("record-activity"), "{stderr}");
    assert_eq!(
        n(&format!("SELECT COUNT(*) AS n FROM activities WHERE id = '{plan}_r' AND title = '東寺' AND poi_id = '{plan}_poi2'")),
        "1"
    );
    assert_eq!(version(&plan), "5");
    // same title on a recorded row = not a change, not refused
    let (ok, stdout, stderr) = run_cmd(&[
        "set-activity-title", "1", "morning", &format!("{plan}_r"), "東寺",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "same-title rerun must not be refused; stdout={stdout} stderr={stderr}");
    // unrecorded row: behaviour unchanged (retitles, clears poi)
    let (ok, stdout, stderr) = run_cmd(&[
        "set-activity-title", "1", "morning", &format!("{plan}_a"), "野柳",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "unrecorded retitle must still work; stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains(&format!("--match \"野柳\" --plan-id {plan} --dest {dest}")),
        "set-activity-title relink hint must be copy-pasteable: {stdout}"
    );
    assert_eq!(
        n(&format!("SELECT COUNT(*) AS n FROM activities WHERE id = '{plan}_a' AND title = '野柳' AND poi_id IS NULL")),
        "1"
    );
}

#[test]
fn retitle_unrecorded_row_clears_poi_and_prints_note() {
    fixture!(plan, dest);
    let (ok, stdout, stderr) = run_cmd(&[
        "update-activity", "1", "morning", "鼻頭角", "--title", "野柳地質公園",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "retitle failed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("note: cleared poi_id") && stdout.contains(&format!("{plan}_poi")), "{stdout}");
    assert!(
        stdout.contains(&format!("--match \"野柳地質公園\" --plan-id {plan} --dest {dest}")),
        "relink hint must be copy-pasteable: {stdout}"
    );
    assert_eq!(
        n(&format!("SELECT COUNT(*) AS n FROM activities WHERE id = '{plan}_a' AND title = '野柳地質公園' AND poi_id IS NULL")),
        "1"
    );
    // audit records the poi clear (changed + from./to. KV)
    let kv = |k: &str| {
        n(&format!(
            "SELECT value AS v FROM plan_event_data WHERE plan_id = '{plan}' AND scope = 'timeline' AND key = '{k}'"
        ))
    };
    assert_eq!(kv("changed"), "title,poi_id");
    assert_eq!(kv("from.poi_id"), format!("{plan}_poi"));
    assert_eq!(kv("to.poi_id"), "undefined");
}

#[test]
fn same_value_rerun_is_no_change_even_on_dirty_stored_row() {
    fixture!(plan, dest);
    exec_ok(&format!(
        "INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, \
             title, start_time, end_time, updated_at) \
           VALUES ('{plan}_e', '{plan}', '{dest}', 1, 'noon', 0, \
             'x https://www.google.com/maps/dir/?api=1&origin=a&destination=b', '10:00', '09:00', '2000-01-01 00:00:00');"
    ));
    let (ok, stdout, stderr) = run_cmd(&[
        "update-activity", "1", "noon", &format!("{plan}_e"), "--start", "10:00", "--end", "09:00",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "same-value rerun must be No change, not a validation error; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("No change"), "{stdout}");
    assert_eq!(version(&plan), "5");
}

#[test]
fn dirty_stored_endpoint_fails_loud_when_other_endpoint_changes() {
    fixture!(plan, dest);
    exec_ok(&format!(
        "UPDATE activities SET start_time = '9am' WHERE id = '{plan}_a'"
    ));
    let (ok, _o, stderr) = run_cmd(&[
        "update-activity", "1", "morning", &format!("{plan}_a"), "--end", "08:00",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(!ok, "dirty stored start + new --end must fail loud");
    assert!(stderr.contains("9am"), "{stderr}");
    assert_eq!(n(&format!("SELECT COUNT(*) AS n FROM activities WHERE id = '{plan}_a' AND end_time IS NULL")), "1");
    assert_eq!(version(&plan), "5");
}

#[test]
fn works_on_variant_day() {
    fixture!(plan, dest);
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, variant_of) \
           VALUES ('{plan}', '{dest}', 9, '2026-01-01', 'full', 1); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 9, 'noon'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) \
           VALUES ('{plan}_v', '{plan}', '{dest}', 9, 'noon', 0, '雨天替代：博物館');"
    ));
    let (ok, stdout, stderr) = run_cmd(&[
        "update-activity", "9", "noon", "博物館", "--duration", "90",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "variant day edit must work; stdout={stdout} stderr={stderr}");
    assert_eq!(n(&format!("SELECT duration_min AS v FROM activities WHERE id = '{plan}_v'")), "90");
}

#[test]
fn validation_failure_writes_nothing() {
    fixture!(plan, dest);
    // --end earlier than the STORED start (09:00): caught only on the merged row.
    let (ok, _o, stderr) = run_cmd(&[
        "update-activity", "1", "morning", "鼻頭角", "--duration", "99", "--end", "08:00",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(!ok, "merged start>end must fail");
    assert!(stderr.contains("after"), "{stderr}");
    // broken Maps URL title
    let (ok, _o, _e) = run_cmd(&[
        "update-activity", "1", "morning", "鼻頭角", "--duration", "99",
        "--title", "x https://www.google.com/maps/dir/?api=1&origin=a&destination=b",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(!ok, "map-url title must fail");
    // flag-level failures
    for bad in [
        vec!["--duration", "0"],
        vec!["--priority", "critical"],
        vec!["--clear", "title"],
        vec!["--area", "x", "--clear", "area"],
    ] {
        let mut a = vec!["update-activity", "1", "morning", "鼻頭角", "--plan-id", &plan, "--dest", &dest];
        a.extend(bad.iter());
        let (ok, _o, _e) = run_cmd(&a);
        assert!(!ok, "{bad:?} must fail");
    }
    assert_eq!(
        n(&format!("SELECT COUNT(*) AS n FROM activities WHERE plan_id = '{plan}' AND updated_at <> '2000-01-01 00:00:00' AND id IN ('{plan}_a','{plan}_b')")),
        "0"
    );
    assert_eq!(n(&format!("SELECT duration_min AS v FROM activities WHERE id = '{plan}_a'")), "60");
    assert_eq!(version(&plan), "5");
    assert_eq!(events(&plan, "activity_updated"), "0");
}
