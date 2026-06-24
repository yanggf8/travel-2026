use crate::model::{Day, RouteSegment};
use super::{esc, session};

/// "What to wear" tips, banded by temperature so it works for both cold-season
/// (Tokyo/Kyoto Feb) and hot-season (Okinawa Jun) trips. Faithful port of the
/// old TS `clothingTip()` (workers/trip-dashboard/src/render.ts) — same thresholds,
/// same strings. Returns the tip lines; the caller joins + wraps them.
/// `temp_low`/`temp_high` should be FEELS-LIKE when available (see weather_strip).
pub fn clothing_tip(temp_low: f64, temp_high: f64, rain_pct: f64, lang: &str) -> Vec<String> {
    let zh = lang != "en";
    let mut tips: Vec<String> = Vec::new();
    let lo = temp_low.round() as i64;
    let hi = temp_high.round() as i64;

    // Primary band.
    if temp_low <= 0.0 {
        tips.push(if zh { "🧣 早晚接近0°C——厚外套、圍巾、手套".to_string() }
                  else { "🧣 Near 0°C morning/evening—heavy coat, scarf, gloves".to_string() });
    } else if temp_low <= 5.0 {
        tips.push(if zh { format!("🧥 早晚{lo}°C——冬季外套+毛衣，下午可脫") }
                  else { format!("🧥 {lo}°C morning/evening—winter coat+sweater, lighter afternoon") });
    } else if temp_low <= 10.0 {
        tips.push(if zh { format!("🧥 早晚{lo}°C——外套+薄毛衣") }
                  else { format!("🧥 {lo}°C morning/evening—jacket+light sweater") });
    } else if temp_high <= 18.0 {
        tips.push(if zh { format!("🧥 涼爽{hi}°C——長袖+薄外套") }
                  else { format!("🧥 Cool {hi}°C—long sleeves + light jacket") });
    } else if temp_high <= 24.0 {
        tips.push(if zh { format!("👕 舒適{hi}°C——長短袖皆可，早晚帶薄外套") }
                  else { format!("👕 Mild {hi}°C—short or long sleeves, light layer for evening") });
    } else if temp_high <= 29.0 {
        tips.push(if zh { format!("👕 溫暖{hi}°C——透氣短袖、好走的鞋") }
                  else { format!("👕 Warm {hi}°C—breathable short sleeves, comfy shoes") });
    } else {
        tips.push(if zh { format!("☀️ 炎熱{hi}°C——排汗短袖、防曬、帽子、多補水") }
                  else { format!("☀️ Hot {hi}°C—moisture-wicking tee, sun protection, hat, hydrate") });
    }

    // Afternoon warmth note if a big temp swing.
    if temp_high - temp_low >= 10.0 {
        tips.push(if zh { format!("☀️ 午後回暖到{hi}°C——穿脫方便的洋蔥式穿搭") }
                  else { format!("☀️ Warms to {hi}°C afternoon—layer for easy removal") });
    }

    // Rain gear.
    if rain_pct >= 30.0 {
        tips.push(if zh { "☔ 帶傘（降雨機率高）".to_string() }
                  else { "☔ Bring umbrella (likely rain)".to_string() });
    } else if rain_pct >= 15.0 {
        tips.push(if zh { "🌂 帶折疊傘備用".to_string() }
                  else { "🌂 Compact umbrella just in case".to_string() });
    }

    tips
}

