//! Real-Turso tests for the trip-record overwrite guard (spec X3/X4,
//! docs/superpowers/specs/2026-11-16-trip-record-design.md §5): every command
//! that deletes / rebuilds / cross-day-moves itinerary rows must REFUSE when
//! the touched rows (or, for whole-day scopes, session summaries) carry trip
//! records — and leave plans.version, row counts and plan_events untouched.
//!
//! Recorded state is seeded via direct `db exec` UPDATEs (the record-* CLI
//! path is covered by record_cli.rs; a variant-day record is unreachable
//! through the CLI by design, so the guard rows are seeded raw here).
//! Fixtures are per-run unique (`zz_test_<nanos>`); RAII `Guard` armed right
//! after the plan id is bound. Skips cleanly without Turso creds.

use std::process::Command;

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

const TODAY: &str = "2026-11-17";

fn exec_ok(sql: &str) -> common::Rows {
    db_exec(sql).unwrap_or_else(|| panic!("db exec skipped unexpectedly for SQL: {sql}"))
}

fn n(sql: &str) -> String {
    exec_ok(sql).scalar().unwrap_or_default()
}

fn creds() -> bool {
    db_exec("SELECT 1 AS n").is_some()
}

/// Run the travel binary (TRAVEL_TODAY pinned, TRAVEL_PLAN_ID removed).
fn run_cmd(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .env("TRAVEL_TODAY", TODAY)
        .output()
        .expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Run a command that MUST be refused: non-zero exit, stderr naming the
/// recorded row (`marker`), and version / events unchanged.
fn expect_refused(args: &[&str], marker: &str, plan: &str) {
    let version_before = n(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'"));
    let events_before = n(&format!(
        "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}'"
    ));
    let (ok, stdout, stderr) = run_cmd(args);
    assert!(!ok, "must be refused; stdout={stdout} stderr={stderr}");
    assert!(
        stderr.contains(marker),
        "stderr must name the recorded row ({marker:?}): {stderr}"
    );
    assert_eq!(
        n(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'")),
        version_before,
        "a refused command must not bump plans.version"
    );
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}'"
        )),
        events_before,
        "a refused command must not write plan_events"
    );
}

/// Seed plan + main day 1 (noon + afternoon sessions) + one 東寺 activity.
fn seed_base(plan: &str, dest: &str) -> String {
    let id = format!("{plan}_act");
    seed_plan(plan, dest, 0);
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan}', '{dest}', 1, '2026-11-10', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 1, 'noon'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 1, 'afternoon'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) \
           VALUES ('{id}', '{plan}', '{dest}', 1, 'noon', 0, '東寺');"
    ));
    id
}

fn guard(plan: &str, dest: &str) -> Guard<impl FnMut()> {
    Guard::new({
        let (plan, dest) = (plan.to_string(), dest.to_string());
        move || teardown_plan(&plan, &dest)
    })
}

#[test]
fn set_meals_refused_when_recorded() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = guard(&plan, &dest);
    seed_base(&plan, &dest);
    exec_ok(&format!(
        "INSERT INTO session_meals (plan_id, destination, day_number, session_type, sort_order, meal) \
           VALUES ('{plan}', '{dest}', 1, 'noon', 0, 'ラーメン大盛');"
    ));
    exec_ok(&format!(
        "UPDATE session_meals SET record_status = 'done' \
           WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 AND sort_order = 0;"
    ));

    expect_refused(
        &["set-meals", "1", "noon", "--meal", "別的家", "--plan-id", &plan, "--dest", &dest],
        "ラーメン大盛",
        &plan,
    );
    // The recorded meal row is untouched.
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM session_meals \
             WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 \
               AND session_type = 'noon' AND meal = 'ラーメン大盛' AND record_status = 'done'"
        )),
        "1"
    );
}

#[test]
fn clear_meals_refused_when_recorded() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = guard(&plan, &dest);
    seed_base(&plan, &dest);
    exec_ok(&format!(
        "INSERT INTO session_meals (plan_id, destination, day_number, session_type, sort_order, meal) \
           VALUES ('{plan}', '{dest}', 1, 'noon', 0, '牛カツ定食');"
    ));
    exec_ok(&format!(
        "UPDATE session_meals SET record_status = 'skipped', record_note = '排隊太長' \
           WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1;"
    ));

    expect_refused(
        &["clear-meals", "1", "noon", "--plan-id", &plan, "--dest", &dest],
        "牛カツ定食",
        &plan,
    );
}

