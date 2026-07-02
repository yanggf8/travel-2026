//! Real-Turso behavior LOCK for the `set_activity` command family before its DAL
//! migration. This is one representative mutation-surface unit: delete,
//! booking, reorder, add-after, move, and title/poi clearing all run through the
//! real binary and assert the current table writes plus the audit back half.

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
        eprintln!(
            "skipping set-activity mutation lock (no Turso creds): {}",
            stderr.trim()
        );
        return None;
    }
    panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
}

fn exec_ok(sql: &str) -> String {
    db_exec(sql).unwrap_or_else(|| panic!("db exec skipped unexpectedly for SQL: {sql}"))
}

fn run_cmd(args: &[&str]) -> Option<(String, String)> {
    let out = bin()
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() && is_skip(&stderr) {
        eprintln!(
            "skipping set-activity mutation lock mid-test: {}",
            stderr.trim()
        );
        return None;
    }
    assert!(
        out.status.success(),
        "travel {args:?} failed; stdout={stdout} stderr={stderr}"
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
        "DELETE FROM activity_tags \
           WHERE activity_id LIKE {p} || '-%' \
              OR activity_id IN (SELECT id FROM activities WHERE plan_id = {p}); \
         DELETE FROM session_meals WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM session_activities_zh WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM cascade_dirty_flags WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM process_statuses WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM plan_event_data WHERE plan_id = {p}; \
         DELETE FROM plan_events WHERE plan_id = {p}; \
         DELETE FROM operation_runs WHERE plan_id = {p}; \
         DELETE FROM activities WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM timesofday WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM days WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM destination_poi_tags WHERE slug = {d}; \
         DELETE FROM destination_cluster_pois WHERE slug = {d}; \
         DELETE FROM destination_pois WHERE slug = {d}; \
         DELETE FROM destination_config WHERE slug = {d}; \
         DELETE FROM plan_metadata WHERE plan_id = {p}; \
         DELETE FROM plans WHERE plan_id = {p};"
    ));
}

fn seed(
    plan: &str,
    dest: &str,
    act_alpha: &str,
    act_bait: &str,
    act_exact: &str,
    act_delete: &str,
    act_title: &str,
) -> bool {
    let p = sql_lit(plan);
    let d = sql_lit(dest);
    let alpha = sql_lit(act_alpha);
    let bait = sql_lit(act_bait);
    let exact = sql_lit(act_exact);
    let deleted = sql_lit(act_delete);
    let title = sql_lit(act_title);
    let bait_title = sql_lit(&format!("Bait title mentions {act_exact}"));
    let stale_poi = sql_lit(&format!("stale_poi_{dest}"));
    db_exec(&format!(
        "INSERT INTO plans (plan_id, schema_version, version) VALUES ({p}, '4.2.0', 0); \
         INSERT INTO plan_metadata (plan_id, schema_version, active_destination) VALUES ({p}, '4.2.0', {d}); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, status, updated_at) \
           VALUES ({p}, {d}, 1, '2026-09-01', 'full', 'draft', '2020-01-01 00:00:00'); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, status, updated_at) \
           VALUES ({p}, {d}, 2, '2026-09-02', 'full', 'draft', '2020-01-01 00:00:00'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, updated_at) \
           VALUES ({p}, {d}, 1, 'morning', '2020-01-01 00:00:00'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, updated_at) \
           VALUES ({p}, {d}, 1, 'afternoon', '2020-01-01 00:00:00'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, updated_at) \
           VALUES ({p}, {d}, 2, 'morning', '2020-01-01 00:00:00'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, area, nearest_station, duration_min, priority, notes, updated_at) \
           VALUES ({alpha}, {p}, {d}, 1, 'morning', 0, 'Alpha Garden Walk', 'naha', 'Makishi', 45, 'want', 'alpha notes', '2020-01-01 00:00:00'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, priority, updated_at) \
           VALUES ({bait}, {p}, {d}, 1, 'morning', 1, {bait_title}, 'want', '2020-01-01 00:00:00'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, area, nearest_station, duration_min, booking_required, booking_status, booking_ref, book_by, priority, notes, updated_at) \
           VALUES ({exact}, {p}, {d}, 1, 'morning', 2, 'Exact Id Winner', 'kokusai', 'Prefecture Office', 60, 0, NULL, NULL, NULL, 'must', 'preserve me', '2020-01-01 00:00:00'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, priority, updated_at) \
           VALUES ({deleted}, {p}, {d}, 1, 'morning', 3, 'Delete Me Locked Row', 'optional', '2020-01-01 00:00:00'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, poi_id, priority, updated_at) \
           VALUES ({title}, {p}, {d}, 1, 'morning', 4, 'Old POI Title', {stale_poi}, 'want', '2020-01-01 00:00:00'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, priority, updated_at) \
           VALUES ({p} || '-target-a', {p}, {d}, 1, 'afternoon', 0, 'Target Already First', 'want', '2020-01-01 00:00:00'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, priority, updated_at) \
           VALUES ({p} || '-target-b', {p}, {d}, 1, 'afternoon', 3, 'Target Already Last', 'want', '2020-01-01 00:00:00'); \
         INSERT INTO activity_tags (activity_id, tag, updated_at) VALUES ({deleted}, 'cleanup-a', '2020-01-01 00:00:00'); \
         INSERT INTO activity_tags (activity_id, tag, updated_at) VALUES ({deleted}, 'cleanup-b', '2020-01-01 00:00:00');"
    ))
    .is_some()
}

