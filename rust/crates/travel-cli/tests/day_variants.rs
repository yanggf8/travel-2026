//! Real-Turso integration tests for day variants (Task A1): `days.variant_of` /
//! `variant_label(_zh)` invariants enforced by `validate publish` (BLOCK).
//!
//! Harness mirrors tests/swap_days.rs: nanos-tagged throwaway plan, Guard armed right after the
//! plan id is bound, canonical teardown_plan. Skips cleanly when Turso creds are absent.

use std::process::Command;

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

/// Serializes this file's tests: swap-days / day-variant writes run inside one remote
/// transaction, and a parallel test's write would hit SQLITE_BUSY on the shared Turso DB.
/// Taken first in each test so it is released last (after the teardown guard).
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn publish(plan_id: &str, dest: &str) -> (bool, String) {
    let out = Command::new(bin())
        .args(["validate", "publish", "--plan-id", plan_id, "--dest", dest])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run validate publish: {e}"));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn variant_lines(out: &str) -> Vec<&str> {
    out.lines().filter(|l| l.contains("[variant]")).collect()
}

#[test]
fn variant_invariants_block_publish() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }
    let tag = nanos();
    let plan_id = format!("zz-test-{tag}-var");
    let dest = format!("zz_test_{tag}_var");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || teardown_plan(&plan_id, &dest)
    });
    seed_plan(&plan_id, &dest, 0);
    let ok = db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-09-01', 'T1', 'arrival', 'draft'); \
         INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 2, '2026-09-02', 'T2', 'departure', 'draft'); \
         INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status, variant_of) \
           VALUES ('{plan_id}', '{dest}', 3, '2026-09-05', 'V', 'arrival', 'draft', 1);"
    ))
    .is_some();
    assert!(ok, "seed days failed");

    // 1. date differs from main day's -> blocked, a line mentions the variant.
    let (success, out) = publish(&plan_id, &dest);
    assert!(!success, "date mismatch must block; out={out}");
    assert!(!variant_lines(&out).is_empty(), "expected a [variant] line; out={out}");
    assert!(out.contains("date"), "expected a date message; out={out}");

    // 2. date fixed, labels blank -> still blocked (labels).
    db_exec(&format!(
        "UPDATE days SET date='2026-09-01' WHERE plan_id='{plan_id}' AND day_number=3"
    ))
    .expect("fix date");
    let (success, out) = publish(&plan_id, &dest);
    assert!(!success, "blank labels must block; out={out}");
    assert!(out.contains("no variant_label"), "expected label message; out={out}");

    // 3. labels on variant AND main -> the variant category passes.
    db_exec(&format!(
        "UPDATE days SET variant_label='Sunny plan', variant_label_zh='晴天方案' \
           WHERE plan_id='{plan_id}' AND day_number=3; \
         UPDATE days SET variant_label='Rainy plan', variant_label_zh='雨天方案' \
           WHERE plan_id='{plan_id}' AND day_number=1;"
    ))
    .expect("set labels");
    let (_, out) = publish(&plan_id, &dest);
    assert!(
        variant_lines(&out).is_empty(),
        "valid variant must raise no [variant] line; out={out}"
    );

    // 4. variant_of pointing at a variant (chain) -> blocked.
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status, \
                           variant_of, variant_label, variant_label_zh) \
           VALUES ('{plan_id}', '{dest}', 4, '2026-09-01', 'V2', 'arrival', 'draft', 3, 'Chain', '鏈');"
    ))
    .expect("seed chain");
    let (success, out) = publish(&plan_id, &dest);
    assert!(!success, "chained variant must block; out={out}");
    assert!(out.contains("itself a variant"), "expected chain message; out={out}");
}

// ---------------------------------------------------------------------------
// Task A2: add-day-variant / set-day-variant-label / remove-day-variant
// ---------------------------------------------------------------------------

