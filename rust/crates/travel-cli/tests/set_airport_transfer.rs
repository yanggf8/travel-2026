//! Real-Turso behavior-LOCK integration test for `set-airport-transfer`.
//!
//! Locks the CURRENT (un-migrated, inline-SQL) DB write surface BEFORE the DAL
//! migration. MUST PASS against current code. It seeds a unique zztest plan +
//! plan_metadata(active_destination=dest) + the process_3_transportation status
//! row, runs `set-airport-transfer arrival booked --selected ... --candidate ...`,
//! then asserts:
//!   * the airport_transfers row (selected_* columns + status; booking_url/notes NULL)
//!   * the airport_transfer_candidates rows (DELETE-then-reinsert; sort_order = index)
//!   * plan_events (dest_process + timeline, event=airport_transfer_updated,
//!     process_id=process_3_transportation) + plan_event_data KV
//!     (direction/status/selected_id/candidates_count)
//!   * exactly one operation_runs row command_type='set-airport-transfer'
//!   * plans.version bumped by 1
//!
//! Skips cleanly if Turso creds are absent. Every assertion is scoped by the
//! unique plan/dest. Panic-safe teardown via the shared common::Guard.

use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, is_credless, nanos, seed_plan, teardown_plan, Guard};

static SET_AIRPORT_TRANSFER_LOCK: Mutex<()> = Mutex::new(());

// --- verbatim ports of the id-building helpers from set_airport_transfer.rs so
// --- we can assert the EXACT selected_id / candidate_id the command writes. ---

/// Mirror of `slugify` in src/set_airport_transfer.rs.
fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut last_underscore = true; // suppress leading underscore
    for c in text.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_alphanumeric() {
            out.push(lc);
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.len() > 48 {
        out.truncate(48);
        while out.ends_with('_') {
            out.pop();
        }
    }
    out
}

/// Mirror of `hash_string` (djb2, int32-truncated, base36, <=6 chars) in
/// src/set_airport_transfer.rs.
fn hash_string(text: &str) -> String {
    let mut hash: i32 = 5381;
    for c in text.chars() {
        let h = (hash as i64).wrapping_mul(33).wrapping_add(c as i64);
        hash = h as i32;
    }
    let abs = (hash as i64).unsigned_abs();
    if abs == 0 {
        return "0".to_string();
    }
    let mut s = String::new();
    let mut n = abs;
    while n > 0 {
        let d = (n % 36) as u32;
        let c = if d < 10 { b'0' + d as u8 } else { b'a' + (d - 10) as u8 };
        s.insert(0, c as char);
        n /= 36;
    }
    if s.len() > 6 {
        s.truncate(6);
    }
    s
}

fn transfer_id(direction: &str, title: &str, route: &str) -> String {
    format!("{}_{}_{}", direction, slugify(title), hash_string(route))
}

fn seed_transport_status(plan: &str, dest: &str) {
    db_exec(&format!(
        "INSERT OR REPLACE INTO process_statuses (plan_id, destination, process_id, status) \
           VALUES ('{plan}', '{dest}', 'process_3_transportation', 'pending');"
    ))
    .expect("creds");
}

