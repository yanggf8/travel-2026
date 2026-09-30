//! `travel set-day-excursion <day> on|off [--dest <slug>]` — flag a far-off day
//! trip (e.g. a coach tour 100 km out). snapshot-maps keeps a flagged day's stops
//! off the sightseeing overview (plan.png) — they would flatten its zoom — and
//! draws them as their own inset map (plan-excursion.png) beside the
//! hotel/airport inset. The day's own day-N.png is unchanged.
//!
//! Audited: one timeline `plan_events` row (`day_excursion_set`), its KV,
//! `operation_runs`, and a `plans.version` bump. Re-run snapshot-maps after.

use crate::cascade::common::{
    insert_event, insert_kv_rows, next_timeline_sort_order, now_db_datetime, now_rfc3339,
    read_version, record_operation, resolve_active_destination,
};
use travel_db::repo::itinerary;

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let (day, on, dest) = parse(args)?;
    let conn = crate::db::connect_write().await?;
    let destination = resolve_active_destination(&conn, &plan_id, dest.as_deref()).await?;
    let Some(before) = itinerary::day_excursion(&conn, &plan_id, &destination, day).await? else {
        return Err(format!("set-day-excursion: day {day} not found for {plan_id} / {destination}"));
    };
    let state = if on { "on" } else { "off" };
    if before == on {
        println!("day {day} excursion already {state} — no change");
        return Ok(());
    }

    let now_db = now_db_datetime();
    let now_iso = now_rfc3339();
    let version_before = read_version(&conn, &plan_id).await?;
    itinerary::set_day_excursion(&conn, &plan_id, &destination, day, on).await?;
    itinerary::touch_day(&conn, &plan_id, &destination, day, &now_db).await?;

    let sort_order = next_timeline_sort_order(&conn, &plan_id).await?;
    insert_event(
        &conn, &plan_id, "timeline", "", "", sort_order, "day_excursion_set", &now_iso, None, None,
    )
    .await?;
    insert_kv_rows(
        &conn,
        &plan_id,
        "timeline",
        "",
        "",
        sort_order,
        &[
            ("destination", destination.clone()),
            ("day", day.to_string()),
            ("excursion", state.to_string()),
        ],
    )
    .await?;
    record_operation(
        &conn,
        &plan_id,
        "set-day-excursion",
        &format!("set-day-excursion {destination} day={day} {state}"),
        version_before,
        version_before + 1,
        &now_db,
    )
    .await?;

    println!("✅ Day {day} excursion {state}");
    println!("plan: {plan_id}");
    println!("destination: {destination}");
    println!("next: ./bin/travel snapshot-maps --plan-id {plan_id}");
    Ok(())
}

fn parse(args: &[String]) -> Result<(i64, bool, Option<String>), String> {
    let mut positional: Vec<&str> = Vec::new();
    let mut dest = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dest" => {
                dest = Some(
                    args.get(i + 1)
                        .filter(|v| !v.starts_with("--"))
                        .ok_or("--dest requires a value")?
                        .clone(),
                );
                i += 2;
            }
            f if crate::plan_resolver::is_resolver_flag(f) => i += 2,
            f if f.starts_with("--") => {
                return Err(format!("set-day-excursion: unknown argument '{f}'"))
            }
            p => {
                positional.push(p);
                i += 1;
            }
        }
    }
    let [day, state] = positional[..] else {
        return Err("set-day-excursion: usage <day> on|off".into());
    };
    let day = day
        .parse::<i64>()
        .ok()
        .filter(|d| *d >= 1)
        .ok_or(format!("set-day-excursion: day must be a positive integer, got {day}"))?;
    let on = match state {
        "on" => true,
        "off" => false,
        other => return Err(format!("set-day-excursion: state must be on|off, got {other}")),
    };
    Ok((day, on, dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_day_state_dest() {
        assert_eq!(parse(&s(&["3", "on"])).unwrap(), (3, true, None));
        assert_eq!(
            parse(&s(&["2", "off", "--dest", "kyoto"])).unwrap(),
            (2, false, Some("kyoto".into()))
        );
    }

    #[test]
    fn parse_rejects_bad_input() {
        assert!(parse(&s(&["3"])).is_err());
        assert!(parse(&s(&["0", "on"])).is_err());
        assert!(parse(&s(&["x", "on"])).is_err());
        assert!(parse(&s(&["3", "yes"])).is_err());
        assert!(parse(&s(&["3", "on", "extra"])).is_err());
        assert!(parse(&s(&["3", "on", "--bogus"])).is_err());
    }
}