#[test]
fn bulk_route_segments_refused_when_recorded() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = guard(&plan, &dest);
    seed_base(&plan, &dest);
    exec_ok(&format!(
        "INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode) \
           VALUES ('{plan}', '{dest}', 1, 0, '京都駅', '東寺', 'transit');"
    ));
    exec_ok(&format!(
        "UPDATE day_route_segments SET record_status = 'done' \
           WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1;"
    ));

    expect_refused(
        &[
            "set-route-segments-bulk", "1", "--seg", "京都駅|東寺|transit",
            "--plan-id", &plan, "--dest", &dest,
        ],
        "京都駅 → 東寺",
        &plan,
    );
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM day_route_segments WHERE plan_id = '{plan}' AND record_status = 'done'"
        )),
        "1",
        "the recorded leg must still be there"
    );
}

#[test]
fn set_route_segment_refused_when_recorded() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = guard(&plan, &dest);
    seed_base(&plan, &dest);
    exec_ok(&format!(
        "INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode) \
           VALUES ('{plan}', '{dest}', 1, 0, '京都駅', '東寺', 'transit');"
    ));
    exec_ok(&format!(
        "UPDATE day_route_segments SET record_status = 'done' \
           WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 AND sort_order = 0;"
    ));

    expect_refused(
        &[
            "set-route-segment", "1", "0", "京都駅", "清水寺", "transit",
            "--plan-id", &plan, "--dest", &dest,
        ],
        "京都駅 → 東寺",
        &plan,
    );
}

#[test]
fn derive_routes_refused_when_recorded() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = guard(&plan, &dest);
    seed_base(&plan, &dest);
    // Two station-chained activities (morning → noon) so derive wants a rebuild.
    exec_ok(&format!(
        "INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title, nearest_station) \
           VALUES ('{plan}_m', '{plan}', '{dest}', 1, 'morning', 0, '京都駅周邊', '京都駅'); \
         UPDATE activities SET nearest_station = '東寺' WHERE id = '{plan}_act';"
    ));
    // A RECORDED ai_recommended leg whose endpoints differ from the derived chain
    // (so derive-routes reaches its delete-rebuild step, not the "unchanged" skip).
    exec_ok(&format!(
        "INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode, source) \
           VALUES ('{plan}', '{dest}', 1, 0, '烏丸駅', '五条駅', 'transit', 'ai_recommended');"
    ));
    exec_ok(&format!(
        "UPDATE day_route_segments SET record_status = 'done' \
           WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1 AND sort_order = 0;"
    ));

    expect_refused(
        &["derive-routes", "--day", "1", "--plan-id", &plan, "--dest", &dest],
        "烏丸駅 → 五条駅",
        &plan,
    );
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM day_route_segments WHERE plan_id = '{plan}' AND record_status = 'done'"
        )),
        "1",
        "the recorded ai leg must survive"
    );
}

#[test]
fn delete_activity_refused_when_recorded() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = guard(&plan, &dest);
    let id = seed_base(&plan, &dest);
    exec_ok(&format!(
        "UPDATE activities SET record_status = 'done' WHERE id = '{id}';"
    ));

    expect_refused(
        &["delete-activity", "1", "noon", "東寺", "--plan-id", &plan, "--dest", &dest],
        "東寺",
        &plan,
    );
    assert_eq!(
        n(&format!("SELECT COUNT(*) AS n FROM activities WHERE id = '{id}'")),
        "1",
        "the recorded activity must survive"
    );
}

#[test]
fn scaffold_force_refused_when_recorded() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = guard(&plan, &dest);
    let id = seed_base(&plan, &dest);
    exec_ok(&format!(
        "INSERT INTO date_anchors (plan_id, destination, start_date, end_date, days) \
           VALUES ('{plan}', '{dest}', '2026-11-10', '2026-11-10', 1); \
         UPDATE activities SET record_status = 'skipped' WHERE id = '{id}';"
    ));

    expect_refused(
        &["scaffold-itinerary", "--force", "--plan-id", &plan, "--dest", &dest],
        "東寺",
        &plan,
    );
    assert_eq!(
        n(&format!("SELECT COUNT(*) AS n FROM days WHERE plan_id = '{plan}'")),
        "1",
        "the day skeleton must survive"
    );
}

