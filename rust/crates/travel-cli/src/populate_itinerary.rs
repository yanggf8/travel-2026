// `travel populate-itinerary --goals "<c1,c2,...>" [--pace relaxed|balanced|packed]
//  [--assign "<cluster:day,...>"] [--dest slug] [--force]` —
// port of src/cli/commands/populate-itinerary.ts + the StateManager
// methods it drives (setDayTheme / setSessionFocus / addActivity).
//
// Incremental: adds activities from destination clusters onto the existing
// day skeleton. Does NOT overwrite days. Mirrors the TS allocation + pacing:
//   - allocateClustersToDays: clusters → day_numbers (full days round-robin,
//     last_day* clusters → departure day, explicit --assign overrides).
//   - pace → session count (relaxed=1, balanced=2, packed=3) over the
//     day-type session order (arrival: afternoon/evening; departure:
//     morning/noon; full: all four).
//   - chunkEvenly: POIs spread round-robin across the used sessions.
//   - theme/focus set only when empty (or --force); activities deduped by
//     title (case-insensitive) unless --force.
//
// Each added activity → one `activities` row (+ `activity_tags` child rows).
// One `activity_added` plan_event per add (matching addActivity), plus a
// `set_day_theme` / `set_session_focus` event per theme/focus write — all
// folded into a single version bump + audit row at the end.

use libsql::Connection;
use std::collections::BTreeMap;

#[derive(Default, Debug)]
struct ParsedArgs {
    goals: Vec<String>,
    pace: String,
    assign: Option<String>,
    dest: Option<String>,
    force: bool,
}

struct Day {
    day_number: i64,
    day_type: String,
}

struct Poi {
    title: String,
    area: String, // area_id
    nearest_station: Option<String>,
    duration_min: Option<i64>,
    booking_required: bool,
    booking_url: Option<String>,
    cost_estimate: Option<i64>,
    notes: Option<String>,
    hours: Option<String>,
    address: Option<String>,
    tags: Vec<String>,
}

struct Cluster {
    name: String,
    pois: Vec<String>, // poi_ids in order
}

