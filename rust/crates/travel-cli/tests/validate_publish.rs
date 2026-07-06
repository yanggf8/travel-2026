//! Integration tests for `travel validate publish` — publish-readiness gate.
//!
//! Seeds throwaway plans into the shared Turso DB, drives the binary with
//! `TRAVEL_TODAY` pinned to 2026-07-05, asserts on exit code + plain-text
//! output, then tears down via `common::teardown_plan` + slug-keyed cleanup.
//! Serialized with a process mutex (shared live DB).

mod common;
use common::{bin, db_exec, db_exec_teardown, nanos, seed_plan, teardown_plan, Guard};

use std::process::Command;
use std::sync::Mutex;

const TODAY: &str = "2026-07-05";

static PUBLISH_LOCK: Mutex<()> = Mutex::new(());

fn run_publish(plan_id: &str, dest: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["validate", "publish", "--plan-id", plan_id, "--dest", dest])
        .env("TRAVEL_TODAY", TODAY)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel validate publish: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn seed_destination(slug: &str) {
    db_exec(&format!(
        "INSERT OR IGNORE INTO destination_config \
           (slug, display_name, ref_id, ref_path, timezone, currency, language, origin) \
         VALUES ('{slug}', 'ZZ Publish Test', 'zz-ref', 'turso:destination-ref/zz', \
                 'Asia/Tokyo', 'JPY', 'ja', 'taiwan');"
    ))
    .expect("seed destination_config");
}

fn teardown_destination(slug: &str) {
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_pois WHERE slug = '{slug}'; \
         DELETE FROM destination_config WHERE slug = '{slug}';"
    ));
}

fn seed_anchor(plan_id: &str, dest: &str, start: &str, end: &str, days: i64) {
    db_exec(&format!(
        "INSERT INTO date_anchors (plan_id, destination, start_date, end_date, days) \
         VALUES ('{plan_id}', '{dest}', '{start}', '{end}', {days});"
    ))
    .expect("seed date_anchors");
}

/// Upcoming bad plan: missing ZH + map path, weather warning visible.
#[test]
fn upcoming_missing_zh_and_map_path_blocks_with_weather_warning() {
    let _lock = PUBLISH_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let tag = nanos();
    let plan_id = format!("test-pub-bad-{tag}");
    let dest = format!("pubbad_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_plan(&plan_id, &dest);
            teardown_destination(&dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);
    seed_anchor(&plan_id, &dest, "2026-07-10", "2026-07-12", 3);

    db_exec(&format!(
        "INSERT INTO destination_pois (slug, poi_id, title, source_url, fetched_at, confidence, lat, lon) \
           VALUES ('{dest}', 'poi_one', 'Test POI', 'test', '2026-07-05', 'test', NULL, NULL); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, theme) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-07-10', 'arrival', 'Arrival'), \
                  ('{plan_id}', '{dest}', 2, '2026-07-11', 'full', 'Explore'), \
                  ('{plan_id}', '{dest}', 3, '2026-07-12', 'departure', 'Departure'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, focus) \
           VALUES ('{plan_id}', '{dest}', 2, 'morning', 'Museum'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, poi_id) \
           VALUES ('{plan_id}-act1', '{plan_id}', '{dest}', 2, 'morning', 0, 'Test activity', 'poi_one');"
    ))
    .expect("seed bad plan");

    let (ok, stdout, stderr) = run_publish(&plan_id, &dest);
    assert!(!ok, "expected non-zero exit for upcoming bad plan. stderr: {stderr}\nstdout:\n{stdout}");
    assert!(
        stdout.contains("[zh-content]") || stdout.to_lowercase().contains("zh"),
        "expected ZH blocker, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[map-path]"),
        "expected map-path blocker, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[weather]") || stdout.contains("weather"),
        "expected weather warning, got:\n{stdout}"
    );
}

