use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

mod common;
use common::Guard;

static UPDATE_OFFER_LOCK: Mutex<()> = Mutex::new(());

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_travel")
}

fn nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn db_exec(sql: &str) -> (bool, String, String) {
    let out = Command::new(bin())
        .args(["db", "exec", sql])
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run db exec");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn is_skip(stderr: &str) -> bool {
    stderr.contains("turso auth login")
        || stderr.contains("Missing Turso")
        || stderr.contains("Missing Turso data")
        || stderr.contains("failed to connect to Turso")
        || stderr.contains("TRAVEL_TURSO")
}

fn db_or_skip(sql: &str) -> Option<String> {
    let (ok, stdout, stderr) = db_exec(sql);
    if ok {
        return Some(stdout);
    }
    if is_skip(&stderr) {
        eprintln!("skipping update-offer test (no Turso creds): {}", stderr.trim());
        return None;
    }
    panic!("travel db exec failed: {}\nSQL: {sql}", stderr.trim());
}

fn values(sql: &str) -> Option<HashMap<String, String>> {
    let stdout = db_or_skip(sql)?;
    let mut out = HashMap::new();
    for part in stdout.trim().split(',') {
        if let Some((key, value)) = part.trim().split_once(':') {
            out.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    Some(out)
}

fn scalar(sql: &str, col: &str) -> Option<String> {
    values(sql)?.get(col).cloned()
}

fn teardown(plan: &str) {
    let sql = format!(
        "DELETE FROM plan_offer_date_pricing WHERE plan_id = '{plan}'; \
         DELETE FROM operation_runs WHERE plan_id = '{plan}'; \
         DELETE FROM plan_event_data WHERE plan_id = '{plan}'; \
         DELETE FROM plan_events WHERE plan_id = '{plan}'; \
         DELETE FROM plan_metadata WHERE plan_id = '{plan}'; \
         DELETE FROM plans WHERE plan_id = '{plan}';"
    );
    let _ = db_exec(&sql);
}

fn seed(plan: &str, dest: &str, offer: &str, date: &str) -> bool {
    let sql = format!(
        "INSERT OR REPLACE INTO plans (plan_id, schema_version, version) \
           VALUES ('{plan}', '4.2.0', 0); \
         INSERT OR REPLACE INTO plan_metadata (plan_id, schema_version, active_destination) \
           VALUES ('{plan}', '4.2.0', '{dest}'); \
         INSERT OR REPLACE INTO plan_offer_date_pricing \
           (plan_id, destination, offer_id, date, price, availability, seats_remaining, currency) \
           VALUES ('{plan}', '{dest}', '{offer}', '{date}', 10000, 'available', 5, 'TWD');"
    );
    db_or_skip(&sql).is_some()
}

fn run_update(
    plan: &str,
    offer: &str,
    date: &str,
    availability: &str,
    extra: &[&str],
) -> (bool, String, String) {
    let mut args = vec!["update-offer", offer, date, availability];
    args.extend_from_slice(extra);
    args.extend_from_slice(&["--plan-id", plan]);

    let out = Command::new(bin())
        .args(&args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .expect("run update-offer");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn pricing(plan: &str, dest: &str, offer: &str, date: &str) -> Option<HashMap<String, String>> {
    values(&format!(
        "SELECT price AS p, availability AS a, seats_remaining AS s, currency AS c \
         FROM plan_offer_date_pricing \
         WHERE plan_id = '{plan}' AND destination = '{dest}' \
           AND offer_id = '{offer}' AND date = '{date}'"
    ))
}

#[test]
fn update_offer_updates_existing_pricing_and_preserves_omitted_fields() {
    let _lock = UPDATE_OFFER_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_or_skip("SELECT 1").is_none() {
        return;
    }

    let tag = nanos();
    let plan = format!("zztest{tag}");
    let dest = format!("zztest_dest_{tag}");
    let offer = format!("zztest_offer_{tag}");
    let date = "2026-09-04";

    teardown(&plan);
    let _g = Guard::new({
        let plan = plan.clone();
        move || teardown(&plan)
    });

    if !seed(&plan, &dest, &offer, date) {
        return;
    }

    let (ok, stdout, stderr) =
        run_update(&plan, &offer, date, "limited", &["12345", "2", "agent"]);
    if !ok && is_skip(&stderr) {
        eprintln!("skipping update-offer test (no Turso creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "full update should succeed; stdout={stdout} stderr={stderr}");

    let row = pricing(&plan, &dest, &offer, date).expect("pricing query should run");
    assert_eq!(row.get("p").map(String::as_str), Some("12345"));
    assert_eq!(row.get("a").map(String::as_str), Some("limited"));
    assert_eq!(row.get("s").map(String::as_str), Some("2"));
    assert_eq!(row.get("c").map(String::as_str), Some("TWD"));

    let op_count = scalar(
        &format!(
            "SELECT COUNT(*) AS n FROM operation_runs \
             WHERE plan_id = '{plan}' AND command_type = 'update-offer'"
        ),
        "n",
    );
    assert_eq!(op_count.as_deref(), Some("1"));

    let summary = scalar(
        &format!(
            "SELECT command_summary AS cs FROM operation_runs \
             WHERE plan_id = '{plan}' AND command_type = 'update-offer'"
        ),
        "cs",
    );
    assert_eq!(
        summary.as_deref(),
        Some(format!("{offer} {date} limited").as_str())
    );

    let version = scalar(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'"), "v");
    assert_eq!(version.as_deref(), Some("1"));

    let (ok, stdout, stderr) = run_update(&plan, &offer, date, "sold_out", &[]);
    if !ok && is_skip(&stderr) {
        eprintln!("skipping update-offer test (no Turso creds): {}", stderr.trim());
        return;
    }
    assert!(
        ok,
        "omitted price/seats update should succeed; stdout={stdout} stderr={stderr}"
    );

    let row = pricing(&plan, &dest, &offer, date).expect("pricing query should run");
    assert_eq!(row.get("p").map(String::as_str), Some("12345"));
    assert_eq!(row.get("a").map(String::as_str), Some("sold_out"));
    assert_eq!(row.get("s").map(String::as_str), Some("2"));
    assert_eq!(row.get("c").map(String::as_str), Some("TWD"));

    let version = scalar(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'"), "v");
    assert_eq!(version.as_deref(), Some("2"));
}
