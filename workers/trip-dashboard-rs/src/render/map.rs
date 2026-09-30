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
    pub days: HashMap<i64, Option<String>>,
    /// Daily hotel-to-destination return maps (`day-{n}-logistics.png`).
    pub day_logistics: HashMap<i64, Option<String>>,
    /// Numbered-pin legends per map key (`plan.png`, `day-3.png`, …), loaded from
    /// the `map_legend_stops` rows the CLI's snapshot-maps writes in lockstep with
    /// the PNG it uploads. Empty for snapshots taken before the table existed —
    /// the map then renders without a legend, exactly as before.
    pub legends: HashMap<String, Vec<LegendStop>>,
}

/// One numbered map pin's legend entry: `seq` is the number drawn on the PNG
/// (assigned by snapshot-maps in pin order), `label` the place name in the
/// itinerary's own language, `lat`/`lon` the pin coordinates for the keyless
/// Google Maps link (same pattern as `Stop::maps_link`).
#[derive(Clone, Debug)]
pub struct LegendStop {
    pub seq: i64,
    pub label: String,
    pub lat: f64,
    pub lon: f64,
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
fn map_legend_html(legend: &[LegendStop]) -> String {
    if legend.is_empty() {
        return String::new();
    }
    let mut h = String::from("<ol class=\"map-legend\">");
    for s in legend {
        h.push_str(&format!(
            "<li><span class=\"leg-num\">{}</span><a href=\"https://www.google.com/maps?q={},{}\" \
             target=\"_blank\" rel=\"noopener\">{}</a></li>",
            s.seq,
            s.lat,
            s.lon,
            esc(&s.label),
        ));
    }
    h.push_str("</ol>");
    h
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
            map_legend_html(legend),
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
            map_legend_html(legend),
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
        map_legend_html(legend),
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
            map_legend_html(legend),
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
    if let Some(v) = version {
        format!(
            "<figure class=\"map-frame map-frame--day-logistics\"><img class=\"planmap planmap--inset\" alt=\"{}\" src=\"/map/{}/day-{}-logistics.png{}\"><figcaption>{}</figcaption>{}</figure>",
            esc(&caption),
            esc_url_attr(plan_id),
            day_number,
            cache_bust(v),
            map_caption(&caption),
            map_legend_html(legend),
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
        }
    }

    #[test]
    fn map_legend_lists_numbered_clickable_stops() {
        let legend = vec![
            leg(1, "金閣寺", 35.0394, 135.7292),
            leg(2, "北野天滿宮", 35.0117, 135.7409),
        ];
        let h = map_legend_html(&legend);
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
    fn empty_map_legend_renders_nothing() {
        assert_eq!(map_legend_html(&[]), "");
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
