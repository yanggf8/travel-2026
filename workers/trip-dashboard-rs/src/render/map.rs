use super::{esc, esc_url_attr, render_activity_text};
use crate::i18n;
use crate::model::Stop;
use std::collections::HashMap;

/// PNG magic bytes — first four bytes of every valid PNG file.
const PNG_MAGIC: [u8; 4] = [0x89, 0x50, 0x4E, 0x47];

/// Minimum byte length for a real map PNG. Objects at or below 64 bytes are
/// treated as garbage/placeholder (the worker's own 1×1 placeholder is 66 bytes).
pub const MIN_MAP_PNG_BYTES: usize = 64;

/// Server-side map availability for a plan page render. Built in the async router
/// (R2 HEAD/get per key) and threaded into the sync render layer.
#[derive(Debug, Default)]
pub struct MapStatus {
    pub plan: bool,
    pub days: HashMap<i64, bool>,
}

/// True when the body is a real PNG map (not a 1-byte garbage capture or tiny stub).
pub fn is_valid_map_png(bytes: &[u8]) -> bool {
    bytes.len() > MIN_MAP_PNG_BYTES
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

/// Framed plan-overview map slot. Emits a real `<img>` only when `has_map` is true;
/// otherwise a styled missing placeholder (never a blind broken-image `<img>`).
pub fn plan_map_slot(plan_id: &str, has_map: bool, lang: &str) -> String {
    let caption = i18n::t("tripOverview", lang);
    if has_map {
        format!(
            "<figure class=\"map-frame\"><img class=\"planmap\" alt=\"{}\" \
             src=\"/map/{}/plan.png\"><figcaption>{}</figcaption></figure>",
            esc(caption),
            esc_url_attr(plan_id),
            esc(caption),
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

/// Framed per-day route map slot. Same contract as `plan_map_slot`.
pub fn day_map_slot(plan_id: &str, day_number: i64, has_map: bool, lang: &str) -> String {
    let caption = day_route_caption(day_number, lang);
    if has_map {
        format!(
            "<figure class=\"map-frame\"><img class=\"daymap\" alt=\"{}\" \
             src=\"/map/{}/day-{}.png\"><figcaption>{}</figcaption></figure>",
            esc(&caption),
            esc_url_attr(plan_id),
            day_number,
            esc(&caption),
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
    fn plan_map_slot_with_map_emits_img() {
        let h = plan_map_slot("okinawa-2026", true, "en");
        assert!(h.contains("map-frame"));
        assert!(h.contains("class=\"planmap\""));
        assert!(h.contains("/map/okinawa-2026/plan.png"));
        assert!(h.contains("<figcaption>Trip overview</figcaption>"));
        assert!(!h.contains("map-missing"));
    }

    #[test]
    fn plan_map_slot_without_map_emits_placeholder() {
        let h = plan_map_slot("okinawa-2026", false, "en");
        assert!(h.contains("map-frame map-missing"));
        assert!(h.contains("map-missing-box"));
        assert!(h.contains("Map not available yet"));
        assert!(h.contains("<figcaption>Trip overview</figcaption>"));
        assert!(!h.contains("src=\"/map"));
        assert!(!h.contains("<img"));
    }

    #[test]
    fn day_map_slot_with_map_emits_img() {
        let h = day_map_slot("okinawa-2026", 2, true, "zh");
        assert!(h.contains("map-frame"));
        assert!(h.contains("class=\"daymap\""));
        assert!(h.contains("/map/okinawa-2026/day-2.png"));
        assert!(h.contains("第 2 天路線"));
        assert!(!h.contains("map-missing"));
    }

    #[test]
    fn day_map_slot_without_map_emits_placeholder_zh() {
        let h = day_map_slot("okinawa-2026", 3, false, "zh");
        assert!(h.contains("map-missing"));
        assert!(h.contains("地圖尚未產生"));
        assert!(h.contains("第 3 天路線"));
        assert!(!h.contains("src=\"/map"));
    }

    #[test]
    fn is_valid_map_png_rejects_tiny_and_garbage() {
        assert!(!is_valid_map_png(&[0x89]));
        assert!(!is_valid_map_png(&[0u8; 64]));
        assert!(!is_valid_map_png(b"not-a-png"));
        let mut ok = vec![0x89, 0x50, 0x4E, 0x47];
        ok.resize(100, 0);
        assert!(is_valid_map_png(&ok));
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
