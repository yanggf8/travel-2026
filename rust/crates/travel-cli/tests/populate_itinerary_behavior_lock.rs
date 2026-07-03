//! Real-Turso behavior lock for `populate-itinerary` before its DAL migration.
//!
//! This drives the current binary and locks the inline SQL behavior: destination
//! ref reads, allocation/session ordering, empty-only theme/focus writes,
//! case-insensitive activity dedupe unless `--force`, ordered event flushing, one
//! operation audit row per invocation, and one plan version bump per invocation.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard};

static LOCK: Mutex<()> = Mutex::new(());

fn exec_ok(sql: &str) -> common::Rows {
    db_exec(sql).unwrap_or_else(|| panic!("db exec skipped unexpectedly for SQL: {sql}"))
}

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn planned_count(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|l| {
        l.trim()
            .strip_prefix("Planned additions: ")
            .map(str::to_string)
    })
}

fn teardown(plan: &str, dest: &str) {
    let p = sql_lit(plan);
    let d = sql_lit(dest);
    let _ = db_exec_teardown(&format!(
        "DELETE FROM activity_tags \
            WHERE activity_id IN (SELECT id FROM activities WHERE plan_id = {p} AND destination = {d});"
    ));
    teardown_plan(plan, dest);
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_poi_tags WHERE slug = {d}; \
         DELETE FROM destination_cluster_pois WHERE slug = {d}; \
         DELETE FROM destination_pois WHERE slug = {d}; \
         DELETE FROM destination_clusters WHERE slug = {d}; \
         DELETE FROM destination_areas WHERE slug = {d}; \
         DELETE FROM destination_config WHERE slug = {d};"
    ));
}

