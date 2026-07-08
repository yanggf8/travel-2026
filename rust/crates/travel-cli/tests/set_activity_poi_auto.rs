//! Integration tests for `travel set-activity-poi --auto`.
//!
//! These tests use shared Turso. Run serialized and in the background:
//! `cargo test -p travel-cli --test set_activity_poi_auto -- --test-threads=1`.

mod common;
use common::{
    bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard,
};

use std::process::Command;

fn run_cmd(plan_id: &str, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env("TRAVEL_PLAN_ID", plan_id)
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn seed_destination(dest: &str) {
    db_exec(&format!(
        "INSERT OR IGNORE INTO destination_config \
           (slug, display_name, ref_id, ref_path, timezone, currency, language, origin) \
         VALUES ('{dest}', 'ZZ Auto POI Test', 'zz-ref', 'turso:destination-ref/zz', \
                 'Asia/Tokyo', 'JPY', 'ja', 'taiwan');"
    ))
    .expect("seed destination_config");
}

fn teardown_destination(dest: &str) {
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_pois WHERE slug = '{dest}'; \
         DELETE FROM destination_config WHERE slug = '{dest}';"
    ));
}

fn scalar(sql: &str) -> Option<String> {
    db_exec(sql)?.scalar()
}

fn count(sql: &str) -> Option<i64> {
    scalar(sql).and_then(|s| s.parse::<i64>().ok())
}

fn seed_days(plan_id: &str, dest: &str) {
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, theme, theme_zh) \
         VALUES ('{plan_id}', '{dest}', 1, '2026-02-01', 'full', 'Explore', '探索');"
    ))
    .expect("seed day");
}

#[test]
fn auto_links_exact_and_gloss_substring_but_leaves_ambiguous_no_match_and_ungeocoded() {
    let tag = nanos();
    let plan_id = format!("test-actpoi-auto-{tag}");
    let dest = format!("actpoiauto_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_destination(&dest);
            teardown_plan(&plan_id, &dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 10);
    seed_destination(&dest);
    seed_days(&plan_id, &dest);

    db_exec(&format!(
        "INSERT INTO destination_pois (slug, poi_id, title, lat, lon) VALUES \
           ('{dest}', 'kuromon_market', 'Kuromon Ichiba Market', 34.6650, 135.5063), \
           ('{dest}', 'dotonbori', 'Dotonbori Canal', 34.6687, 135.5013), \
           ('{dest}', 'namba_parks', 'Namba Parks', 34.6617, 135.5017), \
           ('{dest}', 'namba_yasaka', 'Namba Yasaka Shrine', 34.6623, 135.4960), \
           ('{dest}', 'tsutenkaku', 'Tsutenkaku Tower', NULL, NULL); \
         INSERT INTO activities \
           (id, plan_id, destination, poi_id, day_number, session_type, sort_order, title, \
            booking_required, is_fixed_time, priority, source) \
         VALUES \
           ('{plan_id}-exact', '{plan_id}', '{dest}', NULL, 1, 'morning', 0, \
            'Kuromon Ichiba Market 黑門市場', 0, 0, 'want', 'confirmed'), \
           ('{plan_id}-substr', '{plan_id}', '{dest}', NULL, 1, 'morning', 1, \
            'Dotonbori 道頓堀', 0, 0, 'want', 'confirmed'), \
           ('{plan_id}-ambig', '{plan_id}', '{dest}', NULL, 1, 'afternoon', 0, \
            'Namba', 0, 0, 'want', 'confirmed'), \
           ('{plan_id}-nomatch', '{plan_id}', '{dest}', NULL, 1, 'afternoon', 1, \
            'Shinsaibashi-suji Shopping 心齋橋筋商店街', 0, 0, 'want', 'confirmed'), \
           ('{plan_id}-ungeocoded', '{plan_id}', '{dest}', NULL, 1, 'evening', 0, \
            'Tsutenkaku Tower 通天閣', 0, 0, 'want', 'confirmed');"
    ))
    .expect("seed auto case");

    let (ok, stdout, stderr) = run_cmd(&plan_id, &["set-activity-poi", "--auto", "--dest", &dest]);
    if is_credless(&stderr) {
        return;
    }

    assert!(ok, "--auto should succeed. stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(stdout.contains("linked 2"), "expected two links. stdout:\n{stdout}");
    assert!(stdout.contains("Kuromon Ichiba Market 黑門市場"), "exact+gloss row missing. stdout:\n{stdout}");
    assert!(stdout.contains("Dotonbori 道頓堀"), "substring+gloss row missing. stdout:\n{stdout}");
    assert!(stdout.contains("unlinked 3"), "expected three manual rows. stdout:\n{stdout}");
    assert!(stdout.contains("ambiguous POI match"), "ambiguous reason missing. stdout:\n{stdout}");
    assert!(stdout.contains("no POI match"), "no-match reason missing. stdout:\n{stdout}");
    assert!(
        stdout.contains("matched but POI ungeocoded"),
        "ungeocoded reason missing. stdout:\n{stdout}"
    );

    assert_eq!(
        scalar(&format!(
            "SELECT COALESCE(poi_id, 'NULL') AS poi_id FROM activities \
             WHERE plan_id = '{plan_id}' AND id = '{plan_id}-exact'"
        )),
        Some("kuromon_market".to_string())
    );
    assert_eq!(
        scalar(&format!(
            "SELECT COALESCE(poi_id, 'NULL') AS poi_id FROM activities \
             WHERE plan_id = '{plan_id}' AND id = '{plan_id}-substr'"
        )),
        Some("dotonbori".to_string())
    );
    for id in ["ambig", "nomatch", "ungeocoded"] {
        assert_eq!(
            scalar(&format!(
                "SELECT COALESCE(poi_id, 'NULL') AS poi_id FROM activities \
                 WHERE plan_id = '{plan_id}' AND id = '{plan_id}-{id}'"
            )),
            Some("NULL".to_string()),
            "{id} must remain unlinked"
        );
    }

    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs \
             WHERE plan_id = '{plan_id}' AND command_type = 'set-activity-poi' \
               AND command_summary = 'auto-linked 2 POI(s)' AND status = 'completed'"
        )),
        Some(1),
        "auto mode must write exactly one completed operation_runs row"
    );
    assert_eq!(
        scalar(&format!("SELECT version FROM plans WHERE plan_id = '{plan_id}'")),
        Some("11".to_string()),
        "auto mode must bump version exactly once"
    );
}

