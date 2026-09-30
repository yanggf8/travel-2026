//! `travel snapshot-maps` — query Turso, render route diagrams in Rust, and upload PNGs to R2.
//!
//! The renderer prefers destination POI coordinates over Nominatim, then uses itinerary
//! endpoints, cached Nominatim coordinates, and cached OSRM road geometry. The geographic
//! background is an ArcGIS Online static basemap (World Street Map, falling back to World
//! Topo Map) fetched via the MapServer `export` endpoint in Web Mercator, which matches the
//! renderer's own Mercator projection 1:1; when ArcGIS is unreachable it falls back to an
//! Overpass-derived road web. It never fetches OpenStreetMap raster tiles; automated/headless
//! tile downloads and saved tile composites are prohibited by the OpenStreetMap tile policy.
//! OSM-derived data is credited in every generated image, and Esri is credited whenever a
//! basemap is composited.

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::Cursor,
    path::PathBuf,
    process::Command,
    thread,
    time::Duration,
};

use libsql::{params, Connection};
use png::{BitDepth, ColorType, Encoder};
use sha2::{Digest, Sha256};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 440;
const MIN_PNG_BYTES: usize = 200;
const BUCKET: &str = "trip-dashboard-maps";
/// Overpass vector endpoints (vector data, NOT raster tiles — the OSM tile-policy
/// ban is on automated tile fetching/composites; the Overpass API is the sanctioned
/// vector path, same family as the OSRM/Nominatim calls this module already makes).
/// The public main instance rate-limits bursts, so a mirror is tried in the same
/// round before backing off.
const OVERPASS_URLS: [&str; 2] = [
    "https://overpass-api.de/api/interpreter",
    "https://overpass.kumi.systems/api/interpreter",
];
/// ArcGIS Online static basemap export endpoints (one PNG per request, NOT raster tiles —
/// the OSM tile-policy ban is on automated fetching of tile.openstreetmap.org; these are
/// Esri-hosted services rendered to an exact Web Mercator bbox). Tried in order: street map
/// first (closest to the old road-web look), topo map as fallback.
const ARCGIS_BASEMAPS: [&str; 2] = [
    "https://server.arcgisonline.com/ArcGIS/rest/services/World_Street_Map/MapServer/export",
    "https://server.arcgisonline.com/ArcGIS/rest/services/World_Topo_Map/MapServer/export",
];
/// Web Mercator sphere radius (EPSG:3857), converting the renderer's unitless Mercator
/// coordinates into the metres the ArcGIS export endpoint expects.
const WEB_MERCATOR_R: f64 = 6_378_137.0;
/// How far beyond the stop/route bounds the road background reaches, as a fraction
/// of the bounds' span, so the frame edges are not bare.
const ROAD_PAD_FRAC: f64 = 0.20;
const USER_AGENT: &str =
    "travel-2026-route-geocoder/1.1 (+https://trip-dashboard-rs.yanggf.workers.dev/)";
const DAY_COLORS: [[u8; 3]; 7] = [
    [230, 25, 75],
    [60, 180, 75],
    [67, 99, 216],
    [245, 130, 49],
    [145, 99, 196],
    [0, 128, 128],
    [154, 99, 36],
];

#[derive(Clone, Debug)]
struct Point {
    lat: f64,
    lon: f64,
    color: [u8; 3],
    kind: Kind,
    /// Legend name for this pin. Rendered by the dashboard worker from
    /// `map_legend_stops` (the PNG bitmap font has no CJK glyphs, so the
    /// number→name mapping cannot be drawn into the image itself).
    label: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Sightseeing,
    Hotel,
    Airport,
}

#[derive(Debug)]
struct Segment {
    day: i64,
    from: String,
    to: String,
    mode: String,
    duration_min: Option<i64>,
}

/// Straight-line km a leg of `mode` could plausibly cover in `duration_min`, plus
/// slack. A pair of pins farther apart than this means one end is geocoded to the
/// wrong place (智恩寺 → Kyoto's 百万遍知恩寺, 80 km off, on a "15 min driving"
/// leg). `None` when the leg carries no duration to check against.
fn plausible_leg_km(mode: &str, duration_min: Option<i64>) -> Option<f64> {
    let minutes = duration_min.filter(|m| *m > 0)? as f64;
    let kmh = match mode {
        "walking" => 8.0,
        "driving" => 100.0,
        _ => 300.0, // transit: allow shinkansen
    };
    Some(kmh * minutes / 60.0 + 3.0)
}

#[derive(Clone)]
struct RouteLine {
    points: Vec<(f64, f64)>,
    routed: bool,
    color: [u8; 3],
}

#[derive(Clone, Debug)]
pub(crate) struct PoiRow {
    pub(crate) poi_id: String,
    pub(crate) title: String,
    pub(crate) lat: f64,
    pub(crate) lon: f64,
}

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_usage();
        return Ok(());
    }
    let dest_override = parse_dest(args)?;
    let read = crate::db::connect_read().await?;
    let dest = crate::cascade::common::resolve_active_destination(
        &read,
        &plan_id,
        dest_override.as_deref(),
    )
    .await?;
    let write = crate::db::connect_write().await?;
    let context = geocode_context(&read, &dest).await?;
    let days = query_days(&read, &plan_id).await?;
    if days.is_empty() {
        return Err(format!("no itinerary days for plan {plan_id}"));
    }

    // Far-off day trips (set-day-excursion) stay off the sightseeing overview and
    // get their own inset map — a 100 km coach tour otherwise flattens the city.
    let excursion: HashSet<i64> = travel_db::repo::itinerary::excursion_days(&read, &plan_id, &dest)
        .await?
        .into_iter()
        .collect();
    let mut cache = load_geocodes(&read).await?;
    let segments = query_segments(&read, &plan_id, &dest).await?;
    let pois = query_pois(&read, &plan_id, &dest).await?;
    let dest_pois = query_destination_pois(&read, &dest).await?;
    let hotel_names = query_hotel_names(&read, &plan_id, &dest).await?;
    let mut day_points = HashMap::<i64, Vec<Point>>::new();
    // OSRM legs fetched this run, shared by the per-day and overview renders so
    // the same pair is fetched (and persisted) at most once per snapshot run.
    let mut osrm_live = HashMap::<String, Vec<(f64, f64)>>::new();

    for day in &days {
        let mut points = Vec::new();
        let mut seen = HashSet::new();
        let mut unresolved = HashSet::new();
        for seg in segments.iter().filter(|s| s.day == *day) {
            let mut ends: [Option<(f64, f64)>; 2] = [None, None];
            for (end, label) in [&seg.from, &seg.to].into_iter().enumerate() {
                if label.trim().is_empty() {
                    continue;
                }
                let resolved = resolve_segment_label(
                    &read,
                    &write,
                    label,
                    &context,
                    &mut cache,
                    &dest_pois,
                    &hotel_names,
                )
                .await?;
                if resolved.is_none() && unresolved.insert(label.clone()) {
                    eprintln!(
                        "   ⚠ day {day}: \"{label}\" could not be geocoded — it is left off the map; \
                         rename the stop or pin it with set-place-geocode"
                    );
                }
                if let Some((lat, lon, kind)) = resolved {
                    ends[end] = Some((lat, lon));
                    if seen.insert(coord_key(lat, lon)) {
                        points.push(Point {
                            lat,
                            lon,
                            color: DAY_COLORS[(*day as usize - 1) % DAY_COLORS.len()],
                            kind,
                            label: display_label(label, &hotel_names),
                        });
                    }
                }
            }
            if let ([Some(a), Some(b)], Some(max_km)) =
                (ends, plausible_leg_km(&seg.mode, seg.duration_min))
            {
                let km = haversine_m(a, b) / 1000.0;
                if km > max_km {
                    eprintln!(
                        "   ⚠ day {day}: \"{}\" → \"{}\" geocodes {km:.1} km apart, but the leg says {} min {} \
                         — one end is probably pinned to the wrong place; check route_place_geocodes \
                         and fix it with set-place-geocode",
                        seg.from,
                        seg.to,
                        seg.duration_min.unwrap_or_default(),
                        seg.mode
                    );
                }
            }
        }
        if points.is_empty() {
            for p in pois.iter().filter(|p| p.0 == *day) {
                if seen.insert(coord_key(p.2, p.3)) {
                    points.push(Point {
                        lat: p.2,
                        lon: p.3,
                        color: DAY_COLORS[(*day as usize - 1) % DAY_COLORS.len()],
                        kind: Kind::Sightseeing,
                        label: p.1.clone(),
                    });
                }
            }
        }
        day_points.insert(*day, points);
    }

    let mut required_ok = true;
    let mut all_sightseeing = Vec::new();
    let mut excursion_points: Vec<Point> = Vec::new();
    let mut daily_logistics = HashMap::<i64, (Vec<Point>, Vec<RouteLine>)>::new();
    for day in &days {
        let all_pts = day_points.get(day).cloned().unwrap_or_default();
        let local_pts = all_pts.iter().filter(|p| p.kind == Kind::Sightseeing).cloned().collect::<Vec<_>>();
        if local_pts.is_empty() {
            record_artifact(
                &write,
                &plan_id,
                &format!("day-{day}.png"),
                0,
                None,
                "skipped",
                Some("no mappable stops"),
                None,
                None,
            )
            .await?;
            println!("   skipped day-{day}.png (no mappable stops)");
        } else {
            let routes = cached_routes(&read, &write, &mut osrm_live, &local_pts).await?;
            if !upload_map(
                &write,
                &plan_id,
                &format!("day-{day}.png"),
                &format!("Day {day} route"),
                &local_pts,
                &routes,
                MapKind::Day,
            )
            .await?
            {
                required_ok = false;
            }
        }
        // Keep each day's local map zoomed to its sightseeing area. A separate
        // compact map shows the hotel, first stop, last stop, and both commute legs.
        let hotels = all_pts.iter().filter(|p| p.kind == Kind::Hotel).cloned().collect::<Vec<_>>();
        let sights = &local_pts;
        if let (Some(hotel), Some(first), Some(last)) = (hotels.first(), sights.first(), sights.last()) {
            let mut points = vec![hotel.clone()];
            if !same_place(&points[0], first) { points.push(first.clone()); }
            if !same_place(points.last().unwrap(), last) { points.push(last.clone()); }
            let mut routes = cached_routes(&read, &write, &mut osrm_live, &[hotel.clone(), first.clone()]).await?;
            if !same_place(first, last) {
                routes.extend(cached_routes(&read, &write, &mut osrm_live, &[last.clone(), hotel.clone()]).await?);
            }
            daily_logistics.insert(*day, (points, routes));
        }
        let pts = all_pts;
        let bucket = if excursion.contains(day) {
            &mut excursion_points
        } else {
            &mut all_sightseeing
        };
        for p in pts.into_iter().filter(|p| p.kind == Kind::Sightseeing) {
            if !bucket
                .iter()
                .any(|existing: &Point| existing.lat == p.lat && existing.lon == p.lon)
            {
                bucket.push(p);
            }
        }
    }

    for day in &days {
        let key = format!("day-{day}-logistics.png");
        if let Some((points, routes)) = daily_logistics.get(day) {
            if !upload_map(&write, &plan_id, &key, &format!("Day {day} hotel round trip"), points, routes, MapKind::DayLogistics).await? {
                required_ok = false;
            }
        } else {
            record_artifact(&write, &plan_id, &key, 0, None, "skipped", Some("no geocoded hotel and sightseeing endpoints"), None, None).await?;
            println!("   skipped {key} (no geocoded hotel and sightseeing endpoints)");
        }
    }

    // Logistics endpoints are computed BEFORE plan.png so a DOMESTIC trip (no
    // airport segment — every endpoint is the stay) can merge the hotel pin into
    // the overview map instead of emitting a second full-size map: for a
    // self-drive domestic stay the hotel IS an itinerary place, and the second
    // map slot only wasted vertical space (user request 2026-09-28). Plans WITH
    // airport endpoints (Japan) keep the split — distant endpoints must not
    // flatten the sightseeing overview's zoom level.
    let logistics = await_logistics(
        &read,
        &write,
        &plan_id,
        &dest,
        &context,
        &mut cache,
        &dest_pois,
        &hotel_names,
    )
    .await?;
    for day in days.iter().filter(|d| !excursion.contains(d)) {
        if let Some(points) = day_points.get(day)
            && let Some((stop, km)) = distant_excursion_stop(points, &logistics)
        {
            eprintln!(
                "   ⚠ 建議：Day {day} 景點「{}」距飯店直線約 {km:.1} 公里（超過 40 公里），可標記一日遊地圖：travel set-day-excursion {day} on --plan-id {plan_id} --dest {dest}",
                stop.label
            );
        }
    }
    let has_airport = logistics.iter().any(|p| p.kind == Kind::Airport);
    let mut plan_points = all_sightseeing.clone();
    if !has_airport {
        for p in logistics.iter().filter(|p| p.kind == Kind::Hotel) {
            if !plan_points
                .iter()
                .any(|existing: &Point| existing.lat == p.lat && existing.lon == p.lon)
            {
                plan_points.push(p.clone());
            }
        }
    }

    if plan_points.is_empty() {
        record_artifact(
            &write,
            &plan_id,
            "plan.png",
            0,
            None,
            "skipped",
            Some("no sightseeing points"),
            None,
            None,
        )
        .await?;
        println!("   skipped plan.png (no sightseeing points)");
    } else {
        let mut routes = Vec::new();
        for day in days.iter().filter(|d| !excursion.contains(d)) {
            let points = day_points
                .get(day)
                .into_iter()
                .flatten()
                .filter(|p| p.kind == Kind::Sightseeing)
                .cloned()
                .collect::<Vec<_>>();
            if points.len() > 1 {
                routes.extend(cached_routes(&read, &write, &mut osrm_live, &points).await?);
            }
        }
        if !upload_map(
            &write,
            &plan_id,
            "plan.png",
            "Sightseeing overview",
            &plan_points,
            &routes,
            MapKind::Plan,
        )
        .await?
        {
            required_ok = false;
        }
    }

    if excursion_points.is_empty() {
        remove_stale_map(&write, &plan_id, "plan-excursion.png", "no excursion day").await?;
    } else {
        let mut routes = Vec::new();
        for day in days.iter().filter(|d| excursion.contains(d)) {
            let points = day_points
                .get(day)
                .into_iter()
                .flatten()
                .filter(|p| p.kind == Kind::Sightseeing)
                .cloned()
                .collect::<Vec<_>>();
            if points.len() > 1 {
                routes.extend(cached_routes(&read, &write, &mut osrm_live, &points).await?);
            }
        }
        if !upload_map(
            &write,
            &plan_id,
            "plan-excursion.png",
            "Day trip",
            &excursion_points,
            &routes,
            MapKind::Plan,
        )
        .await?
        {
            required_ok = false;
        }
    }

    if logistics.is_empty() {
        record_artifact(
            &write,
            &plan_id,
            "plan-logistics.png",
            0,
            None,
            "skipped",
            Some("no geocoded hotel/airport endpoints"),
            None,
            None,
        )
        .await?;
        println!("   skipped plan-logistics.png (no geocoded hotel/airport endpoints)");
    } else if !has_airport {
        // Domestic: the stay is already a pin on plan.png — no separate logistics map.
        record_artifact(
            &write,
            &plan_id,
            "plan-logistics.png",
            0,
            None,
            "skipped",
            Some("domestic trip: no airport endpoints; stay merged into plan.png"),
            None,
            None,
        )
        .await?;
        println!("   skipped plan-logistics.png (domestic: stay merged into plan.png, no airport endpoints)");
    } else if !upload_map(
        &write,
        &plan_id,
        "plan-logistics.png",
        "Hotels and airports",
        &logistics,
        &[],
        MapKind::Logistics,
    )
    .await?
    {
        required_ok = false;
    }

    if required_ok {
        crate::db::connect_write().await?.execute(
            "INSERT INTO plan_map_snapshots (plan_id, snapshotted_at) VALUES (?1, datetime('now')) ON CONFLICT(plan_id) DO UPDATE SET snapshotted_at=datetime('now')",
            params![plan_id.clone()],
        ).await.map_err(|e| format!("map freshness stamp failed: {e}"))?;
        println!("snapshot-maps: completed {plan_id} ({dest}); no OSM raster tiles requested");
        Ok(())
    } else {
        Err(format!(
            "one or more map uploads failed for {plan_id}; freshness was not stamped"
        ))
    }
}

