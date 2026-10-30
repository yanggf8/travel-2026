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

pub(crate) const WIDTH: u32 = 640;
pub(crate) const HEIGHT: u32 = 440;
const MIN_PNG_BYTES: usize = 200;
pub(crate) const BUCKET: &str = "trip-dashboard-maps";
/// The no-basemap canvas: a flat light fill crossed by a 32 px grid. verify-site
/// reads "only these colors" as "no geographic background".
pub(crate) const GRID_BG: [u8; 3] = [247, 249, 251];
pub(crate) const GRID_LINE: [u8; 3] = [229, 234, 238];
/// Overpass road-web colors (major = motorway/trunk/primary, minor =
/// secondary/tertiary), drawn under everything when ArcGIS is unavailable.
pub(crate) const ROAD_MAJOR: [u8; 3] = [186, 196, 208];
pub(crate) const ROAD_MINOR: [u8; 3] = [222, 228, 234];
/// Title ink and scale; the title strip is `title_rect`.
const TITLE_INK: [u8; 3] = [45, 61, 75];
pub(crate) const TITLE_SCALE: i32 = 2;
/// Credit ink; the credit strip is `CREDIT_RECT` (scale 1).
const CREDIT_INK: [u8; 3] = [70, 82, 94];
/// Reserved bottom-right credit strip (x0, y0, x1, y1): kept clear of displaced
/// pins and backed white over a basemap.
pub(crate) const CREDIT_RECT: (i32, i32, i32, i32) = (366, HEIGHT as i32 - 18, 584, HEIGHT as i32 - 4);
/// The two credit lines `render_png` draws: with and without an Esri basemap.
pub(crate) const CREDIT_WITH_BASEMAP: &str = "© ESRI + OPENSTREETMAP CONTRIBUTORS";
pub(crate) const CREDIT_NO_BASEMAP: &str = "© OPENSTREETMAP CONTRIBUTORS";
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
pub(crate) struct Point {
    lat: f64,
    lon: f64,
    color: [u8; 3],
    kind: Kind,
    /// Itinerary day a sightseeing pin belongs to (its DAY_COLORS color is
    /// derived from it); None for logistics endpoints. Drives the overview's
    /// DAY legend — the color cycle repeats after 7 days, so the color alone
    /// cannot name the day.
    day: Option<u32>,
    /// Legend name for this pin. Rendered by the dashboard worker from
    /// `map_legend_stops` (the PNG bitmap font has no CJK glyphs, so the
    /// number→name mapping cannot be drawn into the image itself).
    label: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
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
pub(crate) struct RouteLine {
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
    // The SAME assembly verify-site recomputes read-only (NetMode::CacheOnly), so the
    // input_sha256 recorded here and the one verify-site compares against cannot drift.
    // Every map is assembled before any upload: an assembly error aborts the run
    // with the live set untouched (all old), never half re-uploaded.
    let source = MapSource::load(&read, &plan_id, &dest).await?;
    let mut net = DbResolver::new(&read, NetMode::Fetch(&write), &dest).await?;
    let maps = assemble_maps(&source, &mut net, &plan_id, &dest).await?;

    let mut required_ok = true;
    for map in &maps {
        match &map.plan {
            MapPlan::Skip { reason, note } => {
                // The worker renders any PNG it finds, so a map that is no longer
                // produced must have its earlier upload removed, not just marked skipped.
                remove_stale_map(&write, &plan_id, &map.key, reason).await?;
                if let Some(note) = note {
                    println!("{note}");
                }
            }
            MapPlan::Render {
                title,
                kind,
                points,
                routes,
            } => {
                if !upload_map(&write, &plan_id, &map.key, title, points, routes, *kind).await? {
                    required_ok = false;
                }
            }
        }
    }

    let next = format!("next: travel verify-site --plan-id {plan_id}");
    if required_ok {
        crate::db::connect_write().await?.execute(
            "INSERT INTO plan_map_snapshots (plan_id, snapshotted_at) VALUES (?1, datetime('now')) ON CONFLICT(plan_id) DO UPDATE SET snapshotted_at=datetime('now')",
            params![plan_id.clone()],
        ).await.map_err(|e| format!("map freshness stamp failed: {e}"))?;
        println!("snapshot-maps: completed {plan_id} ({dest}); no OSM raster tiles requested");
        println!("{next}");
        Ok(())
    } else {
        println!("{next}");
        Err(format!(
            "one or more map uploads failed for {plan_id}; freshness was not stamped"
        ))
    }
}

/// Everything the map assembly reads from Turso for one plan + destination,
/// loaded up front so `assemble_maps` itself only resolves places and roads.
pub(crate) struct MapSource {
    days: Vec<i64>,
    excursion: HashSet<i64>,
    segments: Vec<Segment>,
    pois: Vec<(i64, String, f64, f64)>,
    dest_pois: Vec<PoiRow>,
    hotel_names: Vec<String>,
    flights: Vec<FlightRow>,
    /// `day_route_segments` endpoints in the logistics query's own row order
    /// (that query has no ORDER BY; its order decides the logistics pin order).
    route_endpoints: Vec<String>,
    /// Domestic (Taiwan) booked stay's hotel name, if any.
    domestic_stay: Option<String>,
    /// The destination hub (`destination_config.lat/lon`); None when either is
    /// NULL, which turns the MAX_HUB_DISTANCE_KM gate off.
    hub: Option<(f64, f64)>,
}

/// One `flight_legs` row as the logistics map reads it.
struct FlightRow {
    /// Non-empty departure/arrival code + airport columns, in column order.
    labels: Vec<String>,
    /// `XXX-YYY` code pairs parsed from `flight_number`, in order.
    codes: Vec<String>,
}

impl MapSource {
    pub(crate) async fn load(read: &Connection, plan_id: &str, dest: &str) -> Result<Self, String> {
        let days = query_days(read, plan_id).await?;
        if days.is_empty() {
            return Err(format!("no itinerary days for plan {plan_id}"));
        }
        // Far-off day trips (set-day-excursion) stay off the sightseeing overview and
        // get their own inset map — a 100 km coach tour otherwise flattens the city.
        let excursion = travel_db::repo::itinerary::excursion_days(read, plan_id, dest)
            .await?
            .into_iter()
            .collect();
        Ok(Self {
            days,
            excursion,
            segments: query_segments(read, plan_id, dest).await?,
            pois: query_pois(read, plan_id, dest).await?,
            dest_pois: query_destination_pois(read, dest).await?,
            hotel_names: query_hotel_names(read, plan_id, dest).await?,
            flights: query_flight_rows(read, plan_id, dest).await?,
            route_endpoints: query_route_endpoints(read, plan_id, dest).await?,
            domestic_stay: query_domestic_stay(read, plan_id, dest)
                .await?
                .map(|(_title, hotel)| hotel),
            hub: query_destination_hub(read, dest).await?,
        })
    }
}

/// The network side of the map assembly: place geocoding and road geometry.
/// snapshot-maps resolves through Turso caches with network write-through
/// (`NetMode::Fetch`); verify-site resolves through the same caches only.
pub(crate) trait MapResolver {
    /// Coordinates for a place label; None when it does not resolve.
    async fn geocode(&mut self, place: &str) -> Result<Option<(f64, f64)>, String>;
    /// Road-following lines for consecutive stop pairs.
    async fn routes(&mut self, points: &[Point]) -> Result<Vec<RouteLine>, String>;
    /// Print snapshot-maps' stderr diagnostics (unresolved stops, implausible
    /// legs, excursion hints).
    fn diagnostics(&self) -> bool;
    /// Labels with no geocode cache row recorded since the last call (cache-only
    /// mode; always empty when misses are fetched).
    fn take_misses(&mut self) -> Vec<String>;
}

/// How `DbResolver` handles a cache miss.
#[derive(Clone, Copy)]
pub(crate) enum NetMode<'a> {
    /// snapshot-maps: fetch the miss (Nominatim / OSRM) and write it through.
    Fetch(&'a Connection),
    /// verify-site: read-only. A geocode miss is recorded and left unresolved; a
    /// road-leg miss draws the straight connector — exactly what snapshot-maps
    /// draws (and hashes) when its own OSRM fetch fails, and a leg it fetched
    /// successfully is in the cache, so the recomputed hash is the recorded one.
    CacheOnly,
}

pub(crate) struct DbResolver<'a> {
    read: &'a Connection,
    mode: NetMode<'a>,
    context: String,
    cache: HashMap<String, (f64, f64)>,
    /// OSRM legs fetched this run, shared by the per-day and overview renders so
    /// the same pair is fetched (and persisted) at most once per snapshot run.
    osrm_live: HashMap<String, Vec<(f64, f64)>>,
    misses: Vec<String>,
}

impl<'a> DbResolver<'a> {
    pub(crate) async fn new(read: &'a Connection, mode: NetMode<'a>, dest: &str) -> Result<Self, String> {
        Ok(Self {
            read,
            mode,
            context: geocode_context(read, dest).await?,
            cache: load_geocodes(read).await?,
            osrm_live: HashMap::new(),
            misses: Vec::new(),
        })
    }
}

impl MapResolver for DbResolver<'_> {
    async fn geocode(&mut self, place: &str) -> Result<Option<(f64, f64)>, String> {
        match lookup_place_cache(self.read, place, &self.context, &mut self.cache).await? {
            CacheLookup::Hit(p) => Ok(Some(p.coords)),
            CacheLookup::Unresolvable => Ok(None),
            CacheLookup::Miss => match self.mode {
                NetMode::Fetch(write) => Ok(fetch_nominatim(write, place, &self.context, &mut self.cache)
                    .await?
                    .map(|p| p.coords)),
                NetMode::CacheOnly => {
                    if !self.misses.iter().any(|m| m == place) {
                        self.misses.push(place.to_string());
                    }
                    Ok(None)
                }
            },
        }
    }

    async fn routes(&mut self, points: &[Point]) -> Result<Vec<RouteLine>, String> {
        let write = match self.mode {
            NetMode::Fetch(w) => Some(w),
            NetMode::CacheOnly => None,
        };
        cached_routes(self.read, write, &mut self.osrm_live, points).await
    }

    fn diagnostics(&self) -> bool {
        matches!(self.mode, NetMode::Fetch(_))
    }

    fn take_misses(&mut self) -> Vec<String> {
        std::mem::take(&mut self.misses)
    }
}

/// One map key's planned outcome for the current itinerary.
pub(crate) struct PlannedMap {
    pub(crate) key: String,
    plan: MapPlan,
    /// Geocode cache misses among this map's inputs (cache-only mode): its input
    /// hash cannot be reproduced without the network.
    pub(crate) geocode_misses: Vec<String>,
    /// Resolved stops among this map's inputs kept off it by the hub gate
    /// (beyond MAX_HUB_DISTANCE_KM); a day's own go to its `day-{n}-away.png`.
    off_area: Vec<OffAreaStop>,
}

enum MapPlan {
    Render {
        title: String,
        kind: MapKind,
        points: Vec<Point>,
        routes: Vec<RouteLine>,
    },
    /// `note` is the stdout line snapshot-maps prints for the skip (None = silent).
    Skip {
        reason: &'static str,
        note: Option<String>,
    },
}

impl PlannedMap {
    fn render(
        key: &str,
        kind: MapKind,
        points: Vec<Point>,
        routes: Vec<RouteLine>,
        misses: Vec<String>,
        off_area: Vec<OffAreaStop>,
    ) -> Self {
        Self {
            key: key.to_string(),
            plan: MapPlan::Render {
                title: map_title(key).expect("assembled map keys always have a title"),
                kind,
                points,
                routes,
            },
            geocode_misses: misses,
            off_area,
        }
    }

    fn skip(
        key: &str,
        reason: &'static str,
        note: Option<String>,
        misses: Vec<String>,
        off_area: Vec<OffAreaStop>,
    ) -> Self {
        Self {
            key: key.to_string(),
            plan: MapPlan::Skip { reason, note },
            geocode_misses: misses,
            off_area,
        }
    }

    /// The `map_artifacts.input_sha256` snapshot-maps records for this map; None
    /// when the current inputs produce no map (snapshot-maps records a skip).
    pub(crate) fn input_sha256(&self) -> Option<String> {
        match &self.plan {
            MapPlan::Render {
                title,
                kind,
                points,
                routes,
            } => Some(map_input_sha256(title, *kind, points, routes)),
            MapPlan::Skip { .. } => None,
        }
    }
}

/// Map title drawn into the PNG for a map key. The single source of the titles:
/// the assembly titles each map through it and verify-site sizes the reserved
/// title strip from it.
pub(crate) fn map_title(key: &str) -> Option<String> {
    match key {
        "plan.png" => Some("Sightseeing overview".to_string()),
        "plan-excursion.png" => Some("Day trip".to_string()),
        "plan-airport.png" => Some("Airport days".to_string()),
        "plan-logistics.png" => Some("Hotels and airports".to_string()),
        _ => {
            let rest = key.strip_prefix("day-")?;
            if let Some(n) = rest.strip_suffix("-logistics.png") {
                Some(format!("Day {} hotel round trip", n.parse::<i64>().ok()?))
            } else if let Some(n) = rest.strip_suffix("-away.png") {
                Some(format!("Day {} beyond the area", n.parse::<i64>().ok()?))
            } else {
                Some(format!("Day {} route", rest.strip_suffix(".png")?.parse::<i64>().ok()?))
            }
        }
    }
}

