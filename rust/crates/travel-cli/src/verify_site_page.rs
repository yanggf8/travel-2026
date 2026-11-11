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
use sha2::{Digest, Sha256};
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

/// One `details.day-variant` block as the page renders it.
#[derive(Clone, Debug, Default)]
pub(crate) struct VariantBlock {
    /// `data-variant-of` (the main day the block claims to belong to).
    pub(crate) of: Option<i64>,
    /// `data-day` of the nearest enclosing `section.day` card, if any.
    pub(crate) card_day: Option<i64>,
    /// The `open` attribute — the worker serves every variant collapsed (without it).
    pub(crate) open: bool,
    pub(crate) summary: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DocumentLink {
    pub(crate) key: String,
    pub(crate) href: String,
    pub(crate) summary: bool,
    pub(crate) day: Option<i64>,
    pub(crate) variant: bool,
}

/// Everything layer 1 reads from one page.
#[derive(Clone, Debug, Default)]
pub(crate) struct PageDoc {
    /// `section.day` cards (one per trip day; variants live inside them).
    pub(crate) day_cards: usize,
    pub(crate) variants: Vec<VariantBlock>,
    /// Visible text of the body (no `<script>` / `<style>` content).
    pub(crate) visible_text: String,
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
    pub(crate) document_links: Vec<DocumentLink>,
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
    out.day_cards = body.select(&sel("section.day")).count();
    let summary = sel("summary.day-variant-summary");
    for d in body.select(&sel("details.day-variant")) {
        out.variants.push(VariantBlock {
            of: d.value().attr("data-variant-of").and_then(|v| v.trim().parse().ok()),
            card_day: d
                .ancestors()
                .filter_map(ElementRef::wrap)
                .find(|a| a.value().name() == "section" && has_class(*a, "day"))
                .and_then(|a| a.value().attr("data-day"))
                .and_then(|v| v.trim().parse().ok()),
            open: d.value().attr("open").is_some(),
            summary: d.select(&summary).next().map(|s| text_of(s).trim().to_string()).unwrap_or_default(),
        });
    }
    for node in body.descendants() {
        if let Some(t) = node.value().as_text() {
            let hidden = node
                .parent()
                .and_then(ElementRef::wrap)
                .is_some_and(|p| matches!(p.value().name(), "script" | "style"));
            if !hidden {
                out.visible_text.push_str(t);
                out.visible_text.push(' ');
            }
        }
    }
    out.stay_photos = srcs(body, ".booking-cand-detail img.candidate-gallery-img");
    out.stay_minimaps = srcs(body, ".booking-cand-detail img.cand-minimap-img");
    out.voucher_hrefs = body
        .select(&sel("a.voucher-link"))
        .filter_map(|a| a.value().attr("href").map(str::to_string))
        .collect();
    for a in body.select(&sel("a.doc-link")) {
        let ancestors = a.ancestors().filter_map(ElementRef::wrap).collect::<Vec<_>>();
        out.document_links.push(DocumentLink {
            key: a.value().attr("data-doc").unwrap_or("").to_string(),
            href: a.value().attr("href").unwrap_or("").to_string(),
            summary: ancestors.iter().any(|el| {
                el.value().name() == "section" && has_class(*el, "plan-documents")
            }),
            day: ancestors
                .iter()
                .find(|el| el.value().name() == "section" && has_class(**el, "day"))
                .and_then(|el| el.value().attr("data-day"))
                .and_then(|value| value.parse().ok()),
            variant: ancestors.iter().any(|el| {
                el.value().name() == "details" && has_class(*el, "day-variant")
            }),
        });
    }
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
                None,
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

/// True when `text` holds `prefix{n}suffix` where a digit does not continue `n`.
fn mentions_day(text: &str, prefix: &str, n: i64, suffix: &str) -> bool {
    let needle = format!("{prefix}{n}{suffix}");
    text.match_indices(&needle).any(|(i, _)| {
        suffix.starts_with(|c: char| !c.is_ascii_digit())
            || !text[i + needle.len()..].starts_with(|c: char| c.is_ascii_digit())
    })
}

/// Day-variant markup checks (worker render/day.rs): `variants` is the plan's
/// variant day_number -> (main day, EN label); `trip_days` the number of trip
/// days (variants excluded); `labels` maps each main day to its variants' labels in the page
/// language (each must match one block's summary, one-to-one). A plan with no variants and a page with no
/// variant blocks yields no findings, so plain plans report exactly as before. Each block must be
/// served collapsed (no `open` attribute): the main plan reads first.
pub(crate) fn variant_checks(
    doc: &PageDoc,
    variants: &HashMap<i64, (i64, Option<String>)>,
    labels: &HashMap<i64, Vec<String>>,
    trip_days: usize,
) -> Vec<(&'static str, Check)> {
    if variants.is_empty() && doc.variants.is_empty() {
        return Vec::new();
    }
    let mut problems = Vec::new();
    if variants.is_empty() {
        problems.push(format!("{} day-variant block(s) on a plan with no day variants", doc.variants.len()));
    } else if doc.variants.is_empty() {
        return vec![(
            "variant",
            Check::fail(
                format!("{} day variant(s) in the data but no details.day-variant on the page", variants.len()),
                Some(Next::Deploy),
            ),
        )];
    }
    if doc.day_cards != trip_days {
        problems.push(format!("{} section.day card(s) for {trip_days} trip day(s)", doc.day_cards));
    }
    if doc.variants.len() != variants.len() {
        problems.push(format!("{} day-variant block(s) for {} variant(s)", doc.variants.len(), variants.len()));
    }
    for v in &doc.variants {
        let name = v.of.map_or("?".to_string(), |n| n.to_string());
        match (v.of, v.card_day) {
            (None, _) => problems.push("a day-variant has no numeric data-variant-of".to_string()),
            (Some(of), Some(card)) if of == card => {}
            (Some(of), card) => problems.push(format!(
                "day-variant of day {of} sits in {} instead of section.day[data-day={of}]",
                card.map_or("no data-day card".to_string(), |c| format!("the card for day {c}"))
            )),
        }
        if v.open {
            problems.push(format!("day-variant of day {name} is served open (must be collapsed: no open attribute)"));
        }
        if v.summary.is_empty() {
            problems.push(format!("day-variant of day {name} has an empty summary"));
        }
    }
    let mut mains: Vec<i64> = labels.keys().copied().collect();
    for v in &doc.variants {
        if let Some(of) = v.of {
            if !mains.contains(&of) {
                mains.push(of);
            }
        }
    }
    mains.sort_unstable();
    for main in mains {
        let mut unused: Vec<&str> = doc.variants.iter().filter(|v| v.of == Some(main)).map(|v| v.summary.as_str()).collect();
        for label in labels.get(&main).map(Vec::as_slice).unwrap_or(&[]) {
            match unused.iter().position(|s| s.contains(label.as_str())) {
                Some(i) => {
                    unused.remove(i);
                }
                None => problems.push(format!("day {main}: no day-variant block labelled {label:?}")),
            }
        }
        for extra in unused {
            problems.push(format!("day {main}: unexpected day-variant block {extra:?} (no matching variant label)"));
        }
    }
    let mut internal: Vec<i64> = variants.keys().copied().collect();
    internal.sort_unstable();
    for n in internal {
        if mentions_day(&doc.visible_text, "第 ", n, " 天") || mentions_day(&doc.visible_text, "Day ", n, "") {
            problems.push(format!("the variant's internal day number {n} is visible as a day heading"));
        }
    }
    let check = if problems.is_empty() {
        Check::pass(format!("{} day variant(s) collapsed inside their main day cards", doc.variants.len()))
    } else {
        Check::fail(problems.join("; "), None)
    };
    vec![("variant", check)]
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

fn document_identity_error(
    link: &DocumentLink,
    expected: &[travel_db::repo::plan_documents::PlanDocument],
) -> Option<String> {
    let Some(document) = expected.iter().find(|document| document.doc_key == link.key) else {
        return Some(format!("unknown data-doc {:?}", link.key));
    };
    let base = Url::parse("https://dashboard.invalid/").expect("static base url");
    let path = match base.join(&link.href) {
        Ok(url) => url.path().to_string(),
        Err(error) => return Some(format!("data-doc {} has an invalid href: {error}", link.key)),
    };
    let expected_path = format!("/voucher/{}", document.r2_key);
    if path != expected_path {
        return Some(format!(
            "data-doc {} has href path {path}, expected {expected_path}",
            link.key
        ));
    }
    None
}

pub(crate) fn valid_document_hrefs(
    doc: &PageDoc,
    expected: &[travel_db::repo::plan_documents::PlanDocument],
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for link in &doc.document_links {
        if document_identity_error(link, expected).is_none() {
            out.entry(link.key.clone()).or_insert_with(|| link.href.clone());
        }
    }
    out
}

pub(crate) fn document_placement_checks(
    doc: &PageDoc,
    expected: &[travel_db::repo::plan_documents::PlanDocument],
) -> Vec<(String, Check)> {
    let mut out = Vec::new();
    if expected.is_empty() && doc.document_links.is_empty() {
        return vec![("summary".to_string(), Check::pass("no plan documents"))];
    }

    let mut summary_counts: HashMap<String, usize> = HashMap::new();
    for link in doc.document_links.iter().filter(|link| link.summary) {
        *summary_counts.entry(link.key.clone()).or_default() += 1;
    }
    let mut summary_problems = Vec::new();
    for document in expected {
        match summary_counts.get(&document.doc_key).copied().unwrap_or(0) {
            0 => summary_problems.push(format!("missing {}", document.doc_key)),
            1 => {}
            count => summary_problems.push(format!("duplicate {} ({count} links)", document.doc_key)),
        }
    }
    for (key, count) in &summary_counts {
        if !expected.iter().any(|document| &document.doc_key == key) {
            summary_problems.push(format!("unexpected {key} ({count} links)"));
        }
    }
    out.push((
        "summary".to_string(),
        if summary_problems.is_empty() {
            Check::pass(format!("{} summary document link(s)", expected.len()))
        } else {
            Check::fail(
                format!("summary document keys differ: {}", summary_problems.join(", ")),
                Some(Next::Deploy),
            )
        },
    ));

    for (index, link) in doc.document_links.iter().enumerate() {
        if let Some(error) = document_identity_error(link, expected) {
            out.push((
                format!("identity-{}", index + 1),
                Check::fail(error, Some(Next::Deploy)),
            ));
        }
    }
    if doc.document_links.iter().any(|link| link.variant) {
        out.push((
            "variants".to_string(),
            Check::fail("a document link rendered inside a day-variant block", Some(Next::Deploy)),
        ));
    }
    for (index, link) in doc.document_links.iter().enumerate().filter(|(_, link)| !link.variant) {
        match link.day {
            Some(day) => {
                if let Some(document) = expected.iter().find(|document| document.doc_key == link.key)
                    && !document.days.contains(&day)
                {
                    out.push((
                        format!("unexpected-day-{}", index + 1),
                        Check::fail(
                            format!("unexpected day placement: {} is not mapped to day {day}", link.key),
                            Some(Next::Deploy),
                        ),
                    ));
                }
            }
            None if !link.summary => out.push((
                format!("placement-{}", index + 1),
                Check::fail(
                    format!("{} is outside the summary and main day cards", link.key),
                    Some(Next::Deploy),
                ),
            )),
            None => {}
        }
    }
    for document in expected {
        for day in &document.days {
            let found = doc.document_links.iter().any(|link| {
                link.key == document.doc_key
                    && link.day == Some(*day)
                    && !link.variant
                    && document_identity_error(link, expected).is_none()
            });
            out.push((
                format!("{}:day-{day}", document.doc_key),
                if found {
                    Check::pass(format!("{} linked in day {day}", document.doc_key))
                } else {
                    Check::fail(
                        format!("{} has no valid doc-link in the main day {day} card", document.doc_key),
                        Some(Next::Deploy),
                    )
                },
            ));
        }
    }
    out
}

pub(crate) fn document_verdict(http: u16, body: &[u8], expected_sha256: &str) -> Check {
    if http != 200 {
        return Check::fail(format!("HTTP {http}: document PDF could not be fetched"), None);
    }
    if !body.starts_with(b"%PDF-") {
        return Check::fail(format!("HTTP 200 but the {}-byte body is not a PDF", body.len()), None);
    }
    let actual = format!("{:x}", Sha256::digest(body));
    if actual != expected_sha256 {
        return Check::fail("document PDF sha256 differs from plan_documents.sha256", None);
    }
    Check::pass(format!("PDF, {} bytes, sha256 matches", body.len()))
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
        403 => Check::fail("HTTP 403: the token has no permission for this plan's voucher", None),
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

    fn variants_of(pairs: &[(i64, i64)]) -> HashMap<i64, (i64, Option<String>)> {
        pairs.iter().map(|(v, m)| (*v, (*m, Some("Rainy".to_string())))).collect()
    }

    fn variant_html(details: &str) -> String {
        viewer_page(&format!(
            "<section class=\"day day-full\" data-day=\"2\"><h2>第 2 天 · 2026-11-15</h2><span class=\"day-option-label\">晴天</span>{details}</section>"
        ))
    }

    const GOOD_VARIANT: &str = "<details class=\"day-variant\" data-variant-of=\"2\"><summary class=\"day-variant-summary\">替代方案：雨天</summary><div class=\"day-variant-body\"><div class=\"theme\">第 2 天 · 雨天</div></div></details>";

    fn labels_of(pairs: &[(i64, &str)]) -> HashMap<i64, Vec<String>> {
        let mut m: HashMap<i64, Vec<String>> = HashMap::new();
        for (main, l) in pairs {
            m.entry(*main).or_default().push(l.to_string());
        }
        m
    }

    fn variant_verdict(html: &str, v: &HashMap<i64, (i64, Option<String>)>) -> Check {
        variant_verdict_with(html, v, &labels_of(&[(2, "雨天")]))
    }

    fn variant_verdict_with(html: &str, v: &HashMap<i64, (i64, Option<String>)>, l: &HashMap<i64, Vec<String>>) -> Check {
        let doc = parse_page(html);
        let mut out = variant_checks(&doc, v, l, 2);
        assert_eq!(out.len(), 1);
        out.remove(0).1
    }

    #[test]
    fn plain_page_has_no_variant_findings() {
        assert!(variant_checks(&parse_page(&viewer_page("")), &HashMap::new(), &HashMap::new(), 1).is_empty());
    }

    #[test]
    fn variant_page_passes() {
        let c = variant_verdict(&variant_html(GOOD_VARIANT), &variants_of(&[(3, 2)]));
        assert_eq!(c.level, Level::Pass, "{c:?}");
    }

    #[test]
    fn variant_outside_its_card_fails() {
        let html = viewer_page(&format!(
            "<section class=\"day day-full\" data-day=\"2\"><h2>第 2 天</h2></section>{GOOD_VARIANT}"
        ));
        let c = variant_verdict(&html, &variants_of(&[(3, 2)]));
        assert_eq!(c.level, Level::Fail, "{c:?}");
        let wrong = viewer_page(&format!(
            "<section class=\"day day-full\" data-day=\"1\">{GOOD_VARIANT}</section>"
        ));
        assert_eq!(variant_verdict(&wrong, &variants_of(&[(3, 2)])).level, Level::Fail);
    }

    #[test]
    fn duplicate_variant_blocks_fail() {
        let sunny = GOOD_VARIANT.replace("雨天", "晴天");
        let html = variant_html(&format!("{sunny}{sunny}"));
        let v = variants_of(&[(3, 2), (4, 2)]);
        let c = variant_verdict_with(&html, &v, &labels_of(&[(2, "晴天"), (2, "陰天")]));
        assert_eq!(c.level, Level::Fail, "{c:?}");
        assert!(c.detail.contains("陰天"), "{c:?}");
    }

    #[test]
    fn wrong_label_fails() {
        let c = variant_verdict_with(&variant_html(GOOD_VARIANT), &variants_of(&[(3, 2)]), &labels_of(&[(2, "晴天")]));
        assert_eq!(c.level, Level::Fail, "{c:?}");
        assert!(c.detail.contains("晴天"), "{c:?}");
    }

    #[test]
    fn open_variant_fails() {
        let open = GOOD_VARIANT.replace("data-variant-of=\"2\">", "data-variant-of=\"2\" open>");
        assert_ne!(open, GOOD_VARIANT);
        let c = variant_verdict(&variant_html(&open), &variants_of(&[(3, 2)]));
        assert_eq!(c.level, Level::Fail, "{c:?}");
        assert!(c.detail.contains("served open"), "{c:?}");
    }

    #[test]
    fn internal_number_visible_fails() {
        let leaky = GOOD_VARIANT.replace("第 2 天 · 雨天", "第 3 天 · 雨天");
        let c = variant_verdict(&variant_html(&leaky), &variants_of(&[(3, 2)]));
        assert_eq!(c.level, Level::Fail, "{c:?}");
        let leaky_en = GOOD_VARIANT.replace("第 2 天 · 雨天", "Day 3 · Rainy");
        assert_eq!(variant_verdict(&variant_html(&leaky_en), &variants_of(&[(3, 2)])).level, Level::Fail);
        let fine = GOOD_VARIANT.replace("第 2 天 · 雨天", "Day 31 and 第 30 天");
        assert_eq!(variant_verdict(&variant_html(&fine), &variants_of(&[(3, 2)])).level, Level::Pass);
    }

    #[test]
    fn empty_summary_and_missing_markup_fail() {
        let empty = GOOD_VARIANT.replace("替代方案：雨天", "  ");
        assert_eq!(variant_verdict(&variant_html(&empty), &variants_of(&[(3, 2)])).level, Level::Fail);
        let c = variant_verdict(&viewer_page(""), &variants_of(&[(3, 2)]));
        assert_eq!(c.level, Level::Fail);
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
        assert_eq!((checks[0].1.level, checks[0].1.next), (Level::Fail, None));
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
            <a class=\"voucher-link\" href=\"/voucher/p-1/v.pdf?token=abc\" target=\"_blank\">📄</a>\
            <section class=\"plan-documents\"><a class=\"doc-link\" data-doc=\"flight\" href=\"/voucher/p-1/doc-flight.pdf?token=abc\">機票</a></section>\
            <section class=\"day\" data-day=\"1\"><a class=\"doc-link\" data-doc=\"flight\" href=\"/voucher/p-1/doc-flight.pdf?token=abc\">機票</a>\
            <details class=\"day-variant\"><a class=\"doc-link\" data-doc=\"wrong\" href=\"/voucher/p-1/doc-wrong.pdf?token=abc\">錯</a></details></section>";
        let doc = parse_page(&viewer_page(extra));
        let c = doc.companion.as_ref().unwrap();
        assert_eq!(c.cards, 2);
        assert_eq!(c.stopless_options, 1, "option B renders an empty stop list");
        assert_eq!(c.img_srcs, ["https://server.arcgisonline.com/x/export?bbox=1,2,3,4&bboxSR=4326"]);
        assert_eq!(doc.stay_photos, ["https://img.example/1.jpg"]);
        assert_eq!(doc.stay_minimaps, ["https://server.arcgisonline.com/mm"]);
        assert_eq!(doc.voucher_hrefs, ["/voucher/p-1/v.pdf?token=abc"]);
        assert_eq!(doc.document_links.len(), 3);
        assert!(doc.document_links[0].summary);
        assert_eq!(doc.document_links[1].day, Some(1));
        assert!(!doc.document_links[1].variant);
        assert!(doc.document_links[2].variant);
    }

    fn expected_documents() -> Vec<travel_db::repo::plan_documents::PlanDocument> {
        vec![
            travel_db::repo::plan_documents::PlanDocument {
                doc_key: "A".into(),
                r2_key: "p-1/doc-a.pdf".into(),
                days: vec![1],
                ..Default::default()
            },
            travel_db::repo::plan_documents::PlanDocument {
                doc_key: "B".into(),
                r2_key: "p-1/doc-b.pdf".into(),
                days: vec![2],
                ..Default::default()
            },
        ]
    }

    #[test]
    fn no_plan_documents_is_one_quiet_pass() {
        let checks = document_placement_checks(&parse_page(&viewer_page("")), &[]);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].1.level, Level::Pass);
        assert_eq!(checks[0].1.detail, "no plan documents");
    }

