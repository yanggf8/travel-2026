//! Booking summary: flights + hotel + airport transfers.
//! Renders from raw Turso `Row`s (BTreeMap<String, Value>). Turso returns every
//! scalar as a JSON STRING, so read fields via `rs()` (as_str → owned String).
//!
//! Regression note: the old TS worker rendered transfers as "—" (it read the
//! wrong/absent columns). Here transfers MUST render selected_route + price.

use super::{esc, esc_url_attr, urlencode};
use crate::i18n::t;
use crate::model::Plan;
use crate::turso::Row;

/// Read a Turso row field as an owned String (scalars come back as JSON strings).
fn rs(row: &Row, k: &str) -> String {
    row.get(k)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Group an integer with thousands separators (e.g. 27888 → "27,888"), matching the
/// TS worker's `Number.toLocaleString()` for prices. Handles negatives defensively.
fn group_thousands(n: i64) -> String {
    let neg = n < 0;
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    let len = digits.len();
    for (idx, ch) in digits.chars().enumerate() {
        if idx > 0 && (len - idx) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}

/// True when an image_url should NOT be loaded as an <img> — empty or the legacy
/// via.placeholder.com host which Chrome now reports as broken (0x0). In that case
/// we render a CSS placeholder div instead of an external request.
fn is_placeholder_image(url: &str) -> bool {
    let t = url.trim();
    t.is_empty() || t.contains("via.placeholder.com")
}

/// Join non-empty parts with a single space (skips blanks so we don't emit "  ").
fn join_parts(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

fn fit_source_label(source: &str) -> &str {
    match source {
        "liontravel" => "雄獅",
        "travel4u" => "山富",
        "besttour" => "喜鴻",
        "lifetour" => "五福",
        "settour" => "東南",
        "liangyou" => "良友",
        other => other,
    }
}

/// Prefer the page language, then the other, so a ZH-only note still shows on `?lang=en`.
fn fit_text(lang: &str, zh: &str, en: &str) -> String {
    if lang == "en" {
        if !en.is_empty() {
            en.to_string()
        } else {
            zh.to_string()
        }
    } else if !zh.is_empty() {
        zh.to_string()
    } else {
        en.to_string()
    }
}

fn fit_currency(currency: &str) -> &str {
    if currency.is_empty() {
        "TWD"
    } else {
        currency
    }
}

/// Lowest / price-delta against the cheapest shown offer. Skipped when fewer
/// than two priced offers share one currency — a single price, or a mixed-currency
/// list, is not a comparison.
fn fit_price_badge(
    fit: &crate::model::FitOffer,
    all: &[crate::model::FitOffer],
) -> Option<FitPriceBadge> {
    let priced: Vec<&crate::model::FitOffer> = all.iter().filter(|o| o.price > 0).collect();
    if priced.len() < 2 {
        return None;
    }
    let cur = fit_currency(&priced[0].currency);
    if !priced.iter().all(|o| fit_currency(&o.currency) == cur) {
        return None;
    }
    if fit.price <= 0 || fit_currency(&fit.currency) != cur {
        return None;
    }
    let min = priced.iter().map(|o| o.price).min()?;
    if fit.price == min {
        Some(FitPriceBadge::Lowest)
    } else {
        Some(FitPriceBadge::Delta {
            amount: fit.price - min,
            currency: cur.to_string(),
        })
    }
}

enum FitPriceBadge {
    Lowest,
    Delta { amount: i64, currency: String },
}

fn fit_badge_html(
    lang: &str,
    recommended: bool,
    booked: bool,
    price: Option<FitPriceBadge>,
) -> String {
    let mut badges = String::new();
    if recommended {
        let label = match (booked, lang == "en") {
            (true, true) => "Booked",
            (true, false) => "已訂",
            (false, true) => "Recommended",
            (false, false) => "推薦",
        };
        badges.push_str(&format!(
            "<span class=\"fit-badge fit-badge-rec\">{}</span>",
            esc(label)
        ));
    }
    match price {
        Some(FitPriceBadge::Lowest) => {
            let label = if lang == "en" { "Lowest" } else { "最低價" };
            badges.push_str(&format!(
                "<span class=\"fit-badge fit-badge-low\">{}</span>",
                esc(label)
            ));
        }
        Some(FitPriceBadge::Delta { amount, currency }) => {
            let label = if lang == "en" {
                format!("+{currency} {} / person", group_thousands(amount))
            } else {
                format!("貴 {currency} {}／人", group_thousands(amount))
            };
            badges.push_str(&format!(
                "<span class=\"fit-badge fit-badge-delta\">{}</span>",
                esc(&label)
            ));
        }
        None => {}
    }
    if badges.is_empty() {
        String::new()
    } else {
        format!("<div class=\"fit-badges\">{badges}</div>")
    }
}

/// Wrap flight display text in a Google search link (opens new tab). Port of the
/// TS worker's `flightLink` (render.ts:979-982): the search query is the
/// PERCENT-ENCODED flight number ONLY (`number.trim()`), never the airline. With
/// an empty flight number → plain escaped text (no anchor), mirroring the TS guard.
fn flight_link(display_text: &str, flight_number: &str) -> String {
    if flight_number.trim().is_empty() {
        return esc(display_text);
    }
    let query = urlencode(flight_number.trim());
    let url = format!("https://www.google.com/search?q={}", query);
    format!(
        "<a href=\"{}\" target=\"_blank\" rel=\"noopener\" style=\"color:inherit;text-decoration:underline dotted;text-underline-offset:3px\">{}</a>",
        esc_url_attr(&url),
        esc(display_text),
    )
}

/// Render the hotel `notes` blob into grouped <ul> bullets.
///
/// Notes are newline-delimited. A line starting with `## ` opens a new group
/// (label = the rest after the marker, rendered in `.hotel-group-label`); every
/// following non-empty, non-`##` line becomes a `<li>` inside that group's
/// `<ul>`. Blank lines are skipped. Lines before any `## ` header (or notes with
/// no headers at all) fall into an implicit unlabeled leading group so legacy
/// flat notes still render as a bullet list. Every label and line is esc()'d.
fn render_notes(notes: &str) -> String {
    let mut h = String::new();
    let mut group_open = false; // a <ul> (with optional label div) is open
    let mut wrapper_open = false; // the <div class="hotel-group"> is open

    let close_group = |h: &mut String, group_open: &mut bool, wrapper_open: &mut bool| {
        if *group_open {
            h.push_str("</ul>");
            *group_open = false;
        }
        if *wrapper_open {
            h.push_str("</div>");
            *wrapper_open = false;
        }
    };

    for raw in notes.split('\n') {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(label) = line.strip_prefix("## ") {
            close_group(&mut h, &mut group_open, &mut wrapper_open);
            h.push_str("<div class=\"hotel-group\">");
            wrapper_open = true;
            h.push_str(&format!(
                "<div class=\"hotel-group-label\">{}</div>",
                esc(label.trim())
            ));
            h.push_str("<ul>");
            group_open = true;
        } else {
            // A fact line before any header → open an unlabeled leading group.
            if !group_open {
                if !wrapper_open {
                    h.push_str("<div class=\"hotel-group\">");
                    wrapper_open = true;
                }
                h.push_str("<ul>");
                group_open = true;
            }
            h.push_str(&format!("<li>{}</li>", esc(line)));
        }
    }
    close_group(&mut h, &mut group_open, &mut wrapper_open);
    h
}

/// Guest-rating chips, one per review source. Each keeps its own scale because
/// Booking.com scores out of 10 and Google out of 5 — normalizing them would
/// publish a number nobody actually gave.
fn ratings_row(ratings: &[crate::model::CandidateRating], lang: &str) -> String {
    if ratings.is_empty() {
        return String::new();
    }
    let mut h = String::from("<div class=\"candidate-ratings\">");
    for r in ratings {
        if r.scale <= 0.0 {
            continue;
        }
        let count = if r.review_count > 0 {
            let n = group_thousands(r.review_count);
            if lang == "en" {
                format!(" · {n} reviews")
            } else {
                format!(" · {n} 則")
            }
        } else {
            String::new()
        };
        h.push_str(&format!(
            "<span class=\"candidate-rating\"><b>{}</b>/{} {}{}</span>",
            esc(&trim_num(r.score)),
            esc(&trim_num(r.scale)),
            esc(&r.source),
            esc(&count)
        ));
    }
    h.push_str("</div>");
    h
}

/// "Booking.com 房價 · 2026-09-04 查 · 剩 1 間 · 9/28 前免費取消".
/// Every part is optional; an empty result renders nothing.
fn price_note(c: &crate::model::DomesticCandidate, lang: &str) -> String {
    let en = lang == "en";
    let mut parts: Vec<String> = Vec::new();
    if !c.price_source.is_empty() {
        parts.push(if en {
            format!("{} rate", c.price_source)
        } else {
            format!("{} 房價", c.price_source)
        });
    }
    // Stock is a SNAPSHOT, so it hangs off the check date instead of standing on its
    // own — "1 left" on its own reads as a live OTA counter.
    if !c.price_checked_at.is_empty() {
        let d = date_only(&c.price_checked_at);
        let stock = if c.rooms_left > 0 {
            if en {
                format!(" ({} left then)", c.rooms_left)
            } else {
                format!("（當時剩 {} 間）", c.rooms_left)
            }
        } else {
            String::new()
        };
        parts.push(if en {
            format!("checked {d}{stock}")
        } else {
            format!("{d} 查價{stock}")
        });
    } else if c.rooms_left > 0 {
        // No read date: say so rather than implying the count is current.
        parts.push(if en {
            format!("{} left (date unknown)", c.rooms_left)
        } else {
            format!("剩 {} 間（查價日不明）", c.rooms_left)
        });
    }
    if !c.free_cancel_until.is_empty() {
        let d = date_only(&c.free_cancel_until);
        parts.push(if en {
            format!("free cancellation until {d}")
        } else {
            format!("{d} 前免費取消")
        });
    } else if !c.price_source.is_empty() {
        // We looked up a rate but recorded no cancellation deadline. Saying nothing
        // let a reader assume this stay cancels as freely as the ones beside it —
        // the same trap the missing breakfast tag set.
        parts.push(t("cancelUnknown", lang).to_string());
    }
    if parts.is_empty() {
        return String::new();
    }
    format!(
        "<div class=\"candidate-note\">{}</div>",
        esc(&parts.join(" · "))
    )
}

/// `2026-09-04 01:51:21` -> `2026-09-04`. Leaves an already-bare date alone.
fn date_only(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or(s)
}

/// Per-candidate location minimap: ArcGIS World Street Map static export,
/// CENTERED on the trip's 旅遊地 (the itinerary's hub cluster, e.g. 九份) with
/// the hub→stay distance spanning a THIRD of the frame (span = 3× the stay's
/// dominant-axis offset; was 4×/quarter line — user request 2026-09-28: 二點
/// 距離可以佔 1/3, a slightly tighter, larger-reading crop) so every card reads
/// "this far, this direction from where we're going" at a glance. Zoom adapts
/// to the stay's distance (a 0.2 km-in-cluster stay gets a street-scale map; a
/// 4 km outlier pulls out), and stops beyond the frame are simply not drawn
/// (the caption's nearest-stop distance covers them) — including ALL stops
/// made every card a 40 km north-coast overview where the cluster-vs-stay
/// contrast vanished.
/// Pins are positioned by Mercator math over the known bbox (the ArcGIS
/// `marker` param is silently ignored, verified). Never requests OSM raster
/// tiles (tile policy) — same sanctioned ArcGIS static basemap the route
/// snapshots composite. Keep the © Esri credit whenever this renders.
fn candidate_minimap(
    c: &crate::model::DomesticCandidate,
    lat: f64,
    lon: f64,
    stops: &[crate::model::PoiStop],
    lang: &str,
) -> String {
    // ---- frame: either destination-centered quarter geometry (v3) or, with
    // no stops at all, the v1 tight stay-centered fallback. The lon/lat span
    // ratio is always the 3:2 image aspect (1.5/cos(lat)) so the export
    // maps the bbox exactly onto the image — the percentage-positioned pins
    // then land on the right pixels.
    let (lat_min, lat_max, lon_min, lon_max, visible, hub): (
        f64,
        f64,
        f64,
        f64,
        Vec<&crate::model::PoiStop>,
        Option<&crate::model::PoiStop>,
    ) = if stops.is_empty() {
        let lat_span = 0.0032; // ~355 m tall, enough street context
        let lon_span = lat_span * 1.5 / lat.to_radians().cos();
        (
            lat - lat_span / 2.0,
            lat + lat_span / 2.0,
            lon - lon_span / 2.0,
            lon + lon_span / 2.0,
            Vec::new(),
            None,
        )
        } else {
            let hub = &stops[hub_stop(stops)];
            let (c_lat, c_lon) = (hub.lat, hub.lon);
            let aspect = 1.5 / c_lat.to_radians().cos();
            // Span so the hub→stay offset lands at 1/3 of the frame along its
            // DOMINANT axis (full span = 3× the offset — user request 2026-09-28,
            // 二點距離佔 1/3; was 4× = quarter line). Floor keeps a coincident
            // stay readable at street scale.
            let d_lat = (lat - c_lat).abs();
            let d_lon = (lon - c_lon).abs();
            let lat_span = (3.0 * d_lat.max(d_lon / aspect)).max(0.004);
            let lon_span = lat_span * aspect;
            let (la_min, la_max) = (c_lat - lat_span / 2.0, c_lat + lat_span / 2.0);
            let (lo_min, lo_max) = (c_lon - lon_span / 2.0, c_lon + lon_span / 2.0);
            // Off-frame stops are omitted, NOT clamped — a dot pinned to the
            // edge would claim a position the stay-geometry never gave it.
            let visible = stops
                .iter()
                .filter(|s| {
                    s.lat >= la_min && s.lat <= la_max && s.lon >= lo_min && s.lon <= lo_max
                })
                .collect();
            (la_min, la_max, lo_min, lo_max, visible, Some(hub))
        };
    let lat_span = lat_max - lat_min;
    let lon_span = lon_max - lon_min;
    let bbox = format!(
        "{:.6},{:.6},{:.6},{:.6}",
        lon_min, lat_min, lon_max, lat_max
    );
    let img = format!(
        // 600,400 = the 3:2 frame at 2× — same geometry, crisp labels (the
        // 300,200 export upscaled on the card rendered blurry place names).
        "https://server.arcgisonline.com/ArcGIS/rest/services/World_Street_Map/MapServer/export?bbox={}&bboxSR=4326&size=600,400&format=png&f=image",
        bbox
    );
    // ---- Mercator positioning: ArcGIS renders in Web Mercator, so y is not
    // linear in latitude. y(lat) = ln(tan(π/4 + φ/2)).
    let merc = |la: f64| (std::f64::consts::PI / 4.0 + la.to_radians() / 2.0).tan().ln();
    let y_top = merc(lat_max);
    let y_span = merc(lat_max) - merc(lat_min);
    let pos = |la: f64, lo: f64| -> (f64, f64) {
        let x = (lo - lon_min) / lon_span * 100.0;
        let y = (y_top - merc(la)) / y_span * 100.0;
        (x.clamp(0.0, 100.0), y.clamp(0.0, 100.0))
    };
    // ---- 旅遊地 marker: 🚩 on the hub stop. The bbox is centered on it, so
    // the flag sits at the image center — the visible answer to 中間是旅遊
    // 目的地. A dashed hub→stay connector (percent-coordinate SVG, no client
    // JS) makes the pair read as a 交通路線圖.
    let (px, py) = pos(lat, lon);
    let mut pins = String::new();
    if let Some(h) = hub {
        let (hx, hy) = pos(h.lat, h.lon);
        pins.push_str(&format!(
            "<span class=\"cand-minimap-hub\" style=\"left:{hx:.2}%;top:{hy:.2}%\">\u{1F6A9}\
             <em class=\"cand-minimap-hub-label\">{}</em></span>",
            esc(&h.label)
        ));
        pins.push_str(&format!(
            "<svg class=\"cand-minimap-route\" viewBox=\"0 0 100 100\" preserveAspectRatio=\"none\">\
             <line x1=\"{hx:.2}\" y1=\"{hy:.2}\" x2=\"{px:.2}\" y2=\"{py:.2}\" \
             vector-effect=\"non-scaling-stroke\" /></svg>"
        ));
    }
    // ---- stay pin (large 📍, tip on the point).
    pins.push_str(&format!(
        "<span class=\"cand-minimap-pin\" style=\"left:{px:.2}%;top:{py:.2}%\">\u{1F4CD}</span>"
    ));
    // ---- itinerary reference dots with labels (visible ones only). Labels
    // flip to the LEFT of the dot near the right edge (overflow:hidden would
    // clip them) and alternate above/below so near-coincident stops (九份老街
    // vs 九份住宿) don't overwrite each other.
    for (i, s) in visible.iter().enumerate() {
        if hub.is_some_and(|h| std::ptr::eq(*s, h)) {
            continue; // the hub stop is the 🚩 旅遊地, not a blue dot
        }
        let (x, y) = pos(s.lat, s.lon);
        let side = if x > 78.0 { " cand-minimap-poi--left" } else { "" };
        let vert = if i % 2 == 0 { " cand-minimap-poi--above" } else { "" };
        pins.push_str(&format!(
            "<span class=\"cand-minimap-poi{side}{vert}\" style=\"left:{x:.2}%;top:{y:.2}%\">\
             <i class=\"cand-minimap-dot\"></i><em class=\"cand-minimap-poi-label\">{}</em></span>",
            esc(&s.label)
        ));
    }
    // ---- caption: distance to the 旅遊地 hub — the same endpoint the dashed
    // connector draws to, so the number and the line tell ONE story. (With no
    // stops there is no hub and no distance note at all.)
    let dist_txt = match hub {
        Some(h) if lang == "en" => {
            format!("~{:.1} km from {}", distance_km(lat, lon, h.lat, h.lon), h.label)
        }
        Some(h) => format!(
            "距{} 約 {:.1} km",
            h.label,
            distance_km(lat, lon, h.lat, h.lon)
        ),
        None => String::new(),
    };
    let maps = format!("https://www.google.com/maps?q={:.6},{:.6}", lat, lon);
    let alt = if lang == "en" {
        format!("{} location map", c.hotel_name)
    } else {
        format!("{} 位置圖", c.hotel_name)
    };
    let caption = if dist_txt.is_empty() {
        format!("{} · \u{00A9} Esri", t("locationMap", lang))
    } else {
        format!("{} · {} · \u{00A9} Esri", t("locationMap", lang), dist_txt)
    };
    format!(
        "<div class=\"cand-minimap\">\
         <a class=\"cand-minimap-link\" href=\"{}\" target=\"_blank\" rel=\"noopener\" title=\"{}\">\
         <img class=\"cand-minimap-img\" src=\"{}\" alt=\"{}\" loading=\"lazy\" />{}</a>\
         <span class=\"cand-minimap-caption\">{}</span>\
         </div>",
        esc_url_attr(&maps),
        esc(t("openMap", lang)),
        esc_url_attr(&img),
        esc(&alt),
        pins,
        esc(&caption),
    )
}

/// The criteria line's date part, from the plan's own date anchors —
/// 「10/12（一）1 晚」 / "Mon 10/12 · 1 night". Empty when the plan has no
/// dates yet (nothing to state).
fn criteria_dates(plan: &crate::model::Plan, lang: &str) -> String {
    let parse = |d: &str| -> Option<(i64, i64, i64)> {
        let mut it = d.split('-');
        let y = it.next()?.parse().ok()?;
        let m = it.next()?.parse().ok()?;
        let day = it.next()?.parse().ok()?;
        Some((y, m, day))
    };
    let Some((y, m, day)) = parse(&plan.start_date) else {
        return String::new();
    };
    // days since the Unix epoch (Hinnant's civil_from_days inverse) → weekday;
    // 1970-01-01 was a Thursday, so (days + 4) mod 7 indexes 0=Sunday.
    let days = civil_days(y, m, day);
    let wd_idx = (days + 4).rem_euclid(7) as usize;
    let (wd_zh, wd_en) = [
        ("日", "Sun"),
        ("一", "Mon"),
        ("二", "Tue"),
        ("三", "Wed"),
        ("四", "Thu"),
        ("五", "Fri"),
        ("六", "Sat"),
    ][wd_idx];
    let nights = parse(&plan.end_date)
        .map(|(ey, em, ed)| civil_days(ey, em, ed) - days)
        .filter(|n| *n > 0);
    if lang == "en" {
        match nights {
            Some(n) => format!("{wd_en} {m}/{day} · {n} night{}", if n > 1 { "s" } else { "" }),
            None => format!("{wd_en} {m}/{day}"),
        }
    } else {
        match nights {
            Some(n) => format!("{m}/{day}（{wd_zh}）{n} 晚"),
            None => format!("{m}/{day}（{wd_zh}）"),
        }
    }
}

/// Days since 1970-01-01 for a civil (proleptic Gregorian) date.
fn civil_days(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The trip's 旅遊地: the stop with the most neighbors within 5 km (tie →
/// earliest) — where the itinerary CONCENTRATES (the 九份 base for jiufen).
/// Excursion stops (野柳/金山/淡水) must not drag the center off the base.
/// Returns its INDEX; the bbox is centered on the stop itself so the 🚩 sits
/// exactly at the image center and the hub→stay distance spans a third of the
/// frame.
fn hub_stop(stops: &[crate::model::PoiStop]) -> usize {
    let mut best = 0usize;
    let mut best_cnt = 0usize;
    for (i, s) in stops.iter().enumerate() {
        let cnt = stops
            .iter()
            .enumerate()
            .filter(|(j, t)| *j != i && distance_km(s.lat, s.lon, t.lat, t.lon) <= 5.0)
            .count();
        if cnt > best_cnt {
            best_cnt = cnt;
            best = i;
        }
    }
    best
}

/// Equirectangular distance in km (fine at Taiwan scale — minimap geometry only).
fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let dy = (lat2 - lat1) * 110.574;
    let dx = (lon2 - lon1) * 111.320 * lat1.to_radians().cos();
    (dx * dx + dy * dy).sqrt()
}

/// `9` not `9.0`, but `4.6` stays `4.6`.
fn trim_num(v: f64) -> String {
    if (v - v.round()).abs() < f64::EPSILON {
        format!("{}", v.round() as i64)
    } else {
        format!("{v}")
    }
}

pub fn render(plan: &Plan, lang: &str, token: Option<&str>) -> String {
    let mut h = String::new();
    // `summary-box` adds the dashed frame (user visibility request); the plan map
    // slot is rendered ABOVE this section (render_plan), never inside the frame.
    h.push_str("<section class=\"booking-summary summary-box\">");

    // Package offer (booking-summary package row) — the chosen plan_offer + its
    // selected-date price. Only shown when a package was selected (flight+hotel
    // booked-separately plans have no offer). Ported from render.ts:1078-1089.
    if let Some(offer) = &plan.offer {
        // Agoda is hotel-only; it must not become a package headline.
        if offer.source_id != "agoda" {
            let heading = t("package", lang);
            h.push_str(&format!("<h2>{}</h2>", esc(heading)));
            h.push_str("<div class=\"booking-grid\">");
            h.push_str("<div class=\"booking-item package\">");
            h.push_str("<span class=\"booking-icon\">📦</span>");
            h.push_str("<div class=\"booking-detail\">");
            let title = join_parts(&[offer.source_id.clone(), offer.product_code.clone()]);
            h.push_str(&format!(
                "<div class=\"booking-value\">{}</div>",
                esc(if title.is_empty() { "—" } else { &title })
            ));
            if offer.price > 0 {
                let cur = if offer.currency.is_empty() {
                    "TWD"
                } else {
                    &offer.currency
                };
                h.push_str(&format!(
                    "<div class=\"booking-sub\">{} {}{} ({} {})</div>",
                    esc(cur),
                    group_thousands(offer.price),
                    esc(t("perPerson", lang)),
                    esc(t("forTwo", lang)),
                    group_thousands(offer.price * 2),
                ));
            }
            h.push_str("</div></div>");
            h.push_str("</div>");
        }
    }

    // FIT alternatives are shown as a comparison row, never as the current
    // selection. Agoda/separate-booking stays remain in the normal hotel block.
    if !plan.fit_offers.is_empty() {
        // Once P4 is booked the comparison is history: the heading says so and the
        // single recommended pick (set-fit-note --recommend) reads as the booked one.
        let fit_booked = plan.p4_status == "booked";
        h.push_str(&format!(
            "<h2>{}</h2>",
            esc(match (fit_booked, lang == "en") {
                (true, true) => "FIT options (booked — pre-booking comparison)",
                (true, false) => "FIT 方案比較（已訂，以下為訂購前比價參考）",
                (false, true) => "FIT options (preferred comparison)",
                (false, false) => "FIT 方案比較（優先評估，非已訂）",
            })
        ));
        let compare = fit_text(lang, &plan.fit_compare_zh, &plan.fit_compare_en);
        if !compare.is_empty() {
            h.push_str(&format!(
                "<div class=\"fit-compare\">{}</div>",
                esc(&compare)
            ));
        }
        // Group by travel agency. Each agency is one collapsed disclosure so the
        // reader sees agency → flights → hotel in a stable, repeatable order.
        let mut groups: Vec<(String, Vec<&crate::model::FitOffer>)> = Vec::new();
        for fit in &plan.fit_offers {
            if let Some((_, offers)) = groups
                .iter_mut()
                .find(|(source, _)| source == &fit.source_id)
            {
                offers.push(fit);
            } else {
                groups.push((fit.source_id.clone(), vec![fit]));
            }
        }

        h.push_str("<div class=\"fit-agency-list\">");
        for (source_id, offers) in &groups {
            let source = if source_id.is_empty() {
                "—"
            } else {
                fit_source_label(source_id)
            };
            let recommended = offers.iter().any(|fit| fit.recommended);
            let rec_class = if recommended { " is-recommended" } else { "" };
            let header_badge = if recommended {
                format!(
                    "<span class=\"fit-badge fit-badge-rec\">{}</span>",
                    esc(match (fit_booked, lang == "en") {
                        (true, true) => "Booked",
                        (true, false) => "已訂",
                        (false, true) => "Recommended",
                        (false, false) => "推薦",
                    })
                )
            } else {
                String::new()
            };
            h.push_str(&format!("<details class=\"fit-agency{rec_class}\" open>"));
            h.push_str(&format!(
                "<summary><span class=\"fit-agency-heading\"><span class=\"fit-agency-chevron\" aria-hidden=\"true\">▶</span><span class=\"fit-agency-name\">{}</span>{header_badge}</span><span class=\"fit-agency-count\">{} {}</span></summary>",
                esc(source),
                offers.len(),
                esc(if lang == "en" { "offers" } else { "個方案" })
            ));
            h.push_str("<div class=\"fit-agency-body\">");
            for fit in offers {
                let hotel = if !fit.hotel_name.is_empty() {
                    &fit.hotel_name
                } else if !fit.title.is_empty() {
                    &fit.title
                } else {
                    "—"
                };
                let price = if fit.price > 0 {
                    format!(
                        "{} {}／人",
                        if fit.currency.is_empty() {
                            "TWD"
                        } else {
                            &fit.currency
                        },
                        group_thousands(fit.price)
                    )
                } else {
                    "價格未確認".to_string()
                };
                let mut date_text = if fit.departure_date.is_empty() {
                    "日期未確認".to_string()
                } else if fit.return_date.is_empty() {
                    format!("{} 起（回程未確認）", fit.departure_date)
                } else {
                    format!("{}–{}", fit.departure_date, fit.return_date)
                };
                if fit.nights > 0 {
                    date_text.push_str(&format!(" · {} 晚", fit.nights));
                }
                if !fit.availability.is_empty() {
                    date_text.push_str(&format!(" · {}", fit.availability));
                }

                h.push_str("<div class=\"fit-agency-offer\">");
                h.push_str(&format!("<div class=\"fit-detail-row\"><span class=\"fit-detail-label\">{}</span><span>{}</span></div>", esc(if lang == "en" { "Agency" } else { "旅行社" }), esc(source)));
                let flights = [
                    fit.airline.as_str(),
                    fit.flight_outbound.as_str(),
                    fit.flight_return.as_str(),
                ]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
                h.push_str(&format!("<div class=\"fit-detail-row\"><span class=\"fit-detail-label\">{}</span><span>{}</span></div>", esc(if lang == "en" { "Flights" } else { "班機" }), esc(if flights.is_empty() { "—" } else { &flights })));
                h.push_str(&format!("<div class=\"fit-detail-row\"><span class=\"fit-detail-label\">{}</span><span>{}</span></div>", esc(if lang == "en" { "Hotel" } else { "酒店" }), esc(hotel)));
                let room = fit_text(lang, &fit.room_zh, &fit.room_en);
                if !room.is_empty() {
                    h.push_str(&format!(
                        "<div class=\"fit-detail-row\"><span class=\"fit-detail-label\">{}</span><span>{}</span></div>",
                        esc(if lang == "en" { "Size" } else { "面積" }),
                        esc(&room)
                    ));
                }
                h.push_str(&format!("<div class=\"fit-detail-row\"><span class=\"fit-detail-label\">{}</span><span class=\"fit-detail-price\">{}</span></div>", esc(if lang == "en" { "Price" } else { "價格" }), esc(&price)));
                h.push_str(&fit_badge_html(
                    lang,
                    fit.recommended,
                    fit_booked,
                    fit_price_badge(fit, &plan.fit_offers),
                ));
                let reason = fit_text(lang, &fit.note_zh, &fit.note_en);
                if !reason.is_empty() {
                    h.push_str(&format!(
                        "<div class=\"fit-offer-reason\">{}</div>",
                        esc(&reason)
                    ));
                }
                h.push_str(&format!(
                    "<div class=\"fit-detail-meta\">{}</div>",
                    esc(&date_text)
                ));
                h.push_str("</div>");
            }
            h.push_str("</div></details>");
        }
        h.push_str("</div>");
        h.push_str(&format!("<div class=\"fit-offer-note\">{}</div>", esc(if lang == "en" { "Only FIT options with a morning outbound and an afternoon return are shown; dates may differ when the foliage outcome is comparable." } else { "只列上午去程、午後回程的 FIT；日期可以不同，但楓紅效果必須可比。未符合者不列入。" })));
    }

    // A separately booked flight + hotel is the current plan shape. Give the
    // combined choice one truthful name; Agoda remains only the hotel source.
    if plan.offer.as_ref().map(|o| o.source_id.as_str()) == Some("agoda") {
        h.push_str(&format!(
            "<h2>{}</h2>",
            esc(if lang == "en" {
                "Custom flight + hotel"
            } else {
                "自訂機酒"
            })
        ));
    }

    // Flights
    if !plan.flights.is_empty() {
        h.push_str(&format!("<h2>{}</h2>", esc(t("flights", lang))));
        h.push_str("<div class=\"booking-grid\">");
        for f in &plan.flights {
            let number = rs(f, "flight_number");
            let airline = rs(f, "airline");
            let dep = join_parts(&[
                rs(f, "departure_code"),
                rs(f, "departure_terminal"),
                rs(f, "departure_time"),
            ]);
            let arr = join_parts(&[
                rs(f, "arrival_code"),
                rs(f, "arrival_terminal"),
                rs(f, "arrival_time"),
            ]);
            let date = rs(f, "flight_date");
            h.push_str("<div class=\"booking-item flight\">");
            h.push_str("<span class=\"booking-icon\">✈️</span>");
            h.push_str("<div class=\"booking-detail\">");
            let display = join_parts(&[number.clone(), airline.clone()]);
            h.push_str(&format!(
                "<div class=\"booking-value\">{}</div>",
                flight_link(&display, &number)
            ));
            h.push_str(&format!(
                "<div class=\"booking-sub\">{} → {}</div>",
                esc(&dep),
                esc(&arr)
            ));
            if !date.is_empty() {
                h.push_str(&format!("<div class=\"booking-sub\">{}</div>", esc(&date)));
            }
            h.push_str("</div></div>");
        }
        h.push_str("</div>");
    }

    // Hotel
    if let Some(hotel) = &plan.hotel {
        let name_zh = rs(hotel, "name_zh");
        let name = if lang == "zh" && !name_zh.is_empty() {
            name_zh
        } else {
            rs(hotel, "name")
        };
        let check_in = rs(hotel, "check_in");
        let notes = rs(hotel, "notes");
        let voucher_url = rs(hotel, "voucher_url");
        h.push_str(&format!("<h2>{}</h2>", esc(t("hotel", lang))));
        h.push_str("<div class=\"booking-grid\">");
        h.push_str("<div class=\"booking-item hotel\">");
        h.push_str("<span class=\"booking-icon\">🏨</span>");
        h.push_str("<div class=\"booking-detail\">");
        h.push_str(&format!(
            "<div class=\"booking-value\">{}</div>",
            esc(&name)
        ));
        if !check_in.is_empty() {
            h.push_str(&format!(
                "<div class=\"booking-sub\">{}</div>",
                esc(&check_in)
            ));
        }
        // Hotel access lines (transit directions to the hotel) — TS rendered these as
        // a comma-joined access list (render.ts:1143). One labeled sub-line; skipped
        // when there are no rows.
        if !plan.hotel_access_lines.is_empty() {
            h.push_str(&format!(
                "<div class=\"booking-sub\">{}: {}</div>",
                esc(t("hotelAccess", lang)),
                esc(&plan.hotel_access_lines.join(", "))
            ));
        }
        // Voucher PDF link (own /voucher/* R2 route). 404s until the PDF is uploaded.
        // The /voucher/* route is auth-gated (same scope as the plan view), so a
        // tokenless link would 403 on click. Thread the page's token onto local
        // voucher links so the click carries the SAME token the page loaded with.
        if !voucher_url.is_empty() {
            let mut href = voucher_url.clone();
            if voucher_url.starts_with("/voucher/") {
                if let Some(tok) = token.filter(|t| !t.is_empty()) {
                    href.push_str("?token=");
                    href.push_str(tok);
                }
            }
            h.push_str(&format!(
                "<a class=\"voucher-link\" href=\"{}\" target=\"_blank\" rel=\"noopener\">📄 {}</a>",
                esc_url_attr(&href), esc(t("voucher", lang))
            ));
        }
        // PNR / cancellation text behind native <details> (progressive disclosure, no JS),
        // reformatted into grouped bullets (## Group headers → labeled <ul> lists).
        if !notes.is_empty() {
            h.push_str(&format!(
                "<details class=\"booking-notes\"><summary>{}</summary><div class=\"booking-notes-body\">{}</div></details>",
                esc(t("details", lang)), render_notes(&notes)
            ));
        }
        h.push_str("</div></div>");
        h.push_str("</div>");
    }

    // Domestic stays (Taiwan, via bookings_current category=accommodation)
    // P4-aware: when p4_status == booked, this is the primary block; when pending/selecting
    // and there is no booked stay, the block is hidden (empty → no render).
    let is_booked = plan.p4_status == "booked";
    if !plan.domestic_stays.is_empty() {
        let domestic_title = if is_booked {
            if lang == "zh" {
                "🏠 已訂住宿"
            } else {
                "🏠 Booked Accommodation"
            }
        } else {
            t("domesticStay", lang)
        };
        h.push_str(&format!("<h2>{}</h2>", esc(domestic_title)));
        h.push_str("<div class=\"booking-grid\">");
        for stay in &plan.domestic_stays {
            // Prefer hotel_name + room_type split; fallback to raw title.
            let name = if !stay.hotel_name.is_empty() {
                if stay.room_type.is_empty() {
                    stay.hotel_name.clone()
                } else {
                    format!("{} {}", stay.hotel_name, stay.room_type)
                }
            } else {
                stay.title.clone()
            };
            let cur = if stay.currency.is_empty() {
                "TWD"
            } else {
                &stay.currency
            };
            // Booked stays get the obvious green "已訂" treatment (badge + tinted card).
            let item_class = if is_booked {
                "booking-item domestic domestic--booked"
            } else {
                "booking-item domestic"
            };
            h.push_str(&format!("<div class=\"{item_class}\">"));
            h.push_str("<span class=\"booking-icon\">🏠</span>");
            h.push_str("<div class=\"booking-detail\">");
            h.push_str(&format!(
                "<div class=\"booking-value\">{}</div>",
                esc(&name)
            ));
            if is_booked {
                h.push_str(&format!(
                    "<span class=\"booked-badge\">✓ {}</span>",
                    esc(t("bookedBadge", lang))
                ));
            }
            // Price line — TWD grouped, no hard-coded JPY.
            if stay.price_twd > 0 {
                h.push_str(&format!(
                    "<div class=\"booking-sub\">{} {}</div>",
                    esc(cur),
                    group_thousands(stay.price_twd)
                ));
            }
            if !stay.selected_date.is_empty() {
                h.push_str(&format!(
                    "<div class=\"booking-sub\">{}</div>",
                    esc(&stay.selected_date)
                ));
            }
            if !stay.status.is_empty() {
                h.push_str(&format!(
                    "<div class=\"booking-sub\">{}</div>",
                    esc(&stay.status)
                ));
            }
            h.push_str("</div></div>");
        }
        h.push_str("</div>");
    }

    // Domestic candidates — sea-view shortlist (jiufen three) from domestic_accommodations.
    // P4-aware headings (pure HTML/CSS, no JS):
    //   booked   → 「其他海景參考」 + 小字「僅供參考」
    //   pending/selecting/empty → 「🏨 海景候選 · 正在選」 (candidates as primary, dashed frame)
    // Only RANKED candidates render — an unranked row is a ruled-out/backup
    // note that fails the trip's hard conditions (浴缸×海景 for jiufen), not a
    // card to compare (user request 2026-09-28: 不合條件的不列出). An
    // all-unranked set is data still being entered (nothing decided yet), so it
    // still shows everything — an empty section would hide in-progress work.
    let ranked: Vec<&crate::model::DomesticCandidate> = plan
        .candidates
        .iter()
        .filter(|c| c.ranking.is_some())
        .collect();
    let show: Vec<&crate::model::DomesticCandidate> = if ranked.is_empty() {
        plan.candidates.iter().collect()
    } else {
        ranked.clone()
    };
    if !show.is_empty() {
        let (cand_title, cand_sub): (&str, Option<&str>) = if is_booked {
            (
                if lang == "zh" {
                    "其他海景參考"
                } else {
                    "Other Sea-View References"
                },
                Some(if lang == "zh" {
                    "僅供參考 · 已選定住宿"
                } else {
                    "For reference · accommodation booked"
                }),
            )
        } else {
            (
                if lang == "zh" {
                    "🏨 海景候選 · 正在選"
                } else {
                    "🏨 Sea-View Candidates · Selecting"
                },
                None,
            )
        };
        h.push_str(&format!("<h2>{}</h2>", esc(cand_title)));
        // Say it in words, not only through a dashed border a reader may not decode.
        // The count is the cards actually RENDERED under the heading — counting
        // plan.candidates would say 9 while only the ranked ones list (the
        // "9 間比較中" that read as nine matching stays).
        let sub_text = cand_sub.map(str::to_string).unwrap_or_else(|| {
            t("notBookedYet", lang).replace("{n}", &show.len().to_string())
        });
        h.push_str(&format!(
            "<div class=\"candidate-sub\">{}</div>",
            esc(&sub_text)
        ));
        // 搜尋條件 line — what the cards were filtered against, so 「N 間比較中」
        // has its denominator visible (user request 2026-09-28). The date part
        // comes from the plan's anchors; the requirement enumeration is section
        // chrome (same precedent as the 「海景候選」 heading itself) — the
        // authoritative criteria text lives in docs/trips/<plan>.md.
        if !is_booked {
            let dates = criteria_dates(plan, lang);
            // No dates yet → drop the trailing " · " the {dates} placeholder
            // leaves behind rather than publishing "條件：… · ".
            let criteria = t("criteriaLine", lang)
                .replace("{dates}", &dates)
                .trim_end()
                .trim_end_matches('·')
                .trim_end()
                .to_string();
            if !criteria.is_empty() {
                h.push_str(&format!(
                    "<div class=\"candidate-criteria\">{}</div>",
                    esc(&criteria)
                ));
            }
        }
        // 推薦排序 — the section must not be a bare price grid: when rankings
        // exist, state the order and the reason (user request 2026-09-28).
        if !ranked.is_empty() && !is_booked {
            h.push_str(&format!(
                "<div class=\"candidate-ranking\"><span class=\"candidate-ranking-title\">{}</span><ol>",
                esc(t("recommendOrder", lang))
            ));
            for c in &ranked {
                let title = if c.room_type.is_empty() {
                    c.hotel_name.clone()
                } else {
                    format!("{} {}", c.hotel_name, c.room_type)
                };
                let badge = if c.ranking == Some(1) {
                    t("firstChoice", lang)
                } else {
                    ""
                };
                h.push_str(&format!(
                    "<li><span class=\"candidate-ranking-name\">{}</span>{}<span class=\"candidate-ranking-notes\">{}</span></li>",
                    esc(&title),
                    if badge.is_empty() {
                        String::new()
                    } else {
                        format!("<span class=\"candidate-rank-badge\">{}</span>", esc(badge))
                    },
                    esc(&c.notes)
                ));
            }
            h.push_str("</ol></div>");
        }
        h.push_str("<div class=\"candidate-grid\">");
        // `show` is ranked-only (or everything when nothing is ranked yet) —
        // unranked rows are 不合條件, not cards to compare. Booked state shows
        // the same set flat (ranking is history then, not advice).
        for c in &show {
            let title = if c.room_type.is_empty() {
                c.hotel_name.clone()
            } else {
                format!("{} {}", c.hotel_name, c.room_type)
            };
            let cur = if c.currency.is_empty() {
                "TWD"
            } else {
                &c.currency
            };
            // Selecting state → dashed frame on each card (visual "not booked yet").
            let card_class = if is_booked {
                "candidate-card"
            } else {
                "candidate-card candidate-card--selecting"
            };
            h.push_str(&format!("<div class=\"{card_class}\">"));
            if is_placeholder_image(&c.image_url) {
                h.push_str(&format!(
                    "<div class=\"candidate-image candidate-image--placeholder\" aria-label=\"{}\">{}</div>",
                    esc(&c.hotel_name),
                    esc(&c.hotel_name)
                ));
            } else {
                h.push_str(&format!(
                    "<img class=\"candidate-image\" src=\"{}\" alt=\"{}\" loading=\"lazy\" referrerpolicy=\"no-referrer\" />",
                    esc_url_attr(&c.image_url),
                    esc(&c.hotel_name)
                ));
            }
            // Rank badge on the card (selecting state only — once booked the
            // ranking is history, not current advice).
            let rank_badge = if is_booked {
                String::new()
            } else {
                match c.ranking {
                    Some(1) => format!(
                        "<span class=\"candidate-rank-badge candidate-rank-badge--first\">{}</span>",
                        esc(t("firstChoice", lang))
                    ),
                    Some(n) => format!(
                        "<span class=\"candidate-rank-badge\">No.{n}</span>"
                    ),
                    None => format!(
                        "<span class=\"candidate-rank-badge candidate-rank-badge--none\">{}</span>",
                        esc(t("notRanked", lang))
                    ),
                }
            };
            h.push_str(&format!(
                "<div class=\"candidate-name\">{}{}</div>",
                rank_badge,
                esc(&title)
            ));
            if c.price_twd > 0 {
                h.push_str(&format!(
                    "<div class=\"candidate-price\">{} {}",
                    esc(cur),
                    group_thousands(c.price_twd)
                ));
                h.push_str(&format!(
                    "<span class=\"candidate-unit\">{}</span>",
                    esc(t("perNight", lang))
                ));
                if c.room_size_sqm > 0 {
                    h.push_str(&format!(
                        "<span class=\"candidate-size\">{} m\u{b2}</span>",
                        c.room_size_sqm
                    ));
                }
                h.push_str("</div>");
                // Provenance line — a published rate without its source and read
                // date is indistinguishable from a made-up one.
                h.push_str(&price_note(c, lang));
            }
            h.push_str(&ratings_row(&c.ratings, lang));
            // Tags: sea view + breakfast
            let mut tags: Vec<String> = Vec::new();
            if c.sea_view == 1 {
                tags.push(format!(
                    "<span class=\"candidate-tag candidate-tag--sea\">{}</span>",
                    esc(t("seaView", lang))
                ));
            }
            if c.breakfast_included == 1 {
                tags.push(format!(
                    "<span class=\"candidate-tag candidate-tag--bf\">{}</span>",
                    esc(t("breakfast", lang))
                ));
            } else {
                // State it. An absent breakfast tag was indistinguishable from
                // "we never checked", and a NT$7,199 room reads as half-board by default.
                tags.push(format!(
                    "<span class=\"candidate-tag candidate-tag--nobf\">{}</span>",
                    esc(t("noBreakfast", lang))
                ));
            }
            // Bathtub: verified per ROOM TYPE (property-level filters lie — a 浴缸
            // filter can match via other room types). NULL (未查) renders no tag:
            // unverified is not "no".
            match c.has_bathtub {
                Some(1) => tags.push(format!(
                    "<span class=\"candidate-tag candidate-tag--tub\">{}</span>",
                    esc(t("bathtub", lang))
                )),
                Some(_) => tags.push(format!(
                    "<span class=\"candidate-tag candidate-tag--notub\">{}</span>",
                    esc(t("noBathtub", lang))
                )),
                None => {}
            }
            if !tags.is_empty() {
                h.push_str(&format!(
                    "<div class=\"candidate-tags\">{}</div>",
                    tags.join(" ")
                ));
            }
            // 優劣比較 — the card carries its own pros/cons text so the reader
            // can decide without cross-referencing the ranking list.
            if !c.notes.is_empty() {
                h.push_str(&format!(
                    "<div class=\"candidate-notes\"><b>{}：</b>{}</div>",
                    esc(t("prosConsLabel", lang)),
                    esc(&c.notes)
                ));
            }
            // Media strip at the card bottom — facts-first (price → rating →
            // tags → notes), media LAST. The 位置小圖 leads the strip (it answers
            // the decision question — how far, which direction from 旅遊地),
            // followed by one thumbnail per room type / area, each linking to the
            // full image (SSR-only, no JS lightbox). The minimap is a grid cell
            // AMONG the photos, not its own full-width row (user request
            // 2026-09-28: 住宿點相對圖不用自己佔一格).
            let gallery: Vec<_> = c
                .images
                .iter()
                .filter(|g| !is_placeholder_image(&g.image_url))
                .collect();
            let minimap = match (c.latitude, c.longitude) {
                (Some(lat), Some(lon)) => Some(candidate_minimap(c, lat, lon, &plan.poi_stops, lang)),
                _ => None,
            };
            if minimap.is_some() || !gallery.is_empty() {
                h.push_str("<div class=\"candidate-gallery\">");
                if let Some(mm) = minimap {
                    h.push_str(&format!(
                        "<figure class=\"candidate-gallery-item candidate-gallery-item--map\">{mm}</figure>"
                    ));
                }
                for g in gallery {
                    h.push_str("<figure class=\"candidate-gallery-item\">");
                    h.push_str(&format!(
                        "<a href=\"{}\" target=\"_blank\" rel=\"noopener\">\
                         <img class=\"candidate-gallery-img\" src=\"{}\" alt=\"{}\" loading=\"lazy\" referrerpolicy=\"no-referrer\" /></a>",
                        esc_url_attr(&g.image_url),
                        esc_url_attr(&g.image_url),
                        esc(if g.label.is_empty() { &c.hotel_name } else { &g.label }),
                    ));
                    if !g.label.is_empty() {
                        h.push_str(&format!(
                            "<figcaption class=\"candidate-gallery-label\">{}</figcaption>",
                            esc(&g.label)
                        ));
                    }
                    h.push_str("</figure>");
                }
                h.push_str("</div>");
            }
            // External rooms/availability link — it opens a real booking engine, so the label
            // says so rather than promising a passive room list.
            if !c.link_url.is_empty() {
                h.push_str(&format!(
                    "<a class=\"candidate-link\" href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a>",
                    esc_url_attr(&c.link_url),
                    esc(t("moreRoomTypes", lang))
                ));
            }
            h.push_str("</div>");
        }
        h.push_str("</div>");
    }

    // Transfers (the old "—" bug lived here)
    if !plan.transfers.is_empty() {
        h.push_str(&format!("<h2>{}</h2>", esc(t("transfers", lang))));
        h.push_str("<div class=\"booking-grid\">");
        for tr in &plan.transfers {
            let title = rs(tr, "selected_title");
            let route = rs(tr, "selected_route");
            let dur = rs(tr, "selected_duration_min");
            let price = rs(tr, "selected_price_yen");
            h.push_str("<div class=\"booking-item transfer\">");
            h.push_str("<span class=\"booking-icon\">🚃</span>");
            h.push_str("<div class=\"booking-detail\">");
            if !title.is_empty() {
                h.push_str(&format!(
                    "<div class=\"booking-value\">{}</div>",
                    esc(&title)
                ));
            }
            if !route.is_empty() {
                h.push_str(&format!("<div class=\"booking-sub\">{}</div>", esc(&route)));
            }
            let mut meta: Vec<String> = Vec::new();
            if !dur.is_empty() {
                meta.push(format!("~{} min", dur));
            }
            if !price.is_empty() {
                meta.push(format!("¥{}", price));
            }
            if !meta.is_empty() {
                h.push_str(&format!(
                    "<div class=\"booking-sub\">{}</div>",
                    esc(&meta.join(" · "))
                ));
            }
            h.push_str("</div></div>");
        }
        h.push_str("</div>");
    }

    // Japan-only entry info (Visit Japan Web + Japan Tourism), shown when the
    // destination currency is JPY. Ported from render.ts:1049,1203-1218.
    if plan.currency == "JPY" {
        h.push_str(&format!("<h2>{}</h2>", esc(t("japanEntry", lang))));
        h.push_str("<div class=\"booking-grid\">");
        // Visit Japan Web (entry application)
        h.push_str("<div class=\"booking-item japan-entry\">");
        h.push_str("<span class=\"booking-icon\">🛂</span>");
        h.push_str("<div class=\"booking-detail\">");
        h.push_str(&format!(
            "<div class=\"booking-value\"><a href=\"https://www.vjw.digital.go.jp/main/#/vjwplo001\" \
             target=\"_blank\" rel=\"noopener\">{}</a></div>",
            esc(t("visitJapanWeb", lang))
        ));
        h.push_str("</div></div>");
        // Japan Tourism Agency
        h.push_str("<div class=\"booking-item japan-entry\">");
        h.push_str("<span class=\"booking-icon\">🗾</span>");
        h.push_str("<div class=\"booking-detail\">");
        h.push_str(&format!(
            "<div class=\"booking-value\"><a href=\"https://www.japan.travel/\" \
             target=\"_blank\" rel=\"noopener\">{}</a></div>",
            esc(t("japanTourism", lang))
        ));
        h.push_str(&format!(
            "<div class=\"booking-sub\">{}</div>",
            esc(t("japanTourismSub", lang))
        ));
        h.push_str("</div></div>");
        h.push_str("</div>");
    }

    h.push_str("</section>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FitOffer, Offer, Plan};
    use crate::turso::Row;

    #[test]
    fn group_thousands_formats_prices() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(27888), "27,888");
        assert_eq!(group_thousands(55776), "55,776");
        assert_eq!(group_thousands(1234567), "1,234,567");
    }

    #[test]
    fn package_offer_renders_with_per_person_and_for_two() {
        let plan = Plan {
            offer: Some(Offer {
                source_id: "besttour".into(),
                product_code: "TYO06MM260213AM2".into(),
                price: 27888,
                currency: "TWD".into(),
            }),
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(html.contains("besttour"));
        assert!(html.contains("TYO06MM260213AM2"));
        assert!(html.contains("TWD 27,888")); // per-person, grouped
        assert!(html.contains("55,776")); // for-2 = price*2
    }

    fn sample_fit(
        source: &str,
        price: i64,
        currency: &str,
        recommended: bool,
        note_zh: &str,
    ) -> FitOffer {
        FitOffer {
            source_id: source.into(),
            price,
            currency: currency.into(),
            hotel_name: source.into(),
            recommended,
            note_zh: note_zh.into(),
            note_en: if note_zh.is_empty() {
                String::new()
            } else {
                format!("en-{source}")
            },
            ..Default::default()
        }
    }

    #[test]
    fn fit_comparison_reads_as_booked_once_p4_is_booked() {
        let plan = Plan {
            p4_status: "booked".into(),
            fit_offers: vec![
                sample_fit("liontravel", 21646, "TWD", true, "已訂雄獅。"),
                sample_fit("lifetour", 18990, "TWD", false, ""),
            ],
            ..Default::default()
        };
        let zh = render(&plan, "zh", None);
        assert!(zh.contains("FIT 方案比較（已訂，以下為訂購前比價參考）"), "{zh}");
        assert!(!zh.contains("非已訂"));
        assert!(zh.contains("<span class=\"fit-badge fit-badge-rec\">已訂</span>"));
        assert!(!zh.contains("fit-badge-rec\">推薦"));
        let en = render(&plan, "en", None);
        assert!(en.contains("FIT options (booked — pre-booking comparison)"));
        assert!(en.contains("fit-badge-rec\">Booked</span>"));
    }

    #[test]
    fn fit_comparison_renders_recommendation_lowest_and_delta() {
        let plan = Plan {
            fit_compare_zh: "三家都是上午去、午後回。".into(),
            fit_compare_en: "Morning out, afternoon back.".into(),
            fit_offers: vec![
                sample_fit("lifetour", 18990, "TWD", true, "同已訂去程時刻，最低價。"),
                sample_fit("liontravel", 21646, "TWD", false, "同一組樂桃。"),
                sample_fit("settour", 25467, "TWD", false, ""),
            ],
            ..Default::default()
        };
        let mut plan = plan;
        plan.fit_offers[0].room_zh = "約 12㎡（約 3.6 坪）".into();
        plan.fit_offers[0].room_en = "about 12 m² (about 3.6 ping)".into();
        let zh = render(&plan, "zh", None);
        assert!(zh.contains("fit-compare"));
        assert!(zh.contains("面積"));
        assert!(zh.contains("約 12㎡（約 3.6 坪）"));
        assert!(zh.contains("三家都是上午去、午後回。"));
        assert!(zh.contains("fit-badge-rec"));
        assert!(zh.contains("推薦"));
        assert!(zh.contains("is-recommended"));
        assert!(zh.contains("最低價"));
        assert!(zh.contains("貴 TWD 2,656／人"));
        assert!(zh.contains("貴 TWD 6,477／人"));
        assert!(zh.contains("同已訂去程時刻，最低價。"));
        assert!(zh.contains("同一組樂桃。"));
        // The cheapest card is not also labeled as more expensive.
        let lifetour = zh.split("fit-agency-offer").nth(1).unwrap_or("");
        assert!(lifetour.contains("最低價"), "{lifetour}");
        assert!(!lifetour.contains("貴 TWD"), "{lifetour}");

        let en = render(&plan, "en", None);
        assert!(en.contains("Recommended"));
        assert!(en.contains("Lowest"));
        assert!(en.contains("+TWD 2,656 / person"));
        assert!(en.contains("+TWD 6,477 / person"));
        assert!(en.contains("Morning out, afternoon back."));
        assert!(en.contains("en-lifetour"));
        assert!(en.contains("Size"));
        assert!(en.contains("about 12 m² (about 3.6 ping)"));
    }

    #[test]
    fn fit_price_badges_skip_a_single_offer_and_mixed_currency() {
        let one = Plan {
            fit_offers: vec![sample_fit("lifetour", 18990, "TWD", true, "只有一家。")],
            ..Default::default()
        };
        let html = render(&one, "zh", None);
        assert!(html.contains("推薦"));
        assert!(!html.contains("最低價"));
        assert!(!html.contains("fit-badge-delta"));

        let mixed = Plan {
            fit_offers: vec![
                sample_fit("lifetour", 18990, "TWD", false, ""),
                sample_fit("liontravel", 80000, "JPY", true, "不同幣別。"),
            ],
            ..Default::default()
        };
        let html = render(&mixed, "zh", None);
        assert!(html.contains("推薦"));
        assert!(html.contains("不同幣別。"));
        assert!(!html.contains("最低價"));
        assert!(!html.contains("fit-badge-delta"));
    }

    #[test]
    fn fit_reason_is_escaped() {
        let plan = Plan {
            fit_offers: vec![sample_fit(
                "lifetour",
                100,
                "TWD",
                false,
                "<script>alert(1)</script>",
            )],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
    }

    #[test]
    fn no_offer_means_no_package_block() {
        let plan = Plan::default(); // offer: None
        let html = render(&plan, "en", None);
        assert!(!html.contains("booking-item package"));
    }

    #[test]
    fn hotel_access_lines_render_in_hotel_block() {
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        let plan = Plan {
            hotel: Some(hotel),
            hotel_access_lines: vec!["Yui Rail Asato 3min".into(), "JR Naha 8min".into()],
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(html.contains("Yui Rail Asato 3min, JR Naha 8min"));
        assert!(html.contains("Access:"));
    }

    #[test]
    fn japan_entry_rows_shown_only_for_jpy() {
        let jpy = Plan {
            currency: "JPY".into(),
            ..Default::default()
        };
        let html = render(&jpy, "en", None);
        assert!(html.contains("Visit Japan Web"));
        assert!(html.contains("vjw.digital.go.jp"));

        let twd = Plan {
            currency: "TWD".into(),
            ..Default::default()
        };
        let html2 = render(&twd, "en", None);
        assert!(!html2.contains("Visit Japan Web"));
    }

    #[test]
    fn transfer_renders_route_and_price() {
        let mut tr = Row::new();
        tr.insert("direction".into(), serde_json::json!("arrival"));
        tr.insert("selected_title".into(), serde_json::json!("Yui Rail"));
        tr.insert(
            "selected_route".into(),
            serde_json::json!("Naha Airport → Asato"),
        );
        tr.insert("selected_duration_min".into(), serde_json::json!("24"));
        tr.insert("selected_price_yen".into(), serde_json::json!("340"));
        let plan = Plan {
            transfers: vec![tr],
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(html.contains("Naha Airport → Asato"));
        assert!(html.contains("340"));
        assert!(html.contains("Yui Rail"));
    }

    #[test]
    fn flight_renders_number_and_route() {
        let mut f = Row::new();
        for (k, v) in [
            ("flight_number", "CI120"),
            ("airline", "China Airlines"),
            ("departure_code", "TPE"),
            ("departure_time", "08:00"),
            ("arrival_code", "OKA"),
            ("arrival_time", "10:45"),
            ("flight_date", "2026-06-12"),
        ] {
            f.insert(k.into(), serde_json::json!(v));
        }
        let plan = Plan {
            flights: vec![f],
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(html.contains("CI120"));
        assert!(html.contains("TPE"));
        assert!(html.contains("OKA"));
    }

    #[test]
    fn flight_number_is_clickable_google_search_link() {
        let mut f = Row::new();
        for (k, v) in [
            ("flight_number", "CI 120"),
            ("airline", "China Airlines"),
            ("departure_code", "TPE"),
            ("arrival_code", "OKA"),
        ] {
            f.insert(k.into(), serde_json::json!(v));
        }
        let plan = Plan {
            flights: vec![f],
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        // (a) href contains the percent-encoded flight number ("CI 120" → "CI%20120").
        assert!(
            html.contains("href=\"https://www.google.com/search?q=CI%20120\""),
            "encoded flight number missing from href; got: {html}"
        );
        // (b) visible anchor text includes the airline (airline is NOT in the query).
        assert!(html.contains(">CI 120 China Airlines</a>"), "got: {html}");
        // anchor styling/attrs mirror the TS port.
        assert!(html.contains("target=\"_blank\""));
        assert!(html.contains("rel=\"noopener\""));
        assert!(html.contains("text-decoration:underline dotted"));
    }

    #[test]
    fn flight_without_number_renders_plain_text_no_anchor() {
        let mut f = Row::new();
        f.insert("airline".into(), serde_json::json!("China Airlines"));
        f.insert("departure_code".into(), serde_json::json!("TPE"));
        let plan = Plan {
            flights: vec![f],
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        // (c) empty flight_number → no <a tag in the flight value.
        assert!(
            !html.contains("<a "),
            "unexpected anchor for empty flight number; got: {html}"
        );
        assert!(html.contains("China Airlines"), "got: {html}");
    }

    #[test]
    fn hotel_notes_behind_details_and_zh_name() {
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("Hotel Aqua Citta Naha"));
        hotel.insert("name_zh".into(), serde_json::json!("那霸水都飯店"));
        hotel.insert("check_in".into(), serde_json::json!("2026-06-21 15:00"));
        hotel.insert(
            "notes".into(),
            serde_json::json!("CFM 1234567 cancellation by 2026-06-14"),
        );
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(html.contains("那霸水都飯店")); // zh name preferred
        assert!(!html.contains("Hotel Aqua Citta Naha")); // en name not shown when zh present
        assert!(html.contains("<details"));
        assert!(html.contains("CFM 1234567")); // PNR present but collapsed
    }

    #[test]
    fn hotel_grouped_notes_render_labels_and_bullets() {
        let notes = "## 房型 Room\nStandard twin · non-smoking\n## 訂單 Booking\n4 nights: 2026-06-12 → 2026-06-16\n⚠ Non-refundable\n## 用餐 Dining\nBreakfast ONLY\n## 交通 Access\nYui Rail: Asato Station";
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        hotel.insert("notes".into(), serde_json::json!(notes));
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        // wrapper preserved
        assert!(html.contains("<details"));
        // group labels present (rendered without the "## " prefix)
        assert!(html.contains("房型 Room"));
        assert!(html.contains("訂單 Booking"));
        assert!(html.contains("用餐 Dining"));
        assert!(html.contains("交通 Access"));
        assert!(!html.contains("## ")); // the marker prefix must be stripped
                                        // fact lines become <li> items
        assert!(html.contains("<li>Standard twin · non-smoking</li>"));
        assert!(html.contains("<li>⚠ Non-refundable</li>"));
        // 4 groups → 4 <ul> blocks
        assert_eq!(html.matches("<ul").count(), 4);
        assert_eq!(html.matches("hotel-group-label").count(), 4);
    }

    #[test]
    fn hotel_notes_blank_lines_skipped() {
        let notes = "## 房型 Room\n\nStandard twin\n\n";
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        hotel.insert("notes".into(), serde_json::json!(notes));
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(html.contains("<li>Standard twin</li>"));
        assert!(!html.contains("<li></li>")); // no empty bullets
    }

    #[test]
    fn hotel_voucher_link_renders_with_href_and_target() {
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        hotel.insert(
            "voucher_url".into(),
            serde_json::json!("/voucher/okinawa-2026/azat-voucher.pdf"),
        );
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "en", Some("3b9412d0fa2b9961d80a044cab0ebbf4"));
        assert!(html.contains("class=\"voucher-link\""));
        // Gated route → href must carry the page token.
        assert!(html.contains(
            "href=\"/voucher/okinawa-2026/azat-voucher.pdf?token=3b9412d0fa2b9961d80a044cab0ebbf4\""
        ));
        assert!(html.contains("target=\"_blank\""));
        assert!(html.contains("rel=\"noopener\""));
        assert!(html.contains("Hotel voucher (PDF)")); // en label
    }

    #[test]
    fn hotel_voucher_link_echoes_loading_token() {
        // The href echoes WHATEVER token loaded the page (owner or share),
        // proving it is the request token and not a hardcoded one.
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        hotel.insert(
            "voucher_url".into(),
            serde_json::json!("/voucher/okinawa-2026/azat-voucher.pdf"),
        );
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "en", Some("dd90508f2efd063ee760197d127fffa4"));
        assert!(html.contains(
            "href=\"/voucher/okinawa-2026/azat-voucher.pdf?token=dd90508f2efd063ee760197d127fffa4\""
        ));
    }

    #[test]
    fn hotel_voucher_link_without_token_has_no_query() {
        // No token (shouldn't happen for a gated page) → bare href, no ?token=.
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        hotel.insert(
            "voucher_url".into(),
            serde_json::json!("/voucher/okinawa-2026/azat-voucher.pdf"),
        );
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(html.contains("href=\"/voucher/okinawa-2026/azat-voucher.pdf\""));
        assert!(!html.contains("?token="));
    }

    #[test]
    fn hotel_without_voucher_url_renders_no_link() {
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(!html.contains("voucher-link"));
    }

    #[test]
    fn empty_plan_renders_no_section_headings() {
        let plan = Plan::default();
        let html = render(&plan, "en", None);
        assert!(!html.contains("Flights"));
        assert!(!html.contains("Hotel"));
        assert!(!html.contains("Transfers"));
    }

    #[test]
    fn summary_section_carries_dashed_box_class() {
        let plan = Plan::default();
        let html = render(&plan, "en", None);
        assert!(html.contains("<section class=\"booking-summary summary-box\">"));
    }

    #[test]
    fn booked_domestic_stay_shows_green_badge_and_icon_title() {
        use crate::model::DomesticStay;
        let plan = Plan {
            p4_status: "booked".into(),
            domestic_stays: vec![DomesticStay {
                title: "海論 海景雙人房".into(),
                hotel_name: "海論".into(),
                room_type: "海景雙人房".into(),
                price_twd: 5200,
                selected_date: "2026-10-12".into(),
                status: "booked".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(html.contains("🏠 已訂住宿"), "booked h2 title, got: {html}");
        assert!(html.contains("domestic--booked"), "green booked card class");
        assert!(html.contains("booked-badge"), "已訂 badge");
        assert!(html.contains("✓ 已訂"));
    }

    #[test]
    fn selecting_candidates_show_icon_title_and_dashed_cards() {
        use crate::model::DomesticCandidate;
        let plan = Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "海論".into(),
                room_type: "海景雙人房".into(),
                price_twd: 5200,
                sea_view: 1,
                breakfast_included: 1,
                image_url: "https://example.com/a.webp".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(
            html.contains("🏨 海景候選 · 正在選"),
            "selecting h2, got: {html}"
        );
        assert!(
            html.contains("candidate-card--selecting"),
            "dashed card class"
        );
        assert!(!html.contains("已訂住宿"));
    }

    #[test]
    fn booked_candidates_lose_dashed_frame_and_show_reference_sub() {
        use crate::model::DomesticCandidate;
        let plan = Plan {
            p4_status: "booked".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "海論".into(),
                room_type: "海景雙人房".into(),
                image_url: "https://example.com/a.webp".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(html.contains("其他海景參考"));
        assert!(html.contains("僅供參考"));
        assert!(!html.contains("candidate-card--selecting"));
    }

    #[test]
    fn candidate_minimap_centers_on_destination_with_stay_at_third() {
        use crate::model::{DomesticCandidate, PoiStop};
        // The live jiufen-2026 legend set (plan.png stops) + a stay OUTSIDE the
        // 九份 cluster — the case the two-point geometry exists to expose.
        let stops = vec![
            PoiStop { label: "九份老街".into(), lat: 25.108719, lon: 121.8435077 },
            PoiStop { label: "九份住宿".into(), lat: 25.1081276, lon: 121.8383274 },
            PoiStop { label: "野柳地質公園".into(), lat: 25.2113474, lon: 121.6963751 },
            PoiStop { label: "金山老街".into(), lat: 25.221883, lon: 121.6361852 },
            PoiStop { label: "淡水".into(), lat: 25.1727, lon: 121.4377 },
        ];
        let plan = |lat: f64, lon: f64| Plan {
            p4_status: "selecting".into(),
            poi_stops: stops.clone(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "魚礁十五號".into(),
                room_type: "四人房－附浴缸".into(),
                price_twd: 4000,
                latitude: Some(lat),
                longitude: Some(lon),
                ..Default::default()
            }],
            ..Default::default()
        };
        // ---- OUTLIER stay (4.5 km from the hub 九份老街): bbox centered ON the
        // hub stop (🚩 at the image center), span = 3× the stay's dominant-axis
        // offset → hub→stay distance spans a THIRD of the frame, dashed
        // connector in between.
        let html = render(&plan(25.1330327, 121.807191), "zh", None);
        assert!(
            html.contains("server.arcgisonline.com"),
            "basemap host: {html}"
        );
        assert!(
            html.contains("bbox=121.783093,25.072248,121.903922,25.145190"),
            "destination-centered bbox: {html}"
        );
        assert!(html.contains("cand-minimap-pin"), "CSS pin overlay: {html}");
        assert!(
            html.contains("left:19.94%;top:16.67%"),
            "stay pin a third of the frame from the center: {html}"
        );
        // The minimap is a cell in the media strip, not its own full-width row.
        assert!(
            html.contains("<figure class=\"candidate-gallery-item candidate-gallery-item--map\">"),
            "minimap inside the gallery strip: {html}"
        );
        // The 旅遊地 is marked — a 🚩 on the hub stop with its label, not just
        // another blue dot, and a dashed hub→stay connector (交通路線).
        assert!(
            html.contains("🚩<em class=\"cand-minimap-hub-label\">九份老街"),
            "hub flag + label at the image center: {html}"
        );
        assert!(
            html.contains("cand-minimap-route") && html.contains("<line "),
            "dashed hub→stay connector: {html}"
        );
        // Hub-cluster dot renders; off-frame excursion stops are OMITTED (a
        // clamped dot would claim a position the geometry never gave it).
        assert!(html.contains("cand-minimap-dot"), "POI dot: {html}");
        for label in ["九份老街", "九份住宿"] {
            assert!(html.contains(label), "on-frame POI label {label}: {html}");
        }
        for label in ["野柳地質公園", "金山老街", "淡水"] {
            assert!(!html.contains(label), "off-frame POI {label} omitted: {html}");
        }
        // Caption distance is to the HUB — the same endpoint the connector
        // draws to, so the number and the line tell one story.
        assert!(
            html.contains("距九份老街 約 4.5 km"),
            "hub distance in caption: {html}"
        );
        assert!(
            html.contains("https://www.google.com/maps?q=25.133033,121.807191"),
            "maps link: {html}"
        );
        assert!(html.contains("© Esri"), "attribution: {html}");

        // ---- IN-CLUSTER stay (0.2 km from the hub): street-scale map (the
        // 3× span floors at 0.004°), still destination-centered with the stay
        // a third of the frame out — not the 40 km full-coast overview that
        // pulled every card out.
        let html = render(&plan(25.1095, 121.8415), "zh", None);
        assert!(
            html.contains("bbox=121.840195,25.106719,121.846821,25.110719"),
            "in-cluster street-scale bbox: {html}"
        );
        assert!(
            html.contains("left:19.70%;top:30.48%"),
            "in-cluster stay a third of the frame out: {html}"
        );
        assert!(
            html.contains("cand-minimap-hub-label\">九份老街"),
            "in-cluster hub flag: {html}"
        );
    }

    #[test]
    fn candidate_minimap_falls_back_to_tight_bbox_without_stops() {
        use crate::model::DomesticCandidate;
        let with_coords = |lat: Option<f64>, lon: Option<f64>| Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "魚礁十五號".into(),
                room_type: "四人房－附浴缸".into(),
                price_twd: 4000,
                latitude: lat,
                longitude: lon,
                ..Default::default()
            }],
            ..Default::default()
        };
        // No stops (plan.png legend empty / map status missing) → v1 behavior:
        // stay-centered ±0.0016° bbox, pin at the center, no distance note.
        let html = render(&with_coords(Some(25.1330327), Some(121.807191)), "zh", None);
        assert!(html.contains("server.arcgisonline.com"), "basemap host: {html}");
        assert!(
            html.contains("bbox=121.804540,25.131433,121.809842,25.134633"),
            "tight fallback bbox: {html}"
        );
        assert!(html.contains("left:50.00%;top:50.00%"), "centered pin: {html}");
        assert!(!html.contains("cand-minimap-dot"), "no POI dots: {html}");
        assert!(
            !html.contains("cand-minimap-hub") && !html.contains("cand-minimap-route"),
            "no hub flag / connector without stops: {html}"
        );
        assert!(html.contains("© Esri"), "attribution: {html}");
        assert!(html.contains("位置"), "ZH caption label: {html}");
        // No coordinates → no minimap block at all (not a broken placeholder).
        let html = render(&with_coords(None, None), "zh", None);
        assert!(!html.contains("cand-minimap"), "no minimap without coords: {html}");
    }

    #[test]
    fn candidate_shows_ratings_size_and_price_provenance() {
        use crate::model::{CandidateRating, DomesticCandidate};
        let plan = Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "95行館".into(),
                room_type: "海景豪華雙人房".into(),
                price_twd: 3300,
                currency: "TWD".into(),
                room_size_sqm: 18,
                rooms_left: 1,
                free_cancel_until: "2026-09-28".into(),
                price_source: "Booking.com".into(),
                price_checked_at: "2026-09-04 01:51:21".into(),
                ratings: vec![
                    CandidateRating {
                        source: "Booking.com".into(),
                        score: 9.0,
                        scale: 10.0,
                        review_count: 266,
                    },
                    CandidateRating {
                        source: "Google 地圖".into(),
                        score: 4.6,
                        scale: 5.0,
                        review_count: 119,
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        // Each source keeps its own scale — never averaged into one number.
        assert!(
            html.contains("<b>9</b>/10 Booking.com"),
            "booking chip: {html}"
        );
        assert!(html.contains("<b>4.6</b>/5 Google"), "google chip: {html}");
        assert!(html.contains("266 則") && html.contains("119 則"));
        assert!(html.contains("18 m²"), "room size beside the price");
        // The price carries its source, read date, stock and cancellation deadline.
        assert!(html.contains("Booking.com 房價"));
        assert!(
            html.contains("2026-09-04 查價"),
            "datetime trimmed to a date: {html}"
        );
        // Stock hangs off the read date — never a bare "剩 1 間" that reads as live stock.
        assert!(
            html.contains("2026-09-04 查價（當時剩 1 間）"),
            "stock tied to the date: {html}"
        );
        assert!(
            !html.contains("· 剩 1 間"),
            "stock must not stand as its own clause: {html}"
        );

        assert!(html.contains("2026-09-28 前免費取消"));
    }

    #[test]
    fn missing_cancellation_policy_is_stated_not_left_silent() {
        use crate::model::DomesticCandidate;
        let mk = |until: &str| Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "H".into(),
                price_twd: 3710,
                price_source: "Cloudbeds".into(),
                price_checked_at: "2026-09-04".into(),
                free_cancel_until: until.into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        // Recorded → show the deadline.
        assert!(render(&mk("2026-09-28"), "zh", None).contains("2026-09-28 前免費取消"));
        // Not recorded → say so, rather than letting the reader assume it matches
        // the cards beside it.
        let blank = render(&mk(""), "zh", None);
        assert!(blank.contains("取消政策未查"), "{blank}");
        assert!(render(&mk(""), "en", None).contains("cancellation policy not checked"));
    }

    #[test]
    fn bathtub_tag_is_three_state() {
        use crate::model::DomesticCandidate;
        let mk = |tub: Option<i64>| Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "H".into(),
                price_twd: 1600,
                has_bathtub: tub,
                ..Default::default()
            }],
            ..Default::default()
        };
        // Verified yes → positive tag.
        let yes = render(&mk(Some(1)), "zh", None);
        assert!(yes.contains("candidate-tag--tub") && yes.contains("浴缸"), "{yes}");
        assert!(!yes.contains("無浴缸"), "{yes}");
        // Verified no → stated, not silent.
        let no = render(&mk(Some(0)), "zh", None);
        assert!(no.contains("candidate-tag--notub") && no.contains("無浴缸"), "{no}");
        assert!(render(&mk(Some(0)), "en", None).contains("No bathtub"));
        // Unverified (NULL) → no tag at all: 未查 ≠ 無浴缸. (The criteria line
        // legitimately contains the word 浴缸 — assert on the tag, not the word.)
        let unknown = render(&mk(None), "zh", None);
        assert!(
            !unknown.contains("candidate-tag--tub") && !unknown.contains("candidate-tag--notub"),
            "{unknown}"
        );
    }

    #[test]
    fn ranking_and_notes_render_comparison_block() {
        use crate::model::DomesticCandidate;
        let mk = |ranking: Option<i64>, notes: &str| DomesticCandidate {
            id: "c1".into(),
            hotel_name: "魚礁十五號".into(),
            room_type: "四人房－附浴缸".into(),
            price_twd: 4000,
            ranking,
            notes: notes.into(),
            ..Default::default()
        };
        let plan = Plan {
            p4_status: "selecting".into(),
            candidates: vec![
                mk(Some(1), "9.6 評分傑出；免訂金、可免費取消；缺點：無電梯"),
                DomesticCandidate {
                    id: "c2".into(),
                    hotel_name: "淘汰民宿".into(),
                    room_type: "海景雙人房".into(),
                    price_twd: 2351,
                    ranking: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        // The sub line counts the cards RENDERED — unranked rows are 不合條件
        // and do not list, so one card, not two (the "9 間比較中" overcount).
        assert!(html.contains("1 間比較中"), "{html}");
        assert!(!html.contains("淘汰民宿"), "unranked card not listed: {html}");
        // 推薦排序 block: ordered list, first-choice badge, the reason text.
        assert!(html.contains("candidate-ranking"), "{html}");
        assert!(html.contains("推薦排序"), "{html}");
        assert!(html.contains("首選"), "{html}");
        assert!(html.contains("無電梯"), "{html}");
        // Ranked candidate appears ONCE in the <ol>; the unranked one does not.
        let ol = html.split("<ol>").nth(1).unwrap_or("").split("</ol>").next().unwrap_or("");
        assert!(ol.contains("魚礁十五號"), "{ol}");
        // Card-level: notes block + rank badge + the criteria line.
        assert!(html.contains("candidate-notes"), "{html}");
        assert!(html.contains("優劣比較"), "{html}");
        assert!(
            html.contains("條件：2 人 · 浴缸＋海景＋免費停車（自駕）"),
            "criteria line: {html}"
        );
        // EN labels render for ?lang=en.
        let en = render(&plan, "en", None);
        assert!(en.contains("Recommended order"), "{en}");
        assert!(en.contains("Top pick"), "{en}");
        assert!(en.contains("comparing 1 stays"), "{en}");
        assert!(
            en.contains("Requirements: 2 guests · bathtub + sea view + free parking"),
            "EN criteria line: {en}"
        );
    }

    #[test]
    fn criteria_dates_render_plan_anchor_dates_with_weekday() {
        use crate::model::DomesticCandidate;
        let plan = Plan {
            p4_status: "selecting".into(),
            start_date: "2026-10-12".into(),
            end_date: "2026-10-13".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "H".into(),
                price_twd: 4000,
                ranking: Some(1),
                ..Default::default()
            }],
            ..Default::default()
        };
        let zh = render(&plan, "zh", None);
        assert!(
            zh.contains("條件：2 人 · 浴缸＋海景＋免費停車（自駕） · 10/12（一）1 晚"),
            "criteria line with anchor dates: {zh}"
        );
        let en = render(&plan, "en", None);
        assert!(en.contains("Mon 10/12 · 1 night"), "{en}");
    }

    #[test]
    fn no_ranking_means_no_comparison_block_but_count_still_shown() {
        use crate::model::DomesticCandidate;
        let plan = Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "H".into(),
                price_twd: 1600,
                ranking: None,
                notes: String::new(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(!html.contains("candidate-ranking"), "{html}");
        assert!(html.contains("1 間比較中"), "{html}");
        // No notes → no notes block (nothing to compare yet).
        assert!(!html.contains("candidate-notes"), "{html}");
    }

    #[test]
    fn booked_state_hides_ranking_advice() {
        use crate::model::DomesticCandidate;
        let plan = Plan {
            p4_status: "booked".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "H".into(),
                price_twd: 1600,
                ranking: Some(1),
                notes: "理由".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        // Booked → the block is reference-only; ranking advice is history.
        assert!(!html.contains("candidate-ranking"), "{html}");
        assert!(!html.contains("candidate-rank-badge"), "{html}");
        // Notes stay — they describe the property, useful as reference.
        assert!(html.contains("candidate-notes"), "{html}");
    }

    #[test]
    fn no_breakfast_is_stated_not_left_silent() {
        use crate::model::DomesticCandidate;
        let mk = |bf: i64| Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "H".into(),
                price_twd: 3300,
                breakfast_included: bf,
                ..Default::default()
            }],
            ..Default::default()
        };
        // 0 must SAY so — an absent tag reads as "included" for a Taiwanese guesthouse.
        let none = render(&mk(0), "zh", None);
        assert!(none.contains("不含早餐"), "{none}");
        assert!(
            !none.contains("含早餐</span>") || none.contains("不含早餐"),
            "{none}"
        );
        assert!(render(&mk(0), "en", None).contains("No breakfast"));
        // 1 keeps the positive tag and must NOT also claim the negative.
        let yes = render(&mk(1), "zh", None);
        assert!(yes.contains("candidate-tag--bf"));
        assert!(!yes.contains("不含早餐"), "{yes}");
    }

    #[test]
    fn price_states_what_it_buys_and_that_nothing_is_booked() {
        use crate::model::DomesticCandidate;
        let plan = Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "H".into(),
                price_twd: 3300,
                ..Default::default()
            }],
            ..Default::default()
        };
        let zh = render(&plan, "zh", None);
        assert!(
            zh.contains("每房每晚"),
            "a bare number does not say per night: {zh}"
        );
        assert!(
            zh.contains("尚未預訂"),
            "say it in words, not only a dashed border: {zh}"
        );
        let en = render(&plan, "en", None);
        assert!(en.contains("per room / night"));
        assert!(en.contains("Nothing booked yet"));
    }

    #[test]
    fn candidate_without_facts_renders_no_empty_rows() {
        use crate::model::DomesticCandidate;
        let plan = Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "海論".into(),
                price_twd: 5200,
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(!html.contains("candidate-ratings"), "no empty rating row");
        assert!(!html.contains("candidate-note"), "no empty provenance row");
        assert!(!html.contains("candidate-size"), "no empty size span");
    }

    #[test]
    fn price_provenance_is_english_on_lang_en() {
        use crate::model::DomesticCandidate;
        let plan = Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "95".into(),
                price_twd: 3300,
                rooms_left: 1,
                price_source: "Booking.com".into(),
                price_checked_at: "2026-09-04".into(),
                free_cancel_until: "2026-09-28".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(html.contains("Booking.com rate"));
        assert!(
            html.contains("checked 2026-09-04 (1 left then)"),
            "stock tied to the date: {html}"
        );
        assert!(
            !html.contains("· 1 left "),
            "stock must not stand alone in English either"
        );
        assert!(html.contains("free cancellation until 2026-09-28"));
    }

    // A stock count with no read date must SAY the date is unknown rather than
    // silently implying the number is current.
    #[test]
    fn rooms_left_without_a_check_date_is_marked_unknown() {
        use crate::model::DomesticCandidate;
        let plan = Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "H".into(),
                price_twd: 3300,
                rooms_left: 2,
                price_source: "Booking.com".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(render(&plan, "zh", None).contains("剩 2 間（查價日不明）"));
        assert!(render(&plan, "en", None).contains("2 left (date unknown)"));
    }

    #[test]
    fn rating_with_zero_scale_is_dropped_not_divided_by_zero() {
        use crate::model::CandidateRating;
        let out = ratings_row(
            &[CandidateRating {
                source: "Bogus".into(),
                score: 4.0,
                scale: 0.0,
                review_count: 0,
            }],
            "zh",
        );
        assert!(
            !out.contains("Bogus"),
            "a scale-less rating is unrenderable: {out}"
        );
    }

    #[test]
    fn trim_num_drops_only_trailing_zero_decimals() {
        assert_eq!(trim_num(9.0), "9");
        assert_eq!(trim_num(4.6), "4.6");
        assert_eq!(trim_num(10.0), "10");
    }

    #[test]
    fn candidate_gallery_renders_labeled_thumbs_and_room_link() {
        use crate::model::{CandidateImage, DomesticCandidate};
        let plan = Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "海論".into(),
                room_type: "海景雙人房".into(),
                image_url: "https://example.com/main.webp".into(),
                link_url: "https://example.com/rooms".into(),
                images: vec![
                    CandidateImage {
                        image_url: "https://example.com/quad.webp".into(),
                        label: "海景高級四人房".into(),
                    },
                    CandidateImage {
                        image_url: "https://example.com/common.webp".into(),
                        label: "公區".into(),
                    },
                    // placeholder/empty urls are skipped
                    CandidateImage {
                        image_url: "".into(),
                        label: "不該出現".into(),
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(html.contains("candidate-gallery"), "gallery wrapper");
        assert!(html.contains("https://example.com/quad.webp"));
        assert!(html.contains("海景高級四人房"));
        assert!(html.contains("公區"));
        assert!(
            !html.contains("不該出現"),
            "placeholder gallery rows skipped"
        );
        assert_eq!(html.matches("candidate-gallery-img").count(), 2);
        // Hotlinked photos must not leak the referrer — Booking's CDN 403s on some
        // external Referers, and Google's signed URLs are equally picky.
        assert_eq!(
            html.matches("referrerpolicy=\"no-referrer\"").count(),
            3,
            "hero + 2 gallery imgs each need it: {html}"
        );
        // thumbs link to the full image in a new tab
        assert!(html.contains("target=\"_blank\""));
        // rooms-and-availability external link
        assert!(html.contains("class=\"candidate-link\""));
        assert!(html.contains("href=\"https://example.com/rooms\""));
        assert!(html.contains("查看房型與空房 ↗"));
    }

    #[test]
    fn candidate_without_gallery_or_link_renders_neither() {
        use crate::model::DomesticCandidate;
        let plan = Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "CHLIV".into(),
                room_type: "海景雙人房".into(),
                image_url: "https://example.com/a.webp".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(!html.contains("candidate-gallery"));
        assert!(!html.contains("candidate-link"));
    }
}