#[derive(Clone, Copy)]
enum MapKind {
    Day,
    Plan,
    Logistics,
    DayLogistics,
}

async fn upload_map(
    conn: &Connection,
    plan: &str,
    key: &str,
    title: &str,
    points: &[Point],
    routes: &[RouteLine],
    kind: MapKind,
) -> Result<bool, String> {
    let input_sha = map_input_sha256(title, kind, points, routes);
    let projection = map_projection(points, routes);
    // Geographic background: ArcGIS static basemap first; only when ArcGIS is
    // unreachable fall back to the Overpass road web (slower, rate-limited, and
    // much sparser than a real basemap).
    let basemap = fetch_basemap(&projection).await;
    let roads = if basemap.is_some() {
        Vec::new()
    } else {
        fetch_road_web(padded_bounds(points, routes)).await
    };
    let has_background = basemap.is_some() || !roads.is_empty();
    if !has_background && previous_map_reusable(conn, plan, key, &input_sha).await? {
        println!(
            "   kept {key} (geographic background unavailable; previous map has same stops + background)"
        );
        // Same PNG re-served, but labels may have changed (input_sha deliberately
        // excludes them) — refresh the legend to the current point names.
        write_legend_stops(conn, plan, key, points).await?;
        return Ok(true);
    }
    // A grid with pins is not a map.  It is especially misleading for the
    // long-distance koyo routes, where a failed Overpass request used to be
    // uploaded as a successful (mostly white) PNG.  Leave the previous good
    // artifact in place when possible; otherwise record a failed artifact so
    // the dashboard shows its missing-map state and freshness is not stamped.
    if !has_background {
        record_artifact(
            conn,
            plan,
            key,
            0,
            None,
            "failed",
            Some("no geographic background (ArcGIS basemap and Overpass road web both unavailable)"),
            Some(&input_sha),
            Some(0),
        )
        .await?;
        eprintln!("   FAIL {key}: no geographic background; map was not uploaded");
        return Ok(false);
    }
    let png = render_png(title, points, routes, &roads, basemap.as_deref(), kind)?;
    if png.len() < MIN_PNG_BYTES {
        return Err(format!(
            "rendered map {key} is undersized ({} bytes)",
            png.len()
        ));
    }
    let sha = format!("{:x}", Sha256::digest(&png));
    let dir = env::temp_dir().join(format!("travel-map-{plan}"));
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let file = dir.join(key);
    fs::write(&file, &png).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    let mut command = wrangler_command();
    let status = command
        .args([
            "wrangler",
            "r2",
            "object",
            "put",
            &format!("{BUCKET}/{plan}/{key}"),
            "--file",
        ])
        .arg(&file)
        .args(["--content-type", "image/png", "--remote"])
        .status()
        .map_err(|e| format!("failed to run Wrangler for {key}: {e}"))?;
    // has_roads = "has a geographic background" (ArcGIS basemap or Overpass road web);
    // by this point has_background is guaranteed true.
    let has_roads = 1;
    if !status.success() {
        record_artifact(
            conn,
            plan,
            key,
            png.len(),
            Some(&sha),
            "failed",
            Some("Wrangler upload failed"),
            Some(&input_sha),
            Some(has_roads),
        )
        .await?;
        eprintln!("   FAIL {key}: Wrangler exited with {status}");
        return Ok(false);
    }
    record_artifact(
        conn,
        plan,
        key,
        png.len(),
        Some(&sha),
        "uploaded",
        None,
        Some(&input_sha),
        Some(has_roads),
    )
    .await?;
    write_legend_stops(conn, plan, key, points).await?;
    println!("   uploaded {key} ({} bytes)", png.len());
    Ok(true)
}

/// Rewrite the numbered-pin legend rows for one map key. `seq` mirrors the pin
/// numbers `render_png` draws (`points.iter().enumerate()` + 1); the PNG and
/// this table are written from the same `points` slice, so they cannot drift.
/// Called ONLY on paths that (re)serve the current PNG (upload success and the
/// reusable-keep path): skip/fail paths leave the previous rows in place because
/// the dashboard still serves the previous PNG from R2, and its legend must keep
/// matching it.
async fn write_legend_stops(
    _conn: &Connection,
    plan: &str,
    key: &str,
    points: &[Point],
) -> Result<(), String> {
    // Reconnect after each Wrangler upload, which can outlive Turso's idle timeout.
    let conn = crate::db::connect_write().await?;
    conn.execute(
        "DELETE FROM map_legend_stops WHERE plan_id=?1 AND map_key=?2",
        params![plan.to_string(), key.to_string()],
    )
    .await
    .map_err(err("map legend delete"))?;
    for (i, p) in points.iter().enumerate() {
        conn.execute(
            "INSERT INTO map_legend_stops (plan_id, map_key, seq, label, lat, lon) VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                plan.to_string(),
                key.to_string(),
                (i + 1) as i64,
                p.label.clone(),
                p.lat,
                p.lon
            ],
        )
        .await
        .map_err(err("map legend insert"))?;
    }
    Ok(())
}

fn map_kind_tag(kind: MapKind) -> &'static str {
    match kind {
        MapKind::Day => "day",
        MapKind::Plan => "plan",
        MapKind::Logistics => "logistics",
        MapKind::DayLogistics => "day-logistics",
    }
}

fn kind_tag(kind: Kind) -> &'static str {
    match kind {
        Kind::Sightseeing => "sightseeing",
        Kind::Hotel => "hotel",
        Kind::Airport => "airport",
    }
}

fn map_input_sha256(
    title: &str,
    kind: MapKind,
    points: &[Point],
    routes: &[RouteLine],
) -> String {
    let mut hasher = Sha256::new();
    fn put_str(hasher: &mut Sha256, s: &str) {
        hasher.update((s.len() as u64).to_le_bytes());
        hasher.update(s.as_bytes());
    }
    put_str(&mut hasher, title);
    put_str(&mut hasher, map_kind_tag(kind));
    hasher.update((points.len() as u64).to_le_bytes());
    for p in points {
        hasher.update(p.lat.to_le_bytes());
        hasher.update(p.lon.to_le_bytes());
        put_str(&mut hasher, kind_tag(p.kind));
        hasher.update(&p.color);
    }
    hasher.update((routes.len() as u64).to_le_bytes());
    for r in routes {
        hasher.update((r.points.len() as u64).to_le_bytes());
        for (lat, lon) in &r.points {
            hasher.update(lat.to_le_bytes());
            hasher.update(lon.to_le_bytes());
        }
        hasher.update([u8::from(r.routed)]);
        hasher.update(&r.color);
    }
    format!("{:x}", hasher.finalize())
}

async fn previous_map_reusable(
    conn: &Connection,
    plan: &str,
    key: &str,
    input_sha: &str,
) -> Result<bool, String> {
    let mut r = conn
        .query(
            "SELECT status, has_roads, input_sha256 FROM map_artifacts WHERE plan_id=?1 AND map_key=?2",
            params![plan.to_string(), key.to_string()],
        )
        .await
        .map_err(err("map artifact lookup"))?;
    let Some(row) = r.next().await.map_err(err("map artifact lookup read"))? else {
        return Ok(false);
    };
    let status = row.get::<Option<String>>(0).ok().flatten();
    let has_roads = row.get::<Option<i64>>(1).ok().flatten();
    let prev_sha = row.get::<Option<String>>(2).ok().flatten();
    Ok(status.as_deref() == Some("uploaded")
        && has_roads == Some(1)
        && prev_sha.as_deref() == Some(input_sha))
}