    #[test]
    fn document_placement_correct_page_passes() {
        let html = viewer_page(
            "<section class=\"plan-documents\">\
             <a class=\"doc-link\" data-doc=\"A\" href=\"/voucher/p-1/doc-a.pdf?token=abc\"></a>\
             <a class=\"doc-link\" data-doc=\"B\" href=\"/voucher/p-1/doc-b.pdf?token=abc\"></a></section>\
             <section class=\"day\" data-day=\"1\"><a class=\"doc-link\" data-doc=\"A\" href=\"/voucher/p-1/doc-a.pdf?token=abc\"></a></section>\
             <section class=\"day\" data-day=\"2\"><a class=\"doc-link\" data-doc=\"B\" href=\"/voucher/p-1/doc-b.pdf?token=abc\"></a></section>",
        );
        let doc = parse_page(&html);
        let expected = expected_documents();
        let checks = document_placement_checks(&doc, &expected);
        assert!(checks.iter().all(|(_, check)| check.level == Level::Pass), "{checks:?}");
        assert_eq!(valid_document_hrefs(&doc, &expected).len(), 2);
    }

    #[test]
    fn duplicate_summary_link_names_missing_document() {
        let html = viewer_page(
            "<section class=\"plan-documents\">\
             <a class=\"doc-link\" data-doc=\"A\" href=\"/voucher/p-1/doc-a.pdf?token=abc\"></a>\
             <a class=\"doc-link\" data-doc=\"A\" href=\"/voucher/p-1/doc-a.pdf?token=abc\"></a></section>",
        );
        let checks = document_placement_checks(&parse_page(&html), &expected_documents());
        let summary = checks.iter().find(|(key, _)| key == "summary").map(|(_, check)| check).unwrap();
        assert_eq!(summary.level, Level::Fail);
        assert!(summary.detail.contains("missing B"), "{summary:?}");
        assert!(summary.detail.contains("duplicate A"), "{summary:?}");
    }

