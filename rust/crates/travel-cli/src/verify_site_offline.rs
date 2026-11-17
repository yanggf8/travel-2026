//! Disposable CDP checks for the offline dashboard contract.
//!
//! Evidence model (Round 2 review): `served=cache`, the banner and cached
//! assets are NOT proof of offline — the SW also serves the cache on 5xx and
//! on the 4s timeout while still online. Every offline case therefore also
//! makes the page fetch a never-cached same-origin URL
//! (`/__td/offline-probe?n=<nonce>`) and requires the fetch to REJECT with a
//! TypeError (a network failure); any HTTP answer, 5xx included, fails the
//! case. No such evidence → FAIL.
//!
//! Documented limit (integrator decision, Round 3): that probe proves only
//! the PAGE target lost network — not the SW's own `self.fetch`.
//! `set_offline` enumerates the context's SW targets at call time (T6 sets
//! overrides only after SAVE, while the SW runs); an SW that starts or
//! restarts after the call without the override keeps network for its own
//! fetches. A false PASS then needs BOTH the missed override AND the live
//! site answering 5xx / timing out at that exact moment — with a normal 200
//! the SW serves net, the page reports `served=net`, and the case FAILs. No
//! probe hook is added to `sw.js` for testing; every offline case's pass
//! text repeats this limit.
//!
//! partial-keep (spec §8 1b): the harness NEVER deletes or restores the asset
//! under test. The fault hits an unversioned /voucher/ or /map/ URL — an
//! asset every SAVE re-fetches while the old copy is already cached — so an
//! SW that wrongly dropped the old copy cannot pass. The case also checks
//! that the kept reply's failure list names the faulted asset, that the
//! offline copy's data-rendered-at is still the pre-fault one, that the old
//! manifest entry is unchanged (complete:true, same asset list), and — from
//! the Cache API (`caches.match` on the exact key), never by loading the
//! asset, because the HTTP cache (`max-age=86400` on /map/) could serve a
//! deleted entry — that every old asset still matches its key. When the
//! plan's data cannot produce the condition (every local asset versioned),
//! the case WARNs with the reason instead of faking it through the cache.
//!
//! revoke / late-403: the token's td-viewer/td-index cache keys are recorded
//! before revocation; after the 403 the Cache API is re-read — the keys must
//! be gone and the surviving token's shared map still cached. The late-403
//! context holds ONLY token3's copies, so it records every key — maps
//! included (map keys carry no token param; in that isolated context no
//! other manifest references them, so the purge must take them too).

use std::time::Duration;

use bridge_core::cdp::{BridgeBrowser, IsolatedPage};
use libsql::Connection;
use serde::Deserialize;
use travel_db::repo::plan_documents::PlanDocument;

use crate::verify_site::{self, Check, Level, Tally};
use crate::verify_site_render::scrub;

pub(crate) const LAYER: &str = "offline";

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct OfflineProbe {
    view: String,
    served: String,
    banner: bool,
    state: String,
    rendered_at: String,
    broken_maps: Vec<String>,
    vouchers: Vec<(String, bool)>,
    /// `ul.offline-failed` items of the ONLINE page that reported the partial
    /// SAVE (empty on freshly loaded pages).
    failed: Vec<String>,
    miss_page: bool,
    offline_evidence: bool,
    /// outerHTML of today's `section[data-date]` card (empty when absent):
    /// today_open/today_marked are judged in Rust over this real fragment
    /// with direct-child selectors, so a nested open block cannot satisfy
    /// them (Round 2 finding 4).
    today_html: String,
    today_count: i64,
    now_hidden: bool,
}

/// The manifest body the SW stores at `/__td/manifest?k=<page key>`
/// (`sw.js` saveCopy: `{assets:[url…], complete:bool, renderedAt:ms}`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManifestBody {
    complete: bool,
    rendered_at: i64,
    assets: Vec<String>,
}

/// The async checks (voucher fetches + the offline-evidence probe) finish into
/// window.__tdProbe; evaluate_json itself does not await a promise. Local
/// links only, including hotel voucher links.
pub(crate) const OFFLINE_PROBE_JS: &str = r#"(function(){
  var b=document.body, os=document.querySelector('#offline-status'), bn=document.querySelector('#offline-banner');
  var maps=Array.from(document.querySelectorAll('img[src^="/map/"]')).filter(function(i){return !i.complete||i.naturalWidth===0;}).map(function(i){return i.getAttribute('src');});
  var links=Array.from(document.querySelectorAll('a.doc-link,a.voucher-link')).filter(function(a){return a.getAttribute('href')&&a.getAttribute('href').indexOf('/voucher/')===0;});
  var failed=Array.from(document.querySelectorAll('#offline-status .offline-failed li')).map(function(li){return (li.textContent||'').replace(/\s+/g,' ').replace(/^ | $/g,'');});
  var tz=b&&b.getAttribute('data-tz'),localToday='';
  try{var pts=new Intl.DateTimeFormat('en-US',{timeZone:tz,year:'numeric',month:'2-digit',day:'2-digit'}).formatToParts(new Date()),o={};pts.forEach(function(x){o[x.type]=x.value;});localToday=o.year+'-'+o.month+'-'+o.day;}catch(e){}
  var sec=localToday?document.querySelector('section[data-date="'+localToday+'"]'):null;
  var now=document.querySelector('#sec-now');
  var p={view:b&&b.getAttribute('data-view')||'',served:b&&b.getAttribute('data-served')||'',banner:!!(bn&&!bn.hidden),state:os&&os.getAttribute('data-state')||'',renderedAt:b&&b.getAttribute('data-rendered-at')||'',brokenMaps:maps,vouchers:[],failed:failed,missPage:!!(b&&!b.hasAttribute('data-plan-id') && /這一頁還沒存到這支手機|This page has not been saved on this phone/.test(b.textContent||'')),offlineEvidence:false,todayHtml:sec?sec.outerHTML:'',todayCount:document.querySelectorAll('[data-today]').length,nowHidden:!now||now.hidden||now.hasAttribute('data-now-pinned')};
  window.__tdProbe=null;
  var nonce=''+Date.now()+'-'+Math.random().toString(36).slice(2);
  var net=fetch('/__td/offline-probe?n='+nonce).then(function(r){return false;},function(e){return e instanceof TypeError;});
  var vs=links.map(function(a){var href=a.getAttribute('href');return fetch(href).then(function(r){if(r.status!==200)return [href,false];return r.arrayBuffer().then(function(ab){var v=new Uint8Array(ab);return [href,v.length>=4&&String.fromCharCode(v[0],v[1],v[2],v[3])==='%PDF'];});}).catch(function(){return [href,false];});});
  Promise.all([net].concat(vs)).then(function(res){p.offlineEvidence=!!res[0];p.vouchers=res.slice(1);window.__tdProbe=p;},function(){window.__tdProbe=p;});
  return true;
})()"#;

const OFFLINE_EVIDENCE_MISSING: &str = "no offline-only evidence: the never-cached same-origin probe request (/__td/offline-probe) answered over HTTP instead of rejecting with a network TypeError — served=cache alone cannot tell offline from a 5xx/timeout fallback while online";

/// Read one manifest entry's body from td-viewer by its exact cache key.
/// `__KEY__` is the manifest URL. Same flag/var pattern as `cache_keys`:
/// evaluate_json does not await promises, so the result lands in a window
/// variable first (`__tdManifestRead` is the done-flag; the value can be
/// null when the entry is gone).
const MANIFEST_READ_JS: &str = r#"(function(){window.__tdManifestRead=false;window.__tdManifestVal=null;caches.open('td-viewer').then(function(c){return c.match(__KEY__);}).then(function(r){return r?r.json():null;}).then(function(j){window.__tdManifestVal=j;window.__tdManifestRead=true;},function(){window.__tdManifestVal=null;window.__tdManifestRead=true;});return true;})()"#;