struct PlannedAdd {
    day: i64,
    session: &'static str,
    title: String,
}

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_args(args)?;

    let conn = crate::db::connect_write().await?;
    let destination = read_destination(&conn, &parsed.dest, &plan_id, &conn).await?;

    // Existing day skeleton (required).
    let days = read_days(&conn, &plan_id, &destination).await?;
    if days.is_empty() {
        return Err("No itinerary days found. Run scaffold-itinerary first.".to_string());
    }

    // Load destination reference (clusters / pois / areas) from Turso.
    let clusters = load_clusters(&conn, &destination).await?;
    let pois = load_pois(&conn, &destination).await?;
    let area_names = load_area_names(&conn, &destination).await?;

    // Allocation: clusters → day numbers.
    let explicit = parse_assignments(parsed.assign.as_deref());
    let allocation = allocate_clusters_to_days(&parsed.goals, &days, &explicit);

    // pace → session count.
    let session_count = match parsed.pace.as_str() {
        "relaxed" => 1,
        "balanced" => 2,
        _ => 3, // packed
    };

    let day_type_by_num: BTreeMap<i64, String> =
        days.iter().map(|d| (d.day_number, d.day_type.clone())).collect();

    let mut planned: Vec<PlannedAdd> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    // Track theme/focus event count for audit summary only.
    let mut theme_focus_events = 0i64;
    let mut activity_events = 0i64;

    // Defer the version bump/audit to the end; events accumulate as we go.
    let mut pending_events: Vec<(String, Vec<(String, String)>)> = Vec::new();

    for item in &allocation {
        let cluster = match clusters.get(&item.cluster_id) {
            Some(c) => c,
            None => {
                skipped.push(format!("Unknown cluster: {}", item.cluster_id));
                continue;
            }
        };
        if cluster.pois.is_empty() {
            skipped.push(format!("Cluster has no POIs: {}", item.cluster_id));
            continue;
        }
        let day_type = match day_type_by_num.get(&item.day_number) {
            Some(t) => t.as_str(),
            None => continue,
        };

        let session_order = session_order_for_day_type(day_type);
        let used: Vec<&'static str> = session_order
            .iter()
            .take(session_count.min(session_order.len()))
            .copied()
            .collect();
        if used.is_empty() {
            continue;
        }
        let per_session = chunk_evenly(&cluster.pois, used.len());

        // Theme: set if empty (or force).
        let current_theme = read_day_theme(&conn, &plan_id, &destination, item.day_number).await?;
        if parsed.force || current_theme.as_deref().unwrap_or("").is_empty() {
            set_day_theme(&conn, &plan_id, &destination, item.day_number, &cluster.name).await?;
            pending_events.push((
                "itinerary_day_theme_set".to_string(),
                vec![
                    ("day_number".to_string(), item.day_number.to_string()),
                    ("theme".to_string(), cluster.name.clone()),
                    ("theme_zh".to_string(), "null".to_string()),
                ],
            ));
            theme_focus_events += 1;
        }

        // Focus per used session: set if empty (or force).
        for sess in &used {
            let current_focus =
                read_session_focus(&conn, &plan_id, &destination, item.day_number, sess).await?;
            if parsed.force || current_focus.as_deref().unwrap_or("").is_empty() {
                set_session_focus(&conn, &plan_id, &destination, item.day_number, sess, &cluster.name)
                    .await?;
                pending_events.push((
                    "itinerary_session_focus_set".to_string(),
                    vec![
                        ("day_number".to_string(), item.day_number.to_string()),
                        ("session".to_string(), (*sess).to_string()),
                        ("focus".to_string(), cluster.name.clone()),
                    ],
                ));
                theme_focus_events += 1;
            }
        }

        // Activities: spread POIs across used sessions.
        for (i, sess) in used.iter().enumerate() {
            for poi_id in &per_session[i] {
                let poi = match pois.get(poi_id) {
                    Some(p) => p,
                    None => {
                        skipped.push(format!("Missing POI in ref: {poi_id}"));
                        continue;
                    }
                };
                let title = poi.title.clone();

                // Dedup by case-insensitive title within (day, session).
                let dup = session_has_title(
                    &conn,
                    &plan_id,
                    &destination,
                    item.day_number,
                    sess,
                    &title,
                )
                .await?;
                if dup && !parsed.force {
                    continue;
                }

                planned.push(PlannedAdd {
                    day: item.day_number,
                    session: sess,
                    title: title.clone(),
                });

                // Resolve area name (fall back to area_id).
                let area_name = area_names
                    .get(&poi.area)
                    .cloned()
                    .unwrap_or_else(|| poi.area.clone());
                let notes = build_notes(poi);
                let priority = if poi.booking_required { "must" } else { "want" };

                let activity_id = add_activity(
                    &conn,
                    &plan_id,
                    &destination,
                    item.day_number,
                    sess,
                    &title,
                    &area_name,
                    poi.nearest_station.as_deref(),
                    poi.duration_min,
                    poi.booking_required,
                    poi.booking_url.as_deref(),
                    poi.cost_estimate,
                    notes.as_deref(),
                    priority,
                    &poi.tags,
                )
                .await?;

                pending_events.push((
                    "activity_added".to_string(),
                    vec![
                        ("day_number".to_string(), item.day_number.to_string()),
                        ("session".to_string(), (*sess).to_string()),
                        ("activity_id".to_string(), activity_id),
                        ("title".to_string(), title),
                    ],
                ));
                activity_events += 1;
            }
        }
    }

    // touchItinerary on each affected day already done via add/theme/focus
    // writes (those set updated_at on the day row); ensure all touched days
    // get a final updated_at bump for parity with touchItinerary.
    let mut touched: Vec<i64> = allocation.iter().map(|a| a.day_number).collect();
    touched.sort_unstable();
    touched.dedup();
    for d in &touched {
        touch_day(&conn, &plan_id, &destination, *d).await?;
    }

    // Console output (mirror TS).
    println!("\n🧩 populate-itinerary ({destination})");
    println!("   Pace: {}", parsed.pace);
    println!("   Goals: {}", parsed.goals.join(", "));
    if let Some(a) = &parsed.assign {
        println!("   Assign: {a}");
    }
    println!("   Ref: turso:destination-ref/{destination}");

    if !skipped.is_empty() {
        println!("\nSkipped:");
        for s in skipped.iter().take(10) {
            println!("  - {s}");
        }
        if skipped.len() > 10 {
            println!("  ... and {} more", skipped.len() - 10);
        }
    }

    println!("\nPlanned additions: {}", planned.len());
    for a in planned.iter().take(20) {
        println!("  - Day {} {}: {}", a.day, a.session, a.title);
    }
    if planned.len() > 20 {
        println!("  ... and {} more", planned.len() - 20);
    }

    // Emit all accumulated events under one version bump + audit row.
    let _ = (theme_focus_events, activity_events);
    flush_events(
        &conn,
        &plan_id,
        &destination,
        "populate-itinerary",
        &format!("{destination} {} activities", planned.len()),
        &pending_events,
    )
    .await?;

    println!("\n✅ Itinerary populated (incremental adds)");
    println!("\nNext action: run status --full, then adjust with updateActivity/removeActivity as needed");
    Ok(())
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut p = ParsedArgs::default();
    let mut goals_opt: Option<String> = None;
    let mut pace_opt: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--goals" => {
                goals_opt = Some(arg_value(args, i, "--goals")?);
                i += 2;
            }
            "--pace" => {
                pace_opt = Some(arg_value(args, i, "--pace")?);
                i += 2;
            }
            "--assign" => {
                p.assign = Some(arg_value(args, i, "--assign")?);
                i += 2;
            }
            "--dest" => {
                p.dest = Some(arg_value(args, i, "--dest")?);
                i += 2;
            }
            "--force" => {
                p.force = true;
                i += 1;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let pace = pace_opt.unwrap_or_else(|| "balanced".to_string()).to_lowercase();
    if !["relaxed", "balanced", "packed"].contains(&pace.as_str()) {
        return Err("--pace must be one of: relaxed | balanced | packed".to_string());
    }
    p.pace = pace;

    let goals_raw =
        goals_opt.ok_or_else(|| "populate-itinerary requires --goals \"<cluster1,cluster2,...>\"".to_string())?;
    let goals: Vec<String> = goals_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if goals.is_empty() {
        return Err("--goals had no usable cluster IDs".to_string());
    }
    p.goals = goals;
    Ok(p)
}

