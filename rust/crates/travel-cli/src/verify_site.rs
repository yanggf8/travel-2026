//! `travel verify-site [--plan-id <id>] [--contact-sheet <dir>]` — read-only
//! check of what the LIVE dashboard actually serves for each plan
//! (docs/plans/2026-10-30-verify-site.md, phases 1-3: layers 1-4).
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
//! Layer 1 (page / lang / owner / ident / slot / legend / cached / extra):
//! each plan gets a verify token for this run only
//! (`plan_share_tokens.created_by='verify-site'`, minted before its viewer page
//! is fetched and DELETED right after, error or not; leftovers of an
//! interrupted run are removed when the next run starts — so two verify-site
//! runs must not overlap: the second's sweep would delete the first's token
//! mid-check). No lasting token is
//! needed — nothing stays shared. The viewer page is fetched in zh and en,
//! parsed, and judged by the pure functions in `verify_site_page`. Every
//! token-bearing URL reaches curl through `-K -` (stdin), never argv; only
//! token fingerprints are ever printed.
//!
//! Layer 4 (`--contact-sheet <dir>`): one thumbnail-grid PNG per plan
//! (`verify_site_sheet`); advisory, never a verdict.
//!
//! Read-only apart from the run's own verify token rows: no other Turso writes, no
//! Nominatim/OSRM/Overpass calls, no cookies. Plain-text verdict lines; exits
//! non-zero iff any FAIL.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use libsql::Connection;
use sha2::{Digest, Sha256};
use url::Url;

use crate::snapshot_maps as maps;
use crate::verify_site_page::{self as page, Lang, LegendRow, SlotFacts};
use crate::verify_site_sheet::{self as sheet, SheetCell};

pub const USAGE: &str = "travel verify-site [--plan-id <id>] [--contact-sheet <dir>]\n  \
Read-only check of the LIVE dashboard: each map key's live bytes vs map_artifacts \
(sha256 + size; skipped keys must serve the worker placeholder), input-hash freshness \
(recomputed from cached geocodes/road legs only), image content (size, geographic \
background, title + credit), and the viewer page in zh \
and en (page, language, no owner chrome, data-plan-id/data-view, map slots vs \
map_artifacts via data-map-key, the ?v= map URL bytes, legends vs map_legend_stops, \
companion/stay images, voucher PDF). All live plans by default. One verdict line per \
check, then a summary; exits 1 on any FAIL. --contact-sheet <dir> also writes \
<dir>/<plan>.png, a labelled thumbnail grid of the plan's live maps (advisory). The \
viewer page is opened with a verify token minted for this run and deleted right after \
(fingerprints only). Host: TRAVEL_DASHBOARD_HOST (as share-token).";

/// `plan_share_tokens.created_by` of the verify token.
const VERIFY_CREATED_BY: &str = "verify-site";

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
    /// Deploy the dashboard worker (the live page predates a markup change).
    Deploy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Check {
    pub(crate) level: Level,
    pub(crate) detail: String,
    pub(crate) next: Option<Next>,
}