/// `caches.match` every old asset key in td-viewer and report which no longer
/// match. This is the survival proof — loading the asset would prove nothing
/// (the HTTP cache can serve an entry the Cache API deleted).
const ASSETS_MATCH_JS: &str = r#"(function(){window.__tdAssetsDone=false;window.__tdAssetsMissing=null;var ks=__KEYS__;caches.open('td-viewer').then(function(c){return Promise.all(ks.map(function(k){return c.match(k).then(function(r){return r?null:k;});}));}).then(function(m){window.__tdAssetsMissing=m.filter(function(x){return x!==null;});window.__tdAssetsDone=true;},function(){window.__tdAssetsMissing=ks.slice();window.__tdAssetsDone=true;});return true;})()"#;

/// The documented limit (module comment), appended to every offline case's
/// pass text so a report never overstates what the case proved.
const SW_LIMIT_NOTE: &str = "limit: proves the page target offline, not the SW's own fetch — a false PASS needs the override missed AND a live 5xx/timeout at that moment (a 200 makes the SW serve net and FAILs the case)";

fn safe_detail(raw: &str) -> String {
    let token = url::Url::parse(&format!("https://verify.invalid{raw}"))
        .ok()
        .and_then(|u| u.query_pairs().find(|(k, _)| k == "token").map(|(_, v)| v.to_string()));
    scrub(raw, token.as_deref().unwrap_or(""))
}

pub(crate) fn judge_first_visit(p: &OfflineProbe) -> Check {
    let mut bad = Vec::new();
    if !p.offline_evidence { bad.push(OFFLINE_EVIDENCE_MISSING.to_string()); }
    if p.view != "viewer" { bad.push(format!("view={}", p.view)); }
    if p.served != "cache" { bad.push(format!("offline simulation did not yield a cache page (served={})", p.served)); }
    if !p.banner { bad.push("offline banner hidden".into()); }
    if !p.broken_maps.is_empty() { bad.push(format!("broken maps: {}", safe_detail(&p.broken_maps.join(", ")))); }
    let failed: Vec<_> = p.vouchers.iter().filter(|(_, ok)| !ok).map(|(u, _)| safe_detail(u)).collect();
    if !failed.is_empty() { bad.push(format!("bad vouchers: {}", failed.join(", "))); }
    if bad.is_empty() {
        Check::pass(format!(
            "offline override applied before the reload AND proven offline-only: the never-cached /__td/offline-probe fetch rejected with a network TypeError (a 5xx or 4s-timeout fallback would still answer HTTP); cache page, visible banner, no broken maps, {} local voucher(s) return PDF; {SW_LIMIT_NOTE}",
            p.vouchers.len()
        ))
    } else {
        Check::fail(bad.join("; "), None)
    }
}

const NAV_TIMEOUT: Duration = Duration::from_secs(25);
const STATE_TIMEOUT: Duration = Duration::from_secs(35);

async fn navigate(p: &IsolatedPage, url: &str) -> Result<(), String> {
    p.page().goto(url, NAV_TIMEOUT).await.map_err(|e| e.to_string())
}

async fn wait_state(p: &IsolatedPage, state: &str) -> Result<(), String> {
    let js = format!("document.querySelector('#offline-status')?.getAttribute('data-state') === {}", serde_json::to_string(state).unwrap());
    p.page().wait_for_function(&js, STATE_TIMEOUT).await.map(|_| ()).map_err(|e| e.to_string())
}

async fn probe(p: &IsolatedPage) -> Result<OfflineProbe, String> {
    p.page().evaluate_json("Array.from(document.querySelectorAll('img[src^=\"/map/\"]')).forEach(function(i){i.loading='eager';});true")
        .await.map_err(|e| e.to_string())?;
    let _ = p.page().wait_for_function("Array.from(document.querySelectorAll('img[src^=\"/map/\"]')).every(function(i){return i.complete;})", Duration::from_secs(15)).await;
    p.page().evaluate_json(OFFLINE_PROBE_JS).await.map_err(|e| e.to_string())?;
    p.page().wait_for_function("window.__tdProbe !== null && window.__tdProbe !== undefined", Duration::from_secs(20))
        .await.map_err(|e| e.to_string())?;
    let value = p.page().evaluate_json("window.__tdProbe").await.map_err(|e| e.to_string())?;
    serde_json::from_value(value).map_err(|e| format!("offline probe decode: {e}"))
}

async fn dispose(browser: &BridgeBrowser, p: IsolatedPage) -> Result<(), String> {
    let id = p.context_id().to_string();
    match p.dispose().await {
        Ok(()) => Ok(()),
        Err(e) => browser.dispose_context(&e.context_id).await.map_err(|r| format!("dispose {id}: {e}; retry: {r}")),
    }
}

fn emit(tally: &mut Tally, plan: &str, key: &str, check: Check, tokens: &[&str]) {
    let mut detail = check.detail;
    for token in tokens { detail = scrub(&detail, token); }
    tally.emit(plan, LAYER, key, &Check { detail, ..check });
}

fn url_for(plan: &str, token: &str, lang: &str) -> String {
    let base = crate::share_token::share_url(plan, token);
    if lang == "en" { format!("{base}&lang=en") } else { base }
}

/// The share token a td-viewer/td-index cache key belongs to: HTML and voucher
/// keys carry it as a query param; a manifest key carries the whole page key
/// percent-encoded in its `k` param.
fn token_of_key(key: &str) -> Option<String> {
    let outer = url::Url::parse(key).ok()?;
    let target = if outer.path().starts_with("/__td/manifest") {
        let inner = outer.query_pairs().find(|(k, _)| k == "k")?.1.to_string();
        url::Url::parse(&inner).ok()?
    } else {
        outer
    };
    target.query_pairs().find(|(k, _)| k == "token").map(|(_, v)| v.to_string())
}

/// The kinds text of a purge check, derived from what was actually recorded:
/// map keys carry no token param, so "maps" belongs in the report only when
/// the recorded set really contains map keys (the late-403 context records
/// everything; the revoke case records only token-keyed entries).
fn purge_kinds(recorded: &[String]) -> &'static str {
    if recorded.iter().any(|k| k.contains("/map/")) {
        "HTML, manifest, index, vouchers, maps"
    } else {
        "HTML, manifest, index, vouchers"
    }
}

/// Run an in-page script that parks its eventual result in a window variable
/// (`flag_expr` turns true once the value is final) and read it back.
/// evaluate_json does not await promises, so every Cache-API read needs this
/// flag pattern.
async fn window_value(p: &IsolatedPage, script: &str, flag_expr: &str, read_expr: &str) -> Result<serde_json::Value, String> {
    p.page().evaluate_json(script).await.map_err(|e| e.to_string())?;
    p.page().wait_for_function(flag_expr, Duration::from_secs(10)).await.map_err(|e| e.to_string())?;
    p.page().evaluate_json(read_expr).await.map_err(|e| e.to_string())
}

/// List a Cache API cache's keys (as URL strings) from the page. evaluate_json
/// does not await promises, so the result lands in window.__tdKeys first.
async fn cache_keys(p: &IsolatedPage, cache: &str) -> Result<Vec<String>, String> {
    let js = format!(
        r#"(function(){{window.__tdKeys=null;caches.open({cache:?}).then(function(c){{return c.keys();}}).then(function(ks){{window.__tdKeys=ks.map(function(r){{return typeof r==='string'?r:r.url;}});}});return true;}})()"#
    );
    let v = window_value(p, &js, "Array.isArray(window.__tdKeys)", "window.__tdKeys").await?;
    serde_json::from_value(v).map_err(|e| format!("cache keys decode: {e}"))
}