/// Run the CLI with the R2 dry-delete seam on (tests must never run real wrangler).
fn cli(args: &[&str], plan_id: &str, dest: &str) -> (bool, String) {
    let out = Command::new(bin())
        .args(args)
        .args(["--plan-id", plan_id, "--dest", dest])
        .env("TRAVEL_MAPS_DRY_DELETE", "1")
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run {args:?}: {e}"));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn scalar(sql: &str) -> String {
    db_exec(sql).and_then(|r| r.scalar()).unwrap_or_default()
}

/// Two trip days (Day 1 arrival 2026-09-01, Day 2 departure 2026-09-02), no variants.
fn seed_two_days(plan_id: &str, dest: &str) {
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 1, '2026-09-01', 'T1', 'arrival', 'draft'); \
         INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 2, '2026-09-02', 'T2', 'departure', 'draft');"
    ))
    .expect("seed days");
}

macro_rules! fixture {
    ($tag:ident, $plan_id:ident, $dest:ident, $guard:ident) => {
        let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        if db_exec("SELECT 1 AS n").is_none() {
            return;
        }
        let $tag = nanos();
        let $plan_id = format!("zz-test-{}-va", $tag);
        let $dest = format!("zz_test_{}_va", $tag);
        let $guard = Guard::new({
            let (p, d) = ($plan_id.clone(), $dest.clone());
            move || teardown_plan(&p, &d)
        });
        seed_plan(&$plan_id, &$dest, 0);
        seed_two_days(&$plan_id, &$dest);
    };
}

#[test]
fn add_variant_creates_row_and_sessions() {
    fixture!(tag, plan_id, dest, _g);
    let (ok, out) = cli(
        &["add-day-variant", "1", "--label", "Sunny plan", "--label-zh", "晴天方案",
          "--main-label", "Rainy plan", "--main-label-zh", "雨天方案"],
        &plan_id, &dest,
    );
    assert!(ok, "add failed: {out}");
    assert!(
        out.contains("variant day 3 of day 1 created — write it with the usual commands on day 3"),
        "out={out}"
    );
    let row = scalar(&format!(
        "SELECT date || '|' || day_type || '|' || variant_of || '|' || variant_label || '|' || variant_label_zh \
         AS v FROM days WHERE plan_id='{plan_id}' AND day_number=3"
    ));
    assert_eq!(row, "2026-09-01|arrival|1|Sunny plan|晴天方案");
    let main = scalar(&format!(
        "SELECT variant_label || '|' || variant_label_zh AS v FROM days WHERE plan_id='{plan_id}' AND day_number=1"
    ));
    assert_eq!(main, "Rainy plan|雨天方案");
    assert_eq!(
        scalar(&format!("SELECT COUNT(*) AS n FROM timesofday WHERE plan_id='{plan_id}' AND day_number=3")),
        "4"
    );
    assert_eq!(scalar(&format!("SELECT version AS v FROM plans WHERE plan_id='{plan_id}'")), "1");
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id='{plan_id}' AND command_type='add-day-variant'"
        )),
        "1"
    );
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id='{plan_id}' AND event='day_variant_added'"
        )),
        "2"
    );
}

#[test]
fn add_variant_requires_main_label_first_time() {
    fixture!(tag, plan_id, dest, _g);
    let (ok, out) = cli(
        &["add-day-variant", "1", "--label", "Sunny plan", "--label-zh", "晴天方案"],
        &plan_id, &dest,
    );
    assert!(!ok, "must refuse without main labels: {out}");
    assert!(out.contains("--main-label"), "out={out}");
    assert_eq!(
        scalar(&format!("SELECT COUNT(*) AS n FROM days WHERE plan_id='{plan_id}'")),
        "2",
        "nothing created"
    );
}

