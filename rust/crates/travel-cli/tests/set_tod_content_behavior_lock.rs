//! Real-Turso behavior LOCK for the `set-tod` command family (the last of the 5
//! itinerary-cluster behavior-locks) before its DAL migration. Drives the real
//! binary for all four session-content writers and asserts the CURRENT write
//! surface plus the audit back half:
//!
//!   - set-tod-focus  --zh   → timesofday.focus + focus_zh
//!   - set-tod-time-range    → timesofday.time_range_start/end (+ both-flags-required guard)
//!   - set-tod-zh            → focus_zh + transit_notes_zh + session_activities_zh child rows
//!                             (DELETE-then-reinsert; --clear-activities empties them);
//!                             does NOT touch session_meals
//!   - set-meals             → session_meals child rows (DELETE-then-reinsert);
//!                             does NOT touch session_activities_zh
//!
//! Each invocation writes exactly one operation_runs row for its command_type and
//! bumps plans.version by one; the running version count is tracked across the
//! sequence. This is a LOCK of current behavior, NOT a spec — assertions model
//! what the code does today so the upcoming migration must reproduce it.

use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

static LOCK: Mutex<()> = Mutex::new(());

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn is_skip(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("Missing Turso data")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
}

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn db_exec(sql: &str) -> Option<String> {
    let out = bin()
        .args(["db", "exec", sql])
        .output()
        .expect("run travel db exec");
    if out.status.success() {
        return Some(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if is_skip(&stderr) {
        eprintln!("skipping set-tod content lock (no Turso creds): {}", stderr.trim());
        return None;
    }
    panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
}

fn exec_ok(sql: &str) -> String {
    db_exec(sql).unwrap_or_else(|| panic!("db exec skipped unexpectedly for SQL: {sql}"))
}

/// Run a mutation command; returns None on a credless mid-test skip.
fn run_cmd(args: &[&str]) -> Option<(String, String)> {
    let out = bin()
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() && is_skip(&stderr) {
        eprintln!("skipping set-tod content lock mid-test: {}", stderr.trim());
        return None;
    }
    assert!(
        out.status.success(),
        "travel {args:?} should succeed; stdout={stdout} stderr={stderr}"
    );
    Some((stdout, stderr))
}

/// Run a command expected to FAIL (non-zero exit); returns (stdout, stderr).
/// Distinguishes a real refusal from a credless skip.
fn run_cmd_expect_fail(args: &[&str]) -> Option<(String, String)> {
    let out = bin()
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if is_skip(&stderr) {
        eprintln!("skipping set-tod content lock mid-test: {}", stderr.trim());
        return None;
    }
    assert!(
        !out.status.success(),
        "travel {args:?} should FAIL but succeeded; stdout={stdout} stderr={stderr}"
    );
    Some((stdout, stderr))
}

fn scalar(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
}

fn column(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
        .filter(|v| !v.is_empty())
        .collect()
}

fn teardown(plan: &str, dest: &str) {
    let p = sql_lit(plan);
    let d = sql_lit(dest);
    let _ = db_exec(&format!(
        "DELETE FROM session_meals WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM session_activities_zh WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM cascade_dirty_flags WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM process_statuses WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM plan_event_data WHERE plan_id = {p}; \
         DELETE FROM plan_events WHERE plan_id = {p}; \
         DELETE FROM operation_runs WHERE plan_id = {p}; \
         DELETE FROM timesofday WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM days WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM plan_metadata WHERE plan_id = {p}; \
         DELETE FROM plans WHERE plan_id = {p};"
    ));
}

/// Seed a plan + plan_metadata + a scaffolded day 1 with the four sessions
/// (morning/noon/afternoon/evening). Mirrors how the scaffold/set_activity locks
/// seed a day+sessions directly. Returns false on a credless skip.
fn seed(plan: &str, dest: &str) -> bool {
    let p = sql_lit(plan);
    let d = sql_lit(dest);
    db_exec(&format!(
        "INSERT INTO plans (plan_id, schema_version, version) VALUES ({p}, '4.2.0', 0); \
         INSERT INTO plan_metadata (plan_id, schema_version, active_destination) VALUES ({p}, '4.2.0', {d}); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, status, updated_at) \
           VALUES ({p}, {d}, 1, '2026-09-01', 'full', 'draft', '2020-01-01 00:00:00'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, updated_at) \
           VALUES ({p}, {d}, 1, 'morning', '2020-01-01 00:00:00'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, updated_at) \
           VALUES ({p}, {d}, 1, 'noon', '2020-01-01 00:00:00'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, updated_at) \
           VALUES ({p}, {d}, 1, 'afternoon', '2020-01-01 00:00:00'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, updated_at) \
           VALUES ({p}, {d}, 1, 'evening', '2020-01-01 00:00:00');"
    ))
    .is_some()
}

/// Assert the audit back half after an invocation: exactly `expected_count`
/// operation_runs rows total, plans.version == `expected_version`, and the
/// LATEST completed operation_runs row for `command_type` recording the +1 bump
/// (version_before == expected_version - 1). `cmd_type_rows` is how many rows
/// this `command_type` should have accumulated so far (set-tod-zh is invoked
/// twice, so it accumulates 2 rows under the same command_type).
fn assert_audit(
    plan: &str,
    expected_count: i64,
    expected_version: i64,
    command_type: &str,
    cmd_type_rows: i64,
) {
    let p = sql_lit(plan);
    let count = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = {p}"
    ));
    assert_eq!(
        scalar(&count).as_deref(),
        Some(expected_count.to_string().as_str()),
        "operation_runs total count after {command_type}"
    );

    let version = exec_ok(&format!("SELECT version AS v FROM plans WHERE plan_id = {p}"));
    assert_eq!(
        scalar(&version).as_deref(),
        Some(expected_version.to_string().as_str()),
        "plans.version after {command_type}"
    );

    let cmd = sql_lit(command_type);
    // Rows accumulated so far for this command_type (set-session-zh runs twice).
    let cmd_count = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = {p} AND command_type = {cmd}"
    ));
    assert_eq!(
        scalar(&cmd_count).as_deref(),
        Some(cmd_type_rows.to_string().as_str()),
        "operation_runs row count for {command_type}"
    );
    // The LATEST row for this command_type (highest version_after) records the +1 bump.
    let row = exec_ok(&format!(
        "SELECT version_before || '>' || COALESCE(version_after, -1) || '|' || status AS v \
         FROM operation_runs WHERE plan_id = {p} AND command_type = {cmd} \
         ORDER BY version_after DESC LIMIT 1"
    ));
    assert_eq!(
        scalar(&row).as_deref(),
        Some(format!("{}>{expected_version}|completed", expected_version - 1).as_str()),
        "latest completed operation row for {command_type} records the +1 bump; out={row}"
    );
}

