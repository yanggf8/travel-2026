//! `travel set-plan-name <name> [--dest <slug>]` — rename plan_destinations.display_name.

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let (name, dest_opt) = parse(args)?;
    let conn = crate::db::connect_write().await?;
    let slugs = travel_db::repo::plan_lifecycle::list_destination_slugs(&conn, &plan_id).await?;
    let slug = match (&dest_opt, slugs.as_slice()) {
        (Some(d), _) => {
            if !slugs.iter().any(|s| s == d) {
                return Err(format!("destination {d} is not a destination of plan {plan_id}"));
            }
            d.clone()
        }
        (None, [one]) => one.clone(),
        (None, []) => return Err(format!("plan {plan_id} has no destinations (not found?)")),
        (None, _many) => {
            return Err(format!(
                "plan {plan_id} has multiple destinations; pass --dest <slug>. available: {}",
                slugs.join(", ")
            ));
        }
    };
    let now_db = crate::cascade::common::now_db_datetime();
    let version_before = crate::cascade::common::read_version(&conn, &plan_id).await?;
    let version_after = version_before + 1;
    let affected =
        travel_db::repo::plan_lifecycle::set_display_name(&conn, &plan_id, &slug, &name, &now_db)
            .await?;
    if affected != 1 {
        return Err(format!(
            "set-plan-name updated {affected} rows (expected 1) for plan {plan_id} slug {slug}"
        ));
    }
    crate::cascade::common::record_operation(
        &conn,
        &plan_id,
        "set-plan-name",
        &format!("{plan_id} {slug} -> {name}"),
        version_before,
        version_after,
        &now_db,
    )
    .await?;
    println!("\n✅ Renamed plan {plan_id} → \"{name}\"");
    Ok(())
}

fn parse(args: &[String]) -> Result<(String, Option<String>), String> {
    let mut name_parts: Vec<String> = Vec::new();
    let mut dest_opt: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dest" => {
                dest_opt = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --dest".to_string())?
                        .clone(),
                );
                i += 2;
            }
            f if crate::plan_resolver::is_resolver_flag(f) => {
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            other => {
                name_parts.push(other.to_string());
                i += 1;
            }
        }
    }
    let name = name_parts.join(" ");
    if name.is_empty() {
        return Err(
            "Usage: travel set-plan-name <name> [--dest <slug>] [--plan-id <id>]\n  \
             Rename a plan's display label (plan_destinations.display_name)."
                .to_string(),
        );
    }
    Ok((name, dest_opt))
}