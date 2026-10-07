//! `travel set-companion-route` — structured stops for one 同行方案 option
//! (unconfirmed day-trip alternative), powering the dashboard companion map:
//! both options' routes drawn in option colors on ONE ArcGIS static frame.
//!
//! Deliberately NOT day_route_segments / day_landmarks — those are the
//! CONFIRMED itinerary and an unconfirmed option must never write itinerary
//! data. Stops with unresolvable coords are kept with NULL lat/lon: they
//! render as chips in the option's card and simply don't get a pin (台2線海岸
//! is a road, not a point — NULL is normal, not an error).
//!
//! Coords resolve at write time, in order: literal "lat,lon" token → this
//! plan's map_legend_stops (exact label; legend stops are already snapshot-
//! verified) → route_place_geocodes cache → Nominatim with the destination's
//! country context (road_leg::resolve_token; writes the cache back). An
//! area-centroid hit is used but flagged ⚠ — same policy as snapshot-maps.
//!
//! Audited: one timeline `plan_events` row (`companion_route_set` /
//! `companion_route_cleared`) + KV, `operation_runs`, `plans.version` bump.

use crate::cascade::common::{
    insert_event, insert_kv_rows, next_timeline_sort_order, now_db_datetime, now_rfc3339,
    read_version, record_operation,
};
use crate::snapshot_maps::ResolvedPlace;

const USAGE: &str = "Usage:\n  travel set-companion-route <A|B> --title <title> --stops \"stop 1, stop 2, ...\" [--dest <slug>]\n  travel set-companion-route <A|B> --clear\n  (stop tokens: place label, literal \"lat,lon\", or label already in map_legend_stops / the geocode cache; unresolvable stops are stored with NULL coords and render chip-only)";

#[derive(Debug)]
struct Parsed {
    option_key: char,
    title: Option<String>,
    stops: Option<Vec<String>>,
    clear: bool,
    dest: Option<String>,
}

fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut p = Parsed {
        option_key: '\0',
        title: None,
        stops: None,
        clear: false,
        dest: None,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--title" => {
                i += 1;
                p.title = Some(args.get(i).ok_or("--title needs a value")?.clone());
            }
            "--stops" => {
                i += 1;
                let raw = args.get(i).ok_or("--stops needs a value")?;
                let stops: Vec<String> = raw
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if stops.is_empty() {
                    return Err("set-companion-route: --stops is empty".into());
                }
                p.stops = Some(stops);
            }
            "--clear" => p.clear = true,
            "--dest" => {
                i += 1;
                p.dest = Some(args.get(i).ok_or("--dest needs a value")?.clone());
            }
            // main.rs already resolved the plan via plan_resolver; tolerate the flag.
            "--plan-id" => {
                i += 1;
            }
            other if p.option_key == '\0' && other.len() == 1 => {
                let c = other.chars().next().unwrap();
                if !c.is_ascii_uppercase() {
                    return Err(format!("set-companion-route: option key must be A-Z, got '{c}'"));
                }
                p.option_key = c;
            }
            other => return Err(format!("set-companion-route: unknown arg '{other}'\n{USAGE}")),
        }
        i += 1;
    }
    if p.option_key == '\0' {
        return Err(format!("set-companion-route: option key (A|B|...) required\n{USAGE}"));
    }
    if p.clear {
        if p.title.is_some() || p.stops.is_some() {
            return Err("set-companion-route: --clear takes no --title/--stops".into());
        }
    } else if p.title.is_none() || p.stops.is_none() {
        return Err(format!("set-companion-route: --title and --stops required\n{USAGE}"));
    }
    Ok(p)
}

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse(args)?;
    let conn = crate::db::connect_write().await?;

    conn.execute("BEGIN", libsql::params![])
        .await
        .map_err(|e| format!("set-companion-route BEGIN failed: {e}"))?;

    let outcome = write_route(&conn, &plan_id, &parsed).await;
    match outcome {
        Ok(lines) => {
            conn.execute("COMMIT", libsql::params![])
                .await
                .map_err(|e| format!("set-companion-route COMMIT failed: {e}"))?;
            for line in lines {
                println!("{line}");
            }
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", libsql::params![]).await;
            Err(e)
        }
    }
}

