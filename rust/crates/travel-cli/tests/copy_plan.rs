//! Real-Turso integration test for `copy-plan <src> <dst> [--dry-run]`.
//!
//! Seeds a throwaway `zzcp<nanos>-src` plan with two activities, a day, a route
//! segment, an operation_runs row, a plan_event_data row that references an
//! activity id, the four booking-ledger tables (incl. an activity booking key),
//! an activity_tags row, and one row in two SKIPPED tables. Then locks:
//!   - --dry-run writes nothing;
//!   - a failure injected right before COMMIT (TRAVEL_COPY_PLAN_FAIL_AT=commit)
//!     leaves NO destination rows anywhere — the copy is one transaction;
//!   - a real copy rewrites plan_id / booking keys / trip_id, gives activities and
//!     operation_runs fresh ids, remaps activity ids everywhere, skips the map and
//!     share-token tables, bumps the new plan to source version + 1 with one
//!     copy-plan operation_runs row, and leaves the source untouched;
//!   - a second copy to the same destination fails loud.
//! Skips cleanly without Turso creds.

use std::process::Command;

mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard};

fn copy_plan(args: &[&str], fail_at: Option<&str>) -> (bool, String, String) {
    let mut cmd = Command::new(bin());
    cmd.arg("copy-plan").args(args).env_remove("TRAVEL_PLAN_ID");
    match fail_at {
        Some(f) => cmd.env("TRAVEL_COPY_PLAN_FAIL_AT", f),
        None => cmd.env_remove("TRAVEL_COPY_PLAN_FAIL_AT"),
    };
    let out = cmd.output().expect("run copy-plan");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn scalar(sql: &str) -> String {
    db_exec(sql).and_then(|r| r.scalar()).unwrap_or_default()
}

fn count(sql: &str) -> i64 {
    scalar(sql).parse().unwrap_or_else(|_| panic!("not a count: {sql}"))
}

/// Rows for one plan id across the plan-keyed tables this test seeds, plus the
/// booking ledger for its trip id.
fn footprint(plan: &str) -> i64 {
    let trip = plan.replace('-', "_");
    count(&format!(
        "SELECT (SELECT COUNT(*) FROM plans WHERE plan_id='{plan}') \
              + (SELECT COUNT(*) FROM plan_metadata WHERE plan_id='{plan}') \
              + (SELECT COUNT(*) FROM days WHERE plan_id='{plan}') \
              + (SELECT COUNT(*) FROM activities WHERE plan_id='{plan}') \
              + (SELECT COUNT(*) FROM day_route_segments WHERE plan_id='{plan}') \
              + (SELECT COUNT(*) FROM operation_runs WHERE plan_id='{plan}') \
              + (SELECT COUNT(*) FROM plan_event_data WHERE plan_id='{plan}') \
              + (SELECT COUNT(*) FROM map_artifacts WHERE plan_id='{plan}') \
              + (SELECT COUNT(*) FROM plan_share_tokens WHERE plan_id='{plan}') \
              + (SELECT COUNT(*) FROM bookings_current WHERE trip_id='{trip}' OR substr(booking_key,1,length('{trip}:'))='{trip}:') \
              + (SELECT COUNT(*) FROM bookings_current_payload WHERE substr(booking_key,1,length('{trip}:'))='{trip}:') \
              + (SELECT COUNT(*) FROM bookings_events WHERE substr(booking_key,1,length('{trip}:'))='{trip}:') \
              + (SELECT COUNT(*) FROM bookings_event_data WHERE substr(booking_key,1,length('{trip}:'))='{trip}:') AS n"
    ))
}

/// Non-plan-keyed rows (booking ledger by trip prefix, activity_tags of the
/// plans' activities). Must run BEFORE teardown_plan deletes the activities.
fn teardown_non_plan_rows(plans: &[&str]) {
    let mut sql = String::new();
    for plan in plans {
        let trip = plan.replace('-', "_");
        sql.push_str(&format!(
            "DELETE FROM activity_tags WHERE activity_id IN (SELECT id FROM activities WHERE plan_id='{plan}'); \
             DELETE FROM bookings_event_data WHERE substr(booking_key,1,length('{trip}:'))='{trip}:'; \
             DELETE FROM bookings_events WHERE substr(booking_key,1,length('{trip}:'))='{trip}:'; \
             DELETE FROM bookings_current_payload WHERE substr(booking_key,1,length('{trip}:'))='{trip}:'; \
             DELETE FROM bookings_current WHERE trip_id='{trip}' OR substr(booking_key,1,length('{trip}:'))='{trip}:'; "
        ));
    }
    let _ = db_exec_teardown(&sql);
}

#[test]
fn copy_plan_copies_atomically_with_remapped_ids() {
    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping copy-plan test (no Turso creds)");
        return;
    }

    let n = nanos();
    let src = format!("zzcp{n}-src");
    let dst = format!("zzcp{n}-dst");
    let failed = format!("zzcp{n}-fail");
    let twin = format!("zzcp{n}_twin");
    let dest = format!("zzcp_dest_{n}");

    // Guard armed right after the ids are bound: non-plan rows first, then the
    // canonical plan teardown for every plan id this test can create.
    let _g = Guard::new({
        let (src, dst, failed, twin, dest) =
            (src.clone(), dst.clone(), failed.clone(), twin.clone(), dest.clone());
        move || {
            teardown_non_plan_rows(&[&src, &dst, &failed]);
            teardown_plan(&src, &dest);
            teardown_plan(&dst, &dest);
            teardown_plan(&failed, &dest);
            teardown_plan(&twin, &dest);
        }
    });

    let src_trip = src.replace('-', "_");
    let dst_trip = dst.replace('-', "_");
    let act_a = format!("zzcpact{n}a");
    let act_b = format!("zzcpact{n}b");
    let run_id = format!("zzcprun{n}");
    let tag = format!("zzcptag{n}");
    let token = format!("zzcptok{n}");
    let key_act = format!("{src_trip}:{dest}:activity:1:morning:{act_a}");
    let key_xfer = format!("{src_trip}:{dest}:transfer:arrival");

    seed_plan(&src, &dest, 5);
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) VALUES ('{src}', '{dest}', 1, '2026-12-01', 'full'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) VALUES ('{act_a}', '{src}', '{dest}', 1, 'morning', 0, 'zz Temple A'); \
         INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) VALUES ('{act_b}', '{src}', '{dest}', 1, 'afternoon', 0, 'zz Garden B'); \
         INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode) VALUES ('{src}', '{dest}', 1, 0, 'A', 'B', 'walk'); \
         INSERT INTO operation_runs (run_id, plan_id, command_type, status, version_before, version_after) VALUES ('{run_id}', '{src}', 'add-activity', 'completed', 4, 5); \
         INSERT INTO plan_event_data (plan_id, scope, destination, process_id, sort_order, key, value) VALUES ('{src}', 'destination', '{dest}', 'process_5_daily_itinerary', 0, 'activity_id', '{act_a}'); \
         INSERT INTO map_artifacts (plan_id, map_key, status, generated_at) VALUES ('{src}', 'day-1.png', 'uploaded', '2026-01-01T00:00:00Z'); \
         INSERT INTO plan_share_tokens (plan_id, token, created_at) VALUES ('{src}', '{token}', '2026-01-01 00:00:00'); \
         INSERT INTO bookings_current (booking_key, trip_id, destination, category, title, status) VALUES ('{key_act}', '{src_trip}', '{dest}', 'activity', 'zz tour', 'pending'); \
         INSERT INTO bookings_current (booking_key, trip_id, destination, category, title, status) VALUES ('{key_xfer}', '{src_trip}', '{dest}', 'transfer', 'zz transfer', 'booked'); \
         INSERT INTO bookings_current_payload (booking_key, sort_order, key, value) VALUES ('{key_act}', 0, 'activity_id', '{act_a}'); \
         INSERT INTO bookings_events (booking_key, event_type, new_status) VALUES ('{key_act}', 'created', 'pending'); \
         INSERT INTO bookings_event_data (booking_key, event_at, sort_order, key, value) VALUES ('{key_act}', '2026-01-01 00:00:00', 0, 'ref', 'zz-ref'); \
         INSERT INTO activity_tags (activity_id, tag) VALUES ('{act_a}', '{tag}');"
    ))
    .expect("seed");
    let src_footprint = footprint(&src);
    assert_eq!(src_footprint, 15, "seed footprint");

    // ── 1. --dry-run writes nothing ──────────────────────────────────
    let (ok, stdout, stderr) = copy_plan(&[&src, &dst, "--dry-run"], None);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping copy-plan test (no Turso creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "dry-run should succeed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("DRY RUN"), "dry-run banner; stdout={stdout}");
    assert!(stdout.contains("activity ids remapped: 2"), "stdout={stdout}");
    assert!(stdout.contains("booking keys rewritten: 2 (1 with an embedded activity id remapped)"), "stdout={stdout}");
    assert!(stdout.contains("map_artifacts") && stdout.contains("plan_share_tokens"), "skipped tables listed; stdout={stdout}");
    assert_eq!(footprint(&dst), 0, "dry-run must write nothing");

    // ── 2. failure right before COMMIT rolls everything back ─────────
    let (ok, stdout, stderr) = copy_plan(&[&src, &failed], Some("commit"));
    assert!(!ok, "injected failure must exit non-zero; stdout={stdout}");
    assert!(stderr.contains("rolled back"), "stderr={stderr}");
    assert_eq!(footprint(&failed), 0, "a failed copy must leave no rows behind");
    assert_eq!(
        count(&format!("SELECT COUNT(*) AS n FROM activity_tags WHERE tag='{tag}'")),
        1,
        "a failed copy must not add activity_tags"
    );

    // ── 3. the real copy ─────────────────────────────────────────────
    let (ok, stdout, stderr) = copy_plan(&[&src, &dst], None);
    assert!(ok, "copy should succeed; stdout={stdout} stderr={stderr}");
    assert!(stdout.contains(&format!("travel mark-plan-deleted {src}")), "next steps; stdout={stdout}");

    // audit: version = src + 1, one copy-plan row, the copied run got a fresh id
    assert_eq!(scalar(&format!("SELECT version AS v FROM plans WHERE plan_id='{dst}'")), "6");
    assert_eq!(scalar(&format!("SELECT version AS v FROM plans WHERE plan_id='{src}'")), "5");
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id='{dst}' AND command_type='copy-plan' \
             AND command_summary='copied from {src}' AND version_before=5 AND version_after=6"
        )),
        1
    );
    assert_eq!(count(&format!("SELECT COUNT(*) AS n FROM operation_runs WHERE plan_id='{dst}'")), 2);
    assert_eq!(count(&format!("SELECT COUNT(*) AS n FROM operation_runs WHERE run_id='{run_id}'")), 1, "source run untouched, not reused");

    // activities: fresh ids, same content
    assert_eq!(count(&format!("SELECT COUNT(*) AS n FROM activities WHERE plan_id='{dst}'")), 2);
    assert_eq!(
        count(&format!("SELECT COUNT(*) AS n FROM activities WHERE plan_id='{dst}' AND id IN ('{act_a}','{act_b}')")),
        0,
        "copied activities must get fresh ids"
    );
    let new_a = scalar(&format!("SELECT id AS v FROM activities WHERE plan_id='{dst}' AND title='zz Temple A'"));
    assert!(!new_a.is_empty() && new_a != act_a, "new id for activity A: {new_a:?}");
    assert_eq!(count(&format!("SELECT COUNT(*) AS n FROM days WHERE plan_id='{dst}'")), 1);
    assert_eq!(count(&format!("SELECT COUNT(*) AS n FROM day_route_segments WHERE plan_id='{dst}'")), 1);

    // activity-id references remapped
    assert_eq!(
        scalar(&format!("SELECT value AS v FROM plan_event_data WHERE plan_id='{dst}' AND key='activity_id'")),
        new_a
    );
    assert_eq!(
        scalar(&format!("SELECT tag AS v FROM activity_tags WHERE activity_id='{new_a}'")),
        tag
    );

    // booking ledger: prefix + trip_id + embedded activity id rewritten
    let new_key_act = format!("{dst_trip}:{dest}:activity:1:morning:{new_a}");
    let new_key_xfer = format!("{dst_trip}:{dest}:transfer:arrival");
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) AS n FROM bookings_current WHERE trip_id='{dst_trip}' \
             AND booking_key IN ('{new_key_act}', '{new_key_xfer}')"
        )),
        2
    );
    assert_eq!(
        scalar(&format!("SELECT value AS v FROM bookings_current_payload WHERE booking_key='{new_key_act}' AND key='activity_id'")),
        new_a
    );
    assert_eq!(count(&format!("SELECT COUNT(*) AS n FROM bookings_events WHERE booking_key='{new_key_act}'")), 1);
    assert_eq!(count(&format!("SELECT COUNT(*) AS n FROM bookings_event_data WHERE booking_key='{new_key_act}'")), 1);

    // skipped tables have no destination rows
    assert_eq!(count(&format!("SELECT COUNT(*) AS n FROM map_artifacts WHERE plan_id='{dst}'")), 0);
    assert_eq!(count(&format!("SELECT COUNT(*) AS n FROM plan_share_tokens WHERE plan_id='{dst}'")), 0);

    // source untouched
    assert_eq!(footprint(&src), src_footprint, "source rows must be unchanged");
    assert_eq!(
        count(&format!("SELECT COUNT(*) AS n FROM activities WHERE plan_id='{src}' AND id IN ('{act_a}','{act_b}')")),
        2
    );
    assert_eq!(
        scalar(&format!("SELECT value AS v FROM bookings_current_payload WHERE booking_key='{key_act}'")),
        act_a
    );
    assert_eq!(scalar(&format!("SELECT tag AS v FROM activity_tags WHERE activity_id='{act_a}'")), tag);

    // ── 4. a second copy to the same destination fails loud ──────────
    let (ok, stdout, stderr) = copy_plan(&[&src, &dst], None);
    assert!(!ok, "second copy must fail; stdout={stdout}");
    assert!(stderr.contains("already exists"), "stderr={stderr}");

    // ── 5. a destination whose trip id another plan already owns fails loud ──
    // (`<n>_twin` and `<n>-twin` share the booking trip id `<n>_twin`, even with
    // an empty ledger.)
    seed_plan(&twin, &dest, 1);
    let twin_dst = twin.replace('_', "-");
    let (ok, stdout, stderr) = copy_plan(&[&src, &twin_dst], None);
    assert!(!ok, "trip-id collision must fail; stdout={stdout}");
    assert!(stderr.contains("booking trip id") && stderr.contains(&twin), "stderr={stderr}");
    assert_eq!(count(&format!("SELECT COUNT(*) AS n FROM plans WHERE plan_id='{twin_dst}'")), 0);
}
