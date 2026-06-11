use crate::model::Stop;
use super::{esc, esc_url_attr};

/// <img> pointing at the R2-served map for this day. Image key convention:
/// `<plan_id>/day-<n>.png` (plan-level = `<plan_id>/plan.png`), served by the
/// worker's `/map/*` route (Task 9) from the MAPS bucket.
/// NOTE: the src is built ONLY from trusted, controlled components (plan_id +
/// day number) — never from free user text — so it is a safe URL by construction.
pub fn day_map_img(plan_id: &str, day_number: i64) -> String {
    format!("<img class=\"daymap\" loading=\"lazy\" alt=\"Day {day_number} map\" src=\"/map/{}/day-{}.png\">", esc_url_attr(plan_id), day_number)
}
pub fn plan_map_img(plan_id: &str) -> String {
    format!("<img class=\"planmap\" loading=\"lazy\" alt=\"Trip map\" src=\"/map/{}/plan.png\">", esc_url_attr(plan_id))
}

/// A list of stops with their Google Maps links (keyless q=lat,lon). The
/// `maps_link` field was built by the model from trusted lat/lon (or a
/// /maps/search/<title> fallback) — render it as the href; esc() guards the
/// attribute and text.
pub fn stop_list(stops: &[Stop]) -> String {
    if stops.is_empty() { return String::new(); }
    let mut h = String::from("<ul class=\"stoplist\">");
    for s in stops {
        h.push_str(&format!(
            "<li><a href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a>{}</li>",
            esc_url_attr(&s.maps_link), esc(&s.title),
            if s.address.is_empty() { String::new() } else { format!("<span class=\"addr\">{}</span>", esc(&s.address)) }
        ));
    }
    h.push_str("</ul>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Stop;
    #[test]
    fn stop_list_links_to_maps() {
        let stops = vec![Stop{ title:"Naminoue".into(), maps_link:"https://www.google.com/maps?q=26.2,127.6".into(), ..Default::default()}];
        let h = stop_list(&stops);
        assert!(h.contains("q=26.2,127.6"));
        assert!(h.contains("Naminoue"));
    }
    #[test]
    fn day_map_points_at_r2_route() {
        assert!(day_map_img("okinawa-2026", 2).contains("/map/okinawa-2026/day-2.png"));
    }
    #[test]
    fn plan_map_points_at_r2_route() {
        assert!(plan_map_img("okinawa-2026").contains("/map/okinawa-2026/plan.png"));
    }
    #[test]
    fn empty_stops_render_nothing() {
        assert_eq!(stop_list(&[]), "");
    }
}
