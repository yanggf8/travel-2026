//! `travel snapshot-maps` — query Turso, render route diagrams in Rust, and upload PNGs to R2.
//!
//! The renderer uses itinerary endpoints, cached Nominatim coordinates, and cached OSRM road
//! geometry. It never fetches raster map tiles; automated/headless tile downloads and saved tile
//! composites are prohibited by the OpenStreetMap tile policy. OSM-derived data is credited in
//! every generated image.

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
                if let Some((lat, lon)) =
                    resolve_place(&read, &write, label, &context, &mut cache).await?
                {
                    if seen.insert(coord_key(lat, lon)) {
                        points.push(Point {
                            lat,
                            lon,
                            color: DAY_COLORS[(*day as usize - 1) % DAY_COLORS.len()],
                            kind: classify(label),
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

    let logistics = await_logistics(&read, &write, &plan_id, &dest, &context, &mut cache).await?;
    if logistics.is_empty() {
        record_artifact(
            &write,
            &plan_id,
            "plan-logistics.png",
            0,
            None,
            "skipped",
            Some("no geocoded hotel/airport endpoints"),
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
    let png = render_png(title, points, routes, kind)?;
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
    if !status.success() {
        record_artifact(
            conn,
            plan,
            key,
            png.len(),
            Some(&sha),
            "failed",
            Some("Wrangler upload failed"),
        )
        .await?;
        eprintln!("   FAIL {key}: Wrangler exited with {status}");
        return Ok(false);
    }
    record_artifact(conn, plan, key, png.len(), Some(&sha), "uploaded", None).await?;
    println!("   uploaded {key} ({} bytes)", png.len());
    Ok(true)
}

fn render_png(
    title: &str,
    points: &[Point],
    routes: &[RouteLine],
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

async fn cached_routes(c: &Connection, points: &[Point]) -> Result<Vec<RouteLine>, String> {
    let mut routes = Vec::new();
    for pair in points.windows(2) {
        let key = format!(
            "{:.5},{:.5}>{:.5},{:.5}|osrm-demo|driving",
            pair[0].lat, pair[0].lon, pair[1].lat, pair[1].lon
        );
        let mut rows = c
            .query(
                "SELECT p.lat, p.lon FROM route_road_leg_points p \
             JOIN route_road_legs l ON l.leg_key=p.leg_key \
             WHERE p.leg_key=?1 AND l.status='ok' ORDER BY p.point_order",
                params![key],
            )
            .await
            .map_err(err("cached road geometry query"))?;
        let mut road = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(err("cached road geometry read"))?
        {
            if let (Ok(lat), Ok(lon)) = (row.get::<f64>(0), row.get::<f64>(1)) {
                road.push((lat, lon));
            }
        }
        if road.len() > 1 {
            routes.push(RouteLine {
                points: road,
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
        p => (p.to_string(), context.to_string()),
    }
}

async fn await_logistics(
    read: &Connection,
    write: &Connection,
    p: &str,
    d: &str,
    ctx: &str,
    cache: &mut HashMap<String, (f64, f64)>,
) -> Result<Vec<Point>, String> {
    let mut labels = Vec::<(String, Kind)>::new();
    let mut r = read
        .query(
            "SELECT name FROM hotels WHERE plan_id=?1 AND destination=?2",
            params![p.to_string(), d.to_string()],
        )
        .await
        .map_err(err("hotel map query"))?;
    while let Some(x) = r.next().await.map_err(err("hotel map read"))? {
        if let Ok(name) = x.get::<String>(0) {
            labels.push((name, Kind::Hotel));
        }
    }
    let mut f=read.query("SELECT departure_code, arrival_code, departure_airport, arrival_airport, flight_number FROM flight_legs WHERE plan_id=?1 AND destination=?2 ORDER BY direction,leg_order",params![p.to_string(),d.to_string()]).await.map_err(err("flight map query"))?;
    while let Some(x) = f.next().await.map_err(err("flight map read"))? {
        for i in 0..4 {
            if let Ok(Some(s)) = x.get::<Option<String>>(i) {
                if !s.trim().is_empty() {
                    labels.push((s, Kind::Airport));
                }
            }
        }
        if let Ok(flight) = x.get::<String>(4) {
            for pair in flight.split_whitespace() {
                if let Some((a, b)) = pair.split_once('-') {
                    if a.len() == 3
                        && b.len() == 3
                        && a.chars().all(|c| c.is_ascii_uppercase())
                        && b.chars().all(|c| c.is_ascii_uppercase())
                    {
                        labels.push((a.into(), Kind::Airport));
                        labels.push((b.into(), Kind::Airport));
                    }
                }
            }
        }
    }
    let mut r=read.query("SELECT from_place,to_place FROM day_route_segments WHERE plan_id=?1 AND destination=?2",params![p.to_string(),d.to_string()]).await.map_err(err("logistics route query"))?;
    while let Some(x) = r.next().await.map_err(err("logistics route read"))? {
        for i in 0..2 {
            if let Ok(s) = x.get::<String>(i) {
                let k = classify(&s);
                if k != Kind::Sightseeing {
                    labels.push((s, k));
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (label, kind) in labels {
        if let Some((lat, lon)) = resolve_place(read, write, &label, ctx, cache).await? {
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
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    c.execute("INSERT INTO map_artifacts (plan_id,map_key,byte_size,sha256,status,skip_reason,generated_at) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(plan_id,map_key) DO UPDATE SET byte_size=excluded.byte_size,sha256=excluded.sha256,status=excluded.status,skip_reason=excluded.skip_reason,generated_at=excluded.generated_at",params![p.to_string(),key.to_string(),size as i64,sha.unwrap_or_default().to_string(),status.to_string(),reason.map(str::to_string),now]).await.map_err(err("map manifest write"))?;
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
