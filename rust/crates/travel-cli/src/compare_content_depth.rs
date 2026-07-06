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

fn destination_for(plan_id: &str) -> String {
    plan_id.replace('-', "_")
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

pub async fn run(rest: &[String]) -> Result<(), String> {
    let args = ContentDepthArgs::parse(rest)?;
    let conn = db::connect_read().await?;

    let drill_dest = destination_for(&args.plan_id);
    let ref_dest = destination_for(&args.against);

    let drill_rows = depth_rows(&conn, &args.plan_id, &drill_dest).await?;
    let ref_rows = depth_rows(&conn, &args.against, &ref_dest).await?;

    println!("activities meals routes");
    for row in &drill_rows {
        println!(
            "{} {} {}/{}/{}",
            row.day_number, row.day_type, row.activities, row.meals, row.routes
        );
    }
    for row in &ref_rows {
        println!(
            "ref {} {} {}/{}/{}",
            row.day_number, row.day_type, row.activities, row.meals, row.routes
        );
    }

    Ok(())
}