fn assert_audit(plan: &str, expected_count: i64, expected_version: i64, command_type: &str) {
    let p = sql_lit(plan);
    let count = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = {p}"
    ));
    assert_eq!(
        scalar(&count).as_deref(),
        Some(expected_count.to_string().as_str())
    );

    let version = exec_ok(&format!(
        "SELECT version AS v FROM plans WHERE plan_id = {p}"
    ));
    assert_eq!(
        scalar(&version).as_deref(),
        Some(expected_version.to_string().as_str())
    );

    let cmd = sql_lit(command_type);
    let row = exec_ok(&format!(
        "SELECT COUNT(*) || '|' || MIN(version_before) || '>' || MAX(version_after) || '|' || MIN(status) AS v \
         FROM operation_runs WHERE plan_id = {p} AND command_type = {cmd}"
    ));
    assert_eq!(
        scalar(&row).as_deref(),
        Some(format!("1|{}>{expected_version}|completed", expected_version - 1).as_str()),
        "one completed operation row for {command_type}; out={row}"
    );
}

fn activity_order(plan: &str, dest: &str, day: i64, session: &str) -> Vec<String> {
    let p = sql_lit(plan);
    let d = sql_lit(dest);
    let s = sql_lit(session);
    let out = exec_ok(&format!(
        "SELECT id || '=' || sort_order AS v \
         FROM activities \
         WHERE plan_id = {p} AND destination = {d} AND day_number = {day} AND session_type = {s} \
         ORDER BY sort_order, id"
    ));
    column(&out)
}