fn seed_plan_and_destination(plan: &str, dest: &str) {
    let d = sql_lit(dest);
    seed_plan(plan, dest, 40);
    exec_ok(&format!(
        "INSERT INTO destination_config (slug, display_name, ref_id, ref_path, timezone, currency, language, origin) \
           VALUES ({d}, 'Populate Lock Destination', 'zz-ref', 'turso:destination-ref/populate-lock', \
                   'Asia/Tokyo', 'JPY', 'ja', 'taiwan'); \
         INSERT INTO destination_areas (slug, area_id, name, type, vibe, source_url, fetched_at, confidence) VALUES \
           ({d}, 'alpha_area', 'Alpha Ward', 'district', 'walkable', 'test', '2026-07-02', 'test'), \
           ({d}, 'beta_area', 'Beta Bay', 'district', 'waterfront', 'test', '2026-07-02', 'test'), \
           ({d}, 'last_area', 'Last Stop', 'district', 'departure', 'test', '2026-07-02', 'test'); \
         INSERT INTO destination_clusters \
           (slug, cluster_id, name, description, duration_min, best_area, source_url, fetched_at, confidence) VALUES \
           ({d}, 'alpha_core', 'Alpha Cluster', 'alpha desc', 300, 'alpha_area', 'test', '2026-07-02', 'test'), \
           ({d}, 'beta_core', 'Beta Cluster', 'beta desc', 240, 'beta_area', 'test', '2026-07-02', 'test'), \
           ({d}, 'souvenir_last_day', 'Last Day Cluster', 'last desc', 180, 'last_area', 'test', '2026-07-02', 'test'); \
         INSERT INTO destination_cluster_pois (slug, cluster_id, poi_id, sort_order) VALUES \
           ({d}, 'alpha_core', 'alpha_museum', 0), \
           ({d}, 'alpha_core', 'alpha_cafe', 1), \
           ({d}, 'alpha_core', 'alpha_gallery', 2), \
           ({d}, 'beta_core', 'beta_tower', 0), \
           ({d}, 'beta_core', 'beta_market', 1), \
           ({d}, 'souvenir_last_day', 'last_shrine', 0), \
           ({d}, 'souvenir_last_day', 'last_station', 1); \
         INSERT INTO destination_pois \
           (slug, poi_id, title, area, nearest_station, duration_min, booking_required, booking_url, \
            cost_estimate, notes, hours, address, source_url, fetched_at, confidence) VALUES \
           ({d}, 'alpha_museum', 'Alpha Museum', 'alpha_area', 'Alpha Sta', 90, 1, 'https://example.test/alpha-museum', \
            1200, 'Reserve ahead', '09:00-17:00', '1 Alpha Rd', 'test', '2026-07-02', 'test'), \
           ({d}, 'alpha_cafe', 'Alpha Cafe', 'alpha_area', 'Alpha Sta', 45, 0, NULL, \
            800, 'Coffee break', '', '2 Alpha Rd', 'test', '2026-07-02', 'test'), \
           ({d}, 'alpha_gallery', 'Alpha Gallery', 'alpha_area', NULL, 60, 0, NULL, \
            NULL, '', '10:00-18:00', '', 'test', '2026-07-02', 'test'), \
           ({d}, 'beta_tower', 'Beta Tower', 'beta_area', 'Beta Sta', 80, 0, NULL, \
            1500, 'Observation deck', '10:00-20:00', '1 Beta Bay', 'test', '2026-07-02', 'test'), \
           ({d}, 'beta_market', 'Beta Market', 'beta_area', 'Market Sta', 70, 0, NULL, \
            500, NULL, NULL, NULL, 'test', '2026-07-02', 'test'), \
           ({d}, 'last_shrine', 'Last Shrine', 'last_area', 'Last Sta', 50, 0, NULL, \
            0, 'Final stop', NULL, '9 Last Ln', 'test', '2026-07-02', 'test'), \
           ({d}, 'last_station', 'Last Station', 'last_area', 'Airport Line', 40, 0, NULL, \
            NULL, NULL, NULL, NULL, 'test', '2026-07-02', 'test'); \
         INSERT INTO destination_poi_tags (slug, poi_id, tag, sort_order) VALUES \
           ({d}, 'alpha_museum', 'museum', 0), \
           ({d}, 'alpha_museum', 'ticketed', 1), \
           ({d}, 'alpha_cafe', 'food', 0), \
           ({d}, 'beta_tower', 'view', 0), \
           ({d}, 'beta_market', 'shopping', 0), \
           ({d}, 'last_shrine', 'culture', 0);"
    ));
}

fn seed_itinerary_skeleton(plan: &str, dest: &str) {
    let p = sql_lit(plan);
    let d = sql_lit(dest);
    let seed_activity = sql_lit(&format!("seed_case_dedupe_{dest}"));
    exec_ok(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status, updated_at) VALUES \
           ({p}, {d}, 1, '2026-09-01', NULL, 'arrival', 'draft', '2000-01-01 00:00:00'), \
           ({p}, {d}, 2, '2026-09-02', NULL, 'full', 'draft', '2000-01-01 00:00:00'), \
           ({p}, {d}, 3, '2026-09-03', NULL, 'full', 'draft', '2000-01-01 00:00:00'), \
           ({p}, {d}, 4, '2026-09-04', NULL, 'departure', 'draft', '2000-01-01 00:00:00'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, focus, transit_notes, booking_notes, updated_at) VALUES \
           ({p}, {d}, 1, 'morning', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 1, 'noon', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 1, 'afternoon', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 1, 'evening', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 2, 'morning', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 2, 'noon', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 2, 'afternoon', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 2, 'evening', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 3, 'morning', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 3, 'noon', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 3, 'afternoon', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 3, 'evening', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 4, 'morning', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 4, 'noon', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 4, 'afternoon', NULL, NULL, NULL, '2000-01-01 00:00:00'), \
           ({p}, {d}, 4, 'evening', NULL, NULL, NULL, '2000-01-01 00:00:00'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, area, \
            booking_required, booking_status, priority, is_fixed_time, updated_at) \
         VALUES ({seed_activity}, {p}, {d}, 2, 'morning', 0, 'alpha museum', 'Seed Area', \
                 0, 'not_required', 'want', 0, '2000-01-01 00:00:00');"
    ));
}

