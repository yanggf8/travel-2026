pub mod activity_text;
pub mod alerts;
pub mod auth;
pub mod day;
pub mod index;
pub mod map;
pub mod session;
pub mod share;
pub mod summary;
use crate::model::Plan;
pub use activity_text::render_activity_text;

/// Wrap a rendered body in the full HTML page shell: charset, mobile viewport,
/// `notranslate` (the ZH content must not be browser-auto-translated), and the
/// inlined stylesheet.
pub fn page(title: &str, body: &str, lang: &str) -> String {
    let lang_attr = if lang == "en" { "en" } else { "zh-TW" };
    format!(
        "<!doctype html><html lang=\"{lang_attr}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"google\" content=\"notranslate\">\
         <title>{}</title><style>{}</style></head><body>{}</body></html>",
        esc(title),
        crate::styles::CSS,
        body,
    )
}

/// Render a full plan page: plan overview map, booking summary, then each day card.
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
        body.push_str(share::COPY_SCRIPT);
    }
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
    // day-trip map (only when one exists) and the hotel/airport map.
    let show_logistics = map_status.plan_logistics.is_some() || !plan.flights.is_empty();
    if map_status.plan_excursion.is_some() || show_logistics {
        body.push_str("<div class=\"map-insets\">");
        if let Some(v) = map_status.plan_excursion.as_deref() {
            body.push_str(&map::plan_excursion_map_slot(
                &plan.plan_id,
                v,
                lang,
                map_status.legend_for("plan-excursion.png"),
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
    for d in &plan.days {
        let map_ver = map_status
            .days
            .get(&d.day_number)
            .and_then(|v| v.as_deref());
        body.push_str(&day::render_with_logistics(
            d,
            &plan.plan_id,
            lang,
            map_ver,
            map_status
                .day_logistics
                .get(&d.day_number)
                .and_then(|v| v.as_deref()),
            map_status.legend_for(&format!("day-{}.png", d.day_number)),
            map_status.legend_for(&format!("day-{}-logistics.png", d.day_number)),
        ));
    }
    // Meal pending-booking alerts AFTER the day cards (mirror render.ts:1393),
    // then the transit cheat-sheet (mirror render.ts:1394).
    body.push_str(&alerts::render_pending_alerts(plan, lang, true));
    body.push_str(&alerts::render_transit_summary(plan, lang));
    page(&plan.display_name, &body, lang)
}

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
    let label = if lang == "en" { "Companion options · pick one" } else { "同行方案 · 可選擇" };
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
    out.push_str("</div>");
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
    let alt = if lang == "en" { "Companion options route map" } else { "同行方案路線圖" };
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
                    e.3.push_str(&format!("／{}", s.label));
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
            "<span class=\"swatch swatch-{}\"></span>「{}」",
            tags[i].to_lowercase(),
            esc(&o.title)
        ));
    }
    let note = if lang == "en" {
        " · unconfirmed, for discussion · © Esri</p>"
    } else {
        " · 尚未確認，僅供討論 · © Esri</p>"
    };
    out.push_str(note);
    out
}

/// AI-recommended provenance badge. Empty string for confirmed/any non-ai_recommended source, so
/// callers can append it unconditionally. Bilingual: ZH default, English on ?lang=en.
pub fn ai_rec_badge(source: &str, lang: &str) -> String {
    if source != "ai_recommended" {
        return String::new();
    }
    let label = if lang == "en" {
        "\u{1F916} AI-recommended (unconfirmed)"
    } else {
        "\u{1F916} 建議（未確認）"
    };
    format!("<span class=\"ai-rec-badge\">{}</span>", esc(label))
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
            days: [(1i64, Some("etag2".into()))].into_iter().collect(),
            day_logistics: std::collections::HashMap::new(),
            legends: [
                (
                    "plan.png".to_string(),
                    vec![map::LegendStop {
                        seq: 1,
                        label: "首里城".into(),
                        lat: 26.2186,
                        lon: 127.7196,
                    }],
                ),
                (
                    "day-1.png".to_string(),
                    vec![map::LegendStop {
                        seq: 2,
                        label: "国際通り".into(),
                        lat: 26.2125,
                        lon: 127.6809,
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
