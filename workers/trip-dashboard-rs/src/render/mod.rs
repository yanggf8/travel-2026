pub mod activity_text;
pub mod alerts;
pub mod auth;
pub mod day;
pub mod index;
pub mod map;
pub mod session;
pub mod share;
pub mod summary;
use crate::i18n::{t, tf};
use crate::model::{Day, Plan};
pub use activity_text::render_activity_text;

pub fn day_option_name<'a>(day: &'a Day, lang: &str) -> &'a str {
    if lang == "en" && !day.variant_label.is_empty() {
        &day.variant_label
    } else {
        &day.variant_label_zh
    }
}

pub fn day_option_label(day: &Day, main_day: i64, lang: &str) -> String {
    tf(
        "dayOption",
        lang,
        &[
            ("day", &main_day.to_string()),
            ("label", day_option_name(day, lang)),
        ],
    )
}

/// Wrap a rendered body in the full HTML page shell: charset, mobile viewport,
/// `notranslate` (the ZH content must not be browser-auto-translated), and the
/// inlined stylesheet.
pub fn page(title: &str, body: &str, lang: &str) -> String {
    page_with_body_attrs(title, body, lang, "")
}

/// `page` with extra attributes on `<body>` (each with its leading space,
/// values already escaped). The plan page uses it for the `data-plan-id` /
/// `data-view` markers `travel verify-site` reads; every other page passes "".
fn page_with_body_attrs(title: &str, body: &str, lang: &str, body_attrs: &str) -> String {
    let lang_attr = if lang == "en" { "en" } else { "zh-TW" };
    format!(
        "<!doctype html><html lang=\"{lang_attr}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"google\" content=\"notranslate\">\
         <title>{}</title><style>{}</style></head><body{body_attrs}>{}</body></html>",
        esc(title),
        crate::styles::CSS,
        body,
    )
}

/// The plan page's outer-wrapper markers: which plan this page is, and whether
/// it rendered the owner view (owner chrome present — only an owner session
/// gets it) or the token viewer view. Read by `travel verify-site` to confirm
/// it is looking at the right plan through the viewer's eyes.
fn plan_body_attrs(plan_id: &str, owner_chrome: &str) -> String {
    let view = if owner_chrome.is_empty() { "viewer" } else { "owner" };
    format!(" data-plan-id=\"{}\" data-view=\"{view}\"", esc(plan_id))
}

/// Render a full plan page: plan overview map, booking summary, weather overview,
/// meal overview, then each day card.
/// `token` is the access token the page was loaded with — threaded into the
/// auth-gated voucher link so a click carries the same token (else 403).
/// `owner_chrome` is the logged-in owner top bar (copy share link); empty for viewers.
pub fn render_plan(
    plan: &Plan,
    lang: &str,
    token: Option<&str>,
    map_status: &map::MapStatus,
    owner_chrome: &str,
) -> String {
    let mut body = String::new();
    if !owner_chrome.is_empty() {
        body.push_str(owner_chrome);
        body.push_str(&share::copy_script());
    }
    // Weather + meal overviews are built up front so the 目錄 knows whether
    // they render; they are placed further down, right before the day cards.
    // Weather overview: last block of the top summary area, right before the
    // day cards it summarizes. Built from the already-loaded days + clothing
    // bands (no extra query); renders nothing when no day has weather.
    let weather = day::weather_overview(&plan.days, &plan.clothing_tips, lang);
    // Meal overview right after it (same slot when there is no weather): from
    // the already-loaded session meals, no extra query; nothing without meals.
    let meals = day::meal_overview(&plan.days, lang);
    // 目錄 at the very top of the page (user request 2026-11-11: the page is very
    // long on a phone): links to the blocks that actually rendered, plus
    // collapse/expand-all.
    body.push_str(&page_toc(plan, lang, !weather.is_empty(), !meals.is_empty()));
    // Non-meal pending-booking alerts BEFORE the summary (mirror render.ts:1388).
    body.push_str(&alerts::render_pending_alerts(plan, lang, false));
    // Plan overview map ABOVE the booking summary (its own frame, never inside
    // the summary's dashed box) — user visibility request.
    body.push_str(&map::plan_map_slot(
        &plan.plan_id,
        map_status.plan.as_deref(),
        lang,
        map_status.legend_for("plan.png"),
    ));
    body.push_str(&render_companion_options(plan, lang));
    // The logistics map only exists where the trip has an airport segment
    // (flights). A DOMESTIC self-drive plan has none — rendering the slot
    // anyway just showed a 地圖尚未產生 placeholder that wasted vertical
    // space (user request 2026-09-28). With flights it renders as a compact
    // inset (崁入小圖) instead of a second full-size map.
    // Supplementary insets sit side by side under the overview: the far-off
    // day-trip map and the arrival/departure-day map (each only when one
    // exists), then the hotel/airport map.
    let show_logistics = map_status.plan_logistics.is_some() || !plan.flights.is_empty();
    if map_status.plan_excursion.is_some() || map_status.plan_airport.is_some() || show_logistics {
        body.push_str("<div class=\"map-insets\">");
        if let Some(v) = map_status.plan_excursion.as_deref() {
            body.push_str(&map::plan_excursion_map_slot(
                &plan.plan_id,
                v,
                lang,
                map_status.legend_for("plan-excursion.png"),
            ));
        }
        if let Some(v) = map_status.plan_airport.as_deref() {
            body.push_str(&map::plan_airport_map_slot(
                &plan.plan_id,
                v,
                lang,
                map_status.legend_for("plan-airport.png"),
            ));
        }
        if show_logistics {
            body.push_str(&map::plan_logistics_map_slot(
                &plan.plan_id,
                map_status.plan_logistics.as_deref(),
                lang,
                map_status.legend_for("plan-logistics.png"),
            ));
        }
        body.push_str("</div>");
    }
    body.push_str(&summary::render(plan, lang, token));
    body.push_str(&weather);
    body.push_str(&meals);
    for d in &plan.days {
        body.push_str(&day::render_with_documents(
            d,
            &plan.plan_id,
            lang,
            map_status,
            &plan.clothing_tips,
            &plan.documents,
            token,
        ));
    }
    // Meal pending-booking alerts AFTER the day cards (mirror render.ts:1393),
    // then the transit cheat-sheet (mirror render.ts:1394).
    body.push_str(&alerts::render_pending_alerts(plan, lang, true));
    body.push_str(&alerts::render_transit_summary(plan, lang));
    // Last in <body>: every block it restores/wires already exists. Viewer and
    // owner pages alike; the page works fully without it.
    body.push_str(BLOCK_MEMORY_SCRIPT);
    page_with_body_attrs(&plan.display_name, &body, lang, &plan_body_attrs(&plan.plan_id, owner_chrome))
}

/// The plan page's 目錄 / Contents: plain fragment links (no JS needed) to each
/// rendered block — documents, booking summary, weather + meal overviews (only
/// when they rendered), then one chip per main day card (variants live inside
/// their day, no entry of their own). In page order. The collapse-all /
/// expand-all buttons are served `hidden` and un-hidden by
/// [`BLOCK_MEMORY_SCRIPT`], so a no-JS page never shows a dead button. The
/// floating ↑ link (pure CSS position:fixed) jumps back here from anywhere.
fn page_toc(plan: &Plan, lang: &str, has_weather: bool, has_meals: bool) -> String {
    let mut items: Vec<(&str, String)> = Vec::new();
    if !plan.documents.is_empty() {
        items.push((summary::BLOCK_DOCS, esc(t("ticketsVouchers", lang))));
    }
    items.push((summary::BLOCK_SUMMARY, esc(t("bookingSummary", lang))));
    if has_weather {
        items.push((day::BLOCK_WEATHER, esc(t("weatherOverview", lang))));
    }
    if has_meals {
        items.push((day::BLOCK_MEALS, esc(t("mealOverview", lang))));
    }
    let mut h = format!(
        "<nav class=\"page-toc\" id=\"toc\" aria-label=\"{toc}\"><div class=\"toc-head\"><h2>{toc}</h2>\
         <div class=\"toc-actions\"><button type=\"button\" class=\"toc-btn\" data-blocks=\"close\" hidden>{}</button>\
         <button type=\"button\" class=\"toc-btn\" data-blocks=\"open\" hidden>{}</button></div></div><ol class=\"toc-list\">",
        esc(t("collapseAll", lang)),
        esc(t("expandAll", lang)),
        toc = esc(t("toc", lang)),
    );
    for (id, label) in items {
        h.push_str(&format!("<li><a href=\"#{id}\">{label}</a></li>"));
    }
    for d in &plan.days {
        let date = summary::date_with_weekday(&d.date, lang).unwrap_or_else(|| d.date.clone());
        let heading = if date.is_empty() {
            tf("dayLabel", lang, &[("day", &d.day_number.to_string())])
        } else {
            tf("dayHeading", lang, &[("day", &d.day_number.to_string()), ("date", &date)])
        };
        let theme = day::day_theme(d, lang);
        let theme = if theme.is_empty() {
            String::new()
        } else {
            format!("<span class=\"toc-day-theme\">{}</span>", esc(theme))
        };
        h.push_str(&format!(
            "<li><a class=\"toc-day\" href=\"#{}\"><span class=\"toc-day-n\">{}</span>{theme}</a></li>",
            day::day_block_id(d.day_number),
            esc(&heading),
        ));
    }
    h.push_str(&format!(
        "</ol></nav><a class=\"toc-fab\" href=\"#toc\">{}</a>",
        esc(t("tocBack", lang))
    ));
    h
}