/// Render the per-day weather strip: label + temp range + feels-like + rain%,
/// followed by the clothing tips. Each piece is shown only when present.
fn weather_strip(day: &Day, lang: &str) -> String {
    // No weather data at all → keep the old plain label line (or nothing).
    if day.weather_label.is_empty()
        && day.temp_low_c.is_none() && day.temp_high_c.is_none()
        && day.precipitation_pct.is_none()
    {
        return String::new();
    }

    let zh = lang != "en";
    let mut strip = String::from("<div class=\"weather\">🌧️ ");
    if !day.weather_label.is_empty() {
        strip.push_str(&esc(&day.weather_label));
    }
    // temp range
    if let (Some(lo), Some(hi)) = (day.temp_low_c, day.temp_high_c) {
        strip.push_str(&format!(" · {}\u{2013}{}°C", lo.round() as i64, hi.round() as i64));
    }
    // feels-like range
    if let (Some(flo), Some(fhi)) = (day.feels_like_low_c, day.feels_like_high_c) {
        let label = if zh { "體感" } else { "Feels" };
        strip.push_str(&format!(" · {label} {}\u{2013}{}°C", flo.round() as i64, fhi.round() as i64));
    }
    // rain
    if let Some(rain) = day.precipitation_pct {
        strip.push_str(&format!(" · 💧{}%", rain.round() as i64));
    }
    strip.push_str("</div>");

    // Clothing tips: feels-like temps when available, else actual; rain default 0.
    let tip_low = day.feels_like_low_c.or(day.temp_low_c);
    let tip_high = day.feels_like_high_c.or(day.temp_high_c);
    if let (Some(tl), Some(th)) = (tip_low, tip_high) {
        let rain = day.precipitation_pct.unwrap_or(0.0);
        let tips = clothing_tip(tl, th, rain, lang);
        if !tips.is_empty() {
            let joined = tips.iter().map(|t| esc(t)).collect::<Vec<_>>().join(" · ");
            strip.push_str(&format!("<div class=\"weather-clothing\">{joined}</div>"));
        }
    }
    strip
}

/// Icon for a route-segment travel mode.
fn mode_icon(mode: &str) -> &'static str {
    match mode {
        "driving" => "🚗",
        "walking" => "🚶",
        _ => "🚌", // transit + any unknown/default
    }
}

/// Render the per-day "今日路線" (today's route) block from door-to-door segments.
fn render_route_block(segments: &[RouteSegment], lang: &str) -> String {
    let heading = if lang == "en" { "Today's route" } else { "今日路線" };
    let mut h = String::new();
    h.push_str(&format!("<div class=\"route-block\"><div class=\"route-heading\">{heading}</div>"));
    for seg in segments {
        h.push_str("<div class=\"route-seg\">");
        h.push_str(&format!("<span class=\"route-mode\">{}</span> ", mode_icon(&seg.mode)));
        h.push_str(&format!("{} → {}", esc(&seg.from_place), esc(&seg.to_place)));
        if !seg.start_time.is_empty() {
            h.push_str(&format!(" · {}", esc(&seg.start_time)));
        }
        h.push_str(&format!(" · ~{} min", seg.duration_min));
        if !seg.notes.is_empty() {
            h.push_str(&format!(" · {}", esc(&seg.notes)));
        }
        h.push_str("</div>");
    }
    h.push_str("</div>");
    h
}