#[test]
fn add_variant_refuses_a_variant_as_main() {
    fixture!(tag, plan_id, dest, _g);
    let (ok, out) = cli(
        &["add-day-variant", "1", "--label", "Sunny plan", "--label-zh", "晴天方案",
          "--main-label", "Rainy plan", "--main-label-zh", "雨天方案"],
        &plan_id, &dest,
    );
    assert!(ok, "{out}");
    let (ok, out) = cli(
        &["add-day-variant", "3", "--label", "Other", "--label-zh", "其他",
          "--main-label", "X", "--main-label-zh", "甲"],
        &plan_id, &dest,
    );
    assert!(!ok, "variant as main must be refused: {out}");
    assert!(out.contains("itself a variant"), "out={out}");
}

#[test]
fn label_rules() {
    fixture!(tag, plan_id, dest, _g);
    let long = "x".repeat(25);
    for bad in ["晴天", long.as_str(), "", "Rain/sun", "Yang's plan"] {
        let (ok, out) = cli(
            &["add-day-variant", "1", "--label", bad, "--label-zh", "晴天方案",
              "--main-label", "Rainy", "--main-label-zh", "雨天"],
            &plan_id, &dest,
        );
        assert!(!ok, "EN label {bad:?} must be rejected: {out}");
    }
    let (ok, out) = cli(
        &["add-day-variant", "1", "--label", "Sunny", "--label-zh", &"晴".repeat(17),
          "--main-label", "Rainy", "--main-label-zh", "雨天"],
        &plan_id, &dest,
    );
    assert!(!ok, "17-char ZH label must be rejected: {out}");
    let (ok, out) = cli(
        &["set-day-variant-label", "1", "--label", "caf\u{e9}", "--label-zh", "好"],
        &plan_id, &dest,
    );
    assert!(!ok, "non-ASCII on set-day-variant-label must be rejected: {out}");
    assert_eq!(
        scalar(&format!("SELECT COUNT(*) AS n FROM days WHERE plan_id='{plan_id}'")),
        "2"
    );
    // exact bounds pass
    let (ok, out) = cli(
        &["set-day-variant-label", "1", "--label", &"x".repeat(24), "--label-zh", &"晴".repeat(16)],
        &plan_id, &dest,
    );
    assert!(ok, "boundary labels must pass: {out}");
}

#[test]
fn remove_refuses_a_trip_day() {
    fixture!(tag, plan_id, dest, _g);
    let (ok, out) = cli(&["remove-day-variant", "2"], &plan_id, &dest);
    assert!(!ok, "{out}");
    assert!(out.contains("not a variant"), "out={out}");
    assert_eq!(
        scalar(&format!("SELECT COUNT(*) AS n FROM days WHERE plan_id='{plan_id}'")),
        "2"
    );
}

