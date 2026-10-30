//! `travel verify-site [--plan-id <id>]` — read-only check of what the LIVE
//! dashboard actually serves for each plan's maps
//! (docs/plans/2026-10-30-verify-site.md, phase 1: layers 2 and 3).
//!
//! Layer 2 (asset / fresh): every expected map key (check-maps-fresh's own key
//! list) and every other `map_artifacts` key is fetched from
//! `https://<dashboard host>/map/<plan>/<key>?verify=<nonce>` and compared with
//! the manifest: an `uploaded` key must serve exactly the recorded bytes (sha256
//! + size), a `skipped` key must serve the worker placeholder, a `failed` key
//! always FAILs. The placeholder is learned LIVE once per run from a key that
//! cannot exist, not copied from the worker. Freshness recomputes the map input
//! hash with snapshot-maps' own assembly (`snapshot_maps::assemble_maps`) in
//! cache-only mode and compares it with `map_artifacts.input_sha256` — this sees
//! route-only edits that the timestamp lint cannot.
//!
//! Layer 3 (content): pure checks over the decoded live PNG of every `uploaded`
//! key whose live bytes are a real image (identical to the manifest or not —
//! when they differ, the live bytes are what viewers see): size, geographic
//! background (not the bare grid), title and © credit ink in their reserved
//! strips (FAIL); flatness and CARTO-style repeated text stamps (WARN only).
//!
//! Read-only: no Turso writes, no Nominatim/OSRM/ArcGIS/Overpass calls, no
//! cookies. Plain-text verdict lines; exits non-zero iff any FAIL.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::Cursor,
    path::Path,
    process::Command,
};

use libsql::Connection;
use sha2::{Digest, Sha256};

use crate::snapshot_maps as maps;

pub const USAGE: &str = "travel verify-site [--plan-id <id>]\n  \
Read-only check of the LIVE dashboard maps: each map key's live bytes vs map_artifacts \
(sha256 + size; skipped keys must serve the worker placeholder), input-hash freshness \
(recomputed from cached geocodes/road legs only), and image content (size, geographic \
background, title + credit). All live plans by default. One verdict line per check, \
then a summary; exits 1 on any FAIL. Host: TRAVEL_DASHBOARD_HOST (as share-token).";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Level {
    Pass,
    Warn,
    Fail,
}

/// The command a non-PASS verdict points the agent at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Next {
    /// Re-render and re-upload the plan's maps.
    Snapshot,
    /// Delete a lingering R2 object the manifest says must not exist.
    DeleteStale,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Check {
    pub(crate) level: Level,
    pub(crate) detail: String,
    pub(crate) next: Option<Next>,
}

impl Check {
    fn pass(detail: impl Into<String>) -> Self {
        Self { level: Level::Pass, detail: detail.into(), next: None }
    }
    fn warn(detail: impl Into<String>, next: Option<Next>) -> Self {
        Self { level: Level::Warn, detail: detail.into(), next }
    }
    fn fail(detail: impl Into<String>, next: Option<Next>) -> Self {
        Self { level: Level::Fail, detail: detail.into(), next }
    }
}

/// One `map_artifacts` row.
#[derive(Clone, Debug, Default)]
pub(crate) struct ArtifactRow {
    pub(crate) status: String,
    pub(crate) byte_size: i64,
    pub(crate) sha256: String,
    pub(crate) skip_reason: Option<String>,
    pub(crate) input_sha256: Option<String>,
}

/// One live HTTP response.
#[derive(Clone, Debug)]
pub(crate) struct Live {
    pub(crate) http: u16,
    pub(crate) content_type: String,
    pub(crate) body: Vec<u8>,
}

impl Live {
    fn is_png(&self) -> bool {
        self.content_type
            .split(';')
            .next()
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("image/png"))
    }
}

/// What the current itinerary produces for one key (from `assemble_maps`).
#[derive(Clone, Debug)]
pub(crate) struct PlannedKey {
    /// None = the current inputs produce no map (snapshot-maps records a skip).
    pub(crate) input_sha256: Option<String>,
    pub(crate) geocode_misses: Vec<String>,
}

pub async fn run(args: &[String]) -> Result<(), String> {
    let plan_arg = parse_args(args)?;
    let conn = crate::db::connect_read()
        .await
        .map_err(|e| format!("failed to connect to Turso (read tier): {e}"))?;
    let live_plans = crate::check_maps_fresh::all_live_plans(&conn).await?;
    let plan_ids = match plan_arg {
        Some(id) => {
            if !live_plans.contains(&id) {
                return Err(format!("plan '{id}' not found or soft-deleted (verify-site checks live plans only)"));
            }
            vec![id]
        }
        None => live_plans,
    };
    let Some(first_plan) = plan_ids.first().cloned() else {
        println!("No plans to check.");
        println!("summary: 0 plan(s) · 0 PASS · 0 WARN · 0 FAIL");
        return Ok(());
    };

    let host = crate::share_token::dashboard_host();
    let nonce = nonce()?;
    let tmp = std::env::temp_dir().join(format!("travel-verify-site-{nonce}.body"));
    let result = verify_plans(&conn, &plan_ids, &first_plan, &host, &nonce, &tmp).await;
    let _ = fs::remove_file(&tmp);
    let tally = result?;

    println!(
        "summary: {} plan(s) · {} PASS · {} WARN · {} FAIL",
        plan_ids.len(),
        tally.pass,
        tally.warn,
        tally.fail
    );
    if tally.fail > 0 {
        return Err(format!("verify-site: {} FAIL(s)", tally.fail));
    }
    Ok(())
}

