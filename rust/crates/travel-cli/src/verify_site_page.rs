//! Layer 1 of `travel verify-site` (docs/plans/2026-10-30-verify-site.md, phase
//! 2): what the token VIEWER page actually shows. Pure — HTML in, findings out;
//! every judgement here is unit-tested from worker-shaped HTML. The fetching
//! lives in `verify_site.rs`.
//!
//! The page is parsed with a real HTML parser and every element query runs
//! inside `<body>`: each page inlines the whole stylesheet, so a string search
//! for a class name would hit the CSS. Only `<html lang>` and the notranslate
//! `<meta>` are read outside the body (that is where they live).

use std::collections::HashMap;

use scraper::{ElementRef, Html, Selector};
use url::Url;

use crate::verify_site::{Check, Next};

/// Owner-only controls a viewer page must never carry (worker render/share.rs).
const OWNER_SELECTORS: [&str; 5] = [
    ".owner-chrome",
    ".grant-manager",
    ".grant-form",
    ".copy-share-btn",
    "input[type=\"checkbox\"][name=\"token\"]",
];

fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap_or_else(|e| panic!("static selector {s:?} does not parse: {e:?}"))
}

fn text_of(el: ElementRef) -> String {
    el.text().collect::<String>()
}

fn has_class(el: ElementRef, class: &str) -> bool {
    el.value().classes().any(|c| c == class)
}

/// One `<li>` of a map legend as the page renders it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LegendItem {
    /// `span.leg-num` text (the number drawn on the pin).
    pub(crate) number: String,
    /// The link text (the place name).
    pub(crate) name: String,
    pub(crate) href: Option<String>,
}

/// One `figure.map-frame` on the page.
#[derive(Clone, Debug, Default)]
pub(crate) struct MapFigure {
    pub(crate) key: Option<String>,
    /// `src` of the figure's `<img>`; None for the missing-map frame.
    pub(crate) img_src: Option<String>,
    pub(crate) missing_frame: bool,
    pub(crate) legend: Vec<LegendItem>,
    /// `div.map-legend-day` blocks (a legend grouped by day).
    pub(crate) legend_day_blocks: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Companion {
    pub(crate) cards: usize,
    /// Options whose `div.companion-stops` holds no `span.stop-chip`.
    pub(crate) stopless_options: usize,
    pub(crate) img_srcs: Vec<String>,
}

/// Everything layer 1 reads from one page.
#[derive(Clone, Debug, Default)]
pub(crate) struct PageDoc {
    pub(crate) html_lang: Option<String>,
    pub(crate) notranslate: bool,
    pub(crate) booking_summary: bool,
    /// Index into `figures` of the overview map: the first map figure, before
    /// the booking summary, that is not an inset.
    pub(crate) overview: Option<usize>,
    pub(crate) auth_page: bool,
    /// Which OWNER_SELECTORS (or a `/grants/*` form) matched.
    pub(crate) owner_markers: Vec<String>,
    pub(crate) plan_id_attr: Option<String>,
    pub(crate) view_attr: Option<String>,
    pub(crate) figures: Vec<MapFigure>,
    pub(crate) companion: Option<Companion>,
    pub(crate) stay_photos: Vec<String>,
    pub(crate) stay_minimaps: Vec<String>,
    pub(crate) voucher_hrefs: Vec<String>,
}

pub(crate) fn parse_page(html: &str) -> PageDoc {
    let doc = Html::parse_document(html);
    let mut out = PageDoc {
        html_lang: doc.root_element().value().attr("lang").map(str::to_string),
        notranslate: doc.select(&sel("head meta[name=\"google\"]")).any(|m| {
            m.value()
                .attr("content")
                .is_some_and(|c| c.split(',').any(|t| t.trim().eq_ignore_ascii_case("notranslate")))
        }),
        ..PageDoc::default()
    };
    let Some(body) = doc.select(&sel("body")).next() else {
        return out;
    };

    out.auth_page = body.select(&sel(".auth-page")).next().is_some();
    for s in OWNER_SELECTORS {
        if body.select(&sel(s)).next().is_some() {
            out.owner_markers.push(s.to_string());
        }
    }
    let base = Url::parse("https://dashboard.invalid/").expect("static base url");
    if body.select(&sel("form")).any(|f| {
        f.value()
            .attr("action")
            .and_then(|a| base.join(a).ok())
            .is_some_and(|u| u.path().starts_with("/grants/"))
    }) {
        out.owner_markers.push("form[action=/grants/*]".to_string());
    }

    // The page identity marker: on <body> itself, or on the first wrapper below it.
    let marker = if body.value().attr("data-plan-id").is_some() {
        Some(body)
    } else {
        body.select(&sel("[data-plan-id]")).next()
    };
    if let Some(m) = marker {
        out.plan_id_attr = m.value().attr("data-plan-id").map(str::to_string);
        out.view_attr = m.value().attr("data-view").map(str::to_string);
    }

    let legend_li = sel("ol.map-legend > li");
    let leg_num = sel("span.leg-num");
    let link = sel("a");
    let img = sel("img");
    let day_block = sel("div.map-legend-day");
    let mut seen_summary = false;
    for el in body.select(&sel("figure.map-frame, section.booking-summary")) {
        if el.value().name() == "section" {
            out.booking_summary |= has_class(el, "summary-box");
            seen_summary = true;
            continue;
        }
        let legend = el
            .select(&legend_li)
            .map(|li| {
                let a = li.select(&link).next();
                LegendItem {
                    number: li.select(&leg_num).next().map(text_of).unwrap_or_default().trim().to_string(),
                    name: a.map(text_of).unwrap_or_default(),
                    href: a.and_then(|a| a.value().attr("href")).map(str::to_string),
                }
            })
            .collect();
        if !seen_summary && out.overview.is_none() && out.figures.is_empty() && !has_class(el, "map-frame--inset") {
            out.overview = Some(out.figures.len());
        }
        out.figures.push(MapFigure {
            key: el.value().attr("data-map-key").map(str::to_string),
            img_src: el.select(&img).next().and_then(|i| i.value().attr("src")).map(str::to_string),
            missing_frame: has_class(el, "map-missing"),
            legend,
            legend_day_blocks: el.select(&day_block).count(),
        });
    }

    if let Some(section) = body.select(&sel("section.companion-options")).next() {
        out.companion = Some(Companion {
            cards: section.select(&sel("article.companion-card")).count(),
            stopless_options: section
                .select(&sel("div.companion-stops"))
                .filter(|d| d.select(&sel("span.stop-chip")).next().is_none())
                .count(),
            img_srcs: srcs(section, "img.companion-map-img"),
        });
    }
    out.stay_photos = srcs(body, ".booking-cand-detail img.candidate-gallery-img");
    out.stay_minimaps = srcs(body, ".booking-cand-detail img.cand-minimap-img");
    out.voucher_hrefs = body
        .select(&sel("a.voucher-link"))
        .filter_map(|a| a.value().attr("href").map(str::to_string))
        .collect();
    out
}

fn srcs(scope: ElementRef, selector: &str) -> Vec<String> {
    scope
        .select(&sel(selector))
        .filter_map(|i| i.value().attr("src").map(str::to_string))
        .collect()
}

/// The page language a fetch asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lang {
    Zh,
    En,
}