/// Read one manifest body from td-viewer by its exact key (None when the
/// entry is gone). Round 3 fix 3 evidence.
async fn read_manifest(p: &IsolatedPage, key: &str) -> Result<Option<ManifestBody>, String> {
    let js = MANIFEST_READ_JS.replace("__KEY__", &format!("{key:?}"));
    let v = window_value(p, &js, "window.__tdManifestRead===true", "window.__tdManifestVal").await?;
    serde_json::from_value(v).map_err(|e| format!("manifest decode: {e}"))
}

/// `caches.match` every old asset key in td-viewer; return those with no hit.
/// Round 3 fix 1 evidence — the survival proof is the Cache API itself.
async fn missing_from_cache(p: &IsolatedPage, keys: &[String]) -> Result<Vec<String>, String> {
    if keys.is_empty() { return Ok(Vec::new()); }
    let js = ASSETS_MATCH_JS.replace("__KEYS__", &serde_json::to_string(keys).unwrap());
    let v = window_value(p, &js, "window.__tdAssetsDone===true", "window.__tdAssetsMissing").await?;
    serde_json::from_value(v).map_err(|e| format!("assets match decode: {e}"))
}

/// Poll both caches until none of `targets` remains (15 s deadline — the SW
/// purges in waitUntil, so the delete can trail the 403); the returned
/// snapshot is what the purge judge compares against.
async fn wait_keys_gone(p: &IsolatedPage, targets: &[String]) -> Result<Vec<String>, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let mut now = cache_keys(p, "td-viewer").await?;
        now.extend(cache_keys(p, "td-index").await?);
        if targets.iter().all(|k| !now.contains(k)) || tokio::time::Instant::now() >= deadline {
            return Ok(now);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// A local asset the next SAVE is GUARANTEED to re-fetch while the old copy is
/// already cached: an unversioned /voucher/ link (no matching sha → no ?v=)
/// or, failing that, an unversioned /map/ image (empty etag → no ?v=).
/// Faulting its URL makes the next SAVE partial without the harness touching
/// the cache (Round 2 finding 1). None when the plan's data cannot produce
/// the condition.
const FAULT_TARGET_JS: &str = r#"(function(){
  function clean(s){return String(s||'').replace(/\s+/g,' ').replace(/^ | $/g,'');}
  var vs=Array.from(document.querySelectorAll('a.doc-link,a.voucher-link')).filter(function(a){var h=a.getAttribute('href')||'';return h.indexOf('/voucher/')===0&&!/[?&]v=/.test(h);});
  if(vs.length){var lb=clean(vs[0].textContent);var n=Array.from(document.querySelectorAll('a.doc-link,a.voucher-link')).filter(function(a){return clean(a.textContent)===lb;}).length;return {kind:'voucher',url:vs[0].getAttribute('href'),label:lb,labelCount:n};}
  var ms=Array.from(document.querySelectorAll('img[src^="/map/"]')).filter(function(i){return !/[?&]v=/.test(i.getAttribute('src')||'');});
  if(ms.length){var lb2=clean(ms[0].alt)||ms[0].getAttribute('src');var n2=Array.from(document.querySelectorAll('img[src^="/map/"]')).filter(function(i){return (clean(i.alt)||i.getAttribute('src'))===lb2;}).length;return {kind:'map',url:ms[0].getAttribute('src'),label:lb2,labelCount:n2};}
  return null;
})()"#;

struct FaultTarget {
    label: String,
    needle: String,
    /// How many elements on the page share the faulted asset's display label
    /// — the failure list carries labels only, so >1 makes that match
    /// ambiguous (Round 3 finding 4).
    label_count: usize,
}

async fn pick_fault_target(p: &IsolatedPage) -> Result<Option<FaultTarget>, String> {
    let v = p.page().evaluate_json(FAULT_TARGET_JS).await.map_err(|e| e.to_string())?;
    let obj = match v {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::Object(m) => m,
        _ => return Err("fault target probe returned a non-object".into()),
    };
    let url = obj.get("url").and_then(|x| x.as_str()).ok_or("fault target probe missing url")?.to_string();
    let needle = url.split('?').next().unwrap_or(&url).to_string();
    Ok(Some(FaultTarget {
        label: obj.get("label").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        needle,
        label_count: obj.get("labelCount").and_then(|x| x.as_u64()).ok_or("fault target probe missing labelCount")? as usize,
    }))
}