fn render_png(
    title: &str,
    points: &[Point],
    routes: &[RouteLine],
    roads: &[RoadWay],
    basemap: Option<&[u8]>,
    kind: MapKind,
) -> Result<Vec<u8>, String> {
    if points.is_empty() {
        return Err("cannot render a map without points".into());
    }
    // A basemap that is not exactly one canvas of RGBA is not a basemap.
    let basemap = basemap.filter(|b| b.len() == (WIDTH * HEIGHT * 4) as usize);
    let mut raster = Raster::new(WIDTH as usize, HEIGHT as usize);
    // ArcGIS basemap first (under everything); without it, the old light grid.
    if let Some(pixels) = basemap {
        raster.blit(pixels);
    } else {
        raster.grid();
    }
    let projection = map_projection(points, routes);
    let xy = points
        .iter()
        .map(|p| mercator(p.lat, p.lon))
        .collect::<Vec<_>>();
    let route_xy = routes
        .iter()
        .map(|r| {
            r.points
                .iter()
                .map(|(lat, lon)| mercator(*lat, *lon))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    // Road web FIRST (under everything): a light OSM-derived street context drawn
    // from vector geometry so the corridor line no longer floats on a bare grid.
    // Off-canvas points are clipped by Raster::put.
    for road in roads {
        let color = if road.major {
            [186, 196, 208]
        } else {
            [222, 228, 234]
        };
        let width = if road.major { 2 } else { 1 };
        let pts: Vec<_> = road
            .points
            .iter()
            .map(|(lat, lon)| projection.point(mercator(*lat, *lon)))
            .collect();
        for pair in pts.windows(2) {
            raster.line(pair[0], pair[1], color, width, false);
        }
    }
    for (idx, line) in route_xy.iter().enumerate() {
        let color = routes[idx].color;
        for pair in line.windows(2) {
            let a = projection.point(pair[0]);
            let b = projection.point(pair[1]);
            raster.line(a, b, [255, 255, 255], 8, !routes[idx].routed);
            raster.line(a, b, color, 4, !routes[idx].routed);
        }
    }
    if routes.is_empty() && !matches!(kind, MapKind::Logistics | MapKind::DayLogistics) {
        for pair in xy.windows(2) {
            let a = projection.point(pair[0]);
            let b = projection.point(pair[1]);
            raster.line(a, b, [255, 255, 255], 8, true);
            raster.line(a, b, [80, 110, 140], 4, true);
        }
    }
    for (i, p) in points.iter().enumerate() {
        let (x, y) = projection.point(mercator(p.lat, p.lon));
        let color = match p.kind {
            Kind::Sightseeing => p.color,
            Kind::Hotel => [21, 101, 192],
            Kind::Airport => [239, 108, 0],
        };
        raster.pin(x, y, color, i + 1);
    }
    let title = title.to_ascii_uppercase();
    if basemap.is_some() {
        // Solid backing strips keep title and credits readable over the basemap.
        raster.fill_rect(8, 8, title.len() as i32 * 12 + 8, 20, [255, 255, 255]);
        raster.fill_rect(366, HEIGHT as i32 - 18, 218, 14, [255, 255, 255]);
    }
    raster.text(12, 10, &title, [45, 61, 75], 2);
    if matches!(kind, MapKind::Logistics) {
        raster.legend(12, 31, "HOTEL", [21, 101, 192]);
        raster.legend(105, 31, "AIRPORT", [239, 108, 0]);
    } else if matches!(kind, MapKind::DayLogistics) {
        raster.legend(12, 31, "HOTEL", [21, 101, 192]);
        raster.legend(105, 31, "DESTINATION", [230, 25, 75]);
    }
    let credit = if basemap.is_some() {
        "© ESRI + OPENSTREETMAP CONTRIBUTORS"
    } else {
        "© OPENSTREETMAP CONTRIBUTORS"
    };
    raster.text(370, HEIGHT as i32 - 14, credit, [70, 82, 94], 1);
    encode_rgba_png(&raster.pixels, WIDTH, HEIGHT)
}

fn encode_rgba_png(pixels: &[u8], w: u32, h: u32) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    let mut encoder = Encoder::new(&mut out, w, h);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("PNG header failed: {e}"))?;
    writer
        .write_image_data(pixels)
        .map_err(|e| format!("PNG encoding failed: {e}"))?;
    drop(writer);
    Ok(out.into_inner())
}

/// Mercator-space projection covering all stops and route geometry on the canvas.
/// Extracted so `upload_map` can fetch a basemap for exactly the rendered extent.
fn map_projection(points: &[Point], routes: &[RouteLine]) -> Projection {
    let mut bounds: Vec<(f64, f64)> = points.iter().map(|p| mercator(p.lat, p.lon)).collect();
    for r in routes {
        bounds.extend(r.points.iter().map(|(lat, lon)| mercator(*lat, *lon)));
    }
    Projection::fit(&bounds, WIDTH as f64, HEIGHT as f64)
}

/// Web Mercator (EPSG:3857) bbox in metres covering the FULL canvas under this projection,
/// so the returned ArcGIS export aligns 1:1 with canvas pixels. `Projection.min_x`/`max_y`
/// hold the Mercator-space centre (see Projection::fit).
fn basemap_bbox(p: &Projection) -> (f64, f64, f64, f64) {
    let half_w = WIDTH as f64 / 2.0 / p.scale;
    let half_h = HEIGHT as f64 / 2.0 / p.scale;
    (
        (p.min_x - half_w) * WEB_MERCATOR_R,
        (p.max_y - half_h) * WEB_MERCATOR_R,
        (p.min_x + half_w) * WEB_MERCATOR_R,
        (p.max_y + half_h) * WEB_MERCATOR_R,
    )
}

/// Fetch the ArcGIS static basemap covering the rendered canvas. Returns RGBA8 pixels
/// (WIDTH*HEIGHT*4) on success, None when every endpoint fails or returns a non-image
/// body (ArcGIS answers errors as JSON even on HTTP 200 — the PNG decode rejects those).
async fn fetch_basemap(p: &Projection) -> Option<Vec<u8>> {
    let (min_x, min_y, max_x, max_y) = basemap_bbox(p);
    for base in ARCGIS_BASEMAPS {
        let url = format!(
            "{base}?bbox={min_x},{min_y},{max_x},{max_y}&bboxSR=3857&imageSR=3857&size={WIDTH},{HEIGHT}&format=png32&transparent=false&f=image"
        );
        let output = match Command::new("curl")
            .args(["-sS", "--max-time", "30", "-H"])
            .arg(format!("User-Agent: {USER_AGENT}"))
            .arg(&url)
            .output()
        {
            Ok(o) => o,
            Err(e) => {
                eprintln!("   warn: ArcGIS basemap spawn failed ({e}); trying next basemap");
                continue;
            }
        };
        if !output.status.success() {
            eprintln!(
                "   warn: ArcGIS basemap at {base} exited {}; trying next basemap",
                output.status
            );
            continue;
        }
        match decode_rgba_png(&output.stdout) {
            Some(pixels) => return Some(pixels),
            None => {
                eprintln!("   warn: ArcGIS basemap at {base} returned a non-map body; trying next basemap");
            }
        }
    }
    eprintln!("   warn: no ArcGIS basemap available; falling back to the Overpass road web");
    None
}

/// Decode a PNG body into RGBA8 pixels of exactly WIDTH×HEIGHT. Pure — unit-tested.
fn decode_rgba_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.width != WIDTH || info.height != HEIGHT || info.bit_depth != BitDepth::Eight {
        return None;
    }
    let px = (WIDTH * HEIGHT) as usize;
    let buf = &buf[..info.buffer_size()];
    match info.color_type {
        ColorType::Rgba if buf.len() == px * 4 => Some(buf.to_vec()),
        ColorType::Rgb if buf.len() == px * 3 => {
            let mut out = vec![255u8; px * 4];
            for i in 0..px {
                out[i * 4..i * 4 + 3].copy_from_slice(&buf[i * 3..i * 3 + 3]);
            }
            Some(out)
        }
        ColorType::Grayscale if buf.len() == px => {
            let mut out = vec![255u8; px * 4];
            for i in 0..px {
                out[i * 4..i * 4 + 3].copy_from_slice(&[buf[i]; 3]);
            }
            Some(out)
        }
        ColorType::GrayscaleAlpha if buf.len() == px * 2 => {
            let mut out = vec![255u8; px * 4];
            for i in 0..px {
                out[i * 4..i * 4 + 3].copy_from_slice(&[buf[i * 2]; 3]);
                out[i * 4 + 3] = buf[i * 2 + 1];
            }
            Some(out)
        }
        _ => None,
    }
}

struct Projection {
    min_x: f64,
    max_y: f64,
    scale: f64,
    cx: f64,
    cy: f64,
}
impl Projection {
    fn fit(points: &[(f64, f64)], w: f64, h: f64) -> Self {
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        for (x, y) in points {
            min_x = min_x.min(*x);
            max_x = max_x.max(*x);
            min_y = min_y.min(*y);
            max_y = max_y.max(*y);
        }
        if max_x - min_x < 1e-7 {
            min_x -= 0.005;
            max_x += 0.005;
        }
        if max_y - min_y < 1e-7 {
            min_y -= 0.005;
            max_y += 0.005;
        }
        let pad = 58.0;
        let scale = ((w - 2.0 * pad) / (max_x - min_x)).min((h - 2.0 * pad) / (max_y - min_y));
        let cx = (min_x + max_x) / 2.0;
        let cy = (min_y + max_y) / 2.0;
        Self {
            min_x: cx,
            max_y: cy,
            scale,
            cx: w / 2.0,
            cy: h / 2.0,
        }
    }
    fn point(&self, p: (f64, f64)) -> (i32, i32) {
        (
            (self.cx + (p.0 - self.min_x) * self.scale).round() as i32,
            (self.cy + (self.max_y - p.1) * self.scale).round() as i32,
        )
    }
}
fn mercator(lat: f64, lon: f64) -> (f64, f64) {
    let lat = lat.clamp(-85.0, 85.0).to_radians();
    (
        lon.to_radians(),
        (std::f64::consts::FRAC_PI_4 + lat / 2.0).tan().ln(),
    )
}
fn coord_key(lat: f64, lon: f64) -> String {
    format!("{lat:.5},{lon:.5}")
}
fn same_place(a: &Point, b: &Point) -> bool {
    coord_key(a.lat, a.lon) == coord_key(b.lat, b.lon)
}

struct Raster {
    w: usize,
    h: usize,
    pixels: Vec<u8>,
}
impl Raster {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            pixels: vec![0; w * h * 4],
        }
    }
    fn put(&mut self, x: i32, y: i32, c: [u8; 3]) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        self.pixels[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
    }
    fn blit(&mut self, rgba: &[u8]) {
        if rgba.len() == self.pixels.len() {
            self.pixels.copy_from_slice(rgba);
        }
    }
    fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: [u8; 3]) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.put(xx, yy, c);
            }
        }
    }
    fn grid(&mut self) {
        for y in 0..self.h {
            for x in 0..self.w {
                self.put(x as i32, y as i32, [247, 249, 251]);
            }
        }
        for x in (0..self.w).step_by(32) {
            self.line(
                (x as i32, 0),
                (x as i32, self.h as i32),
                [229, 234, 238],
                1,
                false,
            );
        }
        for y in (0..self.h).step_by(32) {
            self.line(
                (0, y as i32),
                (self.w as i32, y as i32),
                [229, 234, 238],
                1,
                false,
            );
        }
    }
    fn line(&mut self, a: (i32, i32), b: (i32, i32), color: [u8; 3], width: i32, dashed: bool) {
        let (mut x0, mut y0) = (a.0, a.1);
        let (x1, y1) = (b.0, b.1);
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let mut n = 0;
        loop {
            if !dashed || (n / 10) % 2 == 0 {
                self.dot(x0, y0, width / 2, color);
            }
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx
            }
            if e2 <= dx {
                err += dx;
                y0 += sy
            }
            n += 1;
        }
    }
    fn dot(&mut self, cx: i32, cy: i32, r: i32, c: [u8; 3]) {
        for y in -r..=r {
            for x in -r..=r {
                if x * x + y * y <= r * r {
                    self.put(cx + x, cy + y, c)
                }
            }
        }
    }
    fn pin(&mut self, x: i32, y: i32, c: [u8; 3], n: usize) {
        self.dot(x, y, 15, [255, 255, 255]);
        self.dot(x, y, 12, c);
        let label = n.to_string();
        let width = label.len() as i32 * 4 - 1;
        self.text(x - width / 2, y - 2, &label, [255, 255, 255], 1);
    }
    fn legend(&mut self, x: i32, y: i32, label: &str, c: [u8; 3]) {
        self.dot(x + 5, y + 5, 5, c);
        self.text(x + 14, y + 2, label, [50, 63, 76], 1);
    }
    fn text(&mut self, x: i32, y: i32, s: &str, c: [u8; 3], scale: i32) {
        let mut cx = x;
        for ch in s.chars() {
            let glyph = glyph(ch);
            for (gy, row) in glyph.iter().enumerate() {
                for gx in 0..5 {
                    if row & (1 << (4 - gx)) != 0 {
                        for yy in 0..scale {
                            for xx in 0..scale {
                                self.put(cx + gx * scale + xx, y + gy as i32 * scale + yy, c)
                            }
                        }
                    }
                }
            }
            cx += 6 * scale;
        }
    }
}
fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 2, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 25, 21, 19, 19, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '+' => [0, 4, 4, 31, 4, 4, 0],
        '©' => [14, 17, 23, 20, 23, 17, 14],
        ' ' => [0; 7],
        _ => [0; 7],
    }
}

async fn query_days(c: &Connection, p: &str) -> Result<Vec<i64>, String> {
    let mut r = c
        .query(
            "SELECT day_number FROM days WHERE plan_id=?1 ORDER BY day_number",
            params![p.to_string()],
        )
        .await
        .map_err(err("days query"))?;
    let mut v = Vec::new();
    while let Some(x) = r.next().await.map_err(err("days read"))? {
        v.push(x.get(0).map_err(err("day number"))?);
    }
    Ok(v)
}

/// Great-circle distance in metres (haversine, mean-earth radius).
fn haversine_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (lat1, lon1) = (a.0.to_radians(), a.1.to_radians());
    let (lat2, lon2) = (b.0.to_radians(), b.1.to_radians());
    let dlat = lat2 - lat1;
    let dlon = lon2 - lon1;
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * 6_371_008.8 * h.sqrt().asin()
}

