//! Output-visibility fixes from CLI self-review, behavior-locked:
//!   - `mark-booked` prints the resolved plan_id (a dest can be shared by
//!     several plans; the internally-resolved plan was invisible before).
//!   - `sync-bookings` prints the effective trip_id — the DELETE-and-rewrite
//!     scope of the sync — not just the plan.
//!   - `share-token --show` prints plan ownership on every token line.
//!   - `share-token --all` alone does NOT mint a token (it used to fall
//!     through to the default mint action and create one).
//!
//! Real-Turso integration test; unique zztest fixtures; skips cleanly without
//! creds. Serialized per file via a static mutex.

mod common;
use common::{bin, db_exec, nanos, seed_plan, teardown_plan, Guard};

use std::process::Command;
use std::sync::Mutex;

static OPV_LOCK: Mutex<()> = Mutex::new(());

fn run_cmd(plan: &str, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(args)
        .env("TRAVEL_PLAN_ID", plan)
        .output()
        .unwrap_or_else(|e| panic!("run travel {args:?}: {e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn mark_booked_prints_resolved_plan_id() {
    let _g = OPV_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zztest-opv-{tag}");
    let dest = format!("zzopv{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping (no Turso creds)");
        return;
    }
    seed_plan(&plan, &dest, 0);

    // dry-run: no writes, but the header must still name the resolved plan.
    let (ok, stdout, stderr) = run_cmd(&plan, &["mark-booked", "--dest", &dest, "--dry-run"]);
    assert!(ok, "dry-run should succeed; stderr={stderr}");
    assert!(
        stdout.contains(&plan),
        "mark-booked header must print the resolved plan_id (dest is shared by multiple plans); got:\n{stdout}"
    );
}

#[test]
fn sync_bookings_prints_effective_trip_id() {
    let _g = OPV_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zztest-opv-{tag}");
    let dest = format!("zzopv{tag}");
    let trip = plan.replace('-', "_");
    let _g = Guard::new({
        let (plan, dest, trip) = (plan.clone(), dest.clone(), trip.clone());
        move || {
            let _ = common::db_exec_teardown(&format!(
                "DELETE FROM bookings_current_payload WHERE booking_key LIKE '{trip}:%'; \
                 DELETE FROM bookings_event_data WHERE booking_key LIKE '{trip}:%'; \
                 DELETE FROM bookings_events WHERE booking_key LIKE '{trip}:%'; \
                 DELETE FROM bookings_current WHERE trip_id = '{trip}'"
            ));
            teardown_plan(&plan, &dest);
        }
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping (no Turso creds)");
        return;
    }
    seed_plan(&plan, &dest, 0);

    let (ok, stdout, stderr) = run_cmd(&plan, &["sync-bookings", "--dry-run"]);
    assert!(ok, "sync-bookings --dry-run should succeed; stderr={stderr}");
    assert!(
        stdout.contains(&trip),
        "sync-bookings must print the effective trip_id — the delete-and-rewrite scope; got:\n{stdout}"
    );
}

#[test]
fn share_token_show_lists_plan_ownership() {
    let _g = OPV_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zztest-opv-{tag}");
    let dest = format!("zzopv{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping (no Turso creds)");
        return;
    }
    seed_plan(&plan, &dest, 0);
    db_exec(&format!(
        "INSERT INTO plan_share_tokens (plan_id, token, created_at, status) \
         VALUES ('{plan}', 'aa{tag:030x}', '2026-10-29 00:00:00', 'active')"
    ))
    .expect("seed token");

    let (ok, stdout, stderr) = run_cmd(&plan, &["share-token", "--show"]);
    assert!(ok, "share-token --show should succeed; stderr={stderr}");
    // Backward-compatible shape: the line still LEADS with `token:` (codex
    // review 2026-10-29: preserve the prefix), with plan ownership appended.
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("token:") && l.contains(&plan)),
        "every token line must keep the token: prefix and carry plan ownership; got:\n{stdout}"
    );
}

#[test]
fn share_token_malformed_listing_never_mints() {
    let _g = OPV_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zztest-opv-{tag}");
    let dest = format!("zzopv{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping (no Turso creds)");
        return;
    }
    seed_plan(&plan, &dest, 0);

    // `--dest --show`: the bare `--show` must not be swallowed as the --dest
    // value, silently leaving the default MINT action (codex review). Any
    // malformed listing invocation must reject and mint NOTHING.
    for args in [
        vec!["share-token", "--dest", "--show"],
        vec!["share-token", "--show", "deactivate", "aa"],
    ] {
        let (ok, stdout, stderr) = run_cmd(&plan, &args);
        let minted = db_exec(&format!(
            "SELECT COUNT(*) AS n FROM plan_share_tokens WHERE plan_id = '{plan}'"
        ))
        .and_then(|r| r.scalar().and_then(|s| s.parse::<i64>().ok()))
        .unwrap_or(1);
        assert!(
            !ok && minted == 0,
            "{args:?} must be rejected (conflicting/swallowed actions) without minting; \
             ok={ok} minted={minted} stdout={stdout} stderr={stderr}"
        );
    }
}

#[test]
fn share_token_all_alone_does_not_mint() {
    let _g = OPV_LOCK.lock().unwrap();
    let tag = nanos();
    let plan = format!("zztest-opv-{tag}");
    let dest = format!("zzopv{tag}");
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });
    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping (no Turso creds)");
        return;
    }
    seed_plan(&plan, &dest, 0);

    let (ok, stdout, stderr) = run_cmd(&plan, &["share-token", "--all"]);
    let minted = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM plan_share_tokens WHERE plan_id = '{plan}'"
    ))
    .and_then(|r| r.scalar().and_then(|s| s.parse::<i64>().ok()))
    .unwrap_or(1); // treat a failed count check as minted (fail the assert)

    assert!(
        !ok && minted == 0,
        "share-token --all alone must be rejected, not fall through to mint \
         (ok={ok}, minted={minted}); stdout={stdout} stderr={stderr}"
    );
}