/// Layer 5 adds cases only on --render. All token mutations belong to this
/// verify-site run; token2/3 are removed on every path below.
pub(crate) async fn run(conn: &Connection, plan: &str, _host: &str, token: &str,
                        documents: &[PlanDocument], tally: &mut Tally) {
    let endpoint = bridge_core::runtime::cdp_endpoint(&bridge_core::sys::SysEnv);
    let browser = match bridge_core::cdp::connect(&endpoint).await {
        Ok(b) => b,
        Err(e) => { emit(tally, plan, "cdp", Check::fail(format!("offline Chrome connection: {e}"), None), &[token]); return; }
    };
    let before = match browser.browser_context_count().await {
        Ok(n) => n,
        Err(e) => { emit(tally, plan, "cleanup", Check::fail(format!("context count before: {e}"), None), &[token]); return; }
    };
    let token2 = match crate::share_token::mint_for_plan(conn, plan, verify_site::VERIFY_CREATED_BY).await {
        Ok(t) => t,
        Err(e) => { emit(tally, plan, "multi-token", Check::fail(format!("mint token2: {e}"), None), &[token]); return; }
    };
    let mut token3 = None;
    let tokens = [token, token2.as_str()];
    let sequence = async {
        let p = browser.new_isolated_page().await.map_err(|e| e.to_string())?;
        let case = async {
            let first_url = url_for(plan, token, "zh");
            navigate(&p, &first_url).await?;
            wait_state(&p, "saved").await?;
            // Round 3 fixes 1+3 — evidence recorded BEFORE the fault, from
            // the Cache API while the fresh copy is the only one: every
            // cached asset key, and the manifest of the single page saved in
            // this context so far (this token's zh page).
            let viewer_keys = cache_keys(&p, "td-viewer").await?;
            let old_assets: Vec<String> = viewer_keys.iter()
                .filter(|k| k.contains("/map/") || k.contains("/voucher/"))
                .cloned().collect();
            let manifest_keys: Vec<String> = viewer_keys.iter()
                .filter(|k| k.contains("/__td/manifest")).cloned().collect();
            let (manifest_key, manifest_before, manifest_note) = match manifest_keys.len() {
                1 => {
                    let k = manifest_keys[0].clone();
                    match read_manifest(&p, &k).await? {
                        Some(body) => (k, Some(body), String::new()),
                        // Listed by cache.keys() but caches.match cannot read
                        // it back — fail loud, do not pretend it never existed.
                        None => (String::new(), None, "the /__td/manifest entry is listed by cache.keys() but caches.match cannot read it back".to_string()),
                    }
                }
                n => (String::new(), None, format!(
                    "expected exactly one /__td/manifest entry in td-viewer after the first SAVE (only this token's zh page is saved in this context), found {n}"
                )),
            };
            p.set_offline(true).await.map_err(|e| e.to_string())?;
            navigate(&p, &first_url).await?;
            let first_probe = probe(&p).await?;
            emit(tally, plan, "first-visit", judge_first_visit(&first_probe), &tokens);
            let absent: Vec<&str> = documents.iter().filter(|d| !first_probe.vouchers.iter().any(|(href, _)| href.contains(&d.r2_key))).map(|d| d.doc_key.as_str()).collect();
            emit(tally, plan, "document-links", if absent.is_empty() {
                Check::pass(format!("all {} plan document(s) appeared among the local PDF links checked offline", documents.len()))
            } else { Check::fail(format!("plan documents absent from offline PDF links: {}", absent.join(", ")), None) }, &tokens);
            // The old copy's identity for the partial-keep comparison below.
            let saved_rendered_at = first_probe.rendered_at.clone();
            p.set_offline(false).await.map_err(|e| e.to_string())?;

            // partial-keep (1b): fault an asset the next SAVE really
            // refetches while the old copy is already cached — the harness
            // never deletes or restores anything itself.
            match pick_fault_target(&p).await? {
                Some(t) => {
                    p.fail_sw_requests(&t.needle, 500).await.map_err(|e| e.to_string())?;
                    navigate(&p, &first_url).await?;
                    wait_state(&p, "kept").await?;
                    let kept_online = probe(&p).await?;
                    // Round 3 fix 3: the old manifest must be untouched
                    // (complete:true, same asset list) — read back from the
                    // Cache API right after the faulted SAVE, before the
                    // offline reload.
                    if manifest_key.is_empty() {
                        emit(tally, plan, "partial-keep-manifest", Check::fail(manifest_note, None), &tokens);
                    } else {
                        let manifest_after = read_manifest(&p, &manifest_key).await?;
                        emit(tally, plan, "partial-keep-manifest",
                             judge_manifest_unchanged(manifest_before.as_ref(), manifest_after.as_ref()), &tokens);
                    }
                    // Round 3 fix 1: every old asset must still match its
                    // exact td-viewer key via caches.match — loading it would
                    // prove nothing (the HTTP cache, max-age=86400 on /map/,
                    // can serve an entry the Cache API deleted).
                    let missing = missing_from_cache(&p, &old_assets).await?;
                    emit(tally, plan, "partial-keep-assets", judge_old_assets_survive(&old_assets, &missing), &tokens);
                    p.clear_overrides().await.map_err(|e| e.to_string())?;
                    p.set_offline(true).await.map_err(|e| e.to_string())?;
                    navigate(&p, &first_url).await?;
                    let kept_offline = probe(&p).await?;
                    emit(tally, plan, "partial-keep", judge_kept_after_partial(&kept_offline, &kept_online, &t.label, t.label_count, &saved_rendered_at), &tokens);
                    p.set_offline(false).await.map_err(|e| e.to_string())?;
                }
                None => emit(tally, plan, "partial-keep", Check::warn(
                    "cannot build the partial-keep condition on this plan's data: every local /voucher/ and /map/ URL carries a version key (?v=), so the next SAVE skips them all and no fault can make it partial; the harness must not create the condition by editing the cache",
                    None), &tokens),
            }

            let first_en = url_for(plan, token, "en");
            navigate(&p, &first_en).await?; wait_state(&p, "saved").await?;
            let second_zh = url_for(plan, &token2, "zh");
            let second_en = url_for(plan, &token2, "en");
            for (key, u) in [("multi-token-zh", &second_zh), ("multi-token-en", &second_en)] {
                navigate(&p, u).await?; wait_state(&p, "saved").await?;
                emit(tally, plan, key, Check::pass("second token saved in this context; both languages reached saved state"), &tokens);
            }

            // revoke: record token2's cache keys BEFORE the 403, re-check
            // them after it (finding 3).
            let viewer_keys = cache_keys(&p, "td-viewer").await?;
            let mut recorded = viewer_keys.clone();
            recorded.extend(cache_keys(&p, "td-index").await?);
            let t2_keys: Vec<String> = recorded.into_iter().filter(|k| token_of_key(k).as_deref() == Some(token2.as_str())).collect();
            let shared_map = viewer_keys.into_iter().find(|k| k.contains("/map/"));
            if t2_keys.is_empty() {
                emit(tally, plan, "revoke-purge", Check::fail("no td-viewer/td-index entries were recorded for token2 — its SAVEs did not store what this case expects", None), &tokens);
            }
            let removed = verify_site::remove_verify_tokens(conn, Some(&token2)).await?;
            if removed != 1 { return Err(format!("revoke token2 removed {removed} rows, expected 1")); }
            navigate(&p, &second_zh).await?;
            p.page().evaluate_json(&format!("fetch({:?}).then(function(r){{window.__tdHttpStatus=r.status;}})", second_zh)).await.map_err(|e| e.to_string())?;
            p.page().wait_for_function("window.__tdHttpStatus !== undefined", Duration::from_secs(15)).await.map_err(|e| e.to_string())?;
            let status = p.page().evaluate_json("window.__tdHttpStatus").await.map_err(|e| e.to_string())?;
            emit(tally, plan, "revoke-403", if status.as_i64() == Some(403) {
                Check::pass("revoked token returned HTTP 403 online; the SW runs its cache purge before delivering that response")
            } else { Check::fail(format!("revoked token returned HTTP {status}, expected 403"), None) }, &tokens);
            let now_keys = wait_keys_gone(&p, &t2_keys).await?;
            if !t2_keys.is_empty() {
                emit(tally, plan, "revoke-purge", judge_token_purge(&t2_keys, &now_keys, shared_map.as_deref(), "HTML, manifest, index, vouchers"), &tokens);
            }
            p.set_offline(true).await.map_err(|e| e.to_string())?;
            navigate(&p, &second_zh).await?;
            emit(tally, plan, "revoke-miss", judge_miss(&probe(&p).await?), &tokens);
            for (key, u) in [("survivor-zh", &first_url), ("survivor-en", &first_en)] {
                navigate(&p, u).await?;
                emit(tally, plan, key, judge_first_visit(&probe(&p).await?), &tokens);
            }
            Ok::<(), String>(())
        }.await;
        let cleanup = dispose(&browser, p).await;
        match (case, cleanup) { (Ok(()), Ok(())) => Ok(()), (Err(e), Ok(())) | (Ok(()), Err(e)) => Err(e), (Err(e), Err(c)) => Err(format!("{e}; cleanup: {c}")) }
    }.await;
    if let Err(e) = sequence { emit(tally, plan, "sequence", Check::fail(e, None), &tokens); }

    match crate::share_token::mint_for_plan(conn, plan, verify_site::VERIFY_CREATED_BY).await {
        Ok(t) => token3 = Some(t),
        Err(e) => emit(tally, plan, "late-403", Check::fail(format!("mint token3: {e}"), None), &tokens),
    }
    if let Some(ref t3) = token3 {
        let u = url_for(plan, t3, "zh");
        let result = async {
            let p = browser.new_isolated_page().await.map_err(|e| e.to_string())?;
            let case = async {
                navigate(&p, &u).await?; wait_state(&p, "saved").await?;
                // Round 3 fix 2: this context holds ONLY token3's copies, so
                // EVERY td-viewer/td-index key is token3's — maps included
                // (map keys carry no token param, so the token_of_key filter
                // the revoke case uses would silently skip them; here no
                // other manifest references them, so the purge must take the
                // maps too). Guard: every non-asset key must reference t3.
                let mut recorded = cache_keys(&p, "td-viewer").await?;
                recorded.extend(cache_keys(&p, "td-index").await?);
                for k in &recorded {
                    if !(k.contains("/map/") || k.contains("/voucher/")) && token_of_key(k).as_deref() != Some(t3.as_str()) {
                        return Err(format!("unexpected non-token3 cache key in token3's isolated context: {k}"));
                    }
                }
                let t3_keys = recorded;
                if t3_keys.is_empty() { return Err("no td-viewer/td-index entries were recorded for token3".into()); }
                let removed = verify_site::remove_verify_tokens(conn, Some(t3)).await?;
                if removed != 1 { return Err(format!("revoke token3 removed {removed} rows, expected 1")); }
                p.set_latency_ms(6000).await.map_err(|e| e.to_string())?;
                navigate(&p, &u).await?;
                let early = p.page().evaluate_json("document.body.getAttribute('data-served')").await.map_err(|e| e.to_string())?;
                if early.as_str() != Some("cache") { return Err(format!("late-403 timeout did not serve cache: {early}")); }
                emit(tally, plan, "late-403-cache", Check::pass("6000ms latency applied before the reload; the 4000ms timeout served the cached page while the 403 was still pending (offline-only proof for this same page is asserted in late-403)"), &[token, &token2, t3]);
                tokio::time::sleep(Duration::from_secs(7)).await;
                let now_keys = wait_keys_gone(&p, &t3_keys).await?;
                emit(tally, plan, "late-403-purge", judge_token_purge(&t3_keys, &now_keys, None, purge_kinds(&t3_keys)), &[token, &token2, t3]);
                p.set_offline(true).await.map_err(|e| e.to_string())?;
                navigate(&p, &u).await?;
                emit(tally, plan, "late-403", judge_miss(&probe(&p).await?), &[token, &token2, t3]);
                Ok::<(), String>(())
            }.await;
            let cleanup = dispose(&browser, p).await;
            match (case, cleanup) { (Ok(()), Ok(())) => Ok(()), (Err(e), Ok(())) | (Ok(()), Err(e)) => Err(e), (Err(e), Err(c)) => Err(format!("{e}; cleanup: {c}")) }
        }.await;
        if let Err(e) = result { emit(tally, plan, "late-403", Check::fail(e, None), &[token, &token2, t3]); }
    }

    // A day card and a timezone are needed for the clock transition.
    let stale = async {
        let dates = travel_db::repo::days::main_day_dates(conn, plan).await?;
        let day = dates.first().ok_or("plan has no main day for stale-day case")?.clone();
        let prev = chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d").map_err(|e| e.to_string())?
            .pred_opt().ok_or("invalid previous date")?.to_string();
        let mut rows = conn.query("SELECT dc.timezone FROM days d JOIN destination_config dc ON dc.slug = d.destination WHERE d.plan_id = ?1 AND d.date = ?2 AND d.variant_of IS NULL LIMIT 1", libsql::params![plan, day.as_str()]).await.map_err(|e| e.to_string())?;
        let row = rows.next().await.map_err(|e| e.to_string())?.ok_or("plan destination timezone missing")?;
        let tz: String = row.get(0).map_err(|e| e.to_string())?;
        let p = browser.new_isolated_page().await.map_err(|e| e.to_string())?;
        let case = async {
            let u = url_for(plan, token, "zh");
            let yesterday = local_noon_ms(&p, &prev, &tz).await?;
            p.set_page_clock_ms(yesterday).await.map_err(|e| e.to_string())?;
            navigate(&p, &u).await?; wait_state(&p, "saved").await?;
            p.set_offline(true).await.map_err(|e| e.to_string())?;
            let today = local_noon_ms(&p, &day, &tz).await?;
            p.set_page_clock_ms(today).await.map_err(|e| e.to_string())?;
            navigate(&p, &u).await?;
            emit(tally, plan, "stale-day", judge_stale_day(&probe(&p).await?), &[token, &token2]);
            Ok::<(), String>(())
        }.await;
        let cleanup = dispose(&browser, p).await;
        match (case, cleanup) { (Ok(()), Ok(())) => Ok(()), (Err(e), Ok(())) | (Ok(()), Err(e)) => Err(e), (Err(e), Err(c)) => Err(format!("{e}; cleanup: {c}")) }
    }.await;
    if let Err(e) = stale { emit(tally, plan, "stale-day", Check::fail(e, None), &[token, &token2]); }

    for t in [&token2].into_iter().chain(token3.as_ref()) {
        if let Err(e) = verify_site::remove_verify_tokens(conn, Some(t)).await {
            emit(tally, plan, "token-cleanup", Check::fail(format!("verify token cleanup: {e}"), None), &[token, &token2, t]);
        }
    }
    match browser.browser_context_count().await {
        Ok(after) if after == before => emit(tally, plan, "cleanup", Check::pass(format!("browser contexts restored to {before}")), &[token, &token2]),
        Ok(after) => emit(tally, plan, "cleanup", Check::fail(format!("context count changed: before={before}, after={after}"), None), &[token, &token2]),
        Err(e) => emit(tally, plan, "cleanup", Check::fail(format!("context count after: {e}"), None), &[token, &token2]),
    }
}