impl Lang {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }
    fn html_lang(self) -> &'static str {
        match self {
            Lang::Zh => "zh-TW",
            Lang::En => "en",
        }
    }
}

/// True when the page is the plan page at all (200, not an auth page): the
/// map, legend and extended checks only make sense then.
pub(crate) fn page_usable(http: u16, doc: &PageDoc) -> bool {
    http == 200 && !doc.auth_page
}

/// Page-level checks for one viewer page: (layer, check) pairs. When the page
/// is not usable only the `page` verdict is returned.
pub(crate) fn page_checks(http: u16, doc: &PageDoc, lang: Lang, plan_id: &str) -> Vec<(&'static str, Check)> {
    let mut out = Vec::new();
    if doc.auth_page {
        out.push((
            "page",
            Check::fail(
                format!(
                    "HTTP {http}: an auth page (sign-in / no access / bad link) instead of the plan — the verify token \
                     cannot view this plan"
                ),
                Some(Next::RotateToken),
            ),
        ));
        return out;
    }
    if http != 200 {
        out.push(("page", Check::fail(format!("HTTP {http} (expected 200)"), None)));
        return out;
    }
    let mut missing = Vec::new();
    if !doc.booking_summary {
        missing.push("section.booking-summary.summary-box");
    }
    if doc.overview.is_none() {
        missing.push("the overview map figure before the booking summary");
    }
    out.push((
        "page",
        if missing.is_empty() {
            Check::pass("200, booking summary + overview map, no auth page")
        } else {
            Check::fail(format!("200 but missing {}", missing.join(" and ")), None)
        },
    ));

    let want = lang.html_lang();
    let mut lang_problems = Vec::new();
    if doc.html_lang.as_deref() != Some(want) {
        lang_problems.push(format!(
            "<html lang={}> (expected {want})",
            doc.html_lang.as_deref().unwrap_or("none")
        ));
    }
    if !doc.notranslate {
        lang_problems.push("no <meta name=google content=notranslate>".to_string());
    }
    out.push((
        "lang",
        if lang_problems.is_empty() {
            Check::pass(format!("<html lang={want}>, notranslate meta"))
        } else {
            Check::fail(lang_problems.join("; "), None)
        },
    ));

    out.push((
        "owner",
        if doc.owner_markers.is_empty() {
            Check::pass("no owner chrome on the viewer page")
        } else {
            Check::fail(
                format!("owner-only controls on the token viewer page: {}", doc.owner_markers.join(", ")),
                None,
            )
        },
    ));

    out.push((
        "ident",
        match (doc.plan_id_attr.as_deref(), doc.view_attr.as_deref()) {
            (None, _) | (_, None) => Check::fail(
                "no data-plan-id / data-view marker on the page (live worker predates the verify-site markup)",
                Some(Next::Deploy),
            ),
            (Some(id), Some(view)) => {
                let mut wrong = Vec::new();
                if id != plan_id {
                    wrong.push(format!("data-plan-id={id:?} (expected {plan_id:?})"));
                }
                if view != "viewer" {
                    wrong.push(format!("data-view={view:?} (expected \"viewer\")"));
                }
                if wrong.is_empty() {
                    Check::pass(format!("data-plan-id={id}, data-view=viewer"))
                } else {
                    Check::fail(wrong.join("; "), None)
                }
            }
        },
    ));
    out
}

/// What the plan's data says the page should hold.
pub(crate) struct SlotFacts<'a> {
    pub(crate) day_numbers: &'a [i64],
    pub(crate) has_flights: bool,
    /// map_artifacts status per key.
    pub(crate) statuses: &'a HashMap<String, String>,
}

/// Which slot a key is, by the worker's own presence rules (router.rs
/// map_status_keys + render/mod.rs show_logistics).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotRule {
    /// Always one slot: the map or the missing-map frame (plan.png, day-N.png).
    Always,
    /// The map, or the missing-map frame when the plan has flights.
    Logistics,
    /// Only an uploaded map renders; never a missing-map frame.
    OnlyWithMap,
}

fn slot_keys(day_numbers: &[i64]) -> Vec<(String, SlotRule)> {
    let mut keys = vec![
        ("plan.png".to_string(), SlotRule::Always),
        ("plan-logistics.png".to_string(), SlotRule::Logistics),
        ("plan-excursion.png".to_string(), SlotRule::OnlyWithMap),
        ("plan-airport.png".to_string(), SlotRule::OnlyWithMap),
    ];
    let mut days = day_numbers.to_vec();
    days.sort_unstable();
    days.dedup();
    for n in days {
        keys.push((format!("day-{n}.png"), SlotRule::Always));
        keys.push((format!("day-{n}-logistics.png"), SlotRule::OnlyWithMap));
        keys.push((format!("day-{n}-away.png"), SlotRule::OnlyWithMap));
    }
    keys
}

