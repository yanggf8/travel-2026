//! `travel query-recommendations` — read-only listing of itinerary content rows
//! with `source='ai_recommended'`, scoped by optional day/session/kind filters.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Activity,
    Meal,
    Route,
}

impl Kind {
    #[allow(dead_code)]
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

    let conn = crate::db::connect_read().await?;
    let destination = crate::cascade::common::resolve_active_destination(
        &conn,
        &plan_id,
        parsed.dest.as_deref(),
    )
    .await?;

    let activities = if matches!(parsed.kind, None | Some(Kind::Activity)) {
        travel_db::repo::itinerary::list_recommended_activities(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            parsed.session.as_deref(),
        )
        .await?
    } else {
        Vec::new()
    };
    let meals = if matches!(parsed.kind, None | Some(Kind::Meal)) {
        travel_db::repo::itinerary::list_recommended_meals(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
            parsed.session.as_deref(),
        )
        .await?
    } else {
        Vec::new()
    };
    let routes = if matches!(parsed.kind, None | Some(Kind::Route)) {
        travel_db::repo::route_segments::list_recommended_routes(
            &conn,
            &plan_id,
            &destination,
            parsed.day,
        )
        .await?
    } else {
        Vec::new()
    };

    print_recommendations(&activities, &meals, &routes);
    Ok(())
}

fn dash(s: &str) -> String {
    if s.is_empty() {
        "-".to_string()
    } else {
        s.to_string()
    }
}

fn print_recommendations(
    activities: &[travel_db::repo::itinerary::RecommendedActivity],
    meals: &[travel_db::repo::itinerary::RecommendedMeal],
    routes: &[travel_db::repo::route_segments::RecommendedRoute],
) {
    let total = activities.len() + meals.len() + routes.len();
    if total == 0 {
        println!("\nNo AI-recommended items awaiting confirmation.");
        return;
    }

    println!(
        "\n{total} AI-recommended item(s) awaiting confirmation ({} activities, {} meals, {} routes)",
        activities.len(),
        meals.len(),
        routes.len()
    );

    let bar = "─".repeat(80);

    if !activities.is_empty() {
        println!("\nActivities ({}):", activities.len());
        println!("{bar}");
        for r in activities {
            let scope = format!("Day {} {}", r.day_number, r.session_type);
            let title: String = r.title.chars().take(42).collect();
            let poi = dash(r.poi_id.as_deref().unwrap_or(""));
            let poi_trunc: String = poi.chars().take(16).collect();
            let line = [
                format!("{scope:<12}"),
                format!("{:>3}", r.sort_order),
                format!("{title:<42}"),
                format!("{poi_trunc:<16}"),
            ]
            .join(" │ ");
            println!("{line}");
        }
        println!("{bar}");
    }

    if !meals.is_empty() {
        println!("\nMeals ({}):", meals.len());
        println!("{bar}");
        for r in meals {
            let scope = format!("Day {} {}", r.day_number, r.session_type);
            let meal: String = r.meal.chars().take(50).collect();
            let line = [
                format!("{scope:<12}"),
                format!("{:>3}", r.sort_order),
                format!("{meal:<50}"),
            ]
            .join(" │ ");
            println!("{line}");
        }
        println!("{bar}");
    }

    if !routes.is_empty() {
        println!("\nRoutes ({}):", routes.len());
        println!("{bar}");
        for r in routes {
            let scope = format!("Day {}", r.day_number);
            let route_text = format!("{} -> {}", r.from_place, r.to_place);
            let route: String = route_text.chars().take(42).collect();
            let mode: String = r.mode.chars().take(12).collect();
            let line = [
                format!("{scope:<12}"),
                format!("{:>3}", r.sort_order),
                format!("{route:<42}"),
                format!("{mode:<12}"),
            ]
            .join(" │ ");
            println!("{line}");
        }
        println!("{bar}");
    }
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