#[test]
fn remove_deletes_all_variant_rows_and_clears_main_labels() {
    fixture!(tag, plan_id, dest, _g);
    let (ok, out) = cli(
        &["add-day-variant", "1", "--label", "Sunny plan", "--label-zh", "晴天方案",
          "--main-label", "Rainy plan", "--main-label-zh", "雨天方案"],
        &plan_id, &dest,
    );
    assert!(ok, "{out}");
    let act = format!("zz-act-{tag}");
    // activity_tags has no plan_id column, so teardown_plan misses it; declared after the fixture
    // guard so it drops (and deletes) first.
    let _tags_guard = Guard::new({
        let act = act.clone();
        move || {
            db_exec(&format!("DELETE FROM activity_tags WHERE activity_id = '{act}'"));
        }
    });
    db_exec(&format!(
        "INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) \
           VALUES ('{act}', '{plan_id}', '{dest}', 3, 'morning', 0, 'A'); \
         INSERT INTO activity_tags (activity_id, tag) VALUES ('{act}', 'zz'); \
         INSERT INTO session_meals (plan_id, destination, day_number, session_type, sort_order, meal) \
           VALUES ('{plan_id}', '{dest}', 3, 'noon', 0, 'M'); \
         INSERT INTO session_activities_zh (plan_id, destination, day_number, session_type, sort_order, activity) \
           VALUES ('{plan_id}', '{dest}', 3, 'morning', 0, 'Z'); \
         INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode) \
           VALUES ('{plan_id}', '{dest}', 3, 0, 'a', 'b', 'walk'); \
         INSERT INTO day_landmarks (plan_id, destination, day_number, sort_order, landmark) \
           VALUES ('{plan_id}', '{dest}', 3, 0, 'L');"
    ))
    .expect("seed variant content");
    let (ok, out) = cli(&["remove-day-variant", "3"], &plan_id, &dest);
    assert!(ok, "remove failed: {out}");
    for tbl in [
        "days", "timesofday", "activities", "session_meals", "session_activities_zh",
        "day_route_segments", "day_landmarks",
    ] {
        assert_eq!(
            scalar(&format!("SELECT COUNT(*) AS n FROM {tbl} WHERE plan_id='{plan_id}' AND day_number=3")),
            "0",
            "{tbl} must have no variant rows"
        );
    }
    assert_eq!(scalar(&format!("SELECT COUNT(*) AS n FROM activity_tags WHERE activity_id='{act}'")), "0");
    let main = scalar(&format!(
        "SELECT COALESCE(variant_label,'NULL') || '|' || COALESCE(variant_label_zh,'NULL') AS v \
         FROM days WHERE plan_id='{plan_id}' AND day_number=1"
    ));
    assert_eq!(main, "NULL|NULL", "main labels cleared when no variants remain");
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id='{plan_id}' AND event='day_variant_removed'"
        )),
        "2"
    );
}

#[test]
fn reused_number_purges_old_maps() {
    fixture!(tag, plan_id, dest, _g);
    // add will hand out day 3; seed an uploaded map + legend stops for it.
    db_exec(&format!(
        "INSERT INTO map_artifacts (plan_id, map_key, byte_size, sha256, status, generated_at) \
           VALUES ('{plan_id}', 'day-3.png', 10, 'x', 'uploaded', '2026-01-01'); \
         INSERT INTO map_legend_stops (plan_id, map_key, seq, label, lat, lon) \
           VALUES ('{plan_id}', 'day-3.png', 0, 'S', 1.0, 2.0);"
    ))
    .expect("seed map rows");
    let (ok, out) = cli(
        &["add-day-variant", "1", "--label", "Sunny plan", "--label-zh", "晴天方案",
          "--main-label", "Rainy plan", "--main-label-zh", "雨天方案"],
        &plan_id, &dest,
    );
    assert!(ok, "add failed: {out}");
    assert!(
        out.contains(&format!("would delete trip-dashboard-maps/{plan_id}/day-3.png")),
        "out={out}"
    );
    for tbl in ["map_artifacts", "map_legend_stops"] {
        assert_eq!(
            scalar(&format!("SELECT COUNT(*) AS n FROM {tbl} WHERE plan_id='{plan_id}' AND map_key='day-3.png'")),
            "0",
            "{tbl} rows for the reused key must be gone"
        );
    }
}

// ---------------------------------------------------------------------------
// Task A3: swap-days with variants + bookings resync
// ---------------------------------------------------------------------------

fn insert_activity(id: &str, plan_id: &str, dest: &str, day: i64, session: &str, extra_cols: &str, extra_vals: &str) {
    db_exec(&format!(
        "INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title{extra_cols}) \
           VALUES ('{id}', '{plan_id}', '{dest}', {day}, '{session}', 0, 'T-{id}'{extra_vals});"
    ))
    .expect("seed activity");
}

