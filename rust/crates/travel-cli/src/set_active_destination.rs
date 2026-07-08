//! `travel set-active-destination <slug>` — switch plan_metadata.active_destination.
//!
//! Audit decision (mirrors mark_plan_deleted): switching the active destination is
//! a plan-metadata/lifecycle change, NOT a domain/itinerary mutation, so it does
//! NOT emit `plan_events`. `plan_events` is a current-state projection keyed to
//! processes/destinations (PK = plan_id/scope/destination/process_id/sort_order),
//! not an append-only history log — a metadata flip has no process to attach to,
//! and a `(timeline,"","",0)` event would COLLIDE with create_plan's `plan_created`
//! row (insert_event DELETEs that exact PK first, destroying the creation record).
//! It DOES write the durable audit every mutation owes: bump plans.version +
//! operation_runs (via record_operation).

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let slug = parse(args)?;
    let conn = crate::db::connect_write().await?;
    let slugs = travel_db::repo::plan_lifecycle::list_destination_slugs(&conn, &plan_id).await?;
    if !slugs.iter().any(|s| s == &slug) {
        return Err(format!("destination {slug} is not a destination of plan {plan_id}"));
    }
    let now_db = crate::cascade::common::now_db_datetime();
    let version_before = crate::cascade::common::read_version(&conn, &plan_id).await?;
    let version_after = version_before + 1;
    travel_db::repo::plan_lifecycle::set_active_destination(&conn, &plan_id, &slug, &now_db).await?;
    crate::cascade::common::record_operation(
        &conn,
        &plan_id,
        "set-active-destination",
        &format!("{plan_id} -> {slug}"),
        version_before,
        version_after,
        &now_db,
    )
    .await?;
    println!("\n✅ Active destination for {plan_id} → {slug}");
    Ok(())
}

fn parse(args: &[String]) -> Result<String, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            f if crate::plan_resolver::is_resolver_flag(f) => {
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            other => {
                positional.push(other.to_string());
                i += 1;
            }
        }
    }
    match positional.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(
            "Usage: travel set-active-destination <slug> [--plan-id <id>]\n  \
             Switch plan_metadata.active_destination to one of the plan's destinations."
                .to_string(),
        ),
        _ => Err(format!(
            "set-active-destination expects exactly one slug positional; got {}",
            positional.len()
        )),
    }
}