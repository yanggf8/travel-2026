//! Layer 5 of `travel verify-site` (`--render <dir>`): open the token viewer
//! page in a real Chrome (attach-only CDP via gwebcdb's `bridge-core`), in zh and
//! en at a mobile and a desktop viewport, and judge the RENDERED page — what the
//! DOM-level layer 1 cannot see: sideways overflow, broken images, links/variants/
//! maps that are present but not shown, text that starts left of its card's
//! title (flush against the card border), and text cut off at the card's right
//! edge by `.day { overflow: hidden }`.
//!
//! The browser side is thin: one new blank tab per language × viewport, navigate,
//! scroll (lazy images), wait for images and fonts, ONE metrics expression
//! (`METRICS_JS`) deserialized into `Metrics`, element screenshots, close. Every
//! browser await has a wall-clock bound, so `render_plan` always returns. Every
//! verdict comes from the pure `render_checks(&Metrics)`. The token URL is never
//! printed: every page-derived string is put through `scrub_page_text` (token →
//! fingerprint, any 16+-hex run masked) BEFORE a verdict truncates it, and every
//! emitted detail goes through `scrub` (token → fingerprint, any other 32-hex run
//! masked).
//!
//! Screenshots (`<dir>/<plan>/<lang>-<viewport>-NN.png`, one per summary box and
//! day card) are advisory, never a verdict. They are taken AFTER the verdicts, on
//! a viewport grown to the page height (capped at `MAX_SHOT_VIEWPORT_HEIGHT`) with
//! the root scrollbar hidden, and clip rects measured again in that layout
//! (`SHOT_RECTS_JS`). Why: the CDP clip uses `captureBeyondViewport`, so Chrome
//! grows the viewport to the full page during capture; a vertical scrollbar would
//! then disappear (mobile clientWidth 375 → 390), text would reflow and elements
//! move, and rects measured with the scrollbar cut the top of a card — cumulatively,
//! the further down the page (a mobile kyoto-nov page is ~18,500px, past the cap).
//! Mobile verdicts measure without the scrollbar too (`overlay_scrollbars`: phones
//! draw overlay scrollbars), so mobile shots show the measured layout; desktop
//! shots can differ from the desktop verdict layout by the scrollbar width.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

use regex::Regex;
use serde::Deserialize;

use crate::verify_site::{Check, Next, redact};

/// Layer name on every emitted line.
pub(crate) const LAYER: &str = "render";

/// One emulated viewport.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Viewport {
    pub(crate) name: &'static str,
    pub(crate) width: i64,
    pub(crate) height: i64,
    /// Phones draw overlay scrollbars that take no width, so the measured layout
    /// hides the root scrollbar (`HIDE_SCROLLBAR_JS`) to match a real phone.
    pub(crate) overlay_scrollbars: bool,
}

pub(crate) const VIEWPORTS: [Viewport; 2] = [
    Viewport { name: "mobile", width: 390, height: 844, overlay_scrollbars: true },
    Viewport { name: "desktop", width: 1280, height: 900, overlay_scrollbars: false },
];

/// A text block may start this far left of its card's title text before it is
/// an inset defect (sub-pixel layout rounding).
pub(crate) const INSET_TOLERANCE_PX: f64 = 1.0;
/// A text block whose right edge comes within this of the card's right edge is
/// cut by the card's `overflow: hidden`.
pub(crate) const CLIP_TOLERANCE_PX: f64 = 1.0;
/// Offenders listed per verdict line.
pub(crate) const MAX_LISTED: usize = 5;
/// Characters of an offender's text quoted in a verdict line.
pub(crate) const SNIPPET_CHARS: usize = 20;
/// Characters of a broken image src quoted in a verdict line.
const SRC_CHARS: usize = 120;

const GOTO_TIMEOUT: Duration = Duration::from_secs(90);
/// Bounded wait for every `<img>` to finish and the fonts to load.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(20);
/// Scroll step as a share of the viewport height, and the pause per step.
const SCROLL_STEP_SHARE: f64 = 0.8;
const SCROLL_PAUSE: Duration = Duration::from_millis(80);
/// Upper bound on scroll steps (a runaway page height must not loop forever).
const MAX_SCROLL_STEPS: usize = 400;

// Wall-clock bounds on browser awaits. A tab whose CDP handler died has request
// futures that never resolve; these turn a hang into a verdict.
/// Attaching to the CDP endpoint.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Opening one blank tab.
const NEW_TAB_TIMEOUT: Duration = Duration::from_secs(30);
/// The whole `measure` call: its own goto and settle bounds plus a margin for the
/// scroll loop (≤ MAX_SCROLL_STEPS × SCROLL_PAUSE) and the evaluates.
const MEASURE_DEADLINE: Duration = Duration::from_secs(GOTO_TIMEOUT.as_secs() + SETTLE_TIMEOUT.as_secs() + 60);
/// Re-laying the page out for screenshots and measuring the clip rects.
const SHOT_PREP_TIMEOUT: Duration = Duration::from_secs(30);
/// Settle wait after the screenshot viewport resize (a timeout is ignored).
const SHOT_SETTLE_TIMEOUT: Duration = Duration::from_secs(10);
/// One screenshot.
const SHOT_TIMEOUT: Duration = Duration::from_secs(30);
/// Closing the tab.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(15);
/// Tallest viewport the screenshot pass asks for.
const MAX_SHOT_VIEWPORT_HEIGHT: i64 = 16000;

/// What the metrics expression measured on one rendered page.
/// Strict: a key the JS does not return is a decode error, never a silent zero.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Metrics {
    /// `window.innerWidth` — the emulated viewport (a classic scrollbar makes
    /// `clientWidth` narrower).
    pub(crate) inner_width: f64,
    pub(crate) client_width: f64,
    pub(crate) scroll_width: f64,
    /// `section.day[data-day]` (main day cards).
    pub(crate) day_cards: usize,
    pub(crate) imgs: usize,
    /// `src` of every `img` with `complete && naturalWidth == 0`.
    pub(crate) broken_imgs: Vec<String>,
    /// `img` still loading after the settle wait.
    pub(crate) pending_imgs: usize,
    /// Every `a.doc-link`.
    pub(crate) doc_links: Vec<Shown>,
    /// Every `details.day-variant` (with `open`).
    pub(crate) variants: Vec<Shown>,
    /// Every `figure[data-map-key] img` (label = the map key).
    pub(crate) map_imgs: Vec<Shown>,
    /// Every visible element with direct non-whitespace text inside a
    /// `section.day` or `.summary-box`, outside maps and overlays.
    pub(crate) text_blocks: Vec<TextBlock>,
    pub(crate) fonts_status: String,
}