/// Prefer the day's hotel pins; fall back to known logistics hotels. With
/// multiple hotels, use the nearest one to avoid warning about a nearby stay.
/// Airport endpoints are never excursion candidates.
fn distant_excursion_stop<'a>(points: &'a [Point], logistics: &[Point]) -> Option<(&'a Point, f64)> {
    let mut hotels: Vec<_> = points.iter().filter(|p| p.kind == Kind::Hotel).collect();
    if hotels.is_empty() {
        hotels.extend(logistics.iter().filter(|p| p.kind == Kind::Hotel));
    }
    points
        .iter()
        .filter(|p| p.kind == Kind::Sightseeing)
        .filter_map(|p| {
            let km = hotels
                .iter()
                .map(|h| haversine_m((p.lat, p.lon), (h.lat, h.lon)) / 1000.0)
                .min_by(f64::total_cmp)?;
            (km > 40.0).then_some((p, km))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

/// One cached ok leg's endpoints (geometry points are pulled per matched key).
pub(crate) struct LegRow {
    pub(crate) key: String,
    pub(crate) from: (f64, f64),
    pub(crate) to: (f64, f64),
}

/// Nominatim answers drift between the leg-fetch run and the render run (limit=1
/// feature choice moves a stop by hundreds of metres), so an exact 5-dp key match
/// misses and every leg falls back to a straight dashed connector — the "empty map"
/// regression. A cached leg therefore also matches when BOTH endpoints sit within
/// MATCH_RADIUS_M of the stop pair; the closest such leg wins. Exact key first.
const MATCH_RADIUS_M: f64 = 500.0;

pub(crate) fn match_leg<'a>(legs: &'a [LegRow], from: (f64, f64), to: (f64, f64)) -> Option<&'a LegRow> {
    legs.iter()
        .filter(|l| {
            haversine_m(l.from, from) <= MATCH_RADIUS_M && haversine_m(l.to, to) <= MATCH_RADIUS_M
        })
        .min_by(|a, b| {
            let da = haversine_m(a.from, from) + haversine_m(a.to, to);
            let db = haversine_m(b.from, from) + haversine_m(b.to, to);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
}

/// One OSM way from the road web (major = motorway/trunk/primary, minor =
/// secondary/tertiary). Drawn as the map's light background context.
struct RoadWay {
    points: Vec<(f64, f64)>,
    major: bool,
}

/// lat/lon bounds (min_lat, min_lon, max_lat, max_lon) of the stops and cached
/// route geometry, padded by ROAD_PAD_FRAC of its own span.
fn padded_bounds(points: &[Point], routes: &[RouteLine]) -> (f64, f64, f64, f64) {
    let mut min_lat = f64::INFINITY;
    let mut max_lat = f64::NEG_INFINITY;
    let mut min_lon = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;
    let mut fold = |lat: f64, lon: f64| {
        min_lat = min_lat.min(lat);
        max_lat = max_lat.max(lat);
        min_lon = min_lon.min(lon);
        max_lon = max_lon.max(lon);
    };
    for p in points {
        fold(p.lat, p.lon);
    }
    for r in routes {
        for (lat, lon) in &r.points {
            fold(*lat, *lon);
        }
    }
    let pad_lat = (max_lat - min_lat) * ROAD_PAD_FRAC;
    let pad_lon = (max_lon - min_lon) * ROAD_PAD_FRAC;
    (min_lat - pad_lat, min_lon - pad_lon, max_lat + pad_lat, max_lon + pad_lon)
}

/// Parse an Overpass `out geom` JSON body into drawable ways. `None` = the body
/// is NOT a valid Overpass response (rate-limit/error HTML or JSON without an
/// `elements` array) and the caller should retry; `Some(vec)` = authoritative
/// answer (possibly an empty bbox). Pure — unit-tested.
fn parse_overpass_ways(body: &[u8]) -> Option<Vec<RoadWay>> {
    let parsed: serde_json::Value = serde_json::from_slice(body).ok()?;
    let mut ways = Vec::new();
    let elements = parsed.get("elements")?.as_array()?;
    if elements.is_empty() && parsed.get("remark").is_some() {
        // Empty result WITH a remark = server-side timeout/abort (e.g. a bbox too
        // large), not an authoritative empty bbox — make the caller retry.
        return None;
    }
    for el in elements {
        if el.get("type").and_then(|v| v.as_str()) != Some("way") {
            continue;
        }
        let highway = el
            .get("tags")
            .and_then(|t| t.get("highway"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let (keep, major) = match highway {
            "motorway" | "trunk" | "primary" => (true, true),
            "secondary" | "tertiary" => (true, false),
            _ => (false, false),
        };
        if !keep {
            continue;
        }
        let Some(geom) = el.get("geometry").and_then(|v| v.as_array()) else {
            continue;
        };
        let pts: Vec<(f64, f64)> = geom
            .iter()
            .filter_map(|g| {
                let lat = g.get("lat").and_then(|v| v.as_f64())?;
                let lon = g.get("lon").and_then(|v| v.as_f64())?;
                Some((lat, lon))
            })
            .collect();
        if pts.len() > 1 {
            ways.push(RoadWay { points: pts, major });
        }
    }
    Some(ways)
}

/// Fetch the road web for one map's bbox from Overpass (vector data; NOT raster
/// tiles — see OVERPASS_URL note). Fail-soft: after retries the map renders
/// without background, exactly like a geocode failure. Never fails the snapshot.
async fn fetch_road_web(bounds: (f64, f64, f64, f64)) -> Vec<RoadWay> {
    let (min_lat, min_lon, max_lat, max_lon) = bounds;
    let query = format!(
        "[out:json][timeout:25];way['highway'~'^(motorway|trunk|primary|secondary|tertiary)$']({min_lat},{min_lon},{max_lat},{max_lon});out geom;"
    );
    // Politeness spacing + retry with backoff: the public endpoint rate-limits
    // bursts (429/error bodies), which read as "no elements" — retry those (trying
    // the mirror in the same round), but honour an authoritative empty bbox.
    thread::sleep(Duration::from_millis(2000));
    for attempt in 1..=3 {
        for url in OVERPASS_URLS {
            let output = match Command::new("curl")
                .args(["-sS", "--max-time", "45", "-X", "POST", "-d"])
                .arg(&query)
                .args(["-H", &format!("User-Agent: {USER_AGENT}"), url])
                .output()
            {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("   warn: Overpass spawn failed ({e}); trying next endpoint");
                    continue;
                }
            };
            if !output.status.success() {
                eprintln!(
                    "   warn: Overpass attempt {attempt} at {url} exited {}; trying next endpoint",
                    output.status
                );
                continue;
            }
            match parse_overpass_ways(&output.stdout) {
                Some(ways) => {
                    if ways.is_empty() {
                        eprintln!("   warn: Overpass returned zero roads for this bbox; rendering without road web");
                    }
                    return ways;
                }
                None => {
                    eprintln!("   warn: Overpass attempt {attempt} at {url} invalid response (rate limit?); trying next endpoint");
                }
            }
        }
        if attempt < 3 {
            let backoff = attempt * 5;
            eprintln!("   warn: Overpass round {attempt} exhausted; retrying in {backoff}s");
            thread::sleep(Duration::from_secs(backoff as u64));
        }
    }
    eprintln!("   warn: Overpass gave no valid response after 3 rounds; rendering without road web");
    Vec::new()
}

/// A drift-tolerant leg match reuses geometry fetched for slightly different endpoints
/// (e.g. a Nominatim answer vs the POI coordinate now pinned), so join the line to the
/// actual pins instead of leaving it ending a few hundred metres short.
fn anchor_to_stops(
    mut road: Vec<(f64, f64)>,
    from: (f64, f64),
    to: (f64, f64),
) -> Vec<(f64, f64)> {
    if road.first() != Some(&from) {
        road.insert(0, from);
    }
    if road.last() != Some(&to) {
        road.push(to);
    }
    road
}

/// OSRM demo-router URL for one driving leg. OSRM wants {lon},{lat}; every key
/// and geometry stored in this project is (lat, lon) — the flip happens only
/// here and back again after parsing. `via` waypoints (in order, between from
/// and to) force the router off its default fastest path — that is the ONLY way
/// a leg like 金山→淡水 becomes the 台2 coastal route instead of the mountain
/// shortcut OSRM prefers (re-fetching without waypoints returns the same road).
pub(crate) fn osrm_url(from: (f64, f64), to: (f64, f64), via: &[(f64, f64)]) -> String {
    let mut coords = String::new();
    for (lat, lon) in std::iter::once(from).chain(via.iter().copied()) {
        coords.push_str(&format!("{lon:.6},{lat:.6};"));
    }
    coords.push_str(&format!("{:.6},{:.6}", to.1, to.0));
    format!("https://router.project-osrm.org/route/v1/driving/{coords}?overview=full&geometries=geojson")
}

/// Fetch road-following geometry for one leg from the OSRM demo router and
/// write it through into route_road_legs + route_road_leg_points. The Rust
/// port of the old Tier-2 renderer could only READ this cache, so any leg
/// nobody had cached yet (every new trip) rendered as a straight line; a
/// cache miss now fetches once and persists, keeping later renders offline.
/// Shared with `road_leg refetch` — the CLI surface for replacing a cached leg
/// (e.g. forcing the coastal route). `via` is empty for the snapshot-maps
/// cache-miss path and ordered waypoints for a directed re-fetch.
pub(crate) async fn fetch_osrm_leg(
    write: &Connection,
    from: (f64, f64),
    to: (f64, f64),
    via: Vec<(f64, f64)>,
) -> Result<Vec<(f64, f64)>, String> {
    let key = format!(
        "{:.5},{:.5}>{:.5},{:.5}|osrm-demo|driving",
        from.0, from.1, to.0, to.1
    );
    thread::sleep(Duration::from_millis(1100)); // demo router: stay a polite caller
    let output = Command::new("curl")
        .args(["-sS", "--max-time", "20", &osrm_url(from, to, &via)])
        .output()
        .map_err(|e| format!("OSRM request failed: {e}"))?;
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("OSRM response parse failed: {e}"))?;
    let Some(route) = parsed.get("routes").and_then(|r| r.get(0)) else {
        return Err("OSRM returned no route".to_string());
    };
    let mut pts = Vec::new();
    if let Some(coords) = route
        .get("geometry")
        .and_then(|g| g.get("coordinates"))
        .and_then(|c| c.as_array())
    {
        for c in coords {
            if let (Some(lon), Some(lat)) = (
                c.get(0).and_then(|v| v.as_f64()),
                c.get(1).and_then(|v| v.as_f64()),
            ) {
                pts.push((lat, lon));
            }
        }
    }
    if pts.len() < 2 {
        return Err("OSRM geometry degenerate".to_string());
    }
    let now = chrono::Utc::now().to_rfc3339();
    let distance = route.get("distance").and_then(|d| d.as_f64());
    write
        .execute(
            "INSERT INTO route_road_legs (leg_key, from_lat, from_lon, to_lat, to_lon, provider, profile, status, point_count, distance_m, failure_reason, fetched_at) \
             VALUES (?1,?2,?3,?4,?5,'osrm-demo','driving','ok',?6,?7,NULL,?8) \
             ON CONFLICT(leg_key) DO UPDATE SET status='ok', point_count=excluded.point_count, \
             distance_m=excluded.distance_m, failure_reason=NULL, fetched_at=excluded.fetched_at",
            params![key.clone(), from.0, from.1, to.0, to.1, pts.len() as i64, distance, now],
        )
        .await
        .map_err(|e| format!("route_road_legs upsert failed: {e}"))?;
    write
        .execute(
            "DELETE FROM route_road_leg_points WHERE leg_key=?1",
            params![key.clone()],
        )
        .await
        .map_err(|e| format!("route_road_leg_points clear failed: {e}"))?;
    // One multi-row INSERT per chunk — a full leg is hundreds of points and
    // that many sequential round-trips would crawl over Turso HTTP.
    for (ci, chunk) in pts.chunks(50).enumerate() {
        let mut sql = String::from(
            "INSERT INTO route_road_leg_points (leg_key, point_order, lat, lon) VALUES ",
        );
        let mut bind: Vec<libsql::Value> = Vec::with_capacity(chunk.len() * 4 + 1);
        bind.push(libsql::Value::Text(key.clone()));
        for (i, (lat, lon)) in chunk.iter().enumerate() {
            let base = bind.len() as i64 + 1; // ?1 is the key; points follow
            sql.push_str(&format!(
                "(?1,?{},?{},?{}),",
                base,
                base + 1,
                base + 2
            ));
            bind.push(libsql::Value::Integer((ci * 50 + i) as i64));
            bind.push(libsql::Value::Real(*lat));
            bind.push(libsql::Value::Real(*lon));
        }
        sql.pop(); // trailing comma
        write
            .execute(&sql, libsql::params_from_iter(bind))
            .await
            .map_err(|e| format!("route_road_leg_points insert failed: {e}"))?;
    }
    Ok(pts)
}

/// Road-following lines for consecutive stop pairs. Reads the route_road_legs
/// cache first; a leg with no cached geometry is fetched from OSRM right here
/// (persisted via `w`, deduped across the day/plan renders of one run through
/// `live`) so a fresh trip follows roads on its FIRST render.
async fn cached_routes(
    c: &Connection,
    w: &Connection,
    live: &mut HashMap<String, Vec<(f64, f64)>>,
    points: &[Point],
) -> Result<Vec<RouteLine>, String> {
    let mut leg_rows = c
        .query(
            "SELECT leg_key, from_lat, from_lon, to_lat, to_lon FROM route_road_legs WHERE status='ok'",
            params![],
        )
        .await
        .map_err(err("cached road legs query"))?;
    let mut legs = Vec::new();
    while let Some(row) = leg_rows
        .next()
        .await
        .map_err(err("cached road legs read"))?
    {
        let (Ok(key), Ok(flat), Ok(flon), Ok(tlat), Ok(tlon)) = (
            row.get::<String>(0),
            row.get::<f64>(1),
            row.get::<f64>(2),
            row.get::<f64>(3),
            row.get::<f64>(4),
        ) else {
            continue;
        };
        legs.push(LegRow {
            key,
            from: (flat, flon),
            to: (tlat, tlon),
        });
    }
    drop(leg_rows);

    let mut routes = Vec::new();
    for pair in points.windows(2) {
        let exact = format!(
            "{:.5},{:.5}>{:.5},{:.5}|osrm-demo|driving",
            pair[0].lat, pair[0].lon, pair[1].lat, pair[1].lon
        );
        // Exact 5-dp key hit, else the nearest cached leg whose endpoints both sit
        // within MATCH_RADIUS_M (geocode drift tolerance).
        let leg = legs
            .iter()
            .find(|l| l.key == exact)
            .or_else(|| match_leg(&legs, (pair[0].lat, pair[0].lon), (pair[1].lat, pair[1].lon)));
        let mut road = Vec::new();
        if let Some(leg) = leg {
            let mut rows = c
                .query(
                    "SELECT p.lat, p.lon FROM route_road_leg_points p \
                 JOIN route_road_legs l ON l.leg_key=p.leg_key \
                 WHERE p.leg_key=?1 AND l.status='ok' ORDER BY p.point_order",
                    params![leg.key.as_str()],
                )
                .await
                .map_err(err("cached road geometry query"))?;
            while let Some(row) = rows
                .next()
                .await
                .map_err(err("cached road geometry read"))?
            {
                if let (Ok(lat), Ok(lon)) = (row.get::<f64>(0), row.get::<f64>(1)) {
                    road.push((lat, lon));
                }
            }
        }
        if road.len() <= 1 {
            // Cache miss (a leg no earlier render ever fetched): pull the road
            // geometry once, persist it, and reuse it for the rest of this run.
            let f = (pair[0].lat, pair[0].lon);
            let t = (pair[1].lat, pair[1].lon);
            let live_key = format!("{:.5},{:.5}>{:.5},{:.5}", f.0, f.1, t.0, t.1);
            if let Some(pts) = live.get(&live_key) {
                road = pts.clone();
            } else {
                match fetch_osrm_leg(w, f, t, vec![]).await {
                    Ok(pts) => {
                        live.insert(live_key, pts.clone());
                        road = pts;
                    }
                    Err(reason) => {
                        eprintln!("   warn: {reason} — leg renders as a straight line");
                    }
                }
            }
        }
        if road.len() > 1 {
            routes.push(RouteLine {
                points: anchor_to_stops(road, (pair[0].lat, pair[0].lon), (pair[1].lat, pair[1].lon)),
                routed: true,
                color: pair[0].color,
            });
        } else {
            routes.push(RouteLine {
                points: vec![(pair[0].lat, pair[0].lon), (pair[1].lat, pair[1].lon)],
                routed: false,
                color: pair[0].color,
            });
        }
    }
    Ok(routes)
}

async fn query_segments(c: &Connection, p: &str, d: &str) -> Result<Vec<Segment>, String> {
    let mut r=c.query("SELECT day_number, from_place, to_place, mode, duration_min FROM day_route_segments WHERE plan_id=?1 AND destination=?2 ORDER BY day_number, sort_order",params![p.to_string(),d.to_string()]).await.map_err(err("route segments query"))?;
    let mut v = Vec::new();
    while let Some(x) = r.next().await.map_err(err("route segments read"))? {
        v.push(Segment {
            day: x.get(0).map_err(err("route day"))?,
            from: x
                .get::<Option<String>>(1)
                .ok()
                .flatten()
                .unwrap_or_default(),
            to: x
                .get::<Option<String>>(2)
                .ok()
                .flatten()
                .unwrap_or_default(),
            mode: x
                .get::<Option<String>>(3)
                .ok()
                .flatten()
                .unwrap_or_default(),
            duration_min: x.get::<Option<i64>>(4).ok().flatten(),
        });
    }
    Ok(v)
}
async fn query_pois(
    c: &Connection,
    p: &str,
    d: &str,
) -> Result<Vec<(i64, String, f64, f64)>, String> {
    let mut r=c.query("SELECT a.day_number, p.title, p.lat, p.lon FROM activities a JOIN destination_pois p ON (p.poi_id=a.poi_id OR (a.poi_id IS NULL AND p.title=a.title)) AND p.slug=?2 WHERE a.plan_id=?1 AND p.lat IS NOT NULL AND p.lon IS NOT NULL ORDER BY a.day_number, a.sort_order",params![p.to_string(),d.to_string()]).await.map_err(err("itinerary POI query"))?;
    let mut v = Vec::new();
    while let Some(x) = r.next().await.map_err(err("itinerary POI read"))? {
        let (Ok(day), Ok(title), Ok(lat), Ok(lon)) = (x.get(0), x.get(1), x.get(2), x.get(3))
        else {
            continue;
        };
        v.push((day, title, lat, lon));
    }
    Ok(v)
}
pub(crate) async fn query_destination_pois(c: &Connection, d: &str) -> Result<Vec<PoiRow>, String> {
    let mut r = c
        .query(
            "SELECT poi_id, title, lat, lon FROM destination_pois WHERE slug=?1 AND lat IS NOT NULL AND lon IS NOT NULL",
            params![d.to_string()],
        )
        .await
        .map_err(err("destination POI query"))?;
    let mut v = Vec::new();
    while let Some(x) = r.next().await.map_err(err("destination POI read"))? {
        let (Ok(poi_id), Ok(title), Ok(lat), Ok(lon)) = (x.get(0), x.get(1), x.get(2), x.get(3))
        else {
            continue;
        };
        v.push(PoiRow {
            poi_id,
            title,
            lat,
            lon,
        });
    }
    Ok(v)
}
async fn query_hotel_names(c: &Connection, p: &str, d: &str) -> Result<Vec<String>, String> {
    let mut r = c
        .query(
            "SELECT name FROM hotels WHERE plan_id=?1 AND destination=?2",
            params![p.to_string(), d.to_string()],
        )
        .await
        .map_err(err("hotel names query"))?;
    let mut v = Vec::new();
    while let Some(x) = r.next().await.map_err(err("hotel names read"))? {
        if let Ok(name) = x.get::<String>(0) {
            if !name.trim().is_empty() {
                v.push(name);
            }
        }
    }
    Ok(v)
}
/// Domestic (Taiwan) booked stay for this plan — the SAME source the dashboard
/// stay card reads (`bookings_current` + its `hotel` payload KV). The `hotels`
/// table is the Japan path only, so without this a domestic stay never pins on
/// the logistics map. Returns (title, hotel_name).
async fn query_domestic_stay(
    c: &Connection,
    p: &str,
    d: &str,
) -> Result<Option<(String, String)>, String> {
    let mut r = c
        .query(
            "SELECT bc.title, bp.value FROM bookings_current bc \
             JOIN bookings_current_payload bp ON bp.booking_key = bc.booking_key AND bp.key='hotel' \
             WHERE bc.trip_id=?1 AND bc.destination=?2 AND bc.category='accommodation' \
               AND bc.status='booked' \
             LIMIT 1",
            params![p.to_string(), d.to_string()],
        )
        .await
        .map_err(err("domestic stay query"))?;
    if let Some(x) = r.next().await.map_err(err("domestic stay read"))? {
        let title: String = x.get(0).map_err(|e| e.to_string())?;
        let hotel: String = x.get(1).map_err(|e| e.to_string())?;
        if !hotel.trim().is_empty() {
            return Ok(Some((title, hotel)));
        }
    }
    Ok(None)
}

pub(crate) async fn geocode_context(c: &Connection, d: &str) -> Result<String, String> {
    let mut r = c
        .query(
            "SELECT display_name, currency FROM destination_config WHERE slug=?1 LIMIT 1",
            params![d.to_string()],
        )
        .await
        .map_err(err("destination context query"))?;
    let (display, currency) = if let Some(x) = r.next().await.map_err(err("destination context read"))? {
        (
            x.get::<Option<String>>(0)
                .ok()
                .flatten()
                .unwrap_or_default(),
            x.get::<Option<String>>(1)
                .ok()
                .flatten()
                .unwrap_or_default(),
        )
    } else {
        (String::new(), String::new())
    };
    if d.to_ascii_lowercase().contains("kyoto") {
        Ok("Kyoto, Japan".into())
    } else if !display.is_empty() {
        Ok(format!("{display}, {}", country_for(&currency)))
    } else {
        Ok(country_for(&currency).to_string())
    }
}
pub(crate) async fn load_geocodes(c: &Connection) -> Result<HashMap<String, (f64, f64)>, String> {
    let mut r=c.query("SELECT raw_place, lat, lon FROM route_place_geocodes WHERE lat IS NOT NULL AND lon IS NOT NULL",params![]).await.map_err(err("geocode cache query"))?;
    let mut m = HashMap::new();
    while let Some(x) = r.next().await.map_err(err("geocode cache read"))? {
        let (Ok(Some(name)), Ok(Some(lat)), Ok(Some(lon))) = (
            x.get::<Option<String>>(0),
            x.get::<Option<f64>>(1),
            x.get::<Option<f64>>(2),
        ) else {
            continue;
        };
        m.insert(name.to_ascii_lowercase(), (lat, lon));
    }
    Ok(m)
}

async fn resolve_segment_label(
    read: &Connection,
    write: &Connection,
    label: &str,
    context: &str,
    cache: &mut HashMap<String, (f64, f64)>,
    dest_pois: &[PoiRow],
    hotel_names: &[String],
) -> Result<Option<(f64, f64, Kind)>, String> {
    if let Some((lat, lon)) = match_poi(label, dest_pois) {
        return Ok(Some((lat, lon, Kind::Sightseeing)));
    }
    let place = match match_hotel(label, hotel_names) {
        Some(name) => name,
        None => label.to_string(),
    };
    Ok(resolve_place(read, write, &place, context, cache)
        .await?
        .map(|(lat, lon)| (lat, lon, classify(label))))
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn trailing_paren_parts(s: &str) -> (String, Option<String>) {
    let s = s.trim();
    for (open, close) in [("(", ")"), ("（", "）")] {
        if let Some(stripped) = s.strip_suffix(close) {
            if let Some(idx) = stripped.rfind(open) {
                let inner = stripped[idx + open.len()..].trim();
                let outer = stripped[..idx].trim();
                if !inner.is_empty() {
                    return (outer.to_string(), Some(inner.to_string()));
                }
            }
        }
    }
    (s.to_string(), None)
}

fn push_norm(keys: &mut Vec<String>, raw: &str) {
    let k = norm(raw);
    if !k.is_empty() && !keys.contains(&k) {
        keys.push(k);
    }
}

fn keys_of(s: &str) -> Vec<String> {
    let mut keys = Vec::new();
    push_norm(&mut keys, s);
    let (outer, inner) = trailing_paren_parts(s);
    push_norm(&mut keys, &outer);
    if let Some(inner) = inner {
        push_norm(&mut keys, &inner);
    }
    keys
}

pub(crate) fn match_poi(label: &str, pois: &[PoiRow]) -> Option<(f64, f64)> {
    let wanted: HashSet<String> = keys_of(label).into_iter().collect();
    if wanted.is_empty() {
        return None;
    }
    let mut coords = HashSet::new();
    let mut hit = None;
    for poi in pois {
        let mut keys = keys_of(&poi.title);
        push_norm(&mut keys, &poi.poi_id);
        if keys.iter().any(|k| wanted.contains(k)) {
            coords.insert(coord_key(poi.lat, poi.lon));
            hit = Some((poi.lat, poi.lon));
        }
    }
    if coords.len() == 1 { hit } else { None }
}

fn match_hotel(label: &str, hotel_names: &[String]) -> Option<String> {
    let wanted: HashSet<String> = keys_of(label).into_iter().collect();
    if wanted.is_empty() {
        return None;
    }
    let mut hits = Vec::new();
    for name in hotel_names {
        if keys_of(name).iter().any(|k| wanted.contains(k)) && !hits.iter().any(|h| h == name) {
            hits.push(name.clone());
        }
    }
    if hits.is_empty() && is_generic_hotel_token(label) && hotel_names.len() == 1 {
        // Route segments use a bare generic token ("hotel") for the stay; with
        // exactly one candidate it resolves to that hotel instead of geocoding
        // "hotel, <city>" to an arbitrary first Nominatim hit.
        return hotel_names.first().cloned();
    }
    if hits.len() == 1 { hits.pop() } else { None }
}

/// A bare stay word used as a segment endpoint, resolvable when the plan has a
/// unique hotel. Case-insensitive for the ASCII tokens.
fn is_generic_hotel_token(label: &str) -> bool {
    let t = label.trim();
    t.eq_ignore_ascii_case("hotel")
        || t.eq_ignore_ascii_case("hostel")
        || t.eq_ignore_ascii_case("ryokan")
        || matches!(t, "飯店" | "旅館" | "民宿" | "ホテル")
}

/// Legend label for a segment endpoint: the hotel's full name when the label
/// resolves to the plan's hotel, else the segment label verbatim — the legend
/// must speak the itinerary's own naming, not a re-derived one.
fn display_label(label: &str, hotel_names: &[String]) -> String {
    match_hotel(label, hotel_names).unwrap_or_else(|| label.to_string())
}

/// A resolved place, with what Nominatim said about it. `area_centroid` marks
/// an administrative AREA (district/town) resolved to its geometric centre —
/// usually inland, OFF the road a `road-leg refetch --via` caller wants (the
/// 三芝 → 101 縣道 miss: the via "resolved", then OSRM shortcut inland from it).
#[derive(Clone, Debug)]
pub(crate) struct ResolvedPlace {
    pub(crate) coords: (f64, f64),
    pub(crate) display_name: String,
    pub(crate) area_centroid: bool,
}

/// Nominatim category/type → administrative area? jsonv2 emits `category`
/// (e.g. "boundary", "place") and `type` (e.g. "administrative", "town").
pub(crate) fn nominatim_is_area(category: &str, ntype: &str) -> bool {
    category == "boundary"
        || matches!(
            ntype,
            "administrative"
                | "city"
                | "town"
                | "village"
                | "district"
                | "borough"
                | "suburb"
                | "municipality"
                | "county"
        )
}

pub(crate) async fn resolve_place(
    read: &Connection,
    write: &Connection,
    place: &str,
    context: &str,
    cache: &mut HashMap<String, (f64, f64)>,
) -> Result<Option<(f64, f64)>, String> {
    Ok(
        resolve_place_meta(read, write, place, context, cache)
            .await?
            .map(|p| p.coords),
    )
}

pub(crate) async fn resolve_place_meta(
    read: &Connection,
    write: &Connection,
    place: &str,
    context: &str,
    cache: &mut HashMap<String, (f64, f64)>,
) -> Result<Option<ResolvedPlace>, String> {
    let key = place.trim().to_ascii_lowercase();
    if let Some(p) = cache.get(&key) {
        // In-memory hit: coords only (metadata is advisory, best-effort).
        return Ok(Some(ResolvedPlace {
            coords: *p,
            display_name: String::new(),
            area_centroid: false,
        }));
    }
    let (search, ctx) = normalize_place(place, context);
    let query_key = format!("{}|{}", search.trim().to_ascii_lowercase(), ctx);
    let mut r = read
        .query(
            "SELECT lat, lon, display_name FROM route_place_geocodes WHERE query_key=?1",
            params![query_key.clone()],
        )
        .await
        .map_err(err("geocode cache lookup"))?;
    if let Some(row) = r.next().await.map_err(err("geocode cache lookup read"))? {
        if let (Ok(Some(lat)), Ok(Some(lon)), Ok(display)) = (
            row.get::<Option<f64>>(0),
            row.get::<Option<f64>>(1),
            row.get::<Option<String>>(2),
        ) {
            cache.insert(key, (lat, lon));
            return Ok(Some(ResolvedPlace {
                coords: (lat, lon),
                display_name: display.unwrap_or_default(),
                area_centroid: false, // category is not persisted; unknown here
            }));
        }
        return Ok(None);
    }
    thread::sleep(Duration::from_millis(1100));
    let output = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "20",
            "-G",
            "https://nominatim.openstreetmap.org/search",
            "-H",
            &format!("User-Agent: {USER_AGENT}"),
        ])
        .arg("--data-urlencode")
        .arg(format!("q={search}, {ctx}"))
        .args([
            "--data-urlencode",
            "format=jsonv2",
            "--data-urlencode",
            "limit=1",
            "--data-urlencode",
            "addressdetails=0",
        ])
        .output()
        .map_err(|e| format!("Nominatim request failed for {place}: {e}"))?;
    let parsed: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap_or_default();
    let Some(item) = parsed.first() else {
        return Ok(None);
    };
    let Some(lat) = item
        .get("lat")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<f64>().ok())
    else {
        return Ok(None);
    };
    let Some(lon) = item
        .get("lon")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<f64>().ok())
    else {
        return Ok(None);
    };
    let display = item
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let category = item.get("category").and_then(|v| v.as_str()).unwrap_or("");
    let ntype = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let osm_id = item
        .get("osm_id")
        .and_then(|v| v.as_i64())
        .map(|n| n.to_string())
        .unwrap_or_default();
    let osm_type = item.get("osm_type").and_then(|v| v.as_str()).unwrap_or("");
    let now = chrono::Utc::now().to_rfc3339();
    write.execute("INSERT INTO route_place_geocodes (query_key,raw_place,lat,lon,display_name,osm_id,osm_type,provider,confidence,review,failure_reason,fetched_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'nominatim','ok',0,NULL,?8) ON CONFLICT(query_key) DO UPDATE SET raw_place=excluded.raw_place,lat=excluded.lat,lon=excluded.lon,display_name=excluded.display_name,osm_id=excluded.osm_id,osm_type=excluded.osm_type,provider='nominatim',confidence='ok',failure_reason=NULL,fetched_at=excluded.fetched_at",params![query_key,place.to_string(),lat,lon,display.to_string(),osm_id,osm_type.to_string(),now]).await.map_err(err("geocode cache write"))?;
    cache.insert(key, (lat, lon));
    Ok(Some(ResolvedPlace {
        coords: (lat, lon),
        display_name: display.to_string(),
        area_centroid: nominatim_is_area(category, ntype),
    }))
}
/// Country suffix for Nominatim search context, from the destination's
/// currency (the same signal that classifies a plan as domestic). The old
/// code hardcoded ", Japan" — a Taiwan trip then searched "淡水, 九份, Japan"
/// and every such lookup missed.
pub(crate) fn country_for(currency: &str) -> &'static str {
    match currency.trim().to_ascii_uppercase().as_str() {
        "TWD" => "Taiwan",
        _ => "Japan",
    }
}