async fn write_route(
    conn: &libsql::Connection,
    plan_id: &str,
    p: &Parsed,
) -> Result<Vec<String>, String> {
    let now_db = now_db_datetime();
    let now_iso = now_rfc3339();
    let version_before = read_version(conn, plan_id).await?;
    let version_after = version_before + 1;
    let option = p.option_key.to_string();

    if p.clear {
        conn.execute(
            "DELETE FROM companion_option_stops WHERE plan_id = ?1 AND option_key = ?2",
            libsql::params![plan_id, option.as_str()],
        )
        .await
        .map_err(|e| format!("clear stops failed: {e}"))?;
        conn.execute(
            "DELETE FROM companion_options WHERE plan_id = ?1 AND option_key = ?2",
            libsql::params![plan_id, option.as_str()],
        )
        .await
        .map_err(|e| format!("clear option failed: {e}"))?;
        emit_audit(
            conn,
            plan_id,
            "companion_route_cleared",
            &[("option", option.clone())],
            &format!("set-companion-route clear {option}"),
            version_before,
            version_after,
            &now_db,
            &now_iso,
        )
        .await?;
        return Ok(vec![format!("✅ Cleared companion option {option} {plan_id}")]);
    }

    let title = p.title.clone().unwrap_or_default();
    let stops = p.stops.clone().unwrap_or_default();
    let dest = p.dest.clone();

    // Resolve coords per stop. Order: legend → cache → Nominatim (resolve_token).
    // A legend hit is snapshot-verified for THIS plan, so it outranks the cache.
    let mut geocodes = crate::snapshot_maps::load_geocodes(conn).await?;
    let mut rows: Vec<(i64, String, Option<f64>, Option<f64>)> = Vec::new();
    let mut lines = Vec::new();
    for (idx, label) in stops.iter().enumerate() {
        let (lat, lon, note) = resolve_stop(conn, plan_id, label, &dest, &mut geocodes).await?;
        if let Some(n) = &note {
            lines.push(format!("⚠ {label}: {n}"));
        }
        rows.push((idx as i64, label.clone(), lat, lon));
    }

    conn.execute(
        "DELETE FROM companion_option_stops WHERE plan_id = ?1 AND option_key = ?2",
        libsql::params![plan_id, option.as_str()],
    )
    .await
    .map_err(|e| format!("replace stops failed: {e}"))?;
    conn.execute(
        "DELETE FROM companion_options WHERE plan_id = ?1 AND option_key = ?2",
        libsql::params![plan_id, option.as_str()],
    )
    .await
    .map_err(|e| format!("replace option failed: {e}"))?;

    conn.execute(
        "INSERT INTO companion_options (plan_id, option_key, title, updated_at) VALUES (?1, ?2, ?3, ?4)",
        libsql::params![plan_id, option.as_str(), title.as_str(), now_db.as_str()],
    )
    .await
    .map_err(|e| format!("insert option failed: {e}"))?;
    for (seq, label, lat, lon) in &rows {
        conn.execute(
            "INSERT INTO companion_option_stops (plan_id, option_key, seq, label, lat, lon) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            libsql::params![plan_id, option.as_str(), *seq, label.as_str(), *lat, *lon],
        )
        .await
        .map_err(|e| format!("insert stop failed: {e}"))?;
    }

    let geocoded = rows.iter().filter(|(_, _, la, _)| la.is_some()).count();
    let stop_list = rows
        .iter()
        .map(|(_, l, la, _)| match la {
            Some(_) => l.clone(),
            None => format!("{l}(no-coord)"),
        })
        .collect::<Vec<_>>()
        .join(" → ");
    emit_audit(
        conn,
        plan_id,
        "companion_route_set",
        &[
            ("option", option.clone()),
            ("title", title.clone()),
            ("stops", stop_list.clone()),
            ("geocoded", format!("{geocoded}/{}", rows.len())),
        ],
        &format!("set-companion-route {option} {title}: {stop_list}"),
        version_before,
        version_after,
        &now_db,
        &now_iso,
    )
    .await?;

    lines.insert(
        0,
        format!(
            "✅ Companion option {option}「{title}」 {plan_id}: {} stops, {geocoded} geocoded",
            rows.len()
        ),
    );
    Ok(lines)
}