#[derive(Default)]
struct Tally {
    pass: usize,
    warn: usize,
    fail: usize,
}

impl Tally {
    fn emit(&mut self, plan: &str, layer: &str, key: &str, check: &Check) {
        match check.level {
            Level::Pass => self.pass += 1,
            Level::Warn => self.warn += 1,
            Level::Fail => self.fail += 1,
        }
        println!("{}", format_line(plan, layer, key, check));
    }
}

fn format_line(plan: &str, layer: &str, key: &str, check: &Check) -> String {
    let tag = match check.level {
        Level::Pass => "PASS",
        Level::Warn => "WARN",
        Level::Fail => "FAIL",
    };
    let mut line = format!("{tag} {plan} {layer:<7} {key}  {}", check.detail);
    match check.next {
        Some(Next::Snapshot) => line.push_str(&format!(" → travel snapshot-maps --plan-id {plan}")),
        Some(Next::DeleteStale) => line.push_str(&format!(
            " → unset CLOUDFLARE_API_TOKEN && npx wrangler r2 object delete {}/{plan}/{key} --remote",
            maps::BUCKET
        )),
        None => {}
    }
    line
}

async fn verify_plans(
    conn: &Connection,
    plan_ids: &[String],
    first_plan: &str,
    host: &str,
    nonce: &str,
    tmp: &Path,
) -> Result<Tally, String> {
    // The worker answers every unusable /map/* request with one placeholder PNG
    // (HTTP 200). Learn its bytes from a key that cannot exist.
    let probe = format!("https://{host}/map/{first_plan}/__verify_site_absent__{nonce}.png?verify={nonce}");
    let probed = http_get(&probe, tmp)?;
    let placeholder_defect = check_placeholder(&probed).map_err(|e| format!("{e} (probe {probe})"))?;
    let placeholder = probed.body;

    let mut tally = Tally::default();
    if let Some(defect) = placeholder_defect {
        tally.emit("*", "asset", "placeholder", &defect);
    }
    for plan in plan_ids {
        let expected = crate::check_maps_fresh::expected_keys_for_plan(conn, plan).await?;
        let manifest = read_manifest(conn, plan).await?;
        let mut keys = expected.clone();
        let mut extra: Vec<&String> = manifest.keys().filter(|k| !expected.contains(k)).collect();
        extra.sort();
        keys.extend(extra.into_iter().cloned());

        let planned = match planned_keys(conn, plan).await {
            Ok(p) => Some(p),
            Err(e) => {
                tally.emit(plan, "fresh", "*", &Check::warn(format!("cannot recompute map inputs: {e}"), None));
                None
            }
        };

        for key in &keys {
            let Some(row) = manifest.get(key) else {
                tally.emit(
                    plan,
                    "asset",
                    key,
                    &Check::fail("MISSING: expected map has no map_artifacts row", Some(Next::Snapshot)),
                );
                continue;
            };
            let url = format!("https://{host}/map/{plan}/{key}?verify={nonce}");
            match http_get(&url, tmp) {
                Ok(live) => {
                    tally.emit(plan, "asset", key, &asset_verdict(row, &live, &placeholder));
                    if row.status == "uploaded" && live.http == 200 && live.body != placeholder {
                        let report = content_checks(&live.body, key);
                        if report.findings.is_empty() {
                            tally.emit(plan, "content", key, &Check::pass(report.summary()));
                        }
                        for finding in &report.findings {
                            tally.emit(plan, "content", key, finding);
                        }
                    }
                }
                Err(e) => tally.emit(plan, "asset", key, &Check::fail(format!("live fetch failed: {e}"), None)),
            }
            if let Some(planned) = &planned {
                tally.emit(plan, "fresh", key, &fresh_verdict(row, planned.get(key)));
            }
        }
    }
    Ok(tally)
}

fn parse_args(args: &[String]) -> Result<Option<String>, String> {
    let mut plan = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--plan-id" => {
                let v = args
                    .get(i + 1)
                    .filter(|v| !v.trim().is_empty() && !v.starts_with("--"))
                    .ok_or("--plan-id requires a value")?;
                plan = Some(v.clone());
                i += 2;
            }
            other => return Err(format!("unexpected argument: {other}\nUsage:\n  {USAGE}")),
        }
    }
    Ok(plan)
}