fn arg_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("missing value for {flag}"))
}

// ─────────────────────────────────────────────────────────────────
// Allocation / pacing (port of itinerary-helpers.ts)
// ─────────────────────────────────────────────────────────────────

struct Allocation {
    cluster_id: String,
    day_number: i64,
}

fn allocate_clusters_to_days(
    cluster_ids: &[String],
    days: &[Day],
    explicit: &BTreeMap<String, i64>,
) -> Vec<Allocation> {
    let full_days: Vec<i64> = days
        .iter()
        .filter(|d| d.day_type == "full")
        .map(|d| d.day_number)
        .collect();
    let arrival_day = days.iter().find(|d| d.day_type == "arrival").map(|d| d.day_number);
    let departure_day = days.iter().find(|d| d.day_type == "departure").map(|d| d.day_number);
    let day_numbers: std::collections::BTreeSet<i64> = days.iter().map(|d| d.day_number).collect();

    let assigned_days: std::collections::BTreeSet<i64> = explicit.values().copied().collect();
    let available_full: Vec<i64> = full_days
        .iter()
        .copied()
        .filter(|d| !assigned_days.contains(d))
        .collect();

    let mut result = Vec::new();
    let mut full_idx = 0usize;
    for id in cluster_ids {
        let mut target: Option<i64> = None;
        if let Some(e) = explicit.get(id) {
            if day_numbers.contains(e) {
                target = Some(*e);
            }
        }
        if id.contains("last_day") {
            if let Some(dep) = departure_day {
                target = Some(dep);
            }
        }
        if target.is_none() {
            if !available_full.is_empty() {
                target = Some(available_full[full_idx % available_full.len()]);
            }
            full_idx += 1;
        }
        if target.is_none() {
            target = arrival_day;
        }
        if target.is_none() {
            target = departure_day;
        }
        let day_number = target.unwrap_or(1);
        result.push(Allocation {
            cluster_id: id.clone(),
            day_number,
        });
    }
    result
}

fn parse_assignments(assign_opt: Option<&str>) -> BTreeMap<String, i64> {
    let mut map = BTreeMap::new();
    let s = match assign_opt {
        Some(s) => s,
        None => return map,
    };
    for part in s.split(',') {
        let mut it = part.splitn(2, ':');
        let cluster = it.next().map(|x| x.trim()).unwrap_or("");
        let day_str = it.next().map(|x| x.trim()).unwrap_or("");
        if cluster.is_empty() || day_str.is_empty() {
            continue;
        }
        if let Ok(day) = day_str.parse::<i64>() {
            if day > 0 {
                map.insert(cluster.to_string(), day);
            }
        }
    }
    map
}