#[test]
fn promote_variant_swaps_content_and_labels() {
    fixture!(tag, plan_id, dest, _g);
    let (ok, out) = cli(
        &["add-day-variant", "1", "--label", "Sunny plan", "--label-zh", "晴天方案",
          "--main-label", "Rainy plan", "--main-label-zh", "雨天方案"],
        &plan_id, &dest,
    );
    assert!(ok, "{out}");
    db_exec(&format!("UPDATE days SET theme='V' WHERE plan_id='{plan_id}' AND day_number=3")).expect("theme");
    let (a1, a3) = (format!("zz-a1-{tag}"), format!("zz-a3-{tag}"));
    insert_activity(&a1, &plan_id, &dest, 1, "morning", "", "");
    insert_activity(&a3, &plan_id, &dest, 3, "morning", "", "");

    let (ok, out) = cli(&["swap-days", "1", "3"], &plan_id, &dest);
    assert!(ok, "promote swap failed: {out}");

    let row = |d: i64| scalar(&format!(
        "SELECT theme || '|' || variant_label || '|' || variant_label_zh || '|' || date || '|' || COALESCE(variant_of,'NULL') AS v \
         FROM days WHERE plan_id='{plan_id}' AND day_number={d}"
    ));
    assert_eq!(row(1), "V|Sunny plan|晴天方案|2026-09-01|NULL", "main now holds variant content + labels");
    assert_eq!(row(3), "T1|Rainy plan|雨天方案|2026-09-01|1", "variant now holds old main content + labels");
    assert_eq!(scalar(&format!("SELECT day_number AS n FROM activities WHERE id='{a1}'")), "3");
    assert_eq!(scalar(&format!("SELECT day_number AS n FROM activities WHERE id='{a3}'")), "1");
}

#[test]
fn trip_day_swap_carries_variants() {
    fixture!(tag, plan_id, dest, _g);
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 3, '2026-09-03', 'T3', 'full', 'draft'); \
         INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
           VALUES ('{plan_id}', '{dest}', 4, '2026-09-04', 'T4', 'full', 'draft');"
    ))
    .expect("seed days 3-4");
    // Variant of day 4 gets day_number 5.
    let (ok, out) = cli(
        &["add-day-variant", "4", "--label", "Sunny plan", "--label-zh", "晴天方案",
          "--main-label", "Rainy plan", "--main-label-zh", "雨天方案"],
        &plan_id, &dest,
    );
    assert!(ok, "{out}");
    assert_eq!(scalar(&format!("SELECT variant_of AS n FROM days WHERE plan_id='{plan_id}' AND day_number=5")), "4");

    let (ok, out) = cli(&["swap-days", "2", "4"], &plan_id, &dest);
    assert!(ok, "trip-day swap failed: {out}");

    let v = scalar(&format!(
        "SELECT variant_of || '|' || date || '|' || day_type AS v FROM days WHERE plan_id='{plan_id}' AND day_number=5"
    ));
    assert_eq!(v, "2|2026-09-02|departure", "variant follows content to day 2 and takes its date/day_type");
    let labels = scalar(&format!(
        "SELECT COALESCE(variant_label,'NULL') || '|' || COALESCE(variant_label_zh,'NULL') AS v FROM days WHERE plan_id='{plan_id}' AND day_number=2"
    ));
    assert_eq!(labels, "Rainy plan|雨天方案", "main labels moved with the content");
    assert_eq!(
        scalar(&format!("SELECT variant_label AS v FROM days WHERE plan_id='{plan_id}' AND day_number=4")),
        "",
        "day 4 no longer has labels"
    );
    assert_eq!(
        scalar(&format!(
            "SELECT COUNT(*) AS n FROM plan_event_data WHERE plan_id='{plan_id}' AND key='variants_repointed' AND value='5->2'"
        )),
        "2" // dest_process + timeline event copies, as with every audited command
    );
}

#[test]
fn swap_refuses_foreign_variant_and_two_variants() {
    fixture!(tag, plan_id, dest, _g);
    for main in ["1", "2"] {
        let (ok, out) = cli(
            &["add-day-variant", main, "--label", "Alt", "--label-zh", "替代",
              "--main-label", "Main", "--main-label-zh", "主"],
            &plan_id, &dest,
        );
        assert!(ok, "{out}");
    }
    // Day 3 is a variant of 1, day 4 a variant of 2.
    let (ok, out) = cli(&["swap-days", "2", "3"], &plan_id, &dest);
    assert!(!ok && out.contains("variant_of"), "foreign variant must be refused: {out}");
    let (ok, out) = cli(&["swap-days", "3", "4"], &plan_id, &dest);
    assert!(!ok && out.contains("variant_of"), "two variants must be refused: {out}");
    assert_eq!(
        scalar(&format!("SELECT variant_of AS n FROM days WHERE plan_id='{plan_id}' AND day_number=3")),
        "1",
        "refused swap changed nothing"
    );
}