fn run_set(
    plan: &str,
    dest: &str,
    direction: &str,
    status: &str,
    selected: &str,
    candidates: &[&str],
) -> (bool, String, String) {
    let mut args: Vec<String> = vec![
        "set-airport-transfer".to_string(),
        direction.to_string(),
        status.to_string(),
        "--selected".to_string(),
        selected.to_string(),
        "--dest".to_string(),
        dest.to_string(),
        "--plan-id".to_string(),
        plan.to_string(),
    ];
    for c in candidates {
        args.push("--candidate".to_string());
        args.push(c.to_string());
    }
    let out = Command::new(bin())
        .args(&args)
        .output()
        .expect("run set-airport-transfer");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn set_airport_transfer_writes_transfer_candidates_events_and_audit() {
    let _lock = SET_AIRPORT_TRANSFER_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping set-airport-transfer test (no Turso creds)");
        return;
    }

    let tag = nanos();
    let plan = format!("zztest{tag}");
    let dest = format!("zztest_dest_{tag}");

    teardown_plan(&plan, &dest);
    let _g = Guard::new({
        let plan = plan.clone();
        let dest = dest.clone();
        move || teardown_plan(&plan, &dest)
    });

    seed_plan(&plan, &dest, 0);
    seed_transport_status(&plan, &dest);

    // Representative arg set: arrival + booked, one --selected + two --candidate.
    let direction = "arrival";
    let status = "booked";
    let selected_title = "Limousine Bus";
    let selected_route = "NRT T1 → Shiodome";
    let selected_spec = "Limousine Bus|NRT T1 → Shiodome|85|3200|19:40 → ~21:05";

    let cand0_title = "Narita Express";
    let cand0_route = "NRT → Tokyo Station";
    let cand0_spec = "Narita Express|NRT → Tokyo Station|60|3070|19:44 → ~20:47";

    let cand1_title = "Taxi";
    let cand1_route = "NRT → Shiodome";
    let cand1_spec = "Taxi|NRT → Shiodome|70|21000|"; // empty schedule → NULL

    let selected_id = transfer_id(direction, selected_title, selected_route);
    let cand0_id = transfer_id(direction, cand0_title, cand0_route);
    let cand1_id = transfer_id(direction, cand1_title, cand1_route);

    let (ok, stdout, stderr) = run_set(
        &plan,
        &dest,
        direction,
        status,
        selected_spec,
        &[cand0_spec, cand1_spec],
    );
    if !ok && is_credless(&stderr) {
        eprintln!(
            "skipping set-airport-transfer test (no Turso creds): {}",
            stderr.trim()
        );
        return;
    }
    assert!(
        ok,
        "set-airport-transfer should succeed; stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("Airport transfer updated"),
        "stdout should confirm update; stdout={stdout}"
    );

    // --- airport_transfers row: selected_* columns + status; booking_url/notes NULL ---
    let transfer = db_exec(&format!(
        "SELECT status || '|' || selected_id || '|' || selected_title || '|' || \
                selected_route || '|' || selected_duration_min || '|' || selected_price_yen || \
                '|' || selected_schedule || '|' || \
                CASE WHEN selected_booking_url IS NULL THEN 'NULL' ELSE selected_booking_url END || \
                '|' || CASE WHEN selected_notes IS NULL THEN 'NULL' ELSE selected_notes END AS v \
         FROM airport_transfers \
         WHERE plan_id = '{plan}' AND destination = '{dest}' AND direction = '{direction}'"
    ))
    .expect("creds");
    assert_eq!(
        transfer.scalar().as_deref(),
        Some(
            format!(
                "{status}|{selected_id}|Limousine Bus|NRT T1 → Shiodome|85|3200|19:40 → ~21:05|NULL|NULL"
            )
            .as_str()
        ),
        "airport_transfers selected_* columns + status must match the --selected spec; out={transfer}"
    );

    // exactly one airport_transfers row for this plan/dest/direction.
    let transfer_count = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM airport_transfers \
         WHERE plan_id = '{plan}' AND destination = '{dest}'"
    ))
    .expect("creds");
    assert_eq!(
        transfer_count.scalar().as_deref(),
        Some("1"),
        "exactly one airport_transfers row; out={transfer_count}"
    );

    // --- airport_transfer_candidates: two rows, sort_order = index, booking_url/notes NULL ---
    let cands = db_exec(&format!(
        "SELECT sort_order || '|' || candidate_id || '|' || title || '|' || route || '|' || \
                duration_min || '|' || price_yen || '|' || \
                CASE WHEN schedule IS NULL THEN 'NULL' ELSE schedule END || '|' || \
                CASE WHEN booking_url IS NULL THEN 'NULL' ELSE booking_url END || '|' || \
                CASE WHEN notes IS NULL THEN 'NULL' ELSE notes END AS li \
         FROM airport_transfer_candidates \
         WHERE plan_id = '{plan}' AND destination = '{dest}' AND direction = '{direction}' \
         ORDER BY sort_order"
    ))
    .expect("creds");
    assert_eq!(
        cands.column(),
        vec![
            format!(
                "0|{cand0_id}|Narita Express|NRT → Tokyo Station|60|3070|19:44 → ~20:47|NULL|NULL"
            ),
            format!("1|{cand1_id}|Taxi|NRT → Shiodome|70|21000|NULL|NULL|NULL"),
        ],
        "candidates reinserted in --candidate order with sort_order = index; out={cands}"
    );

    // --- plan_events: dest_process row + timeline row, event=airport_transfer_updated ---
    let dest_event = db_exec(&format!(
        "SELECT event AS v FROM plan_events \
         WHERE plan_id = '{plan}' AND scope = 'dest_process' AND destination = '{dest}' \
           AND process_id = 'process_3_transportation'"
    ))
    .expect("creds");
    assert_eq!(
        dest_event.scalar().as_deref(),
        Some("airport_transfer_updated"),
        "dest_process plan_event must be airport_transfer_updated; out={dest_event}"
    );

    let timeline_event = db_exec(&format!(
        "SELECT event || '|' || process_id AS v FROM plan_events \
         WHERE plan_id = '{plan}' AND scope = 'timeline' AND destination = ''"
    ))
    .expect("creds");
    assert_eq!(
        timeline_event.scalar().as_deref(),
        Some("airport_transfer_updated|process_3_transportation"),
        "timeline plan_event must be airport_transfer_updated on process_3_transportation; out={timeline_event}"
    );

    // exactly two plan_events rows total for this plan (dest_process + timeline).
    let event_count = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM plan_events WHERE plan_id = '{plan}'"
    ))
    .expect("creds");
    assert_eq!(
        event_count.scalar().as_deref(),
        Some("2"),
        "exactly two plan_events rows (dest_process + timeline); out={event_count}"
    );

    // --- plan_event_data KV: {direction, status, selected_id, candidates_count} on each scope ---
    let dest_kv = db_exec(&format!(
        "SELECT key || '=' || value AS li FROM plan_event_data \
         WHERE plan_id = '{plan}' AND scope = 'dest_process' AND destination = '{dest}' \
           AND process_id = 'process_3_transportation' \
         ORDER BY key"
    ))
    .expect("creds");
    assert_eq!(
        dest_kv.column(),
        vec![
            "candidates_count=2".to_string(),
            format!("direction={direction}"),
            format!("selected_id={selected_id}"),
            format!("status={status}"),
        ],
        "dest_process plan_event_data KV must match {{direction,status,selected_id,candidates_count}}; out={dest_kv}"
    );

    let timeline_kv = db_exec(&format!(
        "SELECT key || '=' || value AS li FROM plan_event_data \
         WHERE plan_id = '{plan}' AND scope = 'timeline' AND destination = '' \
         ORDER BY key"
    ))
    .expect("creds");
    assert_eq!(
        timeline_kv.column(),
        vec![
            "candidates_count=2".to_string(),
            format!("direction={direction}"),
            format!("selected_id={selected_id}"),
            format!("status={status}"),
        ],
        "timeline plan_event_data KV must match {{direction,status,selected_id,candidates_count}}; out={timeline_kv}"
    );

    // --- operation_runs: exactly one set-airport-transfer row ---
    let op_runs = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan}' AND command_type = 'set-airport-transfer'"
    ))
    .expect("creds");
    assert_eq!(
        op_runs.scalar().as_deref(),
        Some("1"),
        "exactly one set-airport-transfer operation_run; out={op_runs}"
    );

    // --- plans.version bumped by one (seeded at 0) ---
    let version = db_exec(&format!(
        "SELECT version AS v FROM plans WHERE plan_id = '{plan}'"
    ))
    .expect("creds");
    assert_eq!(
        version.scalar().as_deref(),
        Some("1"),
        "plans.version should bump by one; out={version}"
    );
}