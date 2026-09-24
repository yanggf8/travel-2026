//! `travel snapshot-maps` — query Turso, render route diagrams in Rust, and upload PNGs to R2.
//!
//! The renderer prefers destination POI coordinates over Nominatim, then uses itinerary
//! endpoints, cached Nominatim coordinates, and cached OSRM road geometry. It never fetches
//! raster map tiles; automated/headless tile downloads and saved tile composites are
//! prohibited by the OpenStreetMap tile policy. OSM-derived data is credited in every
//! generated image.

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
}

#[derive(Clone)]
struct RouteLine {
    points: Vec<(f64, f64)>,
    routed: bool,
    color: [u8; 3],
}

#[derive(Clone, Debug)]
struct PoiRow {
    poi_id: String,
    title: String,
    lat: f64,
    lon: f64,
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

    let mut cache = load_geocodes(&read).await?;
    let segments = query_segments(&read, &plan_id, &dest).await?;
    let pois = query_pois(&read, &plan_id, &dest).await?;
    let dest_pois = query_destination_pois(&read, &dest).await?;
    let hotel_names = query_hotel_names(&read, &plan_id, &dest).await?;
    let mut day_points = HashMap::<i64, Vec<Point>>::new();
    let mut day_routes = HashMap::<i64, Vec<RouteLine>>::new();

    for day in &days {
        let mut points = Vec::new();
        let mut seen = HashSet::new();
        for seg in segments.iter().filter(|s| s.day == *day) {
            for label in [&seg.from, &seg.to] {
                if label.trim().is_empty() {
                    continue;
                }
                if let Some((lat, lon, kind)) = resolve_segment_label(
                    &read,
                    &write,
                    label,
                    &context,
                    &mut cache,
                    &dest_pois,
                    &hotel_names,
                )
                .await?
                {
                    if seen.insert(coord_key(lat, lon)) {
                        points.push(Point {
                            lat,
                            lon,
                            color: DAY_COLORS[(*day as usize - 1) % DAY_COLORS.len()],
                            kind,
                        });
                    }
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
                    });
                }
            }
        }
        if points.len() > 1 {
            day_routes.insert(*day, cached_routes(&read, &points).await?);
        }
        day_points.insert(*day, points);
    }

    let mut required_ok = true;
    let mut all_sightseeing = Vec::new();
    for day in &days {
        let pts = day_points.get(day).cloned().unwrap_or_default();
        if pts.is_empty() {
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
            let routes = day_routes.get(day).cloned().unwrap_or_default();
            if !upload_map(
                &write,
                &plan_id,
                &format!("day-{day}.png"),
                &format!("Day {day} route"),
                &pts,
                &routes,
                MapKind::Day,
            )
            .await?
            {
                required_ok = false;
            }
        }
        for p in pts.into_iter().filter(|p| p.kind == Kind::Sightseeing) {
            if !all_sightseeing
                .iter()
                .any(|existing: &Point| existing.lat == p.lat && existing.lon == p.lon)
            {
                all_sightseeing.push(p);
            }
        }
    }

    if all_sightseeing.is_empty() {
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
        for day in &days {
            let points = day_points
                .get(day)
                .into_iter()
                .flatten()
                .filter(|p| p.kind == Kind::Sightseeing)
                .cloned()
                .collect::<Vec<_>>();
            if points.len() > 1 {
                routes.extend(cached_routes(&read, &points).await?);
            }
        }
        if !upload_map(
            &write,
            &plan_id,
            "plan.png",
            "Sightseeing overview",
            &all_sightseeing,
            &routes,
            MapKind::Plan,
        )
        .await?
        {
            required_ok = false;
        }
    }

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
        write.execute(
            "INSERT INTO plan_map_snapshots (plan_id, snapshotted_at) VALUES (?1, datetime('now')) ON CONFLICT(plan_id) DO UPDATE SET snapshotted_at=datetime('now')",
            params![plan_id.clone()],
        ).await.map_err(|e| format!("map freshness stamp failed: {e}"))?;
        println!("snapshot-maps: completed {plan_id} ({dest}); no raster map tiles requested");
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
    let roads = fetch_road_web(padded_bounds(points, routes)).await;
    if roads.is_empty() && previous_map_reusable(conn, plan, key, &input_sha).await? {
        println!(
            "   kept {key} (road web unavailable; previous map has same stops + roads)"
        );
        return Ok(true);
    }
    let png = render_png(title, points, routes, &roads, kind)?;
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
    let has_roads = if roads.is_empty() { 0 } else { 1 };
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
    if roads.is_empty() {
        println!(
            "   warn: {key} uploaded WITHOUT road web (Overpass unavailable); re-run snapshot-maps later"
        );
    }
    println!("   uploaded {key} ({} bytes)", png.len());
    Ok(true)
}