fn reset_day_markers(plan: &str, dest: &str) {
    exec_ok(&format!(
        "UPDATE days SET updated_at = '2000-01-01 00:00:00' \
         WHERE plan_id = {} AND destination = {}",
        sql_lit(plan),
        sql_lit(dest)
    ));
}

fn run_populate(plan: &str, extra: &[&str]) -> (bool, String, String) {
    let goals = "alpha_core,beta_core,souvenir_last_day";
    let mut cmd = Command::new(bin());
    cmd.args(["populate-itinerary", "--goals", goals, "--pace", "balanced"])
        .args(extra)
        .env("TRAVEL_PLAN_ID", plan);
    let out = cmd.output().expect("run populate-itinerary");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn assert_touched_days(plan: &str, dest: &str, expected: &[&str]) {
    let touched = exec_ok(&format!(
        "SELECT day_number AS value_text FROM days \
         WHERE plan_id = {} AND destination = {} \
           AND updated_at <> '2000-01-01 00:00:00' \
         ORDER BY day_number",
        sql_lit(plan),
        sql_lit(dest)
    ));
    assert_eq!(
        touched.column(),
        expected,
        "touched days should be exact; out={touched}"
    );
}

fn assert_activity_order_after_first(plan: &str, dest: &str) {
    let rows = exec_ok(&format!(
        "SELECT day_number || '|' || session_type || '|' || sort_order || '|' || title || '|' || \
                COALESCE(area, '<NULL>') || '|' || COALESCE(nearest_station, '<NULL>') || '|' || \
                COALESCE(duration_min, -1) || '|' || booking_required || '|' || \
                COALESCE(booking_url, '<NULL>') || '|' || COALESCE(cost_estimate, -1) || '|' || \
                COALESCE(notes, '<NULL>') || '|' || priority AS row_text \
         FROM activities \
         WHERE plan_id = {} AND destination = {} \
         ORDER BY day_number, \
                  CASE session_type WHEN 'morning' THEN 0 WHEN 'noon' THEN 1 WHEN 'afternoon' THEN 2 ELSE 3 END, \
                  sort_order, title",
        sql_lit(plan),
        sql_lit(dest)
    ));
    assert_eq!(
        rows.column(),
        vec![
            "2|morning|0|alpha museum|Seed Area|<NULL>|-1|0|<NULL>|-1|<NULL>|want",
            "2|morning|1|Alpha Gallery|Alpha Ward|<NULL>|60|0|<NULL>|-1|Hours: 10:00-18:00|want",
            "2|noon|0|Alpha Cafe|Alpha Ward|Alpha Sta|45|0|<NULL>|800|Coffee break | Address: 2 Alpha Rd|want",
            "3|morning|0|Beta Tower|Beta Bay|Beta Sta|80|0|<NULL>|1500|Observation deck | Hours: 10:00-20:00 | Address: 1 Beta Bay|want",
            "3|noon|0|Beta Market|Beta Bay|Market Sta|70|0|<NULL>|500|<NULL>|want",
            "4|morning|0|Last Shrine|Last Stop|Last Sta|50|0|<NULL>|0|Final stop | Address: 9 Last Ln|want",
            "4|noon|0|Last Station|Last Stop|Airport Line|40|0|<NULL>|-1|<NULL>|want",
        ],
        "allocation, session order, sort_order, and inserted activity fields should match current behavior; out={rows}"
    );
}

fn event_rows(plan: &str, scope: &str, destination_filter: &str) -> Vec<String> {
    let rows = exec_ok(&format!(
        "SELECT e.sort_order || '|' || e.event || '|' || \
                COALESCE((SELECT value FROM plan_event_data ed \
                          WHERE ed.plan_id = e.plan_id AND ed.scope = e.scope \
                            AND ed.destination = e.destination AND ed.process_id = e.process_id \
                            AND ed.sort_order = e.sort_order AND ed.key = 'day_number'), '') || '|' || \
                COALESCE((SELECT value FROM plan_event_data ed \
                          WHERE ed.plan_id = e.plan_id AND ed.scope = e.scope \
                            AND ed.destination = e.destination AND ed.process_id = e.process_id \
                            AND ed.sort_order = e.sort_order AND ed.key = 'session'), '') || '|' || \
                COALESCE((SELECT value FROM plan_event_data ed \
                          WHERE ed.plan_id = e.plan_id AND ed.scope = e.scope \
                            AND ed.destination = e.destination AND ed.process_id = e.process_id \
                            AND ed.sort_order = e.sort_order AND ed.key = 'title'), \
                         (SELECT value FROM plan_event_data ed \
                          WHERE ed.plan_id = e.plan_id AND ed.scope = e.scope \
                            AND ed.destination = e.destination AND ed.process_id = e.process_id \
                            AND ed.sort_order = e.sort_order AND ed.key = 'theme'), \
                         (SELECT value FROM plan_event_data ed \
                          WHERE ed.plan_id = e.plan_id AND ed.scope = e.scope \
                            AND ed.destination = e.destination AND ed.process_id = e.process_id \
                            AND ed.sort_order = e.sort_order AND ed.key = 'focus'), '') AS row_text \
         FROM plan_events e \
         WHERE e.plan_id = {} AND e.scope = {} AND e.destination = {} \
           AND e.process_id = 'process_5_daily_itinerary' \
         ORDER BY e.sort_order",
        sql_lit(plan),
        sql_lit(scope),
        sql_lit(destination_filter)
    ));
    rows.column()
}

#[test]
fn populate_itinerary_locks_current_write_surface() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping populate-itinerary behavior lock (no Turso creds)");
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

    seed_plan_and_destination(&plan, &dest);
    seed_itinerary_skeleton(&plan, &dest);

    let (ok, out, err) = run_populate(&plan, &[]);
    if !ok && is_credless(&err) {
        eprintln!(
            "skipping populate-itinerary behavior lock (no Turso creds): {}",
            err.trim()
        );
        return;
    }
    assert!(
        ok,
        "populate-itinerary should succeed; err={err}\nout={out}"
    );
    assert_eq!(
        planned_count(&out).as_deref(),
        Some("6"),
        "stdout planned count; out={out}"
    );
    assert_touched_days(&plan, &dest, &["2", "3", "4"]);
    assert_activity_order_after_first(&plan, &dest);

    let tags = exec_ok(&format!(
        "SELECT a.title || ':' || at.tag AS row_text \
         FROM activity_tags at \
         JOIN activities a ON a.id = at.activity_id \
         WHERE a.plan_id = {} AND a.destination = {} \
         ORDER BY a.title, at.tag",
        sql_lit(&plan),
        sql_lit(&dest)
    ));
    assert_eq!(
        tags.column(),
        vec![
            "Alpha Cafe:food",
            "Beta Market:shopping",
            "Beta Tower:view",
            "Last Shrine:culture",
        ],
        "tags should be inserted for added activities only; deduped Alpha Museum tags are absent; out={tags}"
    );

    let first_events = vec![
        "0|itinerary_day_theme_set|2||Alpha Cluster",
        "1|itinerary_session_focus_set|2|morning|Alpha Cluster",
        "2|itinerary_session_focus_set|2|noon|Alpha Cluster",
        "3|activity_added|2|morning|Alpha Gallery",
        "4|activity_added|2|noon|Alpha Cafe",
        "5|itinerary_day_theme_set|3||Beta Cluster",
        "6|itinerary_session_focus_set|3|morning|Beta Cluster",
        "7|itinerary_session_focus_set|3|noon|Beta Cluster",
        "8|activity_added|3|morning|Beta Tower",
        "9|activity_added|3|noon|Beta Market",
        "10|itinerary_day_theme_set|4||Last Day Cluster",
        "11|itinerary_session_focus_set|4|morning|Last Day Cluster",
        "12|itinerary_session_focus_set|4|noon|Last Day Cluster",
        "13|activity_added|4|morning|Last Shrine",
        "14|activity_added|4|noon|Last Station",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    assert_eq!(event_rows(&plan, "dest_process", &dest), first_events);
    assert_eq!(event_rows(&plan, "timeline", ""), first_events);

    let op = exec_ok(&format!(
        "SELECT command_type || '|' || command_summary || '|' || version_before || '>' || \
                COALESCE(version_after, -1) || '|' || status AS row_text \
         FROM operation_runs WHERE plan_id = {} ORDER BY version_before",
        sql_lit(&plan)
    ));
    assert_eq!(
        op.column(),
        vec![format!(
            "populate-itinerary|{dest} 6 activities|40>41|completed"
        )],
        "first invocation should hand-roll exactly one audit row and one version bump; out={op}"
    );
    let version = exec_ok(&format!(
        "SELECT version AS value_text FROM plans WHERE plan_id = {}",
        sql_lit(&plan)
    ));
    assert_eq!(version.scalar().as_deref(), Some("41"));

    exec_ok(&format!(
        "UPDATE days SET theme = 'Manual Theme', updated_at = '2000-01-01 00:00:00' \
         WHERE plan_id = {} AND destination = {} AND day_number = 2; \
         UPDATE timesofday SET focus = 'Manual Focus' \
         WHERE plan_id = {} AND destination = {} AND day_number = 2 AND session_type = 'morning';",
        sql_lit(&plan),
        sql_lit(&dest),
        sql_lit(&plan),
        sql_lit(&dest)
    ));
    reset_day_markers(&plan, &dest);

    let (ok, out, err) = run_populate(&plan, &[]);
    assert!(
        ok,
        "second populate-itinerary should succeed; err={err}\nout={out}"
    );
    assert_eq!(
        planned_count(&out).as_deref(),
        Some("0"),
        "dedupe should skip existing titles; out={out}"
    );
    assert_touched_days(&plan, &dest, &["2", "3", "4"]);

    let manual = exec_ok(&format!(
        "SELECT d.theme || '|' || t.focus AS row_text \
         FROM days d \
         JOIN timesofday t ON t.plan_id = d.plan_id AND t.destination = d.destination \
                           AND t.day_number = d.day_number AND t.session_type = 'morning' \
         WHERE d.plan_id = {} AND d.destination = {} AND d.day_number = 2",
        sql_lit(&plan),
        sql_lit(&dest)
    ));
    assert_eq!(
        manual.scalar().as_deref(),
        Some("Manual Theme|Manual Focus"),
        "non-force run must not overwrite non-empty theme/focus; out={manual}"
    );
    let count_after_second = exec_ok(&format!(
        "SELECT COUNT(*) AS value_text FROM activities WHERE plan_id = {} AND destination = {}",
        sql_lit(&plan),
        sql_lit(&dest)
    ));
    assert_eq!(
        count_after_second.scalar().as_deref(),
        Some("7"),
        "seed + six first-run activities only"
    );
    let event_count_after_second = exec_ok(&format!(
        "SELECT COUNT(*) AS value_text FROM plan_events WHERE plan_id = {} AND scope = 'timeline'",
        sql_lit(&plan)
    ));
    assert_eq!(
        event_count_after_second.scalar().as_deref(),
        Some("15"),
        "empty pending event flush should not create event rows"
    );

    reset_day_markers(&plan, &dest);
    let (ok, out, err) = run_populate(&plan, &["--force"]);
    assert!(
        ok,
        "force populate-itinerary should succeed; err={err}\nout={out}"
    );
    assert_eq!(
        planned_count(&out).as_deref(),
        Some("7"),
        "force bypasses title dedupe; out={out}"
    );
    assert_touched_days(&plan, &dest, &["2", "3", "4"]);

    let forced = exec_ok(&format!(
        "SELECT d.theme || '|' || t.focus AS row_text \
         FROM days d \
         JOIN timesofday t ON t.plan_id = d.plan_id AND t.destination = d.destination \
                           AND t.day_number = d.day_number AND t.session_type = 'morning' \
         WHERE d.plan_id = {} AND d.destination = {} AND d.day_number = 2",
        sql_lit(&plan),
        sql_lit(&dest)
    ));
    assert_eq!(
        forced.scalar().as_deref(),
        Some("Alpha Cluster|Alpha Cluster"),
        "force run should overwrite non-empty theme/focus; out={forced}"
    );

    let final_activity_counts = exec_ok(&format!(
        "SELECT COUNT(*) || '|' || \
                SUM(CASE WHEN title = 'Alpha Museum' THEN 1 ELSE 0 END) || '|' || \
                SUM(CASE WHEN lower(title) = 'alpha museum' THEN 1 ELSE 0 END) AS row_text \
         FROM activities WHERE plan_id = {} AND destination = {}",
        sql_lit(&plan),
        sql_lit(&dest)
    ));
    assert_eq!(
        final_activity_counts.scalar().as_deref(),
        Some("14|1|2"),
        "force should add seven duplicates, including Alpha Museum despite the lowercase seed; out={final_activity_counts}"
    );
    let final_tag_count = exec_ok(&format!(
        "SELECT COUNT(*) AS value_text \
         FROM activity_tags at \
         JOIN activities a ON a.id = at.activity_id \
         WHERE a.plan_id = {} AND a.destination = {}",
        sql_lit(&plan),
        sql_lit(&dest)
    ));
    assert_eq!(
        final_tag_count.scalar().as_deref(),
        Some("10"),
        "first run inserted four tags; force inserted six more"
    );

    let force_events = vec![
        "15|itinerary_day_theme_set|2||Alpha Cluster",
        "16|itinerary_session_focus_set|2|morning|Alpha Cluster",
        "17|itinerary_session_focus_set|2|noon|Alpha Cluster",
        "18|activity_added|2|morning|Alpha Museum",
        "19|activity_added|2|morning|Alpha Gallery",
        "20|activity_added|2|noon|Alpha Cafe",
        "21|itinerary_day_theme_set|3||Beta Cluster",
        "22|itinerary_session_focus_set|3|morning|Beta Cluster",
        "23|itinerary_session_focus_set|3|noon|Beta Cluster",
        "24|activity_added|3|morning|Beta Tower",
        "25|activity_added|3|noon|Beta Market",
        "26|itinerary_day_theme_set|4||Last Day Cluster",
        "27|itinerary_session_focus_set|4|morning|Last Day Cluster",
        "28|itinerary_session_focus_set|4|noon|Last Day Cluster",
        "29|activity_added|4|morning|Last Shrine",
        "30|activity_added|4|noon|Last Station",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    let mut all_events = first_events;
    all_events.extend(force_events);
    assert_eq!(event_rows(&plan, "dest_process", &dest), all_events);
    assert_eq!(event_rows(&plan, "timeline", ""), all_events);

    let operations = exec_ok(&format!(
        "SELECT command_type || '|' || command_summary || '|' || version_before || '>' || \
                COALESCE(version_after, -1) || '|' || status AS row_text \
         FROM operation_runs WHERE plan_id = {} ORDER BY version_before",
        sql_lit(&plan)
    ));
    assert_eq!(
        operations.column(),
        vec![
            format!("populate-itinerary|{dest} 6 activities|40>41|completed"),
            format!("populate-itinerary|{dest} 0 activities|41>42|completed"),
            format!("populate-itinerary|{dest} 7 activities|42>43|completed"),
        ],
        "each invocation should create exactly one audit row and one version bump; out={operations}"
    );
    let final_version = exec_ok(&format!(
        "SELECT version AS value_text FROM plans WHERE plan_id = {}",
        sql_lit(&plan)
    ));
    assert_eq!(
        final_version.scalar().as_deref(),
        Some("43"),
        "final plan version"
    );
}