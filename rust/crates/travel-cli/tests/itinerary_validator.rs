//! Integration port of tests/integration/itinerary-validator.regression.test.ts
//!
//! The TS test drives `ItineraryValidator.validate()` with an injected "now".
//! The Rust validator (`travel validate-itinerary`) uses the real system clock
//! for "today", so we seed days whose dates are PAST vs ACTIVE/FUTURE relative
//! to the actual run date, each holding a booking-required activity with an
//! already-passed `book_by` deadline.
//!
//! Ported behaviors:
//!   1. historical itinerary days do NOT fail for past booking deadlines
//!      (validate-itinerary exits 0 / "VALID", no booking_deadline error)
//!   2. active/future itinerary days DO fail for passed booking deadlines
//!      (validate-itinerary exits 1 / "ISSUES FOUND" with a "Booking deadline
//!      PASSED" error)
//!
//! Seeds days + activities into Turso under a throwaway plan/dest, asserts on
//! validate-itinerary stdout/exit, then tears the rows down.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_travel"))
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn is_credless(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso data")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
}

/// Returns false (credless skip) if the seed fails for missing creds.
fn db_exec(sql: &str) -> Option<String> {
    let out = bin().args(["db", "exec", sql]).output().expect("run travel db exec");
    if out.status.success() {
        return Some(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if is_credless(&stderr) {
        eprintln!("skipping itinerary-validator Turso test: {}", stderr.trim());
        return None;
    }
    panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
}

/// Compute today's civil date (UTC) the same way the Rust validator does, so
/// "past" and "future" are unambiguous regardless of when the suite runs.
fn today_civil() -> (i64, u32, u32) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

fn seed_plan(plan_id: &str, dest: &str) -> Option<String> {
    db_exec(&format!(
        "INSERT INTO plans (plan_id, schema_version, version) VALUES ('{plan_id}', '4.2.0', 0); \
         INSERT INTO plan_metadata (plan_id, schema_version, active_destination) VALUES ('{plan_id}', '4.2.0', '{dest}');"
    ))
}

/// Seed one day + one booking-required activity (pending) with a passed deadline.
/// `day_type` controls arrival/full/departure; `theme` left non-arrival so the
/// validator does not waive booking checks for arrival/departure framing.
fn seed_day_with_pending_booking(
    plan_id: &str,
    dest: &str,
    day_number: i64,
    date: &str,
    book_by: &str,
) {
    let act_id = format!("teamlab-{plan_id}-{day_number}");
    db_exec(&format!(
        "INSERT INTO days (plan_id, destination, day_number, date, theme, day_type, status) \
         VALUES ('{plan_id}', '{dest}', {day_number}, '{date}', 'Museum day', 'full', 'draft'); \
         INSERT INTO activities \
           (id, plan_id, destination, day_number, session_type, sort_order, title, \
            duration_min, booking_required, booking_status, book_by, is_fixed_time, priority) \
         VALUES ('{act_id}', '{plan_id}', '{dest}', {day_number}, 'morning', 0, 'teamLab Borderless', \
            120, 1, 'pending', '{book_by}', 0, 'want');"
    ));
}

fn teardown(plan_id: &str) {
    let sql = format!(
        "DELETE FROM activities WHERE plan_id = '{plan_id}'; \
         DELETE FROM days WHERE plan_id = '{plan_id}'; \
         DELETE FROM plan_metadata WHERE plan_id = '{plan_id}'; \
         DELETE FROM plans WHERE plan_id = '{plan_id}';"
    );
    let _ = bin().args(["db", "exec", &sql]).output();
}

/// Run validate-itinerary for the throwaway plan/dest.
/// Returns (success/exit0, stdout, stderr).
fn validate(plan_id: &str, dest: &str) -> (bool, String, String) {
    let out = bin()
        .args(["validate-itinerary", "--dest", dest])
        .env("TRAVEL_PLAN_ID", plan_id)
        .output()
        .expect("run travel validate-itinerary");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// ── 1. historical day does NOT fail for a past deadline ─────────────────────
#[tokio::test]
async fn historical_day_passes_past_deadline() {
    let tag = nanos();
    let plan_id = format!("test-validator-past-{tag}");
    let _g = Guard::new({
        let plan_id = plan_id.clone();
        move || teardown(&plan_id)
    });
    let dest = format!("validatorpast_{tag}");

    if seed_plan(&plan_id, &dest).is_none() {
        return; // credless skip
    }
    // Day far in the past (year 2000) with a deadline before that.
    // The validator skips booking-deadline checks for days whose date < today.
    seed_day_with_pending_booking(&plan_id, &dest, 1, "2000-02-13", "2000-02-10");

    let (ok, stdout, stderr) = validate(&plan_id, &dest);

    assert!(ok, "historical day should validate clean (exit 0); stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("Result: VALID"), "expected VALID; got {stdout}");
    assert!(
        !stdout.contains("Booking deadline PASSED"),
        "historical day must not raise a passed-deadline error; got {stdout}"
    );
    assert!(stdout.contains("0 error(s)"), "expected zero errors; got {stdout}");
}

// ── 2. active/future day DOES fail for a passed deadline ────────────────────
#[tokio::test]
async fn future_day_fails_passed_deadline() {
    let tag = nanos();
    let plan_id = format!("test-validator-future-{tag}");
    let _g = Guard::new({
        let plan_id = plan_id.clone();
        move || teardown(&plan_id)
    });
    let dest = format!("validatorfuture_{tag}");

    if seed_plan(&plan_id, &dest).is_none() {
        return; // credless skip
    }
    // Day one year in the future relative to the actual run date, but with a
    // book_by deadline one year in the PAST → deadline already passed while the
    // itinerary day is still active/future. This is the TS "active/future day"
    // case (the TS fixture used today as the day date; a future date is an even
    // stronger form of "not historical").
    let (y, m, d) = today_civil();
    let future_date = format!("{:04}-{:02}-{:02}", y + 1, m, d);
    let past_deadline = format!("{:04}-{:02}-{:02}", y - 1, m, d);
    seed_day_with_pending_booking(&plan_id, &dest, 1, &future_date, &past_deadline);

    let (ok, stdout, stderr) = validate(&plan_id, &dest);

    assert!(!ok, "future day with passed deadline should fail (exit 1); stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("Result: ISSUES FOUND"), "expected ISSUES FOUND; got {stdout}");
    assert!(
        stdout.contains("Booking deadline PASSED"),
        "expected a passed-deadline error; got {stdout}"
    );
    assert!(stdout.contains("teamLab Borderless"), "error should name the activity; got {stdout}");
}