fn map_kind_tag(kind: MapKind) -> &'static str {
    match kind {
        MapKind::Day => "day",
        MapKind::Plan => "plan",
        MapKind::Logistics => "logistics",
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
    kind: MapKind,
) -> Result<Vec<u8>, String> {
    if points.is_empty() {
        return Err("cannot render a map without points".into());
    }
    let mut raster = Raster::new(WIDTH as usize, HEIGHT as usize);
    raster.grid();
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
    let mut bounds = xy.clone();
    for line in &route_xy {
        bounds.extend(line.iter().copied());
    }
    let projection = Projection::fit(&bounds, WIDTH as f64, HEIGHT as f64);
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
    if routes.is_empty() && !matches!(kind, MapKind::Logistics) {
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
    raster.text(12, 10, &title.to_ascii_uppercase(), [45, 61, 75], 2);
    if matches!(kind, MapKind::Logistics) {
        raster.legend(12, 31, "HOTEL", [21, 101, 192]);
        raster.legend(105, 31, "AIRPORT", [239, 108, 0]);
    }
    raster.text(
        370,
        HEIGHT as i32 - 14,
        "© OPENSTREETMAP CONTRIBUTORS",
        [70, 82, 94],
        1,
    );
    let mut out = Cursor::new(Vec::new());
    let mut encoder = Encoder::new(&mut out, WIDTH, HEIGHT);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("PNG header failed: {e}"))?;
    writer
        .write_image_data(&raster.pixels)
        .map_err(|e| format!("PNG encoding failed: {e}"))?;
    drop(writer);
    Ok(out.into_inner())
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

/// One cached ok leg's endpoints (geometry points are pulled per matched key).
struct LegRow {
    key: String,
    from: (f64, f64),
    to: (f64, f64),
}

/// Nominatim answers drift between the leg-fetch run and the render run (limit=1
/// feature choice moves a stop by hundreds of metres), so an exact 5-dp key match
/// misses and every leg falls back to a straight dashed connector — the "empty map"
/// regression. A cached leg therefore also matches when BOTH endpoints sit within
/// MATCH_RADIUS_M of the stop pair; the closest such leg wins. Exact key first.
const MATCH_RADIUS_M: f64 = 500.0;

fn match_leg<'a>(legs: &'a [LegRow], from: (f64, f64), to: (f64, f64)) -> Option<&'a LegRow> {
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

