//! Integration tests for `update-route-segment`, `delete-route-segment`, via carry on the
//! rewrite commands and `road-leg refetch --segment` argument errors
//! (spec 2026-11-20-cli-itinerary-editing-design.md §4, §6).
//!
//! Real Turso, `zz_test_<nanos>_*` plan/destination, Guard armed right after the plan id is
//! bound, canonical `teardown_plan`. Never touches a real plan, never hits the network.
//! Skips cleanly when creds are absent.

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

use std::process::Command;

struct Fx {
    plan: String,
    dest: String,
}

fn setup(tag: &str) -> Option<(Fx, Guard<impl FnMut()>)> {
    let n = nanos();
    let plan = format!("zz-test-{n}-{tag}");
    let dest = format!("zz_test_{n}_{tag}");
    let g = Guard::new({
        let (p, d) = (plan.clone(), dest.clone());
        move || teardown_plan(&p, &d)
    });
    db_exec("SELECT 1")?;
    seed_plan(&plan, &dest, 0);
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
         VALUES ('{plan}', '{dest}', 1, '2026-06-12', 'full');"
    ))?;
    Some((Fx { plan, dest }, g))
}

fn run(fx: &Fx, args: &[&str]) -> (bool, String, String) {
    let mut a: Vec<&str> = args.to_vec();
    a.extend(["--plan-id", &fx.plan, "--dest", &fx.dest]);
    let out = Command::new(bin()).args(&a).output().expect("run travel");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn col(sql: &str) -> Vec<String> {
    db_exec(sql).expect("db").column()
}

fn seg(fx: &Fx, so: i64, from: &str, to: &str, source: &str, dur: i64) {
    db_exec(&format!(
        "INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode, duration_min, start_time, notes, source) \
         VALUES ('{}', '{}', 1, {so}, '{from}', '{to}', 'driving', {dur}, '09:00', 'n{so}', '{source}');",
        fx.plan, fx.dest
    ))
    .unwrap();
}

fn via(fx: &Fx, so: i64, order: i64, lat: f64, lon: f64) {
    db_exec(&format!(
        "INSERT INTO day_route_segment_via (plan_id, destination, day_number, sort_order, via_order, lat, lon) \
         VALUES ('{}', '{}', 1, {so}, {order}, {lat}, {lon});",
        fx.plan, fx.dest
    ))
    .unwrap();
}

fn rows(fx: &Fx) -> Vec<String> {
    col(&format!(
        "SELECT sort_order || '|' || from_place || '|' || to_place || '|' || COALESCE(duration_min,'-') || '|' || COALESCE(start_time,'-') || '|' || source \
         FROM day_route_segments WHERE plan_id='{}' ORDER BY sort_order",
        fx.plan
    ))
}

fn vias(fx: &Fx) -> Vec<String> {
    col(&format!(
        "SELECT sort_order || ':' || via_order || ':' || lat || ',' || lon AS v FROM day_route_segment_via \
         WHERE plan_id='{}' ORDER BY sort_order, via_order",
        fx.plan
    ))
}

fn version(fx: &Fx) -> String {
    col(&format!("SELECT version FROM plans WHERE plan_id='{}'", fx.plan))[0].clone()
}

fn kv(fx: &Fx, event: &str, key: &str) -> Vec<String> {
    col(&format!(
        "SELECT d.value FROM plan_events e JOIN plan_event_data d \
           ON d.plan_id=e.plan_id AND d.scope=e.scope AND d.destination=e.destination \
          AND d.process_id=e.process_id AND d.sort_order=e.sort_order \
         WHERE e.plan_id='{}' AND e.scope='timeline' AND e.event='{event}' AND d.key='{key}'",
        fx.plan
    ))
}

fn audit_count(fx: &Fx, cmd: &str) -> usize {
    col(&format!(
        "SELECT COUNT(*) FROM operation_runs WHERE plan_id='{}' AND command_type='{cmd}' AND status='completed'",
        fx.plan
    ))[0]
        .parse()
        .unwrap()
}

fn via_table_rows(plan: &str) -> String {
    col(&format!("SELECT COUNT(*) FROM day_route_segment_via WHERE plan_id='{plan}'"))[0].clone()
}

#[test]
fn update_only_duration_keeps_source_and_other_fields() {
    let Some((fx, _g)) = setup("dur") else { return };
    seg(&fx, 0, "A", "B", "ai_recommended", 20);
    let (ok, out, err) = run(&fx, &["update-route-segment", "1", "0", "--duration", "30"]);
    assert!(ok, "{out}{err}");
    // bite: set-route-segment would have flipped source to confirmed; update must not.
    assert_eq!(rows(&fx), vec!["0|A|B|30|09:00|ai_recommended"]);
    assert!(out.contains("confirm-recommendations --day 1 --kind route"), "ai_recommended edit must warn: {out}");
    assert_eq!(kv(&fx, "route_segment_patched", "changed"), vec!["duration_min"]);
    assert_eq!(kv(&fx, "route_segment_patched", "from.duration_min"), vec!["20"]);
    assert_eq!(audit_count(&fx, "update-route-segment"), 1);
}

#[test]
fn update_same_value_is_no_change_zero_audit() {
    let Some((fx, _g)) = setup("noop") else { return };
    seg(&fx, 0, "A", "B", "confirmed", 20);
    let v0 = version(&fx);
    let (ok, out, _) = run(&fx, &["update-route-segment", "1", "0", "--duration", "20"]);
    assert!(ok && out.contains("No change"), "{out}");
    assert_eq!(version(&fx), v0, "version must not move");
    assert_eq!(audit_count(&fx, "update-route-segment"), 0);
}

#[test]
fn update_clear_start_time_and_append_notes() {
    let Some((fx, _g)) = setup("clr") else { return };
    seg(&fx, 0, "A", "B", "confirmed", 20);
    let (ok, out, err) = run(&fx, &["update-route-segment", "1", "0", "--clear", "start_time", "--append-notes", "停車"]);
    assert!(ok, "{out}{err}");
    assert_eq!(rows(&fx), vec!["0|A|B|20|-|confirmed"]);
    let notes = col(&format!("SELECT notes FROM day_route_segments WHERE plan_id='{}'", fx.plan));
    assert_eq!(notes, vec!["n0；停車"]);
    assert_eq!(kv(&fx, "route_segment_patched", "notes_mode"), vec!["append"]);
    // Re-run: append is idempotent.
    let v = version(&fx);
    let (ok, out, _) = run(&fx, &["update-route-segment", "1", "0", "--append-notes", "停車"]);
    assert!(ok && out.contains("No change"), "{out}");
    assert_eq!(version(&fx), v);
}

#[test]
fn update_recorded_segment_allows_duration_but_not_endpoints() {
    let Some((fx, _g)) = setup("rec") else { return };
    seg(&fx, 0, "A", "B", "confirmed", 20);
    db_exec(&format!("UPDATE day_route_segments SET record_status='done' WHERE plan_id='{}'", fx.plan));
    let (ok, out, err) = run(&fx, &["update-route-segment", "1", "0", "--duration", "25"]);
    assert!(ok, "{out}{err}");
    let v = version(&fx);
    let (ok, _, err) = run(&fx, &["update-route-segment", "1", "0", "--to", "C"]);
    assert!(!ok && err.contains("trip record"), "{err}");
    assert_eq!(version(&fx), v, "rejected edit writes nothing");
    assert_eq!(rows(&fx), vec!["0|A|B|25|09:00|confirmed"]);
}

#[test]
fn update_merged_guard_rejects_walking_over_rail() {
    let Some((fx, _g)) = setup("grd") else { return };
    // 赤嶺駅 -> 單軌沿線某站 as transit is fine; flipping to walking must be refused.
    let (a, b) = ("\u{8D64}\u{5DBA}\u{99C5}", "\u{55AE}\u{8ECC}\u{6CBF}\u{7DDA}\u{67D0}\u{7AD9}");
    db_exec(&format!(
        "INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode, source) \
         VALUES ('{}', '{}', 1, 0, '{a}', '{b}', 'transit', 'confirmed');",
        fx.plan, fx.dest
    ));
    let v = version(&fx);
    let (ok, _, err) = run(&fx, &["update-route-segment", "1", "0", "--mode", "walking"]);
    assert!(!ok && err.contains("walking-over-rail"), "{err}");
    assert_eq!(version(&fx), v);
    assert_eq!(col(&format!("SELECT mode FROM day_route_segments WHERE plan_id='{}'", fx.plan)), vec!["transit"]);
}

#[test]
fn update_missing_slot_lists_existing_sort_orders() {
    let Some((fx, _g)) = setup("miss") else { return };
    seg(&fx, 0, "A", "B", "confirmed", 20);
    seg(&fx, 1, "B", "C", "confirmed", 20);
    let (ok, _, err) = run(&fx, &["update-route-segment", "1", "7", "--duration", "5"]);
    assert!(!ok && err.contains("existing sort_orders: 0, 1"), "{err}");
}

#[test]
fn update_via_set_clear_and_endpoint_change_clears() {
    let Some((fx, _g)) = setup("via") else { return };
    seg(&fx, 0, "A", "B", "confirmed", 20);
    let (ok, out, err) = run(&fx, &["update-route-segment", "1", "0", "--via", "25.2897,121.569;25.2317,121.5018"]);
    assert!(ok, "{out}{err}");
    assert_eq!(vias(&fx), vec!["0:0:25.2897,121.569", "0:1:25.2317,121.5018"]);
    assert_eq!(kv(&fx, "route_segment_patched", "via_count"), vec!["2"]);
    assert!(
        out.contains(&format!("travel road-leg refetch --segment 1 0 --plan-id {} --dest {}", fx.plan, fx.dest)),
        "must print the exact refetch command: {out}"
    );
    // A duration-only edit keeps via.
    let (ok, _, _) = run(&fx, &["update-route-segment", "1", "0", "--duration", "40"]);
    assert!(ok);
    assert_eq!(vias(&fx).len(), 2);
    // Changing --to without --via clears it and says so.
    let (ok, out, err) = run(&fx, &["update-route-segment", "1", "0", "--to", "C"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("via cleared"), "{out}");
    assert!(vias(&fx).is_empty());
    // --clear via on a segment that has none is a no-op.
    let (ok, out, _) = run(&fx, &["update-route-segment", "1", "0", "--clear", "via"]);
    assert!(ok && out.contains("No change"), "{out}");
    // Bad via: nothing written.
    let v = version(&fx);
    let (ok, _, err) = run(&fx, &["update-route-segment", "1", "0", "--via", "0,0"]);
    assert!(!ok && err.contains("null island"), "{err}");
    assert_eq!(version(&fx), v);
    assert_eq!(via_table_rows_after(&fx), "0");
}

fn via_table_rows_after(fx: &Fx) -> String {
    via_table_rows(&fx.plan)
}

#[test]
fn delete_second_of_five_renumbers_segments_and_via_atomically() {
    let Some((fx, _g)) = setup("del") else { return };
    for (i, (f, t)) in [("A", "B"), ("B", "C"), ("C", "D"), ("D", "E"), ("E", "F")].iter().enumerate() {
        seg(&fx, i as i64, f, t, "confirmed", 10 + i as i64);
    }
    via(&fx, 1, 0, 25.1, 121.1); // belongs to the deleted segment
    via(&fx, 2, 0, 25.2, 121.2);
    via(&fx, 4, 0, 25.4, 121.4);
    via(&fx, 4, 1, 25.5, 121.5);
    let (ok, out, err) = run(&fx, &["delete-route-segment", "1", "1"]);
    assert!(ok, "{out}{err}");
    assert_eq!(
        rows(&fx),
        vec!["0|A|B|10|09:00|confirmed", "1|C|D|12|09:00|confirmed", "2|D|E|13|09:00|confirmed", "3|E|F|14|09:00|confirmed"]
    );
    // via of the deleted slot gone; the later ones followed their segments (2->1, 4->3).
    assert_eq!(vias(&fx), vec!["1:0:25.2,121.2", "3:0:25.4,121.4", "3:1:25.5,121.5"]);
    assert_eq!(kv(&fx, "route_segment_deleted", "renumbered_count"), vec!["3"]);
    assert_eq!(kv(&fx, "route_segment_deleted", "from_place"), vec!["B"]);
    assert_eq!(kv(&fx, "route_segment_deleted", "notes"), vec!["n1"]);
    assert_eq!(audit_count(&fx, "delete-route-segment"), 1);
}

#[test]
fn delete_recorded_segment_is_refused_with_zero_writes() {
    let Some((fx, _g)) = setup("delrec") else { return };
    seg(&fx, 0, "A", "B", "confirmed", 20);
    seg(&fx, 1, "B", "C", "confirmed", 20);
    db_exec(&format!("UPDATE day_route_segments SET record_status='done' WHERE plan_id='{}' AND sort_order=0", fx.plan));
    let v = version(&fx);
    let (ok, _, _) = run(&fx, &["delete-route-segment", "1", "0"]);
    assert!(!ok);
    assert_eq!(version(&fx), v);
    assert_eq!(rows(&fx).len(), 2);
    assert_eq!(audit_count(&fx, "delete-route-segment"), 0);
}

#[test]
fn delete_ai_recommended_prints_derive_hint() {
    let Some((fx, _g)) = setup("delai") else { return };
    seg(&fx, 0, "A", "B", "ai_recommended", 20);
    let (ok, out, err) = run(&fx, &["delete-route-segment", "1", "0"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("derive-routes"), "{out}");
    assert!(rows(&fx).is_empty());
}

#[test]
fn set_route_segment_keeps_via_for_same_endpoints_and_clears_changed() {
    let Some((fx, _g)) = setup("setrs") else { return };
    seg(&fx, 0, "A", "B", "confirmed", 20);
    via(&fx, 0, 0, 25.1, 121.1);
    let (ok, out, err) = run(&fx, &["set-route-segment", "1", "0", "A", "B", "driving", "--duration", "33"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("via kept: D1 S0"), "{out}");
    assert_eq!(vias(&fx), vec!["0:0:25.1,121.1"]);
    let (ok, out, err) = run(&fx, &["set-route-segment", "1", "0", "A", "C", "driving"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("via cleared: D1 S0"), "{out}");
    assert!(vias(&fx).is_empty());
}

#[test]
fn bulk_rewrite_keeps_one_via_clears_the_other_and_lists_both() {
    let Some((fx, _g)) = setup("bulk") else { return };
    seg(&fx, 0, "A", "B", "confirmed", 20);
    seg(&fx, 1, "B", "C", "confirmed", 20);
    via(&fx, 0, 0, 25.1, 121.1);
    via(&fx, 1, 0, 25.2, 121.2);
    // New order: B->C moves to slot 0 (kept, renumbered); A->B replaced by A->X (cleared).
    let (ok, out, err) = run(
        &fx,
        &["set-route-segments-bulk", "1", "--seg", "B|C|driving|20", "--seg", "A|X|driving|15"],
    );
    assert!(ok, "{out}{err}");
    assert!(out.contains("via kept: D1 S0 B→C"), "{out}");
    assert!(out.contains("via cleared: D1 S0 舊 A→B"), "{out}");
    assert_eq!(vias(&fx), vec!["0:0:25.2,121.2"]);
}

#[test]
fn teardown_leaves_no_via_rows() {
    let plan;
    {
        let Some((fx, _g)) = setup("leak") else { return };
        seg(&fx, 0, "A", "B", "confirmed", 20);
        via(&fx, 0, 0, 25.1, 121.1);
        plan = fx.plan.clone();
        assert_eq!(via_table_rows(&plan), "1");
    }
    // Guard dropped → teardown_plan ran: the new plan-keyed table is covered.
    assert_eq!(via_table_rows(&plan), "0");
}

#[test]
fn road_leg_refetch_segment_missing_slot_errors_before_any_network() {
    let Some((fx, _g)) = setup("rl") else { return };
    let (ok, _, err) = run(&fx, &["road-leg", "refetch", "--segment", "1", "5"]);
    assert!(!ok && err.contains("no route segment at day 1 sort_order 5"), "{err}");
}

fn seg_d(fx: &Fx, day: i64, so: i64, from: &str, to: &str) {
    db_exec(&format!(
        "INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode, duration_min, source) \
         VALUES ('{}', '{}', {day}, {so}, '{from}', '{to}', 'driving', 10, 'confirmed');",
        fx.plan, fx.dest
    ))
    .unwrap();
}

fn day_row(fx: &Fx, day: i64, variant_of: Option<i64>) {
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, variant_of) \
         VALUES ('{}', '{}', {day}, '2026-06-{:02}', 'full', {});",
        fx.plan,
        fx.dest,
        12 + day,
        variant_of.map(|v| v.to_string()).unwrap_or_else(|| "NULL".into())
    ))
    .unwrap();
}

#[test]
fn join_hint_follows_order_not_numbering_and_crosses_days_both_ways() {
    let Some((fx, _g)) = setup("joinhint") else { return };
    // jiufen-style: no S0 anywhere; D1 only S1, D2 S1..S3.
    seg_d(&fx, 1, 1, "A", "旅館");
    seg_d(&fx, 2, 1, "旅館", "C");
    seg_d(&fx, 2, 2, "C", "D");
    seg_d(&fx, 2, 3, "D", "E");
    let (ok, out, err) = run(&fx, &["update-route-segment", "1", "1", "--to", "新旅館"]);
    assert!(ok, "{out}{err}");
    assert!(
        out.contains("hint: D2 S1 starts at \"旅館\"")
            && out.contains(&format!(
                "travel update-route-segment 2 1 --from \"新旅館\" --plan-id {} --dest {}",
                fx.plan, fx.dest
            )),
        "{out}"
    );
    // Hint only: nothing else moved.
    assert_eq!(col(&format!("SELECT from_place FROM day_route_segments WHERE plan_id='{}' AND day_number=2 AND sort_order=1", fx.plan)), vec!["旅館"]);
    // Reverse direction across the day boundary: first re-join D2 S1 to the new
    // hotel (as the hint says), then break it — prev = D1's LAST segment.
    let (ok, out, err) = run(&fx, &["update-route-segment", "2", "1", "--from", "新旅館"]);
    assert!(ok && !out.contains("hint:"), "re-joining must not hint: {out}{err}");
    let (ok, out, err) = run(&fx, &["update-route-segment", "2", "1", "--from", "別處"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("hint: D1 S1 ends at \"新旅館\"") && out.contains("update-route-segment 1 1 --to \"別處\""), "{out}");
    // Duration-only edit: endpoints unchanged => no hint.
    let (ok, out, _) = run(&fx, &["update-route-segment", "2", "2", "--duration", "33"]);
    assert!(ok && !out.contains("hint:"), "{out}");
}

#[test]
fn join_hint_skips_variant_days_and_flight_gaps() {
    let Some((fx, _g)) = setup("joinvar") else { return };
    day_row(&fx, 2, Some(1)); // D2 is a variant (alternative) — never a neighbour
    day_row(&fx, 3, None);
    seg_d(&fx, 1, 0, "A", "B");
    seg_d(&fx, 2, 0, "B", "VARIANT");
    seg_d(&fx, 3, 0, "B", "D");
    let (ok, out, err) = run(&fx, &["update-route-segment", "1", "0", "--to", "H"]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("hint: D3 S0 starts at \"B\""), "next must skip the variant day: {out}");
    assert!(!out.contains("D2 S0"), "{out}");
    // Flight gap: S1 starts at an airport that S0 never ended at => never joined => silent.
    seg_d(&fx, 3, 1, "桃園機場", "家");
    let (ok, out, _) = run(&fx, &["update-route-segment", "3", "0", "--to", "KIX2"]);
    assert!(ok && !out.contains("hint:"), "{out}");
}

#[test]
fn update_to_hints_next_day_first_segment_without_writing() {
    let Some((fx, _g)) = setup("joinhint") else { return };
    seg(&fx, 0, "A", "頭城（住宿待定）", "confirmed", 20);
    // Day 2's first segment starts at an undecided-stay placeholder.
    db_exec(&format!(
        "INSERT INTO day_route_segments (plan_id, destination, day_number, sort_order, from_place, to_place, mode, duration_min, source) \
         VALUES ('{}', '{}', 2, 0, '頭城（住宿待定）', 'C', 'driving', 10, 'confirmed');",
        fx.plan, fx.dest
    ))
    .unwrap();
    let (ok, out, err) = run(&fx, &["update-route-segment", "1", "0", "--to", "旅館"]);
    assert!(ok, "{out}{err}");
    assert!(
        out.contains("hint: D2 S0 starts at \"頭城（住宿待定）\"")
            && out.contains("travel update-route-segment 2 0 --from \"旅館\" --plan-id"),
        "{out}"
    );
    // Hint only: day 2 untouched.
    let d2 = col(&format!(
        "SELECT from_place FROM day_route_segments WHERE plan_id='{}' AND day_number=2",
        fx.plan
    ));
    assert_eq!(d2, vec!["頭城（住宿待定）"]);
    // A joined chain stays silent.
    let (ok, out, _) = run(&fx, &["update-route-segment", "2", "0", "--from", "旅館"]);
    // D2 S0 now starts at 旅館 == D1 S0 end: nothing joined is broken by THIS edit.
    assert!(ok && !out.contains("hint: D1"), "{out}");
}