fn merged_distinct<T: PartialEq>(lists: impl IntoIterator<Item = Vec<T>>) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for item in lists.into_iter().flatten() {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// How far (great-circle) a resolved stop may sit from the destination hub and
/// still be drawn on the destination's maps. Okinawa day 1 starts at home in
/// Taiwan (紅樹林 淡水 → 桃園機場T2), ~630 km from the Naha hub and 1,700+ km
/// from the Honshu hubs: drawn there it stretches every bbox across the sea,
/// crushing the destination's own stops and timing out the Overpass road web.
/// In-destination outliers (Ishigaki, ~410 km from Naha) and ordinary day-trip
/// excursions (≤ ~370 km) stay well inside.
const MAX_HUB_DISTANCE_KM: f64 = 500.0;

/// A resolved stop the hub gate kept off the area maps; `km` is its rounded
/// great-circle distance from the hub.
#[derive(Clone, Debug, PartialEq)]
struct OffAreaStop {
    label: String,
    km: u32,
}

/// The single place an assembled stop becomes a `Point`: every pin on every
/// map (segment endpoints, itinerary POIs, logistics endpoints) passes through
/// `point`, so no map path can draw an off-area stop. An off-area stop is
/// recorded and its pin set aside for the day's `day-{n}-away.png` instead.
struct HubGate {
    hub: Option<(f64, f64)>,
    off_area: Vec<OffAreaStop>,
    /// Off-area pins in arrival order, one per coordinate.
    away: Vec<Point>,
}

impl HubGate {
    fn new(hub: Option<(f64, f64)>) -> Self {
        Self {
            hub,
            off_area: Vec::new(),
            away: Vec::new(),
        }
    }

    /// The pin for a resolved stop, or None when it lies beyond
    /// MAX_HUB_DISTANCE_KM of the hub (then recorded and set aside).
    fn point(&mut self, label: String, (lat, lon): (f64, f64), color: [u8; 3], kind: Kind, day: Option<u32>) -> Option<Point> {
        let point = Point {
            lat,
            lon,
            color,
            kind,
            day,
            label,
        };
        let Some(hub) = self.hub else {
            return Some(point);
        };
        let km = haversine_m(hub, (lat, lon)) / 1000.0;
        if km <= MAX_HUB_DISTANCE_KM {
            return Some(point);
        }
        let stop = OffAreaStop {
            label: point.label.clone(),
            km: km.round() as u32,
        };
        if !self.off_area.contains(&stop) {
            self.off_area.push(stop);
        }
        if !self.away.iter().any(|p| same_place(p, &point)) {
            self.away.push(point);
        }
        None
    }

    /// Off-area stops and pins gated since the last call.
    fn take(&mut self) -> (Vec<OffAreaStop>, Vec<Point>) {
        (std::mem::take(&mut self.off_area), std::mem::take(&mut self.away))
    }
}

/// Assemble every map's inputs (title, kind, pins, road lines) for one plan, in
/// the order snapshot-maps emits them: each day's local map, each day's hotel
/// round trip, each day's beyond-the-area map, then the sightseeing overview,
/// the excursion inset, the airport-day inset and the hotel/airport map. Shared
/// by snapshot-maps (renders + uploads) and verify-site (recomputes the input
/// hash read-only).
pub(crate) async fn assemble_maps<R: MapResolver>(
    src: &MapSource,
    net: &mut R,
    plan_id: &str,
    dest: &str,
) -> Result<Vec<PlannedMap>, String> {
    let days = &src.days;
    let excursion = &src.excursion;
    let mut gate = HubGate::new(src.hub);
    let mut day_points = HashMap::<i64, Vec<Point>>::new();
    let mut day_misses = HashMap::<i64, Vec<String>>::new();
    let mut day_off_area = HashMap::<i64, Vec<OffAreaStop>>::new();
    let mut day_away = HashMap::<i64, Vec<Point>>::new();
    // Per day, by coord_key: the stops that are an end of an airport leg (a
    // segment with an airport terminal at either drawn end), and the stops that
    // are an end of any other leg. Only ends that resolved AND passed the hub
    // gate count — a gated home airport (TPE) does not make its leg an airport
    // leg. POI-fallback pins (no segments) are on no leg.
    let mut day_airport_leg_stops = HashMap::<i64, HashSet<String>>::new();
    let mut day_other_leg_stops = HashMap::<i64, HashSet<String>>::new();
    // Per day, the airport legs whose two ends are both inset pins (sights or
    // terminals), in segment order: the inset draws each as its own road line,
    // so a city leg between two airport legs is never drawn as if it were one.
    let mut day_airport_legs = HashMap::<i64, Vec<(Point, Point)>>::new();

    for day in days {
        let color = DAY_COLORS[(*day as usize - 1) % DAY_COLORS.len()];
        let mut points = Vec::new();
        let mut seen = HashSet::new();
        let mut unresolved = HashSet::new();
        let mut airport_leg_stops = HashSet::new();
        let mut other_leg_stops = HashSet::new();
        let mut airport_legs: Vec<(Point, Point)> = Vec::new();
        for seg in src.segments.iter().filter(|s| s.day == *day) {
            let mut ends: [Option<Point>; 2] = [None, None];
            for (end, label) in [&seg.from, &seg.to].into_iter().enumerate() {
                if label.trim().is_empty() {
                    continue;
                }
                let resolved =
                    resolve_segment_label(net, label, &src.dest_pois, &src.hotel_names).await?;
                if resolved.is_none() && unresolved.insert(label.clone()) && net.diagnostics() {
                    eprintln!(
                        "   ⚠ day {day}: \"{label}\" could not be geocoded — it is left off the map; \
                         rename the stop or pin it with set-place-geocode"
                    );
                }
                if let Some((lat, lon, kind)) = resolved
                    && let Some(point) = gate.point(
                        display_label(label, &src.hotel_names),
                        (lat, lon),
                        color,
                        kind,
                        Some(*day as u32),
                    )
                {
                    if seen.insert(coord_key(lat, lon)) {
                        points.push(point.clone());
                    }
                    ends[end] = Some(point);
                }
            }
            let is_airport_leg = ends.iter().flatten().any(|e| e.kind == Kind::Airport);
            let leg_stops = if is_airport_leg { &mut airport_leg_stops } else { &mut other_leg_stops };
            for e in ends.iter().flatten() {
                leg_stops.insert(coord_key(e.lat, e.lon));
            }
            if is_airport_leg
                && let [Some(a), Some(b)] = &ends
                && [a, b].iter().all(|p| matches!(p.kind, Kind::Sightseeing | Kind::Airport))
                && coord_key(a.lat, a.lon) != coord_key(b.lat, b.lon)
                && !airport_legs.iter().any(|(x, y)| {
                    coord_key(x.lat, x.lon) == coord_key(a.lat, a.lon)
                        && coord_key(y.lat, y.lon) == coord_key(b.lat, b.lon)
                })
            {
                airport_legs.push((a.clone(), b.clone()));
            }
            if net.diagnostics()
                && let ([Some(a), Some(b)], Some(max_km)) =
                    (&ends, plausible_leg_km(&seg.mode, seg.duration_min))
            {
                let (a, b) = ((a.lat, a.lon), (b.lat, b.lon));
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
            for p in src.pois.iter().filter(|p| p.0 == *day) {
                if let Some(point) = gate.point(p.1.clone(), (p.2, p.3), color, Kind::Sightseeing, Some(*day as u32))
                    && seen.insert(coord_key(p.2, p.3))
                {
                    points.push(point);
                }
            }
        }
        day_points.insert(*day, points);
        day_airport_leg_stops.insert(*day, airport_leg_stops);
        day_other_leg_stops.insert(*day, other_leg_stops);
        day_airport_legs.insert(*day, airport_legs);
        day_misses.insert(*day, net.take_misses());
        let (off_area, away) = gate.take();
        day_off_area.insert(*day, off_area);
        day_away.insert(*day, away);
    }

    // An airport day is one whose own drawn stops (past the hub gate) include an
    // airport terminal — the arrival/departure days, found from the data, not
    // from the day's position. An excursion flag wins: that day stays on the
    // day-trip inset. An airport day is split by route segment, not moved
    // whole: its airport legs (a terminal at either drawn end) go to the
    // airport inset, so the far-off terminal does not stretch the overview,
    // while its city sights stay on plan.png. A stop on both kinds of leg (the
    // station the airport train reaches) is on both maps.
    let airport_days: HashSet<i64> = days
        .iter()
        .filter(|d| !excursion.contains(d))
        .filter(|d| day_points.get(d).is_some_and(|pts| pts.iter().any(|p| p.kind == Kind::Airport)))
        .copied()
        .collect();
    let on_overview = |day: &i64| !excursion.contains(day);
    let on_airport_leg = |day: &i64, p: &Point| {
        day_airport_leg_stops
            .get(day)
            .is_some_and(|s| s.contains(&coord_key(p.lat, p.lon)))
    };
    let on_other_leg = |day: &i64, p: &Point| {
        day_other_leg_stops
            .get(day)
            .is_some_and(|s| s.contains(&coord_key(p.lat, p.lon)))
    };
    // The airport inset draws an airport day's airport-leg ends (sights and
    // terminals), in segment order.
    let airport_day_points = |day: &i64| {
        day_points
            .get(day)
            .into_iter()
            .flatten()
            .filter(|p| matches!(p.kind, Kind::Sightseeing | Kind::Airport) && on_airport_leg(day, p))
            .cloned()
            .collect::<Vec<_>>()
    };
    // The overview draws a non-excursion day's sights, less (on an airport
    // day) the stops reached only by airport legs.
    let overview_day_points = |day: &i64| {
        day_points
            .get(day)
            .into_iter()
            .flatten()
            .filter(|p| p.kind == Kind::Sightseeing)
            .filter(|p| !airport_days.contains(day) || !on_airport_leg(day, p) || on_other_leg(day, p))
            .cloned()
            .collect::<Vec<_>>()
    };

    let misses_of = |day: &i64| day_misses.get(day).cloned().unwrap_or_default();
    let off_area_of = |day: &i64| day_off_area.get(day).cloned().unwrap_or_default();
    let mut maps = Vec::new();
    let mut all_sightseeing = Vec::new();
    let mut excursion_points: Vec<Point> = Vec::new();
    let mut airport_points: Vec<Point> = Vec::new();
    let mut daily_logistics = HashMap::<i64, (Vec<Point>, Vec<RouteLine>)>::new();
    for day in days {
        let all_pts = day_points.get(day).cloned().unwrap_or_default();
        let local_pts = all_pts.iter().filter(|p| p.kind == Kind::Sightseeing).cloned().collect::<Vec<_>>();
        let key = format!("day-{day}.png");
        if local_pts.is_empty() {
            maps.push(PlannedMap::skip(
                &key,
                "no mappable stops",
                Some(format!("   skipped {key} (no mappable stops)")),
                misses_of(day),
                off_area_of(day),
            ));
        } else {
            let routes = net.routes(&local_pts).await?;
            maps.push(PlannedMap::render(&key, MapKind::Day, local_pts, routes, misses_of(day), off_area_of(day)));
        }
        // Keep each day's local map zoomed to its sightseeing area. A separate
        // compact map draws the commute chain — the hotel round trip, with the
        // airport terminal as gateway on arrival/departure days (a pure
        // hotel↔airport transfer day renders as its single hop).
        let (log_points, legs) = day_logistics_legs(&all_pts);
        if !legs.is_empty() {
            let mut routes = Vec::new();
            for (a, b) in &legs {
                routes.extend(net.routes(&[a.clone(), b.clone()]).await?);
            }
            daily_logistics.insert(*day, (log_points, routes));
        }
        if airport_days.contains(day) {
            // Each airport day keeps its own inset pins (deduplicated within
            // the day only): the arrival and departure days usually share a
            // terminal, and each day's terminal pin wears that day's color and
            // number. The day's other sights still feed the overview below.
            airport_points.extend(airport_day_points(day));
        }
        let (bucket, pts) = if excursion.contains(day) {
            let sights = all_pts.into_iter().filter(|p| p.kind == Kind::Sightseeing).collect::<Vec<_>>();
            (&mut excursion_points, sights)
        } else {
            (&mut all_sightseeing, overview_day_points(day))
        };
        for p in pts {
            if !bucket
                .iter()
                .any(|existing: &Point| existing.lat == p.lat && existing.lon == p.lon)
            {
                bucket.push(p);
            }
        }
    }

    for day in days {
        let key = format!("day-{day}-logistics.png");
        if let Some((points, routes)) = daily_logistics.remove(day) {
            maps.push(PlannedMap::render(
                &key,
                MapKind::DayLogistics,
                points,
                routes,
                misses_of(day),
                off_area_of(day),
            ));
        } else {
            maps.push(PlannedMap::skip(
                &key,
                "no commute endpoints (no geocoded hotel/airport/sights)",
                Some(format!(
                    "   skipped {key} (no commute endpoints (no geocoded hotel/airport/sights))"
                )),
                misses_of(day),
                off_area_of(day),
            ));
        }
    }

    // The stops the hub gate kept off the day's maps (e.g. the home-side leg to
    // the departure airport) get their own compact map, in segment order, so
    // they are shown without stretching the area maps across the sea. Its skip
    // is silent: most days of most plans have none.
    for day in days {
        let key = format!("day-{day}-away.png");
        let points = day_away.get(day).cloned().unwrap_or_default();
        if points.is_empty() {
            maps.push(PlannedMap::skip(&key, "no off-area stops", None, misses_of(day), Vec::new()));
        } else {
            let routes = net.routes(&points).await?;
            maps.push(PlannedMap::render(&key, MapKind::Day, points, routes, misses_of(day), Vec::new()));
        }
    }

    // Logistics endpoints are computed BEFORE plan.png so a DOMESTIC trip (no
    // airport segment — every endpoint is the stay) can merge the hotel pin into
    // the overview map instead of emitting a second full-size map: for a
    // self-drive domestic stay the hotel IS an itinerary place, and the second
    // map slot only wasted vertical space (user request 2026-09-28). Plans WITH
    // airport endpoints (Japan) keep the split — distant endpoints must not
    // flatten the sightseeing overview's zoom level.
    let logistics = assemble_logistics(src, net, &mut gate).await?;
    let logistics_misses = net.take_misses();
    // Off-area logistics endpoints draw nowhere: a flight label is tied to no
    // day, and a segment airport already sits on its day's away map.
    let (logistics_off_area, _) = gate.take();
    for day in days.iter().filter(|d| !excursion.contains(d)) {
        if net.diagnostics()
            && let Some(points) = day_points.get(day)
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
            if let Some(existing) = plan_points
                .iter_mut()
                .find(|existing| coord_key(existing.lat, existing.lon) == coord_key(p.lat, p.lon))
            {
                // The route endpoint may already occupy this coordinate as a
                // sightseeing point. Keep one marker, but display the booked
                // stay's hotel pin and label in the overview.
                *existing = p.clone();
            } else {
                plan_points.push(p.clone());
            }
        }
    }

    // The overview's pins come from every non-excursion day (an airport day's
    // airport-leg-only stops excepted), plus (domestic) the stay, and
    // has_airport decides the domestic merge — so any logistics miss makes
    // plan.png unverifiable too. An airport day feeds both plan.png and the
    // airport inset, so its misses and off-area stops are recorded on both.
    let overview_misses = merged_distinct(
        days.iter()
            .filter(|d| on_overview(d))
            .map(misses_of)
            .chain(std::iter::once(logistics_misses.clone())),
    );
    let overview_off_area = merged_distinct(
        days.iter()
            .filter(|d| on_overview(d))
            .map(off_area_of)
            .chain(std::iter::once(logistics_off_area.clone())),
    );
    if plan_points.is_empty() {
        // Same lingering rule: when every sightseeing day left the overview,
        // the old flattened plan.png must not keep rendering from R2.
        maps.push(PlannedMap::skip(
            "plan.png",
            "no sightseeing points",
            Some("   skipped plan.png (no sightseeing points)".to_string()),
            overview_misses,
            overview_off_area,
        ));
    } else {
        let mut routes = Vec::new();
        for day in days.iter().filter(|d| on_overview(d)) {
            let points = overview_day_points(day);
            if points.len() > 1 {
                routes.extend(net.routes(&points).await?);
            }
        }
        maps.push(PlannedMap::render(
            "plan.png",
            MapKind::Plan,
            plan_points,
            routes,
            overview_misses,
            overview_off_area,
        ));
    }

    let excursion_misses = merged_distinct(days.iter().filter(|d| excursion.contains(d)).map(misses_of));
    let excursion_off_area = merged_distinct(days.iter().filter(|d| excursion.contains(d)).map(off_area_of));
    if excursion_points.is_empty() {
        maps.push(PlannedMap::skip(
            "plan-excursion.png",
            "no excursion day",
            None,
            excursion_misses,
            excursion_off_area,
        ));
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
                routes.extend(net.routes(&points).await?);
            }
        }
        maps.push(PlannedMap::render(
            "plan-excursion.png",
            MapKind::Plan,
            excursion_points,
            routes,
            excursion_misses,
            excursion_off_area,
        ));
    }

    // Every airport day's airport legs on one inset, each day in its own color,
    // each leg its own road line (drawn per segment, never by chaining the
    // pins, so a city leg between two airport legs stays off the inset). Its
    // skip is silent (a plan without flights has none), and
    // snapshot-maps still records it so a stale inset is removed.
    let airport_misses = merged_distinct(days.iter().filter(|d| airport_days.contains(d)).map(misses_of));
    let airport_off_area = merged_distinct(days.iter().filter(|d| airport_days.contains(d)).map(off_area_of));
    if airport_points.is_empty() {
        maps.push(PlannedMap::skip(
            "plan-airport.png",
            "no airport day",
            None,
            airport_misses,
            airport_off_area,
        ));
    } else {
        let mut routes = Vec::new();
        for day in days.iter().filter(|d| airport_days.contains(d)) {
            for (a, b) in day_airport_legs.get(day).into_iter().flatten() {
                routes.extend(net.routes(&[a.clone(), b.clone()]).await?);
            }
        }
        maps.push(PlannedMap::render(
            "plan-airport.png",
            MapKind::Plan,
            airport_points,
            routes,
            airport_misses,
            airport_off_area,
        ));
    }

    if logistics.is_empty() {
        maps.push(PlannedMap::skip(
            "plan-logistics.png",
            "no geocoded hotel/airport endpoints",
            Some("   skipped plan-logistics.png (no geocoded hotel/airport endpoints)".to_string()),
            logistics_misses,
            logistics_off_area,
        ));
    } else if !has_airport {
        // Domestic: the stay is already a pin on plan.png — no separate logistics map.
        maps.push(PlannedMap::skip(
            "plan-logistics.png",
            "domestic trip: no airport endpoints; stay merged into plan.png",
            Some(
                "   skipped plan-logistics.png (domestic: stay merged into plan.png, no airport endpoints)"
                    .to_string(),
            ),
            logistics_misses,
            logistics_off_area,
        ));
    } else {
        maps.push(PlannedMap::render(
            "plan-logistics.png",
            MapKind::Logistics,
            logistics,
            Vec::new(),
            logistics_misses,
            logistics_off_area,
        ));
    }
    if net.diagnostics()
        && let Some(note) = off_area_note(&maps, dest)
    {
        eprintln!("{note}");
    }
    Ok(maps)
}