/// Legend first (snapshot-verified for this plan), then road_leg's chain
/// (cache → Nominatim with country context). Returns (lat, lon, warning).
async fn resolve_stop(
    conn: &libsql::Connection,
    plan_id: &str,
    label: &str,
    dest: &Option<String>,
    geocodes: &mut std::collections::HashMap<String, (f64, f64)>,
) -> Result<(Option<f64>, Option<f64>, Option<String>), String> {
    // Literal "lat,lon" — explicit beats every cache.
    if let Some((la, lo)) = parse_latlon(label) {
        return Ok((Some(la), Some(lo), None));
    }
    let mut legend = conn
        .query(
            "SELECT lat, lon FROM map_legend_stops WHERE plan_id = ?1 AND label = ?2 LIMIT 1",
            libsql::params![plan_id.to_string(), label.to_string()],
        )
        .await
        .map_err(|e| format!("legend lookup failed: {e}"))?;
    if let Some(row) = legend.next().await.map_err(|e| e.to_string())? {
        let la: Option<f64> = row.get(0).map_err(|e| e.to_string())?;
        let lo: Option<f64> = row.get(1).map_err(|e| e.to_string())?;
        if let (Some(la), Some(lo)) = (la, lo) {
            return Ok((Some(la), Some(lo), None));
        }
    }

    // road_leg::resolve_token: cache → Nominatim (needs --dest for context).
    // No --dest: legend + cache only — Nominatim needs country context, and
    // guessing it is how pins land in the wrong country.
    let Some(dest_slug) = dest.as_deref() else {
        return Ok((
            None,
            None,
            Some("no coords (no --dest: Nominatim skipped; pass --dest or set-place-geocode)".into()),
        ));
    };
    match crate::road_leg::resolve_token(label, conn, conn, Some(dest_slug), &[], geocodes).await
    {
        Ok(ResolvedPlace {
            coords: (la, lo),
            area_centroid,
            ..
        }) => {
            let note = if area_centroid {
                Some("resolved to an area centroid — verify, or pin with set-place-geocode".into())
            } else {
                None
            };
            Ok((Some(la), Some(lo), note))
        }
        Err(e) => Ok((None, None, Some(format!("no coords ({e})")))),
    }
}

fn parse_latlon(tok: &str) -> Option<(f64, f64)> {
    let (a, b) = tok.trim().split_once(',')?;
    let la = a.trim().parse::<f64>().ok()?;
    let lo = b.trim().parse::<f64>().ok()?;
    if (-90.0..=90.0).contains(&la) && (-180.0..=180.0).contains(&lo) {
        Some((la, lo))
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
async fn emit_audit(
    conn: &libsql::Connection,
    plan_id: &str,
    event: &str,
    kv: &[(&str, String)],
    summary: &str,
    version_before: i64,
    version_after: i64,
    now_db: &str,
    now_iso: &str,
) -> Result<(), String> {
    let sort_order = next_timeline_sort_order(conn, plan_id).await?;
    insert_event(
        conn, plan_id, "timeline", "", "", sort_order, event, now_iso, None, None,
    )
    .await?;
    insert_kv_rows(conn, plan_id, "timeline", "", "", sort_order, kv).await?;
    record_operation(
        conn,
        plan_id,
        "set-companion-route",
        summary,
        version_before,
        version_after,
        now_db,
    )
    .await?;
    Ok(())
}
