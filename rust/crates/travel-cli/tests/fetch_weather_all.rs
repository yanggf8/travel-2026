//! Real-Turso check: each destination commits independently and --all reports failures.
use std::process::Command;
mod common;
use common::{bin, db_exec, db_exec_teardown, is_credless, nanos, seed_plan, teardown_plan, Guard};

#[test]
fn all_partial_failure_commits_good_dest_and_exits_nonzero() {
    let n = nanos();
    let plan = format!("zz_test_{n}");
    let a = format!("zz_test_{n}_a");
    let b = format!("zz_test_{n}_b");
    let _guard = Guard::new({
        let (plan, a, b) = (plan.clone(), a.clone(), b.clone());
        move || {
            let _ = db_exec_teardown(&format!("DELETE FROM destination_config WHERE slug IN ('{a}','{b}')"));
            teardown_plan(&plan, &a);
        }
    });
    if std::env::var("TRAVEL_TURSO_URL").is_err() { eprintln!("skip: Turso credentials absent"); return; }
    seed_plan(&plan, &a, 5);
    let date = chrono::Utc::now().date_naive().succ_opt().unwrap().format("%Y-%m-%d").to_string();
    db_exec(&format!("INSERT INTO plan_destinations (plan_id,slug,display_name,status) VALUES ('{plan}','{b}','Bad','active'); \
        INSERT INTO destination_config (slug,display_name,timezone,lat,lon) VALUES ('{a}','Kyoto','Asia/Tokyo',35.0116,135.7681),('{b}','Bad','Asia/Tokyo',95,135.7681); \
        INSERT INTO days (plan_id,destination,day_number,date,day_type,weather_label) VALUES ('{plan}','{a}',1,'{date}','full','old'),('{plan}','{b}',1,'{date}','full','old');"))
        .expect("seed destinations and days");
    let out = Command::new(bin()).args(["fetch-weather", "--plan-id", &plan, "--all"]).output().expect("run fetch-weather");
    let stderr = String::from_utf8_lossy(&out.stderr);
    if is_credless(&stderr) { eprintln!("skip: {stderr}"); return; }
    let a_label = db_exec(&format!("SELECT weather_label FROM days WHERE plan_id='{plan}' AND destination='{a}'")).expect("read a").scalar();
    if a_label.as_deref() == Some("old") && (stderr.contains("curl exit") || stderr.contains("did not respond") || stderr.contains("TLS")) {
        eprintln!("skip: Open-Meteo unavailable for good destination: {stderr}");
        return;
    }
    assert!(!out.status.success(), "stdout={} stderr={stderr}", String::from_utf8_lossy(&out.stdout));
    assert!(stderr.contains(&b) && stderr.contains("HTTP 400"), "{stderr}");
    assert_ne!(a_label.as_deref(), Some("old"));
    assert_eq!(db_exec(&format!("SELECT weather_label FROM days WHERE plan_id='{plan}' AND destination='{b}'")).unwrap().scalar().as_deref(), Some("old"));
    assert_eq!(db_exec(&format!("SELECT version FROM plans WHERE plan_id='{plan}'")).unwrap().scalar().as_deref(), Some("6"));
    assert_eq!(db_exec(&format!("SELECT count(*) FROM operation_runs WHERE plan_id='{plan}'")).unwrap().scalar().as_deref(), Some("1"));
    assert_eq!(db_exec(&format!("SELECT command_summary FROM operation_runs WHERE plan_id='{plan}'")).unwrap().scalar().as_deref(), Some(a.as_str()));
}