/// The one per-run line naming every stop the hub gate kept off the area maps
/// and where it went. Not a ⚠ line: those are geocoding defects, and an
/// off-area stop is a correctly geocoded place drawn where it belongs.
fn off_area_note(maps: &[PlannedMap], dest: &str) -> Option<String> {
    let stops = merged_distinct(maps.iter().map(|m| m.off_area.clone()));
    if stops.is_empty() {
        return None;
    }
    // Every away map a stop is drawn on (a home-side stop can recur on several days).
    let away_keys = |stop: &OffAreaStop| {
        maps.iter()
            .filter(|m| m.key.ends_with("-away.png"))
            .filter(|m| matches!(&m.plan, MapPlan::Render { points, .. } if points.iter().any(|p| p.label == stop.label)))
            .map(|m| m.key.clone())
            .collect::<Vec<_>>()
    };
    let mut moved = Vec::new();
    let mut nowhere = Vec::new();
    for stop in &stops {
        let keys = away_keys(stop);
        if keys.is_empty() {
            nowhere.push(format!("{} ({} km)", stop.label, stop.km));
        } else {
            moved.push(format!("{} ({} km, {})", stop.label, stop.km, keys.join(" + ")));
        }
    }
    let mut parts = Vec::new();
    if !moved.is_empty() {
        parts.push(format!("moved to their own day-N-away map: {}", moved.join(", ")));
    }
    if !nowhere.is_empty() {
        parts.push(format!("not drawn on any map (tied to no day, e.g. a flight label): {}", nowhere.join(", ")));
    }
    Some(format!(
        "   note: {} stop(s) more than {MAX_HUB_DISTANCE_KM:.0} km from the {dest} hub are kept off the area maps — {}",
        stops.len(),
        parts.join("; ")
    ))
}

#[derive(Clone, Copy)]
pub(crate) enum MapKind {
    Day,
    /// A multi-day map (plan.png and the excursion / airport-day insets): pins
    /// colored and numbered per day.
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
        write_legend_stops(conn, plan, key, points, kind).await?;
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
    write_legend_stops(conn, plan, key, points, kind).await?;
    println!("   uploaded {key} ({} bytes)", png.len());
    Ok(true)
}

/// Rewrite the numbered-pin legend rows for one map key, from `legend_rows` —
/// the same `points` slice, pin numbers and pin colors `render_png` draws, so
/// the PNG and this table cannot drift.
/// Called ONLY on paths that (re)serve the current PNG (upload success and the
/// reusable-keep path): skip/fail paths leave the previous rows in place because
/// the dashboard still serves the previous PNG from R2, and its legend must keep
/// matching it.
async fn write_legend_stops(
    _conn: &Connection,
    plan: &str,
    key: &str,
    points: &[Point],
    kind: MapKind,
) -> Result<(), String> {
    // Reconnect after each Wrangler upload, which can outlive Turso's idle timeout.
    let conn = crate::db::connect_write().await?;
    conn.execute(
        "DELETE FROM map_legend_stops WHERE plan_id=?1 AND map_key=?2",
        params![plan.to_string(), key.to_string()],
    )
    .await
    .map_err(err("map legend delete"))?;
    for row in legend_rows(kind, points) {
        conn.execute(
            "INSERT INTO map_legend_stops (plan_id, map_key, seq, label, lat, lon, day_number, pin_no, color) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                plan.to_string(),
                key.to_string(),
                row.seq,
                row.label,
                row.lat,
                row.lon,
                row.day_number,
                row.pin_no,
                row.color
            ],
        )
        .await
        .map_err(err("map legend insert"))?;
    }
    Ok(())
}

/// One `map_legend_stops` row: `seq` is the unique row order (pin order on the
/// map), `pin_no` the number drawn on the pin, `color` the pin's fill as
/// `#rrggbb`, `day_number` the itinerary day the worker groups the legend by —
/// set only on the multi-day maps (MapKind::Plan). A day map already sits in
/// its day's section, so a day heading there would only repeat it; the
/// logistics maps' pin colors mean hotel/airport, not a day.
#[derive(Debug, PartialEq)]
struct LegendRow {
    seq: i64,
    label: String,
    lat: f64,
    lon: f64,
    day_number: Option<i64>,
    pin_no: i64,
    color: String,
}

fn legend_rows(kind: MapKind, points: &[Point]) -> Vec<LegendRow> {
    let by_day = matches!(kind, MapKind::Plan);
    points
        .iter()
        .zip(pin_numbers(kind, points))
        .enumerate()
        .map(|(i, (p, n))| LegendRow {
            seq: (i + 1) as i64,
            label: p.label.clone(),
            lat: p.lat,
            lon: p.lon,
            day_number: p.day.filter(|_| by_day).map(i64::from),
            pin_no: n as i64,
            color: hex_color(pin_color(kind, p)),
        })
        .collect()
}

