use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, is_credless, nanos, seed_plan, teardown_plan, Guard};

static UPDATE_OFFER_LOCK: Mutex<()> = Mutex::new(());

fn values(sql: &str) -> Option<HashMap<String, String>> {
    let stdout = db_exec(sql)?;
    let mut out = HashMap::new();
    for part in stdout.raw().trim().split(',') {
        if let Some((key, value)) = part.trim().split_once(':') {
            out.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    Some(out)
}

fn seed(plan: &str, dest: &str, offer: &str, date: &str) {
    seed_plan(plan, dest, 0);
    db_exec(&format!(
        "INSERT OR REPLACE INTO plan_offer_date_pricing \
           (plan_id, destination, offer_id, date, price, availability, seats_remaining, currency) \
           VALUES ('{plan}', '{dest}', '{offer}', '{date}', 10000, 'available', 5, 'TWD');"
    ))
    .expect("creds");
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

    if db_exec("SELECT 1").is_none() {
        eprintln!("skipping update-offer test (no Turso creds)");
        return;
    }

    let tag = nanos();
    let plan = format!("zztest{tag}");
    let dest = format!("zztest_dest_{tag}");
    let offer = format!("zztest_offer_{tag}");
    let date = "2026-09-04";

    teardown_plan(&plan, &dest);
    let _g = Guard::new({
        let (plan, dest) = (plan.clone(), dest.clone());
        move || teardown_plan(&plan, &dest)
    });

    seed(&plan, &dest, &offer, date);

    let (ok, stdout, stderr) =
        run_update(&plan, &offer, date, "limited", &["12345", "2", "agent"]);
    if !ok && is_credless(&stderr) {
        eprintln!("skipping update-offer test (no Turso creds): {}", stderr.trim());
        return;
    }
    assert!(ok, "full update should succeed; stdout={stdout} stderr={stderr}");

    let row = pricing(&plan, &dest, &offer, date).expect("pricing query should run");
    assert_eq!(row.get("p").map(String::as_str), Some("12345"));
    assert_eq!(row.get("a").map(String::as_str), Some("limited"));
    assert_eq!(row.get("s").map(String::as_str), Some("2"));
    assert_eq!(row.get("c").map(String::as_str), Some("TWD"));

    let op_count = db_exec(&format!(
        "SELECT COUNT(*) AS n FROM operation_runs \
         WHERE plan_id = '{plan}' AND command_type = 'update-offer'"
    ))
    .expect("creds")
    .scalar();
    assert_eq!(op_count.as_deref(), Some("1"));

    let summary = db_exec(&format!(
        "SELECT command_summary AS cs FROM operation_runs \
         WHERE plan_id = '{plan}' AND command_type = 'update-offer'"
    ))
    .expect("creds")
    .scalar();
    assert_eq!(
        summary.as_deref(),
        Some(format!("{offer} {date} limited").as_str())
    );

    let version = db_exec(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'"))
        .expect("creds")
        .scalar();
    assert_eq!(version.as_deref(), Some("1"));

    let (ok, stdout, stderr) = run_update(&plan, &offer, date, "sold_out", &[]);
    if !ok && is_credless(&stderr) {
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

    let version = db_exec(&format!("SELECT version AS v FROM plans WHERE plan_id = '{plan}'"))
        .expect("creds")
        .scalar();
    assert_eq!(version.as_deref(), Some("2"));
}