async fn cached_routes(c: &Connection, points: &[Point]) -> Result<Vec<RouteLine>, String> {
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
    let mut r=c.query("SELECT day_number, from_place, to_place FROM day_route_segments WHERE plan_id=?1 AND destination=?2 ORDER BY day_number, sort_order",params![p.to_string(),d.to_string()]).await.map_err(err("route segments query"))?;
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
async fn query_destination_pois(c: &Connection, d: &str) -> Result<Vec<PoiRow>, String> {
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
async fn geocode_context(c: &Connection, d: &str) -> Result<String, String> {
    let mut r = c
        .query(
            "SELECT display_name FROM destination_config WHERE slug=?1 LIMIT 1",
            params![d.to_string()],
        )
        .await
        .map_err(err("destination context query"))?;
    let display = if let Some(x) = r.next().await.map_err(err("destination context read"))? {
        x.get::<Option<String>>(0)
            .ok()
            .flatten()
            .unwrap_or_default()
    } else {
        String::new()
    };
    if d.to_ascii_lowercase().contains("kyoto") {
        Ok("Kyoto, Japan".into())
    } else if !display.is_empty() {
        Ok(format!("{display}, Japan"))
    } else {
        Ok("Japan".into())
    }
}
async fn load_geocodes(c: &Connection) -> Result<HashMap<String, (f64, f64)>, String> {
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

fn match_poi(label: &str, pois: &[PoiRow]) -> Option<(f64, f64)> {
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
    if hits.len() == 1 { hits.pop() } else { None }
}

async fn resolve_place(
    read: &Connection,
    write: &Connection,
    place: &str,
    context: &str,
    cache: &mut HashMap<String, (f64, f64)>,
) -> Result<Option<(f64, f64)>, String> {
    let key = place.trim().to_ascii_lowercase();
    if let Some(p) = cache.get(&key) {
        return Ok(Some(*p));
    }
    let (search, ctx) = normalize_place(place, context);
    let query_key = format!("{}|{}", search.trim().to_ascii_lowercase(), ctx);
    let mut r = read
        .query(
            "SELECT lat, lon FROM route_place_geocodes WHERE query_key=?1",
            params![query_key.clone()],
        )
        .await
        .map_err(err("geocode cache lookup"))?;
    if let Some(row) = r.next().await.map_err(err("geocode cache lookup read"))? {
        if let (Ok(Some(lat)), Ok(Some(lon))) =
            (row.get::<Option<f64>>(0), row.get::<Option<f64>>(1))
        {
            cache.insert(key, (lat, lon));
            return Ok(Some((lat, lon)));
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
    let osm_id = item
        .get("osm_id")
        .and_then(|v| v.as_i64())
        .map(|n| n.to_string())
        .unwrap_or_default();
    let osm_type = item.get("osm_type").and_then(|v| v.as_str()).unwrap_or("");
    let now = chrono::Utc::now().to_rfc3339();
    write.execute("INSERT INTO route_place_geocodes (query_key,raw_place,lat,lon,display_name,osm_id,osm_type,provider,confidence,review,failure_reason,fetched_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'nominatim','ok',0,NULL,?8) ON CONFLICT(query_key) DO UPDATE SET raw_place=excluded.raw_place,lat=excluded.lat,lon=excluded.lon,display_name=excluded.display_name,osm_id=excluded.osm_id,osm_type=excluded.osm_type,provider='nominatim',confidence='ok',failure_reason=NULL,fetched_at=excluded.fetched_at",params![query_key,place.to_string(),lat,lon,display.to_string(),osm_id,osm_type.to_string(),now]).await.map_err(err("geocode cache write"))?;
    cache.insert(key, (lat, lon));
    Ok(Some((lat, lon)))
}
fn normalize_place(place: &str, context: &str) -> (String, String) {
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
        p => {
            if let Some(open) = p.find('(') {
                if let Some(len) = p[open + 1..].find(')') {
                    let inner = p[open + 1..open + 1 + len].trim();
                    if !inner.is_empty()
                        && inner.chars().all(|c| {
                            c.is_ascii_alphanumeric()
                                || matches!(c, ' ' | '&' | '-' | '.' | '\'')
                        })
                    {
                        return (inner.to_string(), context.to_string());
                    }
                }
            }
            (p.to_string(), context.to_string())
        }
    }
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
        || matches!(s.trim(), "KIX" | "TPE" | "NRT" | "HND" | "ITM")
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

async fn record_artifact(
    c: &Connection,
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
        "Usage:\n  travel snapshot-maps [--dest <slug>]\n\nRenders route diagrams directly in Rust from Turso itinerary and cached place data, uploads PNGs to the dashboard R2 bucket, and stamps map freshness. It does not fetch raster map tiles. Requires Wrangler authentication."
    );
}
fn err<T: std::fmt::Display>(ctx: &'static str) -> impl FnOnce(T) -> String {
    move |e| format!("{ctx}: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leg(key: &str, from: (f64, f64), to: (f64, f64)) -> LegRow {
        LegRow {
            key: key.into(),
            from,
            to,
        }
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
            Point { lat: 35.0, lon: 135.0, color: [0, 0, 0], kind: Kind::Sightseeing },
            Point { lat: 35.1, lon: 135.2, color: [0, 0, 0], kind: Kind::Sightseeing },
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
    fn parse_overpass_ways_remark_empty_is_retryable() {
        // Server-side timeout: empty elements + remark → None (retry), unlike a
        // clean empty bbox which is Some(empty).
        let body = br#"{"elements":[],"remark":"runtime error: query timed out"}"#;
        assert!(parse_overpass_ways(body).is_none());
    }

    #[test]
    fn render_png_with_road_web_changes_output() {
        let points = vec![
            Point { lat: 35.0, lon: 135.0, color: [200, 50, 120], kind: Kind::Sightseeing },
            Point { lat: 35.01, lon: 135.01, color: [200, 50, 120], kind: Kind::Sightseeing },
        ];
        let roads = vec![RoadWay {
            points: vec![(35.0, 135.0), (35.005, 135.008), (35.01, 135.01)],
            major: true,
        }];
        let bare = render_png("T", &points, &[], &[], MapKind::Day).unwrap();
        let with = render_png("T", &points, &[], &roads, MapKind::Day).unwrap();
        assert!(bare.len() >= MIN_PNG_BYTES && with.len() >= MIN_PNG_BYTES);
        assert_ne!(bare, with, "road web must visibly change the render");
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
    fn map_input_sha256_is_deterministic_and_sensitive() {
        let points = vec![
            Point {
                lat: 35.0,
                lon: 135.0,
                color: [1, 2, 3],
                kind: Kind::Sightseeing,
            },
            Point {
                lat: 35.1,
                lon: 135.2,
                color: [4, 5, 6],
                kind: Kind::Hotel,
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