pub(crate) fn normalize_place(place: &str, context: &str) -> (String, String) {
    match place.trim() {
        "KIX" | "KIX T1" | "KIX T2" => {
            ("Kansai International Airport".into(), "Osaka, Japan".into())
        }
        "ITM" => ("Osaka International Airport".into(), "Osaka, Japan".into()),
        "TPE" | "TPE T1" | "TPE T2" => (
            "Taiwan Taoyuan International Airport".into(),
            "Taoyuan City, Taiwan".into(),
        ),
        "NRT" => (
            "Narita International Airport".into(),
            "Narita, Japan".into(),
        ),
        "HND" => ("Haneda Airport".into(), "Tokyo, Japan".into()),
        "京都站" | "京都駅" => ("Kyoto Station Building".into(), "Kyoto, Japan".into()),
        p if p
            .to_ascii_uppercase()
            .starts_with("APA HOTEL KYOTO EKIHIGASHI") =>
        {
            ("APA Hotel Kyoto Eki Higashi".into(), "Kyoto, Japan".into())
        }
        // CJK-first labels usually carry the searchable ASCII name in parens —
        // "京都哈頓飯店 (Hearton Hotel Kyoto)" geocodes as "Hearton Hotel Kyoto".
        // When the parenthetical is itself CJK ("APA Hotel Kyoto Ekimae (APA京都站前)"),
        // the ASCII outer name is the searchable part.
        p => {
            let (outer, inner) = trailing_paren_parts(p);
            let ascii_inner = inner.as_deref().filter(|i| is_searchable_ascii(i));
            if let Some(inner) = ascii_inner {
                return (inner.to_string(), context.to_string());
            }
            if inner.is_some() && is_searchable_ascii(&outer) {
                return (outer, context.to_string());
            }
            (p.to_string(), context.to_string())
        }
    }
}

