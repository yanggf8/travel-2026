//! `travel clear-offer [--dest <slug>]` — undo `select-offer`: remove the
//! destination's `plan_offer_selection` row (e.g. the agency could not deliver
//! and the trip was booked elsewhere). NO CASCADE: P3/P4 statuses, flight_legs
//! and hotels are left alone — re-stamp their provenance with
//! `set-flight --source` / `set-hotel --source` (the command lists how many
//! rows still say `package:<offer>`).
//!
//! Audited: one timeline `plan_events` row (`offer_selection_cleared`), its KV,
//! `operation_runs`, and a `plans.version` bump.

use crate::cascade::common::{
    insert_event, insert_kv_rows, next_timeline_sort_order, now_db_datetime, now_rfc3339,
    read_version, record_operation, resolve_active_destination,
};
use travel_db::repo::{plan, plan_offers};

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let dest = parse(args)?;
    let conn = crate::db::connect_write().await?;
    let destination = resolve_active_destination(&conn, &plan_id, dest.as_deref()).await?;

    let Some((offer_id, _, _)) = plan::offer_selection(&conn, &plan_id, &destination).await? else {
        return Err(format!(
            "clear-offer: no selected offer for {plan_id} / {destination}"
        ));
    };

    let now_db = now_db_datetime();
    let now_iso = now_rfc3339();
    let version_before = read_version(&conn, &plan_id).await?;
    let version_after = version_before + 1;

    plan_offers::clear_selection(&conn, &plan_id, &destination).await?;

    let sort_order = next_timeline_sort_order(&conn, &plan_id).await?;
    insert_event(
        &conn,
        &plan_id,
        "timeline",
        "",
        "",
        sort_order,
        "offer_selection_cleared",
        &now_iso,
        None,
        None,
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
            ("offer_id", offer_id.clone()),
        ],
    )
    .await?;
    record_operation(
        &conn,
        &plan_id,
        "clear-offer",
        &format!("clear-offer {destination} offer={offer_id}"),
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    println!("✅ Cleared offer selection");
    println!("plan: {plan_id}");
    println!("destination: {destination}");
    println!("offer: {offer_id}");
    println!("version: {version_before} -> {version_after}");
    let stale =
        plan_offers::package_populated_rows(&conn, &plan_id, &destination, &offer_id).await?;
    if stale > 0 {
        println!(
            "note: {stale} flight/hotel row(s) still say populated_from=package:{offer_id} — \
             re-stamp with set-flight <dir> --source <text> / set-hotel --source <text>"
        );
    }
    Ok(())
}

fn parse(args: &[String]) -> Result<Option<String>, String> {
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
            // plan-selection flags (--plan-id / --travel-date / …) are consumed by the
            // resolver; skip flag + value.
            f if crate::plan_resolver::is_resolver_flag(f) => i += 2,
            other => return Err(format!("clear-offer: unknown argument '{other}'")),
        }
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_dest_and_rejects_stray_args() {
        assert_eq!(parse(&s(&["--dest", "kyoto_2026"])).unwrap().as_deref(), Some("kyoto_2026"));
        assert_eq!(parse(&[]).unwrap(), None);
        assert!(parse(&s(&["offer_x"])).is_err());
        assert!(parse(&s(&["--dest"])).is_err());
    }
}