#[test]
fn remove_day_variant_refused_when_recorded() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = guard(&plan, &dest);
    seed_base(&plan, &dest);
    // A variant day (9 → main 1) carrying a RECORDED activity. Records can never
    // be WRITTEN on a variant via the CLI; this row models content that was
    // recorded while the day was a plain trip day before varianting.
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, variant_of) \
           VALUES ('{plan}', '{dest}', 9, '2026-11-10', 'full', 1); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 9, 'noon'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) \
           VALUES ('{plan}_v', '{plan}', '{dest}', 9, 'noon', 0, '雨天替代：博物館'); \
         UPDATE activities SET record_status = 'done' WHERE id = '{plan}_v';"
    ));

    expect_refused(
        &["remove-day-variant", "9", "--plan-id", &plan, "--dest", &dest],
        "雨天替代：博物館",
        &plan,
    );
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM days WHERE plan_id = '{plan}' AND day_number = 9"
        )),
        "1",
        "the variant day must survive"
    );
}

#[test]
fn summary_only_blocks_scaffold_force_and_swap() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = guard(&plan, &dest);
    seed_base(&plan, &dest);
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan}', '{dest}', 2, '2026-11-11', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 2, 'noon'); \
         INSERT INTO date_anchors (plan_id, destination, start_date, end_date, days) \
           VALUES ('{plan}', '{dest}', '2026-11-10', '2026-11-11', 2);"
    ));
    // Summary on day 1 ONLY (via the record-session CLI — a main day in the past).
    let summary = "整天都在下雨，改成室內行程";
    let (ok, _, stderr) = run_cmd(&[
        "record-session", "1", "noon", "--zh", summary,
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "record-session failed: {stderr}");

    // scaffold --force (Destination scope) must refuse.
    expect_refused(
        &["scaffold-itinerary", "--force", "--plan-id", &plan, "--dest", &dest],
        summary,
        &plan,
    );
    // swap-days with the summary on day_a.
    expect_refused(
        &["swap-days", "1", "2", "--plan-id", &plan, "--dest", &dest],
        summary,
        &plan,
    );
    // Move the summary to day 2: swap must STILL refuse (summary on day_b).
    exec_ok(&format!(
        "UPDATE timesofday SET record_summary_zh = NULL \
           WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 1; \
         UPDATE timesofday SET record_summary_zh = '{summary}' \
           WHERE plan_id = '{plan}' AND destination = '{dest}' AND day_number = 2;"
    ));
    expect_refused(
        &["swap-days", "1", "2", "--plan-id", &plan, "--dest", &dest],
        summary,
        &plan,
    );
    // Both days' rows untouched.
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM timesofday WHERE plan_id = '{plan}' AND record_summary_zh IS NOT NULL"
        )),
        "1"
    );
}

#[test]
fn summary_only_day_blocks_delete_day_content() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = guard(&plan, &dest);
    seed_base(&plan, &dest);
    // A variant day whose ONLY record is a session summary (summary seeded raw:
    // record-session refuses variant days, so the CLI can never write this).
    let summary = "變體日的實況摘要";
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, variant_of) \
           VALUES ('{plan}', '{dest}', 9, '2026-11-10', 'full', 1); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, record_summary_zh) \
           VALUES ('{plan}', '{dest}', 9, 'noon', '{summary}');"
    ));

    let summaries_before = n(&format!(
        "SELECT COUNT(*) AS n FROM timesofday WHERE plan_id = '{plan}' AND record_summary_zh IS NOT NULL"
    ));
    let days_before = n(&format!("SELECT COUNT(*) AS n FROM days WHERE plan_id = '{plan}'"));
    expect_refused(
        &["remove-day-variant", "9", "--plan-id", &plan, "--dest", &dest],
        summary,
        &plan,
    );
    assert_eq!(
        n(&format!(
            "SELECT COUNT(*) AS n FROM timesofday WHERE plan_id = '{plan}' AND record_summary_zh IS NOT NULL"
        )),
        summaries_before,
        "the summary row must survive"
    );
    assert_eq!(
        n(&format!("SELECT COUNT(*) AS n FROM days WHERE plan_id = '{plan}'")),
        days_before,
        "the day rows must survive"
    );
}

