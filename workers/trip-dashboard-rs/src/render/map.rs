use super::{esc, esc_url_attr, render_activity_text};
use crate::i18n;
use crate::model::Stop;
use std::collections::HashMap;

/// PNG magic bytes — first four bytes of every valid PNG file.
const PNG_MAGIC: [u8; 4] = [0x89, 0x50, 0x4E, 0x47];

/// Minimum byte length for a real map PNG — kept in sync with `MIN_PNG_BYTES` in
/// travel-cli snapshot_maps.rs so the upload gate and the serve gate agree. This is only
/// a tiny-garbage/stub guard (a real route diagram is hundreds of KB); it does NOT detect
/// a blank-but-valid PNG — `travel snapshot-maps` validates that it rendered itinerary
/// points and writes the PNG directly in Rust before uploading it to R2.
pub const MIN_MAP_PNG_BYTES: usize = 200;

/// Server-side map availability for a plan page render. Built in the async router
/// (R2 HEAD/get per key) and threaded into the sync render layer.
#[derive(Debug, Default)]
pub struct MapStatus {
    /// `Some(version)` when a real PNG is present; the version is stamped onto the
    /// URL so a re-snapshot reaches viewers immediately. Map PNGs are served
    /// `Cache-Control: public, max-age=86400`, so a stable URL meant a refreshed map
    /// stayed invisible to anyone who had already opened the page for a full day —
    /// which is exactly how a batch of watermarked maps kept being served after they
    /// had been regenerated.
    pub plan: Option<String>,
    /// Plan-wide map containing only hotel and airport route endpoints.
    pub plan_logistics: Option<String>,
    /// Far-off day-trip map (`plan-excursion.png`, from `set-day-excursion`),
    /// kept off the overview so it doesn't flatten the city's zoom.
    pub plan_excursion: Option<String>,
    /// Arrival/departure-day map (`plan-airport.png`): the days whose stops
    /// include an airport terminal, kept off the overview for the same reason.
    pub plan_airport: Option<String>,
    pub days: HashMap<i64, Option<String>>,
    /// Daily hotel-to-destination return maps (`day-{n}-logistics.png`).
    pub day_logistics: HashMap<i64, Option<String>>,
    /// Daily beyond-the-area maps (`day-{n}-away.png`): the day's stops more
    /// than 500 km from the destination hub (e.g. the home-side leg to the
    /// departure airport), which snapshot-maps keeps off the area maps.
    pub day_away: HashMap<i64, Option<String>>,
    /// Numbered-pin legends per map key (`plan.png`, `day-3.png`, …), loaded from
    /// the `map_legend_stops` rows the CLI's snapshot-maps writes in lockstep with
    /// the PNG it uploads. Empty for snapshots taken before the table existed —
    /// the map then renders without a legend, exactly as before.
    pub legends: HashMap<String, Vec<LegendStop>>,
}

/// One numbered map pin's legend entry: `seq` is the pin's row order (assigned
/// by snapshot-maps in pin order), `label` the place name in the itinerary's
/// own language, `lat`/`lon` the pin coordinates for the keyless Google Maps
/// link (same pattern as `Stop::maps_link`). `pin_no` is the number drawn on
/// the PNG (a multi-day map numbers each day from 1), `day_number` the day the
/// legend groups the pin under and `color` its fill as written by snapshot-maps;
/// all three are None on rows written before those columns existed.
#[derive(Clone, Debug, Default)]
pub struct LegendStop {
    pub seq: i64,
    pub label: String,
    pub lat: f64,
    pub lon: f64,
    pub day_number: Option<i64>,
    pub pin_no: Option<i64>,
    pub color: Option<String>,
}

