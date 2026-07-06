// `travel create-plan <plan_id> --dest <slug> --start YYYY-MM-DD --end YYYY-MM-DD
// --airport <IATA> [--region <name>] [--display-name <name>] [--origin <code>] [--nights N]`
//
// Fast-path plan creation: seeds plans + plan_metadata + date_anchors + the
// 6-process ladder via `travel_db::repo::plan_lifecycle::create_plan_seed`.
// plan_id is a REQUIRED positional — NOT routed through plan_resolver.

use libsql::Connection;

#[derive(Debug)]
struct Parsed {
    plan_id: String,
    dest: String,
    start: String,
    end: String,
    airport: String,
    region: Option<String>,
    display_name: Option<String>,
    origin: Option<String>,
    nights: Option<i64>,
}

#[derive(Debug)]
struct DestCfg {
    display_name: String,
    origin: Option<String>,
}

const USAGE: &str = "\
Usage:
  travel create-plan <plan_id> --dest <slug> --start YYYY-MM-DD --end YYYY-MM-DD --airport <IATA> \
[--region <name>] [--display-name <name>] [--origin <code>] [--nights N]

Create a fast-path plan (plans + metadata + date_anchors + the process ladder) so \
set-flight/set-hotel/itinerary work. Dest must be registered (/new-destination). \
Dates-inclusive — no separate set-dates needed.";

pub async fn run(args: &[String], _plan_id_unused: String) -> Result<(), String> {
    let parsed = parse(args)?;

    let days = crate::validate::validate_date_range(&parsed.start, &parsed.end)? as i64;

    let conn = crate::db::connect_write().await?;

    if plan_exists(&conn, &parsed.plan_id).await? {
        return Err(format!("Plan already exists: {}", parsed.plan_id));
    }

    let cfg = read_destination_config(&conn, &parsed.dest)
        .await?
        .ok_or_else(|| {
            format!(
                "Destination config not found: {}. Register it with /new-destination first.",
                parsed.dest
            )
        })?;

    let display_name = parsed
        .display_name
        .clone()
        .unwrap_or(cfg.display_name);
    let origin_code = parsed.origin.clone().or(cfg.origin);
    let region = parsed
        .region
        .clone()
        .unwrap_or_else(|| "japan".to_string());
    let nights = parsed.nights.unwrap_or(days - 1);
    let ts = crate::cascade::common::now_rfc3339();

    let seed = travel_db::repo::plan_lifecycle::PlanSeed {
        plan_id: parsed.plan_id.clone(),
        schema_version: "4.2.0".to_string(),
        dest_slug: parsed.dest.clone(),
        display_name: display_name.clone(),
        origin_code,
        region,
        primary_airport: parsed.airport.to_uppercase(),
        nights,
        start_date: parsed.start.clone(),
        end_date: parsed.end.clone(),
        days,
        session: "cli".to_string(),
        ts: ts.clone(),
    };
    travel_db::repo::plan_lifecycle::create_plan_seed(&conn, &seed).await?;

    crate::cascade::common::insert_event(
        &conn,
        &parsed.plan_id,
        "timeline",
        "",
        "",
        0,
        "plan_created",
        &ts,
        None,
        None,
    )
    .await?;
    crate::cascade::common::insert_kv_rows(
        &conn,
        &parsed.plan_id,
        "timeline",
        "",
        "",
        0,
        &[
            ("dest", parsed.dest.clone()),
            ("start", parsed.start.clone()),
            ("end", parsed.end.clone()),
            ("airport", parsed.airport.to_uppercase()),
        ],
    )
    .await?;
    crate::cascade::common::record_operation(
        &conn,
        &parsed.plan_id,
        "create-plan",
        &format!(
            "create-plan {} {} {}->{}",
            parsed.plan_id, parsed.dest, parsed.start, parsed.end
        ),
        0,
        1,
        &crate::cascade::common::now_db_datetime(),
    )
    .await?;

    println!(
        "\n✅ Created plan {} ({}, {}→{}, {} days)",
        parsed.plan_id, parsed.dest, parsed.start, parsed.end, days
    );
    println!("   airport: {}", parsed.airport.to_uppercase());
    println!("   display: {display_name}");
    Ok(())
}

fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut plan_id: Option<String> = None;
    let mut dest: Option<String> = None;
    let mut start: Option<String> = None;
    let mut end: Option<String> = None;
    let mut airport: Option<String> = None;
    let mut region: Option<String> = None;
    let mut display_name: Option<String> = None;
    let mut origin: Option<String> = None;
    let mut nights: Option<i64> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dest" => {
                dest = Some(arg_value(args, i, "--dest")?);
                i += 2;
            }
            "--start" => {
                start = Some(arg_value(args, i, "--start")?);
                i += 2;
            }
            "--end" => {
                end = Some(arg_value(args, i, "--end")?);
                i += 2;
            }
            "--airport" => {
                airport = Some(arg_value(args, i, "--airport")?);
                i += 2;
            }
            "--region" => {
                region = Some(arg_value(args, i, "--region")?);
                i += 2;
            }
            "--display-name" => {
                display_name = Some(arg_value(args, i, "--display-name")?);
                i += 2;
            }
            "--origin" => {
                origin = Some(arg_value(args, i, "--origin")?);
                i += 2;
            }
            "--nights" => {
                let raw = arg_value(args, i, "--nights")?;
                nights = Some(
                    raw.parse::<i64>()
                        .map_err(|_| format!("--nights must be an integer (got \"{raw}\")"))?,
                );
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            other => {
                if plan_id.is_none() {
                    plan_id = Some(other.to_string());
                } else {
                    return Err(format!("unexpected positional argument: {other}\n{USAGE}"));
                }
                i += 1;
            }
        }
    }

    let plan_id = plan_id.ok_or_else(|| format!("missing required <plan_id>\n{USAGE}"))?;
    let dest = dest.ok_or_else(|| format!("missing required --dest\n{USAGE}"))?;
    let start = start.ok_or_else(|| format!("missing required --start\n{USAGE}"))?;
    let end = end.ok_or_else(|| format!("missing required --end\n{USAGE}"))?;
    let airport = airport.ok_or_else(|| format!("missing required --airport\n{USAGE}"))?;

    Ok(Parsed {
        plan_id,
        dest,
        start,
        end,
        airport,
        region,
        display_name,
        origin,
        nights,
    })
}

fn arg_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("missing value for {flag}"))
}

async fn plan_exists(conn: &Connection, plan_id: &str) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM plans WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plans query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plans row read failed: {e}"))?
    {
        let count: i64 = row.get(0).unwrap_or(0);
        return Ok(count > 0);
    }
    Ok(false)
}

async fn read_destination_config(
    conn: &Connection,
    slug: &str,
) -> Result<Option<DestCfg>, String> {
    let mut rows = conn
        .query(
            "SELECT display_name, origin FROM destination_config WHERE slug = ?1",
            libsql::params![slug.to_string()],
        )
        .await
        .map_err(|e| format!("destination_config query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("destination_config row read failed: {e}"))?
    {
        let display_name: String = row.get(0).unwrap_or_default();
        let origin: Option<String> = row.get(1).ok();
        return Ok(Some(DestCfg {
            display_name,
            origin,
        }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal() {
        let p = parse(&[
            "my-plan".to_string(),
            "--dest".to_string(),
            "tokyo_2026".to_string(),
            "--start".to_string(),
            "2026-06-18".to_string(),
            "--end".to_string(),
            "2026-06-24".to_string(),
            "--airport".to_string(),
            "NRT".to_string(),
        ])
        .unwrap();
        assert_eq!(p.plan_id, "my-plan");
        assert_eq!(p.dest, "tokyo_2026");
        assert_eq!(p.start, "2026-06-18");
        assert_eq!(p.end, "2026-06-24");
        assert_eq!(p.airport, "NRT");
        assert!(p.region.is_none());
        assert!(p.nights.is_none());
    }

    #[test]
    fn parse_optional_flags() {
        let p = parse(&[
            "p1".to_string(),
            "--dest".to_string(),
            "d".to_string(),
            "--start".to_string(),
            "2026-01-01".to_string(),
            "--end".to_string(),
            "2026-01-05".to_string(),
            "--airport".to_string(),
            "KIX".to_string(),
            "--region".to_string(),
            "japan".to_string(),
            "--display-name".to_string(),
            "Custom".to_string(),
            "--origin".to_string(),
            "taiwan".to_string(),
            "--nights".to_string(),
            "4".to_string(),
        ])
        .unwrap();
        assert_eq!(p.region.as_deref(), Some("japan"));
        assert_eq!(p.display_name.as_deref(), Some("Custom"));
        assert_eq!(p.origin.as_deref(), Some("taiwan"));
        assert_eq!(p.nights, Some(4));
    }

    #[test]
    fn parse_missing_plan_id_errors() {
        assert!(parse(&[
            "--dest".to_string(),
            "d".to_string(),
            "--start".to_string(),
            "2026-01-01".to_string(),
            "--end".to_string(),
            "2026-01-05".to_string(),
            "--airport".to_string(),
            "KIX".to_string(),
        ])
        .is_err());
    }

    #[test]
    fn parse_unknown_flag_errors() {
        assert!(parse(&[
            "p".to_string(),
            "--bogus".to_string(),
            "--dest".to_string(),
            "d".to_string(),
            "--start".to_string(),
            "2026-01-01".to_string(),
            "--end".to_string(),
            "2026-01-05".to_string(),
            "--airport".to_string(),
            "KIX".to_string(),
        ])
        .is_err());
    }
}