async fn local_noon_ms(p: &IsolatedPage, date: &str, tz: &str) -> Result<i64, String> {
    let js = format!(r#"(function(){{var d={date:?},tz={tz:?},a=d.split('-').map(Number),u=Date.UTC(a[0],a[1]-1,a[2],12),f=new Intl.DateTimeFormat('en-US',{{timeZone:tz,year:'numeric',month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit',hourCycle:'h23'}}),v={{}};f.formatToParts(new Date(u)).forEach(function(p){{v[p.type]=Number(p.value)}});var local=Date.UTC(v.year,v.month-1,v.day,v.hour,v.minute);return u-(local-u);}})()"#);
    p.page().evaluate_json(&js).await.map_err(|e| e.to_string())?.as_i64().ok_or("local noon clock calculation failed".into())
}

/// 1b: `kept_online` is the ONLINE page right after the faulted SAVE (it
/// carries `state=kept` and the populated failure list); `p` is the same URL
/// reloaded OFFLINE afterwards. Proves: the kept reply named the faulted
/// asset (by display label — the only thing the failure list carries; when
/// `label_count` > 1 the note says the match is ambiguous), the offline copy
/// is still the pre-fault HTML (same data-rendered-at) and every old voucher
/// still opens offline — with the harness never having edited the cache.
/// (Manifest/asset survival is proven separately by the Cache-API checks.)
pub(crate) fn judge_kept_after_partial(p: &OfflineProbe, kept_online: &OfflineProbe, fault_label: &str, label_count: usize, saved_rendered_at: &str) -> Check {
    let mut bad = Vec::new();
    if kept_online.state != "kept" {
        bad.push(format!("partial SAVE state={}, expected kept", kept_online.state));
    }
    if kept_online.failed.is_empty() {
        bad.push("kept reply reported an empty failure list — the fault never reached the SAVE".into());
    } else if !fault_label.is_empty() && !kept_online.failed.iter().any(|f| f.contains(fault_label)) {
        bad.push(format!("failure list did not name the faulted asset ({}): {}", safe_detail(fault_label), kept_online.failed.join(" | ")));
    }
    if saved_rendered_at.is_empty() || p.rendered_at != saved_rendered_at {
        bad.push(format!(
            "the offline copy's rendered-at {} is not the pre-fault copy's {} — the old HTML did not survive the partial SAVE",
            p.rendered_at, saved_rendered_at
        ));
    }
    let base = judge_first_visit(p);
    if base.level == Level::Fail {
        bad.push(base.detail);
    }
    let label_note = if label_count > 1 {
        format!("; note: the faulted asset's display label is not unique on the page ({} elements share it) — the failure-list match is by label only, so another same-label asset could satisfy it", label_count)
    } else {
        String::new()
    };
    if bad.is_empty() {
        Check::pass(format!(
            "the fault hit an asset every SAVE refetches while the old copy was already cached (harness never edited the cache); the kept reply named it in the failure list, the offline copy still carries the pre-fault rendered-at {}, and {} local voucher(s) still open offline{label_note}; {SW_LIMIT_NOTE}",
            saved_rendered_at, p.vouchers.len()
        ))
    } else {
        Check::fail(format!("{}{}", bad.join("; "), label_note), None)
    }
}

pub(crate) fn judge_miss(p: &OfflineProbe) -> Check {
    let mut bad = Vec::new();
    if !p.miss_page { bad.push("expected the SW offline-miss page".into()); }
    if !p.vouchers.is_empty() { bad.push(format!("{} local voucher link(s) still open", p.vouchers.len())); }
    if !p.view.is_empty() { bad.push(format!("view={}", p.view)); }
    if !p.offline_evidence { bad.push(OFFLINE_EVIDENCE_MISSING.to_string()); }
    if bad.is_empty() {
        Check::pass(format!("offline-miss page with no voucher links and offline-only network evidence (the never-cached probe fetch rejected with a TypeError); the Cache-API purge of this token is asserted separately by the purge check of this run; {SW_LIMIT_NOTE}"))
    } else {
        Check::fail(bad.join("; "), None)
    }
}

/// A miss page with no links does not prove the SW deleted the revoked
/// token's cached copies — an orphaned entry would be invisible there. The
/// keys recorded before the revocation are re-checked against the live Cache
/// API after the 403; `shared_map` additionally asserts the surviving
/// token's map (same key, no token param) was not swept. `kinds` names what
/// the recorded set contains ("HTML, manifest, index, vouchers[, maps]") —
/// the late-403 context records every key, maps included.
pub(crate) fn judge_token_purge(recorded: &[String], now: &[String], shared_map: Option<&str>, kinds: &str) -> Check {
    let left: Vec<&str> = recorded.iter().filter(|k| now.iter().any(|n| n == *k)).map(|k| k.as_str()).collect();
    let mut bad = Vec::new();
    if !left.is_empty() {
        bad.push(format!("{} of {} recorded cache entries still present: {}", left.len(), recorded.len(), safe_detail(&left.join(", "))));
    }
    let shared_half = match shared_map {
        Some(m) => {
            if now.iter().any(|n| n == m) {
                format!("the map shared with the surviving token is still cached ({})", safe_detail(m))
            } else {
                bad.push(format!("the shared map was swept together with the revoked token: {}", safe_detail(m)));
                String::new()
            }
        }
        None => "no second token shares this browser context, so the shared-asset half is vacuous".to_string(),
    };
    if bad.is_empty() {
        Check::pass(format!(
            "after the online 403 all {} cache entries the revoked token owned ({}) were deleted from the Cache API; {}",
            recorded.len(), kinds, shared_half
        ))
    } else {
        Check::fail(bad.join("; "), None)
    }
}

/// Round 3 fix 3: the old manifest must still be the pre-fault one —
/// `complete:true` and the same asset list. A rewritten manifest behind an
/// unchanged HTML copy would pass the rendered-at check alone.
pub(crate) fn judge_manifest_unchanged(before: Option<&ManifestBody>, after: Option<&ManifestBody>) -> Check {
    let Some(b) = before else {
        return Check::fail("no /__td/manifest entry was found in td-viewer after the first SAVE — the kept-copy premise (a complete copy existed) cannot be verified", None);
    };
    let Some(a) = after else {
        return Check::fail("the old manifest entry is gone from td-viewer after the partial SAVE", None);
    };
    let mut bad = Vec::new();
    if !b.complete {
        bad.push(format!("the pre-fault manifest was not complete (complete={}) — the first SAVE never produced the complete copy this case needs", b.complete));
    } else if !a.complete {
        bad.push("complete flag lost: before=true, after=false (a kept complete copy must stay complete:true)".into());
    }
    if a.assets != b.assets {
        let missing: Vec<String> = b.assets.iter().filter(|x| !a.assets.contains(x)).map(|s| safe_detail(s)).collect();
        let added: Vec<String> = a.assets.iter().filter(|x| !b.assets.contains(x)).map(|s| safe_detail(s)).collect();
        bad.push(format!("asset list changed: missing [{}], added [{}]", missing.join(", "), added.join(", ")));
    }
    if a.rendered_at != b.rendered_at {
        bad.push(format!("renderedAt changed: {} -> {}", b.rendered_at, a.rendered_at));
    }
    if bad.is_empty() {
        Check::pass(format!("the manifest entry is still the pre-fault one: complete:true, {} asset(s) unchanged, renderedAt {}", b.assets.len(), b.rendered_at))
    } else {
        Check::fail(bad.join("; "), None)
    }
}

/// Round 3 fix 1: `missing` lists old-asset keys with no `caches.match` hit
/// in td-viewer. Cache-API proof — loading the asset would prove nothing (the
/// HTTP cache, `max-age=86400` on /map/, can serve a deleted entry).
pub(crate) fn judge_old_assets_survive(old_assets: &[String], missing: &[String]) -> Check {
    if missing.is_empty() {
        if old_assets.is_empty() {
            Check::pass("vacuous: this plan cached no /map/ or /voucher/ assets in the first SAVE")
        } else {
            Check::pass(format!(
                "every one of the {} old asset(s) still matches its exact key in td-viewer (caches.match) — Cache-API proof, immune to the HTTP cache that could serve a deleted image on reload",
                old_assets.len()
            ))
        }
    } else {
        Check::fail(format!(
            "{} of {} old asset(s) no longer match their exact td-viewer key (caches.match): {}",
            missing.len(), old_assets.len(), safe_detail(&missing.join(", "))
        ), None)
    }
}

fn today_card_has(html: &str, selector: &str) -> bool {
    let doc = scraper::Html::parse_fragment(html);
    scraper::Selector::parse(selector).ok().map(|s| doc.select(&s).next().is_some()).unwrap_or(false)
}

/// Today's day card must have its OWN `details[data-block]` open — a direct
/// child. The Round-1 selector matched any descendant, so a nested open block
/// satisfied it while the card itself stayed closed.
fn today_card_open(html: &str) -> bool {
    today_card_has(html, "section[data-date] > details[data-block][open]")
}

fn today_card_marked(html: &str) -> bool {
    today_card_has(html, "section[data-date] > details[data-block][data-today]")
}

pub(crate) fn judge_stale_day(p: &OfflineProbe) -> Check {
    let base = judge_first_visit(p);
    if base.level == Level::Fail { return base; }
    let mut bad = Vec::new();
    if p.today_html.is_empty() {
        bad.push("today's day card (section[data-date]) missing".into());
    } else {
        if !today_card_open(&p.today_html) {
            bad.push("today's day card has no OPEN direct-child details[data-block] (a nested open block does not count)".into());
        }
        if !today_card_marked(&p.today_html) {
            bad.push("today marker missing on the today day card's own details[data-block]".into());
        }
    }
    if p.today_count != 1 {
        bad.push(format!("expected exactly one [data-today] on the page, found {}", p.today_count));
    }
    if !p.now_hidden {
        bad.push("stale now card not hidden".into());
    }
    if bad.is_empty() {
        Check::pass(format!("offline override applied before the reload and proven offline-only (network TypeError on a never-cached fetch); today's own day block (direct child) is open and marked, exactly one [data-today] on the page, stale now card hidden; {SW_LIMIT_NOTE}"))
    } else {
        Check::fail(bad.join("; "), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() -> OfflineProbe {
        OfflineProbe {
            view: "viewer".into(),
            served: "cache".into(),
            banner: true,
            offline_evidence: true,
            ..Default::default()
        }
    }

    /// A real day-card fragment: the open, marked `details[data-block]` is the
    /// section's OWN direct child.
    const DIRECT_CHILD_TODAY: &str = r#"<section class="day" id="day-2" data-day="2" data-date="2026-11-14"><details data-block="day-2" data-today open><summary>day 2</summary><p>body</p></details></section>"#;

    /// The only open details is NESTED inside the (closed) day block: the old
    /// descendant selector satisfied itself on this and passed while the day
    /// card itself stayed closed.
    const NESTED_OPEN_TODAY: &str = r#"<section class="day" id="day-2" data-day="2" data-date="2026-11-14"><details data-block="day-2"><summary>day 2</summary><details data-block="act-1" open><summary>inner</summary></details></details></section>"#;

    #[test]
    fn verify_site_offline_judge_first_visit_passes_clean_probe() {
        assert_eq!(judge_first_visit(&clean()).level, Level::Pass);
    }

    #[test]
    fn verify_site_offline_judge_first_visit_fails_on_net_served() {
        let mut p = clean(); p.served = "net".into();
        let c = judge_first_visit(&p);
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("offline simulation"));
    }

    #[test]
    fn verify_site_offline_judge_first_visit_fails_without_offline_evidence() {
        let mut p = clean(); p.offline_evidence = false;
        let c = judge_first_visit(&p);
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("offline-only evidence"));
    }

    #[test]
    fn verify_site_offline_judge_first_visit_lists_broken_maps_and_bad_vouchers() {
        let mut p = clean();
        p.broken_maps.push("/map/a.png".into());
        p.vouchers.push(("/voucher/a.pdf".into(), false));
        let c = judge_first_visit(&p);
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("/map/a.png") && c.detail.contains("/voucher/a.pdf"));
    }

    #[test]
    fn verify_site_offline_judge_kept_requires_kept_state() {
        let mut online = clean();
        online.state = "kept".into();
        online.failed = vec!["voucher x".into()];
        let mut off = clean(); off.rendered_at = "111".into();
        assert_eq!(judge_kept_after_partial(&off, &online, "voucher x", 1, "111").level, Level::Pass);
        online.state = "saved".into();
        assert_eq!(judge_kept_after_partial(&off, &online, "voucher x", 1, "111").level, Level::Fail);
    }

    #[test]
    fn verify_site_offline_judge_kept_names_the_faulted_asset_in_the_failure_list() {
        let mut online = clean();
        online.state = "kept".into();
        online.failed = vec!["something else".into()];
        let mut off = clean(); off.rendered_at = "111".into();
        let c = judge_kept_after_partial(&off, &online, "hotel voucher", 1, "111");
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("failure list"));
        online.failed = vec!["something else".into(), "hotel voucher".into()];
        assert_eq!(judge_kept_after_partial(&off, &online, "hotel voucher", 1, "111").level, Level::Pass);
        online.failed = vec![];
        let c = judge_kept_after_partial(&off, &online, "hotel voucher", 1, "111");
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("empty failure list"));
    }

    #[test]
    fn verify_site_offline_judge_kept_requires_unchanged_rendered_at() {
        let mut online = clean();
        online.state = "kept".into();
        online.failed = vec!["hotel voucher".into()];
        let mut off = clean(); off.rendered_at = "222".into();
        let c = judge_kept_after_partial(&off, &online, "hotel voucher", 1, "111");
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("rendered-at"));
    }

    fn manifest(complete: bool, rendered_at: i64, assets: &[&str]) -> ManifestBody {
        ManifestBody { complete, rendered_at, assets: assets.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn verify_site_offline_manifest_body_decodes_the_sw_shape() {
        let m: ManifestBody = serde_json::from_str(
            r#"{"assets":["https://h/map/a.png?v=1"],"complete":true,"renderedAt":1731900000000}"#,
        ).unwrap();
        assert!(m.complete);
        assert_eq!(m.rendered_at, 1731900000000);
        assert_eq!(m.assets, vec!["https://h/map/a.png?v=1".to_string()]);
    }

    #[test]
    fn verify_site_offline_judge_manifest_unchanged_passes_identical() {
        let m = manifest(true, 111, &["https://h/map/a.png?v=1", "https://h/voucher/b.pdf?token=t"]);
        let c = judge_manifest_unchanged(Some(&m), Some(&m));
        assert_eq!(c.level, Level::Pass);
        assert!(c.detail.contains("2 asset(s)"), "{}", c.detail);
        assert!(c.detail.contains("111"), "{}", c.detail);
    }

    #[test]
    fn verify_site_offline_judge_manifest_unchanged_fails_on_complete_flip() {
        let b = manifest(true, 111, &["https://h/map/a.png?v=1"]);
        let a = manifest(false, 111, &["https://h/map/a.png?v=1"]);
        let c = judge_manifest_unchanged(Some(&b), Some(&a));
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("complete"), "{}", c.detail);
    }

    #[test]
    fn verify_site_offline_judge_manifest_unchanged_fails_on_asset_list_change() {
        let b = manifest(true, 111, &["https://h/map/a.png?v=1", "https://h/voucher/b.pdf?token=t"]);
        let a = manifest(true, 111, &["https://h/map/a.png?v=1", "https://h/voucher/c.pdf?token=t"]);
        let c = judge_manifest_unchanged(Some(&b), Some(&a));
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("b.pdf") && c.detail.contains("c.pdf"), "{}", c.detail);
    }

    #[test]
    fn verify_site_offline_judge_manifest_unchanged_fails_on_rendered_at_change() {
        let b = manifest(true, 111, &["https://h/map/a.png?v=1"]);
        let a = manifest(true, 222, &["https://h/map/a.png?v=1"]);
        let c = judge_manifest_unchanged(Some(&b), Some(&a));
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("renderedAt"), "{}", c.detail);
    }

    #[test]
    fn verify_site_offline_judge_manifest_unchanged_fails_when_the_entry_is_gone() {
        let b = manifest(true, 111, &["https://h/map/a.png?v=1"]);
        let gone = judge_manifest_unchanged(Some(&b), None);
        assert_eq!(gone.level, Level::Fail);
        assert!(gone.detail.contains("gone"), "{}", gone.detail);
        let none_before = judge_manifest_unchanged(None, None);
        assert_eq!(none_before.level, Level::Fail);
        assert!(none_before.detail.contains("first SAVE"), "{}", none_before.detail);
    }

    #[test]
    fn verify_site_offline_judge_old_assets_survive_passes_when_every_key_matches() {
        let old = vec!["https://h/map/a.png?v=1".to_string(), "https://h/voucher/b.pdf?token=t".to_string()];
        let c = judge_old_assets_survive(&old, &[]);
        assert_eq!(c.level, Level::Pass);
        assert!(c.detail.contains("2") && c.detail.contains("caches.match"), "{}", c.detail);
        let vacuous = judge_old_assets_survive(&[], &[]);
        assert_eq!(vacuous.level, Level::Pass);
        assert!(vacuous.detail.contains("vacuous"), "{}", vacuous.detail);
    }

    #[test]
    fn verify_site_offline_judge_old_assets_survive_fails_listing_missing_keys() {
        let old = vec!["https://h/map/a.png?v=1".to_string(), "https://h/voucher/b.pdf?token=t".to_string()];
        let missing = vec!["https://h/voucher/b.pdf?token=t".to_string()];
        let c = judge_old_assets_survive(&old, &missing);
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("b.pdf"), "{}", c.detail);
    }

    #[test]
    fn verify_site_offline_judge_token_purge_covers_orphaned_maps() {
        let recorded = vec![
            "https://h/?plan=p&lang=zh&token=t3".to_string(),
            "https://h/__td/manifest?k=x".to_string(),
            "https://h/map/m.png?v=3".to_string(),
        ];
        // A map the sweep should have deleted fails the check.
        let now = vec!["https://h/map/m.png?v=3".to_string()];
        let c = judge_token_purge(&recorded, &now, None, purge_kinds(&recorded));
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("map/m.png"), "{}", c.detail);
        // All gone → PASS, and the report names maps among what was checked.
        let c = judge_token_purge(&recorded, &[], None, purge_kinds(&recorded));
        assert_eq!(c.level, Level::Pass);
        assert!(c.detail.contains("maps"), "{}", c.detail);
    }

    #[test]
    fn verify_site_offline_purge_kinds_name_maps_only_when_recorded() {
        let with_maps = vec!["https://h/map/m.png?v=3".to_string()];
        let without = vec!["https://h/?plan=p&lang=zh&token=t3".to_string()];
        assert!(purge_kinds(&with_maps).contains("maps"));
        assert!(!purge_kinds(&without).contains("maps"));
    }

    #[test]
    fn verify_site_offline_judge_kept_notes_a_non_unique_fault_label() {
        let mut online = clean();
        online.state = "kept".into();
        online.failed = vec!["住宿券 PDF".into()];
        let mut off = clean(); off.rendered_at = "111".into();
        let unique = judge_kept_after_partial(&off, &online, "住宿券 PDF", 1, "111");
        assert_eq!(unique.level, Level::Pass);
        assert!(!unique.detail.contains("not unique"), "{}", unique.detail);
        let ambiguous = judge_kept_after_partial(&off, &online, "住宿券 PDF", 3, "111");
        assert_eq!(ambiguous.level, Level::Pass);
        assert!(ambiguous.detail.contains("not unique"), "{}", ambiguous.detail);
        assert!(ambiguous.detail.contains("3"), "{}", ambiguous.detail);
    }

    #[test]
    fn verify_site_offline_pass_texts_state_the_sw_fetch_limit() {
        assert!(judge_first_visit(&clean()).detail.contains("SW's own fetch"));
        let mut m = clean(); m.view = String::new(); m.miss_page = true;
        assert!(judge_miss(&m).detail.contains("SW's own fetch"));
        let mut s = clean(); s.today_html = DIRECT_CHILD_TODAY.into(); s.today_count = 1; s.now_hidden = true;
        assert!(judge_stale_day(&s).detail.contains("SW's own fetch"));
        let mut online = clean(); online.state = "kept".into(); online.failed = vec!["x".into()];
        let mut off = clean(); off.rendered_at = "111".into();
        assert!(judge_kept_after_partial(&off, &online, "x", 1, "111").detail.contains("SW's own fetch"));
    }

    #[test]
    fn verify_site_offline_cache_probe_js_reads_the_cache_api() {
        for part in ["caches.open('td-viewer')", "c.match("] {
            assert!(MANIFEST_READ_JS.contains(part), "manifest js missing {part}");
            assert!(ASSETS_MATCH_JS.contains(part), "assets js missing {part}");
        }
        assert!(MANIFEST_READ_JS.contains("__KEY__"), "manifest js missing the key placeholder");
        assert!(ASSETS_MATCH_JS.contains("__KEYS__"), "assets js missing the keys placeholder");
    }

    #[test]
    fn verify_site_offline_fault_target_js_counts_label_occurrences() {
        assert!(FAULT_TARGET_JS.contains("labelCount"));
    }

    #[test]
    fn verify_site_offline_judge_miss_fails_when_page_still_served() {
        assert_eq!(judge_miss(&clean()).level, Level::Fail);
    }

    #[test]
    fn verify_site_offline_judge_miss_passes_miss_page_with_evidence() {
        let mut p = clean();
        p.view = String::new();
        p.miss_page = true;
        assert_eq!(judge_miss(&p).level, Level::Pass);
    }

    #[test]
    fn verify_site_offline_judge_miss_fails_without_offline_evidence() {
        let mut p = clean();
        p.view = String::new();
        p.miss_page = true;
        p.offline_evidence = false;
        let c = judge_miss(&p);
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("offline-only evidence"));
    }

    #[test]
    fn verify_site_offline_judge_token_purge_passes_when_gone_and_shared_map_survives() {
        let recorded = vec![
            "https://h/?plan=p&lang=zh&token=t2".to_string(),
            "https://h/voucher/a.pdf?token=t2".to_string(),
        ];
        let now = vec![
            "https://h/map/m.png?v=3".to_string(),
            "https://h/?plan=p&lang=zh&token=t1".to_string(),
        ];
        let c = judge_token_purge(&recorded, &now, Some("https://h/map/m.png?v=3"), "HTML, manifest, index, vouchers");
        assert_eq!(c.level, Level::Pass);
        assert!(c.detail.contains("2"), "{}", c.detail);
    }

    #[test]
    fn verify_site_offline_judge_token_purge_fails_on_leftover_key() {
        let recorded = vec!["https://h/voucher/a.pdf?token=t2".to_string()];
        let now = vec!["https://h/voucher/a.pdf?token=t2".to_string()];
        let c = judge_token_purge(&recorded, &now, None, "HTML, manifest, index, vouchers");
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("still present"));
    }

    #[test]
    fn verify_site_offline_judge_token_purge_fails_when_shared_map_swept() {
        let recorded = vec!["https://h/?plan=p&lang=zh&token=t2".to_string()];
        let c = judge_token_purge(&recorded, &[], Some("https://h/map/m.png?v=3"), "HTML, manifest, index, vouchers");
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("shared map"));
    }

    #[test]
    fn verify_site_offline_token_of_key_reads_manifest_voucher_and_map_keys() {
        assert_eq!(token_of_key("https://h/?plan=p&lang=zh&token=t2").as_deref(), Some("t2"));
        assert_eq!(token_of_key("https://h/voucher/a.pdf?v=abc&token=t2").as_deref(), Some("t2"));
        assert_eq!(
            token_of_key("https://h/__td/manifest?k=https%3A%2F%2Fh%2F%3Fplan%3Dp%26lang%3Dzh%26token%3Dt2")
                .as_deref(),
            Some("t2")
        );
        assert_eq!(token_of_key("https://h/map/a.png?v=3"), None);
    }

    #[test]
    fn verify_site_offline_today_open_accepts_only_a_direct_child() {
        assert!(today_card_open(DIRECT_CHILD_TODAY));
        assert!(!today_card_open(NESTED_OPEN_TODAY));
    }

    #[test]
    fn verify_site_offline_today_card_marked_on_the_day_block() {
        assert!(today_card_marked(DIRECT_CHILD_TODAY));
        let unmarked = DIRECT_CHILD_TODAY.replace(" data-today", "");
        assert!(!today_card_marked(&unmarked));
    }

    #[test]
    fn verify_site_offline_judge_stale_day_requires_today_marked_open_and_now_hidden() {
        let mut p = clean();
        p.today_html = DIRECT_CHILD_TODAY.into();
        p.today_count = 1;
        p.now_hidden = true;
        assert_eq!(judge_stale_day(&p).level, Level::Pass);

        let mut nested = p.clone();
        nested.today_html = NESTED_OPEN_TODAY.into();
        let c = judge_stale_day(&nested);
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("direct-child"));

        let mut two = p.clone();
        two.today_count = 2;
        let c = judge_stale_day(&two);
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("[data-today]"));

        let mut unmarked = p.clone();
        unmarked.today_html = DIRECT_CHILD_TODAY.replace(" data-today", "");
        let c = judge_stale_day(&unmarked);
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("today marker"));

        let mut shown = p.clone();
        shown.now_hidden = false;
        let c = judge_stale_day(&shown);
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("now"));

        let mut missing = p.clone();
        missing.today_html = String::new();
        let c = judge_stale_day(&missing);
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("day card"));
    }

    #[test]
    fn verify_site_offline_probe_output_redacts_tokens() {
        let token = "0123456789abcdef0123456789abcdef";
        let mut p = clean();
        p.vouchers.push((format!("/voucher/a.pdf?token={token}"), false));
        let c = judge_first_visit(&p);
        assert!(!c.detail.contains(token));
        assert!(c.detail.contains("012345...abcdef"));
    }

    #[test]
    fn verify_site_offline_probe_js_reads_contract() {
        for part in [
            "data-served",
            "#offline-status",
            "data-state",
            "naturalWidth",
            "%PDF",
            "__td/offline-probe",
            "TypeError",
            "outerHTML",
            "[data-today]",
        ] {
            assert!(OFFLINE_PROBE_JS.contains(part), "{part}");
        }
    }
}
