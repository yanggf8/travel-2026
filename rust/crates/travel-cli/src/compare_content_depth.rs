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

/// ZH slot-completeness gate, aligned verbatim with validate.rs missing_day_zh/missing_session_zh.
/// Returns (translated_eligible, total_eligible) over content-bearing days + sessions.
async fn zh_gate(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<(i64, i64), String> {
    // Day: eligible = activities OR meals OR routes; translated = theme_zh non-blank.
    // Session: eligible = activities OR meals OR transit_notes OR transit_notes_zh;
    //          translated = focus_zh non-blank OR transit_notes_zh non-blank.
    let sql = "SELECT
      (SELECT COUNT(*) FROM days d WHERE d.plan_id=?1 AND d.destination=?2
         AND ( EXISTS(SELECT 1 FROM activities a WHERE a.plan_id=d.plan_id AND a.destination=d.destination AND a.day_number=d.day_number)
            OR EXISTS(SELECT 1 FROM session_meals m WHERE m.plan_id=d.plan_id AND m.destination=d.destination AND m.day_number=d.day_number)
            OR EXISTS(SELECT 1 FROM day_route_segments r WHERE r.plan_id=d.plan_id AND r.destination=d.destination AND r.day_number=d.day_number) )
      ) AS day_elig,
      (SELECT COUNT(*) FROM days d WHERE d.plan_id=?1 AND d.destination=?2
         AND ( EXISTS(SELECT 1 FROM activities a WHERE a.plan_id=d.plan_id AND a.destination=d.destination AND a.day_number=d.day_number)
            OR EXISTS(SELECT 1 FROM session_meals m WHERE m.plan_id=d.plan_id AND m.destination=d.destination AND m.day_number=d.day_number)
            OR EXISTS(SELECT 1 FROM day_route_segments r WHERE r.plan_id=d.plan_id AND r.destination=d.destination AND r.day_number=d.day_number) )
         AND NULLIF(TRIM(COALESCE(d.theme_zh,'')),'') IS NOT NULL
      ) AS day_tr,
      (SELECT COUNT(*) FROM timesofday t WHERE t.plan_id=?1 AND t.destination=?2
         AND ( EXISTS(SELECT 1 FROM activities a WHERE a.plan_id=t.plan_id AND a.destination=t.destination AND a.day_number=t.day_number AND a.session_type=t.session_type)
            OR EXISTS(SELECT 1 FROM session_meals m WHERE m.plan_id=t.plan_id AND m.destination=t.destination AND m.day_number=t.day_number AND m.session_type=t.session_type)
            OR NULLIF(TRIM(COALESCE(t.transit_notes,'')),'') IS NOT NULL
            OR NULLIF(TRIM(COALESCE(t.transit_notes_zh,'')),'') IS NOT NULL )
      ) AS sess_elig,
      (SELECT COUNT(*) FROM timesofday t WHERE t.plan_id=?1 AND t.destination=?2
         AND ( EXISTS(SELECT 1 FROM activities a WHERE a.plan_id=t.plan_id AND a.destination=t.destination AND a.day_number=t.day_number AND a.session_type=t.session_type)
            OR EXISTS(SELECT 1 FROM session_meals m WHERE m.plan_id=t.plan_id AND m.destination=t.destination AND m.day_number=t.day_number AND m.session_type=t.session_type)
            OR NULLIF(TRIM(COALESCE(t.transit_notes,'')),'') IS NOT NULL
            OR NULLIF(TRIM(COALESCE(t.transit_notes_zh,'')),'') IS NOT NULL )
         AND ( NULLIF(TRIM(COALESCE(t.focus_zh,'')),'') IS NOT NULL
            OR NULLIF(TRIM(COALESCE(t.transit_notes_zh,'')),'') IS NOT NULL )
      ) AS sess_tr";
    let mut rows = conn
        .query(sql, params![plan_id.to_string(), destination.to_string()])
        .await
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
        let day_elig: i64 = row.get(0).unwrap_or(0);
        let day_tr: i64 = row.get(1).unwrap_or(0);
        let sess_elig: i64 = row.get(2).unwrap_or(0);
        let sess_tr: i64 = row.get(3).unwrap_or(0);
        return Ok((day_tr + sess_tr, day_elig + sess_elig));
    }
    Ok((0, 0))
}