/// Map slots on the page against map_artifacts and the worker's presence
/// rules: (key, check) pairs. A figure without data-map-key cannot be pinned
/// to a key; when no figure has one the page predates the worker markup and a
/// single FAIL replaces the per-key checks.
pub(crate) fn slot_checks(figures: &[MapFigure], facts: &SlotFacts) -> Vec<(String, Check)> {
    let mut out = Vec::new();
    let unkeyed = figures.iter().filter(|f| f.key.is_none()).count();
    if unkeyed > 0 {
        out.push((
            "*".to_string(),
            Check::fail(
                format!(
                    "{unkeyed} of {} map figure(s) carry no data-map-key (live worker predates the verify-site markup) \
                     — slots cannot be matched to map_artifacts",
                    figures.len()
                ),
                Some(Next::Deploy),
            ),
        ));
        if unkeyed == figures.len() {
            return out;
        }
    }
    let mut by_key: HashMap<&str, Vec<&MapFigure>> = HashMap::new();
    for f in figures {
        if let Some(k) = f.key.as_deref() {
            by_key.entry(k).or_default().push(f);
        }
    }
    let known = slot_keys(facts.day_numbers);
    let mut unexpected: Vec<&str> = by_key
        .keys()
        .copied()
        .filter(|k| !known.iter().any(|(kk, _)| kk == k))
        .collect();
    unexpected.sort_unstable();
    for k in unexpected {
        out.push((
            k.to_string(),
            Check::fail("a map slot the plan's days and the worker's rules do not produce", None),
        ));
    }

    for (key, rule) in &known {
        let status = facts.statuses.get(key).map(String::as_str);
        let figs = by_key.get(key.as_str()).map(Vec::as_slice).unwrap_or(&[]);
        if figs.len() > 1 {
            out.push((key.clone(), Check::fail(format!("{} slots on the page (expected one)", figs.len()), None)));
            continue;
        }
        let fig = figs.first();
        let has_img = fig.is_some_and(|f| f.img_src.is_some());
        if fig.is_some_and(|f| !has_img && !f.missing_frame) {
            out.push((key.clone(), Check::fail("a map figure with neither an <img> nor the missing-map frame", None)));
            continue;
        }
        let status_txt = status.map_or("no map_artifacts row".to_string(), |s| format!("status={s}"));
        if status == Some("uploaded") {
            out.push((
                key.clone(),
                match fig {
                    None => Check::fail("uploaded map has no slot on the page", Some(Next::Snapshot)),
                    Some(_) if !has_img => Check::fail(
                        "uploaded map shows the missing-map frame (the worker found no usable R2 object)",
                        Some(Next::Snapshot),
                    ),
                    Some(_) => Check::pass("uploaded → <img>"),
                },
            ));
            continue;
        }
        if has_img {
            // skipped or no row: snapshot-maps will not touch this key again, so only
            // deleting the lingering R2 object fixes it; failed: a re-render can.
            let next = match status {
                Some("skipped") | None => Next::DeleteStale,
                _ => Next::Snapshot,
            };
            out.push((
                key.clone(),
                Check::fail(
                    format!("{status_txt} but the page shows an <img> (an R2 object the manifest says must not exist)"),
                    Some(next),
                ),
            ));
            continue;
        }
        // Not uploaded and no <img>: is the missing-map frame where the worker puts one?
        let frame_expected = match rule {
            SlotRule::Always => true,
            SlotRule::Logistics => facts.has_flights,
            SlotRule::OnlyWithMap => false,
        };
        match (fig.is_some(), frame_expected) {
            (true, true) => out.push((key.clone(), Check::pass(format!("{status_txt} → missing-map frame")))),
            (false, true) => out.push((
                key.clone(),
                Check::fail(format!("{status_txt} and no slot on the page (expected the missing-map frame)"), None),
            )),
            (true, false) => out.push((
                key.clone(),
                Check::fail(
                    format!("{status_txt}: a missing-map frame where the worker renders nothing without a map"),
                    None,
                ),
            )),
            // Absent and not expected: nothing on the page, nothing to report.
            (false, false) => {}
        }
    }
    out
}

/// One `map_legend_stops` row.
#[derive(Clone, Debug, Default)]
pub(crate) struct LegendRow {
    pub(crate) seq: i64,
    pub(crate) label: Option<String>,
    pub(crate) lat: Option<f64>,
    pub(crate) lon: Option<f64>,
    pub(crate) day_number: Option<i64>,
    pub(crate) pin_no: Option<i64>,
}

/// The rows in the order the worker renders them (render/map.rs
/// map_legend_html): by seq when no row has a day; otherwise each day's rows in
/// first-appearance order of the day, then the day-less rows. Also returns the
/// number of day blocks.
fn render_order(rows: &[LegendRow]) -> (Vec<&LegendRow>, usize) {
    let mut sorted: Vec<&LegendRow> = rows.iter().collect();
    sorted.sort_by_key(|r| r.seq);
    if sorted.iter().all(|r| r.day_number.is_none()) {
        return (sorted, 0);
    }
    let mut days: Vec<i64> = Vec::new();
    for d in sorted.iter().filter_map(|r| r.day_number) {
        if !days.contains(&d) {
            days.push(d);
        }
    }
    let mut out: Vec<&LegendRow> = Vec::new();
    for d in &days {
        out.extend(sorted.iter().copied().filter(|r| r.day_number == Some(*d)));
    }
    out.extend(sorted.iter().copied().filter(|r| r.day_number.is_none()));
    (out, days.len())
}

/// (lat, lon) of a legend link `https://www.google.com/maps?q=<lat>,<lon>`.
fn link_coords(href: &str) -> Option<(f64, f64)> {
    let u = Url::parse(href).ok()?;
    if u.host_str() != Some("www.google.com") || u.path() != "/maps" {
        return None;
    }
    let q = u.query_pairs().find(|(k, _)| k == "q")?.1.into_owned();
    let (la, lo) = q.split_once(',')?;
    Some((la.trim().parse().ok()?, lo.trim().parse().ok()?))
}

fn fmt_row(number: &str, name: &str, coords: Option<(f64, f64)>) -> String {
    match coords {
        Some((la, lo)) => format!("#{number} {name:?} ({la},{lo})"),
        None => format!("#{number} {name:?} (no coordinate link)"),
    }
}