/// An element that must be shown, and how it renders.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Shown {
    pub(crate) label: String,
    /// Empty when shown; else `display:none`, `visibility:hidden` or `zero-size`.
    pub(crate) state: String,
    /// `details` only.
    #[serde(default)]
    pub(crate) open: Option<bool>,
}

/// One text-bearing element: where its direct text starts and ends.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TextBlock {
    /// `tag.class.class`.
    pub(crate) element: String,
    pub(crate) text: String,
    /// Left/right of the union of the element's direct text nodes (trimmed).
    pub(crate) left: f64,
    pub(crate) right: f64,
    /// The container's title text start: its `h2` left + border + padding-left.
    pub(crate) ref_x: f64,
    /// The day card's right edge (`None` in a summary box).
    pub(crate) card_right: Option<f64>,
    /// `text-overflow: ellipsis` on an element whose `overflow-x` is not
    /// `visible` (so the ellipsis can take effect) — a deliberate truncation,
    /// never "clipped".
    pub(crate) ellipsis: bool,
}

/// A document-coordinate screenshot clip (one `SHOT_RECTS_JS` entry).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub(crate) struct ShotRect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

/// The one in-page measurement. An IIFE (`(function(){…})()`) so chromiumoxide
/// evaluates it as an expression and returns the object by value.
pub(crate) const METRICS_JS: &str = r#"(function () {
  var de = document.documentElement;
  function norm(s) { return (s || '').replace(/\s+/g, ' ').trim(); }
  function boxState(el) {
    var cs = getComputedStyle(el), r = el.getBoundingClientRect();
    if (cs.display === 'none') return 'display:none';
    if (cs.visibility === 'hidden') return 'visibility:hidden';
    if (r.width <= 0 || r.height <= 0) return 'zero-size';
    return '';
  }
  function desc(el) {
    var c = typeof el.className === 'string' ? el.className.trim().split(/\s+/).filter(Boolean).join('.') : '';
    return el.tagName.toLowerCase() + (c ? '.' + c : '');
  }
  function directText(el) {
    var left = null, right = null, text = '';
    for (var n = el.firstChild; n; n = n.nextSibling) {
      if (n.nodeType !== 3) continue;
      var s = n.nodeValue, a = s.search(/\S/);
      if (a < 0) continue;
      var b = s.replace(/\s+$/, '').length;
      var range = document.createRange();
      range.setStart(n, a);
      range.setEnd(n, b);
      var r = range.getBoundingClientRect();
      if (r.width <= 0 && r.height <= 0) continue;
      if (!text) text = norm(s).slice(0, 60);
      left = left === null ? r.left : Math.min(left, r.left);
      right = right === null ? r.right : Math.max(right, r.right);
    }
    return left === null ? null : { left: left, right: right, text: text };
  }
  function overlay(el, stop) {
    for (var p = el; p && p !== stop; p = p.parentElement) {
      var pos = getComputedStyle(p).position;
      if (pos === 'absolute' || pos === 'fixed') return true;
    }
    return false;
  }
  var textBlocks = [];
  document.querySelectorAll('section.day, .summary-box').forEach(function (c) {
    if (boxState(c)) return;
    var cr = c.getBoundingClientRect();
    var h2 = c.querySelector(':scope > h2') || c.querySelector('h2');
    var refX;
    if (h2 && !boxState(h2)) {
      var hs = getComputedStyle(h2);
      refX = h2.getBoundingClientRect().left + parseFloat(hs.borderLeftWidth) + parseFloat(hs.paddingLeft);
    } else {
      var cs = getComputedStyle(c);
      refX = cr.left + parseFloat(cs.borderLeftWidth) + parseFloat(cs.paddingLeft);
    }
    var isDay = c.matches('section.day');
    // The container's own direct text as well as every descendant's.
    [c].concat(Array.prototype.slice.call(c.querySelectorAll('*'))).forEach(function (el) {
      if (el.closest('.day-map-stack, figure') || boxState(el) || overlay(el, c)) return;
      var t = directText(el);
      if (!t) return;
      var es = getComputedStyle(el);
      textBlocks.push({
        element: desc(el), text: t.text, left: t.left, right: t.right, refX: refX,
        cardRight: isDay ? cr.right : null,
        // An ellipsis only takes effect when the box does not let overflow show.
        ellipsis: es.textOverflow === 'ellipsis' && es.overflowX !== 'visible'
      });
    });
  });
  var imgs = Array.prototype.slice.call(document.images);
  return {
    innerWidth: window.innerWidth,
    clientWidth: de.clientWidth,
    scrollWidth: de.scrollWidth,
    dayCards: document.querySelectorAll('section.day[data-day]').length,
    imgs: imgs.length,
    brokenImgs: imgs.filter(function (i) { return i.complete && i.naturalWidth === 0; })
      .map(function (i) { return i.getAttribute('src') || ''; }),
    pendingImgs: imgs.filter(function (i) { return !i.complete; }).length,
    docLinks: Array.prototype.map.call(document.querySelectorAll('a.doc-link'), function (a) {
      return { label: norm(a.textContent).slice(0, 40), state: boxState(a) };
    }),
    variants: Array.prototype.map.call(document.querySelectorAll('details.day-variant'), function (d) {
      return { label: norm((d.querySelector('summary') || d).textContent).slice(0, 40), state: boxState(d), open: d.open };
    }),
    mapImgs: Array.prototype.map.call(document.querySelectorAll('figure[data-map-key] img'), function (i) {
      return { label: i.closest('figure').getAttribute('data-map-key') || '', state: boxState(i) };
    }),
    textBlocks: textBlocks,
    fontsStatus: document.fonts ? document.fonts.status : 'unsupported'
  };
})()"#;