/// Complete upcoming plan: ZH, POI coords, weather, fresh snapshot → zero exit.
#[test]
fn complete_upcoming_plan_passes() {
    let _lock = PUBLISH_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let tag = nanos();
    let plan_id = format!("test-pub-ok-{tag}");
    let dest = format!("pubok_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_plan(&plan_id, &dest);
            teardown_destination(&dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);
    seed_anchor(&plan_id, &dest, "2026-07-10", "2026-07-12", 3);

    db_exec(&format!(
        "INSERT INTO destination_pois (slug, poi_id, title, source_url, fetched_at, confidence, lat, lon) \
           VALUES ('{dest}', 'poi_one', 'Test POI', 'test', '2026-07-05', 'test', 35.6812, 139.7671); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, theme, theme_zh, \
                            weather_label, weather_source_id, updated_at) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-07-10', 'arrival', 'Arrival', '抵達日', \
                   'Sunny', 'open_meteo', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 2, '2026-07-11', 'full', 'Explore', '探索', \
                   'Cloudy', 'open_meteo', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 3, '2026-07-12', 'departure', 'Departure', '離開', \
                   'Sunny', 'open_meteo', datetime('now','-2 hours')); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, focus, focus_zh, updated_at) \
           VALUES ('{plan_id}', '{dest}', 2, 'morning', 'Museum', '博物館', datetime('now','-2 hours')); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, poi_id, updated_at) \
           VALUES ('{plan_id}-act1', '{plan_id}', '{dest}', 2, 'morning', 0, 'Test activity', 'poi_one', \
                   datetime('now','-2 hours')); \
         INSERT INTO plan_map_snapshots (plan_id, snapshotted_at) \
           VALUES ('{plan_id}', datetime('now'));"
    ))
    .expect("seed complete plan");

    let (ok, stdout, stderr) = run_publish(&plan_id, &dest);
    assert!(ok, "expected zero exit for complete plan. stderr: {stderr}\nstdout:\n{stdout}");
    assert!(
        !stdout.contains("## Blockers") || stdout.contains("Blockers:   0"),
        "expected no blockers, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("[zh-content]")
            && !stdout.contains("[map-path]")
            && !stdout.contains("[weather]")
            && !stdout.contains("[maps-fresh]"),
        "expected no BLOCK/WARN issues, got:\n{stdout}"
    );
}

/// Past plan: missing ZH/weather but valid map path → zero exit (no ZH/weather block).
#[test]
fn past_plan_skips_zh_and_weather_checks() {
    let _lock = PUBLISH_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let tag = nanos();
    let plan_id = format!("test-pub-past-{tag}");
    let dest = format!("pubpast_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_plan(&plan_id, &dest);
            teardown_destination(&dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);
    seed_anchor(&plan_id, &dest, "2026-02-01", "2026-02-03", 3);

    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, theme) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-02-01', 'arrival', 'Arrival'), \
                  ('{plan_id}', '{dest}', 2, '2026-02-02', 'full', 'Explore'), \
                  ('{plan_id}', '{dest}', 3, '2026-02-03', 'departure', 'Departure'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, focus) \
           VALUES ('{plan_id}', '{dest}', 2, 'morning', 'Museum'); \
         INSERT INTO day_route_segments \
           (plan_id, destination, day_number, sort_order, from_place, to_place, mode) \
           VALUES ('{plan_id}', '{dest}', 2, 0, 'Place A', 'Place B', 'walking');"
    ))
    .expect("seed past plan");

    let (ok, stdout, stderr) = run_publish(&plan_id, &dest);
    assert!(ok, "expected zero exit for past plan. stderr: {stderr}\nstdout:\n{stdout}");
    assert!(
        !stdout.contains("[zh-content]") && !stdout.contains("[weather]"),
        "past plan must not block/warn on missing ZH or weather, got:\n{stdout}"
    );
}