fn session_order_for_day_type(day_type: &str) -> Vec<&'static str> {
    match day_type {
        "arrival" => vec!["afternoon", "evening"],
        "departure" => vec!["morning", "noon"],
        _ => vec!["morning", "noon", "afternoon", "evening"],
    }
}

fn chunk_evenly(items: &[String], buckets: usize) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = (0..buckets).map(|_| Vec::new()).collect();
    if buckets == 0 {
        return out;
    }
    for (i, it) in items.iter().enumerate() {
        out[i % buckets].push(it.clone());
    }
    out
}

fn build_notes(poi: &Poi) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(n) = &poi.notes {
        if !n.is_empty() {
            parts.push(n.clone());
        }
    }
    if let Some(h) = &poi.hours {
        if !h.is_empty() {
            parts.push(format!("Hours: {h}"));
        }
    }
    if let Some(a) = &poi.address {
        if !a.is_empty() {
            parts.push(format!("Address: {a}"));
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" | "))
    }
}

// ─────────────────────────────────────────────────────────────────
// Reference loaders
// ─────────────────────────────────────────────────────────────────

async fn load_clusters(
    conn: &Connection,
    destination: &str,
) -> Result<BTreeMap<String, Cluster>, String> {
    // cluster_pois child rows (ordered).
    let mut pois_by_cluster: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut rows = conn
        .query(
            "SELECT cluster_id, poi_id FROM destination_cluster_pois \
             WHERE slug = ?1 ORDER BY cluster_id, sort_order",
            libsql::params![destination.to_string()],
        )
        .await
        .map_err(|e| format!("destination_cluster_pois query failed: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("cluster_pois row read failed: {e}"))?
    {
        let cid: String = r.get(0).unwrap_or_default();
        let pid: String = r.get(1).unwrap_or_default();
        pois_by_cluster.entry(cid).or_default().push(pid);
    }

    let mut clusters: BTreeMap<String, Cluster> = BTreeMap::new();
    let mut rows = conn
        .query(
            "SELECT cluster_id, name FROM destination_clusters WHERE slug = ?1",
            libsql::params![destination.to_string()],
        )
        .await
        .map_err(|e| format!("destination_clusters query failed: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("clusters row read failed: {e}"))?
    {
        let id: String = r.get(0).unwrap_or_default();
        let name: String = r.get(1).unwrap_or_default();
        let pois = pois_by_cluster.get(&id).cloned().unwrap_or_default();
        clusters.insert(id, Cluster { name, pois });
    }
    Ok(clusters)
}

async fn load_pois(conn: &Connection, destination: &str) -> Result<BTreeMap<String, Poi>, String> {
    // POI tags child rows.
    let mut tags_by_poi: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut rows = conn
        .query(
            "SELECT poi_id, tag FROM destination_poi_tags WHERE slug = ?1 ORDER BY poi_id, sort_order",
            libsql::params![destination.to_string()],
        )
        .await
        .map_err(|e| format!("destination_poi_tags query failed: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("poi_tags row read failed: {e}"))?
    {
        let pid: String = r.get(0).unwrap_or_default();
        let tag: String = r.get(1).unwrap_or_default();
        tags_by_poi.entry(pid).or_default().push(tag);
    }

    let mut pois: BTreeMap<String, Poi> = BTreeMap::new();
    let mut rows = conn
        .query(
            "SELECT poi_id, title, area, nearest_station, duration_min, booking_required, \
                    booking_url, cost_estimate, notes, hours, address \
             FROM destination_pois WHERE slug = ?1",
            libsql::params![destination.to_string()],
        )
        .await
        .map_err(|e| format!("destination_pois query failed: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("pois row read failed: {e}"))?
    {
        let id: String = r.get(0).unwrap_or_default();
        let tags = tags_by_poi.get(&id).cloned().unwrap_or_default();
        pois.insert(
            id,
            Poi {
                title: r.get(1).unwrap_or_default(),
                area: r.get(2).unwrap_or_default(),
                nearest_station: opt_text(&r, 3),
                duration_min: r.get(4).ok(),
                booking_required: r.get::<i64>(5).unwrap_or(0) == 1,
                booking_url: opt_text(&r, 6),
                cost_estimate: r.get(7).ok(),
                notes: opt_text(&r, 8),
                hours: opt_text(&r, 9),
                address: opt_text(&r, 10),
                tags,
            },
        );
    }
    Ok(pois)
}

async fn load_area_names(
    conn: &Connection,
    destination: &str,
) -> Result<BTreeMap<String, String>, String> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    let mut rows = conn
        .query(
            "SELECT area_id, name FROM destination_areas WHERE slug = ?1",
            libsql::params![destination.to_string()],
        )
        .await
        .map_err(|e| format!("destination_areas query failed: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("areas row read failed: {e}"))?
    {
        let id: String = r.get(0).unwrap_or_default();
        let name: String = r.get(1).unwrap_or_default();
        map.insert(id, name);
    }
    Ok(map)
}

async fn read_days(conn: &Connection, plan_id: &str, destination: &str) -> Result<Vec<Day>, String> {
    let mut out = Vec::new();
    let mut rows = conn
        .query(
            "SELECT day_number, day_type FROM days \
             WHERE plan_id = ?1 AND destination = ?2 ORDER BY day_number",
            libsql::params![plan_id.to_string(), destination.to_string()],
        )
        .await
        .map_err(|e| format!("days query failed: {e}"))?;
    while let Some(r) = rows
        .next()
        .await
        .map_err(|e| format!("days row read failed: {e}"))?
    {
        out.push(Day {
            day_number: r.get(0).unwrap_or(0),
            day_type: r.get(1).unwrap_or_default(),
        });
    }
    Ok(out)
}

fn opt_text(row: &libsql::Row, idx: i32) -> Option<String> {
    match row.get_value(idx).ok()? {
        libsql::Value::Text(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────
// Mutations
// ─────────────────────────────────────────────────────────────────

async fn read_day_theme(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT theme FROM days WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3",
            libsql::params![plan_id.to_string(), destination.to_string(), day],
        )
        .await
        .map_err(|e| format!("days theme query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("days theme row read failed: {e}"))?
    {
        return Ok(opt_text(&row, 0));
    }
    Ok(None)
}

async fn set_day_theme(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    theme: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET theme = ?1, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5",
        libsql::params![
            theme.to_string(),
            now_db_datetime(),
            plan_id.to_string(),
            destination.to_string(),
            day
        ],
    )
    .await
    .map_err(|e| format!("days theme UPDATE failed: {e}"))?;
    Ok(())
}

async fn read_session_focus(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
) -> Result<Option<String>, String> {
    let mut rows = conn
        .query(
            "SELECT focus FROM timesofday \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
            libsql::params![plan_id.to_string(), destination.to_string(), day, session.to_string()],
        )
        .await
        .map_err(|e| format!("timesofday focus query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("timesofday focus row read failed: {e}"))?
    {
        return Ok(opt_text(&row, 0));
    }
    Ok(None)
}

async fn set_session_focus(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    focus: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE timesofday SET focus = ?1, updated_at = ?2 \
         WHERE plan_id = ?3 AND destination = ?4 AND day_number = ?5 AND session_type = ?6",
        libsql::params![
            focus.to_string(),
            now_db_datetime(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string()
        ],
    )
    .await
    .map_err(|e| format!("timesofday focus UPDATE failed: {e}"))?;
    Ok(())
}

async fn session_has_title(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    title: &str,
) -> Result<bool, String> {
    let needle = title.to_lowercase();
    let mut rows = conn
        .query(
            "SELECT title FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
            libsql::params![plan_id.to_string(), destination.to_string(), day, session.to_string()],
        )
        .await
        .map_err(|e| format!("activities dedup query failed: {e}"))?;
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities dedup row read failed: {e}"))?
    {
        let t: String = row.get(0).unwrap_or_default();
        if t.to_lowercase() == needle {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn next_sort_order(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) FROM activities \
             WHERE plan_id = ?1 AND destination = ?2 AND day_number = ?3 AND session_type = ?4",
            libsql::params![plan_id.to_string(), destination.to_string(), day, session.to_string()],
        )
        .await
        .map_err(|e| format!("activities MAX(sort_order) query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("activities MAX(sort_order) row read failed: {e}"))?
    {
        let m: i64 = row.get(0).unwrap_or(-1);
        return Ok(m + 1);
    }
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
async fn add_activity(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
    session: &str,
    title: &str,
    area: &str,
    nearest_station: Option<&str>,
    duration_min: Option<i64>,
    booking_required: bool,
    booking_url: Option<&str>,
    cost_estimate: Option<i64>,
    notes: Option<&str>,
    priority: &str,
    tags: &[String],
) -> Result<String, String> {
    let id = new_activity_id();
    let sort_order = next_sort_order(conn, plan_id, destination, day, session).await?;
    conn.execute(
        "INSERT INTO activities \
            (id, plan_id, destination, day_number, session_type, sort_order, title, area, \
             nearest_station, duration_min, booking_required, booking_url, cost_estimate, \
             notes, priority, is_fixed_time, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 0, ?16)",
        libsql::params![
            id.clone(),
            plan_id.to_string(),
            destination.to_string(),
            day,
            session.to_string(),
            sort_order,
            title.to_string(),
            area.to_string(),
            nearest_station.map(|s| s.to_string()),
            duration_min,
            if booking_required { 1i64 } else { 0i64 },
            booking_url.map(|s| s.to_string()),
            cost_estimate,
            notes.map(|s| s.to_string()),
            priority.to_string(),
            now_db_datetime()
        ],
    )
    .await
    .map_err(|e| format!("activities INSERT failed: {e}"))?;

    for tag in tags {
        conn.execute(
            "INSERT OR IGNORE INTO activity_tags (activity_id, tag) VALUES (?1, ?2)",
            libsql::params![id.clone(), tag.clone()],
        )
        .await
        .map_err(|e| format!("activity_tags INSERT failed: {e}"))?;
    }
    Ok(id)
}

fn new_activity_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("activity_{}_{:04x}", base36(nanos as u64), (n & 0xFFFF) as u16)
}

fn base36(mut v: u64) -> String {
    if v == 0 {
        return "0".to_string();
    }
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while v > 0 {
        out.push(DIGITS[(v % 36) as usize]);
        v /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

async fn touch_day(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    day: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE days SET updated_at = ?1 WHERE plan_id = ?2 AND destination = ?3 AND day_number = ?4",
        libsql::params![now_db_datetime(), plan_id.to_string(), destination.to_string(), day],
    )
    .await
    .map_err(|e| format!("days touch UPDATE failed: {e}"))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// Event flush (single version bump + audit row for all events)
// ─────────────────────────────────────────────────────────────────

async fn flush_events(
    conn: &Connection,
    plan_id: &str,
    destination: &str,
    command_type: &str,
    command_summary: &str,
    events: &[(String, Vec<(String, String)>)],
) -> Result<(), String> {
    let now_iso = now_rfc3339();
    let now_db = now_db_datetime();
    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;

    let mut dest_so =
        next_dest_process_sort_order(conn, plan_id, destination, "process_5_daily_itinerary").await?;
    let mut timeline_so = next_timeline_sort_order(conn, plan_id).await?;

    for (event_name, kv) in events {
        let kv_ref: Vec<(&str, String)> =
            kv.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        insert_event(
            conn,
            plan_id,
            "dest_process",
            destination,
            "process_5_daily_itinerary",
            dest_so,
            event_name,
            &now_iso,
        )
        .await?;
        insert_kv(
            conn,
            plan_id,
            "dest_process",
            destination,
            "process_5_daily_itinerary",
            dest_so,
            &kv_ref,
        )
        .await?;
        insert_event(
            conn,
            plan_id,
            "timeline",
            "",
            "process_5_daily_itinerary",
            timeline_so,
            event_name,
            &now_iso,
        )
        .await?;
        insert_kv(
            conn,
            plan_id,
            "timeline",
            "",
            "process_5_daily_itinerary",
            timeline_so,
            &kv_ref,
        )
        .await?;
        dest_so += 1;
        timeline_so += 1;
    }

    let run_id = new_run_id();
    conn.execute(
        "INSERT INTO operation_runs \
            (run_id, plan_id, command_type, command_summary, status, \
             version_before, version_after, started_at, completed_at) \
         VALUES (?1, ?2, ?3, ?4, 'completed', ?5, ?6, ?7, ?7)",
        libsql::params![
            run_id,
            plan_id.to_string(),
            command_type.to_string(),
            command_summary.to_string(),
            version_before,
            version_after,
            now_db.clone()
        ],
    )
    .await
    .map_err(|e| format!("operation_runs INSERT failed: {e}"))?;
    conn.execute(
        "UPDATE plans SET version = ?1, updated_at = ?2 WHERE plan_id = ?3",
        libsql::params![version_after, now_db, plan_id.to_string()],
    )
    .await
    .map_err(|e| format!("plans UPDATE failed: {e}"))?;
    Ok(())
}

// read_destination wrapper (keeps the same shape as other modules but allows
// the conn to be reused for the metadata read).
async fn read_destination(
    _conn0: &Connection,
    dest_opt: &Option<String>,
    plan_id: &str,
    conn: &Connection,
) -> Result<String, String> {
    if let Some(d) = dest_opt {
        return Ok(d.clone());
    }
    let mut rows = conn
        .query(
            "SELECT active_destination FROM plan_metadata WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_metadata query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_metadata row read failed: {e}"))?
    {
        let dest: String = row
            .get(0)
            .map_err(|e| format!("active_destination col read failed: {e}"))?;
        if dest.is_empty() {
            return Err("plan_metadata.active_destination is empty".to_string());
        }
        return Ok(dest);
    }
    Err(format!("plan_metadata row missing for plan_id={plan_id}"))
}

async fn read_version(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT version FROM plans WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plans query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plans row read failed: {e}"))?
    {
        let v: i64 = row
            .get(0)
            .map_err(|e| format!("version col read failed: {e}"))?;
        return Ok(v);
    }
    Err(format!("plans row missing for plan_id={plan_id}"))
}

async fn next_dest_process_sort_order(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'dest_process' \
               AND destination = ?2 AND process_id = ?3",
            libsql::params![plan_id.to_string(), dest.to_string(), process_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_events MAX query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_events MAX row read failed: {e}"))?
    {
        let m: i64 = row
            .get(0)
            .map_err(|e| format!("plan_events MAX col read failed: {e}"))?;
        return Ok(m + 1);
    }
    Ok(0)
}

async fn next_timeline_sort_order(conn: &Connection, plan_id: &str) -> Result<i64, String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(sort_order), -1) AS m FROM plan_events \
             WHERE plan_id = ?1 AND scope = 'timeline'",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_events MAX(timeline) query failed: {e}"))?;
    if let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_events MAX(timeline) row read failed: {e}"))?
    {
        let m: i64 = row
            .get(0)
            .map_err(|e| format!("plan_events MAX(timeline) col read failed: {e}"))?;
        return Ok(m + 1);
    }
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
async fn insert_event(
    conn: &Connection,
    plan_id: &str,
    scope: &str,
    destination: &str,
    process_id: &str,
    sort_order: i64,
    event: &str,
    event_at: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_events \
         WHERE plan_id = ?1 AND scope = ?2 AND destination = ?3 \
           AND process_id = ?4 AND sort_order = ?5",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order],
    )
    .await
    .map_err(|e| format!("plan_events DELETE failed: {e}"))?;
    conn.execute(
        "INSERT INTO plan_events \
            (plan_id, scope, destination, process_id, sort_order, \
             event, event_at, from_state, to_state) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL)",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order, event.to_string(), event_at.to_string()],
    )
    .await
    .map_err(|e| format!("plan_events INSERT failed: {e}"))?;
    Ok(())
}

async fn insert_kv(
    conn: &Connection,
    plan_id: &str,
    scope: &str,
    destination: &str,
    process_id: &str,
    sort_order: i64,
    kv: &[(&str, String)],
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM plan_event_data \
         WHERE plan_id = ?1 AND scope = ?2 AND destination = ?3 \
           AND process_id = ?4 AND sort_order = ?5",
        libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order],
    )
    .await
    .map_err(|e| format!("plan_event_data DELETE failed: {e}"))?;
    for (k, v) in kv {
        conn.execute(
            "INSERT INTO plan_event_data \
                (plan_id, scope, destination, process_id, sort_order, key, value) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            libsql::params![plan_id.to_string(), scope.to_string(), destination.to_string(), process_id.to_string(), sort_order, k.to_string(), v.clone()],
        )
        .await
        .map_err(|e| format!("plan_event_data INSERT failed: {e}"))?;
    }
    Ok(())
}

fn new_run_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        ^ (n as u128);
    let p1 = (nanos & 0xFFFF_FFFF) as u32;
    let p2 = ((nanos >> 32) & 0xFFFF) as u16;
    let p3 = ((nanos >> 48) & 0x0FFF) as u16;
    let p4 = 0x8000 | (((nanos >> 60) & 0x3FFF) as u16);
    let p5 = (nanos as u64) ^ 0xDEAD_BEEF_CAFE_F00D;
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}", p1, p2, p3, p4, p5)
}

fn now_rfc3339() -> String {
    let (year, month, day, hour, min, sec, ms) = now_civil();
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{ms:03}Z")
}

fn now_db_datetime() -> String {
    let (year, month, day, hour, min, sec, _) = now_civil();
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}")
}

fn now_civil() -> (i32, u32, u32, u32, u32, u32, u32) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let ms = d.subsec_millis();
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400) as u32;
    let hour = sod / 3600;
    let min = (sod % 3600) / 60;
    let sec = sod % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d_ = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d_ as u32, hour, min, sec, ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(n: i64, t: &str) -> Day {
        Day { day_number: n, day_type: t.to_string() }
    }

    #[test]
    fn session_order_by_type() {
        assert_eq!(session_order_for_day_type("arrival"), vec!["afternoon", "evening"]);
        assert_eq!(session_order_for_day_type("departure"), vec!["morning", "noon"]);
        assert_eq!(
            session_order_for_day_type("full"),
            vec!["morning", "noon", "afternoon", "evening"]
        );
    }

    #[test]
    fn chunk_round_robin() {
        let items: Vec<String> = vec!["a", "b", "c", "d", "e"].into_iter().map(String::from).collect();
        let out = chunk_evenly(&items, 2);
        assert_eq!(out[0], vec!["a", "c", "e"]);
        assert_eq!(out[1], vec!["b", "d"]);
    }

    #[test]
    fn allocate_full_round_robin() {
        let days = vec![d(1, "arrival"), d(2, "full"), d(3, "full"), d(4, "departure")];
        let goals = vec!["c1".to_string(), "c2".to_string(), "c3".to_string()];
        let alloc = allocate_clusters_to_days(&goals, &days, &BTreeMap::new());
        assert_eq!(alloc[0].day_number, 2);
        assert_eq!(alloc[1].day_number, 3);
        assert_eq!(alloc[2].day_number, 2); // wraps
    }

    #[test]
    fn allocate_last_day_to_departure() {
        let days = vec![d(1, "arrival"), d(2, "full"), d(3, "departure")];
        let goals = vec!["temple_last_day".to_string()];
        let alloc = allocate_clusters_to_days(&goals, &days, &BTreeMap::new());
        assert_eq!(alloc[0].day_number, 3);
    }

    #[test]
    fn allocate_explicit_assignment() {
        let days = vec![d(1, "arrival"), d(2, "full"), d(3, "full"), d(4, "departure")];
        let mut explicit = BTreeMap::new();
        explicit.insert("c1".to_string(), 3i64);
        let goals = vec!["c1".to_string()];
        let alloc = allocate_clusters_to_days(&goals, &days, &explicit);
        assert_eq!(alloc[0].day_number, 3);
    }

    #[test]
    fn parse_assignments_basic() {
        let m = parse_assignments(Some("c1:2, c2:3, bad, c3:0"));
        assert_eq!(m.get("c1"), Some(&2));
        assert_eq!(m.get("c2"), Some(&3));
        assert_eq!(m.get("c3"), None); // day 0 rejected
    }

    #[test]
    fn parse_args_requires_goals() {
        assert!(parse_args(&["--pace".to_string(), "packed".to_string()]).is_err());
    }

    #[test]
    fn parse_args_rejects_bad_pace() {
        assert!(parse_args(&[
            "--goals".to_string(),
            "c1".to_string(),
            "--pace".to_string(),
            "wild".to_string()
        ])
        .is_err());
    }

    #[test]
    fn parse_args_ok() {
        let p = parse_args(&[
            "--goals".to_string(),
            "c1, c2".to_string(),
            "--pace".to_string(),
            "RELAXED".to_string(),
            "--force".to_string(),
        ])
        .unwrap();
        assert_eq!(p.goals, vec!["c1", "c2"]);
        assert_eq!(p.pace, "relaxed");
        assert!(p.force);
    }
}