    #[test]
    fn day_link_identity_must_match_its_data_doc() {
        let html = viewer_page(
            "<section class=\"plan-documents\">\
             <a class=\"doc-link\" data-doc=\"A\" href=\"/voucher/p-1/doc-a.pdf?token=abc\"></a>\
             <a class=\"doc-link\" data-doc=\"B\" href=\"/voucher/p-1/doc-b.pdf?token=abc\"></a></section>\
             <section class=\"day\" data-day=\"1\"><a class=\"doc-link\" data-doc=\"A\" href=\"/voucher/p-1/doc-a.pdf?token=abc\"></a></section>\
             <section class=\"day\" data-day=\"2\"><a class=\"doc-link\" data-doc=\"B\" href=\"/voucher/p-1/doc-a.pdf?token=abc\"></a></section>",
        );
        let checks = document_placement_checks(&parse_page(&html), &expected_documents());
        assert!(checks.iter().any(|(_, check)| {
            check.level == Level::Fail
                && check.detail.contains("data-doc B")
                && check.detail.contains("/voucher/p-1/doc-a.pdf")
        }), "{checks:?}");
    }

    #[test]
    fn unexpected_day_placement_fails() {
        let expected = expected_documents().into_iter().take(1).collect::<Vec<_>>();
        let html = viewer_page(
            "<section class=\"plan-documents\"><a class=\"doc-link\" data-doc=\"A\" href=\"/voucher/p-1/doc-a.pdf?token=abc\"></a></section>\
             <section class=\"day\" data-day=\"2\"><a class=\"doc-link\" data-doc=\"A\" href=\"/voucher/p-1/doc-a.pdf?token=abc\"></a></section>",
        );
        let checks = document_placement_checks(&parse_page(&html), &expected);
        assert!(checks.iter().any(|(key, check)| {
            key.starts_with("unexpected-day-")
                && check.level == Level::Fail
                && check.detail.contains("A is not mapped to day 2")
        }), "{checks:?}");
    }

