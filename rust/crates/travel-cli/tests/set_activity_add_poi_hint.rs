//! Behavior lock for the post-add POI hint (Part C).
//!
//! After `set-activity add`, if the new activity's title unambiguously matches a
//! GEOCODED destination_pois row (same match rule as `set-activity-poi --auto`),
//! the command prints a `💡 matches POI '<id>' — link it ...` hint so the agent
//! links it before the day's map is snapshotted. No match / ambiguous / ungeocoded
//! => no hint. The hint is read-only: it never mutates and never fails the add.
//!
//! Shared Turso via the canonical `common::` harness. Run serialized + background:
//! `cargo test -p travel-cli --test set_activity_add_poi_hint -- --test-threads=1`.

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard};

use std::process::Command;

fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn run_add(plan: &str, dest: &str, day: &str, session: &str, title: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args([
            "add-activity", day, session, title, "--plan-id", plan, "--dest", dest,
        ])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap_or_else(|e| panic!("run travel add-activity: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn teardown(plan: &str, dest: &str) {
    teardown_plan(plan, dest);
    let d = sql_lit(dest);
    let _ = db_exec_teardown(&format!(
        "DELETE FROM destination_pois WHERE slug = {d}; \
         DELETE FROM destination_config WHERE slug = {d};"
    ));
}

#[test]
fn add_activity_hints_only_on_unambiguous_geocoded_poi_match() {
    let tag = nanos();
    let plan = format!("test-addhint-{tag}");
    let dest = format!("addhint_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown(&plan, &dest)
    });

    if db_exec("SELECT 1").is_none() {
        return;
    }

    seed_plan(&plan, &dest, 0);
    let p = sql_lit(&plan);
    let d = sql_lit(&dest);
    db_exec(&format!(
        "INSERT INTO destination_config \
           (slug, display_name, ref_id, ref_path, timezone, currency, language, origin) \
         VALUES ({d}, 'ZZ Add Hint Test', 'zz', 'turso:zz', 'Asia/Tokyo', 'JPY', 'ja', 'taiwan'); \
         INSERT INTO days (plan_id, destination, day_number, date, day_type, status, updated_at) \
           VALUES ({p}, {d}, 1, '2026-09-01', 'full', 'draft', '2020-01-01 00:00:00'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type, updated_at) \
           VALUES ({p}, {d}, 1, 'morning', '2020-01-01 00:00:00'); \
         INSERT INTO destination_pois (slug, poi_id, title, lat, lon) VALUES \
           ({d}, 'osaka_castle', 'Osaka Castle', 34.6873, 135.5262), \
           ({d}, 'dotonbori', 'Dotonbori Canal', 34.6687, 135.5013), \
           ({d}, 'namba_parks', 'Namba Parks', 34.6617, 135.5017), \
           ({d}, 'namba_yasaka', 'Namba Yasaka Shrine', 34.6623, 135.4960), \
           ({d}, 'tsutenkaku', 'Tsutenkaku Tower', NULL, NULL);"
    ))
    .expect("seed add-hint case");

    // 1. Title matches exactly one GEOCODED POI (after ZH-gloss strip) -> hint.
    let (ok, stdout, stderr) = run_add(&plan, &dest, "1", "morning", "Osaka Castle 大阪城");
    if is_credless(&stderr) {
        return;
    }
    assert!(ok, "add must succeed. stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(
        stdout.contains("💡 matches POI 'osaka_castle'"),
        "expected the POI hint for an unambiguous geocoded match. stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("set-activity-poi 1 morning osaka_castle"),
        "hint must include the ready-to-run link command. stdout:\n{stdout}"
    );

    // 1b. SUBSTRING match: gloss-stripped title "Dotonbori" is CONTAINED in the
    // POI title "Dotonbori Canal" (not equal) -> still an unambiguous geocoded
    // hint. This is the case the drill exercised and the exact-match case (1)
    // does not cover.
    let (ok1b, stdout1b, _) = run_add(&plan, &dest, "1", "afternoon", "Dotonbori 道頓堀");
    assert!(ok1b, "substring add must succeed. stdout:\n{stdout1b}");
    assert!(
        stdout1b.contains("💡 matches POI 'dotonbori'"),
        "expected the POI hint for a gloss-stripped substring match. stdout:\n{stdout1b}"
    );
    assert!(
        stdout1b.contains("set-activity-poi 1 afternoon dotonbori"),
        "substring-match hint must carry the right day/session. stdout:\n{stdout1b}"
    );

    // 2. Title matches >1 geocoded POI ("Namba" -> Parks + Yasaka) -> NO hint.
    let (ok2, stdout2, _) = run_add(&plan, &dest, "1", "morning", "Namba");
    assert!(ok2, "ambiguous add must still succeed. stdout:\n{stdout2}");
    assert!(
        !stdout2.contains("💡 matches POI"),
        "an ambiguous match must NOT hint. stdout:\n{stdout2}"
    );

    // 3. Title has no POI match -> NO hint.
    let (ok3, stdout3, _) = run_add(&plan, &dest, "1", "morning", "Totally Unlisted Place");
    assert!(ok3, "no-match add must succeed. stdout:\n{stdout3}");
    assert!(
        !stdout3.contains("💡 matches POI"),
        "a no-match title must NOT hint. stdout:\n{stdout3}"
    );

    // 4. Title matches only an UNGEOCODED POI ("Tsutenkaku Tower") -> NO hint.
    let (ok4, stdout4, _) = run_add(&plan, &dest, "1", "morning", "Tsutenkaku Tower 通天閣");
    assert!(ok4, "ungeocoded-match add must succeed. stdout:\n{stdout4}");
    assert!(
        !stdout4.contains("💡 matches POI"),
        "an ungeocoded-only match must NOT hint (its map pin can't render). stdout:\n{stdout4}"
    );
}