/// Document-coordinate rects of the booking summary, plan documents and day
/// cards, in document order — the screenshot clips. Evaluated only after the
/// viewport has been grown to the page height (see the module doc), so the rects
/// match the layout `captureBeyondViewport` renders.
pub(crate) const SHOT_RECTS_JS: &str = r#"(function () {
  var shots = [];
  document.querySelectorAll('section.booking-summary, section.plan-documents, section.day').forEach(function (el) {
    var r = el.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return;
    shots.push({ x: r.left + window.scrollX, y: r.top + window.scrollY, width: r.width, height: r.height });
  });
  return shots;
})()"#;
/// Hide the root scrollbar (idempotent). Returns the resulting clientWidth.
const HIDE_SCROLLBAR_JS: &str = "(function(){ if (!document.getElementById('verify-site-no-scrollbar')) { var s = document.createElement('style'); s.id = 'verify-site-no-scrollbar'; s.textContent = 'html{scrollbar-width:none}'; document.head.appendChild(s); } return document.documentElement.clientWidth; })()";
const SCROLL_HEIGHT_JS: &str = "(function(){ return document.documentElement.scrollHeight; })()";
const SCROLL_TOP_JS: &str = "(function(){ window.scrollTo(0, 0); return true; })()";

/// Lazy images load regardless of scroll position once marked eager, so the
/// settle wait does not depend on how a (possibly background) tab schedules them.
/// Returns the page height for the scroll loop.
const PREPARE_JS: &str = "(function(){ document.querySelectorAll('img[loading=lazy]').forEach(function(i){ i.loading = 'eager'; }); return document.documentElement.scrollHeight; })()";
const SETTLED_JS: &str = "(function(){ return Array.prototype.every.call(document.images, function(i){ return i.complete; }) && (!document.fonts || document.fonts.status === 'loaded'); })()";

// ---------------------------------------------------------------------------
// Pure verdicts
// ---------------------------------------------------------------------------

/// Every verdict for one rendered page. Pure.
pub(crate) fn render_checks(m: &Metrics) -> Vec<Check> {
    let mut out = Vec::new();

    if m.scroll_width > m.client_width {
        out.push(Check::fail(
            format!(
                "overflow: page scrolls sideways (scrollWidth={:.0} > clientWidth={:.0})",
                m.scroll_width, m.client_width
            ),
            None,
        ));
    }

    if !m.broken_imgs.is_empty() {
        out.push(Check::fail(
            format!(
                "broken-img: {} image(s) failed to load: {}",
                m.broken_imgs.len(),
                listed(m.broken_imgs.iter().map(|s| format!("{:?}", snippet(&mask_hex(s), SRC_CHARS))), m.broken_imgs.len())
            ),
            None,
        ));
    }

    let mut hidden: Vec<String> = Vec::new();
    for d in &m.doc_links {
        if !d.state.is_empty() {
            hidden.push(format!("a.doc-link {:?} {}", snippet(&d.label, SNIPPET_CHARS), d.state));
        }
    }
    for v in &m.variants {
        if !v.state.is_empty() {
            hidden.push(format!("details.day-variant {:?} {}", snippet(&v.label, SNIPPET_CHARS), v.state));
        } else if v.open != Some(true) {
            hidden.push(format!("details.day-variant {:?} closed", snippet(&v.label, SNIPPET_CHARS)));
        }
    }
    for i in &m.map_imgs {
        if !i.state.is_empty() {
            hidden.push(format!("img[data-map-key={}] {}", i.label, i.state));
        }
    }
    if !hidden.is_empty() {
        out.push(Check::fail(
            format!("hidden: {} element(s) present but not shown: {}", hidden.len(), listed(hidden.iter().cloned(), hidden.len())),
            None,
        ));
    }

    let inset: Vec<&TextBlock> = m.text_blocks.iter().filter(|b| b.left < b.ref_x - INSET_TOLERANCE_PX).collect();
    if !inset.is_empty() {
        out.push(Check::fail(
            format!(
                "inset: {} text block(s) start left of their card's title text: {}",
                inset.len(),
                listed(
                    inset.iter().map(|b| format!(
                        "{} {:?} left={:.0} ref={:.0}",
                        b.element,
                        snippet(&b.text, SNIPPET_CHARS),
                        b.left,
                        b.ref_x
                    )),
                    inset.len()
                )
            ),
            None,
        ));
    }

    let clipped: Vec<(&TextBlock, f64)> = m
        .text_blocks
        .iter()
        .filter(|b| !b.ellipsis)
        .filter_map(|b| b.card_right.map(|r| (b, r)))
        .filter(|(b, r)| b.right > r - CLIP_TOLERANCE_PX)
        .collect();
    if !clipped.is_empty() {
        out.push(Check::fail(
            format!(
                "clipped: {} text block(s) run into the day card's right edge (cut by overflow:hidden): {}",
                clipped.len(),
                listed(
                    clipped.iter().map(|(b, r)| format!(
                        "{} {:?} right={:.0} card={:.0}",
                        b.element,
                        snippet(&b.text, SNIPPET_CHARS),
                        b.right,
                        r
                    )),
                    clipped.len()
                )
            ),
            None,
        ));
    }

    if m.fonts_status != "loaded" {
        out.push(Check::warn(format!("fonts: document.fonts.status={:?} (not loaded)", m.fonts_status), None));
    }
    if m.pending_imgs > 0 {
        out.push(Check::warn(
            format!("images: {} img still loading after {}s", m.pending_imgs, SETTLE_TIMEOUT.as_secs()),
            None,
        ));
    }
    if m.day_cards == 0 {
        out.push(Check::warn("empty: no section.day rendered — nothing to measure (auth page?)", None));
    }

    if out.is_empty() {
        out.push(Check::pass(format!(
            "{:.0}px: {} day card(s), {} img, {} doc link(s), {} variant(s), {} map img, {} text block(s); \
             no overflow/broken/hidden/inset/clip",
            m.inner_width,
            m.day_cards,
            m.imgs,
            m.doc_links.len(),
            m.variants.len(),
            m.map_imgs.len(),
            m.text_blocks.len()
        )));
    }
    out
}

/// Up to MAX_LISTED items joined, with "(+N more)" past that.
fn listed(items: impl Iterator<Item = String>, total: usize) -> String {
    let mut s = items.take(MAX_LISTED).collect::<Vec<_>>().join("; ");
    if total > MAX_LISTED {
        s.push_str(&format!("; (+{} more)", total - MAX_LISTED));
    }
    s
}