impl MapStatus {
    /// Legend rows for one map key (`"plan.png"`, `"day-3.png"`, …), empty when
    /// none were recorded for it.
    pub fn legend_for(&self, map_key: &str) -> &[LegendStop] {
        self.legends.get(map_key).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// True when the body is a real PNG map (not a 1-byte garbage capture or tiny stub).
/// Uses `>=` to match the Rust renderer's `png.len() < MIN_PNG_BYTES` reject (i.e. a PNG of exactly
/// MIN bytes is accepted by BOTH the upload gate and this serve gate — no boundary gap).
pub fn is_valid_map_png(bytes: &[u8]) -> bool {
    bytes.len() >= MIN_MAP_PNG_BYTES
        && bytes.len() >= PNG_MAGIC.len()
        && bytes[..PNG_MAGIC.len()] == PNG_MAGIC
}

fn day_route_caption(day_number: i64, lang: &str) -> String {
    if lang == "en" {
        format!("Day {day_number} route")
    } else {
        format!("第 {day_number} 天路線")
    }
}

/// Numbered-pin legend under a map: one item per pin, "N name", each linking to a
/// keyless Google Maps search on the pin's coordinates. Rendered only when the map
/// itself is present — stale legend rows for an absent map must not leak out.
/// Rows that carry a day render grouped by day — a heading with the day's pin
/// color and 第 N 天 / Day N, then that day's pins by the number drawn on them;
/// day-less rows (logistics maps, older snapshots) render as one flat list.
fn map_legend_html(legend: &[LegendStop], lang: &str) -> String {
    if legend.is_empty() {
        return String::new();
    }
    if legend.iter().all(|s| s.day_number.is_none()) {
        return legend_list_html(legend.iter());
    }
    // Days in first-appearance (seq) order; each day's rows keep their order.
    let mut days: Vec<i64> = Vec::new();
    for d in legend.iter().filter_map(|s| s.day_number) {
        if !days.contains(&d) {
            days.push(d);
        }
    }
    let mut h = String::from("<div class=\"map-legend-days\">");
    for d in days {
        let rows = legend.iter().filter(|s| s.day_number == Some(d));
        let swatch = legend
            .iter()
            .filter(|s| s.day_number == Some(d))
            .find_map(|s| s.color.as_deref().filter(|c| is_hex_color(c)))
            .map(|c| format!("<span class=\"leg-swatch\" style=\"background:{c}\"></span>"))
            .unwrap_or_default();
        let label = if lang == "en" { format!("Day {d}") } else { format!("第 {d} 天") };
        h.push_str(&format!(
            "<div class=\"map-legend-day\"><div class=\"leg-day\">{swatch}{}</div>{}</div>",
            esc(&label),
            legend_list_html(rows),
        ));
    }
    let day_less: Vec<&LegendStop> = legend.iter().filter(|s| s.day_number.is_none()).collect();
    if !day_less.is_empty() {
        h.push_str(&legend_list_html(day_less.into_iter()));
    }
    h.push_str("</div>");
    h
}

/// One `<ol class="map-legend">` of pins, numbered by `pin_no` (the number on
/// the PNG), falling back to `seq` for rows written before `pin_no` existed.
fn legend_list_html<'a>(rows: impl Iterator<Item = &'a LegendStop>) -> String {
    let mut h = String::from("<ol class=\"map-legend\">");
    for s in rows {
        h.push_str(&format!(
            "<li><span class=\"leg-num\">{}</span><a href=\"https://www.google.com/maps?q={},{}\" \
             target=\"_blank\" rel=\"noopener\">{}</a></li>",
            s.pin_no.unwrap_or(s.seq),
            s.lat,
            s.lon,
            esc(&s.label),
        ));
    }
    h.push_str("</ol>");
    h
}

/// `#rrggbb` and nothing else: the DB color goes into an inline style, so any
/// other value (a name, a url(), a quote) gets no swatch at all.
fn is_hex_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Framed plan-overview map slot. Emits a real `<img>` only when `has_map` is true;
/// otherwise a styled missing placeholder (never a blind broken-image `<img>`).
pub fn plan_map_slot(plan_id: &str, version: Option<&str>, lang: &str, legend: &[LegendStop]) -> String {
    let caption = i18n::t("tripOverview", lang);
    if let Some(v) = version {
        format!(
            "<figure class=\"map-frame\"><img class=\"planmap\" alt=\"{}\" \
             src=\"/map/{}/plan.png{}\"><figcaption>{}</figcaption>{}</figure>",
            esc(caption),
            esc_url_attr(plan_id),
            cache_bust(v),
            map_caption(caption),
            map_legend_html(legend, lang),
        )
    } else {
        let not_avail = i18n::t("mapNotAvailable", lang);
        format!(
            "<figure class=\"map-frame map-missing\"><div class=\"map-missing-box\">{}</div>\
             <figcaption>{}</figcaption></figure>",
            esc(not_avail),
            esc(caption),
        )
    }
}

/// Separate plan-wide hotel/airport map so distant endpoints do not flatten the
/// sightseeing overview's zoom level. Rendered as a COMPACT INSET (崁入小圖),
/// not a second full-size map — the overview is the page's primary map; this
/// one is a supplementary 機場/住宿示意 (user request 2026-09-28). Domestic
/// plans (no airport segment) never render this slot at all — see render/mod.rs.
pub fn plan_logistics_map_slot(
    plan_id: &str,
    version: Option<&str>,
    lang: &str,
    legend: &[LegendStop],
) -> String {
    let caption = i18n::t("planLogisticsMap", lang);
    if let Some(v) = version {
        format!(
            "<figure class=\"map-frame map-frame--inset\"><img class=\"planmap planmap--inset\" alt=\"{}\" \
             src=\"/map/{}/plan-logistics.png{}\"><figcaption>{}</figcaption>{}</figure>",
            esc(caption),
            esc_url_attr(plan_id),
            cache_bust(v),
            map_caption(caption),
            map_legend_html(legend, lang),
        )
    } else {
        let not_avail = i18n::t("mapNotAvailable", lang);
        format!(
            "<figure class=\"map-frame map-frame--inset map-missing\"><div class=\"map-missing-box\">{}</div>\
             <figcaption>{}</figcaption></figure>",
            esc(not_avail),
            esc(caption),
        )
    }
}

/// Day-trip inset (`plan-excursion.png`): a far-off excursion day drawn on its own
/// so the sightseeing overview keeps the city's zoom. Same compact inset frame as
/// the hotel/airport map, placed beside it. Only rendered when the PNG exists —
/// most plans have no excursion day, so there is no placeholder.
pub fn plan_excursion_map_slot(
    plan_id: &str,
    version: &str,
    lang: &str,
    legend: &[LegendStop],
) -> String {
    let caption = i18n::t("planExcursionMap", lang);
    format!(
        "<figure class=\"map-frame map-frame--inset\"><img class=\"planmap planmap--inset\" alt=\"{}\" \
         src=\"/map/{}/plan-excursion.png{}\"><figcaption>{}</figcaption>{}</figure>",
        esc(caption),
        esc_url_attr(plan_id),
        cache_bust(version),
        map_caption(caption),
        map_legend_html(legend, lang),
    )
}

/// Arrival/departure-day inset (`plan-airport.png`): every day whose stops include
/// an airport terminal, drawn on its own so the overview keeps the city's zoom.
/// Same compact inset frame as the day-trip map, placed beside it. Only rendered
/// when the PNG exists — a plan without flights has none, so no placeholder.
pub fn plan_airport_map_slot(
    plan_id: &str,
    version: &str,
    lang: &str,
    legend: &[LegendStop],
) -> String {
    let caption = i18n::t("planAirportMap", lang);
    format!(
        "<figure class=\"map-frame map-frame--inset\"><img class=\"planmap planmap--inset\" alt=\"{}\" \
         src=\"/map/{}/plan-airport.png{}\"><figcaption>{}</figcaption>{}</figure>",
        esc(caption),
        esc_url_attr(plan_id),
        cache_bust(version),
        map_caption(caption),
        map_legend_html(legend, lang),
    )
}

/// Framed per-day route map slot. Same contract as `plan_map_slot`.
pub fn day_map_slot(
    plan_id: &str,
    day_number: i64,
    version: Option<&str>,
    lang: &str,
    legend: &[LegendStop],
) -> String {
    let caption = day_route_caption(day_number, lang);
    if let Some(v) = version {
        format!(
            "<figure class=\"map-frame\"><img class=\"daymap\" alt=\"{}\" \
             src=\"/map/{}/day-{}.png{}\"><figcaption>{}</figcaption>{}</figure>",
            esc(&caption),
            esc_url_attr(plan_id),
            day_number,
            cache_bust(v),
            map_caption(&caption),
            map_legend_html(legend, lang),
        )
    } else {
        let not_avail = i18n::t("mapNotAvailable", lang);
        format!(
            "<figure class=\"map-frame map-missing\"><div class=\"map-missing-box\">{}</div>\
             <figcaption>{}</figcaption></figure>",
            esc(not_avail),
            esc(&caption),
        )
    }
}

/// Compact map for the day's hotel-to-destination and return legs.
pub fn day_logistics_map_slot(
    plan_id: &str,
    day_number: i64,
    version: Option<&str>,
    lang: &str,
    legend: &[LegendStop],
) -> String {
    let caption = if lang == "en" {
        format!("Day {day_number} hotel round trip")
    } else {
        format!("第 {day_number} 天・旅社來回")
    };
    compact_day_map_slot(plan_id, day_number, "logistics", "", &caption, version, lang, legend)
}

/// Compact map for the day's stops beyond the destination area
/// (`day-{n}-away.png`); most days have none, and then nothing renders.
pub fn day_away_map_slot(
    plan_id: &str,
    day_number: i64,
    version: Option<&str>,
    lang: &str,
    legend: &[LegendStop],
) -> String {
    let caption = if lang == "en" {
        format!("Day {day_number} beyond the area")
    } else {
        format!("第 {day_number} 天・本區以外的行程")
    };
    compact_day_map_slot(plan_id, day_number, "away", " map-frame--day-away", &caption, version, lang, legend)
}

/// The compact per-day map frame shared by `day-{n}-{suffix}.png` maps: only
/// an uploaded PNG renders (no missing-map placeholder slot).
fn compact_day_map_slot(
    plan_id: &str,
    day_number: i64,
    suffix: &str,
    extra_class: &str,
    caption: &str,
    version: Option<&str>,
    lang: &str,
    legend: &[LegendStop],
) -> String {
    if let Some(v) = version {
        format!(
            "<figure class=\"map-frame map-frame--day-logistics{}\"><img class=\"planmap planmap--inset\" alt=\"{}\" src=\"/map/{}/day-{}-{}.png{}\"><figcaption>{}</figcaption>{}</figure>",
            extra_class,
            esc(caption),
            esc_url_attr(plan_id),
            day_number,
            suffix,
            cache_bust(v),
            map_caption(caption),
            map_legend_html(legend, lang),
        )
    } else {
        String::new()
    }
}

/// OSM attribution is printed into each PNG and linked in the surrounding HTML so viewers
/// can reach the data licence and source information.
fn map_caption(caption: &str) -> String {
    format!(
        "{} <span class=\"map-attribution\">© <a href=\"https://www.openstreetmap.org/copyright\" target=\"_blank\" rel=\"noopener\">OpenStreetMap contributors</a></span>",
        esc(caption)
    )
}

/// A list of stops with their Google Maps links (keyless q=lat,lon). The
/// `maps_link` field was built by the model from trusted lat/lon (or a
/// /maps/search/<title> fallback) — render it as the href; esc() guards the
/// attribute and text.
pub fn stop_list(stops: &[Stop]) -> String {
    if stops.is_empty() {
        return String::new();
    }
    let mut h = String::from("<ul class=\"stoplist\">");
    for s in stops {
        // Ticket price badge — shown only for paid POIs (cost_estimate > 0; 0 = free).
        let price = if s.cost_estimate > 0 {
            format!("<span class=\"stop-price\">🎫¥{}</span>", s.cost_estimate)
        } else {
            String::new()
        };
        let addr = if s.address.is_empty() {
            String::new()
        } else {
            format!("<span class=\"addr\">{}</span>", esc(&s.address))
        };
        // An empty maps_link means the model deliberately suppressed a (broken)
        // search-link fallback because the activity text already carries an inline
        // map link. Render the stop name as plain text (no dead/garbage anchor).
        if s.maps_link.is_empty() {
            h.push_str(&format!(
                "<li>{}{}{}</li>",
                render_activity_text(&s.title),
                addr,
                price
            ));
        } else {
            let title = stop_visible_title(&s.title);
            h.push_str(&format!(
                "<li><a href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a>{}{}</li>",
                esc_url_attr(&s.maps_link),
                esc(&title),
                addr,
                price
            ));
        }
    }
    h.push_str("</ul>");
    h
}

/// `?v=<version>` for a map URL. The version is the R2 object's ETag, reduced to
/// URL-safe characters — its exact value is irrelevant, only that it CHANGES when the
/// PNG does. An empty/unusable version yields no query string rather than `?v=`.
fn cache_bust(version: &str) -> String {
    let v: String = version
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(32)
        .collect();
    if v.is_empty() {
        String::new()
    } else {
        format!("?v={v}")
    }
}

fn stop_visible_title(title: &str) -> String {
    let bytes = title.as_bytes();
    let needle = b"google maps";
    if bytes.len() < needle.len() {
        return title.trim().to_string();
    }
    for i in 0..=bytes.len().saturating_sub(needle.len()) {
        if bytes[i..i + needle.len()].eq_ignore_ascii_case(needle) {
            return title[..i].trim().to_string();
        }
    }
    title.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Stop;

    fn leg(seq: i64, label: &str, lat: f64, lon: f64) -> LegendStop {
        LegendStop {
            seq,
            label: label.into(),
            lat,
            lon,
            ..Default::default()
        }
    }

    /// A row as snapshot-maps now writes it for a multi-day map.
    fn day_leg(seq: i64, day: i64, pin: i64, color: &str, label: &str) -> LegendStop {
        LegendStop {
            day_number: Some(day),
            pin_no: Some(pin),
            color: Some(color.into()),
            ..leg(seq, label, 35.0 + seq as f64 / 100.0, 135.0)
        }
    }

    #[test]
    fn map_legend_lists_numbered_clickable_stops() {
        let legend = vec![
            leg(1, "金閣寺", 35.0394, 135.7292),
            leg(2, "北野天滿宮", 35.0117, 135.7409),
        ];
        let h = map_legend_html(&legend, "zh");
        assert!(h.contains("<ol class=\"map-legend\">"), "got: {h}");
        assert!(h.contains("<span class=\"leg-num\">1</span>"), "got: {h}");
        assert!(h.contains("<span class=\"leg-num\">2</span>"), "got: {h}");
        // Keyless Google Maps link, same pattern as stop_list.
        assert!(h.contains("https://www.google.com/maps?q=35.0394,135.7292"), "got: {h}");
        assert!(h.contains(">金閣寺</a>"), "got: {h}");
        // Legend order follows seq (pin numbering).
        let p1 = h.find("金閣寺").expect("first label");
        let p2 = h.find("北野天滿宮").expect("second label");
        assert!(p1 < p2, "seq order must be preserved");
    }

    #[test]
    fn day_rows_render_grouped_by_day_with_their_colors() {
        // plan.png: day 1 has two pins, day 2 one; pin numbers restart per day.
        let legend = vec![
            day_leg(1, 1, 1, "#e6194b", "金閣寺"),
            day_leg(2, 1, 2, "#e6194b", "清水寺"),
            day_leg(3, 2, 1, "#3CB44B", "伏見稻荷"),
        ];
        let h = map_legend_html(&legend, "zh");
        assert!(h.starts_with("<div class=\"map-legend-days\">"), "{h}");
        assert_eq!(h.matches("<div class=\"map-legend-day\">").count(), 2, "{h}");
        let day1 = h.find(">第 1 天</div>").expect("day 1 heading");
        let day2 = h.find(">第 2 天</div>").expect("day 2 heading");
        assert!(day1 < h.find("金閣寺").unwrap() && h.find("清水寺").unwrap() < day2, "{h}");
        assert!(day2 < h.find("伏見稻荷").unwrap(), "{h}");
        // Swatches carry the DB colors (either hex case).
        assert!(h.contains("<span class=\"leg-swatch\" style=\"background:#e6194b\"></span>第 1 天"), "{h}");
        assert!(h.contains("<span class=\"leg-swatch\" style=\"background:#3CB44B\"></span>第 2 天"), "{h}");
        // Items are numbered by the number drawn on the pin, not by seq.
        let day2_list = &h[day2..];
        assert!(day2_list.contains("<span class=\"leg-num\">1</span><a href=\"https://www.google.com/maps?q=35.03,135\""), "{h}");
        assert!(!h.contains("<span class=\"leg-num\">3</span>"), "{h}");
        // Links and escaping as in the flat legend.
        assert!(h.contains(">伏見稻荷</a>"), "{h}");

        let en = map_legend_html(&legend, "en");
        assert!(en.contains("</span>Day 1</div>") && en.contains("</span>Day 2</div>"), "{en}");
        assert!(!en.contains("第 1 天"), "{en}");
    }

    #[test]
    fn swatch_only_for_a_strict_hex_color() {
        for bad in [
            "red",
            "#e6194",
            "#e6194bb",
            "#gg194b",
            "e6194b1",
            "#e6194b;background:url(x)",
            "\"><script>alert(1)</script>",
            "",
        ] {
            let h = map_legend_html(&[day_leg(1, 1, 1, bad, "a")], "zh");
            assert!(!h.contains("leg-swatch"), "{bad:?} → {h}");
            assert!(!h.contains("<script>"), "{bad:?} → {h}");
            assert!(h.contains(">第 1 天</div>"), "the day heading stays: {h}");
        }
        // A later row of the same day may supply the color.
        let h = map_legend_html(&[day_leg(1, 1, 1, "nope", "a"), day_leg(2, 1, 2, "#0080ff", "b")], "zh");
        assert!(h.contains("background:#0080ff"), "{h}");
        // No color recorded at all: heading without a swatch.
        let mut bare = day_leg(1, 4, 1, "", "a");
        bare.color = None;
        let h = map_legend_html(&[bare], "en");
        assert!(!h.contains("leg-swatch") && h.contains(">Day 4</div>"), "{h}");
    }

    #[test]
    fn rows_without_a_day_render_flat_as_before() {
        // Old rows (NULL day/pin/color) render exactly as the legend always did.
        let legend = vec![leg(1, "金閣寺", 35.0394, 135.7292), leg(2, "北野天滿宮", 35.0117, 135.7409)];
        let h = map_legend_html(&legend, "en");
        assert_eq!(
            h,
            "<ol class=\"map-legend\">\
             <li><span class=\"leg-num\">1</span><a href=\"https://www.google.com/maps?q=35.0394,135.7292\" target=\"_blank\" rel=\"noopener\">金閣寺</a></li>\
             <li><span class=\"leg-num\">2</span><a href=\"https://www.google.com/maps?q=35.0117,135.7409\" target=\"_blank\" rel=\"noopener\">北野天滿宮</a></li>\
             </ol>"
        );
        assert!(!h.contains("map-legend-day") && !h.contains("第"), "{h}");
        // A logistics map's rows carry a pin number and color but no day: flat.
        let mut hotel = leg(1, "APA", 34.98, 135.75);
        hotel.pin_no = Some(1);
        hotel.color = Some("#1565c0".into());
        let h = map_legend_html(&[hotel], "zh");
        assert!(h.starts_with("<ol class=\"map-legend\">") && !h.contains("leg-swatch"), "{h}");
        // A day-less row beside day rows (a domestic stay on plan.png) trails
        // the day groups as its own list.
        let mut stay = leg(3, "海論民宿", 25.1, 121.8);
        stay.pin_no = Some(1);
        let h = map_legend_html(&[day_leg(1, 1, 1, "#e6194b", "九份"), day_leg(2, 2, 1, "#3cb44b", "十分"), stay], "zh");
        let last_day = h.find(">十分</a>").unwrap();
        let stay_at = h.find(">海論民宿</a>").expect("stay listed");
        assert!(stay_at > last_day, "{h}");
        assert_eq!(h.matches("<ol class=\"map-legend\">").count(), 3, "{h}");
        assert!(h.ends_with("</ol></div>"), "{h}");
    }

    #[test]
    fn empty_map_legend_renders_nothing() {
        assert_eq!(map_legend_html(&[], "zh"), "");
    }

    #[test]
    fn plan_map_slot_renders_legend_under_caption_when_map_present() {
        let legend = vec![leg(1, "金閣寺", 35.0394, 135.7292)];
        let h = plan_map_slot("kyoto-2026", Some("v1"), "zh", &legend);
        assert!(h.contains("<ol class=\"map-legend\">"), "got: {h}");
        let cap = h.find("</figcaption>").expect("caption");
        let legend_pos = h.find("<ol class=\"map-legend\">").expect("legend");
        assert!(legend_pos > cap, "legend renders after the caption");
        // No map → no legend, even when rows exist (stale rows for an absent map).
        let missing = plan_map_slot("kyoto-2026", None, "zh", &legend);
        assert!(!missing.contains("map-legend"), "got: {missing}");
    }

    #[test]
    fn day_map_slot_renders_legend_when_map_present() {
        let legend = vec![leg(3, "二年坂", 34.9976, 135.7817)];
        let h = day_map_slot("kyoto-2026", 2, Some("v1"), "zh", &legend);
        assert!(h.contains("<span class=\"leg-num\">3</span>"), "got: {h}");
        assert!(h.contains("https://www.google.com/maps?q=34.9976,135.7817"), "got: {h}");
        let missing = day_map_slot("kyoto-2026", 2, None, "zh", &legend);
        assert!(!missing.contains("map-legend"), "got: {missing}");
    }

    #[test]
    fn stop_list_links_to_maps() {
        let stops = vec![Stop {
            title: "Naminoue".into(),
            maps_link: "https://www.google.com/maps?q=26.2,127.6".into(),
            ..Default::default()
        }];
        let h = stop_list(&stops);
        assert!(h.contains("q=26.2,127.6"));
        assert!(h.contains("Naminoue"));
    }

    #[test]
    fn stop_list_plain_embedded_maps_url_renders_short_label() {
        let stops = vec![Stop {
            title:
                "午餐（實際）：安里屋すば\nGoogle Maps：https://www.google.com/maps/search/asatoya"
                    .into(),
            maps_link: "".into(),
            ..Default::default()
        }];
        let h = stop_list(&stops);
        assert!(h.contains("🗺️ Google Maps"), "got: {h}");
        assert!(
            !h.contains(">https://www.google.com/maps/search/"),
            "got: {h}"
        );
    }

    #[test]
    fn stop_list_linked_stop_strips_embedded_maps_url_from_label() {
        let stops = vec![Stop {
            title: "DMM かりゆし水族館\nGoogle Maps：https://www.google.com/maps/search/dmm".into(),
            maps_link: "https://www.google.com/maps?q=26.156,127.650".into(),
            ..Default::default()
        }];
        let h = stop_list(&stops);
        assert!(h.contains(">DMM かりゆし水族館</a>"), "got: {h}");
        assert!(!h.contains("Google Maps："), "got: {h}");
        assert!(!h.contains("search/dmm"), "got: {h}");
    }

    #[test]
    fn stop_list_linked_stop_strip_is_safe_with_non_ascii_before_google_maps() {
        let stops = vec![Stop {
            title: "İ午餐：安里屋すば\nGoogle Maps：https://www.google.com/maps/search/asatoya"
                .into(),
            maps_link: "https://www.google.com/maps?q=26.217,127.696".into(),
            ..Default::default()
        }];
        let h = stop_list(&stops);
        assert!(h.contains(">İ午餐：安里屋すば</a>"), "got: {h}");
        assert!(!h.contains("maps/search/asatoya"), "got: {h}");
    }

    #[test]
    fn map_url_carries_a_cache_busting_version() {
        // Map PNGs are served max-age=86400. Without a version in the URL a
        // re-snapshot stays invisible to anyone who already opened the page.
        let h = plan_map_slot("jiufen-2026", Some("\"abc123\""), "zh", &[]);
        assert!(h.contains("/map/jiufen-2026/plan.png?v=abc123"), "{h}");
        let d = day_map_slot("jiufen-2026", 2, Some("W/\"deadbeef\""), "zh", &[]);
        assert!(d.contains("/map/jiufen-2026/day-2.png?v=Wdeadbeef"), "{d}");
        // A different PNG must produce a different URL.
        let other = plan_map_slot("jiufen-2026", Some("zzz999"), "zh", &[]);
        assert_ne!(h, other);
    }

    #[test]
    fn unusable_version_emits_no_query_string() {
        let h = plan_map_slot("jiufen-2026", Some("\"\""), "zh", &[]);
        assert!(h.contains("/map/jiufen-2026/plan.png\""), "no dangling ?v=: {h}");
        assert!(!h.contains("?v="), "{h}");
    }

    #[test]
    fn plan_map_slot_with_map_emits_img() {
        let h = plan_map_slot("okinawa-2026", Some("v1"), "en", &[]);
        assert!(h.contains("map-frame"));
        assert!(h.contains("class=\"planmap\""));
        assert!(h.contains("/map/okinawa-2026/plan.png"));
        // With a map, the caption carries the linked OSM attribution (map_caption).
        assert!(h.contains("<figcaption>Sightseeing overview <span"));
        assert!(h.contains("OpenStreetMap contributors"));
        assert!(!h.contains("map-missing"));
    }

    #[test]
    fn plan_map_slot_without_map_emits_placeholder() {
        let h = plan_map_slot("okinawa-2026", None, "en", &[]);
        assert!(h.contains("map-frame map-missing"));
        assert!(h.contains("map-missing-box"));
        assert!(h.contains("Map not available yet"));
        assert!(h.contains("<figcaption>Sightseeing overview</figcaption>"));
        assert!(!h.contains("src=\"/map"));
        assert!(!h.contains("<img"));
    }

    #[test]
    fn plan_logistics_map_slot_renders_as_compact_inset() {
        // With a map: inset frame + inset img class (崁入小圖, not a second full map).
        let h = plan_logistics_map_slot("kyoto-2026", Some("v1"), "zh", &[]);
        assert!(h.contains("map-frame map-frame--inset"), "{h}");
        assert!(h.contains("planmap planmap--inset"), "{h}");
        assert!(h.contains("/map/kyoto-2026/plan-logistics.png"));
        // The overview map stays full-size (no inset classes leak into it).
        let overview = plan_map_slot("kyoto-2026", Some("v1"), "zh", &[]);
        assert!(!overview.contains("map-frame--inset"), "{overview}");
        assert!(!overview.contains("planmap--inset"), "{overview}");
        // Missing logistics map: still compact (smaller placeholder box).
        let m = plan_logistics_map_slot("kyoto-2026", None, "zh", &[]);
        assert!(m.contains("map-frame--inset"), "{m}");
        assert!(m.contains("map-missing"));
    }

    #[test]
    fn day_logistics_map_slot_renders_only_an_uploaded_map() {
        let legend = [leg(1, "旅社", 26.2, 127.7)];
        let h = day_logistics_map_slot("okinawa-2026", 1, Some("etag-1"), "zh", &legend);
        // The whole figure, spelled exactly as before compact_day_map_slot existed.
        assert_eq!(
            h,
            format!(
                "<figure class=\"map-frame map-frame--day-logistics\"><img class=\"planmap planmap--inset\" alt=\"第 1 天・旅社來回\" src=\"/map/okinawa-2026/day-1-logistics.png?v=etag1\"><figcaption>{}</figcaption>{}</figure>",
                map_caption("第 1 天・旅社來回"),
                map_legend_html(&legend, "zh"),
            )
        );
        assert!(day_logistics_map_slot("okinawa-2026", 1, Some("e"), "en", &[]).contains("Day 1 hotel round trip"));
        assert_eq!(day_logistics_map_slot("okinawa-2026", 1, None, "zh", &[]), "");
    }

    #[test]
    fn day_away_map_slot_renders_only_an_uploaded_map() {
        let legend = [leg(1, "紅樹林 淡水", 25.1677, 121.4456)];
        let h = day_away_map_slot("okinawa-2026", 1, Some("etag-2"), "zh", &legend);
        assert!(
            h.starts_with(
                "<figure class=\"map-frame map-frame--day-logistics map-frame--day-away\"><img class=\"planmap planmap--inset\""
            ),
            "{h}"
        );
        assert!(h.contains("src=\"/map/okinawa-2026/day-1-away.png?v=etag2\""), "{h}");
        assert!(h.contains("alt=\"第 1 天・本區以外的行程\""), "{h}");
        assert!(h.contains("<figcaption>第 1 天・本區以外的行程 "), "{h}");
        assert!(h.contains("紅樹林 淡水"), "legend under the map: {h}");
        let en = day_away_map_slot("okinawa-2026", 1, Some("e"), "en", &[]);
        assert!(en.contains("<figcaption>Day 1 beyond the area "), "{en}");
        // No away map uploaded: no slot and no missing-map placeholder.
        assert_eq!(day_away_map_slot("okinawa-2026", 1, None, "zh", &legend), "");
    }

    #[test]
    fn plan_airport_map_slot_is_an_inset_with_a_day_legend() {
        let legend = [day_leg(1, 1, 1, "#e6194b", "関西空港"), day_leg(2, 5, 1, "#4363d8", "清水寺")];
        let h = plan_airport_map_slot("osaka-nov-2026", "\"etag-7\"", "zh", &legend);
        assert!(h.starts_with("<figure class=\"map-frame map-frame--inset\"><img class=\"planmap planmap--inset\""), "{h}");
        assert!(h.contains("src=\"/map/osaka-nov-2026/plan-airport.png?v=etag7\""), "{h}");
        assert!(h.contains("alt=\"機場來回\""), "{h}");
        assert!(h.contains("<figcaption>機場來回 "), "{h}");
        let cap = h.find("</figcaption>").unwrap();
        assert!(h.find("<div class=\"map-legend-days\">").expect("grouped legend") > cap, "{h}");
        assert!(h.contains(">第 5 天</div>"), "{h}");
        let en = plan_airport_map_slot("osaka-nov-2026", "e", "en", &legend);
        assert!(en.contains("<figcaption>Airport days ") && en.contains(">Day 5</div>"), "{en}");
    }

    #[test]
    fn plan_excursion_map_slot_is_an_inset() {
        let h = plan_excursion_map_slot("osaka-nov-2026", "v9", "zh", &[]);
        assert!(h.contains("map-frame map-frame--inset"), "{h}");
        assert!(h.contains("/map/osaka-nov-2026/plan-excursion.png"), "{h}");
        assert!(h.contains("一日遊"), "{h}");
    }

    #[test]
    fn day_map_slot_with_map_emits_img() {
        let h = day_map_slot("okinawa-2026", 2, Some("v1"), "zh", &[]);
        assert!(h.contains("map-frame"));
        assert!(h.contains("class=\"daymap\""));
        assert!(h.contains("/map/okinawa-2026/day-2.png"));
        assert!(h.contains("第 2 天路線"));
        assert!(!h.contains("map-missing"));
    }

    #[test]
    fn day_map_slot_without_map_emits_placeholder_zh() {
        let h = day_map_slot("okinawa-2026", 3, None, "zh", &[]);
        assert!(h.contains("map-missing"));
        assert!(h.contains("地圖尚未產生"));
        assert!(h.contains("第 3 天路線"));
        assert!(!h.contains("src=\"/map"));
    }

    #[test]
    fn is_valid_map_png_rejects_tiny_and_garbage() {
        assert!(!is_valid_map_png(&[0x89]));
        assert!(!is_valid_map_png(&[0u8; MIN_MAP_PNG_BYTES])); // at floor but no PNG magic
        assert!(!is_valid_map_png(b"not-a-png"));
        // below the floor with magic is rejected
        let mut below = vec![0x89, 0x50, 0x4E, 0x47];
        below.resize(MIN_MAP_PNG_BYTES - 1, 0);
        assert!(!is_valid_map_png(&below));
        // EXACTLY at the floor with PNG magic is ACCEPTED — matches the script's upload
        // gate (`sz -lt MIN` rejects, so >= MIN is accepted on both sides; no boundary gap)
        let mut at_floor = vec![0x89, 0x50, 0x4E, 0x47];
        at_floor.resize(MIN_MAP_PNG_BYTES, 0);
        assert!(is_valid_map_png(&at_floor));
    }

    #[test]
    fn empty_stops_render_nothing() {
        assert_eq!(stop_list(&[]), "");
    }

    #[test]
    fn stop_with_cost_shows_price_badge() {
        let stops = vec![Stop {
            title: "Shuri Castle".into(),
            maps_link: "https://www.google.com/maps?q=26.2,127.7".into(),
            cost_estimate: 530,
            ..Default::default()
        }];
        let h = stop_list(&stops);
        assert!(h.contains("stop-price"), "got: {h}");
        assert!(h.contains("¥530"), "got: {h}");
    }

    #[test]
    fn free_stop_shows_no_price() {
        let stops = vec![Stop {
            title: "Free Beach".into(),
            maps_link: "https://www.google.com/maps?q=1,1".into(),
            cost_estimate: 0,
            ..Default::default()
        }];
        let h = stop_list(&stops);
        assert!(!h.contains("stop-price"), "got: {h}");
        assert!(!h.contains('¥'), "got: {h}");
    }
}
