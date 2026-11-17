//! Real-Turso tests for the trip-record DAL `travel_db::repo::records`
//! (docs/superpowers/specs/2026-11-16-trip-record-design.md §5): unique matching
//! (ambiguous → Err, variant rows excluded, literal not LIKE), idempotent marking
//! (same value → false), and the overwrite guard `ensure_unrecorded` (row scopes
//! and the summary-aware Day/Destination scopes).
//!
//! Fixtures are per-run unique (`zz_test_<nanos>`); the RAII `Guard` is armed
//! right after the plan id is bound. Skips cleanly without Turso creds.

mod common;
use common::{db_exec, nanos, seed_plan, teardown_plan, Guard};

use travel_db::repo::records::{
    self, RecordScope, RecordStatus,
};

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `None` only when credentials are explicitly absent (skip). Any connect/auth
/// failure with credentials present PANICS: a broken connection must never read
/// as a green test. Same contract as notes_repo.rs.
async fn connect_write() -> Option<libsql::Connection> {
    let url = std::env::var("TRAVEL_TURSO_URL")
        .or_else(|_| std::env::var("TURSO_URL"))
        .ok()?;
    let token = std::env::var("TRAVEL_TURSO_WRITE_TOKEN")
        .or_else(|_| std::env::var("TURSO_TOKEN"))
        .ok()?;
    let db = libsql::Builder::new_remote(url, token)
        .build()
        .await
        .unwrap_or_else(|e| panic!("Turso creds present but connect failed: {e}"));
    Some(db.connect().unwrap_or_else(|e| panic!("Turso creds present but open failed: {e}")))
}

fn seed_day(plan: &str, dest: &str, day: i64, date: &str) {
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type) \
           VALUES ('{plan}', '{dest}', {day}, '{date}', 'full'); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', {day}, 'noon');"
    ))
    .expect("seed day");
}

fn seed_activity(plan: &str, dest: &str, day: i64, id: &str, title: &str) {
    db_exec(&format!(
        "INSERT INTO activities (id, plan_id, destination, day_number, session_type, sort_order, title) \
           VALUES ('{id}', '{plan}', '{dest}', {day}, 'noon', 0, '{title}');"
    ))
    .expect("seed activity");
}

fn seed_variant_day(plan: &str, dest: &str, variant_day: i64, main_day: i64, date: &str) {
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, day_type, variant_of) \
           VALUES ('{plan}', '{dest}', {variant_day}, '{date}', 'full', {main_day}); \
         INSERT INTO timesofday (plan_id, destination, day_number, session_type) \
           VALUES ('{plan}', '{dest}', {variant_day}, 'noon');"
    ))
    .expect("seed variant day");
}

#[tokio::test]
async fn unique_match_rejects_ambiguous() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(conn) = connect_write().await else {
        eprintln!("skipping record_repo test (no Turso creds)");
        return;
    };
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    seed_day(&plan, &dest, 1, "2026-01-01");
    let id_a = format!("{plan}_a");
    let id_b = format!("{plan}_b");
    seed_activity(&plan, &dest, 1, &id_a, "東寺");
    seed_activity(&plan, &dest, 1, &id_b, "仁和寺");

    // needle 寺 matches BOTH titles → Err listing both candidate ids.
    let err = records::resolve_activity_unique(&conn, &plan, &dest, 1, "noon", "寺")
        .await
        .unwrap_err();
    assert!(err.contains(&id_a), "err must list candidate {id_a}: {err}");
    assert!(err.contains(&id_b), "err must list candidate {id_b}: {err}");

    // needle 東寺 matches exactly one → Ok(that id).
    let got = records::resolve_activity_unique(&conn, &plan, &dest, 1, "noon", "東寺")
        .await
        .expect("unique match must resolve");
    assert_eq!(got, id_a);
}

#[tokio::test]
async fn variant_rows_not_candidates() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(conn) = connect_write().await else {
        eprintln!("skipping record_repo test (no Turso creds)");
        return;
    };
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    seed_day(&plan, &dest, 1, "2026-01-01");
    seed_variant_day(&plan, &dest, 9, 1, "2026-01-01");
    let id_main = format!("{plan}_m");
    let id_var = format!("{plan}_v");
    seed_activity(&plan, &dest, 1, &id_main, "東寺");
    seed_activity(&plan, &dest, 9, &id_var, "東寺");

    // Same title on main day and variant day: the main-day match wins — the
    // variant row is never a candidate.
    let got = records::resolve_activity_unique(&conn, &plan, &dest, 1, "noon", "東寺")
        .await
        .expect("main-day match must resolve");
    assert_eq!(got, id_main, "must resolve to the MAIN day's row, not the variant's");

    // Resolving INSIDE a variant day must refuse (variant rows cannot be recorded).
    let err = records::resolve_activity_unique(&conn, &plan, &dest, 9, "noon", "東寺")
        .await
        .unwrap_err();
    assert!(
        err.contains("variant") && err.contains('9'),
        "variant-day resolution must fail loud naming the variant day: {err}"
    );
}