fn hex_color(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

const HOTEL_INK: [u8; 3] = [21, 101, 192];
const AIRPORT_INK: [u8; 3] = [239, 108, 0];

/// The fill `render_png` gives a pin. On a multi-day map (MapKind::Plan) a
/// day's airport terminal wears its day's color like the day's sights — the
/// day color is what ties a pin to its day's numbers; everywhere else hotels
/// and airports keep their own colors.
fn pin_color(kind: MapKind, p: &Point) -> [u8; 3] {
    match p.kind {
        Kind::Sightseeing => p.color,
        Kind::Airport if matches!(kind, MapKind::Plan) && p.day.is_some() => p.color,
        Kind::Hotel => HOTEL_INK,
        Kind::Airport => AIRPORT_INK,
    }
}

/// The number drawn on each pin. A multi-day map (MapKind::Plan) numbers each
/// day's pins from 1 — the day color tells the days apart — with day-less pins
/// (a domestic stay) numbered as one more group; every other map numbers its
/// pins 1..N in order.
fn pin_numbers(kind: MapKind, points: &[Point]) -> Vec<usize> {
    if !matches!(kind, MapKind::Plan) {
        return (1..=points.len()).collect();
    }
    let mut next = HashMap::<Option<u32>, usize>::new();
    points
        .iter()
        .map(|p| {
            let n = next.entry(p.day).or_insert(0);
            *n += 1;
            *n
        })
        .collect()
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
    put_str(&mut hasher, RENDER_REV);
    put_str(&mut hasher, title);
    put_str(&mut hasher, map_kind_tag(kind));
    hasher.update((points.len() as u64).to_le_bytes());
    for p in points {
        hasher.update(p.lat.to_le_bytes());
        hasher.update(p.lon.to_le_bytes());
        put_str(&mut hasher, kind_tag(p.kind));
        hasher.update(&p.color);
        // The DAY legend names the day; colors cycle after 7 days.
        hasher.update(p.day.unwrap_or(0).to_le_bytes());
    }
    // A multi-day map numbers its pins per day, so the drawn numbers are part of
    // its image. Every other map draws 1..N in the point order hashed above —
    // hashing nothing more keeps those hashes (and their PNG reuse) unchanged.
    if matches!(kind, MapKind::Plan) {
        put_str(&mut hasher, "per-day-pins");
        for n in pin_numbers(kind, points) {
            hasher.update((n as u64).to_le_bytes());
        }
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
        let color = if road.major { ROAD_MAJOR } else { ROAD_MINOR };
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
    // Only a single-day map chains its pins when no road line came back: a
    // multi-day map draws each day's own lines, so chaining its pins in order
    // would join different days (or an inset's separate airport legs, e.g.
    // KIX → hotel and hotel → ITM) with a line nobody travels.
    if routes.is_empty() && matches!(kind, MapKind::Day) {
        for pair in xy.windows(2) {
            let a = projection.point(pair[0]);
            let b = projection.point(pair[1]);
            raster.line(a, b, [255, 255, 255], 8, true);
            raster.line(a, b, [80, 110, 140], 4, true);
        }
    }
    // Pins at their true positions collide in dense clusters (嵐山's five
    // stops at all-Kyoto scale stacked into one blob): declutter them, then
    // draw every leader (true-position dot + line) BEFORE any pin so no leader
    // crosses a number.
    let anchors: Vec<(i32, i32)> = points
        .iter()
        .map(|p| projection.point(mercator(p.lat, p.lon)))
        .collect();
    // A displaced pin must not land under the title, legend, or credit strips
    // (drawn after the pins, they would bury its number).
    let (legend_w, legend_rows) = match kind {
        MapKind::Day => (0, 0),
        MapKind::Logistics => (157, 1),
        MapKind::DayLogistics => (day_logistics_legend_entries(points).len() as i32 * 93, 1),
        MapKind::Plan => {
            let (cols, rows) = plan_legend_grid(plan_day_legend_entries(points).len());
            (cols * PLAN_LEGEND_STEP, rows)
        }
    };
    let mut reserved = vec![title_rect(title.len()), CREDIT_RECT];
    if legend_w > 0 {
        reserved.push((8, 30, 12 + legend_w, 30 + legend_rows * PLAN_LEGEND_ROW));
    }
    let placed = declutter_pins(&anchors, &reserved);
    for (i, p) in points.iter().enumerate() {
        if placed[i] != anchors[i] {
            raster.line(anchors[i], placed[i], [255, 255, 255], 3, false);
            raster.line(anchors[i], placed[i], [70, 82, 94], 1, false);
            raster.dot(anchors[i].0, anchors[i].1, 5, [255, 255, 255]);
            raster.dot(anchors[i].0, anchors[i].1, 3, pin_color(kind, p));
        }
    }
    for ((i, p), n) in points.iter().enumerate().zip(pin_numbers(kind, points)) {
        raster.pin(placed[i].0, placed[i].1, pin_color(kind, p), n);
    }
    let title = title.to_ascii_uppercase();
    if basemap.is_some() {
        // Solid backing strips keep title and credits readable over the basemap.
        for (x0, y0, x1, y1) in [title_rect(title.len()), CREDIT_RECT] {
            raster.fill_rect(x0, y0, x1 - x0, y1 - y0, [255, 255, 255]);
        }
    }
    raster.text(12, 10, &title, TITLE_INK, TITLE_SCALE);
    if matches!(kind, MapKind::Logistics) {
        raster.legend(12, 31, "HOTEL", HOTEL_INK);
        raster.legend(105, 31, "AIRPORT", AIRPORT_INK);
    } else if matches!(kind, MapKind::DayLogistics) {
        let mut x = 12;
        for (label, color) in day_logistics_legend_entries(points) {
            raster.legend(x, 31, label, color);
            x += 93;
        }
    } else if matches!(kind, MapKind::Plan) {
        // The overview colors pins by DAY; without a key the colors say
        // nothing. One entry per day that has a pin here (excursion days and
        // airport legs render on their own maps, so their entries leave with
        // them).
        // Long trips wrap onto further rows rather than run off the canvas.
        let entries = plan_day_legend_entries(points);
        if !entries.is_empty() {
            let (cols, rows) = plan_legend_grid(entries.len());
            if basemap.is_some() {
                raster.fill_rect(8, 30, cols * PLAN_LEGEND_STEP + 4, rows * PLAN_LEGEND_ROW, [255, 255, 255]);
            }
            for (i, (label, color)) in entries.into_iter().enumerate() {
                let (col, row) = (i as i32 % cols, i as i32 / cols);
                raster.legend(12 + col * PLAN_LEGEND_STEP, 31 + row * PLAN_LEGEND_ROW, &label, color);
            }
        }
    }
    let credit = if basemap.is_some() {
        CREDIT_WITH_BASEMAP
    } else {
        CREDIT_NO_BASEMAP
    };
    raster.text(CREDIT_RECT.0 + 4, CREDIT_RECT.1 + 4, credit, CREDIT_INK, 1);
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
pub(crate) fn decode_rgba_png(bytes: &[u8]) -> Option<Vec<u8>> {
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

/// Bumped whenever the renderer draws the SAME inputs differently, so the
/// no-background reuse path cannot keep a PNG drawn by an older renderer.
const RENDER_REV: &str = "pins-declutter+day-legend/1";
/// Horizontal step per overview DAY legend entry: dot + "DAY NN" + gap.
const PLAN_LEGEND_STEP: i32 = 62;
/// Vertical step between wrapped overview legend rows.
const PLAN_LEGEND_ROW: i32 = 14;

/// (columns, rows) for `n` overview DAY legend entries: as many per row as fit
/// between the 12px left inset and a matching right margin, then wrap.
fn plan_legend_grid(n: usize) -> (i32, i32) {
    let per_row = ((WIDTH as i32 - 24) / PLAN_LEGEND_STEP).max(1);
    let n = n as i32;
    let cols = n.min(per_row);
    let rows = if n == 0 { 0 } else { (n + per_row - 1) / per_row };
    (cols, rows)
}
/// Reserved title strip (x0, y0, x1, y1) for a title of `chars` characters,
/// drawn at (12, 10) in TITLE_SCALE glyphs: kept clear of displaced pins and
/// backed white over a basemap.
pub(crate) fn title_rect(chars: usize) -> (i32, i32, i32, i32) {
    (8, 8, 16 + chars as i32 * 12, 28)
}

/// Pixels of ink `Raster::text` lays down for `s` at `scale` (the bitmap font's
/// lit cells × scale²) — what verify-site expects to find in a text strip.
pub(crate) fn text_ink(s: &str, scale: i32) -> usize {
    s.chars()
        .map(|c| glyph(c).iter().map(|row| row.count_ones() as usize).sum::<usize>())
        .sum::<usize>()
        * (scale * scale) as usize
}

/// Pin radius drawn by `Raster::pin` (white halo included).
const PIN_RADIUS: i32 = 15;

/// Overview DAY legend: one (label, color) per itinerary day with at least one
/// pin (a sight, or an airport-day inset's terminal), in day order. Built from
/// each pin's own `day`, never from its color — DAY_COLORS cycles after 7 days.
/// Day-less pins (a domestic stay) have no entry.
fn plan_day_legend_entries(points: &[Point]) -> Vec<(String, [u8; 3])> {
    let mut days: Vec<(u32, [u8; 3])> = points
        .iter()
        .filter_map(|p| p.day.map(|d| (d, pin_color(MapKind::Plan, p))))
        .collect();
    days.sort_by_key(|(d, _)| *d);
    days.dedup_by_key(|(d, _)| *d);
    days.into_iter()
        .map(|(d, c)| (format!("DAY {d}"), c))
        .collect()
}

/// Place each pin at its true position when it does not overlap an earlier
/// pin; otherwise walk outward rings (8·k candidates on ring k, starting at the
/// top) and take the first free spot fully inside the canvas and clear of every
/// `reserved` rect (x0, y0, x1, y1). Order is preserved so pin N keeps number N.
/// A pin with no free spot within 4 rings stays at its anchor (overlapping)
/// rather than wandering far from the place it marks.
fn declutter_pins(anchors: &[(i32, i32)], reserved: &[(i32, i32, i32, i32)]) -> Vec<(i32, i32)> {
    let min = 2 * PIN_RADIUS - 2;
    let clear = |c: (i32, i32)| {
        reserved.iter().all(|&(x0, y0, x1, y1)| {
            let (dx, dy) = (c.0 - c.0.clamp(x0, x1), c.1 - c.1.clamp(y0, y1));
            dx * dx + dy * dy >= PIN_RADIUS * PIN_RADIUS
        })
    };
    let fits = |c: (i32, i32), placed: &[(i32, i32)]| {
        placed.iter().all(|p| {
            let (dx, dy) = (p.0 - c.0, p.1 - c.1);
            dx * dx + dy * dy >= min * min
        })
    };
    // Centers whose whole halo stays on pixels 0..=W-1 / 0..=H-1.
    let inside = |c: (i32, i32)| {
        c.0 >= PIN_RADIUS
            && c.0 <= WIDTH as i32 - 1 - PIN_RADIUS
            && c.1 >= PIN_RADIUS
            && c.1 <= HEIGHT as i32 - 1 - PIN_RADIUS
    };
    let mut placed: Vec<(i32, i32)> = Vec::with_capacity(anchors.len());
    for &a in anchors {
        let mut spot = a;
        if !fits(a, &placed) {
            'rings: for ring in 1..=4 {
                let r = f64::from(ring * min);
                let steps = 8 * ring;
                for s in 0..steps {
                    let ang = f64::from(s) * std::f64::consts::TAU / f64::from(steps)
                        - std::f64::consts::FRAC_PI_2;
                    let c = (
                        a.0 + (r * ang.cos()).round() as i32,
                        a.1 + (r * ang.sin()).round() as i32,
                    );
                    if inside(c) && clear(c) && fits(c, &placed) {
                        spot = c;
                        break 'rings;
                    }
                }
            }
        }
        placed.push(spot);
    }
    placed
}

/// The DayLogistics legend entries must match the pins actually on the map:
/// HOTEL always; DESTINATION (in the day's DAY_COLORS color) when a sight pin
/// exists; AIRPORT (orange) when the terminal is one of the pins.
fn day_logistics_legend_entries(points: &[Point]) -> Vec<(&'static str, [u8; 3])> {
    let mut out = vec![("HOTEL", HOTEL_INK)];
    if let Some(sight) = points.iter().find(|p| p.kind == Kind::Sightseeing) {
        out.push(("DESTINATION", sight.color));
    }
    if points.iter().any(|p| p.kind == Kind::Airport) {
        out.push(("AIRPORT", AIRPORT_INK));
    }
    out
}

/// The compact day map's commute endpoints and legs. The hotel pin opens and
/// closes the round trip; an airport terminal pin joins as a gateway — opening
/// the morning chain when it precedes every sight (arrival day) and closing the
/// evening chain when it follows every sight (departure) — so a pure
/// hotel↔airport transfer day renders as its single hop. Returns the dedup'd
/// points and legs; a day with no commute (e.g. a bare hotel pin) is empty.
fn day_logistics_legs(all_pts: &[Point]) -> (Vec<Point>, Vec<(Point, Point)>) {
    let hotel = all_pts.iter().find(|p| p.kind == Kind::Hotel);
    let airport = all_pts.iter().find(|p| p.kind == Kind::Airport);
    let first = all_pts.iter().find(|p| p.kind == Kind::Sightseeing);
    let last = all_pts.iter().rfind(|p| p.kind == Kind::Sightseeing);
    let airport_idx = |p: &Point| {
        all_pts
            .iter()
            .position(|q| q.kind == Kind::Airport && same_place(q, p))
    };
    let sight_idx = |p: &Point| {
        all_pts
            .iter()
            .position(|q| q.kind == Kind::Sightseeing && same_place(q, p))
    };
    let airport_before = airport
        .zip(first)
        .map_or(false, |(a, f)| {
            airport_idx(a).zip(sight_idx(f)).map_or(false, |(x, y)| x < y)
        });
    let airport_after = airport
        .zip(last)
        .map_or(false, |(a, l)| {
            airport_idx(a).zip(sight_idx(l)).map_or(false, |(x, y)| x > y)
        });

    let mut legs: Vec<(Point, Point)> = Vec::new();
    // Morning chain: airport (when it precedes the sights) → hotel → first sight.
    let mut chain: Vec<&Point> = Vec::new();
    if airport_before {
        if let Some(a) = airport {
            chain.push(a);
        }
    }
    if let Some(h) = hotel {
        chain.push(h);
    }
    if let Some(f) = first {
        // The first sight closes the chain only when something precedes it (a
        // gateway or the hotel); a lone sight with no hotel pin has no commute.
        if !chain.is_empty() {
            chain.push(f);
        }
    }
    for pair in chain.windows(2) {
        legs.push((pair[0].clone(), pair[1].clone()));
    }
    // Evening: last sight → gateway.
    if let Some(l) = last {
        let end = if airport_after { airport } else { hotel };
        if let Some(e) = end {
            legs.push((l.clone(), e.clone()));
        }
    }
    // Pure transfer day (no sights): the single hotel↔airport hop.
    if legs.is_empty() {
        if let (Some(h), Some(a)) = (hotel, airport) {
            legs.push((h.clone(), a.clone()));
        }
    }

    let mut points: Vec<Point> = Vec::new();
    legs.retain(|(a, b)| {
        if same_place(a, b) {
            return false;
        }
        for p in [a, b] {
            if !points.iter().any(|e| same_place(e, p)) {
                points.push(p.clone());
            }
        }
        true
    });
    (points, legs)
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
                self.put(x as i32, y as i32, GRID_BG);
            }
        }
        for x in (0..self.w).step_by(32) {
            self.line(
                (x as i32, 0),
                (x as i32, self.h as i32),
                GRID_LINE,
                1,
                false,
            );
        }
        for y in (0..self.h).step_by(32) {
            self.line(
                (0, y as i32),
                (self.w as i32, y as i32),
                GRID_LINE,
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
        self.dot(x, y, PIN_RADIUS, [255, 255, 255]);
        self.dot(x, y, PIN_RADIUS - 3, c);
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
/// `live`) so a fresh trip follows roads on its FIRST render. With `w = None`
/// (cache-only) a miss is not fetched and draws the same straight connector a
/// failed fetch draws.
async fn cached_routes(
    c: &Connection,
    w: Option<&Connection>,
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
            } else if let Some(w) = w {
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
/// The destination's hub coordinates (`destination_config.lat/lon`); None when
/// the row is missing or either column is NULL.
async fn query_destination_hub(c: &Connection, d: &str) -> Result<Option<(f64, f64)>, String> {
    let mut r = c
        .query(
            "SELECT lat, lon FROM destination_config WHERE slug=?1 LIMIT 1",
            params![d.to_string()],
        )
        .await
        .map_err(err("destination hub query"))?;
    let Some(x) = r.next().await.map_err(err("destination hub read"))? else {
        return Ok(None);
    };
    let lat = x.get::<Option<f64>>(0).map_err(err("destination hub lat"))?;
    let lon = x.get::<Option<f64>>(1).map_err(err("destination hub lon"))?;
    Ok(lat.zip(lon))
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

async fn resolve_segment_label<R: MapResolver>(
    net: &mut R,
    label: &str,
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
    Ok(net
        .geocode(&place)
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

/// What the geocode caches say about a place, without touching the network.
enum CacheLookup {
    Hit(ResolvedPlace),
    /// A cache row exists without coordinates: a recorded non-result.
    Unresolvable,
    /// No cache row at all: only a Nominatim fetch can answer.
    Miss,
}

/// In-memory cache first (raw_place keys from `load_geocodes`), then the exact
/// `route_place_geocodes.query_key` row. Read-only.
async fn lookup_place_cache(
    read: &Connection,
    place: &str,
    context: &str,
    cache: &mut HashMap<String, (f64, f64)>,
) -> Result<CacheLookup, String> {
    let key = place.trim().to_ascii_lowercase();
    if let Some(p) = cache.get(&key) {
        // In-memory hit: coords only (metadata is advisory, best-effort).
        return Ok(CacheLookup::Hit(ResolvedPlace {
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
            params![query_key],
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
            return Ok(CacheLookup::Hit(ResolvedPlace {
                coords: (lat, lon),
                display_name: display.unwrap_or_default(),
                area_centroid: false, // category is not persisted; unknown here
            }));
        }
        return Ok(CacheLookup::Unresolvable);
    }
    Ok(CacheLookup::Miss)
}

pub(crate) async fn resolve_place_meta(
    read: &Connection,
    write: &Connection,
    place: &str,
    context: &str,
    cache: &mut HashMap<String, (f64, f64)>,
) -> Result<Option<ResolvedPlace>, String> {
    match lookup_place_cache(read, place, context, cache).await? {
        CacheLookup::Hit(p) => Ok(Some(p)),
        CacheLookup::Unresolvable => Ok(None),
        CacheLookup::Miss => fetch_nominatim(write, place, context, cache).await,
    }
}

/// Cache miss: ask Nominatim once and write the answer through to
/// `route_place_geocodes` (and the in-memory cache).
async fn fetch_nominatim(
    write: &Connection,
    place: &str,
    context: &str,
    cache: &mut HashMap<String, (f64, f64)>,
) -> Result<Option<ResolvedPlace>, String> {
    let key = place.trim().to_ascii_lowercase();
    let (search, ctx) = normalize_place(place, context);
    let query_key = format!("{}|{}", search.trim().to_ascii_lowercase(), ctx);
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

/// `flight_legs` rows ordered (direction, leg_order) — outbound legs first.
async fn query_flight_rows(c: &Connection, p: &str, d: &str) -> Result<Vec<FlightRow>, String> {
    let mut f=c.query("SELECT departure_code, arrival_code, departure_airport, arrival_airport, flight_number, direction FROM flight_legs WHERE plan_id=?1 AND destination=?2 ORDER BY direction,leg_order",params![p.to_string(),d.to_string()]).await.map_err(err("flight map query"))?;
    let mut rows = Vec::new();
    while let Some(x) = f.next().await.map_err(err("flight map read"))? {
        let mut labels = Vec::new();
        for i in 0..4 {
            if let Ok(Some(s)) = x.get::<Option<String>>(i) {
                if !s.trim().is_empty() {
                    labels.push(s);
                }
            }
        }
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
        rows.push(FlightRow { labels, codes });
    }
    Ok(rows)
}

/// Every non-NULL `day_route_segments` endpoint, in the query's own row order.
async fn query_route_endpoints(c: &Connection, p: &str, d: &str) -> Result<Vec<String>, String> {
    let mut r=c.query("SELECT from_place,to_place FROM day_route_segments WHERE plan_id=?1 AND destination=?2",params![p.to_string(),d.to_string()]).await.map_err(err("logistics route query"))?;
    let mut out = Vec::new();
    while let Some(x) = r.next().await.map_err(err("logistics route read"))? {
        for i in 0..2 {
            if let Ok(s) = x.get::<String>(i) {
                out.push(s);
            }
        }
    }
    Ok(out)
}

async fn assemble_logistics<R: MapResolver>(
    src: &MapSource,
    net: &mut R,
    gate: &mut HubGate,
) -> Result<Vec<Point>, String> {
    let hotel_names = &src.hotel_names;
    let dest_pois = &src.dest_pois;
    let mut labels = Vec::<(String, Kind, bool)>::new();
    for name in hotel_names {
        labels.push((name.clone(), Kind::Hotel, false));
    }
    for row in &src.flights {
        for s in &row.labels {
            labels.push((s.clone(), Kind::Airport, false));
        }
    }
    // Home-airport codes to EXCLUDE from the logistics map: the first code of the
    // first outbound flight and the last code of the last return flight. Including
    // the home airport stretches the bbox across countries (e.g. TPE↔KIX, ~1000 km),
    // which flattens the map to empty and times the road-web query out.
    let mut home_codes = Vec::new();
    // rows are ordered (direction, leg_order): outbound legs first. First row =
    // first outbound leg → its first code is home; last row = last return leg →
    // its last code is home.
    if let Some(row) = src.flights.first() {
        if let Some(first) = row.codes.first() {
            home_codes.push(first.clone());
        }
    }
    if let Some(row) = src.flights.last() {
        if let Some(last) = row.codes.last() {
            home_codes.push(last.clone());
        }
    }
    for row in &src.flights {
        for code in &row.codes {
            if !home_codes.contains(code) {
                labels.push((code.clone(), Kind::Airport, false));
            }
        }
    }
    for s in &src.route_endpoints {
        let k = classify(s);
        if k != Kind::Sightseeing {
            labels.push((s.clone(), k, true));
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    // Domestic stay: `hotels` is Japan-path only, so the booked domestic stay
    // arrives here instead. Prefer its geocoded destination_pois row (title
    // contains the hotel name, e.g. 海論 → 海論海景民宿); only if no POI matches
    // does the label fall through to the shared geocode chain below.
    if let Some(hotel) = src.domestic_stay.clone() {
        match dest_pois
            .iter()
            .find(|poi| norm(&poi.title).contains(&norm(&hotel)))
        {
            Some(poi) => {
                if let Some(point) = gate.point(hotel, (poi.lat, poi.lon), HOTEL_INK, Kind::Hotel, None)
                    && seen.insert((coord_key(poi.lat, poi.lon), Kind::Hotel as u8))
                {
                    out.push(point);
                }
            }
            None => labels.push((hotel, Kind::Hotel, false)),
        }
    }
    for (label, kind, from_segment) in labels {
        // Keep the label's hotel/airport kind: this map only draws logistics endpoints,
        // so a label that happens to share a POI key must not turn into a sightseeing pin.
        let resolved = if from_segment {
            resolve_segment_label(net, &label, dest_pois, hotel_names)
                .await?
                .map(|(lat, lon, _)| (lat, lon, kind))
        } else {
            net.geocode(&label)
                .await?
                .map(|(lat, lon)| (lat, lon, kind))
        };
        if let Some((lat, lon, kind)) = resolved {
            let color = if kind == Kind::Hotel {
                HOTEL_INK
            } else {
                AIRPORT_INK
            };
            if let Some(point) = gate.point(display_label(&label, hotel_names), (lat, lon), color, kind, None)
                && seen.insert((coord_key(lat, lon), kind as u8))
            {
                out.push(point);
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
async fn remove_stale_map(_c: &Connection, plan: &str, key: &str, reason: &str) -> Result<(), String> {
    // Reconnect like record_artifact: this runs after Wrangler uploads, which can
    // outlive Turso's idle timeout, so the caller's long-lived `write` may be dead.
    let c = crate::db::connect_write().await?;
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
    record_artifact(&c, plan, key, 0, None, "skipped", Some(reason), None, None).await?;
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

/// Known-good renders for verify-site's content-check tests, drawn by the real
/// renderer so the checks are exercised against exactly what snapshot-maps uploads.
#[cfg(test)]
pub(crate) mod test_render {
    use super::*;

    pub(crate) enum Bg<'a> {
        Grid,
        Roads,
        Basemap(&'a [u8]),
    }

    pub(crate) fn day_map(bg: Bg) -> Vec<u8> {
        day_style_map("day-1.png", bg)
    }

    /// A day-style map (MapKind::Day: day maps and `day-{n}-away.png`) titled
    /// for `key`.
    pub(crate) fn day_style_map(key: &str, bg: Bg) -> Vec<u8> {
        let color = DAY_COLORS[0];
        let stop = |lat, lon| Point {
            lat,
            lon,
            color,
            kind: Kind::Sightseeing,
            day: Some(1),
            label: String::new(),
        };
        let points = vec![stop(35.0, 135.0), stop(35.01, 135.012), stop(35.004, 135.02)];
        let routes = vec![RouteLine {
            points: vec![(35.0, 135.0), (35.005, 135.006), (35.01, 135.012)],
            routed: true,
            color,
        }];
        let roads = match bg {
            Bg::Roads => vec![
                RoadWay { points: vec![(34.99, 135.005), (35.02, 135.005)], major: true },
                RoadWay { points: vec![(35.003, 134.99), (35.003, 135.03)], major: false },
                RoadWay { points: vec![(34.995, 134.995), (35.015, 135.025)], major: false },
            ],
            _ => Vec::new(),
        };
        let basemap = match bg {
            Bg::Basemap(b) => Some(b),
            _ => None,
        };
        render_png(&map_title(key).unwrap(), &points, &routes, &roads, basemap, MapKind::Day).unwrap()
    }

    /// A multi-day map (MapKind::Plan: plan.png and its insets) titled for
    /// `key`: two days, each in its own color with per-day pin numbers and the
    /// DAY legend, day 1 ending at an airport terminal, over a basemap.
    pub(crate) fn plan_style_map(key: &str, basemap: &[u8]) -> Vec<u8> {
        let stop = |lat, lon, kind, day: u32| Point {
            lat,
            lon,
            color: DAY_COLORS[day as usize - 1],
            kind,
            day: Some(day),
            label: String::new(),
        };
        let points = vec![
            stop(35.0, 135.0, Kind::Sightseeing, 1),
            stop(35.01, 135.012, Kind::Airport, 1),
            stop(35.004, 135.02, Kind::Sightseeing, 2),
            stop(34.995, 135.03, Kind::Sightseeing, 2),
        ];
        let routes = vec![
            RouteLine { points: vec![(35.0, 135.0), (35.01, 135.012)], routed: true, color: DAY_COLORS[0] },
            RouteLine { points: vec![(35.004, 135.02), (34.995, 135.03)], routed: true, color: DAY_COLORS[1] },
        ];
        render_png(&map_title(key).unwrap(), &points, &routes, &[], Some(basemap), MapKind::Plan).unwrap()
    }

    /// ArcGIS-like basemap: land-use blocks of similar luminance on a soft
    /// gradient — varied colors, no sharp tile-periodic edges.
    pub(crate) fn textured_basemap() -> Vec<u8> {
        const PAL: [[u8; 3]; 5] = [
            [232, 226, 214],
            [205, 224, 190],
            [170, 211, 223],
            [240, 236, 228],
            [220, 214, 230],
        ];
        let (w, h) = (WIDTH as usize, HEIGHT as usize);
        let mut out = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let c = PAL[(x / 53 + (y / 47) * 3) % PAL.len()];
                let (dx, dy) = ((x / 40) as u8, (y / 40) as u8);
                let i = (y * w + x) * 4;
                out[i..i + 3].copy_from_slice(&[c[0] - dx, c[1] - dy, c[2].saturating_sub(dx / 2 + dy / 2)]);
            }
        }
        out
    }

    pub(crate) fn flat_basemap(c: [u8; 3]) -> Vec<u8> {
        let mut out = vec![255u8; (WIDTH * HEIGHT * 4) as usize];
        for px in out.chunks_mut(4) {
            px[..3].copy_from_slice(&c);
        }
        out
    }

    /// Draw text into a full-canvas RGBA buffer with the renderer's own font.
    pub(crate) fn stamp_text(rgba: &mut Vec<u8>, x: i32, y: i32, s: &str, color: [u8; 3], scale: i32) {
        let mut raster = Raster {
            w: WIDTH as usize,
            h: HEIGHT as usize,
            pixels: std::mem::take(rgba),
        };
        raster.text(x, y, s, color, scale);
        *rgba = raster.pixels;
    }

    pub(crate) fn encode(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
        encode_rgba_png(rgba, w, h).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture resolver: geocodes from a fixed table (recording misses like
    /// NetMode::CacheOnly), road lines as the straight fallback connector.
    struct FakeNet {
        coords: HashMap<&'static str, (f64, f64)>,
        misses: Vec<String>,
    }

    impl MapResolver for FakeNet {
        async fn geocode(&mut self, place: &str) -> Result<Option<(f64, f64)>, String> {
            match self.coords.get(place) {
                Some(c) => Ok(Some(*c)),
                None => {
                    self.misses.push(place.to_string());
                    Ok(None)
                }
            }
        }
        async fn routes(&mut self, points: &[Point]) -> Result<Vec<RouteLine>, String> {
            Ok(points.windows(2).map(|p| straight(&p[0], &p[1])).collect())
        }
        fn diagnostics(&self) -> bool {
            false
        }
        fn take_misses(&mut self) -> Vec<String> {
            std::mem::take(&mut self.misses)
        }
    }

    fn straight(a: &Point, b: &Point) -> RouteLine {
        RouteLine {
            points: vec![(a.lat, a.lon), (b.lat, b.lon)],
            routed: false,
            color: a.color,
        }
    }

    const HOTEL: (f64, f64) = (34.9858, 135.7588);
    const KINKAKU: (f64, f64) = (35.0394, 135.7292);
    const KIYOMIZU: (f64, f64) = (34.9949, 135.7850);
    const INARI: (f64, f64) = (34.9671, 135.7727);
    const KIX: (f64, f64) = (34.4320, 135.2304);
    const KIX_T2: (f64, f64) = (34.4364, 135.2468);
    /// A stop beside KIX, reached only by airport legs.
    const RINKU: (f64, f64) = (34.4056, 135.2956);
    /// Where the airport train arrives: on airport and city legs alike.
    const KYOTO_STN: (f64, f64) = (34.9852, 135.7577);
    /// kyoto_2026's destination_config hub.
    const KYOTO_HUB: (f64, f64) = (35.0116, 135.7681);
    /// ~609 / ~622 km from KYOTO_HUB: beyond MAX_HUB_DISTANCE_KM.
    const BUSAN: (f64, f64) = (35.1796, 129.0756);
    const GIMHAE: (f64, f64) = (35.1795, 128.9382);
    /// ~404 km from KYOTO_HUB: an in-destination outlier that stays.
    const YAMAGUCHI: (f64, f64) = (34.1859, 131.4714);

    fn fake_net() -> FakeNet {
        FakeNet {
            coords: HashMap::from([
                ("APA Hotel", HOTEL),
                ("Fushimi Inari", INARI),
                ("KIX", KIX),
                ("KIX T2", KIX_T2),
                ("Rinku Town", RINKU),
                ("Kyoto Station", KYOTO_STN),
            ]),
            misses: Vec::new(),
        }
    }

    fn seg(day: i64, from: &str, to: &str) -> Segment {
        Segment {
            day,
            from: from.into(),
            to: to.into(),
            mode: "transit".into(),
            duration_min: None,
        }
    }

    /// Two Kyoto days: day 1 hotel → 金閣寺 → 清水寺 (POIs), day 2 hotel →
    /// 伏見稲荷 (geocoded); TPE⇄KIX flights.
    fn fixture_source(day1_last: &str) -> MapSource {
        MapSource {
            days: vec![1, 2],
            excursion: HashSet::new(),
            segments: vec![
                seg(1, "hotel", "Kinkakuji"),
                seg(1, "Kinkakuji", day1_last),
                seg(2, "hotel", "Fushimi Inari"),
            ],
            pois: Vec::new(),
            dest_pois: vec![
                poi("kinkakuji", "Kinkakuji", KINKAKU.0, KINKAKU.1),
                poi("kiyomizu", "Kiyomizu", KIYOMIZU.0, KIYOMIZU.1),
            ],
            hotel_names: vec!["APA Hotel".into()],
            flights: vec![
                FlightRow { labels: vec!["KIX".into()], codes: vec!["TPE".into(), "KIX".into()] },
                FlightRow { labels: Vec::new(), codes: vec!["KIX".into(), "TPE".into()] },
            ],
            route_endpoints: ["hotel", "Kinkakuji", "Kinkakuji", day1_last, "hotel", "Fushimi Inari"]
                .into_iter()
                .map(String::from)
                .collect(),
            domestic_stay: None,
            hub: Some(KYOTO_HUB),
        }
    }

    async fn assembled(src: &MapSource, net: &mut FakeNet) -> HashMap<String, PlannedMap> {
        assemble_maps(src, net, "p", "kyoto_2026")
            .await
            .unwrap()
            .into_iter()
            .map(|m| (m.key.clone(), m))
            .collect()
    }

    fn pt(c: (f64, f64), kind: Kind, day: Option<u32>) -> Point {
        Point {
            lat: c.0,
            lon: c.1,
            color: match (kind, day) {
                (Kind::Hotel, None) => [21, 101, 192],
                (Kind::Airport, None) => [239, 108, 0],
                (_, Some(d)) => DAY_COLORS[(d as usize - 1) % DAY_COLORS.len()],
                (Kind::Sightseeing, None) => unreachable!(),
            },
            kind,
            day,
            label: String::new(),
        }
    }

    #[tokio::test]
    async fn shared_assembly_hashes_what_the_snapshot_loop_rendered() {
        // Expected inputs derived by hand from the pre-extraction run() loop.
        let maps = assembled(&fixture_source("Kiyomizu"), &mut fake_net()).await;
        let keys: Vec<String> = assemble_maps(&fixture_source("Kiyomizu"), &mut fake_net(), "p", "kyoto_2026")
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.key)
            .collect();
        assert_eq!(
            keys,
            [
                "day-1.png",
                "day-2.png",
                "day-1-logistics.png",
                "day-2-logistics.png",
                "day-1-away.png",
                "day-2-away.png",
                "plan.png",
                "plan-excursion.png",
                "plan-airport.png",
                "plan-logistics.png"
            ],
            "snapshot-maps emission order"
        );

        let (k, kz, inari) = (
            pt(KINKAKU, Kind::Sightseeing, Some(1)),
            pt(KIYOMIZU, Kind::Sightseeing, Some(1)),
            pt(INARI, Kind::Sightseeing, Some(2)),
        );
        let hotel_d1 = pt(HOTEL, Kind::Hotel, Some(1));
        let expect = |title: &str, kind, points: &[Point], routes: &[RouteLine]| {
            Some(map_input_sha256(title, kind, points, routes))
        };

        assert_eq!(
            maps["day-1.png"].input_sha256(),
            expect("Day 1 route", MapKind::Day, &[k.clone(), kz.clone()], &[straight(&k, &kz)])
        );
        assert_eq!(
            maps["day-2.png"].input_sha256(),
            expect("Day 2 route", MapKind::Day, &[inari.clone()], &[])
        );
        assert_eq!(
            maps["day-1-logistics.png"].input_sha256(),
            expect(
                "Day 1 hotel round trip",
                MapKind::DayLogistics,
                &[hotel_d1.clone(), k.clone(), kz.clone()],
                &[straight(&hotel_d1, &k), straight(&kz, &hotel_d1)],
            )
        );
        assert_eq!(
            maps["plan.png"].input_sha256(),
            expect(
                "Sightseeing overview",
                MapKind::Plan,
                &[k.clone(), kz.clone(), inari.clone()],
                &[straight(&k, &kz)],
            )
        );
        // TPE is the home airport (first outbound / last return code): excluded.
        assert_eq!(
            maps["plan-logistics.png"].input_sha256(),
            expect(
                "Hotels and airports",
                MapKind::Logistics,
                &[pt(HOTEL, Kind::Hotel, None), pt(KIX, Kind::Airport, None)],
                &[],
            )
        );
        assert_eq!(maps["plan-excursion.png"].input_sha256(), None, "no excursion day → skip");
        assert_eq!(maps["plan-airport.png"].input_sha256(), None, "no airport day → skip");
        assert!(maps.values().all(|m| m.geocode_misses.is_empty()));
        assert!(maps.values().all(|m| m.off_area.is_empty()));
    }

    #[tokio::test]
    async fn a_route_only_edit_changes_exactly_the_affected_hashes() {
        let before = assembled(&fixture_source("Kiyomizu"), &mut fake_net()).await;
        // Same days and activities; only day 1's second day_route_segments row changed.
        let after = assembled(&fixture_source("Fushimi Inari"), &mut fake_net()).await;
        for key in ["day-1.png", "day-1-logistics.png", "plan.png"] {
            assert_ne!(before[key].input_sha256(), after[key].input_sha256(), "{key}");
        }
        for key in ["day-2.png", "day-2-logistics.png", "plan-logistics.png"] {
            assert_eq!(before[key].input_sha256(), after[key].input_sha256(), "{key}");
        }
        // And the assembly is deterministic.
        let again = assembled(&fixture_source("Kiyomizu"), &mut fake_net()).await;
        for (key, m) in &before {
            assert_eq!(m.input_sha256(), again[key].input_sha256(), "{key}");
        }
    }

    #[tokio::test]
    async fn geocode_misses_are_scoped_to_the_maps_they_feed() {
        let mut net = fake_net();
        net.coords.remove("Fushimi Inari");
        let maps = assembled(&fixture_source("Kiyomizu"), &mut net).await;
        let misses = |k: &str| maps[k].geocode_misses.clone();
        assert_eq!(misses("day-2.png"), ["Fushimi Inari"]);
        assert_eq!(misses("day-2-logistics.png"), ["Fushimi Inari"]);
        assert_eq!(misses("plan.png"), ["Fushimi Inari"]);
        assert!(misses("day-1.png").is_empty());
        assert!(misses("plan-logistics.png").is_empty());
    }

    /// `fixture_source("Kiyomizu")` whose day 1 starts abroad: Busan →
    /// Gimhae Airport → hotel (both ~610-620 km from the Kyoto hub), plus an
    /// off-area flight label.
    fn fixture_with_home_leg() -> (MapSource, FakeNet) {
        let mut src = fixture_source("Kiyomizu");
        src.segments.splice(0..0, [seg(1, "Busan", "Gimhae Airport"), seg(1, "Gimhae Airport", "hotel")]);
        src.route_endpoints
            .splice(0..0, ["Busan", "Gimhae Airport", "Gimhae Airport", "hotel"].map(String::from));
        src.flights[0].labels.insert(0, "Gimhae International".into());
        let mut net = fake_net();
        net.coords.extend([("Busan", BUSAN), ("Gimhae Airport", GIMHAE), ("Gimhae International", GIMHAE)]);
        (src, net)
    }

    fn drawn(m: &PlannedMap) -> Vec<(f64, f64)> {
        match &m.plan {
            MapPlan::Render { points, .. } => points.iter().map(|p| (p.lat, p.lon)).collect(),
            MapPlan::Skip { .. } => Vec::new(),
        }
    }

    fn off_area_labels(m: &PlannedMap) -> Vec<&str> {
        m.off_area.iter().map(|s| s.label.as_str()).collect()
    }

    fn km_from_hub(c: (f64, f64)) -> u32 {
        (haversine_m(KYOTO_HUB, c) / 1000.0).round() as u32
    }

    #[tokio::test]
    async fn off_area_stops_leave_the_area_maps_and_their_hashes_untouched() {
        let (src, mut net) = fixture_with_home_leg();
        let with = assembled(&src, &mut net).await;
        let without = assembled(&fixture_source("Kiyomizu"), &mut fake_net()).await;
        for key in [
            "day-1.png",
            "day-2.png",
            "day-1-logistics.png",
            "day-2-logistics.png",
            "day-2-away.png",
            "plan.png",
            "plan-excursion.png",
            // The gated Gimhae Airport is not drawn, so day 1 is no airport day.
            "plan-airport.png",
            "plan-logistics.png",
        ] {
            assert_eq!(with[key].input_sha256(), without[key].input_sha256(), "{key}");
            assert!(
                !drawn(&with[key]).iter().any(|c| *c == BUSAN || *c == GIMHAE),
                "{key} draws an off-area stop: {:?}",
                drawn(&with[key])
            );
        }
        // A gated stop resolved: it is not a geocode miss on any map.
        assert!(with.values().all(|m| m.geocode_misses.is_empty()));
    }

    #[tokio::test]
    async fn off_area_stops_get_their_own_day_away_map() {
        let (src, mut net) = fixture_with_home_leg();
        let maps = assembled(&src, &mut net).await;
        let busan = pt(BUSAN, Kind::Sightseeing, Some(1));
        let gimhae = pt(GIMHAE, Kind::Airport, Some(1));
        // Segment order, with the road line between consecutive off-area stops.
        assert_eq!(
            maps["day-1-away.png"].input_sha256(),
            Some(map_input_sha256(
                "Day 1 beyond the area",
                MapKind::Day,
                &[busan.clone(), gimhae.clone()],
                &[straight(&busan, &gimhae)],
            ))
        );
        assert_eq!(drawn(&maps["day-1-away.png"]), [BUSAN, GIMHAE]);
        // A day with no off-area stop: a silent skip (snapshot-maps still records
        // it, so a stale away map is removed).
        assert!(matches!(
            maps["day-2-away.png"].plan,
            MapPlan::Skip { reason: "no off-area stops", note: None }
        ));
        let plain = assembled(&fixture_source("Kiyomizu"), &mut fake_net()).await;
        assert_eq!(plain["day-1-away.png"].input_sha256(), None);
    }

    #[tokio::test]
    async fn off_area_stops_are_recorded_on_the_maps_they_would_have_fed() {
        let (src, mut net) = fixture_with_home_leg();
        let maps = assembled(&src, &mut net).await;
        let day1 = ["Busan", "Gimhae Airport"];
        assert_eq!(off_area_labels(&maps["day-1.png"]), day1);
        assert_eq!(off_area_labels(&maps["day-1-logistics.png"]), day1);
        assert_eq!(
            off_area_labels(&maps["plan.png"]),
            ["Busan", "Gimhae Airport", "Gimhae International"]
        );
        assert_eq!(
            off_area_labels(&maps["plan-logistics.png"]),
            ["Gimhae International", "Gimhae Airport"]
        );
        for key in ["day-2.png", "day-2-logistics.png", "plan-excursion.png", "day-1-away.png"] {
            assert!(maps[key].off_area.is_empty(), "{key}: {:?}", maps[key].off_area);
        }
        assert_eq!(maps["day-1.png"].off_area[0].km, km_from_hub(BUSAN));

        let all: Vec<PlannedMap> = maps.into_values().collect();
        let note = off_area_note(&all, "kyoto_2026").expect("a note when stops were gated");
        assert!(note.starts_with("   note: 3 stop(s) more than 500 km from the kyoto_2026 hub"), "{note}");
        assert!(!note.contains('⚠'), "{note}");
        assert!(note.contains(&format!("Busan ({} km, day-1-away.png)", km_from_hub(BUSAN))), "{note}");
        assert!(note.contains(&format!("Gimhae Airport ({} km, day-1-away.png)", km_from_hub(GIMHAE))), "{note}");
        assert!(
            note.contains(&format!("not drawn on any map (tied to no day, e.g. a flight label): Gimhae International ({} km)", km_from_hub(GIMHAE))),
            "{note}"
        );
    }

    #[tokio::test]
    async fn the_note_names_every_away_map_a_recurring_stop_is_on() {
        let (mut src, mut net) = fixture_with_home_leg();
        // Busan again on day 2: it is drawn on both days' away maps.
        src.segments.push(seg(2, "Fushimi Inari", "Busan"));
        // The Vec in emission order, as snapshot-maps passes it to off_area_note.
        let all = assemble_maps(&src, &mut net, "p", "kyoto_2026").await.unwrap();
        let day2_away = all.iter().find(|m| m.key == "day-2-away.png").unwrap();
        assert_eq!(drawn(day2_away), [BUSAN]);
        let note = off_area_note(&all, "kyoto_2026").unwrap();
        assert!(
            note.contains(&format!("Busan ({} km, day-1-away.png + day-2-away.png)", km_from_hub(BUSAN))),
            "{note}"
        );
    }

    #[tokio::test]
    async fn a_stop_inside_the_hub_radius_stays_on_the_area_maps() {
        let mut src = fixture_source("Kiyomizu");
        src.segments.push(seg(2, "Fushimi Inari", "Yamaguchi"));
        let mut net = fake_net();
        net.coords.insert("Yamaguchi", YAMAGUCHI);
        assert!(km_from_hub(YAMAGUCHI) > 400 && (km_from_hub(YAMAGUCHI) as f64) < MAX_HUB_DISTANCE_KM);
        let maps = assembled(&src, &mut net).await;
        assert_eq!(drawn(&maps["day-2.png"]), [INARI, YAMAGUCHI]);
        assert!(drawn(&maps["plan.png"]).contains(&YAMAGUCHI));
        assert_eq!(maps["day-2-away.png"].input_sha256(), None);
        assert!(maps.values().all(|m| m.off_area.is_empty()));
    }

    #[tokio::test]
    async fn without_a_hub_nothing_is_gated() {
        let (mut src, mut net) = fixture_with_home_leg();
        src.hub = None;
        let maps = assembled(&src, &mut net).await;
        assert_eq!(drawn(&maps["day-1.png"])[0], BUSAN);
        assert!(drawn(&maps["plan-logistics.png"]).contains(&GIMHAE));
        assert!(maps.values().all(|m| m.off_area.is_empty()));
        for key in maps.keys().filter(|k| k.ends_with("-away.png")) {
            assert_eq!(maps[key].input_sha256(), None, "{key}");
        }
        assert_eq!(off_area_note(&maps.into_values().collect::<Vec<_>>(), "kyoto_2026"), None);
    }

    /// `fixture_source("Kiyomizu")` grown to three days. Day 1 lands at KIX T2
    /// and reaches the hotel by airport legs (KIX T2 → Rinku Town → KIX →
    /// Kyoto Station), then sightsees (Kyoto Station → hotel → 金閣寺 → 清水寺);
    /// day 2 stays in town (hotel → 伏見稲荷); day 3 flies home (hotel → 清水寺
    /// → Kyoto Station → KIX).
    fn fixture_with_airport_days() -> MapSource {
        let mut src = fixture_source("Kiyomizu");
        src.days.push(3);
        let arrival = [
            ("KIX T2", "Rinku Town"),
            ("Rinku Town", "KIX"),
            ("KIX", "Kyoto Station"),
            ("Kyoto Station", "hotel"),
        ];
        let departure = [("hotel", "Kiyomizu"), ("Kiyomizu", "Kyoto Station"), ("Kyoto Station", "KIX")];
        src.segments.splice(0..0, arrival.map(|(a, b)| seg(1, a, b)));
        src.segments.extend(departure.map(|(a, b)| seg(3, a, b)));
        src.route_endpoints
            .splice(0..0, arrival.iter().flat_map(|(a, b)| [a.to_string(), b.to_string()]));
        src.route_endpoints
            .extend(departure.iter().flat_map(|(a, b)| [a.to_string(), b.to_string()]));
        src
    }

    #[tokio::test]
    async fn an_airport_day_splits_its_airport_legs_onto_the_inset() {
        let maps = assembled(&fixture_with_airport_days(), &mut fake_net()).await;
        // Overview: every non-excursion day's sights, less the stops reached
        // only by airport legs. Day 1's city sights stay, in day 1's color;
        // Kyoto Station is on a city leg too, so it stays; Rinku Town (airport
        // legs only) and the terminals leave. Day 3's sights repeat day 1's
        // pins (deduplicated across days) but still draw day 3's road line.
        let ov1 = [
            pt(KYOTO_STN, Kind::Sightseeing, Some(1)),
            pt(KINKAKU, Kind::Sightseeing, Some(1)),
            pt(KIYOMIZU, Kind::Sightseeing, Some(1)),
        ];
        let inari = pt(INARI, Kind::Sightseeing, Some(2));
        let ov3 = [pt(KIYOMIZU, Kind::Sightseeing, Some(3)), pt(KYOTO_STN, Kind::Sightseeing, Some(3))];
        assert_eq!(drawn(&maps["plan.png"]), [KYOTO_STN, KINKAKU, KIYOMIZU, INARI]);
        assert_eq!(
            maps["plan.png"].input_sha256(),
            Some(map_input_sha256(
                "Sightseeing overview",
                MapKind::Plan,
                &[ov1[0].clone(), ov1[1].clone(), ov1[2].clone(), inari],
                &[straight(&ov1[0], &ov1[1]), straight(&ov1[1], &ov1[2]), straight(&ov3[0], &ov3[1])],
            ))
        );
        for c in [KIX, KIX_T2, RINKU] {
            assert!(!drawn(&maps["plan.png"]).contains(&c), "{c:?} on plan.png");
        }
        // Inset: each airport day's airport-leg ends, in segment order and in
        // its own color; the shared KIX pin appears once per day, and so does
        // Kyoto Station (on both maps).
        let day1 = [
            pt(KIX_T2, Kind::Airport, Some(1)),
            pt(RINKU, Kind::Sightseeing, Some(1)),
            pt(KIX, Kind::Airport, Some(1)),
            pt(KYOTO_STN, Kind::Sightseeing, Some(1)),
        ];
        let day3 = [pt(KYOTO_STN, Kind::Sightseeing, Some(3)), pt(KIX, Kind::Airport, Some(3))];
        let routes = [
            straight(&day1[0], &day1[1]),
            straight(&day1[1], &day1[2]),
            straight(&day1[2], &day1[3]),
            straight(&day3[0], &day3[1]),
        ];
        assert_eq!(drawn(&maps["plan-airport.png"]), [KIX_T2, RINKU, KIX, KYOTO_STN, KYOTO_STN, KIX]);
        assert_eq!(
            maps["plan-airport.png"].input_sha256(),
            Some(map_input_sha256("Airport days", MapKind::Plan, &[day1.as_slice(), &day3].concat(), &routes))
        );
        // The per-day maps do not move: each day keeps all its sights.
        assert_eq!(drawn(&maps["day-1.png"]), [RINKU, KYOTO_STN, KINKAKU, KIYOMIZU]);
        assert_eq!(drawn(&maps["day-3.png"]), [KIYOMIZU, KYOTO_STN]);
    }

    #[tokio::test]
    async fn inset_roads_follow_the_airport_legs_not_the_pin_order() {
        // Day 3 leaves KIX for a city stop and comes back later: two airport
        // legs with a city leg between them.
        let mut src = fixture_source("Kiyomizu");
        src.days.push(3);
        src.segments.extend(
            [("KIX", "Kyoto Station"), ("Kyoto Station", "Kiyomizu"), ("Kiyomizu", "Fushimi Inari"), ("Fushimi Inari", "KIX")]
                .map(|(a, b)| seg(3, a, b)),
        );
        let maps = assembled(&src, &mut fake_net()).await;
        let kix = pt(KIX, Kind::Airport, Some(3));
        let stn = pt(KYOTO_STN, Kind::Sightseeing, Some(3));
        let inari = pt(INARI, Kind::Sightseeing, Some(3));
        // Pins in segment order; Kiyomizu (city legs only) is not on the inset.
        assert_eq!(drawn(&maps["plan-airport.png"]), [KIX, KYOTO_STN, INARI]);
        // Two road lines, one per airport leg — no Kyoto Station → 伏見稲荷 line.
        assert_eq!(
            maps["plan-airport.png"].input_sha256(),
            Some(map_input_sha256(
                "Airport days",
                MapKind::Plan,
                &[kix.clone(), stn.clone(), inari.clone()],
                &[straight(&kix, &stn), straight(&inari, &kix)],
            ))
        );
    }

    #[tokio::test]
    async fn an_off_area_airport_makes_no_airport_leg() {
        // Day 3 also flies out of Gimhae (beyond the hub radius) from Yamaguchi.
        let mut src = fixture_with_airport_days();
        src.segments.push(seg(3, "Yamaguchi", "Gimhae Airport"));
        let mut net = fake_net();
        net.coords.extend([("Yamaguchi", YAMAGUCHI), ("Gimhae Airport", GIMHAE)]);
        let maps = assembled(&src, &mut net).await;
        // The gated terminal is not drawn, so its leg is no airport leg:
        // Yamaguchi stays on the overview and off the inset.
        assert!(drawn(&maps["plan.png"]).contains(&YAMAGUCHI));
        assert!(!drawn(&maps["plan-airport.png"]).contains(&YAMAGUCHI));
        assert!(!drawn(&maps["plan-airport.png"]).contains(&GIMHAE));
        assert_eq!(off_area_labels(&maps["plan-airport.png"]), ["Gimhae Airport"]);

        // Ungated, the same leg is an airport leg: Yamaguchi is inset-only.
        src.hub = None;
        let mut net = fake_net();
        net.coords.extend([("Yamaguchi", YAMAGUCHI), ("Gimhae Airport", GIMHAE)]);
        let maps = assembled(&src, &mut net).await;
        assert!(!drawn(&maps["plan.png"]).contains(&YAMAGUCHI));
        assert!(drawn(&maps["plan-airport.png"]).ends_with(&[YAMAGUCHI, GIMHAE]));
    }

    #[tokio::test]
    async fn an_excursion_flag_wins_over_an_airport_stop() {
        let mut src = fixture_with_airport_days();
        src.excursion.insert(1);
        let maps = assembled(&src, &mut fake_net()).await;
        // Day 1 is drawn whole as the day trip it is flagged as (sights only,
        // its airport legs included)…
        assert_eq!(drawn(&maps["plan-excursion.png"]), [RINKU, KYOTO_STN, KINKAKU, KIYOMIZU]);
        // …and only day 3 is left on the airport inset and the overview.
        assert_eq!(drawn(&maps["plan-airport.png"]), [KYOTO_STN, KIX]);
        assert_eq!(drawn(&maps["plan.png"]), [INARI, KIYOMIZU, KYOTO_STN]);
    }

    #[tokio::test]
    async fn no_airport_day_is_a_silent_skip() {
        let maps = assembled(&fixture_source("Kiyomizu"), &mut fake_net()).await;
        assert!(matches!(
            maps["plan-airport.png"].plan,
            MapPlan::Skip { reason: "no airport day", note: None }
        ));
        // Both days stay on the overview.
        assert_eq!(drawn(&maps["plan.png"]), [KINKAKU, KIYOMIZU, INARI]);
    }

    fn legend_of(m: &PlannedMap) -> Vec<LegendRow> {
        match &m.plan {
            MapPlan::Render { kind, points, .. } => legend_rows(*kind, points),
            MapPlan::Skip { .. } => Vec::new(),
        }
    }

    #[tokio::test]
    async fn multi_day_maps_number_pins_per_day() {
        let maps = assembled(&fixture_with_airport_days(), &mut fake_net()).await;
        let numbered = |key: &str| {
            legend_of(&maps[key])
                .into_iter()
                .map(|r| (r.seq, r.day_number, r.pin_no))
                .collect::<Vec<_>>()
        };
        // seq stays the unique row order; pin_no restarts at 1 on each day.
        assert_eq!(
            numbered("plan-airport.png"),
            [
                (1, Some(1), 1),
                (2, Some(1), 2),
                (3, Some(1), 3),
                (4, Some(1), 4),
                (5, Some(3), 1),
                (6, Some(3), 2)
            ]
        );
        // An airport day's city sights keep their day's numbers on the overview.
        assert_eq!(
            numbered("plan.png"),
            [(1, Some(1), 1), (2, Some(1), 2), (3, Some(1), 3), (4, Some(2), 1)]
        );
        let plain = assembled(&fixture_source("Kiyomizu"), &mut fake_net()).await;
        assert_eq!(
            legend_of(&plain["plan.png"]).iter().map(|r| (r.day_number, r.pin_no)).collect::<Vec<_>>(),
            [(Some(1), 1), (Some(1), 2), (Some(2), 1)]
        );
        // A day map is per-day already: 1..N, and no day heading — it sits in
        // its day's section (the away map likewise).
        assert_eq!(numbered("day-1.png"), [(1, None, 1), (2, None, 2), (3, None, 3), (4, None, 4)]);
        // Logistics maps carry no day: their colors mean hotel/airport.
        assert!(legend_of(&maps["day-1-logistics.png"]).iter().all(|r| r.day_number.is_none()));
        // Hotel, KIX and KIX T2 (the fixture's arrival terminal).
        assert_eq!(numbered("plan-logistics.png"), [(1, None, 1), (2, None, 2), (3, None, 3)]);
    }

    #[test]
    fn legend_rows_carry_the_numbers_and_colors_the_png_draws() {
        let at = |lat, lon, kind, day: u32| Point {
            lat,
            lon,
            color: DAY_COLORS[(day as usize - 1) % DAY_COLORS.len()],
            kind,
            day: Some(day),
            label: format!("{lat},{lon}"),
        };
        // Far enough apart that no pin is decluttered off its anchor.
        let points = vec![
            at(35.0, 135.0, Kind::Sightseeing, 1),
            at(35.1, 135.25, Kind::Airport, 1),
            at(35.2, 135.05, Kind::Sightseeing, 2),
        ];
        let rows = legend_rows(MapKind::Plan, &points);
        assert_eq!(
            rows.iter().map(|r| (r.seq, r.day_number, r.pin_no)).collect::<Vec<_>>(),
            [(1, Some(1), 1), (2, Some(1), 2), (3, Some(2), 1)]
        );
        assert_eq!(rows[0].label, "35,135");
        // On a multi-day map the airport pin wears its day's color.
        assert_eq!(rows[1].color, "#e6194b");
        assert_eq!(rows[2].color, "#3cb44b");

        // Each recorded color is the fill actually drawn inside that pin.
        let png = render_png("Airport days", &points, &[], &[], None, MapKind::Plan).unwrap();
        let rgba = decode_rgba_png(&png).unwrap();
        let projection = map_projection(&points, &[]);
        for (p, row) in points.iter().zip(&rows) {
            let (x, y) = projection.point(mercator(p.lat, p.lon));
            // Inside the pin's fill, above its white number.
            let i = (((y - 8) * WIDTH as i32 + x) * 4) as usize;
            assert_eq!(hex_color([rgba[i], rgba[i + 1], rgba[i + 2]]), row.color, "{row:?}");
        }

        // A logistics map keeps the hotel/airport inks and records no day, even
        // for pins that came from a day.
        let log = legend_rows(
            MapKind::DayLogistics,
            &[pt(HOTEL, Kind::Hotel, Some(1)), pt(KIX, Kind::Airport, Some(1))],
        );
        assert_eq!(
            log.iter().map(|r| (r.day_number, r.pin_no, r.color.as_str())).collect::<Vec<_>>(),
            [(None, 1, "#1565c0"), (None, 2, "#ef6c00")]
        );
    }

    #[test]
    fn pin_numbers_count_day_less_pins_as_their_own_group() {
        // Domestic overview: the merged stay has no day.
        let pts = [
            pt(KINKAKU, Kind::Sightseeing, Some(1)),
            pt(HOTEL, Kind::Hotel, None),
            pt(INARI, Kind::Sightseeing, Some(2)),
            pt(KIYOMIZU, Kind::Sightseeing, Some(1)),
        ];
        assert_eq!(pin_numbers(MapKind::Plan, &pts), [1, 1, 1, 2]);
        assert_eq!(pin_numbers(MapKind::Day, &pts), [1, 2, 3, 4]);
    }

    #[test]
    fn map_title_names_every_key_the_assembly_emits() {
        assert_eq!(map_title("day-3.png").as_deref(), Some("Day 3 route"));
        assert_eq!(map_title("day-12-logistics.png").as_deref(), Some("Day 12 hotel round trip"));
        assert_eq!(map_title("plan.png").as_deref(), Some("Sightseeing overview"));
        assert_eq!(map_title("plan-excursion.png").as_deref(), Some("Day trip"));
        assert_eq!(map_title("plan-airport.png").as_deref(), Some("Airport days"));
        assert_eq!(map_title("plan-logistics.png").as_deref(), Some("Hotels and airports"));
        assert_eq!(map_title("day-4-away.png").as_deref(), Some("Day 4 beyond the area"));
        assert_eq!(map_title("day-x-away.png"), None);
        assert_eq!(map_title("day-x.png"), None);
        assert_eq!(map_title("other.png"), None);
    }

    #[test]
    fn text_ink_counts_the_font_cells() {
        // '1' = [4, 12, 4, 4, 4, 4, 14]: 1+2+1+1+1+1+3 = 10 lit cells.
        assert_eq!(text_ink("1", 1), 10);
        assert_eq!(text_ink("1", 2), 40);
        assert_eq!(text_ink("1 1", 1), 20);
    }

    #[test]
    fn excursion_suggestion_uses_sights_and_nearest_hotel() {
        let point = |lat, kind| Point {
            lat,
            lon: 135.0,
            kind,
            day: None,
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
    fn day_logistics_legs_cover_transfer_and_gateway_days() {
        let point = |lat, kind| Point {
            lat,
            lon: 135.0,
            kind,
            day: None,
            label: "test".into(),
            color: [0; 3],
        };
        let hotel = point(35.0, Kind::Hotel);
        let air = point(34.6, Kind::Airport);
        let s1 = point(35.1, Kind::Sightseeing);
        let s2 = point(35.2, Kind::Sightseeing);

        // Pure hotel↔airport transfer day (KIX→hotel): one hop — previously this
        // day rendered no map at all.
        let (points, legs) = day_logistics_legs(&[air.clone(), hotel.clone()]);
        assert_eq!(legs.len(), 1, "{legs:?}");
        assert_eq!(points.len(), 2);

        // Arrival day with one sight: airport→hotel, hotel→sight, sight→hotel.
        let (points, legs) = day_logistics_legs(&[air.clone(), hotel.clone(), s1.clone()]);
        assert_eq!(legs.len(), 3, "{legs:?}");
        assert!(same_place(&legs[0].0, &air) && same_place(&legs[0].1, &hotel));
        assert!(same_place(&legs[1].0, &hotel) && same_place(&legs[1].1, &s1));
        assert!(same_place(&legs[2].0, &s1) && same_place(&legs[2].1, &hotel));
        assert_eq!(points.len(), 3);

        // Normal sightseeing day: the hotel round trip, unchanged.
        let (points, legs) = day_logistics_legs(&[hotel.clone(), s1.clone(), s2.clone()]);
        assert_eq!(legs.len(), 2, "{legs:?}");
        assert!(same_place(&legs[0].0, &hotel) && same_place(&legs[0].1, &s1));
        assert!(same_place(&legs[1].0, &s2) && same_place(&legs[1].1, &hotel));
        assert_eq!(points.len(), 3);

        // Departure day: the evening leg ends at the terminal, not the hotel.
        let (points, legs) = day_logistics_legs(&[hotel.clone(), s1.clone(), s2.clone(), air.clone()]);
        assert_eq!(legs.len(), 2, "{legs:?}");
        assert!(same_place(&legs[0].0, &hotel) && same_place(&legs[0].1, &s1));
        assert!(same_place(&legs[1].0, &s2) && same_place(&legs[1].1, &air));
        assert_eq!(points.len(), 4);

        // Hotel-only day still maps nothing.
        assert_eq!(day_logistics_legs(&[hotel.clone()]).1.len(), 0);
    }

    #[test]
    fn day_logistics_legend_entries_match_the_pins_on_the_map() {
        let point = |lat, kind| Point {
            lat,
            lon: 135.0,
            kind,
            day: None,
            label: "test".into(),
            color: [0; 3],
        };
        let hotel = point(35.0, Kind::Hotel);
        let air = point(34.6, Kind::Airport);
        let sight = Point {
            lat: 35.1,
            lon: 135.0,
            kind: Kind::Sightseeing,
            day: None,
            label: "test".into(),
            color: [60, 180, 75],
        };
        // Sightseeing day: HOTEL + DESTINATION in the day's color.
        assert_eq!(
            day_logistics_legend_entries(&[hotel.clone(), sight.clone()]),
            vec![("HOTEL", [21, 101, 192]), ("DESTINATION", [60, 180, 75])]
        );
        // Pure transfer day: HOTEL + AIRPORT, no DESTINATION swatch.
        assert_eq!(
            day_logistics_legend_entries(&[hotel.clone(), air.clone()]),
            vec![("HOTEL", [21, 101, 192]), ("AIRPORT", [239, 108, 0])]
        );
        // Mixed day: all three entries.
        assert_eq!(
            day_logistics_legend_entries(&[hotel.clone(), sight, air]),
            vec![
                ("HOTEL", [21, 101, 192]),
                ("DESTINATION", [60, 180, 75]),
                ("AIRPORT", [239, 108, 0])
            ]
        );
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
            Point { lat: 35.0, lon: 135.0, color: [0, 0, 0], kind: Kind::Sightseeing, day: None, label: String::new() },
            Point { lat: 35.1, lon: 135.2, color: [0, 0, 0], kind: Kind::Sightseeing, day: None, label: String::new() },
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
    fn only_a_day_map_chains_its_pins_without_road_lines() {
        // Two pins, no routes: e.g. an airport inset whose legs both end at the
        // hotel (KIX → hotel, hotel → ITM) keeps the two terminals unjoined.
        let points = vec![
            Point { lat: 34.43, lon: 135.23, color: [230, 25, 75], kind: Kind::Airport, day: Some(1), label: String::new() },
            Point { lat: 34.79, lon: 135.44, color: [230, 25, 75], kind: Kind::Airport, day: Some(1), label: String::new() },
        ];
        let connector = |kind| {
            let rgba = decode_rgba_png(&render_png("T", &points, &[], &[], None, kind).unwrap()).unwrap();
            rgba.chunks(4).filter(|px| px[..3] == [80, 110, 140]).count()
        };
        assert!(connector(MapKind::Day) > 0, "a day map still chains its pins");
        assert_eq!(connector(MapKind::Plan), 0, "a multi-day map must not invent a line");
    }

    #[test]
    fn render_png_with_road_web_changes_output() {
        let points = vec![
            Point { lat: 35.0, lon: 135.0, color: [200, 50, 120], kind: Kind::Sightseeing, day: None, label: String::new() },
            Point { lat: 35.01, lon: 135.01, color: [200, 50, 120], kind: Kind::Sightseeing, day: None, label: String::new() },
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
            Point { lat: 35.0, lon: 135.0, color: [200, 50, 120], kind: Kind::Sightseeing, day: None, label: String::new() },
            Point { lat: 35.01, lon: 135.01, color: [200, 50, 120], kind: Kind::Sightseeing, day: None, label: String::new() },
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
            Point { lat: 35.0, lon: 135.0, color: [0, 0, 0], kind: Kind::Sightseeing, day: None, label: String::new() },
            Point { lat: 35.1, lon: 135.2, color: [0, 0, 0], kind: Kind::Sightseeing, day: None, label: String::new() },
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
    fn declutter_keeps_free_pins_and_separates_colliding_ones() {
        let min = 2 * PIN_RADIUS - 2;
        // Five pins stacked within a few pixels (the 嵐山 cluster) plus one
        // far-away pin that must stay exactly where it is.
        let anchors = vec![(300, 200), (302, 201), (299, 203), (301, 198), (304, 202), (100, 100)];
        let placed = declutter_pins(&anchors, &[]);
        assert_eq!(placed.len(), anchors.len());
        assert_eq!(placed[0], anchors[0], "the first pin of a cluster keeps its spot");
        assert_eq!(placed[5], anchors[5], "a non-colliding pin never moves");
        for i in 0..placed.len() {
            for j in (i + 1)..placed.len() {
                let (dx, dy) = (placed[i].0 - placed[j].0, placed[i].1 - placed[j].1);
                assert!(dx * dx + dy * dy >= min * min, "pins {i} and {j} still overlap: {placed:?}");
            }
            // Displaced pins stay near the place they mark (≤ 4 rings).
            let (dx, dy) = (placed[i].0 - anchors[i].0, placed[i].1 - anchors[i].1);
            assert!(dx * dx + dy * dy <= (4 * min + 1) * (4 * min + 1), "{placed:?}");
        }
    }

    #[test]
    fn declutter_never_pushes_a_pin_off_canvas() {
        let (w, h) = (WIDTH as i32, HEIGHT as i32);
        let on_canvas = |c: (i32, i32)| {
            c.0 >= PIN_RADIUS && c.1 >= PIN_RADIUS && c.0 <= w - 1 - PIN_RADIUS && c.1 <= h - 1 - PIN_RADIUS
        };
        for corner in [
            (PIN_RADIUS, PIN_RADIUS),
            (w - 1 - PIN_RADIUS, PIN_RADIUS),
            (PIN_RADIUS, h - 1 - PIN_RADIUS),
            (w - 1 - PIN_RADIUS, h - 1 - PIN_RADIUS),
        ] {
            let placed = declutter_pins(&[corner, (corner.0, corner.1)], &[]);
            assert_ne!(placed[1], corner);
            assert!(on_canvas(placed[1]), "corner {corner:?} → {placed:?}");
        }
    }

    #[test]
    fn declutter_keeps_displaced_pins_off_reserved_strips() {
        // Two pins colliding right under the title strip: the second must not
        // be moved up INTO the strip (its first ring candidate is straight up).
        let strip = (8, 8, 300, 44);
        let placed = declutter_pins(&[(100, 58), (100, 58)], &[strip]);
        let c = placed[1];
        let (dx, dy) = (c.0 - c.0.clamp(strip.0, strip.2), c.1 - c.1.clamp(strip.1, strip.3));
        assert!(dx * dx + dy * dy >= PIN_RADIUS * PIN_RADIUS, "{placed:?}");
        assert_ne!(c, (100, 58));
    }

    #[test]
    fn plan_legend_wraps_instead_of_running_off_canvas() {
        // Every entry of a long trip — dot plus "DAY NN" text — stays on the
        // canvas; 11+ days used to start entry 11 past x=640.
        for n in [1usize, 7, 9, 10, 11, 14, 21] {
            let (cols, rows) = plan_legend_grid(n);
            assert!(cols * rows >= n as i32, "n={n} {cols}x{rows}");
            let last_col_x = 12 + (cols - 1) * PLAN_LEGEND_STEP;
            let text_end = last_col_x + 14 + "DAY 21".len() as i32 * 6;
            assert!(text_end <= WIDTH as i32, "n={n} cols={cols} ends at {text_end}");
        }
        assert_eq!(plan_legend_grid(0), (0, 0));
        assert_eq!(plan_legend_grid(3), (3, 1));
        let (cols, rows) = plan_legend_grid(11);
        assert!(rows >= 2 && cols < 11, "11 days must wrap: {cols}x{rows}");
    }

    #[test]
    fn input_sha_tracks_day_even_when_colors_cycle() {
        let at = |day| Point {
            lat: 35.0,
            lon: 135.0,
            color: DAY_COLORS[0],
            kind: Kind::Sightseeing,
            day: Some(day),
            label: String::new(),
        };
        // Day 1 and day 8 share DAY_COLORS[0]; the legend differs, so must the hash.
        assert_ne!(
            map_input_sha256("Sightseeing overview", MapKind::Plan, &[at(1)], &[]),
            map_input_sha256("Sightseeing overview", MapKind::Plan, &[at(8)], &[])
        );
    }

    #[test]
    fn plan_day_legend_comes_from_days_not_colors() {
        let p = |day: Option<u32>, kind: Kind| Point {
            lat: 35.0,
            lon: 135.0,
            color: day
                .map(|d| DAY_COLORS[(d as usize - 1) % DAY_COLORS.len()])
                .unwrap_or([21, 101, 192]),
            kind,
            day,
            label: String::new(),
        };
        // Day 8 shares day 1's color; the legend must still name both, in
        // day order, once each, and skip the day-less hotel pin. An airport
        // day's terminal (the airport inset) names its day like a sight.
        let pts = vec![
            p(Some(8), Kind::Sightseeing),
            p(Some(1), Kind::Sightseeing),
            p(Some(1), Kind::Sightseeing),
            p(Some(3), Kind::Sightseeing),
            p(None, Kind::Hotel),
            p(Some(5), Kind::Airport),
        ];
        let entries = plan_day_legend_entries(&pts);
        let labels: Vec<&str> = entries.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, vec!["DAY 1", "DAY 3", "DAY 5", "DAY 8"]);
        assert_eq!(entries[2].1, DAY_COLORS[4], "the airport pin's day color, not airport orange");
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
            day: None,
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
                day: None,
                label: String::new(),
            },
            Point {
                lat: 35.1,
                lon: 135.2,
                color: [4, 5, 6],
                kind: Kind::Hotel,
                day: None,
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
