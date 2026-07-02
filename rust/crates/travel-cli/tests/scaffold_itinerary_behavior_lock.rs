//! Real-Turso behavior lock for `scaffold-itinerary`.
//!
//! This captures the current inline-SQL write surface before moving the command
//! behind DAL helpers. It drives the real binary, proves the no-force refusal
//! does not clobber existing itinerary rows, then forces a scaffold over stale
//! child rows and asserts the current replace/audit/event behavior.

use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

static SCAFFOLD_ITINERARY_LOCK: Mutex<()> = Mutex::new(());

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_travel")
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn db_exec(sql: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["db", "exec", sql])
        .output()
        .expect("run db exec");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn exec_ok(sql: &str) -> String {
    let (ok, out, err) = db_exec(sql);
    assert!(ok, "db exec failed; err={err}\nsql={sql}");
    out
}

fn is_skip(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
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

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn teardown(plan: &str, dest: &str, stale_activity_id: &str) {
    let p = sql_lit(plan);
    let d = sql_lit(dest);
    let a = sql_lit(stale_activity_id);
    let _ = db_exec(&format!(
        "DELETE FROM activity_tags WHERE activity_id = {a}; \
         DELETE FROM activity_tags WHERE activity_id IN \
            (SELECT id FROM activities WHERE plan_id = {p} AND destination = {d}); \
         DELETE FROM session_meals WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM session_activities_zh WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM activities WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM timesofday WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM days WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM booking_status WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM flight_legs WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM date_anchors WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM cascade_dirty_flags WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM process_statuses WHERE plan_id = {p} AND destination = {d}; \
         DELETE FROM plan_event_data WHERE plan_id = {p}; \
         DELETE FROM plan_events WHERE plan_id = {p}; \
         DELETE FROM operation_runs WHERE plan_id = {p}; \
         DELETE FROM plan_metadata WHERE plan_id = {p}; \
         DELETE FROM plan_destinations WHERE plan_id = {p}; \
         DELETE FROM plans WHERE plan_id = {p};"
    ));
}

fn seed_plan_anchor_and_flights(plan: &str, dest: &str) {
    let p = sql_lit(plan);
    let d = sql_lit(dest);
    exec_ok(&format!(
        "INSERT OR REPLACE INTO plans (plan_id, schema_version, version) \
         VALUES ({p}, '4.2.0', 11)"
    ));
    exec_ok(&format!(
        "INSERT OR REPLACE INTO plan_metadata (plan_id, schema_version, active_destination) \
         VALUES ({p}, '4.2.0', {d})"
    ));
    exec_ok(&format!(
        "INSERT OR REPLACE INTO plan_destinations (plan_id, slug, display_name, status) \
         VALUES ({p}, {d}, 'Scaffold Lock', 'draft')"
    ));
    exec_ok(&format!(
        "INSERT OR REPLACE INTO date_anchors \
            (plan_id, destination, start_date, end_date, days) \
         VALUES ({p}, {d}, '2026-09-04', '2026-09-06', 3)"
    ));
    exec_ok(&format!(
        "INSERT OR REPLACE INTO flight_legs \
            (plan_id, destination, direction, leg_order, flight_number, airline, \
             airline_code, departure_airport, departure_code, departure_terminal, \
             departure_time, arrival_airport, arrival_code, arrival_terminal, \
             arrival_time, flight_date, populated_from, booked_date) \
         VALUES ({p}, {d}, 'outbound', 0, 'CI120', 'China Airlines', \
             'CI', 'Taoyuan', 'TPE', '1', '08:15', 'Naha', 'OKA', 'I', \
             '10:45', '2026-09-04', 'test', '2026-09-04')"
    ));
    exec_ok(&format!(
        "INSERT OR REPLACE INTO flight_legs \
            (plan_id, destination, direction, leg_order, flight_number, airline, \
             airline_code, departure_airport, departure_code, departure_terminal, \
             departure_time, arrival_airport, arrival_code, arrival_terminal, \
             arrival_time, flight_date, populated_from, booked_date) \
         VALUES ({p}, {d}, 'return', 0, 'CI121', 'China Airlines', \
             'CI', 'Naha', 'OKA', 'I', '11:55', 'Taoyuan', 'TPE', '1', \
             '12:30', '2026-09-06', 'test', '2026-09-06')"
    ));
}

fn seed_stale_itinerary(plan: &str, dest: &str, stale_activity_id: &str) {
    let p = sql_lit(plan);
    let d = sql_lit(dest);
    let a = sql_lit(stale_activity_id);
    exec_ok(&format!(
        "INSERT OR REPLACE INTO days \
            (plan_id, destination, day_number, date, theme, day_type, status) \
         VALUES ({p}, {d}, 99, '2026-01-01', 'stale theme', 'full', 'confirmed')"
    ));
    exec_ok(&format!(
        "INSERT OR REPLACE INTO timesofday \
            (plan_id, destination, day_number, session_type, focus, transit_notes, booking_notes) \
         VALUES ({p}, {d}, 99, 'morning', 'stale focus', 'stale transit', 'stale booking')"
    ));
    exec_ok(&format!(
        "INSERT OR REPLACE INTO activities \
            (id, plan_id, destination, day_number, session_type, sort_order, title, booking_required) \
         VALUES ({a}, {p}, {d}, 99, 'morning', 0, 'stale activity', 1)"
    ));
    exec_ok(&format!(
        "INSERT OR REPLACE INTO activity_tags (activity_id, tag) \
         VALUES ({a}, 'stale-tag')"
    ));
    exec_ok(&format!(
        "INSERT OR REPLACE INTO session_meals \
            (plan_id, destination, day_number, session_type, sort_order, meal) \
         VALUES ({p}, {d}, 99, 'morning', 0, 'stale meal')"
    ));
    exec_ok(&format!(
        "INSERT OR REPLACE INTO session_activities_zh \
            (plan_id, destination, day_number, session_type, sort_order, activity) \
         VALUES ({p}, {d}, 99, 'morning', 0, 'stale zh activity')"
    ));
    exec_ok(&format!(
        "INSERT OR REPLACE INTO process_statuses \
            (plan_id, destination, process_id, status) \
         VALUES ({p}, {d}, 'process_5_daily_itinerary', 'pending')"
    ));
    exec_ok(&format!(
        "INSERT OR REPLACE INTO cascade_dirty_flags \
            (plan_id, destination, process_id, dirty, last_changed) \
         VALUES ({p}, {d}, 'process_5_daily_itinerary', 1, CURRENT_TIMESTAMP)"
    ));
}

fn run_scaffold(plan: &str, dest: &str, force: bool) -> (bool, String, String) {
    let mut cmd = Command::new(bin());
    cmd.env("TRAVEL_PLAN_ID", plan)
        .args(["scaffold-itinerary", "--dest", dest]);
    if force {
        cmd.arg("--force");
    }
    let out = cmd.output().expect("run scaffold-itinerary");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn scaffold_itinerary_refuses_then_replaces_and_audits_current_surface() {
    let _lock = SCAFFOLD_ITINERARY_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());

    let (ok, _out, err) = db_exec("SELECT 1 AS n");
    if !ok && is_skip(&err) {
        eprintln!(
            "skipping scaffold-itinerary lock (no Turso creds): {}",
            err.trim()
        );
        return;
    }
    assert!(ok, "db exec probe failed; err={err}");

    let tag = nanos();
    let plan = format!("zztest-{tag}");
    let dest = format!("zztest_{tag}");
    let stale_activity_id = format!("{plan}-stale-activity");

    let _g = Guard::new({
        let (plan, dest, stale_activity_id) =
            (plan.clone(), dest.clone(), stale_activity_id.clone());
        move || teardown(&plan, &dest, &stale_activity_id)
    });
    teardown(&plan, &dest, &stale_activity_id);

    seed_plan_anchor_and_flights(&plan, &dest);
    seed_stale_itinerary(&plan, &dest, &stale_activity_id);

    let (ok, out, err) = run_scaffold(&plan, &dest, false);
    if !ok && is_skip(&err) {
        eprintln!(
            "skipping scaffold-itinerary lock (no Turso creds): {}",
            err.trim()
        );
        return;
    }
    assert!(
        !ok,
        "scaffold-itinerary without --force should refuse existing days; out={out}; err={err}"
    );
    assert!(
        err.contains("Itinerary already has days. Use --force to overwrite."),
        "refusal message should match current command; err={err}; out={out}"
    );

    let p = sql_lit(&plan);
    let d = sql_lit(&dest);
    let a = sql_lit(&stale_activity_id);

    let refusal_rows = exec_ok(&format!(
        "SELECT \
            (SELECT COUNT(*) FROM days WHERE plan_id = {p} AND destination = {d} AND day_number = 99) || '|' || \
            (SELECT COUNT(*) FROM timesofday WHERE plan_id = {p} AND destination = {d} AND day_number = 99) || '|' || \
            (SELECT COUNT(*) FROM activities WHERE plan_id = {p} AND destination = {d} AND id = {a}) || '|' || \
            (SELECT COUNT(*) FROM activity_tags WHERE activity_id = {a}) || '|' || \
            (SELECT COUNT(*) FROM session_meals WHERE plan_id = {p} AND destination = {d} AND day_number = 99) || '|' || \
            (SELECT COUNT(*) FROM session_activities_zh WHERE plan_id = {p} AND destination = {d} AND day_number = 99) AS v"
    ));
    assert_eq!(
        scalar(&refusal_rows).as_deref(),
        Some("1|1|1|1|1|1"),
        "no-force refusal must not clobber stale itinerary rows; out={refusal_rows}"
    );
    let pre_audit = exec_ok(&format!(
        "SELECT \
            (SELECT COUNT(*) FROM operation_runs WHERE plan_id = {p}) || '|' || \
            (SELECT version FROM plans WHERE plan_id = {p}) AS v"
    ));
    assert_eq!(
        scalar(&pre_audit).as_deref(),
        Some("0|11"),
        "refusal must not audit or bump version; out={pre_audit}"
    );

    let (ok, out, err) = run_scaffold(&plan, &dest, true);
    if !ok && is_skip(&err) {
        eprintln!(
            "skipping scaffold-itinerary lock (no Turso creds): {}",
            err.trim()
        );
        return;
    }
    assert!(
        ok,
        "forced scaffold-itinerary should succeed; err={err}\nout={out}"
    );

    let stale_counts = exec_ok(&format!(
        "SELECT \
            (SELECT COUNT(*) FROM days WHERE plan_id = {p} AND destination = {d} AND day_number = 99) || '|' || \
            (SELECT COUNT(*) FROM timesofday WHERE plan_id = {p} AND destination = {d} AND day_number = 99) || '|' || \
            (SELECT COUNT(*) FROM activities WHERE plan_id = {p} AND destination = {d}) || '|' || \
            (SELECT COUNT(*) FROM activity_tags WHERE activity_id = {a}) || '|' || \
            (SELECT COUNT(*) FROM session_meals WHERE plan_id = {p} AND destination = {d}) || '|' || \
            (SELECT COUNT(*) FROM session_activities_zh WHERE plan_id = {p} AND destination = {d}) AS v"
    ));
    assert_eq!(
        scalar(&stale_counts).as_deref(),
        Some("0|0|0|0|0|0"),
        "forced scaffold should clear stale child rows and prove activity_tags were cleared before activities; out={stale_counts}"
    );

    let days = exec_ok(&format!(
        "SELECT day_number || '|' || date || '|' || day_type || '|' || \
                COALESCE(theme, '<NULL>') || '|' || status || '|' || (updated_at IS NOT NULL) AS v \
         FROM days WHERE plan_id = {p} AND destination = {d} ORDER BY day_number"
    ));
    assert_eq!(
        column(&days),
        vec![
            "1|2026-09-04|arrival|<NULL>|draft|1".to_string(),
            "2|2026-09-05|full|<NULL>|draft|1".to_string(),
            "3|2026-09-06|departure|<NULL>|draft|1".to_string(),
        ],
        "days should be ascending over the inclusive date anchor; out={days}"
    );

    let session_counts = exec_ok(&format!(
        "SELECT day_number || ':' || COUNT(*) AS v \
         FROM timesofday WHERE plan_id = {p} AND destination = {d} \
         GROUP BY day_number ORDER BY day_number"
    ));
    assert_eq!(
        column(&session_counts),
        vec!["1:4", "2:4", "3:4"],
        "each inserted day should have four sessions; out={session_counts}"
    );
    let sessions = exec_ok(&format!(
        "SELECT day_number || '|' || session_type || '|' || COALESCE(focus, '<NULL>') || '|' || \
                COALESCE(transit_notes, '<NULL>') || '|' || COALESCE(booking_notes, '<NULL>') || \
                '|' || (updated_at IS NOT NULL) AS v \
         FROM timesofday WHERE plan_id = {p} AND destination = {d} \
         ORDER BY day_number, CASE session_type \
             WHEN 'morning' THEN 0 WHEN 'noon' THEN 1 WHEN 'afternoon' THEN 2 ELSE 3 END"
    ));
    let arrow = '\u{2192}';
    assert_eq!(
        column(&sessions),
        vec![
            format!("1|morning|<NULL>|Arrive OKA 10:45 {arrow} hotel|<NULL>|1"),
            "1|noon|<NULL>|<NULL>|<NULL>|1".to_string(),
            "1|afternoon|<NULL>|<NULL>|<NULL>|1".to_string(),
            "1|evening|<NULL>|<NULL>|<NULL>|1".to_string(),
            "2|morning|<NULL>|<NULL>|<NULL>|1".to_string(),
            "2|noon|<NULL>|<NULL>|<NULL>|1".to_string(),
            "2|afternoon|<NULL>|<NULL>|<NULL>|1".to_string(),
            "2|evening|<NULL>|<NULL>|<NULL>|1".to_string(),
            "3|morning|<NULL>|<NULL>|<NULL>|1".to_string(),
            "3|noon|<NULL>|<NULL>|<NULL>|1".to_string(),
            "3|afternoon|<NULL>|<NULL>|<NULL>|1".to_string(),
            format!("3|evening|<NULL>|Hotel {arrow} OKA for 11:55 flight|<NULL>|1"),
        ],
        "timesofday rows should match current session order and transit-note behavior; out={sessions}"
    );

    let status_dirty = exec_ok(&format!(
        "SELECT \
            (SELECT status FROM process_statuses \
             WHERE plan_id = {p} AND destination = {d} AND process_id = 'process_5_daily_itinerary') || '|' || \
            (SELECT dirty FROM cascade_dirty_flags \
             WHERE plan_id = {p} AND destination = {d} AND process_id = 'process_5_daily_itinerary') AS v"
    ));
    assert_eq!(
        scalar(&status_dirty).as_deref(),
        Some("researching|0"),
        "process_statuses and cascade_dirty_flags should land; out={status_dirty}"
    );

    let operation_count = exec_ok(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id = {p}"
    ));
    assert_eq!(
        scalar(&operation_count).as_deref(),
        Some("1"),
        "one operation run total"
    );
    let operation = exec_ok(&format!(
        "SELECT command_type || '|' || command_summary || '|' || \
                version_before || '>' || COALESCE(version_after, -1) || '|' || status AS v \
         FROM operation_runs WHERE plan_id = {p}"
    ));
    assert_eq!(
        scalar(&operation).as_deref(),
        Some(format!("scaffold-itinerary|{dest} 3 days|11>12|completed").as_str()),
        "operation_runs should record the current scaffold-itinerary audit row; out={operation}"
    );
    let version = exec_ok(&format!(
        "SELECT version AS v FROM plans WHERE plan_id = {p}"
    ));
    assert_eq!(
        scalar(&version).as_deref(),
        Some("12"),
        "plans.version should bump by one"
    );

    let events = exec_ok(&format!(
        "SELECT scope || '|' || destination || '|' || process_id || '|' || sort_order || '|' || \
                event || '|' || COALESCE(from_state, '<NULL>') || '>' || COALESCE(to_state, '<NULL>') AS v \
         FROM plan_events WHERE plan_id = {p} \
         ORDER BY CASE scope WHEN 'dest_process' THEN 0 ELSE 1 END, sort_order"
    ));
    assert_eq!(
        column(&events),
        vec![
            format!("dest_process|{dest}|process_5_daily_itinerary|0|itinerary_scaffolded|<NULL>><NULL>"),
            "timeline||process_5_daily_itinerary|0|itinerary_scaffolded|<NULL>><NULL>".to_string(),
        ],
        "plan_events should include current dest_process and timeline emissions; out={events}"
    );

    let event_data = exec_ok(&format!(
        "SELECT scope || '|' || destination || '|' || process_id || '|' || sort_order || '|' || \
                key || '=' || COALESCE(value, '<NULL>') AS v \
         FROM plan_event_data WHERE plan_id = {p} \
         ORDER BY CASE scope WHEN 'dest_process' THEN 0 ELSE 1 END, key"
    ));
    assert_eq!(
        column(&event_data),
        vec![
            format!("dest_process|{dest}|process_5_daily_itinerary|0|day_types=arrival, full, departure"),
            format!("dest_process|{dest}|process_5_daily_itinerary|0|days_count=3"),
            "timeline||process_5_daily_itinerary|0|day_types=arrival, full, departure".to_string(),
            "timeline||process_5_daily_itinerary|0|days_count=3".to_string(),
        ],
        "plan_event_data should include the current two KV pairs for both emitted scopes; out={event_data}"
    );
}