/// One figure's legend against its `map_legend_stops` rows, row by row:
/// number (pin_no, else seq), name and coordinate link. The worker renders a
/// legend only under a present map, so a missing-map frame expects none. Pure.
pub(crate) fn legend_check(fig: &MapFigure, rows: &[LegendRow]) -> Check {
    let shown = &fig.legend;
    if fig.img_src.is_none() {
        return if shown.is_empty() {
            Check::pass("missing-map frame, no legend")
        } else {
            Check::fail(format!("missing-map frame lists {} legend row(s)", shown.len()), None)
        };
    }
    let (expected, day_blocks) = render_order(rows);
    if shown.len() != expected.len() {
        return Check::fail(
            format!(
                "page lists {} legend row(s), map_legend_stops has {} (the worker drops rows without label/lat/lon)",
                shown.len(),
                expected.len()
            ),
            Some(Next::Snapshot),
        );
    }
    for (i, (li, row)) in shown.iter().zip(&expected).enumerate() {
        let number = row.pin_no.unwrap_or(row.seq).to_string();
        let name = row.label.clone().unwrap_or_default();
        let want = row.lat.zip(row.lon);
        let got = li.href.as_deref().and_then(link_coords);
        if li.number != number || li.name != name || want.is_none() || got != want {
            return Check::fail(
                format!(
                    "row {}: page {} vs map_legend_stops seq {} {}",
                    i + 1,
                    fmt_row(&li.number, &li.name, got),
                    row.seq,
                    fmt_row(&number, &name, want)
                ),
                Some(Next::Snapshot),
            );
        }
    }
    if fig.legend_day_blocks != day_blocks {
        return Check::fail(
            format!("{} day block(s) on the page, map_legend_stops has {day_blocks} day(s)", fig.legend_day_blocks),
            Some(Next::Snapshot),
        );
    }
    if expected.is_empty() {
        Check::pass("no legend rows (none recorded)")
    } else if day_blocks > 0 {
        Check::pass(format!("{} rows match map_legend_stops ({day_blocks} day block(s))", expected.len()))
    } else {
        Check::pass(format!("{} rows match map_legend_stops", expected.len()))
    }
}

/// Parse a map `<img src>` (any attribute order, `?v=` optional) and check it
/// names this figure's map. Ok(Some(url)) = the exact versioned URL a viewer
/// loads (to fetch once); Ok(None) = no `?v=` on it.
pub(crate) fn img_src_check(base: &Url, plan_id: &str, key: &str, src: &str) -> Result<Option<Url>, Check> {
    let url = base
        .join(src)
        .map_err(|e| Check::fail(format!("<img src> does not parse as a URL ({e})"), None))?;
    let want = format!("/map/{plan_id}/{key}");
    if url.host_str() != base.host_str() || url.path() != want {
        return Err(Check::fail(
            format!("<img> loads {}{} (expected {want} on the dashboard host)", url.host_str().unwrap_or(""), url.path()),
            None,
        ));
    }
    Ok(url.query_pairs().any(|(k, v)| k == "v" && !v.is_empty()).then_some(url))
}

/// An external image (companion map, stay photo, location minimap) as a
/// browser would load it. Only an answer that is broken for every viewer FAILs
/// (404/410, or an empty 200); a refusal or error that may only hit a
/// non-browser fetch (401/403/429/5xx, transport) is a WARN.
pub(crate) fn external_image_verdict(http: u16, content_type: &str, len: usize) -> Check {
    let is_image = content_type
        .split(';')
        .next()
        .is_some_and(|t| t.trim().to_ascii_lowercase().starts_with("image/"));
    match http {
        200 if len == 0 => Check::fail("HTTP 200 with an empty body (a broken image)", None),
        200 if is_image => Check::pass(format!("{len} bytes {}", content_type.split(';').next().unwrap_or("").trim())),
        200 => Check::warn(format!("HTTP 200 but content-type {content_type:?}, not an image"), None),
        404 | 410 => Check::fail(format!("HTTP {http}: the image is gone (broken for every viewer)"), None),
        _ => Check::warn(
            format!("HTTP {http} (may only refuse a non-browser fetch — open the page in a browser to confirm)"),
            None,
        ),
    }
}

/// Resolve a voucher link: Ok(url) for a `/voucher/*` link that carries the
/// page's own token; Err(check) otherwise. Never puts the URL or token in the
/// check text.
pub(crate) fn voucher_target(base: &Url, href: &str, token: &str) -> Result<Url, Check> {
    let url = base
        .join(href)
        .map_err(|e| Check::fail(format!("voucher href does not parse as a URL ({e})"), None))?;
    if url.host_str() != base.host_str() || !url.path().starts_with("/voucher/") {
        return Err(Check::warn("voucher link points off the dashboard's /voucher/ route; not checked", None));
    }
    if !url.query_pairs().any(|(k, v)| k == "token" && v == token) {
        return Err(Check::fail(
            "voucher link does not carry the page's token (a viewer's click would get 403)",
            None,
        ));
    }
    Ok(url)
}