    #[test]
    fn document_links_inside_variants_still_fail() {
        let expected = expected_documents().into_iter().take(1).collect::<Vec<_>>();
        let html = viewer_page(
            "<section class=\"plan-documents\"><a class=\"doc-link\" data-doc=\"A\" href=\"/voucher/p-1/doc-a.pdf?token=abc\"></a></section>\
             <section class=\"day\" data-day=\"1\"><details class=\"day-variant\"><a class=\"doc-link\" data-doc=\"A\" href=\"/voucher/p-1/doc-a.pdf?token=abc\"></a></details></section>",
        );
        let checks = document_placement_checks(&parse_page(&html), &expected);
        assert!(checks.iter().any(|(key, check)| key == "variants" && check.level == Level::Fail), "{checks:?}");
    }

    #[test]
    fn document_pdf_requires_signature_and_matching_sha256() {
        let body = b"%PDF-1.7\nfixture";
        let sha = format!("{:x}", Sha256::digest(body));
        assert_eq!(document_verdict(200, body, &sha).level, Level::Pass);
        assert_eq!(document_verdict(200, body, "00").level, Level::Fail);
        assert_eq!(document_verdict(200, b"<html>", &sha).level, Level::Fail);
        assert_eq!(document_verdict(404, body, &sha).level, Level::Fail);
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
