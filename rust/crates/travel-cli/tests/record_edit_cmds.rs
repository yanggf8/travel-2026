//! Real-Turso integration tests for the trip-record edit commands
//! (`add-meal` / `set-meal` / `set-activity-notes`,
//! docs/superpowers/specs/2026-11-16-trip-record-design.md §5 G4): the edits
//! that used to force delete-and-recreate (churning activity ids) now write
//! in place, audited. Fixtures are per-run unique (`zz_test_<nanos>`); RAII
//! `Guard` armed right after the plan id is bound. Skips cleanly without
//! Turso creds.

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

fn seed_plan_with_session(plan: &str, dest: &str) {
    seed_plan(plan, dest, 0);
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan}', '{dest}', 1, '2026-01-01', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 1, 'noon'); \
         INSERT INTO session_meals (plan_id, destination, day_number, session_type, sort_order, meal) \
           VALUES ('{plan}', '{dest}', 1, 'noon', 0, 'ラーメン'), \
                  ('{plan}', '{dest}', 1, 'noon', 1, '牛カツ'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title, poi_id) \
           VALUES ('{plan}_a', '{plan}', '{dest}', 1, 'noon', 0, '東寺', '{plan}_poi');"
    ));
}

#[test]
fn add_meal_appends_not_replaces() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_edit_cmds test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan_with_session(&plan, &dest);

    let (ok, stdout, stderr) = run_cmd(&[
        "add-meal", "1", "noon", "--meal", "抹茶甜點", "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "add-meal failed; stdout={stdout} stderr={stderr}");

    // 3 rows now; the first 2 keep their text AND sort_order (append, not replace).
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM session_meals \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 AND session_type = 'noon'"
        )),
        "3"
    );
    assert_eq!(
        n(&format!(
            "SELECT meal AS v FROM session_meals \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 \
               AND session_type = 'noon' AND sort_order = 0"
        )),
        "ラーメン"
    );
    assert_eq!(
        n(&format!(
            "SELECT meal AS v FROM session_meals \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 \
               AND session_type = 'noon' AND sort_order = 1"
        )),
        "牛カツ"
    );
    assert_eq!(
        n(&format!(
            "SELECT meal AS v FROM session_meals \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 \
               AND session_type = 'noon' AND sort_order = 2"
        )),
        "抹茶甜點"
    );
    // Audited.
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = '{plan}' AND command_type = 'add-meal'"
        )),
        "1"
    );
}

#[test]
fn set_meal_updates_text_in_place() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_edit_cmds test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan_with_session(&plan, &dest);

    let (ok, stdout, stderr) = run_cmd(&[
        "set-meal", "1", "noon", "牛カツ", "--meal", "牛カツ御膳",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "set-meal failed; stdout={stdout} stderr={stderr}");

    // Exactly the matched row changed, in place (same slot); the other untouched.
    assert_eq!(
        n(&format!(
            "SELECT meal AS v FROM session_meals \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 \
               AND session_type = 'noon' AND sort_order = 1"
        )),
        "牛カツ御膳"
    );
    assert_eq!(
        n(&format!(
            "SELECT meal AS v FROM session_meals \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 \
               AND session_type = 'noon' AND sort_order = 0"
        )),
        "ラーメン"
    );
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM session_meals \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 AND session_type = 'noon'"
        )),
        "2",
        "set-meal must not change the row count"
    );
}

#[test]
fn set_meal_ambiguous_fails() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_edit_cmds test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan_with_session(&plan, &dest);
    // Make the needle "カツ" ambiguous: 牛カツ (existing) + a new カツサンド.
    exec_ok(&format!(
        "INSERT INTO session_meals (plan_id, destination, day_number, session_type, sort_order, meal) \
           VALUES ('{plan}', '{dest}', 1, 'noon', 2, 'カツサンド');"
    ));

    let version_before = n(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'"));
    let (ok, _stdout, stderr) = run_cmd(&[
        "set-meal", "1", "noon", "カツ", "--meal", "哪一家？",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(!ok, "ambiguous match must fail");
    assert!(stderr.contains("matches 2"), "must list both candidates: {stderr}");
    assert_eq!(
        n(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'")),
        version_before,
        "a failed set-meal must not bump version"
    );
    // No row's text changed.
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM session_meals \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND meal = '哪一家？'"
        )),
        "0"
    );
}

#[test]
fn set_activity_notes_keeps_id_and_poi() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_edit_cmds test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan_with_session(&plan, &dest);

    let (ok, stdout, stderr) = run_cmd(&[
        "set-activity-notes", "1", "noon", "東寺", "--notes", "五重塔整修中，只看了外觀",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "set-activity-notes failed; stdout={stdout} stderr={stderr}");

    assert_eq!(
        n(&format!(
            "SELECT notes AS v FROM activities WHERE plan_id = '{plan}' AND id = '{plan}_a'"
        )),
        "五重塔整修中，只看了外觀"
    );
    // id and poi_id survive (the whole point: no delete-and-recreate id churn).
    assert_eq!(
        n(&format!("SELECT COUNT(*) AS n FROM activities WHERE plan_id = '{plan}' AND id = '{plan}_a' AND poi_id = '{plan}_poi'")),
        "1"
    );
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = '{plan}' AND command_type = 'set-activity-notes'"
        )),
        "1"
    );

    // --clear resets notes to NULL without touching id/poi.
    let (ok, stdout, stderr) = run_cmd(&[
        "set-activity-notes", "1", "noon", "東寺", "--clear",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "--clear failed; stdout={stdout} stderr={stderr}");
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM activities WHERE plan_id = '{plan}' AND id = '{plan}_a' \
             AND notes IS NULL AND poi_id = '{plan}_poi'"
        )),
        "1"
    );
}

#[test]
fn set_activity_notes_and_set_meal_work_on_variant_day() {
    // Variant days are alternative PLANS — editing them is normal planning
    // (only RECORDING is refused on variants). The unique-match resolvers must
    // not import the record path's main-day restriction.
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_edit_cmds test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan_with_session(&plan, &dest);
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, variant_of) \
           VALUES ('{plan}', '{dest}', 9, '2026-01-01', 'full', 1); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 9, 'noon'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) \
           VALUES ('{plan}_v', '{plan}', '{dest}', 9, 'noon', 0, '雨天替代：博物館'); \
         INSERT INTO session_meals (plan_id, destination, day_number, session_type, sort_order, meal) \
           VALUES ('{plan}', '{dest}', 9, 'noon', 0, '車站便當');"
    ));

    let (ok, stdout, stderr) = run_cmd(&[
        "set-activity-notes", "9", "noon", "博物館", "--notes", "已查好開放時間",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "notes edit on a variant day must work; stdout={stdout} stderr={stderr}");
    assert_eq!(
        n(&format!("SELECT notes AS v FROM activities WHERE id = '{plan}_v'")),
        "已查好開放時間"
    );

    let (ok, stdout, stderr) = run_cmd(&[
        "set-meal", "9", "noon", "車站便當", "--meal", "車站壽司",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "meal edit on a variant day must work; stdout={stdout} stderr={stderr}");
    assert_eq!(
        n(&format!(
            "SELECT meal AS v FROM session_meals WHERE plan_id = '{plan}' AND day_number = 9 AND sort_order = 0"
        )),
        "車站壽司"
    );
}