/// Per-run cache-buster: the worker resolves R2 objects by path only, so a
/// fresh query string reaches the worker instead of any cached response.
fn nonce() -> Result<String, String> {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("nonce generation failed: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// GET one URL with curl. `-q` (first) ignores ~/.curlrc and no cookie options
/// are passed, so no session cookie can ever turn this into an owner request.
fn http_get(url: &str, body_path: &Path) -> Result<Live, String> {
    let out = Command::new("curl")
        .args(["-q", "-sS", "--max-time", "30", "--proto", "=https", "-o"])
        .arg(body_path)
        .args(["-w", "%{http_code}\t%{content_type}"])
        .arg(url)
        .output()
        .map_err(|e| format!("failed to run curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "curl exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let meta = String::from_utf8_lossy(&out.stdout);
    let (code, content_type) = meta.split_once('\t').unwrap_or((meta.as_ref(), ""));
    let http = code
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("curl reported no HTTP status ({meta:?})"))?;
    let body = fs::read(body_path).map_err(|e| format!("cannot read {}: {e}", body_path.display()))?;
    Ok(Live { http, content_type: content_type.trim().to_string(), body })
}

/// The placeholder probe must be identifiable as the worker's 1×1 PNG
/// placeholder (HTTP 200, PNG signature, 1×1 IHDR); anything else means the
/// host is not the dashboard worker (or the worker changed), no `/map/*`
/// verdict could be trusted, and the run aborts (Err). A placeholder that is
/// identifiable but does not decode is itself a site defect: it is returned as
/// a FAIL verdict and the run continues (its bytes still identify the
/// placeholder exactly).
pub(crate) fn check_placeholder(live: &Live) -> Result<Option<Check>, String> {
    if live.http != 200 {
        return Err(format!(
            "worker placeholder probe returned HTTP {} (expected 200) — is TRAVEL_DASHBOARD_HOST the dashboard worker?",
            live.http
        ));
    }
    match png_header_dims(&live.body) {
        Some((1, 1)) => {}
        Some((w, h)) => {
            return Err(format!(
                "worker placeholder probe returned a {w}x{h} PNG (expected the 1x1 placeholder)"
            ));
        }
        None => {
            return Err(format!(
                "worker placeholder probe returned {} bytes that are not a PNG (content-type {:?})",
                live.body.len(),
                live.content_type
            ));
        }
    }
    if png_dims(&live.body) == Some((1, 1)) {
        return Ok(None);
    }
    Ok(Some(Check::fail(
        format!(
            "worker placeholder ({} bytes) has a 1x1 PNG header but does not decode (corrupt chunk data/CRC): \
             a browser cannot draw it wherever an <img> hits a missing map — fix PLACEHOLDER_PNG in \
             workers/trip-dashboard-rs/src/router.rs",
            live.body.len()
        ),
        None,
    )))
}

/// Width × height from a PNG's signature + IHDR, without touching pixel data.
fn png_header_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let be = |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    Some((be(16), be(20)))
}

/// Decode a PNG's first frame and return its size; None if it is not a valid PNG.
fn png_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().ok()?;
    let (w, h) = (reader.info().width, reader.info().height);
    // Refuse absurd headers before allocating a frame buffer for them.
    if u64::from(w) * u64::from(h) > 4096 * 4096 {
        return None;
    }
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    Some((info.width, info.height))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// Layer 2 verdict for one key: the live response against its manifest row.
/// Pure — every branch is unit-tested.
pub(crate) fn asset_verdict(row: &ArtifactRow, live: &Live, placeholder: &[u8]) -> Check {
    let is_placeholder = live.http == 200 && live.body == placeholder;
    match row.status.as_str() {
        "uploaded" => {
            if live.http != 200 {
                return Check::fail(
                    format!("live HTTP {} (expected 200 image/png)", live.http),
                    Some(Next::Snapshot),
                );
            }
            if is_placeholder {
                return Check::fail(
                    "live = worker placeholder (object missing or rejected as invalid)",
                    Some(Next::Snapshot),
                );
            }
            if !live.is_png() {
                return Check::fail(
                    format!("live content-type {:?} (expected image/png)", live.content_type),
                    Some(Next::Snapshot),
                );
            }
            let sha = sha256_hex(&live.body);
            if sha != row.sha256 || live.body.len() as i64 != row.byte_size {
                return Check::fail(
                    format!(
                        "bytes differ (failed upload or stale cache): live {} bytes sha256 {} vs manifest {} bytes sha256 {}",
                        live.body.len(),
                        short(&sha),
                        row.byte_size,
                        short(&row.sha256)
                    ),
                    Some(Next::Snapshot),
                );
            }
            Check::pass(format!("{} bytes, sha256 match", live.body.len()))
        }
        "failed" => {
            let served = if is_placeholder {
                "worker placeholder".to_string()
            } else {
                format!("HTTP {}, {} bytes", live.http, live.body.len())
            };
            Check::fail(
                format!(
                    "status=failed ({}); live = {served}",
                    row.skip_reason.as_deref().unwrap_or("no reason recorded")
                ),
                Some(Next::Snapshot),
            )
        }
        "skipped" => {
            let reason = row.skip_reason.as_deref().unwrap_or("no reason recorded");
            if is_placeholder {
                Check::pass(format!("skipped ({reason}); live = worker placeholder"))
            } else if live.http == 200 {
                Check::fail(
                    format!(
                        "status=skipped ({reason}) but live serves a real {}-byte image (stale R2 object still served)",
                        live.body.len()
                    ),
                    Some(Next::DeleteStale),
                )
            } else {
                Check::fail(
                    format!("status=skipped ({reason}); live HTTP {} (expected the worker placeholder)", live.http),
                    None,
                )
            }
        }
        other => Check::fail(format!("unknown map_artifacts status {other:?}"), Some(Next::Snapshot)),
    }
}

/// Layer 2 freshness: the recomputed input hash against the recorded one.
/// `planned` is None when the current itinerary no longer produces the key.
pub(crate) fn fresh_verdict(row: &ArtifactRow, planned: Option<&PlannedKey>) -> Check {
    let Some(planned) = planned else {
        return Check::warn(
            "orphan manifest key: the current itinerary no longer produces this map (snapshot-maps does not clean it up)",
            None,
        );
    };
    if !planned.geocode_misses.is_empty() {
        return Check::warn(
            format!(
                "cannot verify freshness (geocode cache miss: {}) — snapshot-maps re-asks Nominatim for these \
                 on every run and leaves a stop off the map when it gets no answer (a non-result is not cached); \
                 pin a real stop with travel set-place-geocode",
                planned.geocode_misses.join(", ")
            ),
            Some(Next::Snapshot),
        );
    }
    match (&planned.input_sha256, &row.input_sha256) {
        (Some(current), Some(recorded)) if current == recorded => Check::pass("input hash match"),
        (Some(_), Some(_)) => Check::warn(
            "current input hash != map_artifacts.input_sha256",
            Some(Next::Snapshot),
        ),
        (Some(_), None) if row.status == "skipped" => Check::warn(
            "current inputs produce a map but map_artifacts says skipped",
            Some(Next::Snapshot),
        ),
        (Some(_), None) => Check::warn(
            "map_artifacts.input_sha256 not recorded (snapshot predates input hashing)",
            Some(Next::Snapshot),
        ),
        (None, _) if row.status == "skipped" => Check::pass("current inputs still produce no map"),
        (None, _) => Check::warn(
            format!("current inputs produce no map but map_artifacts status={}", row.status),
            Some(Next::Snapshot),
        ),
    }
}

async fn read_manifest(conn: &Connection, plan_id: &str) -> Result<HashMap<String, ArtifactRow>, String> {
    let mut rows = conn
        .query(
            "SELECT map_key, status, byte_size, sha256, skip_reason, input_sha256 FROM map_artifacts WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("map_artifacts query failed: {e}"))?;
    let mut out = HashMap::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("map_artifacts row read failed: {e}"))?
    {
        let key = row
            .get::<String>(0)
            .map_err(|e| format!("map_key read failed: {e}"))?;
        out.insert(
            key,
            ArtifactRow {
                status: row.get::<Option<String>>(1).ok().flatten().unwrap_or_default(),
                byte_size: row.get::<Option<i64>>(2).ok().flatten().unwrap_or(0),
                sha256: row.get::<Option<String>>(3).ok().flatten().unwrap_or_default(),
                skip_reason: row.get::<Option<String>>(4).ok().flatten().filter(|s| !s.is_empty()),
                input_sha256: row.get::<Option<String>>(5).ok().flatten().filter(|s| !s.is_empty()),
            },
        );
    }
    Ok(out)
}

/// Recompute every map key's input hash exactly as snapshot-maps would, with
/// the plan's active destination, from cached geocodes and road legs only.
async fn planned_keys(conn: &Connection, plan_id: &str) -> Result<HashMap<String, PlannedKey>, String> {
    let dest = crate::cascade::common::resolve_active_destination(conn, plan_id, None).await?;
    let source = maps::MapSource::load(conn, plan_id, &dest).await?;
    let mut net = maps::DbResolver::new(conn, maps::NetMode::CacheOnly, &dest).await?;
    let planned = maps::assemble_maps(&source, &mut net, plan_id, &dest).await?;
    Ok(planned
        .into_iter()
        .map(|m| {
            let key = PlannedKey {
                input_sha256: m.input_sha256(),
                geocode_misses: m.geocode_misses,
            };
            (m.key, key)
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Layer 3: image content (pure, over decoded RGBA)
// ---------------------------------------------------------------------------

/// Share of the non-reserved canvas in the renderer's grid palette above which
/// the background is the bare grid (an ArcGIS basemap has ~none of it).
const GRID_MIN_SHARE: f64 = 0.40;
/// Connected Overpass road-colored pixels that prove the road-web fallback.
const ROAD_MIN_RUN: usize = 20;
/// Flatness WARN: one color covers this share of the basemap area, or the
/// basemap spans fewer than this many 4-bit color bins.
const FLAT_DOMINANT_SHARE: f64 = 0.90;
const FLAT_MIN_COLOR_BINS: usize = 8;
/// Repeated-text WARN: horizontal luminance steps at least this sharp count as
/// text edges; a stamp repeated every map tile shows up as edges that recur at a
/// tile period.
const EDGE_CONTRAST: i32 = 96;
const REPEAT_PERIODS: [(usize, usize); 3] = [(256, 0), (0, 256), (512, 0)];
const REPEAT_MIN_EDGES: usize = 300;
const REPEAT_MIN_SHARE: f64 = 0.30;
/// Text strips: ink is dark; the strip behind it (white backing over a basemap,
/// the grid without one) is mostly light.
const DARK_LUM: u32 = 128;
const LIGHT_LUM: u32 = 170;
const STRIP_MIN_LIGHT_SHARE: f64 = 0.30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Background {
    /// Not the renderer's grid: an ArcGIS (or other raster) basemap.
    Basemap,
    /// The grid plus an Overpass road web.
    OverpassRoads,
    /// The grid alone: no geographic background.
    GridOnly,
}

pub(crate) struct ContentReport {
    pub(crate) findings: Vec<Check>,
    pub(crate) background: Option<Background>,
}

impl ContentReport {
    fn summary(&self) -> String {
        let bg = match self.background {
            Some(Background::Basemap) => "basemap",
            Some(Background::OverpassRoads) => "overpass road web",
            Some(Background::GridOnly) => "grid only",
            None => "unknown",
        };
        format!("{}x{}, background={bg}, title + credit present", maps::WIDTH, maps::HEIGHT)
    }
}

struct Img<'a> {
    rgba: &'a [u8],
    w: usize,
    h: usize,
}

type Rect = (i32, i32, i32, i32);

impl Img<'_> {
    fn px(&self, x: usize, y: usize) -> [u8; 3] {
        let i = (y * self.w + x) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2]]
    }
    fn lum(&self, x: usize, y: usize) -> u32 {
        let [r, g, b] = self.px(x, y);
        (299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b)) / 1000
    }
}

fn in_any(rects: &[Rect], x: usize, y: usize) -> bool {
    let (x, y) = (x as i32, y as i32);
    rects
        .iter()
        .any(|&(x0, y0, x1, y1)| x >= x0 && x < x1 && y >= y0 && y < y1)
}

/// Layer 3 checks for one live PNG. `key` names the map (its title sizes the
/// reserved title strip). Pure.
pub(crate) fn content_checks(bytes: &[u8], key: &str) -> ContentReport {
    let mut findings = Vec::new();
    let report = |findings, background| ContentReport { findings, background };
    let Some((w, h)) = png_dims(bytes) else {
        findings.push(Check::fail("live bytes are not a decodable PNG", Some(Next::Snapshot)));
        return report(findings, None);
    };
    if (w, h) != (maps::WIDTH, maps::HEIGHT) {
        findings.push(Check::fail(
            format!("image is {w}x{h} (expected {}x{})", maps::WIDTH, maps::HEIGHT),
            Some(Next::Snapshot),
        ));
        return report(findings, None);
    }
    let Some(rgba) = maps::decode_rgba_png(bytes) else {
        findings.push(Check::fail("PNG does not decode to 8-bit RGB(A)", Some(Next::Snapshot)));
        return report(findings, None);
    };
    let img = Img { rgba: &rgba, w: w as usize, h: h as usize };
    let title = maps::map_title(key).map(|t| t.to_ascii_uppercase());
    let title_rect = maps::title_rect(title.as_ref().map_or(0, String::len));
    let reserved = [title_rect, maps::CREDIT_RECT];

    let background = classify_background(&img, &reserved);
    match background {
        Background::GridOnly => findings.push(Check::fail(
            "grid-only background: no ArcGIS basemap and no Overpass road web",
            Some(Next::Snapshot),
        )),
        Background::Basemap => {
            if let Some(w) = flatness_warning(&img, &reserved) {
                findings.push(w);
            }
            if let Some(w) = repeated_text_warning(&img, &reserved) {
                findings.push(w);
            }
        }
        Background::OverpassRoads => {}
    }

    if let Some(title) = &title {
        let need = maps::text_ink(title, maps::TITLE_SCALE) / 2;
        if let Some(detail) = strip_text_missing(&img, title_rect, need) {
            findings.push(Check::fail(format!("title text missing in its strip: {detail}"), Some(Next::Snapshot)));
        }
    }
    let credit_need = maps::text_ink(maps::CREDIT_WITH_BASEMAP, 1).min(maps::text_ink(maps::CREDIT_NO_BASEMAP, 1)) / 2;
    if let Some(detail) = strip_text_missing(&img, maps::CREDIT_RECT, credit_need) {
        findings.push(Check::fail(format!("© credit text missing in its strip: {detail}"), Some(Next::Snapshot)));
    }
    report(findings, Some(background))
}

fn classify_background(img: &Img, reserved: &[Rect]) -> Background {
    let (mut grid, mut total) = (0usize, 0usize);
    for y in 0..img.h {
        for x in 0..img.w {
            if in_any(reserved, x, y) {
                continue;
            }
            total += 1;
            let c = img.px(x, y);
            if c == maps::GRID_BG || c == maps::GRID_LINE {
                grid += 1;
            }
        }
    }
    if (grid as f64) < GRID_MIN_SHARE * total as f64 {
        Background::Basemap
    } else if largest_road_run(img, reserved) >= ROAD_MIN_RUN {
        Background::OverpassRoads
    } else {
        Background::GridOnly
    }
}

/// Size of the largest 8-connected run of Overpass road-colored pixels outside
/// the reserved strips (stops counting once ROAD_MIN_RUN is reached).
fn largest_road_run(img: &Img, reserved: &[Rect]) -> usize {
    let is_road = |x: usize, y: usize| {
        let c = img.px(x, y);
        (c == maps::ROAD_MAJOR || c == maps::ROAD_MINOR) && !in_any(reserved, x, y)
    };
    let mut seen = vec![false; img.w * img.h];
    let mut best = 0;
    for y in 0..img.h {
        for x in 0..img.w {
            if seen[y * img.w + x] || !is_road(x, y) {
                continue;
            }
            let mut size = 0;
            let mut queue = VecDeque::from([(x, y)]);
            seen[y * img.w + x] = true;
            while let Some((cx, cy)) = queue.pop_front() {
                size += 1;
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (nx, ny) = (cx as i32 + dx, cy as i32 + dy);
                        if nx < 0 || ny < 0 || nx >= img.w as i32 || ny >= img.h as i32 {
                            continue;
                        }
                        let (nx, ny) = (nx as usize, ny as usize);
                        if !seen[ny * img.w + nx] && is_road(nx, ny) {
                            seen[ny * img.w + nx] = true;
                            queue.push_back((nx, ny));
                        }
                    }
                }
            }
            best = best.max(size);
            if best >= ROAD_MIN_RUN {
                return best;
            }
        }
    }
    best
}