/// A geocodable ASCII name fragment (Nominatim-friendly): alphanumerics plus the
/// few separators that survive URL encoding. Empty is not searchable.
fn is_searchable_ascii(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, ' ' | '&' | '-' | '.' | '\'')
        })
}

async fn await_logistics(
    read: &Connection,
    write: &Connection,
    p: &str,
    d: &str,
    ctx: &str,
    cache: &mut HashMap<String, (f64, f64)>,
    dest_pois: &[PoiRow],
    hotel_names: &[String],
) -> Result<Vec<Point>, String> {
    let mut labels = Vec::<(String, Kind, bool)>::new();
    for name in hotel_names {
        labels.push((name.clone(), Kind::Hotel, false));
    }
    let mut f=read.query("SELECT departure_code, arrival_code, departure_airport, arrival_airport, flight_number, direction FROM flight_legs WHERE plan_id=?1 AND destination=?2 ORDER BY direction,leg_order",params![p.to_string(),d.to_string()]).await.map_err(err("flight map query"))?;
    // Home-airport codes to EXCLUDE from the logistics map: the first code of the
    // first outbound flight and the last code of the last return flight. Including
    // the home airport stretches the bbox across countries (e.g. TPE↔KIX, ~1000 km),
    // which flattens the map to empty and times the road-web query out.
    let mut home_codes = Vec::new();
    let mut rows: Vec<(Vec<String>, String)> = Vec::new();
    while let Some(x) = f.next().await.map_err(err("flight map read"))? {
        for i in 0..4 {
            if let Ok(Some(s)) = x.get::<Option<String>>(i) {
                if !s.trim().is_empty() {
                    labels.push((s, Kind::Airport, false));
                }
            }
        }
        let direction = x.get::<String>(5).unwrap_or_default();
        let mut codes = Vec::new();
        if let Ok(flight) = x.get::<String>(4) {
            for pair in flight.split_whitespace() {
                if let Some((a, b)) = pair.split_once('-') {
                    if a.len() == 3
                        && b.len() == 3
                        && a.chars().all(|c| c.is_ascii_uppercase())
                        && b.chars().all(|c| c.is_ascii_uppercase())
                    {
                        codes.push(a.into());
                        codes.push(b.into());
                    }
                }
            }
        }
        rows.push((codes, direction));
    }
    // rows are ordered (direction, leg_order): outbound legs first. First row =
    // first outbound leg → its first code is home; last row = last return leg →
    // its last code is home.
    if let Some((codes, _)) = rows.first() {
        if let Some(first) = codes.first() {
            home_codes.push(first.clone());
        }
    }
    if let Some((codes, _)) = rows.last() {
        if let Some(last) = codes.last() {
            home_codes.push(last.clone());
        }
    }
    for (codes, _) in rows {
        for code in codes {
            if !home_codes.contains(&code) {
                labels.push((code, Kind::Airport, false));
            }
        }
    }
    let mut r=read.query("SELECT from_place,to_place FROM day_route_segments WHERE plan_id=?1 AND destination=?2",params![p.to_string(),d.to_string()]).await.map_err(err("logistics route query"))?;
    while let Some(x) = r.next().await.map_err(err("logistics route read"))? {
        for i in 0..2 {
            if let Ok(s) = x.get::<String>(i) {
                let k = classify(&s);
                if k != Kind::Sightseeing {
                    labels.push((s, k, true));
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    // Domestic stay: `hotels` is Japan-path only, so the booked domestic stay
    // arrives here instead. Prefer its geocoded destination_pois row (title
    // contains the hotel name, e.g. 海論 → 海論海景民宿); only if no POI matches
    // does the label fall through to the shared geocode chain below.
    if let Some((_title, hotel)) = query_domestic_stay(read, p, d).await? {
        match dest_pois
            .iter()
            .find(|poi| norm(&poi.title).contains(&norm(&hotel)))
        {
            Some(poi) => {
                if seen.insert((coord_key(poi.lat, poi.lon), Kind::Hotel as u8)) {
                    out.push(Point {
                        lat: poi.lat,
                        lon: poi.lon,
                        color: [21, 101, 192],
                        kind: Kind::Hotel,
                        label: hotel,
                    });
                }
            }
            None => labels.push((hotel, Kind::Hotel, false)),
        }
    }
    for (label, kind, from_segment) in labels {
        // Keep the label's hotel/airport kind: this map only draws logistics endpoints,
        // so a label that happens to share a POI key must not turn into a sightseeing pin.
        let resolved = if from_segment {
            resolve_segment_label(read, write, &label, ctx, cache, dest_pois, hotel_names)
                .await?
                .map(|(lat, lon, _)| (lat, lon, kind))
        } else {
            resolve_place(read, write, &label, ctx, cache)
                .await?
                .map(|(lat, lon)| (lat, lon, kind))
        };
        if let Some((lat, lon, kind)) = resolved {
            let key = coord_key(lat, lon);
            if seen.insert((key, kind as u8)) {
                out.push(Point {
                    lat,
                    lon,
                    color: if kind == Kind::Hotel {
                        [21, 101, 192]
                    } else {
                        [239, 108, 0]
                    },
                    kind,
                    label: display_label(&label, hotel_names),
                });
            }
        }
    }
    Ok(out)
}
fn classify(s: &str) -> Kind {
    let l = s.to_ascii_lowercase();
    if l.contains("airport")
        || l.contains("機場")
        || l.contains("空港")
        || l.contains("航廈")
        || matches!(s.trim(), "KIX" | "TPE" | "NRT" | "HND" | "ITM")
        // "KIX 第一航廈" / "TPE T1": an airport code as the first token.
        || matches!(
            s.split_whitespace().next(),
            Some("KIX" | "TPE" | "NRT" | "HND" | "ITM")
        )
    {
        Kind::Airport
    } else if l.contains("hotel")
        || l.contains("旅館")
        || l.contains("飯店")
        || l.contains("民宿")
        || l.contains("hostel")
        || l.contains("ryokan")
    {
        Kind::Hotel
    } else {
        Kind::Sightseeing
    }
}

/// Record `key` as skipped and, when an earlier run uploaded it, delete the R2
/// object — the worker renders any PNG it finds, so a stale one would linger.
async fn remove_stale_map(c: &Connection, plan: &str, key: &str, reason: &str) -> Result<(), String> {
    let mut r = c
        .query(
            "SELECT status FROM map_artifacts WHERE plan_id = ?1 AND map_key = ?2",
            params![plan.to_string(), key.to_string()],
        )
        .await
        .map_err(err("map manifest read"))?;
    let was_uploaded = match r.next().await.map_err(err("map manifest row"))? {
        Some(row) => row.get::<Option<String>>(0).ok().flatten().as_deref() == Some("uploaded"),
        None => false,
    };
    if was_uploaded {
        let status = wrangler_command()
            .args([
                "wrangler",
                "r2",
                "object",
                "delete",
                &format!("{BUCKET}/{plan}/{key}"),
                "--remote",
            ])
            .status()
            .map_err(|e| format!("failed to run Wrangler delete for {key}: {e}"))?;
        if !status.success() {
            return Err(format!("Wrangler could not delete stale {key} (exit {status})"));
        }
        println!("   removed stale {key} ({reason})");
    }
    record_artifact(c, plan, key, 0, None, "skipped", Some(reason), None, None).await?;
    Ok(())
}

async fn record_artifact(
    _c: &Connection,
    p: &str,
    key: &str,
    size: usize,
    sha: Option<&str>,
    status: &str,
    reason: Option<&str>,
    input_sha: Option<&str>,
    has_roads: Option<i64>,
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    let c = crate::db::connect_write().await?;
    c.execute("INSERT INTO map_artifacts (plan_id,map_key,byte_size,sha256,status,skip_reason,generated_at,input_sha256,has_roads) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(plan_id,map_key) DO UPDATE SET byte_size=excluded.byte_size,sha256=excluded.sha256,status=excluded.status,skip_reason=excluded.skip_reason,generated_at=excluded.generated_at,input_sha256=excluded.input_sha256,has_roads=excluded.has_roads",params![p.to_string(),key.to_string(),size as i64,sha.unwrap_or_default().to_string(),status.to_string(),reason.map(str::to_string),now,input_sha.map(str::to_string),has_roads]).await.map_err(err("map manifest write"))?;
    Ok(())
}
fn wrangler_command() -> Command {
    let mut c = Command::new("npx");
    c.env_remove("CLOUDFLARE_API_TOKEN");
    if node_major() < 22 {
        if let Some(dir) = node22_dir() {
            let path = env::var_os("PATH").unwrap_or_default();
            let mut paths = vec![dir];
            paths.extend(env::split_paths(&path));
            if let Ok(path) = env::join_paths(paths) {
                c.env("PATH", path);
            }
        }
    }
    c
}
fn node_major() -> u32 {
    Command::new("node")
        .args(["-p", "Number(process.versions.node.split('.')[0])"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0)
}
fn node22_dir() -> Option<PathBuf> {
    let root = PathBuf::from(env::var_os("HOME")?).join(".nvm/versions/node");
    fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path().join("bin"))
        .filter(|p| p.join("node").is_file())
        .max()
}
fn parse_dest(args: &[String]) -> Result<Option<String>, String> {
    let mut i = 0;
    let mut dest = None;
    while i < args.len() {
        match args[i].as_str() {
            "--dest" => {
                dest = Some(args.get(i + 1).ok_or("--dest requires a value")?.clone());
                i += 2
            }
            "--plan-id" | "--travel-date" | "--travel-start" | "--travel-end" => i += 2,
            x if x.starts_with('-') => return Err(format!("unknown argument: {x}")),
            _ => i += 1,
        }
    }
    Ok(dest)
}
fn print_usage() {
    println!(
        "Usage:\n  travel snapshot-maps [--dest <slug>]\n\nRenders route-diagram PNGs in Rust from Turso itinerary and cached place data, uploads them to the dashboard R2 bucket, and stamps map freshness. Geographic background: ArcGIS static basemap (Web Mercator export), falling back to an Overpass road web; it never fetches OSM raster tiles. Requires Wrangler authentication."
    );
}
fn err<T: std::fmt::Display>(ctx: &'static str) -> impl FnOnce(T) -> String {
    move |e| format!("{ctx}: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excursion_suggestion_uses_sights_and_nearest_hotel() {
        let point = |lat, kind| Point {
            lat,
            lon: 135.0,
            kind,
            label: "test".into(),
            color: [0; 3],
        };
        let hotels = vec![point(35.0, Kind::Hotel)];
        let local = vec![point(35.3, Kind::Sightseeing), point(36.0, Kind::Airport)];
        assert!(distant_excursion_stop(&local, &hotels).is_none());
        let far = vec![point(35.5, Kind::Sightseeing), point(36.0, Kind::Sightseeing)];
        let (stop, km) = distant_excursion_stop(&far, &hotels).unwrap();
        assert_eq!(stop.lat, 36.0);
        assert!(km > 100.0);
        assert!(distant_excursion_stop(&far, &[]).is_none());
        let multiple = vec![point(35.0, Kind::Hotel), point(36.0, Kind::Hotel)];
        assert!(distant_excursion_stop(&far[1..], &multiple).is_none());
        let day_hotel = vec![point(36.0, Kind::Sightseeing), point(36.01, Kind::Hotel)];
        assert!(distant_excursion_stop(&day_hotel, &hotels).is_none());
    }

    #[test]
    fn plausible_leg_flags_a_pin_80km_off_on_a_15min_drive() {
        // 智恩寺 geocoded to 百万遍知恩寺 (Kyoto) vs 傘松公園 (Miyazu): ~80 km.
        let km = haversine_m((35.0300, 135.7810), (35.5868, 135.1951)) / 1000.0;
        let max = plausible_leg_km("driving", Some(15)).unwrap();
        assert!(km > max, "km={km} max={max}");
        // The corrected pin (天橋立 智恩寺) is ~3 km away — well within.
        let ok = haversine_m((35.5578, 135.1846), (35.5868, 135.1951)) / 1000.0;
        assert!(ok <= max, "ok={ok} max={max}");
    }

    #[test]
    fn classify_treats_airport_terminal_labels_as_airports() {
        assert!(matches!(classify("KIX 第一航廈"), Kind::Airport));
        assert!(matches!(classify("KIX 第二航廈"), Kind::Airport));
        assert!(matches!(classify("TPE T1"), Kind::Airport));
        assert!(matches!(classify("桃園機場第二航廈"), Kind::Airport));
        assert!(matches!(classify("京都車站"), Kind::Sightseeing));
    }

    #[test]
    fn plausible_leg_needs_a_duration_and_scales_by_mode() {
        assert_eq!(plausible_leg_km("walking", None), None);
        assert_eq!(plausible_leg_km("walking", Some(0)), None);
        assert!(plausible_leg_km("walking", Some(30)).unwrap() < 8.0);
        // A 2h15 shinkansen leg (Tokyo→Kyoto ~370 km) must not be flagged.
        assert!(plausible_leg_km("transit", Some(135)).unwrap() > 370.0);
    }

    fn leg(key: &str, from: (f64, f64), to: (f64, f64)) -> LegRow {
        LegRow {
            key: key.into(),
            from,
            to,
        }
    }

    #[test]
    fn country_for_maps_currency_to_country() {
        assert_eq!(country_for("TWD"), "Taiwan");
        assert_eq!(country_for("twd"), "Taiwan");
        assert_eq!(country_for("JPY"), "Japan");
        assert_eq!(country_for(""), "Japan");
    }

    #[test]
    fn nominatim_is_area_flags_centroids_not_landmarks() {
        // The 三芝 regression: a bare district name resolves to a boundary/
        // administrative centroid (inland), while a landmark on the road
        // (淺水灣, 淡金公路) resolves to a place/point feature.
        assert!(nominatim_is_area("boundary", "administrative"));
        assert!(nominatim_is_area("place", "town"));
        assert!(nominatim_is_area("place", "district"));
        assert!(nominatim_is_area("place", "village"));
        assert!(!nominatim_is_area("place", "house"));
        assert!(!nominatim_is_area("tourism", "attraction"));
        assert!(!nominatim_is_area("highway", "bus_stop"));
        assert!(!nominatim_is_area("", ""));
    }

    #[test]
    fn osrm_url_flips_to_lon_lat() {
        // 九份海論 → 野柳: storage is (lat, lon); OSRM wants {lon},{lat}.
        let u = osrm_url((25.1103, 121.8451), (25.2113, 121.6964), &[]);
        assert!(
            u.starts_with(
                "https://router.project-osrm.org/route/v1/driving/121.845100,25.110300;121.696400,25.211300?"
            ),
            "got {u}"
        );
    }

    #[test]
    fn osrm_url_appends_via_waypoints_in_order() {
        // 金山 → 石門 → 三芝 → 淡水 (coastal 台2): vias sit between the endpoints.
        let u = osrm_url(
            (25.2219, 121.6362),
            (25.1727, 121.4377),
            &[(25.2897, 121.5690), (25.2317, 121.5018)],
        );
        assert!(
            u.starts_with(
                "https://router.project-osrm.org/route/v1/driving/121.636200,25.221900;121.569000,25.289700;121.501800,25.231700;121.437700,25.172700?"
            ),
            "got {u}"
        );
    }

    #[test]
    fn anchor_to_stops_joins_drifted_geometry_to_pins() {
        let road = vec![(35.0010, 135.0010), (35.0050, 135.0050)];
        let got = anchor_to_stops(road, (35.0, 135.0), (35.006, 135.006));
        assert_eq!(got.first(), Some(&(35.0, 135.0)));
        assert_eq!(got.last(), Some(&(35.006, 135.006)));
        assert_eq!(got.len(), 4);
        let exact = vec![(35.0, 135.0), (35.006, 135.006)];
        assert_eq!(anchor_to_stops(exact.clone(), (35.0, 135.0), (35.006, 135.006)), exact);
    }

    #[test]
    fn haversine_known_distance() {
        // ~111 km per degree of latitude.
        let d = haversine_m((35.0, 135.0), (36.0, 135.0));
        assert!((110_500.0..=112_000.0).contains(&d), "got {d} m");
    }

    #[test]
    fn match_leg_tolerates_geocode_drift() {
        // Sanzen-in regression: fetched 35.11902,135.83018 vs rendered 35.11964,135.83492
        // (~435 m apart) — exact key misses, fuzzy match must recover the leg.
        let legs = vec![leg(
            "34.99123,135.75839>35.11902,135.83018|osrm-demo|driving",
            (34.99123, 135.75839),
            (35.11902, 135.83018),
        )];
        let m = match_leg(&legs, (34.99164, 135.75888), (35.11964, 135.83492));
        assert!(m.is_some(), "drifted pair must still match the cached leg");
    }

    #[test]
    fn match_leg_rejects_far_endpoints() {
        let legs = vec![leg(
            "a",
            (35.0, 135.0),
            (35.5, 135.5),
        )];
        assert!(match_leg(&legs, (35.0, 135.0), (36.0, 136.0)).is_none());
        assert!(match_leg(&legs, (40.0, 140.0), (35.5, 135.5)).is_none());
    }

    #[test]
    fn match_leg_prefers_closest() {
        let near = leg("near", (35.1000, 135.1000), (35.2000, 135.2000));
        let far = leg("far", (35.1010, 135.1010), (35.2020, 135.2020));
        let legs = vec![far, near];
        let m = match_leg(&legs, (35.1000, 135.1000), (35.2000, 135.2000));
        assert_eq!(m.unwrap().key, "near");
    }

    #[test]
    fn parse_overpass_ways_keeps_major_roads_only() {
        let body = br#"{"elements":[
            {"type":"node","id":1,"lat":35.0,"lon":135.0},
            {"type":"way","id":2,"tags":{"highway":"motorway"},"geometry":[
                {"lat":35.00,"lon":135.00},{"lat":35.01,"lon":135.01}]},
            {"type":"way","id":3,"tags":{"highway":"tertiary"},"geometry":[
                {"lat":35.02,"lon":135.00},{"lat":35.02,"lon":135.02}]},
            {"type":"way","id":4,"tags":{"highway":"residential"},"geometry":[
                {"lat":35.03,"lon":135.00},{"lat":35.03,"lon":135.02}]},
            {"type":"way","id":5,"tags":{"highway":"primary"},"geometry":[
                {"lat":35.04,"lon":135.00}]}
        ]}"#;
        let ways = parse_overpass_ways(body).expect("valid response");
        assert_eq!(ways.len(), 2, "motorway+tertiary in, residential and 1-point way out");
        assert!(ways[0].major, "motorway is major");
        assert!(!ways[1].major, "tertiary is minor");
    }

    #[test]
    fn parse_overpass_ways_garbage_is_invalid_not_empty() {
        // Garbage / error bodies are None (retry), NOT Some(empty) (authoritative).
        assert!(parse_overpass_ways(b"not json").is_none());
        assert!(parse_overpass_ways(br#"{"elements":"nope"}"#).is_none());
        assert!(parse_overpass_ways(br#"{"error":"runtime error: query timed out"}"#).is_none());
        // A valid response with zero matching ways is authoritative Some(empty).
        assert!(parse_overpass_ways(br#"{"elements":[]}"#).is_some_and(|w| w.is_empty()));
    }

    #[test]
    fn padded_bounds_pads_by_span_fraction() {
        let points = vec![
            Point { lat: 35.0, lon: 135.0, color: [0, 0, 0], kind: Kind::Sightseeing, label: String::new() },
            Point { lat: 35.1, lon: 135.2, color: [0, 0, 0], kind: Kind::Sightseeing, label: String::new() },
        ];
        let (s, w, n, e) = padded_bounds(&points, &[]);
        assert!((s - 34.98).abs() < 1e-9, "south pad = 0.02 (20% of 0.1°): {s}");
        assert!((w - 134.96).abs() < 1e-9, "west pad = 0.04 (20% of 0.2°): {w}");
        assert!((n - 35.12).abs() < 1e-9 && (e - 135.24).abs() < 1e-9);
    }

    #[test]
    fn normalize_place_extracts_ascii_name_from_parens() {
        let (search, _) = normalize_place("京都哈頓飯店 (Hearton Hotel Kyoto)", "Kyoto, Japan");
        assert_eq!(search, "Hearton Hotel Kyoto");
        // Pure-ASCII and CJK-only labels pass through unchanged.
        assert_eq!(normalize_place("Kifune Shrine", "Kyoto, Japan").0, "Kifune Shrine");
        assert_eq!(normalize_place("貴船神社", "Kyoto, Japan").0, "貴船神社");
    }

    #[test]
    fn normalize_place_falls_back_to_ascii_outer_when_inner_is_cjk() {
        // "APA Hotel Kyoto Ekimae (APA京都站前)": the parenthetical is mixed CJK, but
        // the outer name is the searchable ASCII one — searching the full string
        // returns no_result and drops the hotel pin from every map.
        let (search, _) = normalize_place("APA Hotel Kyoto Ekimae (APA京都站前)", "Kyoto, Japan");
        assert_eq!(search, "APA Hotel Kyoto Ekimae");
        // Both parts non-searchable → unchanged full-string passthrough.
        assert_eq!(normalize_place("嘟嘟房桃園機場貨運1站", "Kyoto, Japan").0, "嘟嘟房桃園機場貨運1站");
    }

    #[test]
    fn parse_overpass_ways_remark_empty_is_retryable() {
        // Server-side timeout: empty elements + remark → None (retry), unlike a
        // clean empty bbox which is Some(empty).
        let body = br#"{"elements":[],"remark":"runtime error: query timed out"}"#;
        assert!(parse_overpass_ways(body).is_none());
    }

    #[test]
    fn render_png_with_road_web_changes_output() {
        let points = vec![
            Point { lat: 35.0, lon: 135.0, color: [200, 50, 120], kind: Kind::Sightseeing, label: String::new() },
            Point { lat: 35.01, lon: 135.01, color: [200, 50, 120], kind: Kind::Sightseeing, label: String::new() },
        ];
        let roads = vec![RoadWay {
            points: vec![(35.0, 135.0), (35.005, 135.008), (35.01, 135.01)],
            major: true,
        }];
        let bare = render_png("T", &points, &[], &[], None, MapKind::Day).unwrap();
        let with = render_png("T", &points, &[], &roads, None, MapKind::Day).unwrap();
        assert!(bare.len() >= MIN_PNG_BYTES && with.len() >= MIN_PNG_BYTES);
        assert_ne!(bare, with, "road web must visibly change the render");
    }

    #[test]
    fn render_png_with_basemap_replaces_grid_background() {
        let points = vec![
            Point { lat: 35.0, lon: 135.0, color: [200, 50, 120], kind: Kind::Sightseeing, label: String::new() },
            Point { lat: 35.01, lon: 135.01, color: [200, 50, 120], kind: Kind::Sightseeing, label: String::new() },
        ];
        let mut base = vec![255u8; (WIDTH * HEIGHT * 4) as usize];
        for i in 0..(WIDTH * HEIGHT) as usize {
            base[i * 4..i * 4 + 3].copy_from_slice(&[10, 20, 30]);
        }
        let grid = render_png("T", &points, &[], &[], None, MapKind::Day).unwrap();
        let based = render_png("T", &points, &[], &[], Some(&base), MapKind::Day).unwrap();
        assert!(based.len() >= MIN_PNG_BYTES);
        assert_ne!(grid, based, "basemap must visibly change the render");
        // A wrong-sized basemap is ignored rather than corrupting the render.
        let wrong = render_png("T", &points, &[], &[], Some(&base[..16]), MapKind::Day).unwrap();
        assert_eq!(grid, wrong);
    }

    #[test]
    fn basemap_bbox_covers_full_canvas_in_mercator_meters() {
        let points = vec![
            Point { lat: 35.0, lon: 135.0, color: [0, 0, 0], kind: Kind::Sightseeing, label: String::new() },
            Point { lat: 35.1, lon: 135.2, color: [0, 0, 0], kind: Kind::Sightseeing, label: String::new() },
        ];
        let p = map_projection(&points, &[]);
        let (min_x, min_y, max_x, max_y) = basemap_bbox(&p);
        // Symmetric around the projection centre (min_x/max_y fields hold the centre).
        let cx = (min_x + max_x) / 2.0;
        let cy = (min_y + max_y) / 2.0;
        assert!((cx - p.min_x * WEB_MERCATOR_R).abs() < 1e-4, "cx {cx}");
        assert!((cy - p.max_y * WEB_MERCATOR_R).abs() < 1e-4, "cy {cy}");
        // Span = full canvas in Mercator units × sphere radius.
        let span_x = (max_x - min_x) - WIDTH as f64 / p.scale * WEB_MERCATOR_R;
        let span_y = (max_y - min_y) - HEIGHT as f64 / p.scale * WEB_MERCATOR_R;
        assert!(span_x.abs() < 1e-4 && span_y.abs() < 1e-4, "{span_x} {span_y}");
    }

    #[test]
    fn decode_rgba_png_roundtrips_and_rejects_bad_bodies() {
        let pixels = vec![128u8; (WIDTH * HEIGHT * 4) as usize];
        let bytes = encode_rgba_png(&pixels, WIDTH, HEIGHT).unwrap();
        let decoded = decode_rgba_png(&bytes).expect("own encoder output must decode");
        assert_eq!(decoded, pixels);
        // ArcGIS error bodies are JSON (HTTP 200), HTML, or empty — all rejected.
        assert!(decode_rgba_png(b"{\"error\":{\"code\":400}}").is_none());
        assert!(decode_rgba_png(b"").is_none());
        // Wrong-sized images are rejected (no rescaling — alignment would break).
        let small = encode_rgba_png(&vec![0u8; 100 * 100 * 4], 100, 100).unwrap();
        assert!(decode_rgba_png(&small).is_none());
    }

    fn poi(id: &str, title: &str, lat: f64, lon: f64) -> PoiRow {
        PoiRow {
            poi_id: id.into(),
            title: title.into(),
            lat,
            lon,
        }
    }

    #[test]
    fn match_poi_uses_id_and_title_keys() {
        let pois = vec![
            poi("kozanji", "Kozanji Temple (高山寺)", 35.06000, 135.58000),
            poi("nonomiya", "Nonomiya Shrine (野宮神社)", 35.01500, 135.67000),
            poi(
                "nishi-hongwanji",
                "Nishi Hongwanji (西本願寺)",
                34.99100,
                135.75100,
            ),
            poi(
                "arashiyama-bamboo-grove",
                "Arashiyama Bamboo Grove",
                35.01700,
                135.67200,
            ),
        ];
        assert_eq!(
            match_poi("Kozanji", &pois),
            Some((35.06000, 135.58000))
        );
        assert_eq!(match_poi("高山寺", &pois), Some((35.06000, 135.58000)));
        assert_eq!(
            match_poi("Nonomiya Shrine", &pois),
            Some((35.01500, 135.67000))
        );
        assert_eq!(
            match_poi("Nishi Hongwanji", &pois),
            Some((34.99100, 135.75100))
        );
        assert!(match_poi("Gion-Shirakawa", &pois).is_none());
        assert!(
            match_poi("Bamboo Grove", &pois).is_none(),
            "substring must not match"
        );
    }

    #[test]
    fn match_poi_ambiguous_coords_are_none() {
        let pois = vec![
            poi("a", "Foo", 1.0, 1.0),
            poi("b", "Foo", 2.0, 2.0),
        ];
        assert!(match_poi("Foo", &pois).is_none());
    }

    #[test]
    fn match_hotel_uses_parenthetical_ascii_name() {
        let hotels = vec!["京都哈頓飯店 (Hearton Hotel Kyoto)".to_string()];
        assert_eq!(
            match_hotel("Hearton Hotel Kyoto", &hotels).as_deref(),
            Some("京都哈頓飯店 (Hearton Hotel Kyoto)")
        );
        assert!(match_hotel("APA HOTEL KYOTO EKIHIGASHI", &hotels).is_none());
    }

    #[test]
    fn match_hotel_generic_token_resolves_when_single_hotel() {
        // Route segments use a bare "hotel" endpoint; with exactly one candidate the
        // pin (and its legend label) must resolve to that hotel, not to a Nominatim
        // "hotel, <city>" first-hit.
        let one = vec!["APA Hotel Kyoto Eki Higashi".to_string()];
        assert_eq!(match_hotel("hotel", &one).as_deref(), Some("APA Hotel Kyoto Eki Higashi"));
        assert_eq!(match_hotel("飯店", &one).as_deref(), Some("APA Hotel Kyoto Eki Higashi"));
        // Ambiguous (two hotels) or non-generic labels keep the old exact-key rules.
        let two = vec!["APA Hotel".to_string(), "Miyako Hotel".to_string()];
        assert!(match_hotel("hotel", &two).is_none());
        assert!(match_hotel("金閣寺", &one).is_none());
    }

    #[test]
    fn display_label_prefers_matched_hotel_full_name() {
        let hotels = vec!["京都哈頓飯店 (Hearton Hotel Kyoto)".to_string()];
        assert_eq!(display_label("hotel", &hotels), "京都哈頓飯店 (Hearton Hotel Kyoto)");
        // Non-hotel labels pass through verbatim — the legend must speak the
        // itinerary's own naming (segment label), not a re-derived one.
        assert_eq!(display_label("金閣寺", &hotels), "金閣寺");
    }

    #[test]
    fn input_sha_ignores_labels_so_a_rename_needs_no_rerender() {
        // The PNG carries no labels; only the legend table does. A label-only edit
        // must not change the reuse hash (it refreshes legend rows on the keep path).
        let base = Point {
            lat: 35.0,
            lon: 135.0,
            color: [1, 2, 3],
            kind: Kind::Sightseeing,
            label: "金閣寺".into(),
        };
        let renamed = Point { label: "Kinkaku-ji".into(), ..base.clone() };
        assert_eq!(
            map_input_sha256("Day 1 route", MapKind::Day, &[base], &[]),
            map_input_sha256("Day 1 route", MapKind::Day, &[renamed], &[])
        );
    }

    #[test]
    fn map_input_sha256_is_deterministic_and_sensitive() {
        let points = vec![
            Point {
                lat: 35.0,
                lon: 135.0,
                color: [1, 2, 3],
                kind: Kind::Sightseeing,
                label: String::new(),
            },
            Point {
                lat: 35.1,
                lon: 135.2,
                color: [4, 5, 6],
                kind: Kind::Hotel,
                label: String::new(),
            },
        ];
        let routes = vec![RouteLine {
            points: vec![(35.0, 135.0), (35.1, 135.2)],
            routed: true,
            color: [7, 8, 9],
        }];
        let a = map_input_sha256("Day 1 route", MapKind::Day, &points, &routes);
        let b = map_input_sha256("Day 1 route", MapKind::Day, &points, &routes);
        assert_eq!(a, b);
        let mut moved = points.clone();
        moved[0].lat = 35.01;
        let c = map_input_sha256("Day 1 route", MapKind::Day, &moved, &routes);
        assert_ne!(a, c);
    }
}