impl Check {
    pub(crate) fn pass(detail: impl Into<String>) -> Self {
        Self { level: Level::Pass, detail: detail.into(), next: None }
    }
    pub(crate) fn warn(detail: impl Into<String>, next: Option<Next>) -> Self {
        Self { level: Level::Warn, detail: detail.into(), next }
    }
    pub(crate) fn fail(detail: impl Into<String>, next: Option<Next>) -> Self {
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
    let args = parse_args(args)?;
    let plan_arg = args.plan;
    // Write tier: the run mints (and deletes) its own verify token per plan.
    let conn = crate::db::connect_write()
        .await
        .map_err(|e| format!("failed to connect to Turso (write tier): {e}"))?;
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

    // A run killed between mint and delete leaves its verify token behind:
    // nothing may stay shared, so the next run removes every such row first.
    let leftover = remove_verify_tokens(&conn, None).await?;
    if leftover > 0 {
        println!("note: removed {leftover} leftover verify token(s) of an interrupted run");
    }

    let host = crate::share_token::dashboard_host();
    let nonce = nonce()?;
    let tmp = std::env::temp_dir().join(format!("travel-verify-site-{nonce}.body"));
    let result = verify_plans(&conn, &plan_ids, &first_plan, &host, &nonce, &tmp, args.contact_sheet.as_deref()).await;
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
            " → unset CLOUDFLARE_API_TOKEN && npx wrangler r2 object delete {}/{plan}/{} --remote",
            maps::BUCKET,
            // Layer-1 keys carry their page language ("zh:day-3.png").
            key.split_once(':').map_or(key, |(_, k)| k)
        )),
        Some(Next::Deploy) => line.push_str(
            " → cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy",
        ),
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
    sheet_dir: Option<&Path>,
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
        let variants = travel_db::repo::days::variant_map(conn, plan, None).await?;
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

        let mut cells: Vec<SheetCell> = Vec::new();
        for key in &keys {
            let Some(row) = manifest.get(key) else {
                tally.emit(
                    plan,
                    "asset",
                    key,
                    &Check::fail("MISSING: expected map has no map_artifacts row", Some(Next::Snapshot)),
                );
                cells.push(SheetCell { key: key.clone(), note: "missing".into(), png: None });
                continue;
            };
            let url = format!("https://{host}/map/{plan}/{key}?verify={nonce}");
            match http_get(&url, tmp) {
                Ok(live) => {
                    tally.emit(plan, "asset", key, &asset_verdict(row, &live, &placeholder));
                    if row.status == "uploaded" && live.http == 200 && live.body != placeholder {
                        let report = content_checks(&live.body, key, maps::variant_of_key(&variants, key));
                        if report.findings.is_empty() {
                            tally.emit(plan, "content", key, &Check::pass(report.summary()));
                        }
                        for finding in &report.findings {
                            tally.emit(plan, "content", key, finding);
                        }
                    }
                    let (note, shown) = sheet_note(row, &live, &placeholder);
                    cells.push(SheetCell { key: key.clone(), note, png: shown.then_some(live.body) });
                }
                Err(e) => {
                    tally.emit(plan, "asset", key, &Check::fail(format!("live fetch failed: {e}"), None));
                    cells.push(SheetCell { key: key.clone(), note: "fetch error".into(), png: None });
                }
            }
            if let Some(planned) = &planned {
                tally.emit(plan, "fresh", key, &fresh_verdict(row, planned.get(key)));
            }
        }

        verify_viewer_page(conn, plan, host, tmp, &manifest, &placeholder, &mut tally).await?;

        if let Some(dir) = sheet_dir {
            let path = write_contact_sheet(dir, plan, &cells)?;
            println!("contact-sheet {plan} {} ({} map(s); advisory, not a verdict)", path.display(), cells.len());
        }
    }
    Ok(tally)
}

/// What a contact-sheet cell shows for a key: (label note, draw the live
/// bytes). A real served map gets an empty note.
fn sheet_note(row: &ArtifactRow, live: &Live, placeholder: &[u8]) -> (String, bool) {
    let real = live.http == 200 && live.body != placeholder;
    match row.status.as_str() {
        "uploaded" if real => (String::new(), true),
        "uploaded" if live.http == 200 => ("placeholder".into(), false),
        "uploaded" => (format!("http {}", live.http), false),
        "skipped" if real => ("skipped but served".into(), true),
        other => (other.to_string(), false),
    }
}

fn write_contact_sheet(dir: &Path, plan: &str, cells: &[SheetCell]) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{plan}.png"));
    let png = sheet::render(plan, cells)?;
    fs::write(&path, png).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Layer 1: the token viewer page
// ---------------------------------------------------------------------------

/// DELETE verify-site token rows: one token (`token`), or every leftover one
/// (`None`). Returns rows removed. Share-token ('cli') rows are never touched.
async fn remove_verify_tokens(conn: &Connection, token: Option<&str>) -> Result<u64, String> {
    conn.execute(
        "DELETE FROM plan_share_tokens WHERE created_by = ?1 AND (?2 IS NULL OR token = ?2)",
        libsql::params![VERIFY_CREATED_BY, token.map(str::to_string)],
    )
    .await
    .map_err(|e| format!("plan_share_tokens verify-token delete failed: {e}"))
}

/// Replace every occurrence of the secret with its fingerprint.
fn redact(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, &crate::share_token::token_fingerprint(secret))
}