#[test]
fn swap_days_resyncs_bookings() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    if db_exec("SELECT 1 AS n").is_none() {
        return;
    }
    let tag = nanos();
    let plan_id = format!("zz-test-{tag}-vb");
    let dest = format!("zz_test_{tag}_vb");
    let trip_id = format!("zz_test_{tag}_vb");
    // bookings_current is keyed by trip_id, not plan_id: delete BEFORE teardown_plan.
    let _g = Guard::new({
        let (p, d, t) = (plan_id.clone(), dest.clone(), trip_id.clone());
        move || {
            common::teardown_trip_bookings(&t);
            teardown_plan(&p, &d)
        }
    });
    seed_plan(&plan_id, &dest, 0);
    seed_two_days(&plan_id, &dest);
    let act = format!("zz-bk-{tag}");
    insert_activity(&act, &plan_id, &dest, 1, "morning", ", booking_required", ", 1");

    let out = Command::new(bin())
        .args(["sync-bookings", "--plan-id", &plan_id])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run sync-bookings");
    assert!(out.status.success(), "sync-bookings failed: {}", String::from_utf8_lossy(&out.stderr));
    let keys = |day: i64| scalar(&format!(
        "SELECT COUNT(*) AS n FROM bookings_current WHERE trip_id='{trip_id}' AND booking_key LIKE '%activity:{day}:%{act}%'"
    ));
    assert_eq!(keys(1), "1", "precondition: key under day 1");

    let (ok, out) = cli(&["swap-days", "1", "2"], &plan_id, &dest);
    assert!(ok, "swap failed: {out}");
    assert!(out.contains("bookings re-synced (1)"), "{out}");
    assert_eq!(keys(2), "1", "key moved to day 2");
    assert_eq!(keys(1), "0", "no stale day-1 key");
}

// ---------------------------------------------------------------------------
// Task A4: readers (populate, content-depth, validate counts) ignore variants
// ---------------------------------------------------------------------------

#[test]
fn content_depth_ignores_variants() {
    fixture!(tag, plan_id, dest, _g);
    // Day 1 = trip day with 2 activities; Day 2 (departure) trip day with none; a variant of
    // day 1 (day 3) carries 5 more activities that must not enter the denominators.
    let (ok, out) = cli(
        &[
            "add-day-variant", "1", "--label", "Sunny plan", "--label-zh", "晴天方案",
            "--main-label", "Rainy plan", "--main-label-zh", "雨天方案",
        ],
        &plan_id,
        &dest,
    );
    assert!(ok, "add-day-variant failed: {out}");
    let variant = scalar(&format!(
        "SELECT day_number FROM days WHERE plan_id='{plan_id}' AND destination='{dest}' AND variant_of=1"
    ));
    for i in 0..2 {
        insert_activity(&format!("{tag}-m{i}"), &plan_id, &dest, 1, "morning", "", "");
    }
    for i in 0..5 {
        insert_activity(&format!("{tag}-v{i}"), &plan_id, &dest, variant.parse().unwrap(), "morning", "", "");
    }
    let out = Command::new(bin())
        .args(["compare", "content-depth", "--plan-id", &plan_id, "--against", &plan_id])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run compare content-depth");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "content-depth failed: {text}");
    // Totals are summed over trip days only: 2 activities, never 7.
    let total_line = text
        .lines()
        .find(|l| l.contains("activities") && l.contains("2"))
        .unwrap_or_else(|| panic!("no activities total line; out={text}"));
    assert!(!total_line.contains('7'), "variant activities leaked into totals: {text}");
    assert!(!text.contains(&format!("Day {variant}")), "variant row listed as a day: {text}");
}