/// First `n` characters (not bytes) of `s`.
fn snippet(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

/// Mask every run of 32+ hex digits (a share token or any other secret-shaped id).
pub(crate) fn mask_hex(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("[0-9a-fA-F]{32,}").expect("static regex"))
        .replace_all(s, "<hex>")
        .into_owned()
}

/// Make any text safe to print: the token becomes its fingerprint, any other
/// 32-hex run is masked.
pub(crate) fn scrub(text: &str, token: &str) -> String {
    mask_hex(&redact(text, token))
}

/// Mask every run of 16+ hex digits. Stricter than `mask_hex` because page text
/// arrives pre-truncated (the JS slices text and labels), so a token can be
/// present only as a fragment that neither `redact` nor a 32-hex mask matches.
pub(crate) fn mask_hex_page(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("[0-9a-fA-F]{16,}").expect("static regex"))
        .replace_all(s, "<hex>")
        .into_owned()
}

/// Make one page-derived string safe before any verdict quotes (and truncates) it.
pub(crate) fn scrub_page_text(s: &str, token: &str) -> String {
    mask_hex_page(&redact(s, token))
}

/// Every page-derived string of `m` through `scrub_page_text`, so the pure
/// `render_checks` never sees a token or a token fragment.
pub(crate) fn scrub_metrics(m: Metrics, token: &str) -> Metrics {
    let s = |v: String| scrub_page_text(&v, token);
    let shown = |v: Vec<Shown>| -> Vec<Shown> { v.into_iter().map(|x| Shown { label: s(x.label), ..x }).collect() };
    Metrics {
        broken_imgs: m.broken_imgs.into_iter().map(s).collect(),
        doc_links: shown(m.doc_links),
        variants: shown(m.variants),
        map_imgs: shown(m.map_imgs),
        text_blocks: m
            .text_blocks
            .into_iter()
            .map(|b| TextBlock { element: s(b.element), text: s(b.text), ..b })
            .collect(),
        fonts_status: s(m.fonts_status),
        ..m
    }
}

/// Viewport height for the screenshot pass: the page height (rounded up), never
/// below the emulated viewport and never above `MAX_SHOT_VIEWPORT_HEIGHT`.
pub(crate) fn shot_viewport_height(scroll_height: f64, vp_height: i64) -> i64 {
    let h = if scroll_height.is_finite() { scroll_height.ceil() as i64 } else { vp_height };
    h.max(vp_height).min(MAX_SHOT_VIEWPORT_HEIGHT)
}

/// A browser step that failed for one language × viewport.
pub(crate) fn failure_check(step: &str, err: &str, token: &str) -> Check {
    Check::fail(format!("render failed at {step}: {}", scrub(err, token)), None)
}

/// No CDP Chrome answered: fail loud, never skip.
pub(crate) fn connect_failure_check(endpoint: &str, err: &str, token: &str) -> Check {
    Check::fail(
        format!("no CDP Chrome answers at {endpoint} ({}); --render needs one", scrub(err, token)),
        Some(Next::Chrome),
    )
}

/// Screenshot file name: `<lang>-<viewport>-NN.png` (1-based).
pub(crate) fn shot_name(lang: &str, viewport: &str, n: usize) -> String {
    format!("{lang}-{viewport}-{n:02}.png")
}

/// A file this layer wrote on an earlier run (removed before a new run so a plan
/// that lost a day card does not keep a stale shot).
pub(crate) fn is_own_shot(name: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(zh|en)-(mobile|desktop)-\d{2,}\.png$").expect("static regex"))
        .is_match(name)
}

// ---------------------------------------------------------------------------
// Browser I/O (thin)
// ---------------------------------------------------------------------------

/// What one plan's render pass produced.
pub(crate) struct RenderReport {
    /// (key, check), key = `<lang>-<viewport>` (or `cdp`).
    pub(crate) checks: Vec<(String, Check)>,
    pub(crate) shot_dir: PathBuf,
    /// Screenshots written; `None` when no browser was reached.
    pub(crate) shots: Option<usize>,
}

/// Render `targets` (`(lang code, token URL)`) at every viewport. Never returns
/// an error: every failure is a verdict, scrubbed of the token. Every browser
/// await is bounded (see the `*_TIMEOUT` / `MEASURE_DEADLINE` constants), so this
/// always returns, and the token can be deleted by the caller right after.
pub(crate) async fn render_plan(plan: &str, targets: &[(&str, String)], token: &str, dir: &Path) -> RenderReport {
    let shot_dir = dir.join(plan);
    let mut report = RenderReport { checks: Vec::new(), shot_dir: shot_dir.clone(), shots: None };
    let endpoint = bridge_core::runtime::cdp_endpoint(&bridge_core::sys::SysEnv);
    let browser = match tokio::time::timeout(CONNECT_TIMEOUT, bridge_core::cdp::connect(&endpoint)).await {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            report.checks.push(("cdp".into(), connect_failure_check(&endpoint, &e.to_string(), token)));
            return report;
        }
        Err(_) => {
            let e = format!("timed out after {}s", CONNECT_TIMEOUT.as_secs());
            report.checks.push(("cdp".into(), connect_failure_check(&endpoint, &e, token)));
            return report;
        }
    };

    let mut written = 0;
    let shots_ready = match prepare_shot_dir(&shot_dir) {
        Ok(warnings) => {
            for w in warnings {
                report.checks.push(("shots".into(), Check::warn(format!("shots: {}", scrub(&w, token)), None)));
            }
            true
        }
        Err(e) => {
            report.checks.push(("shots".into(), Check::warn(format!("screenshots skipped: {}", scrub(&e, token)), None)));
            false
        }
    };
    for (lang, url) in targets {
        for vp in VIEWPORTS {
            let key = format!("{lang}-{}", vp.name);
            let page = match tokio::time::timeout(NEW_TAB_TIMEOUT, browser.new_blank_page()).await {
                Ok(Ok(p)) => p,
                Ok(Err(e)) => {
                    report.checks.push((key, failure_check("new tab", &e.to_string(), token)));
                    continue;
                }
                Err(_) => {
                    let e = format!("timed out after {}s", NEW_TAB_TIMEOUT.as_secs());
                    report.checks.push((key, failure_check("new tab", &e, token)));
                    continue;
                }
            };
            // Everything that touches the tab runs here, so the close below
            // happens on every path (a measure timeout included).
            match tokio::time::timeout(MEASURE_DEADLINE, measure(&page, url, vp)).await {
                Ok(Ok(m)) => {
                    let m = scrub_metrics(m, token);
                    for c in render_checks(&m) {
                        report.checks.push((key.clone(), Check { detail: scrub(&c.detail, token), ..c }));
                    }
                    if shots_ready {
                        let rects = match tokio::time::timeout(SHOT_PREP_TIMEOUT, shot_rects(&page, vp)).await {
                            Ok(r) => r,
                            Err(_) => Err(format!("timed out after {}s", SHOT_PREP_TIMEOUT.as_secs())),
                        };
                        match rects {
                            Ok(rects) => {
                                let (n, errs) = take_shots(&page, &rects, &shot_dir, lang, vp.name).await;
                                written += n;
                                for e in errs {
                                    report.checks.push((key.clone(), Check::warn(format!("screenshot: {}", scrub(&e, token)), None)));
                                }
                            }
                            Err(e) => report.checks.push((
                                key.clone(),
                                Check::warn(format!("screenshots skipped: layout for shots failed: {}", scrub(&e, token)), None),
                            )),
                        }
                    }
                }
                Ok(Err((step, e))) => report.checks.push((key.clone(), failure_check(step, &e, token))),
                Err(_) => {
                    let e = format!("timed out after {}s", MEASURE_DEADLINE.as_secs());
                    report.checks.push((key.clone(), failure_check("measure (deadline)", &e, token)));
                }
            }
            let closed = match tokio::time::timeout(CLOSE_TIMEOUT, page.close()).await {
                Ok(r) => r.map_err(|e| e.to_string()),
                Err(_) => Err(format!("timed out after {}s", CLOSE_TIMEOUT.as_secs())),
            };
            if let Err(e) = closed {
                report.checks.push((key, Check::warn(format!("could not close the render tab: {}", scrub(&e, token)), None)));
            }
        }
    }
    report.shots = Some(written);
    report
}

