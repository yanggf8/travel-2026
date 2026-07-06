use crate::db;
use libsql::params;

pub struct ContentDepthArgs {
    pub plan_id: String,
    pub against: String,
}

impl ContentDepthArgs {
    pub fn parse(rest: &[String]) -> Result<Self, String> {
        let mut plan_id: Option<String> = None;
        let mut against = "okinawa-2026".to_string();
        let mut i = 0;
        while i < rest.len() {
            match rest[i].as_str() {
                "--plan-id" => {
                    plan_id = Some(
                        rest.get(i + 1)
                            .ok_or("--plan-id needs a value")?
                            .clone(),
                    );
                    i += 2;
                }
                "--against" => {
                    against = rest
                        .get(i + 1)
                        .ok_or("--against needs a value")?
                        .clone();
                    i += 2;
                }
                other => {
                    return Err(format!("unknown flag for compare content-depth: {other}"));
                }
            }
        }
        Ok(Self {
            plan_id: plan_id.ok_or("compare content-depth requires --plan-id <drill>")?,
            against,
        })
    }
}

struct DepthRow {
    day_number: i64,
    day_type: String,
    activities: i64,
    meals: i64,
    routes: i64,
}

async fn depth_rows(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<Vec<DepthRow>, String> {
    let sql = "WITH day_rows AS (SELECT day_number, day_type FROM days WHERE plan_id=?1 AND destination=?2),
      a AS (SELECT day_number, COUNT(*) n FROM activities WHERE plan_id=?1 AND destination=?2 GROUP BY day_number),
      m AS (SELECT day_number, COUNT(*) n FROM session_meals WHERE plan_id=?1 AND destination=?2 AND session_type IN ('noon','evening') AND TRIM(meal) <> '' GROUP BY day_number),
      r AS (SELECT day_number, COUNT(*) n FROM day_route_segments WHERE plan_id=?1 AND destination=?2 AND duration_min IS NOT NULL AND duration_min > 0 GROUP BY day_number)
      SELECT d.day_number, d.day_type, COALESCE(a.n,0), COALESCE(m.n,0), COALESCE(r.n,0)
      FROM day_rows d LEFT JOIN a ON a.day_number=d.day_number LEFT JOIN m ON m.day_number=d.day_number LEFT JOIN r ON r.day_number=d.day_number
      ORDER BY d.day_number";
    let mut rows = conn
        .query(sql, params![plan_id.to_string(), destination.to_string()])
        .await
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        out.push(DepthRow {
            day_number: row.get(0).map_err(|e| e.to_string())?,
            day_type: row.get(1).map_err(|e| e.to_string())?,
            activities: row.get(2).unwrap_or(0),
            meals: row.get(3).unwrap_or(0),
            routes: row.get(4).unwrap_or(0),
        });
    }
    Ok(out)
}

async fn zh_coverage_pct(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<i64, String> {
    let sql = "SELECT
        (SELECT COUNT(*) FROM days WHERE plan_id=?1 AND destination=?2 AND TRIM(COALESCE(theme_zh,'')) <> '')
      + (SELECT COUNT(*) FROM timesofday WHERE plan_id=?1 AND destination=?2 AND TRIM(COALESCE(focus_zh,'')) <> '') AS num,
        (SELECT COUNT(*) FROM days WHERE plan_id=?1 AND destination=?2)
      + (SELECT COUNT(*) FROM timesofday WHERE plan_id=?1 AND destination=?2) AS den";
    let mut rows = conn
        .query(sql, params![plan_id.to_string(), destination.to_string()])
        .await
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let num: i64 = row.get(0).unwrap_or(0);
        let den: i64 = row.get(1).unwrap_or(0);
        if den == 0 {
            return Ok(0);
        }
        return Ok((num * 100) / den);
    }
    Ok(0)
}

struct Totals {
    activities: i64,
    meals: i64,
    routes: i64,
    zh: i64,
}

fn totals_of(rows: &[DepthRow], zh: i64) -> Totals {
    Totals {
        activities: rows.iter().map(|r| r.activities).sum(),
        meals: rows.iter().map(|r| r.meals).sum(),
        routes: rows.iter().map(|r| r.routes).sum(),
        zh,
    }
}

