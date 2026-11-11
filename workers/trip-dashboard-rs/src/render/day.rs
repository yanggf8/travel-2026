use super::{esc, render_activity_text, session};
use crate::i18n::{fill, t, tf, wmo_weather};
use crate::model::{ClothingTip, Day, RouteSegment};

/// Render the day's map landmarks (waypoints listed under the day map). Ported from
/// the TS worker, which listed the day's landmarks beneath the route map.
fn render_landmarks(landmarks: &[String], lang: &str) -> String {
    let mut h = String::new();
    h.push_str(&format!(
        "<div class=\"day-landmarks\"><div class=\"landmarks-heading\">{}</div><ul>",
        esc(t("landmarks", lang))
    ));
    for lm in landmarks {
        h.push_str(&format!("<li>{}</li>", esc(lm)));
    }
    h.push_str("</ul></div>");
    h
}

/// "What to wear" tips from the GLOBAL `clothing_tips` reference table (written
/// by `travel set-clothing-tip`; the band texts are content, not chrome). Three
/// groups, evaluated in this fixed order; within a group the FIRST matching row
/// (rows arrive in `sort_order`) wins and contributes one line:
///   1. primary temperature band — `temp_low_max` (temp_low <= threshold),
///      `temp_high_max` (temp_high <= threshold), `temp_else` (always matches;
///      the catch-all after the thresholds),
///   2. big-swing note — `swing_min` (temp_high - temp_low >= threshold),
///   3. rain gear — `rain_min` (rain_pct >= threshold).
/// `{lo}` / `{hi}` in the text are the rounded temps. An empty table renders no
/// tips — there is deliberately no hardcoded fallback.
/// `temp_low`/`temp_high` should be FEELS-LIKE when available (see weather_strip).
pub fn clothing_tip(
    tips: &[ClothingTip],
    temp_low: f64,
    temp_high: f64,
    rain_pct: f64,
    lang: &str,
) -> Vec<String> {
    const GROUPS: [&[&str]; 3] = [
        &["temp_low_max", "temp_high_max", "temp_else"],
        &["swing_min"],
        &["rain_min"],
    ];
    let lo = (temp_low.round() as i64).to_string();
    let hi = (temp_high.round() as i64).to_string();
    let matches = |tip: &ClothingTip| match (tip.kind.as_str(), tip.threshold) {
        ("temp_low_max", Some(th)) => temp_low <= th,
        ("temp_high_max", Some(th)) => temp_high <= th,
        ("temp_else", _) => true,
        ("swing_min", Some(th)) => temp_high - temp_low >= th,
        ("rain_min", Some(th)) => rain_pct >= th,
        _ => false,
    };
    GROUPS
        .iter()
        .filter_map(|group| {
            tips.iter()
                .filter(|tip| group.contains(&tip.kind.as_str()))
                .find(|tip| matches(tip))
        })
        .map(|tip| {
            let text = if lang != "en" { &tip.text_zh } else { &tip.text_en };
            fill(text, &[("lo", &lo), ("hi", &hi)])
        })
        .collect()
}