#[tokio::test]
async fn match_is_literal_not_like() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(conn) = connect_write().await else {
        eprintln!("skipping record_repo test (no Turso creds)");
        return;
    };
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    seed_day(&plan, &dest, 1, "2026-01-01");
    let id_pct = format!("{plan}_p");
    let id_plain = format!("{plan}_q");
    seed_activity(&plan, &dest, 1, &id_pct, "100%_ok");
    seed_activity(&plan, &dest, 1, &id_plain, "plain");

    // `%` / `_` are LITERAL needles (Rust-side contains), never LIKE wildcards:
    // each matches only the 100%_ok row, not every row.
    for needle in ["%", "_"] {
        let got = records::resolve_activity_unique(&conn, &plan, &dest, 1, "noon", needle)
            .await
            .unwrap_or_else(|e| panic!("needle {needle:?} must match literally: {e}"));
        assert_eq!(got, id_pct, "needle {needle:?} must match only 100%_ok");
    }
}

#[tokio::test]
async fn set_record_same_value_returns_false() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(conn) = connect_write().await else {
        eprintln!("skipping record_repo test (no Turso creds)");
        return;
    };
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    seed_day(&plan, &dest, 1, "2026-01-01");
    let id = format!("{plan}_a");
    seed_activity(&plan, &dest, 1, &id, "東寺");

    let changed = records::set_activity_record(
        &conn, &plan, &dest, &id, Some(RecordStatus::Done), None,
    )
    .await
    .expect("first set must succeed");
    assert!(changed, "first set is a real change");

    let again = records::set_activity_record(
        &conn, &plan, &dest, &id, Some(RecordStatus::Done), None,
    )
    .await
    .expect("same-value rerun must succeed");
    assert!(!again, "same value + unchanged note → false (no bump)");
}

#[tokio::test]
async fn ensure_unrecorded_lists_rows() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(conn) = connect_write().await else {
        eprintln!("skipping record_repo test (no Turso creds)");
        return;
    };
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    seed_day(&plan, &dest, 1, "2026-01-01");
    let id = format!("{plan}_a");
    seed_activity(&plan, &dest, 1, &id, "東寺");
    records::set_activity_record(&conn, &plan, &dest, &id, Some(RecordStatus::Done), None)
        .await
        .expect("mark done");

    // ActivitiesOfDay now trips the guard and NAMES the row.
    let err = records::ensure_unrecorded(&conn, &plan, &dest, RecordScope::ActivitiesOfDay { day: 1 })
        .await
        .unwrap_err();
    assert!(
        err.contains("東寺") && err.contains("activities"),
        "guard must name table + title: {err}"
    );

    // MealsOfSession is a different (narrow) scope: still clean.
    records::ensure_unrecorded(&conn, &plan, &dest, RecordScope::MealsOfSession { day: 1, session: "noon" })
        .await
        .expect("no meal rows recorded → Ok");
}

#[tokio::test]
async fn day_scope_covers_summary_only() {
    let _lock = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let Some(conn) = connect_write().await else {
        eprintln!("skipping record_repo test (no Turso creds)");
        return;
    };
    let tag = nanos();
    let plan = format!("zz_test_{tag}");
    let dest = format!("zz_test_{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    seed_plan(&plan, &dest, 0);
    seed_day(&plan, &dest, 1, "2026-01-01");
    seed_day(&plan, &dest, 2, "2026-01-02");

    // ONLY a session summary, no row statuses anywhere.
    let summary = "早上先去東寺，人比預期少";
    let changed = records::set_session_summary(&conn, &plan, &dest, 1, "noon", Some(summary))
        .await
        .expect("set summary");
    assert!(changed);

    // Day { 1 } covers the summary (rows + that day's session summaries).
    let err = records::ensure_unrecorded(&conn, &plan, &dest, RecordScope::Day { day: 1 })
        .await
        .unwrap_err();
    assert!(
        err.contains(summary),
        "Day scope must list the session summary: {err}"
    );

    // Narrow row scopes do NOT look at summaries.
    records::ensure_unrecorded(&conn, &plan, &dest, RecordScope::ActivitiesOfDay { day: 1 })
        .await
        .expect("no activity rows recorded → Ok");
    records::ensure_unrecorded(&conn, &plan, &dest, RecordScope::LegsOfDay { day: 1 })
        .await
        .expect("no leg rows recorded → Ok");

    // Another day's Day scope is clean.
    records::ensure_unrecorded(&conn, &plan, &dest, RecordScope::Day { day: 2 })
        .await
        .expect("day 2 has no records → Ok");
}