/// The voucher PDF fetched with the same token as the page.
pub(crate) fn voucher_verdict(http: u16, body: &[u8]) -> Check {
    match http {
        200 if body.starts_with(b"%PDF") => Check::pass(format!("PDF, {} bytes", body.len())),
        200 => Check::fail(format!("HTTP 200 but the {}-byte body is not a PDF", body.len()), None),
        403 => Check::fail("HTTP 403: the token has no permission for this plan's voucher", Some(Next::RotateToken)),
        404 => Check::fail("HTTP 404: the voucher PDF is missing in R2 (VOUCHERS bucket)", None),
        other => Check::fail(format!("HTTP {other} (expected 200 application/pdf)"), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify_site::Level;

    /// Worker-shaped page shell: the inlined stylesheet names every class the
    /// checks look for, so a string search would "find" them all.
    fn page(lang: &str, body_attrs: &str, body: &str) -> String {
        format!(
            "<!doctype html><html lang=\"{lang}\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
             <meta name=\"google\" content=\"notranslate\">\
             <title>T</title><style>.owner-chrome{{}} .grant-manager{{}} .grant-form{{}} .copy-share-btn{{}} \
             .auth-page{{}} .booking-summary.summary-box{{}} figure.map-frame{{}} ol.map-legend{{}}</style></head>\
             <body{body_attrs}>{body}</body></html>"
        )
    }

    const LEGEND_FLAT: &str = "<ol class=\"map-legend\"><li><span class=\"leg-num\">1</span><a href=\"https://www.google.com/maps?q=35.0394,135.7292\" target=\"_blank\" rel=\"noopener\">金閣寺</a></li><li><span class=\"leg-num\">2</span><a href=\"https://www.google.com/maps?q=35.0117,135.7409\" target=\"_blank\" rel=\"noopener\">北野天滿宮 &amp; 梅</a></li></ol>";

    fn viewer_body(extra: &str) -> String {
        format!(
            "<figure class=\"map-frame\" data-map-key=\"plan.png\"><img class=\"planmap\" alt=\"景點總覽\" src=\"/map/p-1/plan.png?v=abc123\"><figcaption>景點總覽</figcaption>{LEGEND_FLAT}</figure>\
             <div class=\"map-insets\"><figure class=\"map-frame map-frame--inset map-missing\" data-map-key=\"plan-logistics.png\"><div class=\"map-missing-box\">地圖尚未產生</div><figcaption>機場</figcaption></figure></div>\
             <section class=\"booking-summary summary-box\"><h2>住宿</h2></section>\
             <section class=\"day day-arrival\"><div class=\"day-map-stack\"><figure class=\"map-frame map-missing\" data-map-key=\"day-1.png\"><div class=\"map-missing-box\">地圖尚未產生</div><figcaption>第 1 天路線</figcaption></figure></div></section>{extra}"
        )
    }

    fn viewer_page(extra: &str) -> String {
        page("zh-TW", " data-plan-id=\"p-1\" data-view=\"viewer\"", &viewer_body(extra))
    }

    #[test]
    fn a_good_viewer_page_passes_every_page_check() {
        let doc = parse_page(&viewer_page(""));
        let checks = page_checks(200, &doc, Lang::Zh, "p-1");
        let layers: Vec<&str> = checks.iter().map(|(l, _)| *l).collect();
        assert_eq!(layers, ["page", "lang", "owner", "ident"]);
        for (l, c) in &checks {
            assert_eq!(c.level, Level::Pass, "{l}: {c:?}");
        }
        assert_eq!(doc.overview, Some(0));
        assert_eq!(doc.figures.len(), 3);
    }

    #[test]
    fn inlined_css_class_names_do_not_count_as_elements() {
        // The <style> names .owner-chrome, .auth-page, … — none is an element.
        let doc = parse_page(&viewer_page(""));
        assert!(doc.owner_markers.is_empty(), "{:?}", doc.owner_markers);
        assert!(!doc.auth_page);
        // And a body with nothing but text naming them is still clean.
        let doc = parse_page(&page("zh-TW", "", "<p>.owner-chrome .auth-page booking-summary</p>"));
        assert!(doc.owner_markers.is_empty() && !doc.auth_page && !doc.booking_summary);
    }

    #[test]
    fn auth_page_and_non_200_fail_and_skip_the_rest() {
        let bad = page(
            "zh-TW",
            "",
            "<div class=\"auth-page\"><h1>連結無效</h1><p>…</p><p class=\"auth-hint\"><a href=\"/auth/login\">登入</a></p></div>",
        );
        let doc = parse_page(&bad);
        assert!(!page_usable(403, &doc));
        let checks = page_checks(403, &doc, Lang::Zh, "p-1");
        assert_eq!(checks.len(), 1);
        assert_eq!((checks[0].1.level, checks[0].1.next), (Level::Fail, Some(Next::RotateToken)));
        assert!(checks[0].1.detail.contains("auth page"), "{checks:?}");

        let doc = parse_page(&viewer_page(""));
        let checks = page_checks(500, &doc, Lang::Zh, "p-1");
        assert_eq!(checks.len(), 1);
        assert!(checks[0].1.detail.contains("HTTP 500"));
    }

    #[test]
    fn missing_summary_or_overview_fails_page_correct() {
        let doc = parse_page(&page("zh-TW", " data-plan-id=\"p-1\" data-view=\"viewer\"", "<p>empty</p>"));
        let (_, c) = &page_checks(200, &doc, Lang::Zh, "p-1")[0];
        assert_eq!(c.level, Level::Fail);
        assert!(c.detail.contains("booking-summary") && c.detail.contains("overview"), "{c:?}");
        // An inset alone before the summary is not the overview.
        let doc = parse_page(&page(
            "zh-TW",
            "",
            "<figure class=\"map-frame map-frame--inset\" data-map-key=\"plan-logistics.png\"><img src=\"/map/p-1/plan-logistics.png\"></figure>\
             <section class=\"booking-summary summary-box\"></section>",
        ));
        assert_eq!(doc.overview, None);
    }

    #[test]
    fn language_checks_html_lang_and_notranslate() {
        let doc = parse_page(&viewer_page(""));
        let lang = |doc: &PageDoc, l| page_checks(200, doc, l, "p-1").into_iter().find(|(x, _)| *x == "lang").unwrap().1;
        assert_eq!(lang(&doc, Lang::Zh).level, Level::Pass);
        let en = lang(&doc, Lang::En);
        assert_eq!(en.level, Level::Fail);
        assert!(en.detail.contains("expected en"), "{en:?}");
        let en_page = parse_page(&page("en", " data-plan-id=\"p-1\" data-view=\"viewer\"", &viewer_body("")));
        assert_eq!(lang(&en_page, Lang::En).level, Level::Pass);
        let no_meta = viewer_page("").replace("<meta name=\"google\" content=\"notranslate\">", "");
        let c = lang(&parse_page(&no_meta), Lang::Zh);
        assert!(c.level == Level::Fail && c.detail.contains("notranslate"), "{c:?}");
    }

    #[test]
    fn owner_chrome_on_a_viewer_page_fails() {
        let owner = "<div class=\"owner-chrome\"><span class=\"owner-chrome-user\">signed in</span></div>\
            <details class=\"grant-manager\"><summary>links</summary><form method=\"post\" action=\"/grants/deactivate-batch\">\
            <input type=\"checkbox\" name=\"token\" value=\"x\"></form></details>\
            <form class=\"grant-form\" method=\"post\" action=\"/grants/create\"></form>\
            <button type=\"button\" class=\"copy-share-btn\" data-copy-url=\"u\">copy</button>";
        let doc = parse_page(&viewer_page(owner));
        let c = page_checks(200, &doc, Lang::Zh, "p-1").into_iter().find(|(l, _)| *l == "owner").unwrap().1;
        assert_eq!(c.level, Level::Fail);
        for m in [".owner-chrome", ".grant-manager", ".grant-form", ".copy-share-btn", "name=\"token\"", "/grants/*"] {
            assert!(c.detail.contains(m), "{m}: {c:?}");
        }
        // An absolute /grants/ action is caught too.
        let doc = parse_page(&viewer_page("<form action=\"https://x.example/grants/create\"></form>"));
        assert_eq!(doc.owner_markers, ["form[action=/grants/*]"]);
    }

    #[test]
    fn identity_marker_must_name_the_plan_and_the_viewer_view() {
        let ident = |attrs: &str| {
            let doc = parse_page(&page("zh-TW", attrs, &viewer_body("")));
            page_checks(200, &doc, Lang::Zh, "p-1").into_iter().find(|(l, _)| *l == "ident").unwrap().1
        };
        assert_eq!(ident(" data-plan-id=\"p-1\" data-view=\"viewer\"").level, Level::Pass);
        let c = ident("");
        assert_eq!((c.level, c.next), (Level::Fail, Some(Next::Deploy)));
        let c = ident(" data-plan-id=\"p-2\" data-view=\"owner\"");
        assert!(c.level == Level::Fail && c.detail.contains("p-2") && c.detail.contains("owner"), "{c:?}");
        // A wrapper below <body> carries it as well.
        let doc = parse_page(&page("zh-TW", "", "<div data-plan-id=\"p-1\" data-view=\"viewer\"></div>"));
        assert_eq!(doc.plan_id_attr.as_deref(), Some("p-1"));
    }

    fn statuses(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn fig(key: &str, img: bool) -> MapFigure {
        MapFigure {
            key: Some(key.into()),
            img_src: img.then(|| format!("/map/p-1/{key}?v=e1")),
            missing_frame: !img,
            ..MapFigure::default()
        }
    }

    fn levels_by_key(out: &[(String, Check)]) -> Vec<(String, Level)> {
        out.iter().map(|(k, c)| (k.clone(), c.level)).collect()
    }

    #[test]
    fn slots_follow_the_manifest_and_the_worker_presence_rules() {
        let st = statuses(&[
            ("plan.png", "uploaded"),
            ("plan-logistics.png", "skipped"),
            ("day-1.png", "uploaded"),
            ("day-1-logistics.png", "skipped"),
            ("day-2.png", "failed"),
            ("plan-airport.png", "uploaded"),
        ]);
        let facts = SlotFacts { day_numbers: &[1, 2], has_flights: true, statuses: &st };
        let figures = [
            fig("plan.png", true),
            fig("plan-airport.png", true),
            fig("plan-logistics.png", false),
            fig("day-1.png", true),
            fig("day-2.png", false),
        ];
        let out = slot_checks(&figures, &facts);
        assert!(out.iter().all(|(_, c)| c.level == Level::Pass), "{out:?}");
        assert_eq!(out.len(), 5, "{out:?}");

        // Without flights a skipped logistics map has no frame at all.
        let facts = SlotFacts { day_numbers: &[1, 2], has_flights: false, statuses: &st };
        let out = slot_checks(&figures, &facts);
        let bad: Vec<_> = levels_by_key(&out).into_iter().filter(|(_, l)| *l != Level::Pass).collect();
        assert_eq!(bad, [("plan-logistics.png".to_string(), Level::Fail)], "{out:?}");
        let out = slot_checks(
            &[fig("plan.png", true), fig("plan-airport.png", true), fig("day-1.png", true), fig("day-2.png", false)],
            &facts,
        );
        assert!(out.iter().all(|(_, c)| c.level == Level::Pass), "{out:?}");
    }

    #[test]
    fn uploaded_must_be_an_img_and_skipped_must_not() {
        let st = statuses(&[("plan.png", "uploaded"), ("day-1.png", "skipped"), ("day-1-away.png", "uploaded")]);
        let facts = SlotFacts { day_numbers: &[1], has_flights: false, statuses: &st };
        let out = slot_checks(&[fig("plan.png", false), fig("day-1.png", true)], &facts);
        let find = |k: &str| out.iter().find(|(kk, _)| kk == k).unwrap().1.clone();
        let c = find("plan.png");
        assert!(c.level == Level::Fail && c.detail.contains("missing-map frame"), "{c:?}");
        let c = find("day-1.png");
        assert_eq!((c.level, c.next), (Level::Fail, Some(Next::DeleteStale)));
        let c = find("day-1-away.png");
        assert!(c.level == Level::Fail && c.detail.contains("no slot"), "{c:?}");
    }

    #[test]
    fn always_slots_must_exist_and_map_only_slots_have_no_frame() {
        let st = statuses(&[("plan.png", "uploaded")]);
        let facts = SlotFacts { day_numbers: &[1], has_flights: false, statuses: &st };
        // day-1.png has no row and no slot: the worker always renders its frame.
        let out = slot_checks(&[fig("plan.png", true)], &facts);
        let c = &out.iter().find(|(k, _)| k == "day-1.png").unwrap().1;
        assert!(c.level == Level::Fail && c.detail.contains("no map_artifacts row"), "{c:?}");
        // A frame for an excursion map (map-only slot) is a worker rule break.
        let out = slot_checks(&[fig("plan.png", true), fig("day-1.png", false), fig("plan-excursion.png", false)], &facts);
        let c = &out.iter().find(|(k, _)| k == "plan-excursion.png").unwrap().1;
        assert!(c.level == Level::Fail && c.detail.contains("renders nothing without a map"), "{c:?}");
        // An <img> for a key with no map_artifacts row: snapshot-maps never revisits
        // it, so the hint is the R2 delete, not a re-snapshot.
        let out = slot_checks(&[fig("plan.png", true), fig("day-1.png", false), fig("plan-airport.png", true)], &facts);
        let c = &out.iter().find(|(k, _)| k == "plan-airport.png").unwrap().1;
        assert_eq!((c.level, c.next), (Level::Fail, Some(Next::DeleteStale)), "{c:?}");
        // A figure that is neither a map nor the missing-map frame fails.
        let empty = MapFigure { key: Some("day-1.png".into()), ..MapFigure::default() };
        let out = slot_checks(&[fig("plan.png", true), empty], &facts);
        let c = &out.iter().find(|(k, _)| k == "day-1.png").unwrap().1;
        assert!(c.level == Level::Fail && c.detail.contains("neither"), "{c:?}");
        // Duplicate and unknown keys fail.
        let out = slot_checks(&[fig("plan.png", true), fig("plan.png", true), fig("day-9.png", false)], &facts);
        let lv = levels_by_key(&out);
        assert!(lv.contains(&("plan.png".into(), Level::Fail)) && lv.contains(&("day-9.png".into(), Level::Fail)), "{out:?}");
    }

    #[test]
    fn a_page_without_map_keys_is_one_deploy_fail() {
        let st = statuses(&[("plan.png", "uploaded")]);
        let facts = SlotFacts { day_numbers: &[1], has_flights: false, statuses: &st };
        let unkeyed = MapFigure { key: None, img_src: Some("/map/p-1/plan.png".into()), ..MapFigure::default() };
        let out = slot_checks(&[unkeyed.clone(), unkeyed], &facts);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!((out[0].1.level, out[0].1.next), (Level::Fail, Some(Next::Deploy)));
        assert!(out[0].1.detail.contains("2 of 2 map figure(s) carry no data-map-key"), "{out:?}");
    }

    fn row(seq: i64, label: &str, lat: f64, lon: f64) -> LegendRow {
        LegendRow { seq, label: Some(label.into()), lat: Some(lat), lon: Some(lon), ..LegendRow::default() }
    }

    #[test]
    fn flat_legend_matches_row_by_row() {
        let doc = parse_page(&viewer_page(""));
        let overview = &doc.figures[0];
        let rows = [row(1, "金閣寺", 35.0394, 135.7292), row(2, "北野天滿宮 & 梅", 35.0117, 135.7409)];
        assert_eq!(legend_check(overview, &rows).level, Level::Pass);
        // Order is by seq, whatever order the rows come in.
        assert_eq!(legend_check(overview, &[rows[1].clone(), rows[0].clone()]).level, Level::Pass);
        // A different name, a moved pin, or a different number each fail.
        let mut moved = rows.clone();
        moved[1].lat = Some(35.0118);
        let c = legend_check(overview, &moved);
        assert!(c.level == Level::Fail && c.detail.contains("row 2"), "{c:?}");
        let mut renamed = rows.clone();
        renamed[0].label = Some("銀閣寺".into());
        assert_eq!(legend_check(overview, &renamed).level, Level::Fail);
        let mut renumbered = rows.clone();
        renumbered[0].pin_no = Some(7);
        assert_eq!(legend_check(overview, &renumbered).level, Level::Fail);
        // A row the worker cannot decode (no coordinates) is missing on the page.
        let extra = [rows[0].clone(), rows[1].clone(), LegendRow { seq: 3, label: Some("x".into()), ..LegendRow::default() }];
        let c = legend_check(overview, &extra);
        assert!(c.level == Level::Fail && c.detail.contains("page lists 2 legend row(s), map_legend_stops has 3"), "{c:?}");
    }

    #[test]
    fn grouped_legend_is_compared_in_the_worker_render_order() {
        // As render/map.rs map_legend_html draws plan.png: day blocks in first
        // appearance order, pin numbers restart per day, then day-less rows.
        let legend = "<div class=\"map-legend-days\">\
            <div class=\"map-legend-day\"><div class=\"leg-day\"><span class=\"leg-swatch\" style=\"background:#e6194b\"></span>第 1 天</div>\
            <ol class=\"map-legend\"><li><span class=\"leg-num\">1</span><a href=\"https://www.google.com/maps?q=35.01,135\" target=\"_blank\" rel=\"noopener\">金閣寺</a></li>\
            <li><span class=\"leg-num\">2</span><a href=\"https://www.google.com/maps?q=35.03,135\" target=\"_blank\" rel=\"noopener\">清水寺</a></li></ol></div>\
            <div class=\"map-legend-day\"><div class=\"leg-day\">第 2 天</div>\
            <ol class=\"map-legend\"><li><span class=\"leg-num\">1</span><a href=\"https://www.google.com/maps?q=35.02,135\" target=\"_blank\" rel=\"noopener\">伏見稻荷</a></li></ol></div>\
            <ol class=\"map-legend\"><li><span class=\"leg-num\">4</span><a href=\"https://www.google.com/maps?q=35.04,135.5\" target=\"_blank\" rel=\"noopener\">旅館</a></li></ol></div>";
        let html = page(
            "zh-TW",
            "",
            &format!("<figure class=\"map-frame\" data-map-key=\"plan.png\"><img class=\"planmap\" src=\"/map/p-1/plan.png\"><figcaption>x</figcaption>{legend}</figure>"),
        );
        let doc = parse_page(&html);
        let f = &doc.figures[0];
        assert_eq!(f.legend.len(), 4);
        assert_eq!(f.legend_day_blocks, 2);
        let day = |seq, d, pin, label: &str, lat| LegendRow {
            day_number: Some(d),
            pin_no: Some(pin),
            ..row(seq, label, lat, 135.0)
        };
        let rows = [
            day(1, 1, 1, "金閣寺", 35.01),
            day(2, 2, 1, "伏見稻荷", 35.02),
            day(3, 1, 2, "清水寺", 35.03),
            row(4, "旅館", 35.04, 135.5),
        ];
        let c = legend_check(f, &rows);
        assert_eq!(c.level, Level::Pass, "{c:?}");
        assert!(c.detail.contains("2 day block(s)"), "{c:?}");
        // Same rows but day 2's pin now first: the render order changes.
        let mut swapped = rows.clone();
        swapped[0].seq = 5;
        assert_eq!(legend_check(f, &swapped).level, Level::Fail);
    }

    #[test]
    fn missing_frame_expects_no_legend_and_no_ol_is_zero_rows() {
        let doc = parse_page(&viewer_page(""));
        let frame = &doc.figures[1];
        assert!(frame.missing_frame && frame.img_src.is_none());
        assert_eq!(legend_check(frame, &[row(1, "a", 1.0, 2.0)]).level, Level::Pass);
        let bare = MapFigure { key: Some("day-1.png".into()), img_src: Some("/map/p-1/day-1.png".into()), ..MapFigure::default() };
        assert_eq!(legend_check(&bare, &[]).level, Level::Pass);
        assert_eq!(legend_check(&bare, &[row(1, "a", 1.0, 2.0)]).level, Level::Fail);
    }

    #[test]
    fn img_src_parses_as_a_url_whatever_the_attribute_order() {
        let html = page(
            "zh-TW",
            "",
            "<figure data-map-key=\"day-2.png\" class=\"map-frame\"><img src=\"/map/p-1/day-2.png?v=Wdead\" alt=\"x\" class=\"daymap\"></figure>",
        );
        let doc = parse_page(&html);
        let base = Url::parse("https://dash.example/").unwrap();
        let src = doc.figures[0].img_src.as_deref().unwrap();
        let url = img_src_check(&base, "p-1", "day-2.png", src).unwrap().unwrap();
        assert_eq!(url.as_str(), "https://dash.example/map/p-1/day-2.png?v=Wdead");
        // No ?v= → nothing to refetch; wrong key or host → FAIL.
        assert_eq!(img_src_check(&base, "p-1", "day-2.png", "/map/p-1/day-2.png").unwrap(), None);
        assert!(img_src_check(&base, "p-1", "day-3.png", src).is_err());
        assert!(img_src_check(&base, "p-1", "day-2.png", "https://evil.example/map/p-1/day-2.png").is_err());
    }

    #[test]
    fn extended_elements_are_found_inside_their_sections_only() {
        let extra = "<section class=\"companion-options\"><h2>同行方案</h2><div class=\"companion-map-frame\"><img class=\"companion-map-img\" loading=\"lazy\" alt=\"\" src=\"https://server.arcgisonline.com/x/export?bbox=1,2,3,4&amp;bboxSR=4326\"></div>\
            <div class=\"companion-grid\"><article class=\"companion-card\"><h3>A｜x</h3><div class=\"companion-stops\"><span class=\"stop-chip\">s1</span></div></article><article class=\"companion-card\"><h3>B｜y</h3><div class=\"companion-stops\"></div></article></div></section>\
            <div class=\"booking-cand-detail\"><div class=\"candidate-gallery\"><figure class=\"candidate-gallery-item\"><a href=\"u\"><img class=\"candidate-gallery-img\" src=\"https://img.example/1.jpg\" alt=\"h\"></a></figure></div>\
            <div class=\"cand-minimap\"><a class=\"cand-minimap-link\" href=\"m\"><img class=\"cand-minimap-img\" src=\"https://server.arcgisonline.com/mm\" alt=\"\"></a></div></div>\
            <div class=\"candidate-gallery\"><img class=\"candidate-gallery-img\" src=\"https://img.example/candidate-only.jpg\"></div>\
            <a class=\"voucher-link\" href=\"/voucher/p-1/v.pdf?token=abc\" target=\"_blank\">📄</a>";
        let doc = parse_page(&viewer_page(extra));
        let c = doc.companion.as_ref().unwrap();
        assert_eq!(c.cards, 2);
        assert_eq!(c.stopless_options, 1, "option B renders an empty stop list");
        assert_eq!(c.img_srcs, ["https://server.arcgisonline.com/x/export?bbox=1,2,3,4&bboxSR=4326"]);
        assert_eq!(doc.stay_photos, ["https://img.example/1.jpg"]);
        assert_eq!(doc.stay_minimaps, ["https://server.arcgisonline.com/mm"]);
        assert_eq!(doc.voucher_hrefs, ["/voucher/p-1/v.pdf?token=abc"]);
    }

    #[test]
    fn external_images_fail_only_when_broken_for_everyone() {
        assert_eq!(external_image_verdict(200, "image/png", 10).level, Level::Pass);
        assert_eq!(external_image_verdict(200, "image/jpeg; charset=x", 10).level, Level::Pass);
        assert_eq!(external_image_verdict(404, "text/html", 10).level, Level::Fail);
        assert_eq!(external_image_verdict(410, "", 0).level, Level::Fail);
        assert_eq!(external_image_verdict(200, "image/png", 0).level, Level::Fail);
        assert_eq!(external_image_verdict(403, "text/html", 10).level, Level::Warn);
        assert_eq!(external_image_verdict(503, "", 0).level, Level::Warn);
        assert_eq!(external_image_verdict(200, "text/html", 10).level, Level::Warn);
    }

    #[test]
    fn voucher_link_must_carry_the_page_token_and_serve_a_pdf() {
        let base = Url::parse("https://dash.example/?plan=p-1&token=t0k").unwrap();
        let url = voucher_target(&base, "/voucher/p-1/v.pdf?token=t0k", "t0k").unwrap();
        assert_eq!(url.path(), "/voucher/p-1/v.pdf");
        let c = voucher_target(&base, "/voucher/p-1/v.pdf", "t0k").unwrap_err();
        assert_eq!(c.level, Level::Fail);
        assert!(!c.detail.contains("t0k"), "never print the token: {c:?}");
        assert_eq!(voucher_target(&base, "https://other.example/v.pdf", "t0k").unwrap_err().level, Level::Warn);

        assert_eq!(voucher_verdict(200, b"%PDF-1.7 ...").level, Level::Pass);
        assert_eq!(voucher_verdict(200, b"<html>").level, Level::Fail);
        let c = voucher_verdict(403, b"Forbidden");
        assert!(c.level == Level::Fail && c.detail.contains("no permission"), "{c:?}");
        let c = voucher_verdict(404, b"voucher not found");
        assert!(c.level == Level::Fail && c.detail.contains("missing in R2"), "{c:?}");
    }
}