#[test]
fn set_activity_command_family_mutation_surface_is_locked() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping set-activity mutation lock (no Turso creds)");
        return;
    }

    let tag = nanos();
    let plan = format!("zztest-{tag}");
    let dest = format!("zztest_{tag}");
    let act_alpha = format!("{plan}-alpha");
    let act_bait = format!("{plan}-bait");
    let act_exact = format!("{plan}-exact");
    let act_delete = format!("{plan}-delete");
    let act_title = format!("{plan}-title");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown(&plan, &dest)
    });
    teardown(&plan, &dest);
    if !seed(
        &plan,
        &dest,
        &act_alpha,
        &act_bait,
        &act_exact,
        &act_delete,
        &act_title,
    ) {
        return;
    }

    // 1. delete-activity by title substring removes the activity row and child tags.
    if run_cmd(&[
        "delete-activity",
        "1",
        "morning",
        "Delete Me",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ])
    .is_none()
    {
        return;
    }
    let deleted = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM activities WHERE id = {}",
        sql_lit(&act_delete)
    ));
    assert_eq!(
        scalar(&deleted).as_deref(),
        Some("0"),
        "deleted activity row is gone"
    );
    let deleted_tags = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM activity_tags WHERE activity_id = {}",
        sql_lit(&act_delete)
    ));
    assert_eq!(
        scalar(&deleted_tags).as_deref(),
        Some("0"),
        "deleted activity tags are cleaned"
    );
    assert_audit(&plan, 1, 1, "delete-activity");

    // 2. set-activity-booking with an exact id wins over a title containing that id.
    if run_cmd(&[
        "set-activity-booking",
        "1",
        "morning",
        &act_exact,
        "pending",
        "--ref",
        "CONF-LOCK-123",
        "--book-by",
        "2026-08-15",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ])
    .is_none()
    {
        return;
    }
    let booking = exec_ok(&format!(
        "SELECT booking_status || '|' || COALESCE(booking_ref, '<NULL>') || '|' || \
                COALESCE(book_by, '<NULL>') || '|' || booking_required AS v \
         FROM activities WHERE id = {}",
        sql_lit(&act_exact)
    ));
    assert_eq!(
        scalar(&booking).as_deref(),
        Some("pending|CONF-LOCK-123|2026-08-15|1"),
        "booking fields are written on the exact-id row; out={booking}"
    );
    let bait_booking = exec_ok(&format!(
        "SELECT COALESCE(booking_status, '<NULL>') || '|' || booking_required AS v \
         FROM activities WHERE id = {}",
        sql_lit(&act_bait)
    ));
    assert_eq!(
        scalar(&bait_booking).as_deref(),
        Some("<NULL>|0"),
        "title-substring bait row must not be updated when token is an exact id"
    );
    assert_audit(&plan, 2, 2, "set-activity-booking");

    // 3. reorder-activities rewrites a bijection matching the requested order.
    if run_cmd(&[
        "reorder-activities",
        "1",
        "morning",
        "Old POI",
        "Bait title",
        &act_exact,
        "Alpha Garden",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ])
    .is_none()
    {
        return;
    }
    let bijection = exec_ok(&format!(
        "SELECT COUNT(*) || '|' || COUNT(DISTINCT sort_order) || '|' || MIN(sort_order) || '|' || MAX(sort_order) AS v \
         FROM activities WHERE plan_id = {} AND destination = {} AND day_number = 1 AND session_type = 'morning'",
        sql_lit(&plan),
        sql_lit(&dest)
    ));
    assert_eq!(scalar(&bijection).as_deref(), Some("4|4|0|3"));
    assert_eq!(
        activity_order(&plan, &dest, 1, "morning"),
        vec![
            format!("{act_title}=0"),
            format!("{act_bait}=1"),
            format!("{act_exact}=2"),
            format!("{act_alpha}=3"),
        ],
        "sort_order must match the requested reorder exactly"
    );
    assert_audit(&plan, 3, 3, "reorder-activities");

    // 4. add-activity --after shifts later rows first, then inserts at anchor+1.
    if run_cmd(&[
        "add-activity",
        "1",
        "morning",
        "Inserted After Bait",
        "--after",
        "Bait title",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ])
    .is_none()
    {
        return;
    }
    let inserted_id = exec_ok(&format!(
        "SELECT id AS v FROM activities \
         WHERE plan_id = {} AND destination = {} AND title = 'Inserted After Bait'",
        sql_lit(&plan),
        sql_lit(&dest)
    ));
    let inserted_id = scalar(&inserted_id).expect("new activity id");
    assert_eq!(
        activity_order(&plan, &dest, 1, "morning"),
        vec![
            format!("{act_title}=0"),
            format!("{act_bait}=1"),
            format!("{inserted_id}=2"),
            format!("{act_exact}=3"),
            format!("{act_alpha}=4"),
        ],
        "add-activity --after must shift existing rows before inserting"
    );
    assert_audit(&plan, 4, 4, "add-activity");

    // 5. move-activity appends to the target session, preserving row fields and updated_at.
    let target_max = exec_ok(&format!(
        "SELECT COALESCE(MAX(sort_order), -1) AS n FROM activities \
         WHERE plan_id = {} AND destination = {} AND day_number = 1 AND session_type = 'afternoon'",
        sql_lit(&plan),
        sql_lit(&dest)
    ));
    let target_max = scalar(&target_max).unwrap().parse::<i64>().unwrap();
    let before_move = exec_ok(&format!(
        "SELECT id || '|' || title || '|' || COALESCE(area, '<NULL>') || '|' || \
                COALESCE(nearest_station, '<NULL>') || '|' || COALESCE(duration_min, -1) || '|' || \
                booking_required || '|' || COALESCE(booking_status, '<NULL>') || '|' || \
                COALESCE(booking_ref, '<NULL>') || '|' || COALESCE(book_by, '<NULL>') || '|' || \
                priority || '|' || COALESCE(notes, '<NULL>') || '|' || updated_at AS v \
         FROM activities WHERE id = {}",
        sql_lit(&act_exact)
    ));
    let before_move = scalar(&before_move).expect("pre-move activity row");
    if run_cmd(&[
        "move-activity",
        "1",
        "morning",
        "afternoon",
        &act_exact,
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ])
    .is_none()
    {
        return;
    }
    let moved = exec_ok(&format!(
        "SELECT day_number || '|' || session_type || '|' || sort_order || '|' || \
                id || '|' || title || '|' || COALESCE(area, '<NULL>') || '|' || \
                COALESCE(nearest_station, '<NULL>') || '|' || COALESCE(duration_min, -1) || '|' || \
                booking_required || '|' || COALESCE(booking_status, '<NULL>') || '|' || \
                COALESCE(booking_ref, '<NULL>') || '|' || COALESCE(book_by, '<NULL>') || '|' || \
                priority || '|' || COALESCE(notes, '<NULL>') || '|' || updated_at AS v \
         FROM activities WHERE id = {}",
        sql_lit(&act_exact)
    ));
    assert_eq!(
        scalar(&moved).as_deref(),
        Some(format!("1|afternoon|{}|{before_move}", target_max + 1).as_str()),
        "move preserves id and other fields, appends at MAX(sort_order)+1, and does not bump updated_at; out={moved}"
    );
    let source_count = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM activities \
         WHERE id = {} AND day_number = 1 AND session_type = 'morning'",
        sql_lit(&act_exact)
    ));
    assert_eq!(
        scalar(&source_count).as_deref(),
        Some("0"),
        "moved row leaves source session"
    );
    assert_audit(&plan, 5, 5, "move-activity");

    // 6. set-activity-title by title substring clears poi_id when the title changes.
    if run_cmd(&[
        "set-activity-title",
        "1",
        "morning",
        "Old POI",
        "Fresh POI Lock Title",
        "--plan-id",
        &plan,
        "--dest",
        &dest,
    ])
    .is_none()
    {
        return;
    }
    let title_row = exec_ok(&format!(
        "SELECT title || '|' || COALESCE(poi_id, '<NULL>') AS v FROM activities WHERE id = {}",
        sql_lit(&act_title)
    ));
    assert_eq!(
        scalar(&title_row).as_deref(),
        Some("Fresh POI Lock Title|<NULL>"),
        "title change must clear a stale poi_id; out={title_row}"
    );
    assert_audit(&plan, 6, 6, "set-activity-title");
}