/// Render the per-day weather strip: description + temp range + feels-like + rain%,
/// followed by the clothing tips. Each piece is shown only when present.
/// The description comes from the WMO `weather_code` in the page language
/// (`i18n::wmo_weather`); a NULL or unmapped code shows the stored `weather_label`.
fn weather_strip(day: &Day, clothing_tips: &[ClothingTip], lang: &str) -> String {
    let description: &str = day
        .weather_code
        .and_then(|code| wmo_weather(code, lang))
        .unwrap_or(&day.weather_label);
    // No weather data at all → keep the old plain label line (or nothing).
    if description.is_empty()
        && day.temp_low_c.is_none()
        && day.temp_high_c.is_none()
        && day.precipitation_pct.is_none()
    {
        return String::new();
    }

    let mut strip = String::from("<div class=\"weather\">🌧️ ");
    if !description.is_empty() {
        strip.push_str(&esc(description));
    }
    // temp range
    if let (Some(lo), Some(hi)) = (day.temp_low_c, day.temp_high_c) {
        strip.push_str(&format!(
            " · {}\u{2013}{}°C",
            lo.round() as i64,
            hi.round() as i64
        ));
    }
    // feels-like range
    if let (Some(flo), Some(fhi)) = (day.feels_like_low_c, day.feels_like_high_c) {
        let label = t("feelsLike", lang);
        strip.push_str(&format!(
            " · {label} {}\u{2013}{}°C",
            flo.round() as i64,
            fhi.round() as i64
        ));
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
        let tips = clothing_tip(clothing_tips, tl, th, rain, lang);
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
    let heading = t("todaysRoute", lang);
    let mut h = String::new();
    h.push_str(&format!(
        "<div class=\"route-block\"><div class=\"route-heading\">{heading}</div>"
    ));
    for seg in segments {
        h.push_str("<div class=\"route-seg\">");
        h.push_str(&format!(
            "<span class=\"route-mode\">{}</span> ",
            mode_icon(&seg.mode)
        ));
        h.push_str(&format!(
            "{} → {}",
            esc(&seg.from_place),
            esc(&seg.to_place)
        ));
        if !seg.start_time.is_empty() {
            h.push_str(&format!(" · {}", esc(&seg.start_time)));
        }
        // NULL duration arrives as 0 — omit it rather than publish "~0 min".
        if seg.duration_min > 0 {
            h.push_str(&format!(
                " · {}",
                tf("aboutMinutes", lang, &[("n", &seg.duration_min.to_string())])
            ));
        }
        if !seg.notes.is_empty() {
            h.push_str(&format!(" · {}", render_activity_text(&seg.notes)));
        }
        h.push_str(&super::ai_rec_badge(&seg.source, lang));
        h.push_str("</div>");
    }
    h.push_str("</div>");
    h
}

#[cfg(test)]
pub fn render(
    day: &Day,
    plan_id: &str,
    lang: &str,
    map_version: Option<&str>,
    legend: &[super::map::LegendStop],
) -> String {
    let map_status = super::map::MapStatus {
        days: [(day.day_number, map_version.map(str::to_owned))]
            .into_iter()
            .collect(),
        legends: [(format!("day-{}.png", day.day_number), legend.to_vec())]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    render_with_logistics(day, plan_id, lang, &map_status, &[])
}

fn render_content(
    day: &Day,
    plan_id: &str,
    lang: &str,
    map_status: &super::map::MapStatus,
    clothing_tips: &[ClothingTip],
    option_label: Option<&str>,
) -> String {
    let theme = if lang == "zh" && !day.theme_zh.is_empty() {
        &day.theme_zh
    } else {
        &day.theme
    };
    let mut h = format!("<div class=\"theme\">{}</div>", esc(theme));
    if option_label.is_none() {
        h.push_str(&weather_strip(day, clothing_tips, lang));
    }
    h.push_str("<div class=\"day-map-stack\">");
    let map_version = map_status
        .days
        .get(&day.day_number)
        .and_then(|v| v.as_deref());
    let logistics_version = map_status
        .day_logistics
        .get(&day.day_number)
        .and_then(|v| v.as_deref());
    let away_version = map_status
        .day_away
        .get(&day.day_number)
        .and_then(|v| v.as_deref());
    let legend = map_status.legend_for(&format!("day-{}.png", day.day_number));
    let logistics_legend =
        map_status.legend_for(&format!("day-{}-logistics.png", day.day_number));
    let away_legend = map_status.legend_for(&format!("day-{}-away.png", day.day_number));
    if let Some(label) = option_label {
        h.push_str(&super::map::variant_day_map_slot(
            plan_id,
            day.day_number,
            map_version,
            lang,
            legend,
            label,
        ));
        h.push_str(&super::map::variant_day_logistics_map_slot(
            plan_id,
            day.day_number,
            day.variant_of.unwrap_or(day.day_number),
            logistics_version,
            lang,
            logistics_legend,
            super::day_option_name(day, lang),
        ));
        h.push_str(&super::map::variant_day_away_map_slot(
            plan_id,
            day.day_number,
            day.variant_of.unwrap_or(day.day_number),
            away_version,
            lang,
            away_legend,
            super::day_option_name(day, lang),
        ));
    } else {
        h.push_str(&super::map::day_map_slot(
            plan_id,
            day.day_number,
            map_version,
            lang,
            legend,
        ));
        h.push_str(&super::map::day_logistics_map_slot(
            plan_id,
            day.day_number,
            logistics_version,
            lang,
            logistics_legend,
        ));
        h.push_str(&super::map::day_away_map_slot(
            plan_id,
            day.day_number,
            away_version,
            lang,
            away_legend,
        ));
    }
    h.push_str("</div>");
    if !day.landmarks.is_empty() {
        h.push_str(&render_landmarks(&day.landmarks, lang));
    }
    if !day.route_segments.is_empty() {
        h.push_str(&render_route_block(&day.route_segments, lang));
    }
    for sess in &day.sessions {
        // skip wholly-empty sessions to avoid 4 empty boxes on a light day
        if sess.activities.is_empty() && sess.meals.is_empty() && sess.focus_zh.is_empty() {
            continue;
        }
        h.push_str(&session::render(sess, lang));
        h.push_str(&super::map::stop_list(&sess.stops));
    }
    h
}

/// One day card with its map stack and any alternative plans for the same date.
pub fn render_with_logistics(
    day: &Day,
    plan_id: &str,
    lang: &str,
    map_status: &super::map::MapStatus,
    clothing_tips: &[ClothingTip],
) -> String {
    render_with_documents(day, plan_id, lang, map_status, clothing_tips, &[], None)
}

#[allow(clippy::too_many_arguments)]
pub fn render_with_documents(
    day: &Day,
    plan_id: &str,
    lang: &str,
    map_status: &super::map::MapStatus,
    clothing_tips: &[ClothingTip],
    documents: &[crate::model::PlanDocument],
    token: Option<&str>,
) -> String {
    let mut h = String::new();
    h.push_str(&format!(
        "<section class=\"day day-{}\" data-day=\"{}\">",
        esc(&day.day_type),
        day.day_number
    ));
    h.push_str(&format!(
        "<h2>{}</h2>",
        esc(&tf(
            "dayHeading",
            lang,
            &[("day", &day.day_number.to_string()), ("date", &day.date)],
        ))
    ));
    if !day.variants.is_empty() {
        h.push_str(&format!(
            "<span class=\"day-option-label\">{}</span>",
            esc(super::day_option_name(day, lang))
        ));
    }
    h.push_str(&super::summary::day_documents(
        documents,
        day.day_number,
        lang,
        token,
    ));
    h.push_str(&render_content(
        day,
        plan_id,
        lang,
        map_status,
        clothing_tips,
        None,
    ));
    for variant in &day.variants {
        let option_label = super::day_option_label(variant, day.day_number, lang);
        h.push_str(&format!(
            "<details class=\"day-variant\" data-variant-of=\"{}\" open><summary class=\"day-variant-summary\">{}</summary><div class=\"day-variant-body\">",
            day.day_number,
            esc(&tf(
                "dayVariant",
                lang,
                &[("label", super::day_option_name(variant, lang))],
            ))
        ));
        h.push_str(&render_content(
            variant,
            plan_id,
            lang,
            map_status,
            clothing_tips,
            Some(&option_label),
        ));
        h.push_str("</div></details>");
    }
    h.push_str("</section>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Activity, Day, Meal, RouteSegment, Session};
    /// Build an Activity carrying only a display title (the common test case).
    fn act(title: &str) -> Activity {
        Activity {
            title: title.into(),
            ..Default::default()
        }
    }

    #[test]
    fn no_variant_plan_html() {
        let day = Day {
            day_number: 2,
            date: "2026-11-14".into(),
            day_type: "full".into(),
            theme: "Kyoto highlights".into(),
            theme_zh: "京都巡禮".into(),
            weather_label: "晴".into(),
            temp_low_c: Some(12.0),
            temp_high_c: Some(20.0),
            precipitation_pct: Some(10.0),
            landmarks: vec!["金閣寺".into()],
            route_segments: vec![RouteSegment {
                from_place: "旅社".into(),
                to_place: "金閣寺".into(),
                mode: "transit".into(),
                duration_min: 25,
                ..Default::default()
            }],
            sessions: vec![Session {
                session_type: "morning".into(),
                focus_zh: "寺院散策".into(),
                activities: vec![act("金閣寺參觀")],
                ..Default::default()
            }],
            ..Default::default()
        };
        let legend = [super::super::map::LegendStop {
            seq: 1,
            label: "金閣寺".into(),
            lat: 35.0394,
            lon: 135.7292,
            ..Default::default()
        }];
        let map_status = super::super::map::MapStatus {
            days: [(2, Some("day-v1".into()))].into_iter().collect(),
            day_logistics: [(2, Some("log-v1".into()))].into_iter().collect(),
            day_away: [(2, Some("away-v1".into()))].into_iter().collect(),
            legends: [
                ("day-2.png".into(), legend.to_vec()),
                ("day-2-logistics.png".into(), legend.to_vec()),
                ("day-2-away.png".into(), legend.to_vec()),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let html = render_with_logistics(
            &day,
            "kyoto-nov-2026",
            "zh",
            &map_status,
            &sample_tips(),
        );
        assert_eq!(
            html,
            "<section class=\"day day-full\" data-day=\"2\"><h2>第 2 天 · 2026-11-14</h2><div class=\"theme\">京都巡禮</div><div class=\"weather\">🌧️ 晴 · 12–20°C · 💧10%</div><div class=\"weather-clothing\">👕 舒適20°C——長短袖皆可，早晚帶薄外套</div><div class=\"day-map-stack\"><figure class=\"map-frame\" data-map-key=\"day-2.png\"><img class=\"daymap\" alt=\"第 2 天路線\" src=\"/map/kyoto-nov-2026/day-2.png?v=dayv1\"><figcaption>第 2 天路線 <span class=\"map-attribution\">© <a href=\"https://www.openstreetmap.org/copyright\" target=\"_blank\" rel=\"noopener\">OpenStreetMap contributors</a></span></figcaption><ol class=\"map-legend\"><li><span class=\"leg-num\">1</span><a href=\"https://www.google.com/maps?q=35.0394,135.7292\" target=\"_blank\" rel=\"noopener\">金閣寺</a></li></ol></figure><figure class=\"map-frame map-frame--day-logistics\" data-map-key=\"day-2-logistics.png\"><img class=\"planmap planmap--inset\" alt=\"第 2 天・旅社來回\" src=\"/map/kyoto-nov-2026/day-2-logistics.png?v=logv1\"><figcaption>第 2 天・旅社來回 <span class=\"map-attribution\">© <a href=\"https://www.openstreetmap.org/copyright\" target=\"_blank\" rel=\"noopener\">OpenStreetMap contributors</a></span></figcaption><ol class=\"map-legend\"><li><span class=\"leg-num\">1</span><a href=\"https://www.google.com/maps?q=35.0394,135.7292\" target=\"_blank\" rel=\"noopener\">金閣寺</a></li></ol></figure><figure class=\"map-frame map-frame--day-logistics map-frame--day-away\" data-map-key=\"day-2-away.png\"><img class=\"planmap planmap--inset\" alt=\"第 2 天・本區以外的行程\" src=\"/map/kyoto-nov-2026/day-2-away.png?v=awayv1\"><figcaption>第 2 天・本區以外的行程 <span class=\"map-attribution\">© <a href=\"https://www.openstreetmap.org/copyright\" target=\"_blank\" rel=\"noopener\">OpenStreetMap contributors</a></span></figcaption><ol class=\"map-legend\"><li><span class=\"leg-num\">1</span><a href=\"https://www.google.com/maps?q=35.0394,135.7292\" target=\"_blank\" rel=\"noopener\">金閣寺</a></li></ol></figure></div><div class=\"day-landmarks\"><div class=\"landmarks-heading\">地圖景點</div><ul><li>金閣寺</li></ul></div><div class=\"route-block\"><div class=\"route-heading\">今日路線</div><div class=\"route-seg\"><span class=\"route-mode\">🚌</span> 旅社 → 金閣寺 · 約 25 分鐘</div></div><div class=\"session session-morning\"><div class=\"session-label\">上午</div><div class=\"session-focus\">寺院散策</div><div class=\"activity\">金閣寺參觀</div></div></section>"
        );
    }

    #[test]
    fn documents_render_in_main_day_card_but_never_in_variant_blocks() {
        let day = Day {
            day_number: 1,
            day_type: "arrival".into(),
            theme: "Main".into(),
            variants: vec![Day {
                day_number: 3,
                day_type: "arrival".into(),
                theme: "Variant".into(),
                variant_of: Some(1),
                ..Default::default()
            }],
            ..Default::default()
        };
        let documents = vec![
            crate::model::PlanDocument {
                doc_key: "flight".into(),
                kind: "flight_eticket".into(),
                label: "Flight".into(),
                label_zh: "機票".into(),
                r2_key: "p-1/doc-flight.pdf".into(),
                days: vec![1],
                ..Default::default()
            },
            crate::model::PlanDocument {
                doc_key: "hotel".into(),
                kind: "hotel_voucher".into(),
                label: "Hotel".into(),
                label_zh: "住宿券".into(),
                r2_key: "p-1/doc-hotel.pdf".into(),
                days: vec![2],
                ..Default::default()
            },
        ];
        let html = render_with_documents(
            &day,
            "p-1",
            "en",
            &super::super::map::MapStatus::default(),
            &[],
            &documents,
            Some("t0k"),
        );
        let main = html.split("<details class=\"day-variant\"").next().unwrap();
        let variant = html.split("<details class=\"day-variant\"").nth(1).unwrap();
        assert!(main.contains("data-doc=\"flight\""), "{main}");
        assert!(main.contains("href=\"/voucher/p-1/doc-flight.pdf?token=t0k\""));
        assert!(!main.contains("data-doc=\"hotel\""));
        assert!(!variant.contains("doc-link"), "{variant}");
        assert!(html.find("day-documents").unwrap() < html.find("<div class=\"theme\">Main</div>").unwrap());
    }

    #[test]
    fn no_mapped_documents_renders_no_day_document_block() {
        let day = Day { day_number: 1, ..Default::default() };
        let html = render_with_documents(
            &day,
            "p-1",
            "en",
            &super::super::map::MapStatus::default(),
            &[],
            &[],
            None,
        );
        assert!(!html.contains("day-documents"));
    }

    #[test]
    fn renders_route_block() {
        let day = Day {
            day_number: 2,
            date: "2026-06-13".into(),
            day_type: "full".into(),
            route_segments: vec![RouteSegment {
                from_place: "HOTEL AZAT NAHA".into(),
                to_place: "波上宮".into(),
                mode: "driving".into(),
                duration_min: 12,
                notes: "".into(),
                start_time: "09:00".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh", None, &[]);
        assert!(html.contains("今日路線"));
        assert!(html.contains("HOTEL AZAT NAHA"));
        assert!(html.contains("波上宮"));
        assert!(html.contains("🚗"));
        assert!(html.contains("約 12 分鐘"), "{html}");
        let en = render(&day, "okinawa-2026", "en", None, &[]);
        assert!(en.contains("~12 min"), "{en}");
    }

    #[test]
    fn day_heading_follows_the_page_language() {
        let day = Day {
            day_number: 3,
            date: "2026-11-14".into(),
            day_type: "full".into(),
            ..Default::default()
        };
        let zh = render(&day, "kyoto-nov-2026", "zh", None, &[]);
        assert!(zh.contains("<h2>第 3 天 · 2026-11-14</h2>"), "{zh}");
        let en = render(&day, "kyoto-nov-2026", "en", None, &[]);
        assert!(en.contains("<h2>Day 3 · 2026-11-14</h2>"), "{en}");
    }

    #[test]
    fn route_leg_without_duration_omits_minutes() {
        let day = Day {
            day_number: 2,
            date: "2026-11-13".into(),
            day_type: "full".into(),
            route_segments: vec![RouteSegment {
                from_place: "大原".into(),
                to_place: "京都國際會館".into(),
                mode: "transit".into(),
                duration_min: 0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&day, "osaka-nov-2026", "zh", None, &[]);
        assert!(html.contains("京都國際會館"));
        assert!(!html.contains("0 分鐘"), "{html}");
    }

    #[test]
    fn ai_recommended_route_segment_renders_badge_en() {
        let day = Day {
            day_number: 2,
            date: "2026-06-13".into(),
            day_type: "full".into(),
            route_segments: vec![RouteSegment {
                from_place: "Hotel".into(),
                to_place: "Beach".into(),
                mode: "walking".into(),
                duration_min: 15,
                source: "ai_recommended".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "en", None, &[]);
        assert!(html.contains("ai-rec-badge"), "got: {html}");
        assert!(html.contains("AI-recommended (unconfirmed)"), "got: {html}");
    }

    #[test]
    fn day_landmarks_render_as_list() {
        let day = Day {
            day_number: 2,
            date: "2026-02-25".into(),
            day_type: "full".into(),
            landmarks: vec!["Kinkaku-ji Temple".into(), "Kitano Tenmangu Shrine".into()],
            ..Default::default()
        };
        let html = render(&day, "kyoto-2026", "en", None, &[]);
        assert!(html.contains("day-landmarks"));
        assert!(html.contains("Kinkaku-ji Temple"));
        assert!(html.contains("Kitano Tenmangu Shrine"));
    }

    #[test]
    fn no_landmarks_means_no_landmarks_block() {
        let day = Day {
            day_number: 1,
            date: "2026-02-24".into(),
            day_type: "arrival".into(),
            ..Default::default()
        };
        let html = render(&day, "kyoto-2026", "en", None, &[]);
        assert!(!html.contains("day-landmarks"));
    }

    #[test]
    fn route_note_embedded_map_url_becomes_labeled_link() {
        let day = Day {
            day_number: 1,
            date: "2026-06-12".into(),
            day_type: "arrival".into(),
            route_segments: vec![RouteSegment {
                from_place: "紅樹林".into(),
                to_place: "大園出國停車場第三停車場".into(),
                mode: "driving".into(),
                duration_min: 60,
                start_time: "04:00".into(),
                notes: "地址：桃園市大園區中山南路544號旁 Google Maps 導航：https://www.google.com/maps/dir/%E6%96%B0%E5%8C%97/%E6%A1%83%E5%9C%92".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh", None, &[]);
        assert!(html.contains("🗺️ Google Maps 導航"), "got: {html}");
        assert!(
            html.contains(
                "<a href=\"https://www.google.com/maps/dir/%E6%96%B0%E5%8C%97/%E6%A1%83%E5%9C%92\""
            ),
            "got: {html}"
        );
        assert!(
            !html.contains(">https://www.google.com/maps/dir/"),
            "got: {html}"
        );
    }

    #[test]
    fn renders_zh_theme_and_sessions() {
        let day = Day {
            day_number: 3,
            date: "2026-06-14".into(),
            day_type: "full".into(),
            theme_zh: "壺屋陶器街".into(),
            sessions: vec![Session {
                session_type: "noon".into(),
                meals: vec![Meal {
                    text: "Lunch".into(),
                    source: "confirmed".into(),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh", None, &[]);
        assert!(html.contains("壺屋陶器街"));
        assert!(html.contains("中午"));
        assert!(html.contains("Lunch"));
    }
    #[test]
    fn empty_session_is_skipped() {
        let day = Day {
            day_number: 1,
            date: "2026-06-12".into(),
            day_type: "arrival".into(),
            sessions: vec![
                Session {
                    session_type: "morning".into(),
                    activities: vec![act("Arrive")],
                    ..Default::default()
                },
                Session {
                    session_type: "noon".into(),
                    ..Default::default()
                }, // empty → skipped
            ],
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "en", None, &[]);
        assert!(html.contains("Arrive"));
        // the empty noon session should NOT emit a session block
        assert!(!html.contains("session-noon"));
    }
    #[test]
    fn day_includes_day_map_image_when_available() {
        let day = Day {
            day_number: 2,
            date: "2026-06-13".into(),
            day_type: "full".into(),
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "en", Some("v1"), &[]);
        assert!(html.contains("/map/okinawa-2026/day-2.png"));
        assert!(html.contains("map-frame"));
    }

    #[test]
    fn day_map_renders_legend_under_map_when_available() {
        let day = Day {
            day_number: 2,
            date: "2026-06-13".into(),
            day_type: "full".into(),
            ..Default::default()
        };
        let legend = vec![super::super::map::LegendStop {
            seq: 1,
            label: "残波岬".into(),
            lat: 26.1286,
            lon: 127.7303,
            ..Default::default()
        }];
        let html = render(&day, "okinawa-2026", "zh", Some("v1"), &legend);
        assert!(html.contains("<ol class=\"map-legend\">"), "got: {html}");
        assert!(html.contains(">残波岬</a>"), "got: {html}");
        let map_pos = html.find("day-2.png").expect("day map");
        let legend_pos = html.find("map-legend").expect("legend");
        assert!(legend_pos > map_pos, "legend sits under the day map");
    }

    #[test]
    fn day_shows_missing_placeholder_when_map_unavailable() {
        let day = Day {
            day_number: 2,
            date: "2026-06-13".into(),
            day_type: "full".into(),
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "en", None, &[]);
        assert!(html.contains("map-missing"));
        assert!(!html.contains("src=\"/map"));
    }

    /// The clothing_tips rows as loaded into Turso (set-clothing-tip), in
    /// sort_order — the fixture the band-evaluation tests run against.
    fn sample_tips() -> Vec<ClothingTip> {
        let row = |kind: &str, threshold: Option<f64>, zh: &str, en: &str| ClothingTip {
            kind: kind.into(),
            threshold,
            text_zh: zh.into(),
            text_en: en.into(),
        };
        vec![
            row("temp_low_max", Some(0.0), "🧣 早晚接近0°C——厚外套、圍巾、手套", "🧣 Near 0°C morning/evening—heavy coat, scarf, gloves"),
            row("temp_low_max", Some(5.0), "🧥 早晚{lo}°C——冬季外套+毛衣，下午可脫", "🧥 {lo}°C morning/evening—winter coat+sweater, lighter afternoon"),
            row("temp_low_max", Some(10.0), "🧥 早晚{lo}°C——外套+薄毛衣", "🧥 {lo}°C morning/evening—jacket+light sweater"),
            row("temp_high_max", Some(18.0), "🧥 涼爽{hi}°C——長袖+薄外套", "🧥 Cool {hi}°C—long sleeves + light jacket"),
            row("temp_high_max", Some(24.0), "👕 舒適{hi}°C——長短袖皆可，早晚帶薄外套", "👕 Mild {hi}°C—short or long sleeves, light layer for evening"),
            row("temp_high_max", Some(29.0), "👕 溫暖{hi}°C——透氣短袖、好走的鞋", "👕 Warm {hi}°C—breathable short sleeves, comfy shoes"),
            row("temp_else", None, "☀️ 炎熱{hi}°C——排汗短袖、防曬、帽子、多補水", "☀️ Hot {hi}°C—moisture-wicking tee, sun protection, hat, hydrate"),
            row("swing_min", Some(10.0), "☀️ 午後回暖到{hi}°C——穿脫方便的洋蔥式穿搭", "☀️ Warms to {hi}°C afternoon—layer for easy removal"),
            row("rain_min", Some(30.0), "☔ 帶傘（降雨機率高）", "☔ Bring umbrella (likely rain)"),
            row("rain_min", Some(15.0), "🌂 帶折疊傘備用", "🌂 Compact umbrella just in case"),
        ]
    }

    /// Day card rendered with the loaded clothing bands.
    fn render_tips(day: &Day, lang: &str) -> String {
        render_with_logistics(
            day,
            "okinawa-2026",
            lang,
            &super::super::map::MapStatus::default(),
            &sample_tips(),
        )
    }

    // ---- clothing_tip band evaluation (thresholds/order of the old TS clothingTip) ----
    #[test]
    fn clothing_bands_render_exact_text_with_rounded_temps() {
        let tips = clothing_tip(&sample_tips(), 3.4, 14.6, 31.0, "zh");
        assert_eq!(
            tips,
            vec![
                "🧥 早晚3°C——冬季外套+毛衣，下午可脫".to_string(),
                "☀️ 午後回暖到15°C——穿脫方便的洋蔥式穿搭".to_string(),
                "☔ 帶傘（降雨機率高）".to_string(),
            ]
        );
        let en = clothing_tip(&sample_tips(), 7.0, 12.0, 15.0, "en");
        assert_eq!(
            en,
            vec![
                "🧥 7°C morning/evening—jacket+light sweater".to_string(),
                "🌂 Compact umbrella just in case".to_string(),
            ]
        );
        // high <= 18 band once the low is above 10
        assert_eq!(clothing_tip(&sample_tips(), 11.0, 18.0, 0.0, "zh"), vec!["🧥 涼爽18°C——長袖+薄外套".to_string()]);
    }

    #[test]
    fn clothing_empty_table_renders_no_tips() {
        assert!(clothing_tip(&[], 26.0, 30.0, 73.0, "zh").is_empty());
        let day = Day {
            day_number: 3,
            date: "2026-06-14".into(),
            day_type: "full".into(),
            weather_label: "Rain".into(),
            temp_low_c: Some(26.0),
            temp_high_c: Some(30.0),
            precipitation_pct: Some(73.0),
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh", None, &[]);
        assert!(!html.contains("weather-clothing"), "got: {html}");
    }

    #[test]
    fn clothing_hot_rainy_day() {
        // high 30, low 26, rain 73 → 炎熱/防曬 AND 帶傘
        let tips = clothing_tip(&sample_tips(), 26.0, 30.0, 73.0, "zh");
        let joined = tips.join(" · ");
        assert!(joined.contains("炎熱"), "got: {joined}");
        assert!(joined.contains("防曬"), "got: {joined}");
        assert!(joined.contains("帶傘"), "got: {joined}");
    }

    #[test]
    fn clothing_mild_day_no_umbrella() {
        // high 22 → 舒適, rain 5 → no umbrella
        let tips = clothing_tip(&sample_tips(), 16.0, 22.0, 5.0, "zh");
        let joined = tips.join(" · ");
        assert!(joined.contains("舒適"), "got: {joined}");
        assert!(!joined.contains("帶傘"), "got: {joined}");
        assert!(!joined.contains("折疊傘"), "got: {joined}");
    }

    #[test]
    fn clothing_freezing_day() {
        let tips = clothing_tip(&sample_tips(), -1.0, 4.0, 0.0, "zh");
        assert!(tips[0].contains("接近0"), "got: {:?}", tips);
    }

    #[test]
    fn clothing_swing_adds_onion_layer() {
        // high 25, low 12 → swing >= 10 → 洋蔥式
        let tips = clothing_tip(&sample_tips(), 12.0, 25.0, 0.0, "zh");
        let joined = tips.join(" · ");
        assert!(joined.contains("洋蔥式"), "got: {joined}");
    }

    #[test]
    fn clothing_moderate_rain_compact_umbrella() {
        let tips = clothing_tip(&sample_tips(), 16.0, 22.0, 20.0, "zh");
        let joined = tips.join(" · ");
        assert!(joined.contains("折疊傘"), "got: {joined}");
        assert!(!joined.contains("帶傘"), "got: {joined}");
    }

    #[test]
    fn clothing_en_hot() {
        let tips = clothing_tip(&sample_tips(), 26.0, 30.0, 73.0, "en");
        let joined = tips.join(" · ");
        assert!(joined.contains("Hot"), "got: {joined}");
        assert!(joined.contains("Bring umbrella"), "got: {joined}");
    }

    // ---- weather strip ----
    #[test]
    fn weather_strip_shows_feels_like_and_rain() {
        let day = Day {
            day_number: 3,
            date: "2026-06-14".into(),
            day_type: "full".into(),
            weather_label: "Cloudy".into(),
            temp_low_c: Some(26.0),
            temp_high_c: Some(30.0),
            precipitation_pct: Some(73.0),
            feels_like_low_c: Some(28.0),
            feels_like_high_c: Some(34.0),
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh", None, &[]);
        assert!(html.contains("體感"), "got: {html}");
        assert!(html.contains("💧73%"), "got: {html}");
        assert!(html.contains("26\u{2013}30°C"), "got: {html}"); // en-dash range
    }

    #[test]
    fn weather_strip_hot_rainy_renders_clothing() {
        let day = Day {
            day_number: 3,
            date: "2026-06-14".into(),
            day_type: "full".into(),
            weather_label: "Rain".into(),
            temp_low_c: Some(26.0),
            temp_high_c: Some(30.0),
            precipitation_pct: Some(73.0),
            ..Default::default()
        };
        let html = render_tips(&day, "zh");
        assert!(html.contains("weather-clothing"), "got: {html}");
        assert!(html.contains("炎熱"), "got: {html}");
        assert!(html.contains("帶傘"), "got: {html}");
    }

    #[test]
    fn weather_strip_mild_no_umbrella() {
        let day = Day {
            day_number: 3,
            date: "2026-06-14".into(),
            day_type: "full".into(),
            weather_label: "Clear".into(),
            temp_low_c: Some(16.0),
            temp_high_c: Some(22.0),
            precipitation_pct: Some(5.0),
            ..Default::default()
        };
        let html = render_tips(&day, "zh");
        assert!(html.contains("舒適"), "got: {html}");
        assert!(!html.contains("帶傘"), "got: {html}");
    }

    #[test]
    fn weather_strip_uses_feels_like_for_clothing_when_present() {
        // Actual high 28 (would be 溫暖), but feels-like 34 → should pick 炎熱.
        let day = Day {
            day_number: 3,
            date: "2026-06-14".into(),
            day_type: "full".into(),
            weather_label: "Humid".into(),
            temp_low_c: Some(25.0),
            temp_high_c: Some(28.0),
            precipitation_pct: Some(0.0),
            feels_like_low_c: Some(27.0),
            feels_like_high_c: Some(34.0),
            ..Default::default()
        };
        let html = render_tips(&day, "zh");
        assert!(
            html.contains("炎熱"),
            "expected feels-like-driven 炎熱, got: {html}"
        );
    }

    #[test]
    fn weather_strip_zh_renders_chinese_for_mapped_code() {
        let day = Day {
            day_number: 2,
            date: "2026-11-14".into(),
            day_type: "full".into(),
            weather_label: "Moderate rain showers".into(),
            weather_code: Some(81),
            temp_low_c: Some(13.0),
            temp_high_c: Some(18.0),
            precipitation_pct: Some(94.0),
            feels_like_low_c: Some(13.0),
            feels_like_high_c: Some(19.0),
            ..Default::default()
        };
        let html = render(&day, "kyoto-nov-2026", "zh", None, &[]);
        assert!(
            html.contains(
                "<div class=\"weather\">🌧️ 陣雨 · 13\u{2013}18°C · 體感 13\u{2013}19°C · 💧94%</div>"
            ),
            "got: {html}"
        );
        assert!(!html.contains("Moderate rain showers"), "got: {html}");
    }

    #[test]
    fn weather_strip_en_renders_english_for_mapped_code() {
        let day = Day {
            day_number: 2,
            date: "2026-11-14".into(),
            day_type: "full".into(),
            weather_label: "Foggy".into(),
            weather_code: Some(45),
            temp_low_c: Some(10.0),
            temp_high_c: Some(20.0),
            ..Default::default()
        };
        let html = render(&day, "kyoto-nov-2026", "en", None, &[]);
        assert!(
            html.contains("<div class=\"weather\">🌧️ Foggy · 10\u{2013}20°C</div>"),
            "got: {html}"
        );
        let zh = render(&day, "kyoto-nov-2026", "zh", None, &[]);
        assert!(
            zh.contains("<div class=\"weather\">🌧️ 霧 · 10\u{2013}20°C</div>"),
            "got: {zh}"
        );
    }

    #[test]
    fn weather_strip_falls_back_to_stored_label_for_null_or_unmapped_code() {
        // NULL code → stored label, both languages.
        let null_code = Day {
            day_number: 1,
            date: "2026-11-13".into(),
            day_type: "full".into(),
            weather_label: "Partly sunny".into(),
            weather_code: None,
            temp_low_c: Some(12.0),
            temp_high_c: Some(19.0),
            ..Default::default()
        };
        for lang in ["zh", "en"] {
            let html = render(&null_code, "kyoto-nov-2026", lang, None, &[]);
            assert!(
                html.contains("<div class=\"weather\">🌧️ Partly sunny · 12\u{2013}19°C</div>"),
                "{lang}: {html}"
            );
        }
        // Unmapped code → stored label as-is (it is data), both languages.
        let unmapped = Day {
            weather_label: "WMO 123".into(),
            weather_code: Some(123),
            ..null_code
        };
        for lang in ["zh", "en"] {
            let html = render(&unmapped, "kyoto-nov-2026", lang, None, &[]);
            assert!(
                html.contains("<div class=\"weather\">🌧️ WMO 123 · 12\u{2013}19°C</div>"),
                "{lang}: {html}"
            );
        }
    }

    #[test]
    fn weather_strip_only_label_when_no_temps() {
        let day = Day {
            day_number: 1,
            date: "2026-06-12".into(),
            day_type: "arrival".into(),
            weather_label: "Sunny".into(),
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh", None, &[]);
        assert!(html.contains("Sunny"), "got: {html}");
        assert!(!html.contains("體感"), "got: {html}");
        assert!(!html.contains("weather-clothing"), "got: {html}");
    }
}