/// Remembered collapse state for the plan page's blocks — the ONE viewer-page
/// script (inline, no external source, ES5, emitted once at the end of <body>).
/// Every `details[data-block]` (summary, documents, overviews, day cards) and
/// `details.day-variant` (keyed `v<its own data-variant-day>`, stable when a
/// sibling variant is added or removed) is restored from
/// localStorage `trip-blocks:<data-plan-id>` (`key=1|0;…`, plain text) and saved
/// on every `toggle`. 全部收合 closes every block incl. variants; 全部展開 opens
/// every block but leaves each variant as it is (variants are served collapsed by
/// design). Following an in-page link opens the target block (and any closed
/// ancestor), on click and on `hashchange` (Back/Forward). Printing opens every
/// block (`beforeprint`) without saving and restores the screen state after
/// (`afterprint`). Every storage access is in try/catch: no storage → session-only
/// toggles. `<html data-no-block-memory>` (set by `travel verify-site --render`
/// before it opens every block) stops saving, so a check never rewrites a
/// reader's remembered state.
const BLOCK_MEMORY_SCRIPT: &str = r#"<script>
(function(){
  var d=document,de=d.documentElement,k='trip-blocks:'+(d.body.getAttribute('data-plan-id')||''),st={},els=[],ls=null,i;
  try{ls=window.localStorage;var v=ls&&ls.getItem(k);if(v){var p=v.split(';');for(i=0;i<p.length;i++){var kv=p[i].split('=');if(kv.length===2&&kv[0])st[kv[0]]=kv[1]==='1';}}}catch(e){ls=null;}
  function save(){if(!ls||de.hasAttribute('data-no-block-memory'))return;var o=[];for(var x in st)if(st.hasOwnProperty(x))o.push(x+'='+(st[x]?'1':'0'));try{ls.setItem(k,o.join(';'));}catch(e){}}
  var list=d.querySelectorAll('details[data-block],details.day-variant');
  for(i=0;i<list.length;i++){var el=list[i],b=el.getAttribute('data-block'),va=!b;if(va)b='v'+(el.getAttribute('data-variant-day')||'');els.push({el:el,k:b,v:va});if(st.hasOwnProperty(b))el.open=st[b];}
  d.addEventListener('toggle',function(e){if(pr)return;for(var j=0;j<els.length;j++)if(els[j].el===e.target){if(st[els[j].k]!==e.target.open){st[els[j].k]=e.target.open;save();}return;}},true);
  var pr=null;
  window.addEventListener('beforeprint',function(){if(pr)return;pr=[];for(var j=0;j<els.length;j++){pr.push(els[j].el.open);els[j].el.open=true;}});
  window.addEventListener('afterprint',function(){if(!pr)return;var o=pr;for(var j=0;j<els.length;j++)els[j].el.open=o[j];setTimeout(function(){pr=null;},0);});
  var bs=d.querySelectorAll('.toc-btn[data-blocks]');
  for(i=0;i<bs.length;i++){bs[i].hidden=false;bs[i].addEventListener('click',function(){var op=this.getAttribute('data-blocks')==='open';for(var j=0;j<els.length;j++)if(!(op&&els[j].v))els[j].el.open=op;});}
  function reveal(id){var t=id&&d.getElementById(id);if(!t)return null;for(var p=t;p&&p!==d;p=p.parentNode)if(p.tagName==='DETAILS')p.open=true;var inner=t.querySelector&&t.querySelector('details[data-block]');if(inner)inner.open=true;return t;}
  d.addEventListener('click',function(e){for(var a=e.target;a&&a!==d;a=a.parentNode)if(a.tagName==='A'){var h=a.getAttribute('href')||'';if(h.charAt(0)==='#')reveal(h.slice(1));return;}});
  window.addEventListener('hashchange',function(){reveal(location.hash.slice(1));});
  if(location.hash){var t=reveal(location.hash.slice(1));if(t&&t.scrollIntoView)t.scrollIntoView();}
})();
</script>"#;

/// Render an explicit companion-options block from the temporary, unconfirmed
/// itinerary note. This keeps alternatives visually separate from the official
/// day schedule while the group is deciding.
fn render_companion_options(plan: &Plan, lang: &str) -> String {
    let title = plan.days.iter().flat_map(|d| d.sessions.iter())
        .flat_map(|s| s.activities.iter())
        .find(|a| {
            let t = &a.title;
            t.starts_with("同行方案（可複選") || t.starts_with("同行方案（可選擇")
        })
        .map(|a| a.title.as_str());
    let Some(title) = title else { return String::new(); };
    // A card starts at any "A｜"-style line, NOT at a blank line: splitting on
    // "\n\n" depended on the note author's blank-line habits, and the missing
    // blank line before B｜ once rendered B glued inside A's card.
    let mut heading_lines: Vec<&str> = Vec::new();
    let mut cards: Vec<Vec<&str>> = Vec::new();
    for line in title.lines() {
        let mut ch = line.chars();
        let starts_card = matches!(ch.next(), Some(c) if c.is_ascii_uppercase())
            && ch.next() == Some('｜');
        if starts_card {
            cards.push(vec![line]);
        } else if !line.trim().is_empty() {
            match cards.last_mut() {
                Some(card) => card.push(line),
                None => heading_lines.push(line),
            }
        }
    }
    if cards.is_empty() { return String::new(); }
    let heading = heading_lines.join("\n");
    let label = t("companionOptions", lang);
    let map = companion_map(plan, lang);
    let mut out = format!("<section class=\"companion-options\"><h2>{}</h2><p class=\"companion-hint\">{}</p>{}<div class=\"companion-grid\">", esc(label), esc(&heading), map);
    for card in &cards {
        let name = card[0];
        let details = card[1..].iter().map(|&l| esc(l)).collect::<Vec<_>>().join("<br>");
        // The note section's leading letter ties it to its structured option.
        let letter = name.split('｜').next().unwrap_or("").trim().to_string();
        out.push_str(&format!("<article class=\"companion-card\"><h3>{}</h3><p>{}</p>", esc(name), details));
        if let Some(o) = plan
            .companion_options
            .iter()
            .find(|o| o.key == letter)
        {
            let chips: Vec<String> = o
                .stops
                .iter()
                .map(|st| format!("<span class=\"stop-chip\">{}</span>", esc(&st.label)))
                .collect();
            out.push_str(&format!(
                "<div class=\"companion-stops\">{}</div>",
                chips.join("<span class=\"stop-arrow\">→</span>")
            ));
            out.push_str(&companion_card_frame(o, &letter, lang));
        }
        out.push_str("</article>");
    }
    out.push_str("</div></section>");
    out
}

/// A 3:2 Web-Mercator frame around a point set (10% padding, aspect locked
/// for a 600x400 export, floor keeps coincident stops readable). Shared by
/// the companion overlay map and the per-option card frames.
struct Frame {
    la_min: f64,
    la_max: f64,
    lo_min: f64,
    lo_max: f64,
}

impl Frame {
    fn around(points: &[(f64, f64)]) -> Frame {
        let (mut la_min, mut la_max) = (f64::MAX, f64::MIN);
        let (mut lo_min, mut lo_max) = (f64::MAX, f64::MIN);
        for (la, lo) in points {
            la_min = la_min.min(*la);
            la_max = la_max.max(*la);
            lo_min = lo_min.min(*lo);
            lo_max = lo_max.max(*lo);
        }
        let c_la = (la_min + la_max) / 2.0;
        let aspect = 1.5 / c_la.to_radians().cos();
        let span_la = ((la_max - la_min) * 1.2).max((lo_max - lo_min) * 1.2 / aspect).max(0.02);
        let span_lo = span_la * aspect;
        let c_lo = (lo_min + lo_max) / 2.0;
        Frame {
            la_min: c_la - span_la / 2.0,
            la_max: c_la + span_la / 2.0,
            lo_min: c_lo - span_lo / 2.0,
            lo_max: c_lo + span_lo / 2.0,
        }
    }
    fn pos(&self, la: f64, lo: f64) -> (f64, f64) {
        let merc = |x: f64| (std::f64::consts::PI / 4.0 + x.to_radians() / 2.0).tan().ln();
        let x = (lo - self.lo_min) / (self.lo_max - self.lo_min) * 100.0;
        let y = (merc(self.la_max) - merc(la)) / (merc(self.la_max) - merc(self.la_min)) * 100.0;
        (x.clamp(0.0, 100.0), y.clamp(0.0, 100.0))
    }
    fn bbox(&self) -> String {
        format!(
            "{:.6},{:.6},{:.6},{:.6}",
            self.lo_min, self.la_min, self.lo_max, self.la_max
        )
    }
}