struct Totals {
    activities: i64,
    meals: i64,
    routes: i64,
}

fn totals_of(rows: &[DepthRow]) -> Totals {
    Totals {
        activities: rows.iter().map(|r| r.activities).sum(),
        meals: rows.iter().map(|r| r.meals).sum(),
        routes: rows.iter().map(|r| r.routes).sum(),
    }
}

// SHORT (any depth axis drill<ref, or drill ZH gate FAIL) / ALIGNED / BETTER.
// Only the DRILL gate enters verdict; reference gate never does.
fn verdict(drill: &Totals, refr: &Totals, drill_gate_pass: bool) -> String {
    let axes: [(&str, i64, i64); 3] = [
        ("activities", drill.activities, refr.activities),
        ("meals", drill.meals, refr.meals),
        ("routes", drill.routes, refr.routes),
    ];
    let mut short: Vec<String> = axes
        .iter()
        .filter(|(_, d, r)| d < r)
        .map(|(n, _, _)| n.to_string())
        .collect();
    if !drill_gate_pass {
        short.push("ZH-gate".to_string());
    }
    if !short.is_empty() {
        return format!("VERDICT: SHORT: {}", short.join(", "));
    }
    let strictly_greater = axes.iter().filter(|(_, d, r)| d > r).count();
    if strictly_greater == 0 {
        return "VERDICT: ALIGNED — all 3 depth axes equal reference; ZH gate PASS"
            .to_string();
    }
    format!(
        "VERDICT: BETTER — all 3 depth axes >= reference, {strictly_greater} strictly greater; ZH gate PASS"
    )
}

fn delta(d: i64, r: i64) -> String {
    let x = d - r;
    if x > 0 {
        format!("+{x}")
    } else {
        format!("{x}")
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
    let drill_dest =
        crate::cascade::common::resolve_active_destination(&conn, &args.plan_id, None).await?;
    let ref_dest =
        crate::cascade::common::resolve_active_destination(&conn, &args.against, None).await?;

    let drill_rows = depth_rows(&conn, &args.plan_id, &drill_dest).await?;
    let ref_rows = depth_rows(&conn, &args.against, &ref_dest).await?;
    let drill_gate = zh_gate(&conn, &args.plan_id, &drill_dest).await?;
    let ref_gate = zh_gate(&conn, &args.against, &ref_dest).await?;

    let dt = totals_of(&drill_rows);
    let rt = totals_of(&ref_rows);

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
        let (da, dm, dr) = d
            .map(|x| (x.activities, x.meals, x.routes))
            .unwrap_or((0, 0, 0));
        let (ra, rm, rr) = r
            .map(|x| (x.activities, x.meals, x.routes))
            .unwrap_or((0, 0, 0));
        let dcell = format!("{da}/{dm}/{dr}");
        println!("  {:<4} {:<11} {:<14} {}/{}/{}", day, day_type, dcell, ra, rm, rr);
    }
    println!();

    // Totals (3 depth axes only — ZH is a gate, not a depth axis)
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
    println!("  ----------------------------------------------------");

    let (dn, dd) = drill_gate;
    let (rn, rd) = ref_gate;
    let dp = dn == dd;
    let rp = rn == rd;
    println!("\ngates:");
    println!(
        "  ZH slot completeness  drill {dn}/{dd}  {}",
        if dp { "PASS" } else { "FAIL" }
    );
    println!(
        "  ZH slot completeness  ref   {rn}/{rd}  {}",
        if rp { "PASS" } else { "FAIL" }
    );
    if !rp {
        println!(
            "⚠ reference ZH gate FAIL ({rn}/{rd}); depth comparison continues; drill must independently PASS"
        );
    }
    println!("\n{}", verdict(&dt, &rt, dp));

    Ok(())
}