/// Navigate, settle, measure. The error names the failed step.
async fn measure(
    page: &bridge_core::cdp::BridgePage,
    url: &str,
    vp: Viewport,
) -> Result<Metrics, (&'static str, String)> {
    page.set_viewport(vp.width, vp.height).await.map_err(|e| ("set viewport", e.to_string()))?;
    page.goto(url, GOTO_TIMEOUT).await.map_err(|e| ("navigate", e.to_string()))?;
    if vp.overlay_scrollbars {
        page.evaluate_json(HIDE_SCROLLBAR_JS).await.map_err(|e| ("hide scrollbar", e.to_string()))?;
    }
    let height = page
        .evaluate_json(PREPARE_JS)
        .await
        .map_err(|e| ("prepare", e.to_string()))?
        .as_f64()
        .unwrap_or(0.0);
    let step = (vp.height as f64 * SCROLL_STEP_SHARE).max(1.0);
    let mut y = 0.0;
    let mut steps = 0;
    while y < height && steps < MAX_SCROLL_STEPS {
        page.evaluate_json(&format!("(function(){{ window.scrollTo(0, {y:.0}); return true; }})()"))
            .await
            .map_err(|e| ("scroll", e.to_string()))?;
        tokio::time::sleep(SCROLL_PAUSE).await;
        y += step;
        steps += 1;
    }
    page.evaluate_json(SCROLL_TOP_JS).await.map_err(|e| ("scroll", e.to_string()))?;
    // Bounded; a timeout is not an error here — the metrics report what is
    // still pending (pendingImgs / fontsStatus) and the verdicts WARN on it.
    let _ = page.wait_for_function(SETTLED_JS, SETTLE_TIMEOUT).await;
    let value = page.evaluate_json(METRICS_JS).await.map_err(|e| ("measure", e.to_string()))?;
    serde_json::from_value(value).map_err(|e| ("decode metrics", e.to_string()))
}

/// Re-lay the page out for screenshots and measure the clips in THAT layout:
/// grow the viewport to the page height (so `captureBeyondViewport` has nothing
/// left to grow and no scrollbar disappears mid-capture), settle, scroll to the
/// top, then `SHOT_RECTS_JS`. Runs after the verdicts, which keep measuring the
/// real viewport. The caller bounds the whole call.
async fn shot_rects(page: &bridge_core::cdp::BridgePage, vp: Viewport) -> Result<Vec<ShotRect>, String> {
    // No scrollbar in the shot layout, so the capture cannot drop one and reflow.
    page.evaluate_json(HIDE_SCROLLBAR_JS).await.map_err(|e| format!("hide scrollbar: {e}"))?;
    let scroll_height = page
        .evaluate_json(SCROLL_HEIGHT_JS)
        .await
        .map_err(|e| format!("page height: {e}"))?
        .as_f64()
        .ok_or("page height: not a number")?;
    page.set_viewport(vp.width, shot_viewport_height(scroll_height, vp.height))
        .await
        .map_err(|e| format!("set viewport: {e}"))?;
    // Bounded twice (its own poll bound and a hard timeout around a hung
    // evaluate); a timeout only means the shots may show a still-loading image.
    let _ = tokio::time::timeout(SHOT_SETTLE_TIMEOUT, page.wait_for_function(SETTLED_JS, SHOT_SETTLE_TIMEOUT)).await;
    page.evaluate_json(SCROLL_TOP_JS).await.map_err(|e| format!("scroll: {e}"))?;
    let value = page.evaluate_json(SHOT_RECTS_JS).await.map_err(|e| format!("shot rects: {e}"))?;
    serde_json::from_value(value).map_err(|e| format!("decode shot rects: {e}"))
}

/// Create `<dir>/<plan>` and remove this layer's shots from an earlier run.
/// `Err` (the dir cannot be created or listed) skips screenshots; `Ok` carries
/// one warning per directory entry that could not be read and per stale shot
/// that could not be removed (a stale shot left behind would pass for a fresh one).
fn prepare_shot_dir(dir: &Path) -> Result<Vec<String>, String> {
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let entries = fs::read_dir(dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
    let mut warnings = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warnings.push(format!("could not read an entry of {}: {e}", dir.display()));
                continue;
            }
        };
        let name = entry.file_name();
        if let Some(name) = name.to_str().filter(|n| is_own_shot(n)) {
            if let Err(e) = fs::remove_file(entry.path()) {
                warnings.push(format!("could not remove stale {name}: {e}"));
            }
        }
    }
    Ok(warnings)
}