// SHORT (any axis drill<ref, precedence) / ALIGNED (all >=, none strictly >) / BETTER.
fn verdict(drill: &Totals, refr: &Totals) -> String {
    let axes: [(&str, i64, i64); 4] = [
        ("activities", drill.activities, refr.activities),
        ("meals", drill.meals, refr.meals),
        ("routes", drill.routes, refr.routes),
        ("ZH", drill.zh, refr.zh),
    ];
    let short: Vec<&str> = axes
        .iter()
        .filter(|(_, d, r)| d < r)
        .map(|(n, _, _)| *n)
        .collect();
    if !short.is_empty() {
        return format!("VERDICT: SHORT: {}", short.join(", "));
    }
    let strictly_greater = axes.iter().filter(|(_, d, r)| d > r).count();
    if strictly_greater == 0 {
        return "VERDICT: ALIGNED — every axis meets the reference exactly".to_string();
    }
    format!("VERDICT: BETTER — all axes >= reference, {strictly_greater} strictly greater, quality gate PASS")
}

fn delta(d: i64, r: i64) -> String {
    let x = d - r;
    if x > 0 {
        format!("+{x}")
    } else {
        format!("{x}")
    }
}

fn delta_pp(d: i64, r: i64) -> String {
    let x = d - r;
    if x > 0 {
        format!("+{x}pp")
    } else {
        format!("{x}pp")
    }
}

fn axis_cell(d: i64, r: i64) -> &'static str {
    if d >= r {
        ">="
    } else {
        "<"
    }
}

pub async fn run(rest: &[String]) -> Result<(), String> {
    let args = ContentDepthArgs::parse(rest)?;
    let conn = db::connect_read().await?;
    // Resolve the REAL active destination from plan_metadata (fail-loud, no local
    // fallback). A naive plan_id.replace('-','_') is WRONG for plans whose active
    // destination differs from the plan slug (e.g. kyoto-confirm-2026 → kyoto_2026,
    // osaka-drill-2026 → osaka_kyoto_2026) — it would silently report 0/0/0.
    let drill_dest = crate::cascade::common::resolve_active_destination(&conn, &args.plan_id, None).await?;
    let ref_dest = crate::cascade::common::resolve_active_destination(&conn, &args.against, None).await?;

    let drill_rows = depth_rows(&conn, &args.plan_id, &drill_dest).await?;
    let ref_rows = depth_rows(&conn, &args.against, &ref_dest).await?;
    let drill_zh = zh_coverage_pct(&conn, &args.plan_id, &drill_dest).await?;
    let ref_zh = zh_coverage_pct(&conn, &args.against, &ref_dest).await?;

    let dt = totals_of(&drill_rows, drill_zh);
    let rt = totals_of(&ref_rows, ref_zh);

    println!(
        "CONTENT DEPTH — {}  vs  {} (reference)",
        args.plan_id, args.against
    );
    println!();

    // Per-day (union of day numbers, ordered)
    println!("per-day:");
    println!("  day  type        DRILL(a/m/r)   REF(a/m/r)");
    let mut days: Vec<i64> = drill_rows
        .iter()
        .map(|r| r.day_number)
        .chain(ref_rows.iter().map(|r| r.day_number))
        .collect();
    days.sort_unstable();
    days.dedup();
    for day in days {
        let d = drill_rows.iter().find(|r| r.day_number == day);
        let r = ref_rows.iter().find(|r| r.day_number == day);
        let day_type = d
            .map(|x| x.day_type.clone())
            .or_else(|| r.map(|x| x.day_type.clone()))
            .unwrap_or_default();
        let (da, dm, dr) = d.map(|x| (x.activities, x.meals, x.routes)).unwrap_or((0, 0, 0));
        let (ra, rm, rr) = r.map(|x| (x.activities, x.meals, x.routes)).unwrap_or((0, 0, 0));
        let dcell = format!("{da}/{dm}/{dr}");
        println!("  {:<4} {:<11} {:<14} {}/{}/{}", day, day_type, dcell, ra, rm, rr);
    }
    println!();

    // Totals
    println!("totals:");
    println!("                        DRILL   REF    Δ       verdict");
    println!(
        "  activities            {:<7} {:<6} {:<7} {}",
        dt.activities,
        rt.activities,
        delta(dt.activities, rt.activities),
        axis_cell(dt.activities, rt.activities)
    );
    println!(
        "  meals (real)          {:<7} {:<6} {:<7} {}",
        dt.meals,
        rt.meals,
        delta(dt.meals, rt.meals),
        axis_cell(dt.meals, rt.meals)
    );
    println!(
        "  routes (w/ metadata)  {:<7} {:<6} {:<7} {}",
        dt.routes,
        rt.routes,
        delta(dt.routes, rt.routes),
        axis_cell(dt.routes, rt.routes)
    );
    println!(
        "  ZH coverage           {:<7} {:<6} {:<7} {}",
        format!("{}%", dt.zh),
        format!("{}%", rt.zh),
        delta_pp(dt.zh, rt.zh),
        axis_cell(dt.zh, rt.zh)
    );
    println!("  ----------------------------------------------------");
    println!("  {}", verdict(&dt, &rt));

    Ok(())
}