#[test]
fn auto_second_run_is_noop_when_no_null_poi_activities_remain() {
    let tag = nanos();
    let plan_id = format!("test-actpoi-auto-idem-{tag}");
    let dest = format!("actpoiautoidem_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_destination(&dest);
            teardown_plan(&plan_id, &dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 7);
    seed_destination(&dest);
    seed_days(&plan_id, &dest);

    db_exec(&format!(
        "INSERT INTO destination_pois (slug, poi_id, title, lat, lon) VALUES \
           ('{dest}', 'osaka_castle', 'Osaka Castle', 34.6873, 135.5262), \
           ('{dest}', 'umeda_sky', 'Umeda Sky Building', 34.7054, 135.4900); \
         INSERT INTO activities \
           (id, plan_id, destination, poi_id, day_number, session_type, sort_order, title, \
            booking_required, is_fixed_time, priority, source) \
         VALUES \
           ('{plan_id}-castle', '{plan_id}', '{dest}', NULL, 1, 'morning', 0, \
            'Osaka Castle 大阪城', 0, 0, 'want', 'confirmed'), \
           ('{plan_id}-umeda', '{plan_id}', '{dest}', NULL, 1, 'afternoon', 0, \
            'Umeda Sky Building 梅田スカイビル', 0, 0, 'want', 'confirmed');"
    ))
    .expect("seed idempotence case");

    let (ok1, stdout1, stderr1) = run_cmd(&plan_id, &["set-activity-poi", "--auto", "--dest", &dest]);
    if is_credless(&stderr1) {
        return;
    }
    assert!(ok1, "first --auto should link both rows. stdout:\n{stdout1}\nstderr:\n{stderr1}");
    assert!(stdout1.contains("linked 2"), "first run should link 2. stdout:\n{stdout1}");

    let (ok2, stdout2, stderr2) = run_cmd(&plan_id, &["set-activity-poi", "--auto", "--dest", &dest]);
    assert!(ok2, "second --auto should be a noop. stdout:\n{stdout2}\nstderr:\n{stderr2}");
    assert!(
        stdout2.contains("nothing to link"),
        "second run should report nothing to link. stdout:\n{stdout2}"
    );
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs \
             WHERE plan_id = '{plan_id}' AND command_type = 'set-activity-poi' \
               AND command_summary = 'auto-linked 2 POI(s)' AND status = 'completed'"
        )),
        Some(1),
        "second noop must not write another operation_runs row"
    );
    assert_eq!(
        scalar(&format!("SELECT version FROM plans WHERE plan_id = '{plan_id}'")),
        Some("8".to_string()),
        "second noop must not bump version"
    );
}

#[test]
fn auto_rejects_positionals() {
    let tag = nanos();
    let plan_id = format!("test-actpoi-auto-args-{tag}");
    let dest = format!("actpoiautoargs_{tag}");
    let _g = Guard::new({
        let (plan_id, dest) = (plan_id.clone(), dest.clone());
        move || {
            teardown_destination(&dest);
            teardown_plan(&plan_id, &dest);
        }
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan_id, &dest, 0);
    seed_destination(&dest);

    let (ok, stdout, stderr) = run_cmd(
        &plan_id,
        &["set-activity-poi", "--auto", "1", "morning", "foo", "--dest", &dest],
    );
    if is_credless(&stderr) {
        return;
    }

    assert!(!ok, "--auto with positionals must exit non-zero. stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(
        stderr.contains("--auto cannot be combined with <day> <session> <poi_id>"),
        "expected fail-loud usage error. stderr:\n{stderr}"
    );
}