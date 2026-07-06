mod common;
use common::{bin, db_exec, is_credless, nanos, seed_plan, teardown_plan, Guard};
use std::process::Command;

fn run_or_skip(args: &[&str]) -> Option<String> {
    let out = Command::new(bin())
        .args(args)
        .env_remove("TRAVEL_PLAN_ID")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() && is_credless(&stderr) {
        return None;
    }
    assert!(
        out.status.success(),
        "cmd {args:?} failed; stdout={stdout} stderr={stderr}"
    );
    Some(stdout)
}

#[test]
fn help_prints_usage() {
    let out = Command::new(bin())
        .args(["compare", "content-depth", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("Usage:"), "stdout: {s}");
    assert!(
        s.contains("travel compare content-depth --plan-id"),
        "stdout: {s}"
    );
    assert!(
        s.contains("okinawa-2026"),
        "help should name the default reference; stdout: {s}"
    );
}

#[test]
fn missing_plan_id_fails() {
    let out = Command::new(bin())
        .args(["compare", "content-depth", "--against", "okinawa-2026"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let s = String::from_utf8_lossy(&out.stderr);
    assert!(s.contains("--plan-id"), "stderr: {s}");
}

#[test]
fn unknown_flag_fails() {
    let out = Command::new(bin())
        .args([
            "compare",
            "content-depth",
            "--plan-id",
            "x-2026",
            "--bogus",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let s = String::from_utf8_lossy(&out.stderr);
    assert!(
        s.to_lowercase().contains("unknown flag"),
        "stderr: {s}"
    );
}

#[test]
fn gated_query_excludes_blank_meals_and_metadataless_routes() {
    let n = nanos();
    let plan = format!("test-cdepth-drill-{n}");
    let dest = plan.replace('-', "_");
    seed_plan(&plan, &dest, 0);
    let _g = Guard::new({
        let (p, d) = (plan.clone(), dest.clone());
        move || teardown_plan(&p, &d)
    });

    if db_exec(&format!(
        "INSERT INTO days (plan_id,destination,day_number,date,day_type,status,updated_at) VALUES ('{plan}','{dest}',1,'2026-11-01','full','draft','2020-01-01 00:00:00'); \
      INSERT INTO activities (id,plan_id,destination,day_number,session_type,sort_order,title,updated_at) VALUES ('{n}-a0','{plan}','{dest}',1,'morning',0,'act0','2020-01-01 00:00:00'),('{n}-a1','{plan}','{dest}',1,'morning',1,'act1','2020-01-01 00:00:00'),('{n}-a2','{plan}','{dest}',1,'afternoon',0,'act2','2020-01-01 00:00:00'); \
      INSERT INTO session_meals (plan_id,destination,day_number,session_type,sort_order,meal,source) VALUES ('{plan}','{dest}',1,'noon',0,'Real lunch','ai_recommended'),('{plan}','{dest}',1,'evening',0,'Real dinner','ai_recommended'),('{plan}','{dest}',1,'noon',1,'   ','ai_recommended'); \
      INSERT INTO day_route_segments (plan_id,destination,day_number,sort_order,from_place,to_place,mode,duration_min,source) VALUES ('{plan}','{dest}',1,0,'A','B','walk',10,'ai_recommended'),('{plan}','{dest}',1,1,'B','C','train',15,'ai_recommended'),('{plan}','{dest}',1,2,'C','D','walk',NULL,'ai_recommended'),('{plan}','{dest}',1,3,'D','E','walk',0,'ai_recommended')"
    ))
    .is_none()
    {
        return;
    }

    let Some(s) = run_or_skip(&[
        "compare",
        "content-depth",
        "--plan-id",
        &plan,
        "--against",
        &plan,
    ]) else {
        return;
    };
    assert!(
        s.contains("3/2/2"),
        "gated per-day a/m/r should be 3/2/2 (blank meal + NULL/zero routes excluded); stdout: {s}"
    );
    assert!(
        s.contains("activities") && s.contains("meals") && s.contains("routes"),
        "stdout: {s}"
    );
}

#[test]
fn zh_coverage_is_weighted_not_avg() {
    let n = nanos();
    let plan = format!("test-cdepth-zh-{n}");
    let dest = plan.replace('-', "_");
    seed_plan(&plan, &dest, 0);
    let _g = Guard::new({
        let (p, d) = (plan.clone(), dest.clone());
        move || teardown_plan(&p, &d)
    });
    for day in 1..=5 {
        if db_exec(&format!(
            "INSERT INTO days (plan_id,destination,day_number,date,day_type,status,theme_zh,updated_at) VALUES ('{plan}','{dest}',{day},'2026-11-0{day}','full','draft','主題{day}','2020-01-01 00:00:00')"
        ))
        .is_none()
        {
            return;
        }
    }
    let mut filled = 0;
    for day in 1..=5 {
        for st in ["morning", "noon", "afternoon", "evening"] {
            let zh = if filled < 17 {
                filled += 1;
                "'焦點'"
            } else {
                "NULL"
            };
            db_exec(&format!(
                "INSERT INTO timesofday (plan_id,destination,day_number,session_type,focus_zh) VALUES ('{plan}','{dest}',{day},'{st}',{zh})"
            ));
        }
    }
    let Some(s) = run_or_skip(&[
        "compare",
        "content-depth",
        "--plan-id",
        &plan,
        "--against",
        &plan,
    ]) else {
        return;
    };
    assert!(
        s.contains("88%"),
        "expected weighted 88%, not 92%; stdout: {s}"
    );
    assert!(
        !s.contains("92%"),
        "must not use avg-of-ratios (92%); stdout: {s}"
    );
}