fn flatness_warning(img: &Img, reserved: &[Rect]) -> Option<Check> {
    let mut counts: HashMap<[u8; 3], usize> = HashMap::new();
    let mut bins: HashSet<[u8; 3]> = HashSet::new();
    let mut total = 0usize;
    for y in 0..img.h {
        for x in 0..img.w {
            if in_any(reserved, x, y) {
                continue;
            }
            let c = img.px(x, y);
            *counts.entry(c).or_default() += 1;
            bins.insert([c[0] >> 4, c[1] >> 4, c[2] >> 4]);
            total += 1;
        }
    }
    let dominant = counts.values().copied().max().unwrap_or(0);
    let share = dominant as f64 / total.max(1) as f64;
    (share >= FLAT_DOMINANT_SHARE || bins.len() < FLAT_MIN_COLOR_BINS).then(|| {
        Check::warn(
            format!(
                "basemap nearly flat: one color covers {:.0}% of the map, {} color bins (blank/error tile, or open sea?)",
                share * 100.0,
                bins.len()
            ),
            None,
        )
    })
}

fn repeated_text_warning(img: &Img, reserved: &[Rect]) -> Option<Check> {
    let mut edge = vec![false; img.w * img.h];
    for y in 0..img.h {
        for x in 0..img.w.saturating_sub(1) {
            if in_any(reserved, x, y) || in_any(reserved, x + 1, y) {
                continue;
            }
            let step = img.lum(x + 1, y) as i32 - img.lum(x, y) as i32;
            edge[y * img.w + x] = step.abs() >= EDGE_CONTRAST;
        }
    }
    let mut best: Option<(usize, usize, (usize, usize))> = None;
    for (dx, dy) in REPEAT_PERIODS {
        if dx >= img.w || dy >= img.h {
            continue;
        }
        let (mut base, mut matches) = (0usize, 0usize);
        for y in 0..img.h - dy {
            for x in 0..img.w - dx {
                if edge[y * img.w + x] {
                    base += 1;
                    if edge[(y + dy) * img.w + x + dx] {
                        matches += 1;
                    }
                }
            }
        }
        if matches >= REPEAT_MIN_EDGES
            && matches as f64 >= REPEAT_MIN_SHARE * base as f64
            && best.is_none_or(|(m, _, _)| matches > m)
        {
            best = Some((matches, base, (dx, dy)));
        }
    }
    best.map(|(matches, base, (dx, dy))| {
        Check::warn(
            format!(
                "large repeated high-contrast text across the basemap ({matches} of {base} edge px recur at a {dx},{dy} px tile period) — CARTO-style \"API KEY REQUIRED\" stamp?"
            ),
            None,
        )
    })
}