pub fn render(day: &Day, plan_id: &str, lang: &str, has_map: bool) -> String {
    let theme = if lang == "zh" && !day.theme_zh.is_empty() { &day.theme_zh } else { &day.theme };
    let mut h = String::new();
    h.push_str(&format!("<section class=\"day day-{}\">", esc(&day.day_type)));
    h.push_str(&format!("<h2>Day {} · {}</h2>", day.day_number, esc(&day.date)));
    h.push_str(&format!("<div class=\"theme\">{}</div>", esc(theme)));
    h.push_str(&weather_strip(day, lang));
    h.push_str(&super::map::day_map_slot(plan_id, day.day_number, has_map, lang));
    if !day.route_segments.is_empty() {
        h.push_str(&render_route_block(&day.route_segments, lang));
    }
    for sess in &day.sessions {
        // skip wholly-empty sessions to avoid 4 empty boxes on a light day
        if sess.activities.is_empty() && sess.meals.is_empty() && sess.focus_zh.is_empty() { continue; }
        h.push_str(&session::render(sess, lang));
        h.push_str(&super::map::stop_list(&sess.stops));
    }
    h.push_str("</section>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Day, Session, RouteSegment, Activity};
    /// Build an Activity carrying only a display title (the common test case).
    fn act(title: &str) -> Activity { Activity { title: title.into(), ..Default::default() } }
    #[test]
    fn renders_route_block() {
        let day = Day {
            day_number: 2, date: "2026-06-13".into(), day_type: "full".into(),
            route_segments: vec![RouteSegment {
                from_place: "HOTEL AZAT NAHA".into(),
                to_place: "波上宮".into(),
                mode: "driving".into(),
                duration_min: 12,
                notes: "".into(),
                start_time: "09:00".into(),
            }],
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh", false);
        assert!(html.contains("今日路線"));
        assert!(html.contains("HOTEL AZAT NAHA"));
        assert!(html.contains("波上宮"));
        assert!(html.contains("🚗"));
        assert!(html.contains("~12 min"));
    }
    #[test]
    fn renders_zh_theme_and_sessions() {
        let day = Day {
            day_number: 3, date: "2026-06-14".into(), day_type: "full".into(),
            theme_zh: "壺屋陶器街".into(),
            sessions: vec![Session{ session_type:"noon".into(), meals:vec!["Lunch".into()], ..Default::default()}],
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh", false);
        assert!(html.contains("壺屋陶器街"));
        assert!(html.contains("中午"));
        assert!(html.contains("Lunch"));
    }
    #[test]
    fn empty_session_is_skipped() {
        let day = Day {
            day_number: 1, date: "2026-06-12".into(), day_type: "arrival".into(),
            sessions: vec![
                Session{ session_type:"morning".into(), activities: vec![act("Arrive")], ..Default::default()},
                Session{ session_type:"noon".into(), ..Default::default()}, // empty → skipped
            ],
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "en", false);
        assert!(html.contains("Arrive"));
        // the empty noon session should NOT emit a session block
        assert!(!html.contains("session-noon"));
    }
    #[test]
    fn day_includes_day_map_image_when_available() {
        let day = Day { day_number: 2, date: "2026-06-13".into(), day_type: "full".into(), ..Default::default() };
        let html = render(&day, "okinawa-2026", "en", true);
        assert!(html.contains("/map/okinawa-2026/day-2.png"));
        assert!(html.contains("map-frame"));
    }

    #[test]
    fn day_shows_missing_placeholder_when_map_unavailable() {
        let day = Day { day_number: 2, date: "2026-06-13".into(), day_type: "full".into(), ..Default::default() };
        let html = render(&day, "okinawa-2026", "en", false);
        assert!(html.contains("map-missing"));
        assert!(!html.contains("src=\"/map"));
    }

    // ---- clothing_tip thresholds (faithful port of the TS clothingTip) ----
    #[test]
    fn clothing_hot_rainy_day() {
        // high 30, low 26, rain 73 → 炎熱/防曬 AND 帶傘
        let tips = clothing_tip(26.0, 30.0, 73.0, "zh");
        let joined = tips.join(" · ");
        assert!(joined.contains("炎熱"), "got: {joined}");
        assert!(joined.contains("防曬"), "got: {joined}");
        assert!(joined.contains("帶傘"), "got: {joined}");
    }

    #[test]
    fn clothing_mild_day_no_umbrella() {
        // high 22 → 舒適, rain 5 → no umbrella
        let tips = clothing_tip(16.0, 22.0, 5.0, "zh");
        let joined = tips.join(" · ");
        assert!(joined.contains("舒適"), "got: {joined}");
        assert!(!joined.contains("帶傘"), "got: {joined}");
        assert!(!joined.contains("折疊傘"), "got: {joined}");
    }

    #[test]
    fn clothing_freezing_day() {
        let tips = clothing_tip(-1.0, 4.0, 0.0, "zh");
        assert!(tips[0].contains("接近0"), "got: {:?}", tips);
    }

    #[test]
    fn clothing_swing_adds_onion_layer() {
        // high 25, low 12 → swing >= 10 → 洋蔥式
        let tips = clothing_tip(12.0, 25.0, 0.0, "zh");
        let joined = tips.join(" · ");
        assert!(joined.contains("洋蔥式"), "got: {joined}");
    }

    #[test]
    fn clothing_moderate_rain_compact_umbrella() {
        let tips = clothing_tip(16.0, 22.0, 20.0, "zh");
        let joined = tips.join(" · ");
        assert!(joined.contains("折疊傘"), "got: {joined}");
        assert!(!joined.contains("帶傘"), "got: {joined}");
    }

    #[test]
    fn clothing_en_hot() {
        let tips = clothing_tip(26.0, 30.0, 73.0, "en");
        let joined = tips.join(" · ");
        assert!(joined.contains("Hot"), "got: {joined}");
        assert!(joined.contains("Bring umbrella"), "got: {joined}");
    }

    // ---- weather strip ----
    #[test]
    fn weather_strip_shows_feels_like_and_rain() {
        let day = Day {
            day_number: 3, date: "2026-06-14".into(), day_type: "full".into(),
            weather_label: "Cloudy".into(),
            temp_low_c: Some(26.0), temp_high_c: Some(30.0),
            precipitation_pct: Some(73.0),
            feels_like_low_c: Some(28.0), feels_like_high_c: Some(34.0),
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh", false);
        assert!(html.contains("體感"), "got: {html}");
        assert!(html.contains("💧73%"), "got: {html}");
        assert!(html.contains("26\u{2013}30°C"), "got: {html}"); // en-dash range
    }

    #[test]
    fn weather_strip_hot_rainy_renders_clothing() {
        let day = Day {
            day_number: 3, date: "2026-06-14".into(), day_type: "full".into(),
            weather_label: "Rain".into(),
            temp_low_c: Some(26.0), temp_high_c: Some(30.0),
            precipitation_pct: Some(73.0),
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh", false);
        assert!(html.contains("weather-clothing"), "got: {html}");
        assert!(html.contains("炎熱"), "got: {html}");
        assert!(html.contains("帶傘"), "got: {html}");
    }

    #[test]
    fn weather_strip_mild_no_umbrella() {
        let day = Day {
            day_number: 3, date: "2026-06-14".into(), day_type: "full".into(),
            weather_label: "Clear".into(),
            temp_low_c: Some(16.0), temp_high_c: Some(22.0),
            precipitation_pct: Some(5.0),
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh", false);
        assert!(html.contains("舒適"), "got: {html}");
        assert!(!html.contains("帶傘"), "got: {html}");
    }

    #[test]
    fn weather_strip_uses_feels_like_for_clothing_when_present() {
        // Actual high 28 (would be 溫暖), but feels-like 34 → should pick 炎熱.
        let day = Day {
            day_number: 3, date: "2026-06-14".into(), day_type: "full".into(),
            weather_label: "Humid".into(),
            temp_low_c: Some(25.0), temp_high_c: Some(28.0),
            precipitation_pct: Some(0.0),
            feels_like_low_c: Some(27.0), feels_like_high_c: Some(34.0),
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh", false);
        assert!(html.contains("炎熱"), "expected feels-like-driven 炎熱, got: {html}");
    }

    #[test]
    fn weather_strip_only_label_when_no_temps() {
        let day = Day {
            day_number: 1, date: "2026-06-12".into(), day_type: "arrival".into(),
            weather_label: "Sunny".into(),
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh", false);
        assert!(html.contains("Sunny"), "got: {html}");
        assert!(!html.contains("體感"), "got: {html}");
        assert!(!html.contains("weather-clothing"), "got: {html}");
    }
}