/// One PNG per rect. Returns (written, errors); never aborts. A screenshot that
/// times out ends this tab's shots (its CDP session is likely dead, and every
/// further shot would wait out the same bound).
async fn take_shots(
    page: &bridge_core::cdp::BridgePage,
    rects: &[ShotRect],
    dir: &Path,
    lang: &str,
    viewport: &str,
) -> (usize, Vec<String>) {
    let mut written = 0;
    let mut errors = Vec::new();
    for (i, r) in rects.iter().enumerate() {
        let name = shot_name(lang, viewport, i + 1);
        match tokio::time::timeout(SHOT_TIMEOUT, page.screenshot_clip(r.x, r.y, r.width, r.height, 1.0)).await {
            Ok(Ok(png)) => match fs::write(dir.join(&name), png) {
                Ok(()) => written += 1,
                Err(e) => errors.push(format!("{name}: write failed: {e}")),
            },
            Ok(Err(e)) => errors.push(format!("{name}: {e}")),
            Err(_) => {
                errors.push(format!(
                    "{name}: timed out after {}s; {} remaining shot(s) skipped",
                    SHOT_TIMEOUT.as_secs(),
                    rects.len() - i - 1
                ));
                break;
            }
        }
    }
    (written, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify_site::Level;

    fn any_fail(checks: &[Check]) -> bool {
        checks.iter().any(|c| c.level == Level::Fail)
    }

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn block(element: &str, text: &str, left: f64, right: f64) -> TextBlock {
        TextBlock {
            element: element.into(),
            text: text.into(),
            left,
            right,
            ref_x: 20.0,
            card_right: Some(390.0),
            ellipsis: false,
        }
    }

    fn shown(label: &str) -> Shown {
        Shown { label: label.into(), state: String::new(), open: None }
    }

    fn good() -> Metrics {
        Metrics {
            inner_width: 390.0,
            client_width: 375.0,
            scroll_width: 375.0,
            day_cards: 5,
            imgs: 18,
            broken_imgs: vec![],
            pending_imgs: 0,
            doc_links: vec![shown("Ticket A"), shown("Ticket B")],
            variants: vec![Shown { label: "Rain plan".into(), state: String::new(), open: Some(true) }],
            map_imgs: vec![shown("day-1.png")],
            text_blocks: vec![
                block("h2", "第 1 天", 20.0, 120.0),
                block("div.theme", "京都巡禮", 20.0, 200.0),
                // Nested deeper (a variant body) is fine.
                block("div.session-focus", "寺院散策", 45.0, 300.0),
                // Just inside the tolerance on both edges.
                block("li", "stop", 19.2, 389.0 - CLIP_TOLERANCE_PX + 0.5),
            ],
            fonts_status: "loaded".into(),
        }
    }

    fn only(checks: &[Check], prefix: &str) -> Vec<Check> {
        checks.iter().filter(|c| c.detail.starts_with(prefix)).cloned().collect()
    }

    #[test]
    fn all_good_metrics_give_one_pass_line() {
        let checks = render_checks(&good());
        assert_eq!(checks.len(), 1, "{checks:?}");
        assert_eq!(checks[0].level, Level::Pass);
        assert!(checks[0].detail.starts_with("390px: 5 day card(s), 18 img, 2 doc link(s), 1 variant(s)"), "{}", checks[0].detail);
        assert!(checks[0].detail.contains("no overflow/broken/hidden/inset/clip"));
    }

    #[test]
    fn sideways_overflow_fails_with_both_widths() {
        let m = Metrics { scroll_width: 412.0, ..good() };
        assert!(render_checks(&Metrics { scroll_width: 375.0, ..good() })[0].level == Level::Pass);
        let c = only(&render_checks(&m), "overflow:");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].level, Level::Fail);
        assert!(c[0].detail.contains("scrollWidth=412") && c[0].detail.contains("clientWidth=375"), "{}", c[0].detail);
        assert!(!render_checks(&m).iter().any(|c| c.level == Level::Pass));
    }

    #[test]
    fn broken_images_fail_listing_five_masked_srcs() {
        let srcs: Vec<String> = (0..7).map(|i| format!("/voucher/p/{i}?token={TOKEN}")).collect();
        let m = Metrics { broken_imgs: srcs, ..good() };
        let c = only(&render_checks(&m), "broken-img:");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].level, Level::Fail);
        assert!(c[0].detail.contains("7 image(s)") && c[0].detail.contains("(+2 more)"), "{}", c[0].detail);
        assert!(c[0].detail.contains("/voucher/p/4?token=<hex>") && !c[0].detail.contains("/voucher/p/5"), "{}", c[0].detail);
        assert!(!c[0].detail.contains(TOKEN));
    }

    #[test]
    fn hidden_links_closed_variants_and_hidden_maps_fail() {
        let mut m = good();
        m.doc_links.push(Shown { label: "Voucher".into(), state: "zero-size".into(), open: None });
        m.variants.push(Shown { label: "Plan B".into(), state: String::new(), open: Some(false) });
        m.variants.push(Shown { label: "Plan C".into(), state: "display:none".into(), open: Some(true) });
        m.map_imgs.push(Shown { label: "day-2.png".into(), state: "visibility:hidden".into(), open: None });
        let c = only(&render_checks(&m), "hidden:");
        assert_eq!(c.len(), 1, "{c:?}");
        let d = &c[0].detail;
        assert_eq!(c[0].level, Level::Fail);
        assert!(d.starts_with("hidden: 4 element(s)"), "{d}");
        assert!(d.contains("a.doc-link \"Voucher\" zero-size"), "{d}");
        assert!(d.contains("details.day-variant \"Plan B\" closed"), "{d}");
        assert!(d.contains("details.day-variant \"Plan C\" display:none"), "{d}");
        assert!(d.contains("img[data-map-key=day-2.png] visibility:hidden"), "{d}");
    }

    #[test]
    fn text_left_of_the_title_fails_inset() {
        // The fixed defect: doc chips / stop list flush against the card border.
        let mut m = good();
        m.text_blocks.push(block("a.doc-link", "第 2 天 嵐山小火車 車票 (PDF) 下載", 13.0, 200.0));
        m.text_blocks.push(block("li", "金閣寺", 4.0, 100.0));
        let c = only(&render_checks(&m), "inset:");
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].level, Level::Fail);
        let d = &c[0].detail;
        assert!(d.starts_with("inset: 2 text block(s)"), "{d}");
        assert!(d.contains("a.doc-link \"第 2 天 嵐山小火車 車票 (PDF)\u{2026}\" left=13 ref=20"), "{d}");
        assert!(d.contains("li \"金閣寺\" left=4 ref=20"), "{d}");
        // At the tolerance the good fixture already passes; 1.5px left fails.
        let mut m = good();
        m.text_blocks.push(block("p", "x", 20.0 - INSET_TOLERANCE_PX - 0.5, 50.0));
        assert!(any_fail(&render_checks(&m)));
    }

    #[test]
    fn inset_lists_at_most_five_offenders() {
        let mut m = good();
        for i in 0..8 {
            m.text_blocks.push(block("li", &format!("stop {i}"), 4.0, 100.0));
        }
        let d = only(&render_checks(&m), "inset:")[0].detail.clone();
        assert!(d.starts_with("inset: 8 text block(s)") && d.contains("(+3 more)"), "{d}");
        assert!(d.contains("\"stop 4\"") && !d.contains("\"stop 5\""), "{d}");
    }

    #[test]
    fn text_cut_at_the_card_edge_fails_clipped_unless_ellipsis_or_summary() {
        let mut m = good();
        m.text_blocks.push(block("span.stop-price", "¥1,500 大人", 300.0, 420.0));
        let c = only(&render_checks(&m), "clipped:");
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].level, Level::Fail);
        assert!(c[0].detail.contains("span.stop-price \"¥1,500 大人\" right=420 card=390"), "{}", c[0].detail);
        // A deliberate ellipsis and a summary-box block (no card edge) are not clipped.
        let mut m = good();
        m.text_blocks.push(TextBlock { ellipsis: true, ..block("span", "long", 300.0, 420.0) });
        m.text_blocks.push(TextBlock { card_right: None, ..block("div", "summary", 30.0, 2000.0) });
        assert!(only(&render_checks(&m), "clipped:").is_empty());
    }

    #[test]
    fn only_an_effective_ellipsis_exempts_a_clip() {
        // ellipsis=true (text-overflow:ellipsis with overflow-x not visible) is exempt;
        // ellipsis=false (no ellipsis, or one that cannot take effect) FAILs clipped.
        let mut m = good();
        m.text_blocks.push(TextBlock { ellipsis: true, ..block("span.t", "long", 300.0, 420.0) });
        assert!(only(&render_checks(&m), "clipped:").is_empty());
        let mut m = good();
        m.text_blocks.push(TextBlock { ellipsis: false, ..block("span.t", "long", 300.0, 420.0) });
        let c = only(&render_checks(&m), "clipped:");
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].level, Level::Fail);
        // The JS only reports an ellipsis that can take effect.
        assert!(
            METRICS_JS.contains("es.textOverflow === 'ellipsis' && es.overflowX !== 'visible'"),
            "METRICS_JS must require overflow-x != visible for ellipsis"
        );
    }

    #[test]
    fn metrics_js_measures_the_container_text_too() {
        assert!(METRICS_JS.contains("[c].concat(Array.prototype.slice.call(c.querySelectorAll('*')))"));
        assert!(!METRICS_JS.contains("shots"), "shot rects are measured by SHOT_RECTS_JS after the resize");
    }

    #[test]
    fn unloaded_fonts_pending_images_and_empty_pages_only_warn() {
        let m = Metrics { fonts_status: "loading".into(), pending_imgs: 2, ..good() };
        let checks = render_checks(&m);
        assert_eq!(checks.len(), 2, "{checks:?}");
        assert!(checks.iter().all(|c| c.level == Level::Warn));
        assert!(checks[0].detail.starts_with("fonts:") && checks[1].detail.starts_with("images: 2 img"));
        let m = Metrics { day_cards: 0, text_blocks: vec![], ..good() };
        let checks = render_checks(&m);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].level, Level::Warn);
        assert!(checks[0].detail.starts_with("empty:"));
    }

    #[test]
    fn the_token_never_reaches_a_detail() {
        // bridge-core's goto error embeds the URL it navigated to.
        let err = format!("cdp error: navigation to https://h/?plan=p&token={TOKEN}&lang=en timed out");
        let c = failure_check("navigate", &err, TOKEN);
        assert_eq!(c.level, Level::Fail);
        assert!(!c.detail.contains(TOKEN), "{}", c.detail);
        assert!(c.detail.contains("render failed at navigate") && c.detail.contains("012345...abcdef"), "{}", c.detail);
        // Any other 32-hex run (a second token, a voucher id) is masked too.
        let other = "fedcba9876543210fedcba9876543210";
        let c = failure_check("measure", &format!("bad {other}"), TOKEN);
        assert!(!c.detail.contains(other) && c.detail.contains("<hex>"), "{}", c.detail);
        let c = connect_failure_check("http://127.0.0.1:9222", &format!("refused {TOKEN}"), TOKEN);
        assert_eq!((c.level, c.next), (Level::Fail, Some(Next::Chrome)));
        assert!(!c.detail.contains(TOKEN) && c.detail.contains("127.0.0.1:9222"), "{}", c.detail);
        // Page-derived details (a src or quoted text holding the token) are scrubbed.
        let m = Metrics { broken_imgs: vec![format!("/x?token={TOKEN}")], ..good() };
        for c in render_checks(&m) {
            assert!(!scrub(&c.detail, TOKEN).contains(TOKEN));
        }
    }

    #[test]
    fn page_text_is_scrubbed_before_a_verdict_truncates_it() {
        // A 20-char token fragment (the JS sliced the text) is masked; the full token
        // becomes its fingerprint; short hex and ordinary text survive.
        let frag = &TOKEN[..20];
        assert_eq!(scrub_page_text(&format!("ref {frag}"), TOKEN), "ref <hex>");
        let full = scrub_page_text(&format!("x {TOKEN} y"), TOKEN);
        assert!(!full.contains(TOKEN) && full.contains("012345...abcdef"), "{full}");
        assert_eq!(mask_hex_page("#c0ffee 第 1 天"), "#c0ffee 第 1 天");
        assert_eq!(mask_hex_page("0123456789abcde"), "0123456789abcde");
        assert_eq!(mask_hex_page("0123456789abcdef"), "<hex>");
        // Why before: unscrubbed, the 20-char snippet keeps a 16-hex fragment that the
        // final 32-hex scrub cannot see.
        let mut m = good();
        m.text_blocks.push(block("li", &format!("{frag}{frag}"), 4.0, 100.0));
        m.doc_links.push(Shown { label: format!("{TOKEN}"), state: "zero-size".into(), open: None });
        m.broken_imgs.push(format!("/v?t={frag}"));
        let raw: String = render_checks(&m).iter().map(|c| scrub(&c.detail, TOKEN)).collect();
        assert!(raw.contains(&TOKEN[..16]), "the hazard this guards: {raw}");
        let m = scrub_metrics(m, TOKEN);
        let details: String = render_checks(&m).iter().map(|c| scrub(&c.detail, TOKEN)).collect();
        assert!(!details.contains(&TOKEN[..16]) && !details.contains(&TOKEN[16..]), "{details}");
        assert!(details.contains("li \"<hex>\" left=4") && details.contains("/v?t=<hex>"), "{details}");
        // Every page-derived field is covered.
        let m = Metrics {
            broken_imgs: vec![TOKEN.into()],
            doc_links: vec![shown(TOKEN)],
            variants: vec![shown(TOKEN)],
            map_imgs: vec![shown(TOKEN)],
            text_blocks: vec![block(TOKEN, TOKEN, 20.0, 50.0)],
            fonts_status: TOKEN.into(),
            ..good()
        };
        let m = scrub_metrics(m, TOKEN);
        let all = format!("{m:?}");
        assert!(!all.contains(&TOKEN[..16]) && !all.contains(&TOKEN[16..]), "{all}");
    }

    #[test]
    fn metrics_decode_from_the_js_shape_and_reject_missing_keys() {
        let v = serde_json::json!({
            "innerWidth": 390, "clientWidth": 375, "scrollWidth": 375, "dayCards": 2, "imgs": 3,
            "brokenImgs": [], "pendingImgs": 0,
            "docLinks": [{"label": "T", "state": ""}],
            "variants": [{"label": "V", "state": "", "open": true}],
            "mapImgs": [{"label": "day-1.png", "state": ""}],
            "textBlocks": [{"element": "h2", "text": "Day 1", "left": 20.5, "right": 99, "refX": 20.5,
                            "cardRight": null, "ellipsis": false}],
            "fontsStatus": "loaded"
        });
        let m: Metrics = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(m.variants[0].open, Some(true));
        assert_eq!(m.doc_links[0].open, None);
        assert_eq!(m.text_blocks[0].card_right, None);
        assert_eq!(render_checks(&m)[0].level, Level::Pass);
        // Every key the struct reads is one the JS returns.
        for key in v.as_object().unwrap().keys() {
            assert!(METRICS_JS.contains(&format!("{key}:")), "METRICS_JS lacks {key}");
        }
        for key in ["refX", "cardRight", "ellipsis", "label", "state", "open"] {
            assert!(METRICS_JS.contains(&format!("{key}:")), "METRICS_JS lacks {key}");
        }
        let mut missing = v;
        missing.as_object_mut().unwrap().remove("textBlocks");
        assert!(serde_json::from_value::<Metrics>(missing).is_err(), "a missing key must not decode as empty");
    }

    #[test]
    fn shot_rects_decode_from_the_js_shape() {
        let v = serde_json::json!([{"x": 0, "y": 12.5, "width": 390, "height": 700}]);
        let r: Vec<ShotRect> = serde_json::from_value(v).unwrap();
        assert_eq!(r, vec![ShotRect { x: 0.0, y: 12.5, width: 390.0, height: 700.0 }]);
        for key in ["x", "y", "width", "height"] {
            assert!(SHOT_RECTS_JS.contains(&format!("{key}:")), "SHOT_RECTS_JS lacks {key}");
        }
        assert!(SHOT_RECTS_JS.contains("section.booking-summary, section.plan-documents, section.day"));
        assert!(serde_json::from_value::<Vec<ShotRect>>(serde_json::json!([{"x": 0, "y": 1, "width": 2}])).is_err());
    }

    #[test]
    fn shot_viewport_is_the_page_height_within_bounds() {
        assert_eq!(shot_viewport_height(5230.4, 844), 5231);
        assert_eq!(shot_viewport_height(600.0, 844), 844);
        assert_eq!(shot_viewport_height(90_000.0, 900), MAX_SHOT_VIEWPORT_HEIGHT);
        assert_eq!(shot_viewport_height(f64::NAN, 900), 900);
    }

    #[test]
    fn stale_shot_removal_failures_are_reported() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("zz_test_{nanos}_render_shots"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("zh-mobile-01.png"), b"old").unwrap();
        fs::write(dir.join("kyoto.png"), b"keep").unwrap();
        // remove_file on a directory fails: a removal error must surface, not vanish.
        fs::create_dir(dir.join("en-desktop-02.png")).unwrap();
        let warnings = prepare_shot_dir(&dir);
        let gone = !dir.join("zh-mobile-01.png").exists();
        let kept = dir.join("kyoto.png").exists();
        let _ = fs::remove_dir_all(&dir);
        let warnings = warnings.unwrap();
        assert!(gone && kept);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("could not remove stale en-desktop-02.png: "), "{}", warnings[0]);
    }

    #[test]
    fn shot_files_are_named_and_recognised() {
        assert_eq!(shot_name("zh", "mobile", 3), "zh-mobile-03.png");
        assert_eq!(shot_name("en", "desktop", 12), "en-desktop-12.png");
        assert!(is_own_shot("zh-mobile-03.png") && is_own_shot("en-desktop-112.png"));
        assert!(!is_own_shot("kyoto.png") && !is_own_shot("zh-mobile-3.png") && !is_own_shot("fr-mobile-01.png"));
    }
}
