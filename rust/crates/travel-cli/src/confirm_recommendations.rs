//! `travel confirm-recommendations` — batch-flip itinerary content rows from
//! `ai_recommended` → `confirmed`, scoped by optional day/session/kind filters.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Activity,
    Meal,
    Route,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Activity => "activity",
            Kind::Meal => "meal",
            Kind::Route => "route",
        }
    }
}

#[derive(Default, Debug)]
struct Parsed {
    dest: Option<String>,
    day: Option<i64>,
    session: Option<String>,
    kind: Option<Kind>,
}

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse(args)?;
    if parsed.kind == Some(Kind::Route) && parsed.session.is_some() {
        return Err("--session cannot be used with --kind route".to_string());
    }

    let conn = crate::db::connect_write().await?;
    let destination = crate::cascade::common::resolve_active_destination(
        &conn,
        &plan_id,
        parsed.dest.as_deref(),
    )
    .await?;
    let version_before = crate::cascade::common::read_version(&conn, &plan_id).await?;
    let version_after = version_before + 1;
    let now_db = crate::cascade::common::now_db_datetime();
    let now_iso = crate::cascade::common::now_rfc3339();

    let mut activities = 0u64;
    let mut meals = 0u64;
    let mut routes = 0u64;
    if matches!(parsed.kind, None | Some(Kind::Activity)) {
        activities = travel_db::repo::itinerary::confirm_activities(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            parsed.session.as_deref(),
            &now_db,
        )
        .await?;
    }
    if matches!(parsed.kind, None | Some(Kind::Meal)) {
        meals = travel_db::repo::itinerary::confirm_meals(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            parsed.session.as_deref(),
            &now_db,
        )
        .await?;
    }
    if matches!(parsed.kind, None | Some(Kind::Route)) {
        routes = travel_db::repo::route_segments::confirm_routes(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
        )
        .await?;
    }
    let total = activities + meals + routes;
    if total == 0 {
        println!("0 confirmed");
        return Ok(());
    }

    let mut kv: Vec<(&str, String)> = vec![
        ("count", total.to_string()),
        ("activities", activities.to_string()),
        ("meals", meals.to_string()),
        ("routes", routes.to_string()),
    ];
    if let Some(day) = parsed.day {
        kv.push(("day_number", day.to_string()));
    }
    if let Some(session) = &parsed.session {
        kv.push(("session", session.clone()));
    }
    if let Some(kind) = parsed.kind {
        kv.push(("kind", kind.as_str().to_string()));
    }

    crate::cascade::common::emit_event_both(
        &conn,
        &plan_id,
        &destination,
        "process_5_daily_itinerary",
        "recommendations_confirmed",
        &now_iso,
        None,
        None,
        &kv,
    )
    .await?;
    let summary = format!(
        "{total} confirmed ({activities} activities, {meals} meals, {routes} routes)"
    );
    crate::cascade::common::record_operation(
        &conn,
        &plan_id,
        "confirm-recommendations",
        &summary,
        version_before,
        version_after,
        &now_db,
    )
    .await?;

    println!("{total} confirmed");
    println!("activities\t{activities}");
    println!("meals\t{meals}");
    println!("routes\t{routes}");
    Ok(())
}

fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut p = Parsed::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                p.dest = Some(
                    args.get(i + 1)
                        .ok_or_else(|| "missing value for --dest".to_string())?
                        .clone(),
                );
                i += 2;
            }
            "--day" => {
                let raw = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --day".to_string())?;
                let day: i64 = raw
                    .parse()
                    .map_err(|_| "--day must be a positive integer".to_string())?;
                if day < 1 {
                    return Err("--day must be a positive integer".to_string());
                }
                p.day = Some(day);
                i += 2;
            }
            "--session" => {
                let s = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --session".to_string())?;
                if !["morning", "noon", "afternoon", "evening"].contains(&s.as_str()) {
                    return Err(
                        "--session must be one of: morning|noon|afternoon|evening".to_string(),
                    );
                }
                p.session = Some(s.clone());
                i += 2;
            }
            "--kind" => {
                let k = args
                    .get(i + 1)
                    .ok_or_else(|| "missing value for --kind".to_string())?;
                p.kind = Some(match k.as_str() {
                    "activity" => Kind::Activity,
                    "meal" => Kind::Meal,
                    "route" => Kind::Route,
                    other => return Err(format!("unknown kind: {other}")),
                });
                i += 2;
            }
            "--plan-id" => {
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            other => {
                return Err(format!("unexpected positional argument: {other}"));
            }
        }
    }
    Ok(p)
}