#[test]
fn move_activity_cross_day_rejected() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = guard(&plan, &dest);
    let id = seed_base(&plan, &dest);
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan}', '{dest}', 2, '2026-11-11', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 2, 'noon'); \
         UPDATE activities SET record_status = 'done' WHERE id = '{id}';"
    ));

    expect_refused(
        &[
            "move-activity", "1", "noon", "noon", "東寺", "--to-day", "2",
            "--plan-id", &plan, "--dest", &dest,
        ],
        "東寺",
        &plan,
    );
    // Still on day 1.
    assert_eq!(
        n(&format!(
            "SELECT day_number AS v FROM activities WHERE id = '{id}'"
        )),
        "1"
    );
}

#[test]
fn move_activity_same_day_ok() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = guard(&plan, &dest);
    let id = seed_base(&plan, &dest);
    exec_ok(&format!(
        "UPDATE activities SET record_status = 'done' WHERE id = '{id}';"
    ));

    let (ok, stdout, stderr) = run_cmd(&[
        "move-activity", "1", "noon", "afternoon", "東寺",
        "--plan-id", &plan, "--dest", &dest,
    ]);
    assert!(ok, "same-day move must be allowed; stdout={stdout} stderr={stderr}");
    assert_eq!(
        n(&format!(
            "SELECT session_type AS v FROM activities WHERE id = '{id}'"
        )),
        "afternoon"
    );
    assert_eq!(
        n(&format!("SELECT record_status AS v FROM activities WHERE id = '{id}'")),
        "done",
        "the record follows the row within the same day"
    );
}

#[test]
fn swap_days_rejected_when_either_day_recorded() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = guard(&plan, &dest);
    seed_base(&plan, &dest);
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan}', '{dest}', 2, '2026-11-11', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', 2, 'noon'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) \
           VALUES ('{plan}_d2', '{plan}', '{dest}', 2, 'noon', 0, '南禪寺');"
    ));

    // Recorded row on day 1 → refuse.
    exec_ok(&format!(
        "UPDATE activities SET record_status = 'done' WHERE id = '{plan}_act';"
    ));
    expect_refused(
        &["swap-days", "1", "2", "--plan-id", &plan, "--dest", &dest],
        "東寺",
        &plan,
    );
    // Day 1 clean, recorded row on day 2 → still refuse.
    exec_ok(&format!(
        "UPDATE activities SET record_status = NULL WHERE id = '{plan}_act'; \
         UPDATE activities SET record_status = 'skipped' WHERE id = '{plan}_d2';"
    ));
    expect_refused(
        &["swap-days", "1", "2", "--plan-id", &plan, "--dest", &dest],
        "南禪寺",
        &plan,
    );
    // Both rows stay on their original days.
    assert_eq!(
        n(&format!(
            "SELECT day_number AS v FROM activities WHERE id = '{plan}_act'"
        )),
        "1"
    );
    assert_eq!(
        n(&format!(
            "SELECT day_number AS v FROM activities WHERE id = '{plan}_d2'"
        )),
        "2"
    );
}

#[test]
fn copy_plan_carries_records() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if !creds() {
        eprintln!("skipping record_guard test (no Turso creds)");
        return;
    }
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    // copy-plan enforces kebab-case destination ids (like the zzcp<n>-dst fixtures
    // in copy_plan.rs); the source plan keeps the zz_test_<nanos> convention.
    let plan2 = format!("zz-cp-{tag}");
    let _g = Guard::new({
        let (plan, plan2, dest) = (plan.clone(), plan2.clone(), dest.clone());
        move || {
            teardown_plan(&plan2, &dest);
            teardown_plan(&plan, &dest);
        }
    });
    let id = seed_base(&plan, &dest);
    exec_ok(&format!(
        "UPDATE activities SET record_status = 'skipped', record_note = '當天休息' WHERE id = '{id}';"
    ));

    let (ok, stdout, stderr) = run_cmd(&["copy-plan", &plan, &plan2]);
    assert!(ok, "copy-plan failed; stdout={stdout} stderr={stderr}");
    assert_eq!(
        n(&format!(
            "SELECT record_status AS v FROM activities WHERE plan_id = '{plan2}' AND title = '東寺'"
        )),
        "skipped",
        "copy-plan is schema-driven: record columns must ride along"
    );
    assert_eq!(
        n(&format!(
            "SELECT record_note AS v FROM activities WHERE plan_id = '{plan2}' AND title = '東寺'"
        )),
        "當天休息"
    );
}