/// Per-option route frame inside a companion card: THIS option's Day-shape —
/// the thing the shared overlay map cannot give each option on its own.
/// Needs ≥2 geocoded stops; NULL-coord stops join the chips, never the frame.
fn companion_card_frame(o: &crate::model::CompanionOption, tag: &str, lang: &str) -> String {
    let pts: Vec<(f64, f64)> = o
        .stops
        .iter()
        .filter_map(|s| Some((s.lat?, s.lon?)))
        .collect();
    if pts.len() < 2 { return String::new(); }
    let frame = Frame::around(&pts);
    let mut out = format!(
        "<div class=\"companion-map-frame companion-card-map\"><img class=\"companion-map-img\" loading=\"lazy\" alt=\"\" \
         src=\"https://server.arcgisonline.com/ArcGIS/rest/services/World_Street_Map/MapServer/export?bbox={}&bboxSR=4326&size=600,400&format=png&f=image\">\
         <svg class=\"companion-map-routes\" viewBox=\"0 0 100 100\" preserveAspectRatio=\"none\">",
        frame.bbox()
    );
    let pts_svg: Vec<String> = o
        .stops
        .iter()
        .filter_map(|s| {
            let (la, lo) = (s.lat?, s.lon?);
            let (x, y) = frame.pos(la, lo);
            Some(format!("{x:.2},{y:.2}"))
        })
        .collect();
    if pts_svg.len() >= 2 {
        out.push_str(&format!(
            "<polyline class=\"route-{}\" points=\"{}\"/>",
            tag.to_lowercase(),
            pts_svg.join(" ")
        ));
    }
    out.push_str("</svg>");
    for s in &o.stops {
        let (Some(sla), Some(slo)) = (s.lat, s.lon) else { continue };
        let (x, y) = frame.pos(sla, slo);
        let side = if x > 78.0 { " companion-map-stop--left" } else { "" };
        let vert = if y > 82.0 { " companion-map-stop--above" } else { "" };
        out.push_str(&format!(
            "<span class=\"companion-map-stop stop-{}{side}{vert}\" style=\"left:{x:.2}%;top:{y:.2}%\">{}<em>{}</em></span>",
            tag.to_lowercase(),
            esc(tag),
            esc(&s.label)
        ));
    }
    let _ = lang;
    out.push_str("</div><p class=\"companion-card-credit\">© Esri</p>");
    out
}

/// Static companion map: ONE ArcGIS frame with both options' routes overlaid
/// in option colors (A = accent blue, B = amber). Needs ≥2 geocoded stops
/// across options or it renders nothing — a frame with one dot is a
/// placeholder, not information (same judgment as the candidate minimap's
/// no-coords-no-render). NULL-coord stops stay chip-only in the cards. Stops
/// shared by both options merge into one combined A·B pin. Percent coords
/// ride Web Mercator — the same math as the candidate minimap; no client JS.
/// Options beyond the first two render text-only (A/B is the real case).
fn companion_map(plan: &Plan, lang: &str) -> String {
    let opts: Vec<&crate::model::CompanionOption> =
        plan.companion_options.iter().take(2).collect();
    if opts.is_empty() { return String::new(); }
    // Tag from the option's OWN key (set-companion-route accepts A-Z): option
    // B alone must render as B, not as A because of its position.
    let tags: Vec<String> = opts
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let k = o.key.trim();
            if k.len() == 1 && k.chars().all(|c| c.is_ascii_uppercase()) {
                k.to_string()
            } else {
                ["A", "B"][i].to_string()
            }
        })
        .collect();
    let mut geocoded: Vec<(f64, f64)> = Vec::new();
    for o in &opts {
        for s in &o.stops {
            if let (Some(la), Some(lo)) = (s.lat, s.lon) {
                geocoded.push((la, lo));
            }
        }
    }
    let distinct: std::collections::HashSet<(i64, i64)> = geocoded
        .iter()
        .map(|(la, lo)| ((la * 1e4).round() as i64, (lo * 1e4).round() as i64))
        .collect();
    if distinct.len() < 2 { return String::new(); }

    let (mut la_min, mut la_max) = (f64::MAX, f64::MIN);
    let (mut lo_min, mut lo_max) = (f64::MAX, f64::MIN);
    for (la, lo) in &geocoded {
        la_min = la_min.min(*la); la_max = la_max.max(*la);
        lo_min = lo_min.min(*lo); lo_max = lo_max.max(*lo);
    }
    let c_lat = (la_min + la_max) / 2.0;
    let aspect = 1.5 / c_lat.to_radians().cos();
    // 10% padding each side, then enforce the 3:2 frame aspect.
    let span_la = ((la_max - la_min) * 1.2).max((lo_max - lo_min) * 1.2 / aspect).max(0.02);
    let span_lo = span_la * aspect;
    let c_lo = (lo_min + lo_max) / 2.0;
    let (la_min, la_max) = (c_lat - span_la / 2.0, c_lat + span_la / 2.0);
    let (lo_min, lo_max) = (c_lo - span_lo / 2.0, c_lo + span_lo / 2.0);

    // ArcGIS renders Web Mercator, so y is not linear in latitude.
    let merc = |x: f64| (std::f64::consts::PI / 4.0 + x.to_radians() / 2.0).tan().ln();
    let pos = |p_la: f64, p_lo: f64| -> (f64, f64) {
        let x = (p_lo - lo_min) / span_lo * 100.0;
        let y = (merc(la_max) - merc(p_la)) / (merc(la_max) - merc(la_min)) * 100.0;
        (x.clamp(0.0, 100.0), y.clamp(0.0, 100.0))
    };
    let bbox = format!("{:.6},{:.6},{:.6},{:.6}", lo_min, la_min, lo_max, la_max);
    let alt = t("companionMapAlt", lang);
    let mut out = format!(
        "<div class=\"companion-map-frame\"><img class=\"companion-map-img\" loading=\"lazy\" \
         alt=\"{alt}\" \
         src=\"https://server.arcgisonline.com/ArcGIS/rest/services/World_Street_Map/MapServer/export?bbox={bbox}&bboxSR=4326&size=600,400&format=png&f=image\">\
         <svg class=\"companion-map-routes\" viewBox=\"0 0 100 100\" preserveAspectRatio=\"none\">"
    );
    for (i, o) in opts.iter().enumerate() {
        let pts: Vec<String> = o
            .stops
            .iter()
            .filter_map(|s| {
                let (la, lo) = (s.lat?, s.lon?);
                let (x, y) = pos(la, lo);
                Some(format!("{x:.2},{y:.2}"))
            })
            .collect();
        if pts.len() >= 2 {
            out.push_str(&format!(
                "<polyline class=\"route-{}\" points=\"{}\"/>",
                tags[i].to_lowercase(),
                pts.join(" ")
            ));
        }
    }
    out.push_str("</svg>");
    // Combined pins: stops round-shared across options label A·B, not stacked.
    // Grid ~11m (1e4) — fine enough that distinct stops don't collide, coarse
    // enough that the same stop geocoded twice still merges.
    let mut pins: Vec<(i64, i64, String, String)> = Vec::new(); // (la*1e4, lo*1e4, tags, label)
    for (i, o) in opts.iter().enumerate() {
        for s in &o.stops {
            let (Some(sla), Some(slo)) = (s.lat, s.lon) else { continue };
            let key = ((sla * 1e4).round() as i64, (slo * 1e4).round() as i64);
            let tag = &tags[i];
            if let Some(e) = pins.iter_mut().find(|(a, b, _, _)| *a == key.0 && *b == key.1) {
                if !e.2.contains(tag) {
                    e.2.push_str(&format!("·{tag}"));
                    e.3.push_str(t("labelJoin", lang));
                    e.3.push_str(&s.label);
                }
            } else {
                pins.push((key.0, key.1, tag.clone(), s.label.clone()));
            }
        }
    }
    for (pla, plo, tag, label) in &pins {
        let (x, y) = pos(*pla as f64 / 1e4, *plo as f64 / 1e4);
        let class = if tag.contains('·') { "ab".to_string() } else { tag.to_lowercase() };
        // Edge flips: the <em> label must not clip against the frame.
        let side = if x > 78.0 { " companion-map-stop--left" } else { "" };
        let vert = if y > 82.0 { " companion-map-stop--above" } else { "" };
        out.push_str(&format!(
            "<span class=\"companion-map-stop stop-{class}{side}{vert}\" style=\"left:{x:.2}%;top:{y:.2}%\">{}<em>{}</em></span>",
            esc(tag),
            esc(label)
        ));
    }
    out.push_str("</div>");
    out.push_str("<p class=\"companion-map-caption\">");
    for (i, o) in opts.iter().enumerate() {
        out.push_str(&format!(
            "<span class=\"swatch swatch-{}\"></span>{}",
            tags[i].to_lowercase(),
            crate::i18n::tf("quotedTitle", lang, &[("title", &esc(&o.title))])
        ));
    }
    out.push_str(&format!(" · {} · © Esri</p>", t("companionUnconfirmed", lang)));
    out
}