/// None when the strip holds at least `need` dark ink pixels on a mostly light
/// strip; otherwise a short description of what is there instead.
fn strip_text_missing(img: &Img, rect: Rect, need: usize) -> Option<String> {
    let (x0, y0, x1, y1) = rect;
    let (mut dark, mut light, mut area) = (0usize, 0usize, 0usize);
    for y in y0.max(0)..y1.min(img.h as i32) {
        for x in x0.max(0)..x1.min(img.w as i32) {
            let l = img.lum(x as usize, y as usize);
            area += 1;
            if l < DARK_LUM {
                dark += 1;
            } else if l >= LIGHT_LUM {
                light += 1;
            }
        }
    }
    let light_ok = light as f64 >= STRIP_MIN_LIGHT_SHARE * area as f64;
    if dark >= need.max(1) && light_ok {
        None
    } else {
        Some(format!(
            "{dark} dark ink px (need ≥{}), {:.0}% light strip",
            need.max(1),
            light as f64 * 100.0 / area.max(1) as f64
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot_maps::test_render::{self, Bg};

    fn row(status: &str, body: &[u8]) -> ArtifactRow {
        ArtifactRow {
            status: status.into(),
            byte_size: body.len() as i64,
            sha256: sha256_hex(body),
            skip_reason: (status != "uploaded").then(|| "why".to_string()),
            input_sha256: Some("abc".into()),
        }
    }

    fn live(http: u16, body: &[u8]) -> Live {
        Live { http, content_type: "image/png".into(), body: body.to_vec() }
    }

    fn placeholder() -> Vec<u8> {
        test_render::encode(&[0, 0, 0, 0], 1, 1)
    }

    #[test]
    fn placeholder_probe_must_be_a_1x1_png() {
        assert_eq!(check_placeholder(&live(200, &placeholder())), Ok(None));
        assert!(check_placeholder(&live(404, &placeholder())).is_err());
        assert!(check_placeholder(&live(200, b"<html>not found</html>")).is_err());
        let big = test_render::encode(&[0u8; 2 * 2 * 4], 2, 2);
        assert!(check_placeholder(&live(200, &big)).unwrap_err().contains("2x2"));
        // Identifiable (signature + 1x1 IHDR) but the chunk data is corrupt:
        // a FAIL verdict about the worker, not an aborted run.
        let mut corrupt = placeholder();
        let idat = corrupt.windows(4).position(|w| w == b"IDAT").unwrap();
        corrupt[idat + 6] ^= 0xff;
        let defect = check_placeholder(&live(200, &corrupt)).unwrap().unwrap();
        assert_eq!(defect.level, Level::Fail);
        assert!(defect.detail.contains("does not decode"), "{defect:?}");
    }

    #[test]
    fn uploaded_passes_only_on_identical_bytes() {
        let ph = placeholder();
        let map = test_render::day_map(Bg::Grid);
        let r = row("uploaded", &map);
        assert_eq!(asset_verdict(&r, &live(200, &map), &ph).level, Level::Pass);

        // Worker placeholder: object missing or rejected as invalid.
        let v = asset_verdict(&r, &live(200, &ph), &ph);
        assert_eq!((v.level, v.next), (Level::Fail, Some(Next::Snapshot)));
        assert!(v.detail.contains("worker placeholder (object missing or rejected as invalid)"), "{v:?}");

        // Different bytes: failed upload or stale cache.
        let other = test_render::day_map(Bg::Roads);
        let v = asset_verdict(&r, &live(200, &other), &ph);
        assert_eq!(v.level, Level::Fail);
        assert!(v.detail.contains("bytes differ (failed upload or stale cache)"), "{v:?}");

        // Same sha but a manifest size that disagrees still fails.
        let mut sized = r.clone();
        sized.byte_size += 1;
        assert_eq!(asset_verdict(&sized, &live(200, &map), &ph).level, Level::Fail);

        // Non-200 and non-PNG responses fail.
        assert_eq!(asset_verdict(&r, &live(503, &map), &ph).level, Level::Fail);
        let mut html = live(200, &map);
        html.content_type = "text/html".into();
        assert!(asset_verdict(&r, &html, &ph).detail.contains("content-type"));
    }

    #[test]
    fn failed_always_fails_with_its_reason() {
        let ph = placeholder();
        let r = ArtifactRow {
            status: "failed".into(),
            skip_reason: Some("Wrangler upload failed".into()),
            ..ArtifactRow::default()
        };
        for body in [ph.clone(), test_render::day_map(Bg::Grid)] {
            let v = asset_verdict(&r, &live(200, &body), &ph);
            assert_eq!((v.level, v.next), (Level::Fail, Some(Next::Snapshot)));
            assert!(v.detail.contains("Wrangler upload failed"), "{v:?}");
        }
    }

    #[test]
    fn skipped_must_serve_the_placeholder() {
        let ph = placeholder();
        let r = row("skipped", b"");
        assert_eq!(asset_verdict(&r, &live(200, &ph), &ph).level, Level::Pass);
        let v = asset_verdict(&r, &live(200, &test_render::day_map(Bg::Grid)), &ph);
        assert_eq!((v.level, v.next), (Level::Fail, Some(Next::DeleteStale)));
        assert!(v.detail.contains("stale R2 object still served"), "{v:?}");
        assert_eq!(asset_verdict(&r, &live(500, &ph), &ph).level, Level::Fail);
        let line = format_line("p-1", "asset", "day-3.png", &v);
        assert!(line.contains("npx wrangler r2 object delete trip-dashboard-maps/p-1/day-3.png --remote"), "{line}");
    }

    #[test]
    fn unknown_status_fails() {
        let ph = placeholder();
        let v = asset_verdict(&row("pending", b"x"), &live(200, &ph), &ph);
        assert_eq!(v.level, Level::Fail);
    }

    #[test]
    fn fresh_verdict_compares_input_hashes() {
        let r = row("uploaded", b"png");
        let planned = |sha: Option<&str>, misses: &[&str]| PlannedKey {
            input_sha256: sha.map(str::to_string),
            geocode_misses: misses.iter().map(|s| s.to_string()).collect(),
        };
        assert_eq!(fresh_verdict(&r, Some(&planned(Some("abc"), &[]))).level, Level::Pass);
        let v = fresh_verdict(&r, Some(&planned(Some("def"), &[])));
        assert_eq!((v.level, v.next), (Level::Warn, Some(Next::Snapshot)));
        let v = fresh_verdict(&r, Some(&planned(Some("abc"), &["貴船神社"])));
        assert_eq!(v.level, Level::Warn);
        assert!(v.detail.contains("cannot verify freshness (geocode cache miss: 貴船神社)"), "{v:?}");
        // Inputs now produce no map, but an upload is recorded.
        assert_eq!(fresh_verdict(&r, Some(&planned(None, &[]))).level, Level::Warn);
        // A skip that is still a skip is fresh; a skip that would now render is not.
        let mut skipped = row("skipped", b"");
        skipped.input_sha256 = None;
        assert_eq!(fresh_verdict(&skipped, Some(&planned(None, &[]))).level, Level::Pass);
        assert_eq!(fresh_verdict(&skipped, Some(&planned(Some("abc"), &[]))).level, Level::Warn);
        // A key the itinerary no longer produces.
        assert_eq!(fresh_verdict(&r, None).level, Level::Warn);
    }

    fn levels(report: &ContentReport) -> Vec<(Level, String)> {
        report.findings.iter().map(|f| (f.level, f.detail.clone())).collect()
    }

    #[test]
    fn known_good_maps_pass_content_checks() {
        let textured = test_render::textured_basemap();
        let basemap = content_checks(&test_render::day_map(Bg::Basemap(&textured)), "day-1.png");
        assert!(basemap.findings.is_empty(), "{:?}", levels(&basemap));
        assert_eq!(basemap.background, Some(Background::Basemap));

        let roads = content_checks(&test_render::day_map(Bg::Roads), "day-1.png");
        assert!(roads.findings.is_empty(), "{:?}", levels(&roads));
        assert_eq!(roads.background, Some(Background::OverpassRoads));
    }

    #[test]
    fn day_away_map_passes_content_checks_and_needs_its_own_title() {
        // `day-{n}-away.png` is not an expected key; verify-site reaches it as an
        // extra manifest key, and sizes its title strip from map_title.
        let textured = test_render::textured_basemap();
        let png = test_render::day_style_map("day-1-away.png", Bg::Basemap(&textured));
        let report = content_checks(&png, "day-1-away.png");
        assert!(report.findings.is_empty(), "{:?}", levels(&report));
        assert_eq!(report.background, Some(Background::Basemap));

        let title = maps::title_rect(maps::map_title("day-1-away.png").unwrap().len());
        let found = levels(&content_checks(&erase(&png, title), "day-1-away.png"));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].0 == Level::Fail && found[0].1.contains("title text missing"), "{found:?}");
    }

    #[test]
    fn plan_airport_map_passes_content_checks_and_needs_its_own_title() {
        // `plan-airport.png` is conditional (only plans with an airport day), so
        // it is not an expected key either; verify-site reaches it as an extra
        // manifest key, and sizes its title strip from map_title.
        let textured = test_render::textured_basemap();
        let png = test_render::plan_style_map("plan-airport.png", &textured);
        let report = content_checks(&png, "plan-airport.png");
        assert!(report.findings.is_empty(), "{:?}", levels(&report));
        assert_eq!(report.background, Some(Background::Basemap));

        let title = maps::title_rect(maps::map_title("plan-airport.png").unwrap().len());
        let found = levels(&content_checks(&erase(&png, title), "plan-airport.png"));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].0 == Level::Fail && found[0].1.contains("title text missing"), "{found:?}");
    }

    #[test]
    fn flat_basemap_is_only_a_warning() {
        let flat = test_render::flat_basemap([170, 211, 223]);
        let report = content_checks(&test_render::day_map(Bg::Basemap(&flat)), "day-1.png");
        assert_eq!(report.background, Some(Background::Basemap));
        let found = levels(&report);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, Level::Warn);
        assert!(found[0].1.contains("nearly flat"), "{found:?}");
    }

    #[test]
    fn grid_only_background_fails() {
        let report = content_checks(&test_render::day_map(Bg::Grid), "day-1.png");
        assert_eq!(report.background, Some(Background::GridOnly));
        let found = levels(&report);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, Level::Fail);
        assert!(found[0].1.contains("grid-only background"), "{found:?}");
    }

    fn erase(png: &[u8], rect: (i32, i32, i32, i32)) -> Vec<u8> {
        let mut rgba = maps::decode_rgba_png(png).unwrap();
        let w = maps::WIDTH as i32;
        for y in rect.1..rect.3 {
            for x in rect.0..rect.2 {
                let i = ((y * w + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        test_render::encode(&rgba, maps::WIDTH, maps::HEIGHT)
    }

    #[test]
    fn erased_title_or_credit_fails_on_both_backgrounds() {
        let textured = test_render::textured_basemap();
        for (png, bg) in [
            (test_render::day_map(Bg::Basemap(&textured)), "basemap"),
            (test_render::day_map(Bg::Roads), "roads"),
        ] {
            let title = maps::title_rect(maps::map_title("day-1.png").unwrap().len());
            let found = levels(&content_checks(&erase(&png, title), "day-1.png"));
            assert_eq!(found.len(), 1, "{bg}: {found:?}");
            assert!(found[0].0 == Level::Fail && found[0].1.contains("title text missing"), "{bg}: {found:?}");

            let found = levels(&content_checks(&erase(&png, maps::CREDIT_RECT), "day-1.png"));
            assert_eq!(found.len(), 1, "{bg}: {found:?}");
            assert!(found[0].0 == Level::Fail && found[0].1.contains("credit text missing"), "{bg}: {found:?}");
        }
    }

    #[test]
    fn wrong_size_and_placeholder_bytes_fail() {
        let small = test_render::encode(&vec![200u8; 320 * 220 * 4], 320, 220);
        let found = levels(&content_checks(&small, "day-1.png"));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].0 == Level::Fail && found[0].1.contains("320x220"), "{found:?}");

        let found = levels(&content_checks(&placeholder(), "day-1.png"));
        assert!(found[0].0 == Level::Fail && found[0].1.contains("1x1"), "{found:?}");

        let found = levels(&content_checks(b"not a png", "day-1.png"));
        assert!(found[0].0 == Level::Fail && found[0].1.contains("not a decodable PNG"), "{found:?}");
    }

    #[test]
    fn repeated_stamp_text_is_a_warning() {
        let mut stamped = test_render::textured_basemap();
        for y in [70, 70 + 256] {
            for x in [20, 20 + 256, 20 + 512] {
                test_render::stamp_text(&mut stamped, x, y, "API KEY REQUIRED", [90, 90, 90], 2);
            }
        }
        let report = content_checks(&test_render::day_map(Bg::Basemap(&stamped)), "day-1.png");
        let found = levels(&report);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, Level::Warn);
        assert!(found[0].1.contains("repeated high-contrast text"), "{found:?}");
    }

    #[test]
    fn line_format_is_one_parseable_verdict() {
        let line = format_line(
            "kyoto-nov-2026",
            "asset",
            "day-1.png",
            &Check::fail("live = worker placeholder", Some(Next::Snapshot)),
        );
        assert_eq!(
            line,
            "FAIL kyoto-nov-2026 asset   day-1.png  live = worker placeholder → travel snapshot-maps --plan-id kyoto-nov-2026"
        );
    }

    #[test]
    fn args_accept_only_plan_id() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_args(&s(&[])).unwrap(), None);
        assert_eq!(parse_args(&s(&["--plan-id", "p"])).unwrap().as_deref(), Some("p"));
        assert!(parse_args(&s(&["--plan-id"])).is_err());
        assert!(parse_args(&s(&["stray"])).is_err());
    }
}