#[test]
fn validate_publish_weather_gate_and_day_count_skip_variants() {
    fixture!(tag, plan_id, dest, _g);
    let _ = tag;
    // A variant with NULL weather inside the forecast window must not raise the weather WARN;
    // both trip days get weather so the only possible source is the variant row.
    let today = scalar("SELECT date('now') AS d");
    db_exec(&format!(
        "UPDATE days SET date='{today}', weather_label='Clear', weather_source_id='open-meteo' \
           WHERE plan_id='{plan_id}' AND destination='{dest}' AND day_number IN (1,2);"
    ))
    .expect("set weather");
    let (ok, out) = cli(
        &[
            "add-day-variant", "1", "--label", "Sunny plan", "--label-zh", "晴天方案",
            "--main-label", "Rainy plan", "--main-label-zh", "雨天方案",
        ],
        &plan_id,
        &dest,
    );
    assert!(ok, "add-day-variant failed: {out}");
    let (_, out) = publish(&plan_id, &dest);
    assert!(
        !out.contains("missing weather within 16-day forecast window"),
        "variant must not count against the weather window gate; out={out}"
    );
}

#[test]
fn variant_only_excursion_expects_no_plan_excursion() {
    fixture!(tag, plan_id, dest, _g);
    let _ = tag;
    let (ok, out) = cli(
        &[
            "add-day-variant", "1", "--label", "Far trip", "--label-zh", "遠遊",
            "--main-label", "Local plan", "--main-label-zh", "市區方案",
        ],
        &plan_id,
        &dest,
    );
    assert!(ok, "add-day-variant failed: {out}");
    // One manifest row so check-maps-fresh lists the expected keys it finds missing.
    db_exec(&format!(
        "INSERT INTO map_artifacts (plan_id, map_key, byte_size, sha256, status, has_roads, generated_at) \
           VALUES ('{plan_id}', 'plan.png', 1000, 'x', 'uploaded', 1, '2026-01-01T00:00:00Z'); \
         UPDATE days SET excursion=1 WHERE plan_id='{plan_id}' AND destination='{dest}' AND day_number=3;"
    ))
    .expect("seed manifest + variant excursion");
    let check = || {
        let out = Command::new(bin())
            .args(["check-maps-fresh", "--plan-id", &plan_id])
            .env_remove("TRAVEL_PLAN_ID")
            .output()
            .expect("run check-maps-fresh");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let text = check();
    assert!(text.contains("day-3.png"), "a variant still gets its own day slots; out={text}");
    assert!(
        !text.contains("plan-excursion.png"),
        "a variant flagged excursion=1 alone must expect no plan-excursion; out={text}"
    );
    // A trip day with the flag does expect it.
    db_exec(&format!(
        "UPDATE days SET excursion=1 WHERE plan_id='{plan_id}' AND destination='{dest}' AND day_number=2;"
    ))
    .expect("flag trip day");
    let text = check();
    assert!(text.contains("plan-excursion.png"), "a flagged trip day expects it; out={text}");
}

// ---------------------------------------------------------------------------
// Task A6: CLI views group variants under their main day.
// ---------------------------------------------------------------------------

#[test]
fn itinerary_groups_variant_under_main() {
    fixture!(tag, plan_id, dest, _g);
    let _ = tag;
    db_exec(&format!(
        "INSERT INTO date_anchors (plan_id, destination, start_date, end_date, days) \
           VALUES ('{plan_id}', '{dest}', '2026-09-01', '2026-09-02', 2);"
    ))
    .expect("seed anchor");
    let (ok, out) = cli(
        &["add-day-variant", "1", "--label", "Sunny plan", "--label-zh", "晴天方案",
          "--main-label", "Rainy plan", "--main-label-zh", "雨天方案"],
        &plan_id, &dest,
    );
    assert!(ok, "add-day-variant failed: {out}");
    db_exec(&format!(
        "UPDATE days SET theme='VarTheme' WHERE plan_id='{plan_id}' AND destination='{dest}' AND day_number=3;"
    ))
    .expect("variant theme");
    let (ok, out) = cli(&["itinerary"], &plan_id, &dest);
    assert!(ok, "itinerary failed: {out}");
    let hdr = out.lines().find(|l| l.starts_with("Day 1 (")).expect("Day 1 header line");
    assert!(hdr.contains("[雨天方案]"), "main day header shows its option label; hdr={hdr}");
    assert!(out.contains("(2 days)"), "header counts trip days only; out={out}");
    assert!(out.contains("  ↳ 替代方案 [晴天方案] (day 3)"), "variant label line; out={out}");
    assert!(!out.contains("Day 3"), "variant gets no Day header; out={out}");
    let day1 = out.find("Day 1 (").expect("Day 1 header");
    let label = out.find("↳ 替代方案").expect("label line");
    let day2 = out.find("Day 2 (").expect("Day 2 header");
    assert!(day1 < label && label < day2, "variant sits between day 1 and day 2; out={out}");
    assert!(out.contains("    Theme: VarTheme"), "variant content indented; out={out}");

    // `--day <variant>` shows only that variant's block; `--day <main>` shows main + variants.
    let (ok, out) = cli(&["itinerary", "--day", "3"], &plan_id, &dest);
    assert!(ok, "itinerary --day 3 failed: {out}");
    assert!(out.contains("  ↳ 替代方案 [晴天方案] (day 3)"), "variant block; out={out}");
    assert!(out.contains("    Theme: VarTheme"), "variant content; out={out}");
    assert!(!out.contains("Day 1 (") && !out.contains("Day 2 ("), "no trip-day blocks; out={out}");
    let (ok, out) = cli(&["itinerary", "--day", "1"], &plan_id, &dest);
    assert!(ok, "itinerary --day 1 failed: {out}");
    assert!(out.contains("Day 1 (") && out.contains("↳ 替代方案 [晴天方案]"), "main + variant; out={out}");
    let (ok, out) = cli(&["itinerary", "--day", "9"], &plan_id, &dest);
    assert!(!ok && out.contains("variants: 3 of 1"), "missing day lists variants; out={out}");
}

#[test]
fn bookings_label_variant_items() {
    fixture!(tag, plan_id, dest, _g);
    let _ = tag;
    let (ok, out) = cli(
        &["add-day-variant", "1", "--label", "Sunny plan", "--label-zh", "晴天方案",
          "--main-label", "Rainy plan", "--main-label-zh", "雨天方案"],
        &plan_id, &dest,
    );
    assert!(ok, "add-day-variant failed: {out}");
    insert_activity(
        &format!("zz{tag}a"), &plan_id, &dest, 3, "morning",
        ", booking_status, booking_ref", ", 'booked', 'REF1'",
    );
    db_exec(&format!(
        "INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan_id}', '{dest}', 1, 'morning');"
    ))
    .expect("seed main session");
    insert_activity(
        &format!("zz{tag}b"), &plan_id, &dest, 1, "morning",
        ", booking_status, booking_ref", ", 'booked', 'REF2'",
    );
    let (ok, out) = cli(&["bookings"], &plan_id, &dest);
    assert!(ok, "bookings failed: {out}");
    assert!(
        out.lines().any(|l| l.contains("Day 1: [晴天方案] T-") && l.contains("[REF1]")),
        "variant item listed under the main day with its label; out={out}"
    );
    assert!(
        out.lines().any(|l| l.contains("Day 1: T-") && l.contains("[REF2]")),
        "main item stays unprefixed; out={out}"
    );
    assert!(!out.contains("Day 3"), "internal variant number never shown; out={out}");
}