/// AI-recommended provenance badge. Empty string for confirmed/any non-ai_recommended source, so
/// callers can append it unconditionally. Bilingual: ZH default, English on ?lang=en.
pub fn ai_rec_badge(source: &str, lang: &str) -> String {
    if source != "ai_recommended" {
        return String::new();
    }
    format!("<span class=\"ai-rec-badge\">{}</span>", esc(t("aiRecBadge", lang)))
}

/// Escape text for HTML TEXT content and DOUBLE-QUOTED attribute values only.
/// (Escapes & < > ". Not safe for single-quoted attrs, unquoted attrs, URLs, or
/// JS/CSS contexts — build those from trusted components instead.)
/// Escape ONCE — never double-escape (the old TS bug rendered `&amp;amp;`).
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Percent-encode a string for a URL path/query component (RFC 3986 unreserved
/// set passes through; space → `%20`; every other byte → `%XX` over its UTF-8
/// bytes). Single shared implementation — do NOT re-roll this per module.
pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "%20".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Escape a URL for a double-quoted HTML attribute (href/src).
/// Neutralizes attribute-breaking chars (" < > space) via percent-encoding but
/// does NOT touch `&`, so query strings (`?q=a&z=15`) survive intact.
/// Use this for URLs; use esc() for text/non-URL attribute values.
pub fn esc_url_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("%22"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            ' ' => out.push_str("%20"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escapes_ampersand_once() {
        assert_eq!(esc("Museum & Art"), "Museum &amp; Art");
        assert!(!esc("Museum & Art").contains("amp;amp;"));
    }
    #[test]
    fn esc_url_attr_preserves_ampersand_neutralizes_quotes() {
        assert_eq!(esc_url_attr("https://x/?q=a&z=15"), "https://x/?q=a&z=15"); // & preserved
        assert_eq!(esc_url_attr("https://x/?q=\"a\""), "https://x/?q=%22a%22"); // quote neutralized
        assert!(!esc_url_attr("a b").contains(' ')); // space encoded
    }

    #[test]
    fn page_shell_has_notranslate_and_lang() {
        let html = page("My Trip", "<p>hi</p>", "zh");
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("lang=\"zh-TW\""));
        assert!(html.contains("content=\"notranslate\""));
        assert!(html.contains("<title>My Trip</title>"));
        assert!(html.contains("<p>hi</p>"));
    }

    #[test]
    fn page_shell_en_lang() {
        let html = page("Trip", "", "en");
        assert!(html.contains("lang=\"en\""));
    }

    #[test]
    fn render_plan_includes_summary_map_and_days() {
        use crate::model::{Day, Plan};
        let plan = Plan {
            plan_id: "okinawa-2026".into(),
            display_name: "Okinawa".into(),
            days: vec![Day {
                day_number: 1,
                date: "2026-06-21".into(),
                day_type: "arrival".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let map_status = map::MapStatus {
            plan: Some("etag1".into()),
            plan_logistics: Some("etag3".into()),
            plan_excursion: None,
            plan_airport: None,
            days: [(1i64, Some("etag2".into()))].into_iter().collect(),
            day_logistics: std::collections::HashMap::new(),
            day_away: std::collections::HashMap::new(),
            legends: [
                (
                    "plan.png".to_string(),
                    vec![map::LegendStop {
                        seq: 1,
                        label: "首里城".into(),
                        lat: 26.2186,
                        lon: 127.7196,
                        ..Default::default()
                    }],
                ),
                (
                    "day-1.png".to_string(),
                    vec![map::LegendStop {
                        seq: 2,
                        label: "国際通り".into(),
                        lat: 26.2125,
                        lon: 127.6809,
                        ..Default::default()
                    }],
                ),
            ]
            .into_iter()
            .collect(),
        };
        let html = render_plan(&plan, "en", None, &map_status, "");
        assert!(html.contains("booking-summary"));
        assert!(html.contains("/map/okinawa-2026/plan.png"));
        assert!(html.contains("Day 1"));
        // Plan map renders ABOVE the booking summary (its own frame, not in the
        // summary's dashed box).
        let map_pos = html.find("planmap").expect("planmap img missing");
        let summary_pos = html.find("booking-summary").expect("summary missing");
        assert!(
            map_pos < summary_pos,
            "plan map must render before the booking summary"
        );
        // Summary carries the dashed-frame class.
        assert!(html.contains("booking-summary summary-box"));
        // Legends thread through to the plan map and the day map from MapStatus.
        assert!(html.contains(">首里城</a>"), "plan legend missing: {html}");
        assert!(html.contains(">国際通り</a>"), "day legend missing: {html}");
        let plan_leg = html.find("首里城").expect("plan legend");
        let day_leg = html.find("国際通り").expect("day legend");
        let plan_map = html.find("plan.png").expect("plan map img");
        let day_map = html.find("day-1.png").expect("day map img");
        assert!(plan_leg > plan_map, "plan legend sits under its map");
        assert!(day_leg > day_map, "day legend sits under its map");
    }

    #[test]
    fn render_plan_puts_weather_overview_after_summary_before_day_cards() {
        use crate::model::{Day, Plan};
        let plan = Plan {
            plan_id: "kyoto-nov-2026".into(),
            display_name: "Kyoto".into(),
            days: vec![Day {
                day_number: 1,
                date: "2026-11-14".into(),
                day_type: "full".into(),
                weather_code: Some(0),
                temp_low_c: Some(12.0),
                temp_high_c: Some(20.0),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render_plan(&plan, "zh", None, &map::MapStatus::default(), "");
        let summary = html.find("booking-summary").expect("summary");
        let overview = html
            .find("<details class=\"weather-overview\" id=\"sec-weather\" data-block=\"sec-weather\" open>")
            .expect("weather overview");
        let day_card = html.find("<section class=\"day ").expect("day card");
        assert!(summary < overview && overview < day_card, "{html}");
        assert!(html.contains("<td class=\"wx-desc\">\u{2600}\u{FE0F} 晴</td>"), "{html}");

        // No weather on any day → no overview block at all.
        let bare = Plan {
            days: vec![Day { day_number: 1, date: "2026-11-14".into(), ..Default::default() }],
            ..Default::default()
        };
        let html = render_plan(&bare, "zh", None, &map::MapStatus::default(), "");
        // (The inlined stylesheet names the class, so look for the markup.)
        assert!(!html.contains("<details class=\"weather-overview\""), "{html}");
        assert!(!html.contains("<h2>天氣總覽</h2>"), "{html}");
    }

    #[test]
    fn render_plan_puts_meal_overview_after_weather_before_day_cards() {
        use crate::model::{Day, Meal, Plan, Session};
        let noon = |text: &str| Session {
            session_type: "noon".into(),
            meals: vec![Meal { text: text.into(), source: "confirmed".into() }],
            ..Default::default()
        };
        let plan = Plan {
            plan_id: "kyoto-nov-2026".into(),
            display_name: "Kyoto".into(),
            days: vec![Day {
                day_number: 1,
                date: "2026-11-14".into(),
                day_type: "full".into(),
                weather_code: Some(0),
                temp_low_c: Some(12.0),
                temp_high_c: Some(20.0),
                sessions: vec![noon("湯豆腐｜奧丹：南禪寺參道｜map:奧丹")],
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render_plan(&plan, "zh", None, &map::MapStatus::default(), "");
        let weather = html
            .find("<details class=\"weather-overview\" id=\"sec-weather\" data-block=\"sec-weather\" open>")
            .expect("weather overview");
        let meals = html
            .find("<details class=\"meal-overview\" id=\"sec-meals\" data-block=\"sec-meals\" open><summary><h2>用餐總覽</h2></summary>")
            .expect("meal overview");
        let day_card = html.find("<section class=\"day ").expect("day card");
        assert!(weather < meals && meals < day_card, "{html}");
        assert!(html.contains("<td class=\"ml-main\" data-label=\"主選\">湯豆腐｜奧丹</td>"), "{html}");

        // No weather: the meal overview still sits between the summary and the
        // day cards — where the weather overview would be.
        let no_weather = Plan {
            days: vec![Day {
                day_number: 1,
                date: "2026-11-14".into(),
                day_type: "full".into(),
                sessions: vec![noon("Lunch")],
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render_plan(&no_weather, "en", None, &map::MapStatus::default(), "");
        assert!(!html.contains("<details class=\"weather-overview\""), "{html}");
        let summary = html.find("booking-summary").expect("summary");
        let meals = html.find("<details class=\"meal-overview\" id=\"sec-meals\"").expect("meal overview");
        let day_card = html.find("<section class=\"day ").expect("day card");
        assert!(summary < meals && meals < day_card, "{html}");
        assert!(html.contains("<h2>Meal overview</h2>"), "{html}");

        // No meals on any day → no meal overview block at all.
        let bare = Plan {
            days: vec![Day { day_number: 1, date: "2026-11-14".into(), ..Default::default() }],
            ..Default::default()
        };
        let html = render_plan(&bare, "zh", None, &map::MapStatus::default(), "");
        // (The inlined stylesheet names the class, so look for the markup.)
        assert!(!html.contains("<details class=\"meal-overview\""), "{html}");
        assert!(!html.contains("<h2>用餐總覽</h2>"), "{html}");
    }

    /// A plan with every block: documents, weather, meals, two main days (day 1
    /// has a variant, day_number 3).
    fn full_nav_plan() -> crate::model::Plan {
        use crate::model::{Day, Meal, Plan, PlanDocument, Session};
        let noon = Session {
            session_type: "noon".into(),
            meals: vec![Meal { text: "湯豆腐".into(), source: "confirmed".into() }],
            ..Default::default()
        };
        Plan {
            plan_id: "kyoto-nov-2026".into(),
            display_name: "Kyoto".into(),
            documents: vec![PlanDocument {
                doc_key: "flight".into(),
                kind: "flight_eticket".into(),
                label: "Flight".into(),
                label_zh: "機票".into(),
                r2_key: "kyoto-nov-2026/doc-flight.pdf".into(),
                days: vec![1],
                ..Default::default()
            }],
            days: vec![
                Day {
                    day_number: 1,
                    date: "2026-11-14".into(),
                    day_type: "full".into(),
                    theme: "Kyoto highlights".into(),
                    theme_zh: "京都巡禮".into(),
                    weather_code: Some(0),
                    temp_low_c: Some(12.0),
                    temp_high_c: Some(20.0),
                    sessions: vec![noon],
                    variants: vec![Day {
                        day_number: 3,
                        date: "2026-11-14".into(),
                        variant_of: Some(1),
                        variant_label_zh: "雨天方案".into(),
                        variant_label: "Rainy day".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                Day {
                    day_number: 2,
                    date: "2026-11-15".into(),
                    day_type: "departure".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn toc_tops_the_page_and_links_every_rendered_block_in_page_order() {
        let html = render_plan(&full_nav_plan(), "zh", None, &map::MapStatus::default(), "");
        let toc = html.find("<nav class=\"page-toc\" id=\"toc\" aria-label=\"目錄\">").expect("toc");
        // First thing in <body>, above the plan map and the booking summary.
        assert!(html.contains("<body data-plan-id=\"kyoto-nov-2026\" data-view=\"viewer\"><nav class=\"page-toc\""), "{html}");
        let summary = html.find("id=\"sec-summary\"").expect("summary");
        let weather = html.find("<details class=\"weather-overview\" id=\"sec-weather\"").expect("weather");
        assert!(toc < summary && summary < weather, "{html}");
        let nav = &html[toc..html[toc..].find("</nav>").unwrap() + toc];
        let hrefs: Vec<&str> = nav.split("href=\"#").skip(1).map(|s| s.split('"').next().unwrap()).collect();
        assert_eq!(hrefs, ["sec-docs", "sec-summary", "sec-weather", "sec-meals", "day-1", "day-2"], "{nav}");
        // Every link resolves to an id on the page.
        for id in &hrefs {
            assert_eq!(html.matches(&format!(" id=\"{id}\"")).count(), 1, "id {id}: {html}");
        }
        // Day chips: heading with the short dated weekday, then the theme; the
        // variant (internal day 3) has no entry and its number never shows.
        assert!(nav.contains(
            "<li><a class=\"toc-day\" href=\"#day-1\"><span class=\"toc-day-n\">第 1 天 · 11/14（六）</span><span class=\"toc-day-theme\">京都巡禮</span></a></li>"
        ), "{nav}");
        assert!(nav.contains("<span class=\"toc-day-n\">第 2 天 · 11/15（日）</span></a>"), "{nav}");
        assert!(!html.contains("第 3 天"), "{html}");
        // Buttons are served hidden (the script un-hides them); the floating
        // back link follows the nav.
        assert!(nav.contains("<button type=\"button\" class=\"toc-btn\" data-blocks=\"close\" hidden>全部收合</button>"), "{nav}");
        assert!(nav.contains("<button type=\"button\" class=\"toc-btn\" data-blocks=\"open\" hidden>全部展開</button>"), "{nav}");
        assert!(html.contains("</nav><a class=\"toc-fab\" href=\"#toc\">↑ 目錄</a>"), "{html}");

        let en = render_plan(&full_nav_plan(), "en", None, &map::MapStatus::default(), "");
        assert!(en.contains("<h2>Contents</h2>"), "{en}");
        assert!(en.contains(">Collapse all</button>") && en.contains(">Expand all</button>"), "{en}");
        assert!(en.contains("<span class=\"toc-day-n\">Day 1 · Sat 11/14</span><span class=\"toc-day-theme\">Kyoto highlights</span>"), "{en}");
        assert!(en.contains(">↑ Contents</a>"), "{en}");
    }

    #[test]
    fn toc_lists_only_blocks_that_render() {
        use crate::model::{Day, Plan};
        let plan = Plan {
            plan_id: "p".into(),
            days: vec![Day { day_number: 1, ..Default::default() }],
            ..Default::default()
        };
        let html = render_plan(&plan, "zh", None, &map::MapStatus::default(), "");
        let toc = html.find("<nav class=\"page-toc\"").expect("toc");
        let nav = &html[toc..html[toc..].find("</nav>").unwrap() + toc];
        let hrefs: Vec<&str> = nav.split("href=\"#").skip(1).map(|s| s.split('"').next().unwrap()).collect();
        assert_eq!(hrefs, ["sec-summary", "day-1"], "{nav}");
        // No date → the chip is just the day label; no theme → no theme span.
        assert!(nav.contains("<span class=\"toc-day-n\">第 1 天</span></a>"), "{nav}");
        assert!(!nav.contains("toc-day-theme"), "{nav}");
    }

    #[test]
    fn every_info_block_is_an_open_details_with_a_data_block_key() {
        let html = render_plan(&full_nav_plan(), "zh", None, &map::MapStatus::default(), "");
        for (outer, key) in [
            ("<section class=\"plan-documents summary-box\" id=\"sec-docs\">", "sec-docs"),
            ("<section class=\"booking-summary summary-box\" id=\"sec-summary\">", "sec-summary"),
            ("<section class=\"day day-full\" data-day=\"1\" id=\"day-1\">", "day-1"),
            ("<section class=\"day day-departure\" data-day=\"2\" id=\"day-2\">", "day-2"),
        ] {
            assert!(
                html.contains(&format!("{outer}<details data-block=\"{key}\" open><summary><h2>")),
                "{key}: {html}"
            );
        }
        assert!(html.contains("<details class=\"weather-overview\" id=\"sec-weather\" data-block=\"sec-weather\" open><summary><h2>"), "{html}");
        assert!(html.contains("<details class=\"meal-overview\" id=\"sec-meals\" data-block=\"sec-meals\" open><summary><h2>"), "{html}");
        assert_eq!(html.matches("<details data-block=").count() + 2, html.matches(" data-block=\"").count(), "{html}");
        assert_eq!(html.matches(" data-block=\"").count(), 6, "{html}");
        // The variant stays collapsed and is not itself a data-block.
        assert!(html.contains("<details class=\"day-variant\" data-variant-of=\"1\" data-variant-day=\"3\"><summary"), "{html}");
    }

    #[test]
    fn block_memory_script_is_emitted_once_on_viewer_and_owner_pages() {
        let plan = full_nav_plan();
        let status = map::MapStatus::default();
        let viewer = render_plan(&plan, "zh", Some("tok"), &status, "");
        let owner = render_plan(&plan, "zh", None, &status, "<div class=\"owner-chrome\"></div>");
        for (name, html, scripts) in [("viewer", &viewer, 1), ("owner", &owner, 2)] {
            assert_eq!(html.matches("'trip-blocks:'").count(), 1, "{name}: {html}");
            assert_eq!(html.matches("<script>").count(), scripts, "{name}: {html}");
            // At the very end of <body>, after every block it wires.
            assert!(html.ends_with("</script></body></html>"), "{name}");
            assert!(!html.contains("<script src"), "{name}");
        }
        // Storage access is guarded and the verify-site hook stops saving.
        assert!(BLOCK_MEMORY_SCRIPT.contains("try{ls=window.localStorage;"));
        assert!(BLOCK_MEMORY_SCRIPT.contains("try{ls.setItem("));
        assert!(BLOCK_MEMORY_SCRIPT.contains("data-no-block-memory"));
        // Back/Forward to a fragment reveals it; print opens every block; a
        // variant is keyed by its own day number, not its sibling position.
        assert!(BLOCK_MEMORY_SCRIPT.contains("addEventListener('hashchange'"));
        assert!(BLOCK_MEMORY_SCRIPT.contains("addEventListener('beforeprint'"));
        assert!(BLOCK_MEMORY_SCRIPT.contains("addEventListener('afterprint'"));
        assert!(BLOCK_MEMORY_SCRIPT.contains("b='v'+(el.getAttribute('data-variant-day')"));
        assert_eq!(BLOCK_MEMORY_SCRIPT.matches("localStorage").count(), 1);
        // ES5: no arrow functions, let/const or template literals.
        for bad in ["=>", "let ", "const ", "`"] {
            assert!(!BLOCK_MEMORY_SCRIPT.contains(bad), "{bad}");
        }
        // Pages other than the plan page carry no script.
        assert!(!page("t", "x", "zh").contains("<script"));
    }

    #[test]
    fn render_plan_marks_its_plan_and_view_on_body() {
        use crate::model::Plan;
        let plan = Plan {
            plan_id: "jiufen-2026".into(),
            display_name: "九份".into(),
            ..Default::default()
        };
        let status = map::MapStatus::default();
        let viewer = render_plan(&plan, "zh", Some("tok"), &status, "");
        assert!(viewer.contains("<body data-plan-id=\"jiufen-2026\" data-view=\"viewer\">"), "{viewer}");
        let owner = render_plan(&plan, "zh", None, &status, "<div class=\"owner-chrome\"></div>");
        assert!(owner.contains("<body data-plan-id=\"jiufen-2026\" data-view=\"owner\">"), "{owner}");
        // The value is escaped like any attribute; other pages keep a bare <body>.
        assert_eq!(plan_body_attrs("a\"b", ""), " data-plan-id=\"a&quot;b\" data-view=\"viewer\"");
        assert!(page("t", "x", "zh").contains("</head><body>x</body>"));
    }

    #[test]
    fn render_plan_omits_logistics_map_for_domestic() {
        // Domestic plan: no flights AND no logistics PNG in R2 → the slot must
        // not render at all (it used to show a 地圖尚未產生 placeholder that
        // wasted the vertical space a self-drive trip does not need).
        use crate::model::Plan;
        let plan = Plan {
            plan_id: "jiufen-2026".into(),
            display_name: "九份".into(),
            ..Default::default()
        };
        let map_status = map::MapStatus {
            plan: Some("etag1".into()),
            ..Default::default()
        };
        let html = render_plan(&plan, "zh", None, &map_status, "");
        assert!(html.contains("/map/jiufen-2026/plan.png"));
        assert!(
            !html.contains("plan-logistics.png"),
            "domestic plan must not render the logistics map slot: {html}"
        );
        assert!(!html.contains("地圖尚未產生"), "{html}");
    }

    #[test]
    fn render_plan_keeps_logistics_slot_when_flights_exist() {
        // A Japan plan with flights keeps the logistics slot even before the PNG
        // exists (so the missing-map state is visible, prompting a snapshot run)
        // — it renders as a compact inset, not a second full-size map.
        use crate::model::Plan;
        use crate::turso::Row;
        let mut f = Row::new();
        f.insert("flight_number".into(), serde_json::json!("CI120"));
        f.insert("departure_code".into(), serde_json::json!("TPE"));
        f.insert("arrival_code".into(), serde_json::json!("KIX"));
        let plan = Plan {
            plan_id: "kyoto-2026".into(),
            display_name: "Kyoto".into(),
            flights: vec![f],
            ..Default::default()
        };
        let map_status = map::MapStatus::default();
        let html = render_plan(&plan, "zh", None, &map_status, "");
        // The placeholder branch emits no <img>, so assert on the slot's caption
        // and its inset frame — not on the PNG URL.
        assert!(
            html.contains("住宿與機場"),
            "flights exist → logistics slot must render: {html}"
        );
        assert!(html.contains("map-frame--inset"), "{html}");
    }

    #[test]
    fn render_plan_puts_excursion_inset_beside_logistics() {
        use crate::model::Plan;
        let plan = Plan {
            plan_id: "osaka-nov-2026".into(),
            display_name: "Kyoto".into(),
            ..Default::default()
        };
        let map_status = map::MapStatus {
            plan: Some("p".into()),
            plan_logistics: Some("l".into()),
            plan_excursion: Some("e".into()),
            ..Default::default()
        };
        let html = render_plan(&plan, "zh", None, &map_status, "");
        let row = html.split("<div class=\"map-insets\">").nth(1).expect("insets row");
        let row = row.split("</figure></div>").next().unwrap_or("");
        let exc = row.find("plan-excursion.png").expect("excursion inset in the row");
        let log = row.find("plan-logistics.png").expect("logistics inset in the row");
        assert!(exc < log, "day trip first, then hotels/airports: {row}");

        // No excursion PNG → no day-trip slot and no placeholder for it.
        let none = render_plan(&plan, "zh", None, &map::MapStatus { plan_logistics: Some("l".into()), ..Default::default() }, "");
        assert!(!none.contains("plan-excursion.png"));
        assert!(!none.contains("一日遊"), "{none}");
    }

    #[test]
    fn render_plan_puts_airport_inset_right_after_the_excursion_inset() {
        use crate::model::Plan;
        let plan = Plan {
            plan_id: "osaka-nov-2026".into(),
            display_name: "Kyoto".into(),
            ..Default::default()
        };
        let map_status = map::MapStatus {
            plan: Some("p".into()),
            plan_logistics: Some("l".into()),
            plan_excursion: Some("e".into()),
            plan_airport: Some("a".into()),
            legends: [(
                "plan-airport.png".to_string(),
                vec![map::LegendStop {
                    seq: 1,
                    label: "関西空港".into(),
                    lat: 34.43,
                    lon: 135.23,
                    day_number: Some(1),
                    pin_no: Some(1),
                    color: Some("#e6194b".into()),
                }],
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let html = render_plan(&plan, "zh", None, &map_status, "");
        let row = html.split("<div class=\"map-insets\">").nth(1).expect("insets row");
        let row = row.split("</figure></div>").next().unwrap_or("");
        let exc = row.find("/map/osaka-nov-2026/plan-excursion.png?v=e").expect("excursion inset");
        let air = row.find("/map/osaka-nov-2026/plan-airport.png?v=a").expect("airport inset");
        let log = row.find("/map/osaka-nov-2026/plan-logistics.png?v=l").expect("logistics inset");
        assert!(exc < air && air < log, "day trip, airport days, then hotels/airports: {row}");
        // Its grouped legend lands under the airport inset, before the logistics one.
        let leg = row.find(">関西空港</a>").expect("airport legend");
        assert!(air < leg && leg < log, "{row}");
        assert!(row[air..log].contains("第 1 天"), "{row}");

        // An airport inset alone still opens the insets row (no flights, no
        // logistics map, no excursion).
        let alone = map::MapStatus { plan_airport: Some("a".into()), ..Default::default() };
        let html = render_plan(&plan, "en", None, &alone, "");
        let row = html.split("<div class=\"map-insets\">").nth(1).expect("insets row");
        assert!(row.contains("plan-airport.png?v=a") && row.contains("Airport days"), "{row}");
        assert!(!html.contains("plan-logistics.png") && !html.contains("plan-excursion.png"), "{html}");

        // No airport PNG → no slot and no placeholder.
        let none = render_plan(&plan, "zh", None, &map::MapStatus { plan_logistics: Some("l".into()), ..Default::default() }, "");
        assert!(!none.contains("plan-airport.png"), "{none}");
        assert!(!none.contains("機場來回"), "{none}");
    }

    #[test]
    fn render_plan_puts_day_away_map_right_after_the_day_logistics_map() {
        use crate::model::{Day, Plan};
        let day = |n| Day {
            day_number: n,
            date: format!("2026-06-2{n}"),
            ..Default::default()
        };
        let plan = Plan {
            plan_id: "okinawa-2026".into(),
            display_name: "Okinawa".into(),
            days: vec![day(1), day(2)],
            ..Default::default()
        };
        let both = |v: &str| [(1i64, Some(v.to_string())), (2i64, Some(v.to_string()))].into_iter().collect();
        let map_status = map::MapStatus {
            days: both("d"),
            day_logistics: both("l"),
            day_away: [(1i64, Some("a1".to_string())), (2i64, None)].into_iter().collect(),
            legends: [(
                "day-1-away.png".to_string(),
                vec![map::LegendStop { seq: 1, label: "紅樹林 淡水".into(), lat: 25.15, lon: 121.46, ..Default::default() }],
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let html = render_plan(&plan, "zh", None, &map_status, "");
        let day1 = html.split("<section class=\"day").nth(1).expect("day 1 card");
        let stack = day1.split("<div class=\"day-map-stack\">").nth(1).expect("day 1 map stack");
        let stack = stack.split("</figure></div>").next().unwrap_or("");
        let log = stack.find("/map/okinawa-2026/day-1-logistics.png?v=l").expect("logistics map in the stack");
        let away = stack.find("/map/okinawa-2026/day-1-away.png?v=a1").expect("away map in the stack");
        assert!(log < away, "away map directly after the hotel round trip: {stack}");
        // Nothing between the two figures: the away figure opens right where the
        // logistics one closes.
        let between = &stack[log..away];
        assert_eq!(between.matches("<figure").count(), 1, "only the away figure opens in between: {between}");
        assert!(between.contains("</figure><figure class=\"map-frame map-frame--day-logistics map-frame--day-away\""), "{between}");
        // The day-1-away.png legend rows land under the away map, not elsewhere.
        assert!(stack[away..].contains("紅樹林 淡水"), "{stack}");
        assert_eq!(html.matches("紅樹林 淡水").count(), 1, "{html}");
        assert!(stack.contains("第 1 天・本區以外的行程"), "{stack}");

        // Day 2 has no away map: no slot, no placeholder.
        assert!(!html.contains("day-2-away.png"), "{html}");
        assert_eq!(html.matches("本區以外的行程").count(), 2, "alt + caption of day 1 only");

        let en = render_plan(&plan, "en", None, &map_status, "");
        assert!(en.contains("Day 1 beyond the area"), "{en}");
    }

    #[test]
    fn variant_block_renders_collapsed_under_main() {
        use crate::model::{Day, Plan, RouteSegment, Session};
        let plan = Plan {
            plan_id: "kyoto-nov-2026".into(),
            display_name: "Kyoto".into(),
            days: vec![
                Day {
                    day_number: 1,
                    date: "2026-11-14".into(),
                    day_type: "full".into(),
                    variant_label: "Rainy-day plan".into(),
                    variant_label_zh: "雨天方案".into(),
                    theme_zh: "車站周邊".into(),
                    variants: vec![Day {
                        day_number: 3,
                        variant_of: Some(1),
                        variant_label: "Sunny-day plan".into(),
                        variant_label_zh: "晴天方案".into(),
                        theme_zh: "大原散策".into(),
                        weather_label: "variant weather must not render".into(),
                        landmarks: vec!["三千院".into()],
                        route_segments: vec![RouteSegment {
                            from_place: "京都站".into(),
                            to_place: "大原".into(),
                            ..Default::default()
                        }],
                        sessions: vec![Session {
                            session_type: "morning".into(),
                            focus_zh: "寺院散策".into(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                Day {
                    day_number: 2,
                    date: "2026-11-15".into(),
                    day_type: "full".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let html = render_plan(&plan, "zh", None, &map::MapStatus::default(), "");
        assert_eq!(html.matches("<section class=\"day ").count(), 2, "{html}");
        let day1_open = "<section class=\"day day-full\" data-day=\"1\" id=\"day-1\">";
        let day1_start = html.find(day1_open).expect("day 1 card");
        let day1_end = day1_start
            + html[day1_start..]
                .find("</section>")
                .expect("day 1 section end")
                + "</section>".len();
        let day1 = &html[day1_start..day1_end];
        assert!(
            day1.contains("<span class=\"day-option-label\">雨天方案</span>"),
            "{day1}"
        );
        assert!(day1.contains("<details class=\"day-variant\" data-variant-of=\"1\" data-variant-day=\"3\"><summary class=\"day-variant-summary\">替代方案：晴天方案</summary><div class=\"day-variant-body\">"), "{day1}");
        // Collapsed by default: no `open` attribute on the variant block.
        assert!(!day1.contains("data-variant-of=\"1\" open"), "{day1}");
        // The block sits in the day card after the main day's own content.
        let details_at = day1.find("<details class=\"day-variant\"").unwrap();
        assert!(day1.find("車站周邊").unwrap() < details_at, "{day1}");
        for section in html.split("<section class=\"day ").skip(1) {
            let section = section.split("</section>").next().expect("day section end");
            if !section.starts_with("day-full\" data-day=\"1\" id=\"day-1\">") {
                assert!(!section.contains("data-variant-of="), "{section}");
            }
        }
        let body = day1.split("<div class=\"day-variant-body\">").nth(1).unwrap();
        let map = body.find("day-map-stack").unwrap();
        let landmark = body.find("day-landmarks").unwrap();
        let route = body.find("route-block").unwrap();
        let session = body.find("session-morning").unwrap();
        assert!(map < landmark && landmark < route && route < session, "{body}");
        assert!(body.contains("大原散策"), "{body}");
        assert!(body.contains("三千院"), "{body}");
        assert!(body.contains("寺院散策"), "{body}");
        assert!(!html.contains("第 3 天"), "{html}");
        assert!(!html.contains("variant weather must not render"), "{html}");
    }

    #[test]
    fn variant_map_caption_uses_main_day() {
        use crate::model::{Day, Plan};
        let variant = |label: &str| Day {
            day_number: 3,
            variant_of: Some(1),
            variant_label: label.into(),
            variant_label_zh: "晴天方案".into(),
            ..Default::default()
        };
        let plan = |label: &str| Plan {
            plan_id: "kyoto-nov-2026".into(),
            display_name: "Kyoto".into(),
            days: vec![Day {
                day_number: 1,
                date: "2026-11-14".into(),
                day_type: "full".into(),
                variant_label: "Rainy-day plan".into(),
                variant_label_zh: "雨天方案".into(),
                variants: vec![variant(label)],
                ..Default::default()
            }],
            ..Default::default()
        };
        let map_status = map::MapStatus {
            days: [(3, Some("v1".into()))].into_iter().collect(),
            day_logistics: [(3, Some("v2".into()))].into_iter().collect(),
            day_away: [(3, Some("v3".into()))].into_iter().collect(),
            legends: [(
                "day-3.png".into(),
                vec![map::LegendStop {
                    seq: 1,
                    label: "三千院".into(),
                    lat: 35.12,
                    lon: 135.84,
                    day_number: Some(3),
                    ..Default::default()
                }],
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let zh = render_plan(&plan("Sunny-day plan"), "zh", None, &map_status, "");
        assert!(zh.contains("data-map-key=\"day-3.png\""), "{zh}");
        assert!(zh.contains("src=\"/map/kyoto-nov-2026/day-3.png?v=v1\""), "{zh}");
        assert!(zh.contains("alt=\"第 1 天 · 晴天方案\""), "{zh}");
        assert!(zh.contains("<figcaption>第 1 天 · 晴天方案 "), "{zh}");
        assert!(zh.contains("alt=\"第 1 天 · 晴天方案・旅社來回\""), "{zh}");
        assert!(zh.contains("<figcaption>第 1 天 · 晴天方案・旅社來回 "), "{zh}");
        assert!(zh.contains("alt=\"第 1 天 · 晴天方案・本區以外的行程\""), "{zh}");
        assert!(zh.contains("<figcaption>第 1 天 · 晴天方案・本區以外的行程 "), "{zh}");
        assert!(zh.contains("<div class=\"leg-day\">第 1 天 · 晴天方案</div>"), "{zh}");
        assert!(!zh.contains("第 3 天"), "{zh}");

        let en = render_plan(&plan("Sunny-day plan"), "en", None, &map_status, "");
        assert!(en.contains("alt=\"Day 1 · Sunny-day plan\""), "{en}");
        assert!(en.contains("<figcaption>Day 1 · Sunny-day plan "), "{en}");
        assert!(en.contains("alt=\"Day 1 · Sunny-day plan · hotel round trip\""), "{en}");
        assert!(en.contains("<figcaption>Day 1 · Sunny-day plan · hotel round trip "), "{en}");
        assert!(en.contains("alt=\"Day 1 · Sunny-day plan · beyond the area\""), "{en}");
        assert!(en.contains("<figcaption>Day 1 · Sunny-day plan · beyond the area "), "{en}");
        assert!(en.contains("<div class=\"leg-day\">Day 1 · Sunny-day plan</div>"), "{en}");
        assert!(!en.contains("Day 3"), "{en}");
        let fallback = render_plan(&plan(""), "en", None, &map_status, "");
        assert!(fallback.contains("alt=\"Day 1 · 晴天方案\""), "{fallback}");
        assert!(fallback.contains("alt=\"Day 1 · 晴天方案 · hotel round trip\""), "{fallback}");
        assert!(fallback.contains("alt=\"Day 1 · 晴天方案 · beyond the area\""), "{fallback}");
    }

    #[test]
    fn companion_options_render_as_separate_cards() {
        use crate::model::{Activity, Day, Plan, Session};
        let plan = Plan {
            days: vec![Day {
                sessions: vec![Session {
                    activities: vec![Activity {
                        title: "同行方案（可選擇，尚未確認）\n\nA｜北海岸\n路線：野柳 → 金山\n\nB｜坪林\n路線：茶博館 → 老街".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render_companion_options(&plan, "zh");
        assert!(html.contains("同行方案 · 可選擇"), "{html}");
        assert!(html.contains("<h3>A｜北海岸</h3>"), "{html}");
        assert!(html.contains("<h3>B｜坪林</h3>"), "{html}");
    }

    /// The real jiufen-2026 note has NO blank line before B｜ — single \n
    /// throughout. Cards must still split, or B renders glued inside A.
    #[test]
    fn companion_options_split_without_blank_lines() {
        use crate::model::{Activity, Day, Plan, Session};
        let plan = Plan {
            days: vec![Day {
                sessions: vec![Session {
                    activities: vec![Activity {
                        title: "同行方案（可選擇，尚未確認）\nA｜北海岸\n路線：漫海聽風 → 野柳\nB｜坪林\n路線：茶博館 → 老街".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render_companion_options(&plan, "zh");
        assert!(html.contains("<h3>A｜北海岸</h3>"), "{html}");
        assert!(html.contains("<h3>B｜坪林</h3>"), "{html}");
        // B's 路線 must land in B's card, not appended under A's heading.
        let a_end = html.find("<h3>B｜坪林</h3>").unwrap();
        assert!(html[..a_end].contains("野柳"), "{html}");
        assert!(!html[..a_end].contains("茶博館"), "{html}");
    }

    /// Structured stops (set-companion-route) turn on the overlay map: one
    /// ArcGIS frame, both routes in option colors, shared stops merged into
    /// one A·B pin, © Esri in the caption.
    #[test]
    fn companion_map_overlays_both_routes_when_structured() {
        use crate::model::{CompanionOption, CompanionStop};
        let mut plan = note_only_plan();
        plan.companion_options = vec![
            CompanionOption {
                key: "A".into(),
                title: "北海岸".into(),
                stops: vec![
                    CompanionStop { label: "漫海聽風".into(), lat: Some(25.1219), lon: Some(121.8614) },
                    CompanionStop { label: "野柳海洋世界".into(), lat: Some(25.2051), lon: Some(121.6914) },
                    CompanionStop { label: "台2線海岸".into(), lat: None, lon: None },
                ],
            },
            CompanionOption {
                key: "B".into(),
                title: "坪林".into(),
                stops: vec![
                    CompanionStop { label: "漫海聽風".into(), lat: Some(25.1219), lon: Some(121.8614) },
                    CompanionStop { label: "坪林老街".into(), lat: Some(24.9368), lon: Some(121.7096) },
                ],
            },
        ];
        let html = render_companion_options(&plan, "zh");
        assert!(html.contains("companion-map-frame"), "{html}");
        assert!(html.contains("route-a"), "{html}");
        assert!(html.contains("route-b"), "{html}");
        // 漫海聽風 appears in BOTH options → one combined A·B pin, never stacked.
        assert!(html.contains("stop-ab"), "{html}");
        assert!(html.contains("A·B"), "{html}");
        assert!(html.contains("© Esri"), "{html}");
    }

    /// Below two geocoded stops across ALL options a frame would be a
    /// placeholder, not information — the section still renders, map doesn't.
    #[test]
    fn companion_map_skipped_below_two_geocoded_stops() {
        use crate::model::{CompanionOption, CompanionStop};
        let mut plan = note_only_plan();
        plan.companion_options = vec![CompanionOption {
            key: "A".into(),
            title: "北海岸".into(),
            stops: vec![
                CompanionStop { label: "漫海聽風".into(), lat: Some(25.1219), lon: Some(121.8614) },
                CompanionStop { label: "台2線海岸".into(), lat: None, lon: None },
            ],
        }];
        let html = render_companion_options(&plan, "zh");
        assert!(!html.contains("companion-map-frame"), "{html}");
        assert!(html.contains("companion-options"), "{html}");
    }

    /// Pins/lines/swatches follow the option's OWN key: option B alone must
    /// render as B, not as A because of its list position (kimi P3-5).
    #[test]
    fn companion_map_tags_follow_option_keys() {
        use crate::model::{CompanionOption, CompanionStop};
        let mut plan = note_only_plan();
        plan.companion_options = vec![CompanionOption {
            key: "B".into(),
            title: "坪林".into(),
            stops: vec![
                CompanionStop { label: "漫海聽風".into(), lat: Some(25.1219), lon: Some(121.8614) },
                CompanionStop { label: "坪林老街".into(), lat: Some(24.937), lon: Some(121.7117) },
            ],
        }];
        let html = render_companion_options(&plan, "zh");
        // Class-boundary match: "stop-arrow" must not read as stop-a.
        assert!(html.contains("route-b\""), "{html}");
        assert!(html.contains("stop-b\""), "{html}");
        assert!(!html.contains("route-a\""), "{html}");
        assert!(!html.contains("stop-a\""), "{html}");
    }

    /// Two options sharing ONE point (same coords) is a one-dot frame — the
    /// gate counts DISTINCT points, not stops (kimi P3-6).
    #[test]
    fn companion_map_gate_counts_distinct_points() {
        use crate::model::{CompanionOption, CompanionStop};
        let mut plan = note_only_plan();
        let stop = CompanionStop { label: "漫海聽風".into(), lat: Some(25.1219), lon: Some(121.8614) };
        plan.companion_options = vec![
            CompanionOption { key: "A".into(), title: "甲".into(), stops: vec![stop.clone()] },
            CompanionOption { key: "B".into(), title: "乙".into(), stops: vec![stop] },
        ];
        let html = render_companion_options(&plan, "zh");
        assert!(!html.contains("companion-map-frame"), "{html}");
    }

    /// ?lang=en localizes the caption (kimi P3-8).
    #[test]
    fn companion_map_caption_localizes_to_english() {
        use crate::model::{CompanionOption, CompanionStop};
        let mut plan = note_only_plan();
        plan.companion_options = vec![
            CompanionOption {
                key: "A".into(),
                title: "北海岸".into(),
                stops: vec![
                    CompanionStop { label: "漫海聽風".into(), lat: Some(25.1219), lon: Some(121.8614) },
                    CompanionStop { label: "野柳".into(), lat: Some(25.2051), lon: Some(121.6914) },
                ],
            },
            CompanionOption {
                key: "B".into(),
                title: "坪林".into(),
                stops: vec![
                    CompanionStop { label: "漫海聽風".into(), lat: Some(25.1219), lon: Some(121.8614) },
                    CompanionStop { label: "坪林老街".into(), lat: Some(24.937), lon: Some(121.7117) },
                ],
            },
        ];
        let zh = render_companion_options(&plan, "zh");
        assert!(zh.contains("尚未確認，僅供討論 · © Esri"), "{zh}");
        let en = render_companion_options(&plan, "en");
        assert!(en.contains("unconfirmed, for discussion · © Esri"), "{en}");
        assert!(!en.contains("尚未確認，僅供討論"), "{en}");
    }

    /// A stop pinned at the frame's right edge flips its label leftward so it
    /// doesn't clip (kimi P3-9; same judgment as cand-minimap-poi--left).
    #[test]
    fn companion_map_flips_edge_labels_inward() {
        use crate::model::{CompanionOption, CompanionStop};
        let mut plan = note_only_plan();
        plan.companion_options = vec![CompanionOption {
            key: "A".into(),
            title: "北海岸".into(),
            stops: vec![
                // Westmost → left side of frame.
                CompanionStop { label: "淡水".into(), lat: Some(25.1727), lon: Some(121.4377) },
                // Eastmost → right edge → x=100 → label must flip left.
                CompanionStop { label: "漫海聽風".into(), lat: Some(25.1219), lon: Some(121.8614) },
            ],
        }];
        let html = render_companion_options(&plan, "zh");
        assert!(
            html.contains("companion-map-stop--left"),
            "eastmost stop's label must flip: {html}"
        );
    }

    /// Each option card shows its OWN Day-shape: a scannable stop-chip
    /// sequence plus a per-option route frame — B's day must be visible as a
    /// day, not only as three place names in a prose line (user: B方案的
    /// Day 2 完全看不見). NULL-coord stops join the chips, never the frame.
    #[test]
    fn companion_cards_show_stop_chips_and_own_route_frames() {
        use crate::model::{CompanionOption, CompanionStop};
        let mut plan = note_only_plan();
        plan.companion_options = vec![
            CompanionOption {
                key: "A".into(),
                title: "北海岸".into(),
                stops: vec![
                    CompanionStop { label: "漫海聽風".into(), lat: Some(25.1219), lon: Some(121.8614) },
                    CompanionStop { label: "台2線海岸".into(), lat: None, lon: None },
                    CompanionStop { label: "淡水".into(), lat: Some(25.1727), lon: Some(121.4377) },
                ],
            },
            CompanionOption {
                key: "B".into(),
                title: "坪林".into(),
                stops: vec![
                    CompanionStop { label: "漫海聽風".into(), lat: Some(25.1219), lon: Some(121.8614) },
                    CompanionStop { label: "坪林茶業博物館".into(), lat: Some(24.9341), lon: Some(121.7126) },
                    CompanionStop { label: "淡水".into(), lat: Some(25.1727), lon: Some(121.4377) },
                ],
            },
        ];
        let html = render_companion_options(&plan, "zh");
        // Chips: the full stop sequence is scannable per card — including the
        // NULL-coord 台2線海岸, which must not vanish from the sequence.
        assert!(html.contains("stop-chip"), "{html}");
        assert!(html.contains("stop-chip\">台2線海岸</span>"), "{html}");
        // Per-option route frames inside the cards: one each for A and B.
        assert_eq!(
            html.match_indices("companion-card-map").count(),
            2,
            "each option card carries its own route frame: {html}"
        );
        // Each frame composites the ArcGIS basemap itself → its own © Esri.
        assert_eq!(html.match_indices("companion-card-credit\">© Esri").count(), 2, "{html}");
    }
}

/// The real jiufen note fixture shared by the companion tests.
#[cfg(test)]
fn note_only_plan() -> crate::model::Plan {
    use crate::model::{Activity, Day, Plan, Session};
    Plan {
        days: vec![Day {
            sessions: vec![Session {
                activities: vec![Activity {
                    title: "同行方案（可選擇，尚未確認）\n\nA｜北海岸\n路線：野柳 → 金山\n\nB｜坪林\n路線：茶博館 → 老街".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
}