async fn read_legend_rows(conn: &Connection, plan_id: &str) -> Result<HashMap<String, Vec<LegendRow>>, String> {
    let mut rows = conn
        .query(
            "SELECT map_key, seq, label, lat, lon, day_number, pin_no FROM map_legend_stops \
             WHERE plan_id = ?1 ORDER BY map_key, seq",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("map_legend_stops query failed: {e}"))?;
    let mut out: HashMap<String, Vec<LegendRow>> = HashMap::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("map_legend_stops row read failed: {e}"))?
    {
        let v = |i: i32| row.get_value(i).ok();
        let key = row.get::<String>(0).map_err(|e| format!("map_key read failed: {e}"))?;
        out.entry(key).or_default().push(LegendRow {
            seq: v(1).and_then(value_i64).unwrap_or_default(),
            label: v(2).and_then(|x| match x {
                libsql::Value::Text(s) => Some(s),
                _ => None,
            }),
            lat: v(3).and_then(value_f64),
            lon: v(4).and_then(value_f64),
            day_number: v(5).and_then(value_i64),
            pin_no: v(6).and_then(value_i64),
        });
    }
    Ok(out)
}

fn value_f64(v: libsql::Value) -> Option<f64> {
    match v {
        libsql::Value::Real(f) => Some(f),
        libsql::Value::Integer(i) => Some(i as f64),
        libsql::Value::Text(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn value_i64(v: libsql::Value) -> Option<i64> {
    match v {
        libsql::Value::Integer(i) => Some(i),
        libsql::Value::Text(s) => s.trim().parse().ok(),
        _ => None,
    }
}

async fn has_flights(conn: &Connection, plan_id: &str) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM flight_legs WHERE plan_id = ?1 LIMIT 1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("flight_legs query failed: {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("flight_legs row read failed: {e}"))?
        .is_some())
}

/// Layer 1 for one plan: mint this run's verify token, check the viewer page
/// through it, and delete it again — whether the checks succeeded or not.
async fn verify_viewer_page(
    conn: &Connection,
    plan: &str,
    host: &str,
    tmp: &Path,
    manifest: &HashMap<String, ArtifactRow>,
    placeholder: &[u8],
    tally: &mut Tally,
) -> Result<(), String> {
    let token = crate::share_token::mint_for_plan(conn, plan, VERIFY_CREATED_BY)
        .await
        .map_err(|e| format!("cannot mint this run's verify token for {plan}: {e}"))?;
    let checked = viewer_page_checks(conn, plan, &token, host, tmp, manifest, placeholder, tally).await;
    let fp = crate::share_token::token_fingerprint(&token);
    // The delete outcome is reported even when the checks failed too: a token
    // that survived is the security-relevant half.
    let delete_err = match remove_verify_tokens(conn, Some(&token)).await {
        Ok(1) => None,
        Ok(n) => Some(format!(
            "verify token {fp} of {plan}: delete removed {n} row(s), expected 1 (another verify-site run swept it?)"
        )),
        Err(e) => Some(format!(
            "verify token {fp} of {plan} could not be deleted ({e}); run `travel share-token clear --plan-id {plan}`"
        )),
    };
    match (checked.map_err(|e| redact(&e, &token)), delete_err) {
        (Ok(()), None) => Ok(()),
        (Err(e), None) => Err(e),
        (Ok(()), Some(d)) => Err(d),
        (Err(e), Some(d)) => Err(format!("{e}; {d}")),
    }
}

/// The viewer page (zh + en) through `token`.
#[allow(clippy::too_many_arguments)]
async fn viewer_page_checks(
    conn: &Connection,
    plan: &str,
    token: &str,
    host: &str,
    tmp: &Path,
    manifest: &HashMap<String, ArtifactRow>,
    placeholder: &[u8],
    tally: &mut Tally,
) -> Result<(), String> {
    let day_numbers = crate::check_maps_fresh::read_day_numbers(conn, plan).await?;
    let statuses: HashMap<String, String> = manifest.iter().map(|(k, r)| (k.clone(), r.status.clone())).collect();
    let facts = SlotFacts { day_numbers: &day_numbers, has_flights: has_flights(conn, plan).await?, statuses: &statuses };
    let variants = travel_db::repo::days::variant_map(conn, plan, None).await?;
    let trip_days = day_numbers.len().saturating_sub(variants.len());
    let label_rows = travel_db::repo::days::variant_label_rows(conn, plan, None).await?;
    let labels_for = |lang: Lang| {
        let mut m: HashMap<i64, Vec<String>> = HashMap::new();
        for (main, en, zh) in &label_rows {
            let l = if lang == Lang::Zh { zh } else { en };
            if let Some(l) = l.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
                m.entry(*main).or_default().push(l.to_string());
            }
        }
        m
    };
    let legends = read_legend_rows(conn, plan).await?;
    let base = Url::parse(&format!("https://{host}/")).map_err(|e| format!("dashboard host {host:?}: {e}"))?;
    let plan_url = crate::share_token::share_url(plan, token);

    let mut versioned: Vec<(String, Url)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut zh_doc = None;
    for lang in [Lang::Zh, Lang::En] {
        let code = lang.code();
        let url = match lang {
            Lang::Zh => plan_url.clone(),
            Lang::En => format!("{plan_url}&lang=en"),
        };
        let live = match http_get(&url, tmp) {
            Ok(live) => live,
            Err(e) => {
                tally.emit(plan, "page", code, &Check::fail(format!("page fetch failed: {}", redact(&e, token)), None));
                continue;
            }
        };
        let doc = page::parse_page(&String::from_utf8_lossy(&live.body));
        for (layer, check) in page::page_checks(live.http, &doc, lang, plan) {
            tally.emit(plan, layer, code, &check);
        }
        if !page::page_usable(live.http, &doc) {
            continue;
        }
        for (layer, check) in page::variant_checks(&doc, &variants, &labels_for(lang), trip_days) {
            tally.emit(plan, layer, code, &check);
        }
        for (key, check) in page::slot_checks(&doc.figures, &facts) {
            tally.emit(plan, "slot", &format!("{code}:{key}"), &check);
        }
        for fig in &doc.figures {
            let Some(key) = fig.key.as_deref() else { continue };
            let rows = legends.get(key).map(Vec::as_slice).unwrap_or(&[]);
            tally.emit(plan, "legend", &format!("{code}:{key}"), &page::legend_check(fig, rows));
            let Some(src) = fig.img_src.as_deref() else { continue };
            match page::img_src_check(&base, plan, key, src) {
                Ok(Some(u)) => {
                    if seen.insert(u.to_string()) {
                        versioned.push((key.to_string(), u));
                    }
                }
                Ok(None) => tally.emit(
                    plan,
                    "slot",
                    &format!("{code}:{key}"),
                    &Check::warn("<img> has no ?v= cache-buster: viewers may get a CDN copy up to a day old", None),
                ),
                Err(c) => tally.emit(plan, "slot", &format!("{code}:{key}"), &c),
            }
        }
        if lang == Lang::Zh {
            zh_doc = Some(doc);
        }
    }

    // The exact ?v=<etag> URL viewers load, fetched once: same bytes as the manifest?
    for (key, url) in versioned {
        let check = match (manifest.get(&key), http_get(url.as_str(), tmp)) {
            (_, Err(e)) => Check::fail(format!("?v= URL fetch failed: {e}"), None),
            // snapshot-maps never touches a key it has no row for: delete the object.
            (None, Ok(_)) => Check::fail("the page loads a map with no map_artifacts row", Some(Next::DeleteStale)),
            (Some(row), Ok(live)) => {
                let c = asset_verdict(row, &live, placeholder);
                Check { detail: format!("?v= URL viewers load: {}", c.detail), ..c }
            }
        };
        tally.emit(plan, "cached", &key, &check);
    }

    // Extended checks: the language does not change these, so the zh page only.
    if let Some(doc) = zh_doc {
        extended_checks(&doc, plan, &base, token, tmp, tally);
    }
    Ok(())
}

/// Companion option maps, booked-stay photos and location minimaps, voucher PDFs.
fn extended_checks(doc: &page::PageDoc, plan: &str, base: &Url, token: &str, tmp: &Path, tally: &mut Tally) {
    let external = |key: String, src: &str, tally: &mut Tally| {
        let check = match base.join(src) {
            Err(e) => Check::fail(format!("image src does not parse as a URL ({e})"), None),
            Ok(url) => match http_fetch(url.as_str(), tmp, true) {
                Ok(live) => {
                    let c = page::external_image_verdict(live.http, &live.content_type, live.body.len());
                    Check { detail: format!("{}: {}", url.host_str().unwrap_or("?"), c.detail), ..c }
                }
                Err(e) => Check::warn(format!("{}: fetch failed: {e}", url.host_str().unwrap_or("?")), None),
            },
        };
        tally.emit(plan, "extra", &key, &check);
    };
    if let Some(c) = &doc.companion {
        if c.cards == 0 {
            tally.emit(plan, "extra", "companion", &Check::fail("companion section with no option cards", None));
        }
        if c.stopless_options > 0 {
            tally.emit(
                plan,
                "extra",
                "companion",
                &Check::warn(
                    format!("{} companion option(s) render an empty stop list (companion_option_stops)", c.stopless_options),
                    None,
                ),
            );
        }
        for (i, src) in dedup(&c.img_srcs).into_iter().enumerate() {
            external(format!("companion-map-{}", i + 1), src, tally);
        }
    }
    for (i, src) in dedup(&doc.stay_photos).into_iter().enumerate() {
        external(format!("stay-photo-{}", i + 1), src, tally);
    }
    for (i, src) in dedup(&doc.stay_minimaps).into_iter().enumerate() {
        external(format!("stay-minimap-{}", i + 1), src, tally);
    }
    for (i, href) in dedup(&doc.voucher_hrefs).into_iter().enumerate() {
        let check = match page::voucher_target(base, href, token) {
            Err(c) => c,
            Ok(url) => match http_get(url.as_str(), tmp) {
                Ok(live) => page::voucher_verdict(live.http, &live.body),
                Err(e) => Check::fail(format!("voucher fetch failed: {}", redact(&e, token)), None),
            },
        };
        tally.emit(plan, "extra", &format!("voucher-{}", i + 1), &check);
    }
}

fn dedup(items: &[String]) -> Vec<&str> {
    let mut seen = HashSet::new();
    items.iter().map(String::as_str).filter(|s| seen.insert(*s)).collect()
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Args {
    pub(crate) plan: Option<String>,
    pub(crate) contact_sheet: Option<PathBuf>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut out = Args::default();
    let mut i = 0;
    let value = |i: usize, flag: &str| {
        args.get(i + 1)
            .filter(|v| !v.trim().is_empty() && !v.starts_with("--"))
            .cloned()
            .ok_or(format!("{flag} requires a value"))
    };
    while i < args.len() {
        match args[i].as_str() {
            "--plan-id" => {
                out.plan = Some(value(i, "--plan-id")?);
                i += 2;
            }
            "--contact-sheet" => {
                out.contact_sheet = Some(PathBuf::from(value(i, "--contact-sheet")?));
                i += 2;
            }
            other => return Err(format!("unexpected argument: {other}\nUsage:\n  {USAGE}")),
        }
    }
    Ok(out)
}

/// Per-run cache-buster: the worker resolves R2 objects by path only, so a
/// fresh query string reaches the worker instead of any cached response.
fn nonce() -> Result<String, String> {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("nonce generation failed: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// GET one URL with curl, no redirects (the dashboard worker answers directly).
fn http_get(url: &str, body_path: &Path) -> Result<Live, String> {
    http_fetch(url, body_path, false)
}

/// The curl config line that hands `url` to curl on stdin (`-K -`), so a
/// token-bearing URL never appears in the process arguments. Quoted per
/// curl's config syntax; a URL with a line break is refused.
pub(crate) fn curl_config_url(url: &str) -> Result<String, String> {
    if url.contains(['\n', '\r']) {
        return Err("refusing a URL with a line break".into());
    }
    Ok(format!("url = \"{}\"\n", url.replace('\\', "\\\\").replace('"', "\\\"")))
}

/// GET one URL with curl. `-q` (first) ignores ~/.curlrc and no cookie options
/// are passed, so no session cookie can ever turn this into an owner request.
/// The URL travels on stdin (`-K -`). `follow` allows https-only redirects
/// (external images, as a browser would follow them).
fn http_fetch(url: &str, body_path: &Path, follow: bool) -> Result<Live, String> {
    // A response with no body leaves no file: never read a previous fetch's bytes.
    let _ = fs::remove_file(body_path);
    let mut cmd = Command::new("curl");
    cmd.args(["-q", "-sS", "--max-time", "30", "--proto", "=https", "-K", "-", "-o"])
        .arg(body_path)
        .args(["-w", "%{http_code}\t%{content_type}"]);
    if follow {
        cmd.args(["-L", "--max-redirs", "5", "--proto-redir", "=https"]);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to run curl: {e}"))?;
    let config = curl_config_url(url)?;
    child
        .stdin
        .take()
        .ok_or("curl stdin unavailable")?
        .write_all(config.as_bytes())
        .map_err(|e| format!("failed to pass the URL to curl: {e}"))?;
    let out = child
        .wait_with_output()
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
    let body = match fs::read(body_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("cannot read {}: {e}", body_path.display())),
    };
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
pub(crate) fn content_checks(bytes: &[u8], key: &str, variant: Option<(i64, &str)>) -> ContentReport {
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
    let title = maps::map_title(key, variant).map(|t| t.to_ascii_uppercase());
    let title_rect = maps::title_rect(title.as_ref().map_or(0, |t| t.chars().count()));
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
        let basemap = content_checks(&test_render::day_map(Bg::Basemap(&textured)), "day-1.png", None);
        assert!(basemap.findings.is_empty(), "{:?}", levels(&basemap));
        assert_eq!(basemap.background, Some(Background::Basemap));

        let roads = content_checks(&test_render::day_map(Bg::Roads), "day-1.png", None);
        assert!(roads.findings.is_empty(), "{:?}", levels(&roads));
        assert_eq!(roads.background, Some(Background::OverpassRoads));
    }

    #[test]
    fn day_away_map_passes_content_checks_and_needs_its_own_title() {
        // `day-{n}-away.png` is not an expected key; verify-site reaches it as an
        // extra manifest key, and sizes its title strip from map_title.
        let textured = test_render::textured_basemap();
        let png = test_render::day_style_map("day-1-away.png", Bg::Basemap(&textured));
        let report = content_checks(&png, "day-1-away.png", None);
        assert!(report.findings.is_empty(), "{:?}", levels(&report));
        assert_eq!(report.background, Some(Background::Basemap));

        let title = maps::title_rect(maps::map_title("day-1-away.png", None).unwrap().len());
        let found = levels(&content_checks(&erase(&png, title), "day-1-away.png", None));
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
        let report = content_checks(&png, "plan-airport.png", None);
        assert!(report.findings.is_empty(), "{:?}", levels(&report));
        assert_eq!(report.background, Some(Background::Basemap));

        let title = maps::title_rect(maps::map_title("plan-airport.png", None).unwrap().len());
        let found = levels(&content_checks(&erase(&png, title), "plan-airport.png", None));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].0 == Level::Fail && found[0].1.contains("title text missing"), "{found:?}");
    }

    #[test]
    fn flat_basemap_is_only_a_warning() {
        let flat = test_render::flat_basemap([170, 211, 223]);
        let report = content_checks(&test_render::day_map(Bg::Basemap(&flat)), "day-1.png", None);
        assert_eq!(report.background, Some(Background::Basemap));
        let found = levels(&report);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, Level::Warn);
        assert!(found[0].1.contains("nearly flat"), "{found:?}");
    }

    #[test]
    fn grid_only_background_fails() {
        let report = content_checks(&test_render::day_map(Bg::Grid), "day-1.png", None);
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
            let title = maps::title_rect(maps::map_title("day-1.png", None).unwrap().len());
            let found = levels(&content_checks(&erase(&png, title), "day-1.png", None));
            assert_eq!(found.len(), 1, "{bg}: {found:?}");
            assert!(found[0].0 == Level::Fail && found[0].1.contains("title text missing"), "{bg}: {found:?}");

            let found = levels(&content_checks(&erase(&png, maps::CREDIT_RECT), "day-1.png", None));
            assert_eq!(found.len(), 1, "{bg}: {found:?}");
            assert!(found[0].0 == Level::Fail && found[0].1.contains("credit text missing"), "{bg}: {found:?}");
        }
    }

    #[test]
    fn wrong_size_and_placeholder_bytes_fail() {
        let small = test_render::encode(&vec![200u8; 320 * 220 * 4], 320, 220);
        let found = levels(&content_checks(&small, "day-1.png", None));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].0 == Level::Fail && found[0].1.contains("320x220"), "{found:?}");

        let found = levels(&content_checks(&placeholder(), "day-1.png", None));
        assert!(found[0].0 == Level::Fail && found[0].1.contains("1x1"), "{found:?}");

        let found = levels(&content_checks(b"not a png", "day-1.png", None));
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
        let report = content_checks(&test_render::day_map(Bg::Basemap(&stamped)), "day-1.png", None);
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
    fn args_parse_plan_contact_sheet_and_token_flags() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_args(&s(&[])).unwrap(), Args::default());
        assert_eq!(parse_args(&s(&["--plan-id", "p"])).unwrap().plan.as_deref(), Some("p"));
        assert!(parse_args(&s(&["--plan-id"])).is_err());
        assert!(parse_args(&s(&["stray"])).is_err());
        let a = parse_args(&s(&["--contact-sheet", "/tmp/cs", "--plan-id", "p"])).unwrap();
        assert_eq!(a.contact_sheet.as_deref(), Some(Path::new("/tmp/cs")));
        assert!(parse_args(&s(&["--contact-sheet"])).is_err());
        // The token is per run now: the old provisioning flags are gone.
        assert!(parse_args(&s(&["--plan-id", "p", "--provision-token"])).is_err());
    }

    #[test]
    fn urls_reach_curl_on_stdin_quoted_and_tokens_are_redacted() {
        assert_eq!(
            curl_config_url("https://h/?plan=p&token=abc").unwrap(),
            "url = \"https://h/?plan=p&token=abc\"\n"
        );
        assert_eq!(curl_config_url("https://h/a\"b\\c").unwrap(), "url = \"https://h/a\\\"b\\\\c\"\n");
        assert!(curl_config_url("https://h/\nurl = \"x\"").is_err());
        let token = "0123456789abcdef0123456789abcdef";
        let msg = format!("curl exited 7: https://h/?token={token}");
        let red = redact(&msg, token);
        assert!(!red.contains(token) && red.contains("012345...abcdef"), "{red}");
    }

    #[test]
    fn layer1_next_steps_name_the_command_and_the_bare_key() {
        let line = format_line("p-1", "slot", "zh:day-3.png", &Check::fail("x", Some(Next::DeleteStale)));
        assert!(line.contains("r2 object delete trip-dashboard-maps/p-1/day-3.png --remote"), "{line}");
        let line = format_line("p-1", "ident", "zh", &Check::fail("x", Some(Next::Deploy)));
        assert!(line.contains("npx wrangler deploy"), "{line}");
    }

    #[test]
    fn sheet_notes_say_what_a_cell_holds() {
        let ph = placeholder();
        let map = test_render::day_map(Bg::Grid);
        assert_eq!(sheet_note(&row("uploaded", &map), &live(200, &map), &ph), (String::new(), true));
        assert_eq!(sheet_note(&row("uploaded", &map), &live(200, &ph), &ph), ("placeholder".into(), false));
        assert_eq!(sheet_note(&row("skipped", b""), &live(200, &ph), &ph), ("skipped".into(), false));
        assert_eq!(sheet_note(&row("skipped", b""), &live(200, &map), &ph), ("skipped but served".into(), true));
        assert_eq!(sheet_note(&row("failed", b""), &live(200, &ph), &ph), ("failed".into(), false));
    }

    /// Live smoke (TRAVEL_VERIFY_SITE_LIVE=1 only): the real worker's
    /// placeholder probe is recognised, and its token-less plan page is an auth
    /// page the layer-1 parser sees as such — no cookies, URL on stdin.
    #[test]
    fn live_smoke_against_the_dashboard() {
        if std::env::var("TRAVEL_VERIFY_SITE_LIVE").as_deref() != Ok("1") {
            eprintln!("skipping live verify-site smoke (set TRAVEL_VERIFY_SITE_LIVE=1)");
            return;
        }
        let host = crate::share_token::dashboard_host();
        let n = nonce().unwrap();
        let tmp = std::env::temp_dir().join(format!("travel-verify-site-smoke-{n}.body"));
        let probe = http_get(&format!("https://{host}/map/__smoke__/__absent__{n}.png?verify={n}"), &tmp).unwrap();
        assert_eq!(check_placeholder(&probe), Ok(None));
        let page_live = http_get(&format!("https://{host}/?plan=__smoke__&token=00000000000000000000000000000000"), &tmp).unwrap();
        let _ = fs::remove_file(&tmp);
        let doc = page::parse_page(&String::from_utf8_lossy(&page_live.body));
        assert!(doc.auth_page, "a bad token must land on an auth page (HTTP {})", page_live.http);
        assert!(!page::page_usable(page_live.http, &doc));
    }
}