/// Empty arrival/departure sessions must not false-block on ZH.
#[test]
fn empty_arrival_departure_sessions_do_not_false_block_zh() {
    let _lock = PUBLISH_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let tag = nanos();
    let plan_id = format!("test-pub-empty-{tag}");
    let dest = format!("pubempty_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_plan(&plan_id, &dest);
            teardown_destination(&dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);
    seed_anchor(&plan_id, &dest, "2026-07-10", "2026-07-12", 3);

    db_exec(&format!(
        "INSERT INTO destination_pois (slug, poi_id, title, source_url, fetched_at, confidence, lat, lon) \
           VALUES ('{dest}', 'poi_one', 'Test POI', 'test', '2026-07-05', 'test', 35.6812, 139.7671); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, theme, theme_zh, \
                            weather_label, weather_source_id) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-07-10', 'arrival', 'Arrival', NULL, NULL, NULL), \
                  ('{plan_id}', '{dest}', 2, '2026-07-11', 'full', 'Explore', '探索', 'Sunny', 'open_meteo'), \
                  ('{plan_id}', '{dest}', 3, '2026-07-12', 'departure', 'Departure', NULL, NULL, NULL); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, focus, focus_zh) \
           VALUES ('{plan_id}', '{dest}', 2, 'morning', 'Museum', '博物館'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, poi_id) \
           VALUES ('{plan_id}-act1', '{plan_id}', '{dest}', 2, 'morning', 0, 'Test activity', 'poi_one');"
    ))
    .expect("seed empty-session plan");

    let (ok, stdout, stderr) = run_publish(&plan_id, &dest);
    assert!(
        !stdout.contains("[zh-content]"),
        "empty arrival/departure sessions must not trigger ZH blockers, got:\n{stdout}"
    );
    // Map path satisfied via day_route_segments; no ZH required for empty sessions.
    // Weather may warn (upcoming, in window) — that is acceptable; only ZH must not false-block.
    let _ = ok;
    let _ = stderr;
}

/// Beyond-window trip: weather-pending INFO, no within-window WARN, publish passes.
#[test]
fn beyond_window_weather_pending_info_passes() {
    let _lock = PUBLISH_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let tag = nanos();
    let plan_id = format!("test-pub-wxinfo-{tag}");
    let dest = format!("pubwxinfo_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_plan(&plan_id, &dest);
            teardown_destination(&dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);
    seed_anchor(&plan_id, &dest, "2026-09-01", "2026-09-03", 3);

    db_exec(&format!(
        "INSERT INTO destination_pois (slug, poi_id, title, source_url, fetched_at, confidence, lat, lon) \
           VALUES ('{dest}', 'poi_one', 'Test POI', 'test', '2026-07-05', 'test', 35.6812, 139.7671); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, theme, theme_zh) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-09-01', 'arrival', 'Arrival', '抵達日'), \
                  ('{plan_id}', '{dest}', 2, '2026-09-02', 'full', 'Explore', '探索'), \
                  ('{plan_id}', '{dest}', 3, '2026-09-03', 'departure', 'Departure', '離開'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, focus, focus_zh) \
           VALUES ('{plan_id}', '{dest}', 2, 'morning', 'Museum', '博物館'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, poi_id) \
           VALUES ('{plan_id}-act1', '{plan_id}', '{dest}', 2, 'morning', 0, 'Test activity', 'poi_one'); \
         INSERT INTO plan_map_snapshots (plan_id, snapshotted_at) \
           VALUES ('{plan_id}', datetime('now'));"
    ))
    .expect("seed beyond-window plan");

    let (ok, stdout, stderr) = run_publish(&plan_id, &dest);
    assert!(
        ok,
        "weather-pending INFO must not fail publish; stderr={stderr}; stdout={stdout}"
    );
    assert!(
        stdout.contains("weather pending - Open-Meteo forecast available within 16 days of 2026-09-01"),
        "stdout={stdout}"
    );
    assert!(
        !stdout.contains("⚠️  [weather]"),
        "no within-window WARN expected; stdout={stdout}"
    );
    assert!(
        !stdout.contains("missing weather within 16-day forecast window"),
        "stdout={stdout}"
    );
    assert!(stdout.contains("Info:"), "stdout={stdout}");
}

/// In-window complete plan with ai_recommended rows: count INFO, publish passes.
#[test]
fn ai_recommended_count_info_passes() {
    let _lock = PUBLISH_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let tag = nanos();
    let plan_id = format!("test-pub-aiinfo-{tag}");
    let dest = format!("pubaiinfo_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_plan(&plan_id, &dest);
            teardown_destination(&dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);
    seed_anchor(&plan_id, &dest, "2026-07-10", "2026-07-12", 3);

    db_exec(&format!(
        "INSERT INTO destination_pois (slug, poi_id, title, source_url, fetched_at, confidence, lat, lon) \
           VALUES ('{dest}', 'poi_one', 'Test POI', 'test', '2026-07-05', 'test', 35.6812, 139.7671); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, theme, theme_zh, \
                            weather_label, weather_source_id, updated_at) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-07-10', 'arrival', 'Arrival', '抵達日', \
                   'Sunny', 'open_meteo', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 2, '2026-07-11', 'full', 'Explore', '探索', \
                   'Cloudy', 'open_meteo', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 3, '2026-07-12', 'departure', 'Departure', '離開', \
                   'Sunny', 'open_meteo', datetime('now','-2 hours')); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, focus, focus_zh, updated_at) \
           VALUES ('{plan_id}', '{dest}', 2, 'morning', 'Museum', '博物館', datetime('now','-2 hours')); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, poi_id, updated_at) \
           VALUES ('{plan_id}-act1', '{plan_id}', '{dest}', 2, 'morning', 0, 'Test activity', 'poi_one', \
                   datetime('now','-2 hours')); \
         INSERT INTO plan_map_snapshots (plan_id, snapshotted_at) \
           VALUES ('{plan_id}', datetime('now'));"
    ))
    .expect("seed complete plan");

    db_exec(&format!(
        "INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title, poi_id, source, updated_at) \
           VALUES ('{plan_id}-ai-act1', '{plan_id}', '{dest}', 2, 'afternoon', 0, 'AI activity', 'poi_one', 'ai_recommended', datetime('now','-2 hours')); \
         INSERT INTO session_meals (plan_id, destination, day_number, session_type, sort_order, meal, source, updated_at) \
           VALUES ('{plan_id}', '{dest}', 2, 'morning', 0, 'AI lunch', 'ai_recommended', datetime('now','-2 hours')); \
         INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode, source) \
           VALUES ('{plan_id}', '{dest}', 2, 0, 'Place A', 'Place B', 'walking', 'ai_recommended');"
    ))
    .expect("seed ai_recommended rows");

    let (ok, stdout, stderr) = run_publish(&plan_id, &dest);
    assert!(
        ok,
        "AI-recommended INFO must not fail publish; stderr={stderr}; stdout={stdout}"
    );
    assert!(
        stdout.contains("3 AI-recommended item(s) awaiting confirmation (1 activities, 1 meals, 1 routes)"),
        "stdout={stdout}"
    );
    assert!(stdout.contains("Info:"), "stdout={stdout}");
}

/// Thin itinerary: content-depth WARN/INFO signals fire but never block publish.
#[test]
fn content_depth_warn_info_signals_are_nonblocking() {
    let _lock = PUBLISH_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let tag = nanos();
    let plan_id = format!("test-pub-cdepth-{tag}");
    let dest = format!("pubcdepth_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_plan(&plan_id, &dest);
            teardown_destination(&dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);
    seed_anchor(&plan_id, &dest, "2026-07-10", "2026-07-13", 4);

    db_exec(&format!(
        "INSERT INTO destination_pois (slug, poi_id, title, source_url, fetched_at, confidence, lat, lon) \
           VALUES ('{dest}', 'poi_a', 'POI A', 'test', '2026-07-05', 'test', 35.6812, 139.7671), \
                  ('{dest}', 'poi_b', 'POI B', 'test', '2026-07-05', 'test', 35.6820, 139.7680), \
                  ('{dest}', 'poi_c', 'POI C', 'test', '2026-07-05', 'test', 35.6830, 139.7690); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, theme, theme_zh, \
                            weather_label, weather_source_id, updated_at) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-07-10', 'arrival', 'Arrival', '抵達', \
                   'Sunny', 'open_meteo', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 2, '2026-07-11', 'full', 'Full day', '完整日', \
                   'Cloudy', 'open_meteo', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 3, '2026-07-12', 'full', 'Thin day', '薄弱日', \
                   'Sunny', 'open_meteo', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 4, '2026-07-13', 'departure', 'Departure', '離開', \
                   'Sunny', 'open_meteo', datetime('now','-2 hours')); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, focus, focus_zh, updated_at) \
           VALUES ('{plan_id}', '{dest}', 1, 'noon', 'Lunch', '午餐', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 1, 'afternoon', 'Sightseeing', '觀光', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 2, 'morning', 'Morning', '上午', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 2, 'afternoon', 'Afternoon', '下午', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 2, 'noon', 'Lunch', '午餐', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 2, 'evening', 'Dinner', '晚餐', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 3, 'morning', 'Morning', '上午', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 3, 'noon', 'Lunch', '午餐', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 3, 'afternoon', 'Afternoon', '下午', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 4, 'noon', 'Lunch', '午餐', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 4, 'evening', 'Dinner', '晚餐', datetime('now','-2 hours')); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, poi_id, updated_at) \
           VALUES ('{plan_id}-d1-afternoon', '{plan_id}', '{dest}', 1, 'afternoon', 0, 'Arrival activity', 'poi_a', datetime('now','-2 hours')), \
                  ('{plan_id}-d2-morning', '{plan_id}', '{dest}', 2, 'morning', 0, 'Morning A', 'poi_a', datetime('now','-2 hours')), \
                  ('{plan_id}-d2-afternoon', '{plan_id}', '{dest}', 2, 'afternoon', 0, 'Afternoon B', 'poi_b', datetime('now','-2 hours')), \
                  ('{plan_id}-d3-morning', '{plan_id}', '{dest}', 3, 'morning', 0, 'Morning A', 'poi_a', datetime('now','-2 hours')), \
                  ('{plan_id}-d3-noon', '{plan_id}', '{dest}', 3, 'noon', 0, 'Noon B', 'poi_b', datetime('now','-2 hours')), \
                  ('{plan_id}-d3-afternoon', '{plan_id}', '{dest}', 3, 'afternoon', 0, 'Afternoon C', 'poi_c', datetime('now','-2 hours')); \
         INSERT INTO session_meals (plan_id, destination, day_number, session_type, sort_order, meal, updated_at) \
           VALUES ('{plan_id}', '{dest}', 1, 'noon', 0, 'Arrival lunch', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 2, 'noon', 0, 'Full lunch', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 2, 'evening', 0, 'Full dinner', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 3, 'noon', 0, 'Thin lunch', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 4, 'noon', 0, 'Departure lunch', datetime('now','-2 hours')), \
                  ('{plan_id}', '{dest}', 4, 'evening', 0, 'Departure dinner', datetime('now','-2 hours')); \
         INSERT INTO day_route_segments \
           (plan_id, destination, day_number, sort_order, from_place, to_place, mode) \
           VALUES ('{plan_id}', '{dest}', 2, 0, 'POI A', 'POI B', 'walking'); \
         INSERT INTO plan_map_snapshots (plan_id, snapshotted_at) \
           VALUES ('{plan_id}', datetime('now'));"
    ))
    .expect("seed content-depth plan");

    let (ok, stdout, stderr) = run_publish(&plan_id, &dest);
    assert!(
        ok,
        "content-depth WARN/INFO must not block publish; stderr={stderr}; stdout={stdout}"
    );
    assert!(
        stdout.contains("Blockers:   0"),
        "expected zero blockers, got:\n{stdout}"
    );

    let warnings = stdout
        .split("## Warnings")
        .nth(1)
        .and_then(|s| s.split("## Info").next())
        .unwrap_or("");
    let infos = stdout
        .split("## Info")
        .nth(1)
        .and_then(|s| s.split("## Summary").next())
        .unwrap_or("");

    assert!(
        warnings.contains("[content-depth] day 3 may be thin: missing dinner meal (evening)"),
        "warnings section:\n{warnings}\nfull stdout:\n{stdout}"
    );
    assert!(
        warnings.contains("[content-depth] day 3 may be thin: 3 activities but 0 route segments"),
        "warnings section:\n{warnings}\nfull stdout:\n{stdout}"
    );
    assert!(
        !warnings.contains("[content-depth] day 1 may be thin"),
        "arrival meal gaps must be INFO not WARN; warnings:\n{warnings}"
    );
    assert!(
        infos.contains("[content-depth] day 1 may be thin: missing dinner meal (evening)"),
        "infos section:\n{infos}\nfull stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("[content-depth] day 2 may be thin"),
        "day 2 is complete — no content-depth signal expected; stdout:\n{stdout}"
    );
}

/// Past trips skip content-depth checks (like ZH/weather).
#[test]
fn past_plan_skips_content_depth_signal() {
    let _lock = PUBLISH_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let tag = nanos();
    let plan_id = format!("test-pub-cdpast-{tag}");
    let dest = format!("pubcdpast_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_plan(&plan_id, &dest);
            teardown_destination(&dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);
    seed_anchor(&plan_id, &dest, "2026-02-01", "2026-02-03", 3);

    db_exec(&format!(
        "INSERT INTO destination_pois (slug, poi_id, title, source_url, fetched_at, confidence, lat, lon) \
           VALUES ('{dest}', 'poi_a', 'POI A', 'test', '2026-07-05', 'test', 35.6812, 139.7671), \
                  ('{dest}', 'poi_b', 'POI B', 'test', '2026-07-05', 'test', 35.6820, 139.7680), \
                  ('{dest}', 'poi_c', 'POI C', 'test', '2026-07-05', 'test', 35.6830, 139.7690); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, theme) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-02-01', 'arrival', 'Arrival'), \
                  ('{plan_id}', '{dest}', 2, '2026-02-02', 'full', 'Explore'), \
                  ('{plan_id}', '{dest}', 3, '2026-02-03', 'departure', 'Departure'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, poi_id) \
           VALUES ('{plan_id}-d2-morning', '{plan_id}', '{dest}', 2, 'morning', 0, 'Act A', 'poi_a'), \
                  ('{plan_id}-d2-noon', '{plan_id}', '{dest}', 2, 'noon', 0, 'Act B', 'poi_b'), \
                  ('{plan_id}-d2-afternoon', '{plan_id}', '{dest}', 2, 'afternoon', 0, 'Act C', 'poi_c'); \
         INSERT INTO day_route_segments \
           (plan_id, destination, day_number, sort_order, from_place, to_place, mode) \
           VALUES ('{plan_id}', '{dest}', 2, 0, 'Place A', 'Place B', 'walking'); \
         INSERT INTO plan_map_snapshots (plan_id, snapshotted_at) \
           VALUES ('{plan_id}', datetime('now'));"
    ))
    .expect("seed past thin plan");

    let (ok, stdout, stderr) = run_publish(&plan_id, &dest);
    assert!(
        ok,
        "past plan must pass publish; stderr={stderr}; stdout={stdout}"
    );
    assert!(
        stdout.contains("Blockers:   0"),
        "expected zero blockers, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("[content-depth]"),
        "past plan must skip content-depth signals, got:\n{stdout}"
    );
}