#[test]
fn set_tod_content_command_family_write_surface_is_locked() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping set-tod content lock (no Turso creds)");
        return;
    }

    let tag = nanos();
    let plan = format!("zztest-{tag}");
    let dest = format!("zztest_{tag}");

    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown(&plan, &dest)
    });
    teardown(&plan, &dest);
    if !seed(&plan, &dest) {
        return;
    }

    let p = sql_lit(&plan);
    let d = sql_lit(&dest);

    // ── 1. set-tod-focus with --zh → focus + focus_zh on the (day, session) row ──
    // run_focus audits BEFORE applying the field updates, so assert END STATE only.
    if run_cmd(&[
        "set-tod-focus",
        "1",
        "morning",
        "Naha castle morning",
        "--zh",
        "那霸城早晨",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ])
    .is_none()
    {
        return;
    }
    let focus_row = exec_ok(&format!(
        "SELECT COALESCE(focus, '<NULL>') || '|' || COALESCE(focus_zh, '<NULL>') AS v \
         FROM timesofday WHERE plan_id = {p} AND destination = {d} \
           AND day_number = 1 AND session_type = 'morning'"
    ));
    assert_eq!(
        scalar(&focus_row).as_deref(),
        Some("Naha castle morning|那霸城早晨"),
        "set-tod-focus --zh must write both focus and focus_zh; out={focus_row}"
    );
    // The other sessions were NOT touched by a morning focus edit.
    let noon_focus = exec_ok(&format!(
        "SELECT COALESCE(focus, '<NULL>') || '|' || COALESCE(focus_zh, '<NULL>') AS v \
         FROM timesofday WHERE plan_id = {p} AND destination = {d} \
           AND day_number = 1 AND session_type = 'noon'"
    ));
    assert_eq!(
        scalar(&noon_focus).as_deref(),
        Some("<NULL>|<NULL>"),
        "focus edit is scoped to its session; out={noon_focus}"
    );
    assert_audit(&plan, 1, 1, "set-tod-focus", 1);

    // ── 2. set-tod-time-range → time_range_start/end on the (day, session) row ──
    if run_cmd(&[
        "set-tod-time-range",
        "1",
        "morning",
        "--start",
        "08:30",
        "--end",
        "11:45",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ])
    .is_none()
    {
        return;
    }
    let range_row = exec_ok(&format!(
        "SELECT COALESCE(time_range_start, '<NULL>') || '|' || COALESCE(time_range_end, '<NULL>') AS v \
         FROM timesofday WHERE plan_id = {p} AND destination = {d} \
           AND day_number = 1 AND session_type = 'morning'"
    ));
    assert_eq!(
        scalar(&range_row).as_deref(),
        Some("08:30|11:45"),
        "set-tod-time-range must write both time_range_start and time_range_end; out={range_row}"
    );
    // command_type for the time-range writer is the alias form.
    assert_audit(&plan, 2, 2, "set-session-time-range", 1);

    // ── 2b. both-flags-required guard: missing --end must FAIL and write NOTHING. ──
    let before_guard = exec_ok(&format!(
        "SELECT COALESCE(time_range_start, '<NULL>') || '|' || COALESCE(time_range_end, '<NULL>') AS v \
         FROM timesofday WHERE plan_id = {p} AND destination = {d} \
           AND day_number = 1 AND session_type = 'noon'"
    ));
    assert_eq!(
        scalar(&before_guard).as_deref(),
        Some("<NULL>|<NULL>"),
        "noon range starts empty; out={before_guard}"
    );
    if run_cmd_expect_fail(&[
        "set-tod-time-range",
        "1",
        "noon",
        "--start",
        "13:00",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ])
    .is_none()
    {
        return;
    }
    let after_guard = exec_ok(&format!(
        "SELECT COALESCE(time_range_start, '<NULL>') || '|' || COALESCE(time_range_end, '<NULL>') AS v \
         FROM timesofday WHERE plan_id = {p} AND destination = {d} \
           AND day_number = 1 AND session_type = 'noon'"
    ));
    assert_eq!(
        scalar(&after_guard).as_deref(),
        Some("<NULL>|<NULL>"),
        "missing --end must write nothing to the noon range; out={after_guard}"
    );
    // The refusal must not audit or bump version (still 2 rows / version 2).
    assert_audit(&plan, 2, 2, "set-session-time-range", 1);

    // ── 3. set-tod-zh with 2 --activity-zh + a CLEAN --transit-zh pill ──
    // Clean pill "安里駅 → 赤嶺駅" passes the write-time map-link guard.
    let arrow = '\u{2192}';
    let transit_pill = format!("安里駅 {arrow} 赤嶺駅");
    if run_cmd(&[
        "set-tod-zh",
        "1",
        "afternoon",
        "--zh",
        "午後行程",
        "--transit-zh",
        &transit_pill,
        "--activity-zh",
        "首里城",
        "--activity-zh",
        "國際通",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ])
    .is_none()
    {
        return;
    }
    let zh_scalars = exec_ok(&format!(
        "SELECT COALESCE(focus_zh, '<NULL>') || '|' || COALESCE(transit_notes_zh, '<NULL>') AS v \
         FROM timesofday WHERE plan_id = {p} AND destination = {d} \
           AND day_number = 1 AND session_type = 'afternoon'"
    ));
    assert_eq!(
        scalar(&zh_scalars).as_deref(),
        Some(format!("午後行程|安里駅 {arrow} 赤嶺駅").as_str()),
        "set-tod-zh must write focus_zh + transit_notes_zh; out={zh_scalars}"
    );
    let zh_acts = exec_ok(&format!(
        "SELECT sort_order || '=' || activity AS v \
         FROM session_activities_zh WHERE plan_id = {p} AND destination = {d} \
           AND day_number = 1 AND session_type = 'afternoon' ORDER BY sort_order"
    ));
    assert_eq!(
        column(&zh_acts),
        vec!["0=首里城".to_string(), "1=國際通".to_string()],
        "session_activities_zh holds exactly the two --activity-zh values in order 0,1; out={zh_acts}"
    );
    // set-tod-zh must NOT touch session_meals.
    let meals_after_zh = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM session_meals WHERE plan_id = {p} AND destination = {d} \
           AND day_number = 1 AND session_type = 'afternoon'"
    ));
    assert_eq!(
        scalar(&meals_after_zh).as_deref(),
        Some("0"),
        "set-tod-zh must not write session_meals; out={meals_after_zh}"
    );
    assert_audit(&plan, 3, 3, "set-session-zh", 1);

    // ── 3b. set-tod-zh --clear-activities empties the ZH activity list ──
    if run_cmd(&[
        "set-tod-zh",
        "1",
        "afternoon",
        "--clear-activities",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ])
    .is_none()
    {
        return;
    }
    let zh_acts_cleared = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM session_activities_zh WHERE plan_id = {p} AND destination = {d} \
           AND day_number = 1 AND session_type = 'afternoon'"
    ));
    assert_eq!(
        scalar(&zh_acts_cleared).as_deref(),
        Some("0"),
        "--clear-activities removes all session_activities_zh rows; out={zh_acts_cleared}"
    );
    assert_audit(&plan, 4, 4, "set-session-zh", 2);

    // ── 4. set-meals with 2 --meal → session_meals rows in order 0,1 ──
    if run_cmd(&[
        "set-meals",
        "1",
        "noon",
        "--meal",
        "Lunch: soba",
        "--meal",
        "Snack: sata andagi",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ])
    .is_none()
    {
        return;
    }
    let meals = exec_ok(&format!(
        "SELECT sort_order || '=' || meal AS v \
         FROM session_meals WHERE plan_id = {p} AND destination = {d} \
           AND day_number = 1 AND session_type = 'noon' ORDER BY sort_order"
    ));
    assert_eq!(
        column(&meals),
        vec!["0=Lunch: soba".to_string(), "1=Snack: sata andagi".to_string()],
        "session_meals holds exactly the two --meal values in order 0,1; out={meals}"
    );
    // set-meals must NOT touch session_activities_zh (the afternoon list stays cleared,
    // and the noon session has no ZH activity rows either).
    let zh_acts_after_meals = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM session_activities_zh WHERE plan_id = {p} AND destination = {d}"
    ));
    assert_eq!(
        scalar(&zh_acts_after_meals).as_deref(),
        Some("0"),
        "set-meals must not write session_activities_zh; out={zh_acts_after_meals}"
    );
    assert_audit(&plan, 5, 5, "set-meals", 1);
}
