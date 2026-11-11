use super::{esc, render_activity_text, session};
use crate::i18n::{fill, t, tf, wmo_emoji, wmo_weather};
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

/// The day's weather description in the page language: the WMO `weather_code`
/// via `i18n::wmo_weather`; a NULL or unmapped code shows the stored
/// `weather_label` (data, as-is). Empty when neither is present.
fn weather_description<'a>(day: &'a Day, lang: &str) -> &'a str {
    day.weather_code
        .and_then(|code| wmo_weather(code, lang))
        .unwrap_or(&day.weather_label)
}

/// Whether the day has any weather to show (description, a temp, or rain%).
/// The day strip and the weather overview use the same test.
fn has_weather(day: &Day, lang: &str) -> bool {
    !weather_description(day, lang).is_empty()
        || day.temp_low_c.is_some()
        || day.temp_high_c.is_some()
        || day.precipitation_pct.is_some()
}

/// The day's what-to-wear lines (unescaped): feels-like temps when available,
/// else actual; rain defaults to 0. Empty without a temperature pair or when
/// no band matches. Shared by the day strip and the weather overview.
fn day_clothing(day: &Day, clothing_tips: &[ClothingTip], lang: &str) -> Vec<String> {
    let tip_low = day.feels_like_low_c.or(day.temp_low_c);
    let tip_high = day.feels_like_high_c.or(day.temp_high_c);
    match (tip_low, tip_high) {
        (Some(tl), Some(th)) => {
            let rain = day.precipitation_pct.unwrap_or(0.0);
            clothing_tip(clothing_tips, tl, th, rain, lang)
        }
        _ => Vec::new(),
    }
}

/// "12–20°C" from a low/high pair, rounded; None unless both are present.
fn temp_range(lo: Option<f64>, hi: Option<f64>) -> Option<String> {
    match (lo, hi) {
        (Some(lo), Some(hi)) => Some(format!(
            "{}\u{2013}{}°C",
            lo.round() as i64,
            hi.round() as i64
        )),
        _ => None,
    }
}

/// Render the per-day weather strip: icon + description + temp range +
/// feels-like + rain%, followed by the clothing tips. Each piece is shown only
/// when present. The icon comes from the WMO code (`i18n::wmo_emoji`, a neutral
/// 🌡️ for a NULL/unmapped code); the description from [`weather_description`].
fn weather_strip(day: &Day, clothing_tips: &[ClothingTip], lang: &str) -> String {
    // No weather data at all → nothing.
    if !has_weather(day, lang) {
        return String::new();
    }
    let description = weather_description(day, lang);

    let mut strip = format!("<div class=\"weather\">{} ", wmo_emoji(day.weather_code));
    if !description.is_empty() {
        strip.push_str(&esc(description));
    }
    // temp range
    if let Some(range) = temp_range(day.temp_low_c, day.temp_high_c) {
        strip.push_str(&format!(" · {range}"));
    }
    // feels-like range
    if let Some(range) = temp_range(day.feels_like_low_c, day.feels_like_high_c) {
        strip.push_str(&format!(" · {} {range}", t("feelsLike", lang)));
    }
    // rain
    if let Some(rain) = day.precipitation_pct {
        strip.push_str(&format!(" · 💧{}%", rain.round() as i64));
    }
    strip.push_str("</div>");

    let tips = day_clothing(day, clothing_tips, lang);
    if !tips.is_empty() {
        let joined = tips.iter().map(|t| esc(t)).collect::<Vec<_>>().join(" · ");
        strip.push_str(&format!("<div class=\"weather-clothing\">{joined}</div>"));
    }
    strip
}

/// The plan page's collapsible 天氣總覽 / Weather overview: one row per MAIN
/// trip day (variants share their main day's date and carry no weather) with
/// date + weekday, icon + description, temp range, feels-like range, rain%, and
/// the day's clothing tips — the same values the day strips show. A native
/// `<details open>` (no JS; viewer pages are SSR-only) that starts OPEN, so
/// nothing is hidden on load. A missing value is an em dash; when no day has
/// any weather the section renders nothing at all. Phones get one stacked block
/// per day (styles.css `.wx-table`), wider screens a table.
pub fn weather_overview(days: &[Day], clothing_tips: &[ClothingTip], lang: &str) -> String {
    const DASH: &str = "\u{2014}";
    let main_days: Vec<&Day> = days.iter().filter(|d| d.variant_of.is_none()).collect();
    if !main_days.iter().any(|d| has_weather(d, lang)) {
        return String::new();
    }
    let temp_label = esc(t("wxTemp", lang));
    let feels_label = esc(t("feelsLike", lang));
    let rain_label = esc(t("wxRain", lang));
    let mut h = format!(
        "<details class=\"weather-overview\" open><summary><h2>{}</h2></summary>\
         <table class=\"wx-table\"><thead><tr><th scope=\"col\">{}</th><th scope=\"col\">{}</th>\
         <th scope=\"col\">{temp_label}</th><th scope=\"col\">{feels_label}</th>\
         <th scope=\"col\">{rain_label}</th><th scope=\"col\">{}</th></tr></thead><tbody>",
        esc(t("weatherOverview", lang)),
        esc(t("wxDate", lang)),
        esc(t("wxWeather", lang)),
        esc(t("wxWear", lang)),
    );
    for day in main_days {
        let date = match super::summary::date_with_weekday(&day.date, lang) {
            Some(d) => esc(&d),
            None if day.date.is_empty() => DASH.to_string(),
            None => esc(&day.date),
        };
        let description = weather_description(day, lang);
        let description = if description.is_empty() {
            DASH.to_string()
        } else {
            esc(description)
        };
        let temp = temp_range(day.temp_low_c, day.temp_high_c).unwrap_or_else(|| DASH.into());
        let feels = temp_range(day.feels_like_low_c, day.feels_like_high_c)
            .unwrap_or_else(|| DASH.into());
        let rain = day
            .precipitation_pct
            .map(|r| format!("{}%", r.round() as i64))
            .unwrap_or_else(|| DASH.into());
        let tips = day_clothing(day, clothing_tips, lang);
        let wear = if tips.is_empty() {
            DASH.to_string()
        } else {
            tips.iter().map(|t| esc(t)).collect::<Vec<_>>().join(" · ")
        };
        h.push_str(&format!(
            "<tr><th scope=\"row\" class=\"wx-date\">{date}</th>\
             <td class=\"wx-desc\">{} {description}</td>\
             <td class=\"wx-temp\" data-label=\"{temp_label}\">{temp}</td>\
             <td class=\"wx-feels\" data-label=\"{feels_label}\">{feels}</td>\
             <td class=\"wx-rain\" data-label=\"{rain_label}\">{rain}</td>\
             <td class=\"wx-wear\">{wear}</td></tr>",
            wmo_emoji(day.weather_code),
        ));
    }
    h.push_str("</tbody></table></details>");
    h
}

/// The plan page's collapsible 用餐總覽 / Meal overview: one row per day per
/// session that has meals, in trip order — each main day's sessions
/// (morning → noon → afternoon → evening), then each of its variants' sessions.
/// Columns: date + weekday, session name, 主選 (the session's first meal in
/// sort_order), 備選 (every later meal, one per line; em dash when none). Cells
/// show [`session::meal_short`]'s 「料理｜店名」 form plus the day cards'
/// AI-recommended badge. A day with variants labels its rows with the day's
/// option name (「11/15（日）· 雨天方案」) and each variant's rows with its own
/// (「11/15（日）· 晴天方案」); a variant's meals are listed even when they
/// repeat the main day's — the table states what each plan has. Variants only
/// count nested under their main day (as the assembler builds them); a stray
/// top-level variant is skipped, like the weather overview does. A native
/// `<details open>` (no JS), the same pattern as [`weather_overview`]; no meals
/// anywhere → nothing at all. Phones get one stacked block per row
/// (styles.css `.meal-table`), wider screens a table.
pub fn meal_overview(days: &[Day], lang: &str) -> String {
    const DASH: &str = "\u{2014}";
    let date_cell = |day: &Day, fallback_date: &str, option: &str| -> String {
        let raw = if day.date.is_empty() { fallback_date } else { &day.date };
        let date = match super::summary::date_with_weekday(raw, lang) {
            Some(d) => d,
            None if raw.is_empty() => DASH.to_string(),
            None => raw.to_string(),
        };
        if option.is_empty() {
            esc(&date)
        } else {
            esc(&tf("dateOption", lang, &[("date", &date), ("label", option)]))
        }
    };
    let meal_cell = |m: &crate::model::Meal| -> String {
        let short = session::meal_short(&m.text);
        let short = if short.is_empty() { DASH.to_string() } else { esc(&short) };
        format!("{short}{}", super::ai_rec_badge(&m.source, lang))
    };
    let main_label = esc(t("mealMain", lang));
    let backup_label = esc(t("mealBackup", lang));
    let mut rows = String::new();
    for main in days.iter().filter(|d| d.variant_of.is_none()) {
        let has_variants = !main.variants.is_empty();
        for day in std::iter::once(main).chain(main.variants.iter()) {
            let option = if has_variants {
                super::day_option_name(day, lang)
            } else {
                ""
            };
            for sess in day.sessions.iter().filter(|s| !s.meals.is_empty()) {
                let backups: Vec<String> = sess.meals[1..].iter().map(meal_cell).collect();
                let backup = if backups.is_empty() {
                    DASH.to_string()
                } else {
                    backups.join("<br>")
                };
                rows.push_str(&format!(
                    "<tr><th scope=\"row\" class=\"ml-date\">{}</th>\
                     <td class=\"ml-session\">{}</td>\
                     <td class=\"ml-main\" data-label=\"{main_label}\">{}</td>\
                     <td class=\"ml-backup\" data-label=\"{backup_label}\">{backup}</td></tr>",
                    date_cell(day, &main.date, option),
                    esc(session::session_label(&sess.session_type, lang)),
                    meal_cell(&sess.meals[0]),
                ));
            }
        }
    }
    if rows.is_empty() {
        return String::new();
    }
    format!(
        "<details class=\"meal-overview\" open><summary><h2>{}</h2></summary>\
         <table class=\"meal-table\"><thead><tr><th scope=\"col\">{}</th><th scope=\"col\">{}</th>\
         <th scope=\"col\">{main_label}</th><th scope=\"col\">{backup_label}</th></tr></thead>\
         <tbody>{rows}</tbody></table></details>",
        esc(t("mealOverview", lang)),
        esc(t("wxDate", lang)),
        esc(t("mealSession", lang)),
    )
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
    // Each variant is a native `<details>` that starts COLLAPSED (no `open`;
    // SSR-only, no JS): the main plan reads first, the alternative is one tap away.
    for variant in &day.variants {
        let option_label = super::day_option_label(variant, day.day_number, lang);
        h.push_str(&format!(
            "<details class=\"day-variant\" data-variant-of=\"{}\"><summary class=\"day-variant-summary\">{}</summary><div class=\"day-variant-body\">",
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
            weather_label: "Clear sky".into(),
            weather_code: Some(0),
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
            "<section class=\"day day-full\" data-day=\"2\"><h2>第 2 天 · 2026-11-14</h2><div class=\"theme\">京都巡禮</div><div class=\"weather\">☀️ 晴 · 12–20°C · 💧10%</div><div class=\"weather-clothing\">👕 舒適20°C——長短袖皆可，早晚帶薄外套</div><div class=\"day-map-stack\"><figure class=\"map-frame\" data-map-key=\"day-2.png\"><img class=\"daymap\" alt=\"第 2 天路線\" src=\"/map/kyoto-nov-2026/day-2.png?v=dayv1\"><figcaption>第 2 天路線 <span class=\"map-attribution\">© <a href=\"https://www.openstreetmap.org/copyright\" target=\"_blank\" rel=\"noopener\">OpenStreetMap contributors</a></span></figcaption><ol class=\"map-legend\"><li><span class=\"leg-num\">1</span><a href=\"https://www.google.com/maps?q=35.0394,135.7292\" target=\"_blank\" rel=\"noopener\">金閣寺</a></li></ol></figure><figure class=\"map-frame map-frame--day-logistics\" data-map-key=\"day-2-logistics.png\"><img class=\"planmap planmap--inset\" alt=\"第 2 天・旅社來回\" src=\"/map/kyoto-nov-2026/day-2-logistics.png?v=logv1\"><figcaption>第 2 天・旅社來回 <span class=\"map-attribution\">© <a href=\"https://www.openstreetmap.org/copyright\" target=\"_blank\" rel=\"noopener\">OpenStreetMap contributors</a></span></figcaption><ol class=\"map-legend\"><li><span class=\"leg-num\">1</span><a href=\"https://www.google.com/maps?q=35.0394,135.7292\" target=\"_blank\" rel=\"noopener\">金閣寺</a></li></ol></figure><figure class=\"map-frame map-frame--day-logistics map-frame--day-away\" data-map-key=\"day-2-away.png\"><img class=\"planmap planmap--inset\" alt=\"第 2 天・本區以外的行程\" src=\"/map/kyoto-nov-2026/day-2-away.png?v=awayv1\"><figcaption>第 2 天・本區以外的行程 <span class=\"map-attribution\">© <a href=\"https://www.openstreetmap.org/copyright\" target=\"_blank\" rel=\"noopener\">OpenStreetMap contributors</a></span></figcaption><ol class=\"map-legend\"><li><span class=\"leg-num\">1</span><a href=\"https://www.google.com/maps?q=35.0394,135.7292\" target=\"_blank\" rel=\"noopener\">金閣寺</a></li></ol></figure></div><div class=\"day-landmarks\"><div class=\"landmarks-heading\">地圖景點</div><ul><li>金閣寺</li></ul></div><div class=\"route-block\"><div class=\"route-heading\">今日路線</div><div class=\"route-seg\"><span class=\"route-mode\">🚌</span> 旅社 → 金閣寺 · 約 25 分鐘</div></div><div class=\"session session-morning\"><div class=\"session-label\">上午</div><div class=\"session-focus\">寺院散策</div><div class=\"activity\">金閣寺參觀</div></div></section>"
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
            html.contains("<div class=\"weather\">🌫️ Foggy · 10\u{2013}20°C</div>"),
            "got: {html}"
        );
        let zh = render(&day, "kyoto-nov-2026", "zh", None, &[]);
        assert!(
            zh.contains("<div class=\"weather\">🌫️ 霧 · 10\u{2013}20°C</div>"),
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
                html.contains("<div class=\"weather\">🌡️ Partly sunny · 12\u{2013}19°C</div>"),
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
                html.contains("<div class=\"weather\">🌡️ WMO 123 · 12\u{2013}19°C</div>"),
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

    // ---- weather overview ----
    fn overview_day() -> Day {
        Day {
            day_number: 2,
            date: "2026-11-14".into(), // a Saturday
            day_type: "full".into(),
            weather_label: "Moderate rain showers".into(),
            weather_code: Some(81),
            temp_low_c: Some(13.2),
            temp_high_c: Some(17.6),
            precipitation_pct: Some(94.0),
            feels_like_low_c: Some(13.0),
            feels_like_high_c: Some(19.0),
            ..Default::default()
        }
    }

    #[test]
    fn weather_overview_zh_renders_open_details_and_a_full_row() {
        let html = weather_overview(&[overview_day()], &sample_tips(), "zh");
        assert!(
            html.starts_with(
                "<details class=\"weather-overview\" open><summary><h2>天氣總覽</h2></summary>\
                 <table class=\"wx-table\"><thead><tr><th scope=\"col\">日期</th><th scope=\"col\">天氣</th>\
                 <th scope=\"col\">氣溫</th><th scope=\"col\">體感</th><th scope=\"col\">降雨機率</th>\
                 <th scope=\"col\">穿著建議</th></tr></thead><tbody>"
            ),
            "got: {html}"
        );
        // Clothing uses the feels-like pair (13/19) and rain 94% — the same
        // lines the day strip prints.
        assert!(
            html.contains(
                "<tr><th scope=\"row\" class=\"wx-date\">11/14（六）</th>\
                 <td class=\"wx-desc\">\u{1F327}\u{FE0F} 陣雨</td>\
                 <td class=\"wx-temp\" data-label=\"氣溫\">13\u{2013}18°C</td>\
                 <td class=\"wx-feels\" data-label=\"體感\">13\u{2013}19°C</td>\
                 <td class=\"wx-rain\" data-label=\"降雨機率\">94%</td>\
                 <td class=\"wx-wear\">👕 舒適19°C——長短袖皆可，早晚帶薄外套 · ☔ 帶傘（降雨機率高）</td></tr>"
            ),
            "got: {html}"
        );
        assert!(html.ends_with("</tbody></table></details>"), "got: {html}");
        assert!(!html.contains("Moderate rain showers"), "got: {html}");
    }

    #[test]
    fn weather_overview_en_renders_english_chrome_and_row() {
        let html = weather_overview(&[overview_day()], &sample_tips(), "en");
        assert!(html.contains("<summary><h2>Weather overview</h2></summary>"), "got: {html}");
        assert!(html.contains("<th scope=\"col\">What to wear</th>"), "got: {html}");
        assert!(
            html.contains(
                "<tr><th scope=\"row\" class=\"wx-date\">Sat 11/14</th>\
                 <td class=\"wx-desc\">\u{1F327}\u{FE0F} Moderate rain showers</td>\
                 <td class=\"wx-temp\" data-label=\"Temp\">13\u{2013}18°C</td>\
                 <td class=\"wx-feels\" data-label=\"Feels\">13\u{2013}19°C</td>\
                 <td class=\"wx-rain\" data-label=\"Rain\">94%</td>\
                 <td class=\"wx-wear\">👕 Mild 19°C—short or long sleeves, light layer for evening · ☔ Bring umbrella (likely rain)</td></tr>"
            ),
            "got: {html}"
        );
        assert!(!html.contains("陣雨") && !html.contains("天氣總覽"), "got: {html}");
    }

    #[test]
    fn weather_overview_skips_variant_days() {
        let variant = Day {
            day_number: 3,
            date: "2026-11-14".into(),
            variant_of: Some(2),
            weather_label: "variant weather must not render".into(),
            temp_low_c: Some(1.0),
            temp_high_c: Some(2.0),
            ..Default::default()
        };
        let main = Day {
            variants: vec![Day { ..variant }],
            ..overview_day()
        };
        // A variant reaching the top-level list (the assembler nests them) is
        // skipped too: one row, for the main day only.
        let stray = Day {
            day_number: 4,
            date: "2026-11-14".into(),
            variant_of: Some(2),
            weather_label: "variant weather must not render".into(),
            ..Default::default()
        };
        let html = weather_overview(&[main, stray], &sample_tips(), "zh");
        assert_eq!(html.matches("scope=\"row\"").count(), 1, "got: {html}");
        assert!(!html.contains("variant weather must not render"), "got: {html}");
        assert!(!html.contains("1\u{2013}2°C"), "got: {html}");
    }

    #[test]
    fn weather_overview_renders_em_dash_for_missing_values() {
        // Day 2 has only a stored label (NULL code, no temps, no rain): the
        // neutral icon + label, and a dash for every number and the clothing
        // (no temperature pair → no band). Day 3 has nothing at all, and its
        // date does not parse (shown as-is, escaped); day 4 has no date.
        let label_only = Day {
            day_number: 2,
            date: "2026-11-15".into(),
            weather_label: "Partly <sunny>".into(),
            ..Default::default()
        };
        let nothing = Day {
            day_number: 3,
            date: "TBD<".into(),
            ..Default::default()
        };
        let no_date = Day { day_number: 4, ..Default::default() };
        let html = weather_overview(
            &[overview_day(), label_only, nothing, no_date],
            &sample_tips(),
            "zh",
        );
        assert_eq!(html.matches("scope=\"row\"").count(), 4, "got: {html}");
        assert!(
            html.contains(
                "<tr><th scope=\"row\" class=\"wx-date\">11/15（日）</th>\
                 <td class=\"wx-desc\">\u{1F321}\u{FE0F} Partly &lt;sunny&gt;</td>\
                 <td class=\"wx-temp\" data-label=\"氣溫\">\u{2014}</td>\
                 <td class=\"wx-feels\" data-label=\"體感\">\u{2014}</td>\
                 <td class=\"wx-rain\" data-label=\"降雨機率\">\u{2014}</td>\
                 <td class=\"wx-wear\">\u{2014}</td></tr>"
            ),
            "got: {html}"
        );
        assert!(
            html.contains(
                "<tr><th scope=\"row\" class=\"wx-date\">TBD&lt;</th>\
                 <td class=\"wx-desc\">\u{1F321}\u{FE0F} \u{2014}</td>"
            ),
            "got: {html}"
        );
        assert!(
            html.contains("<tr><th scope=\"row\" class=\"wx-date\">\u{2014}</th>"),
            "got: {html}"
        );
        assert!(!html.contains("Partly <sunny>"), "got: {html}");
    }

    #[test]
    fn weather_overview_renders_nothing_without_any_weather() {
        // A day with neither a code/label nor any number, and only variants
        // (nested and stray) carrying weather: nothing at all — variants are
        // skipped, and the section has no row worth showing.
        let days = || {
            vec![
                Day {
                    day_number: 1,
                    date: "2026-11-13".into(),
                    ..Default::default()
                },
                Day {
                    day_number: 2,
                    date: "2026-11-14".into(),
                    variants: vec![Day { variant_of: Some(2), ..overview_day() }],
                    ..Default::default()
                },
                Day { day_number: 3, variant_of: Some(2), ..overview_day() },
            ]
        };
        for lang in ["zh", "en"] {
            assert_eq!(weather_overview(&[], &sample_tips(), lang), "", "{lang}");
            assert_eq!(weather_overview(&days(), &sample_tips(), lang), "", "{lang}");
        }
    }

    // ---- meal overview ----
    fn ml(text: &str, source: &str) -> Meal {
        Meal { text: text.into(), source: source.into() }
    }

    /// The four canonical sessions; `meals` maps a session type to its meals.
    fn meal_sessions(meals: Vec<(&str, Vec<Meal>)>) -> Vec<Session> {
        crate::model::SESSION_ORDER
            .iter()
            .map(|&st| Session {
                session_type: st.into(),
                meals: meals
                    .iter()
                    .find(|(t, _)| *t == st)
                    .map(|(_, m)| m.clone())
                    .unwrap_or_default(),
                ..Default::default()
            })
            .collect()
    }

    const KANEYO: &str = "主選：鰻魚飯｜京極かねよ：新京極六角，排隊約 20 分；招牌錦糸丼｜map:京極かねよ";
    const PEACH: &str = "去程午餐｜樂桃不含機上餐；機場先買便當";

    fn meal_day() -> Day {
        Day {
            day_number: 2,
            date: "2026-11-14".into(), // a Saturday
            sessions: meal_sessions(vec![
                ("noon", vec![ml(PEACH, "confirmed")]),
                (
                    "evening",
                    vec![
                        ml(KANEYO, "confirmed"),
                        ml("備選：親子丼｜鳥岩樓：西陣｜map:鳥岩樓", "ai_recommended"),
                        ml("Ramen & <gyoza> at the station food court near the hotel", "confirmed"),
                    ],
                ),
            ]),
            ..Default::default()
        }
    }

    #[test]
    fn meal_overview_zh_renders_open_details_and_short_rows() {
        let html = meal_overview(&[meal_day()], "zh");
        assert!(
            html.starts_with(
                "<details class=\"meal-overview\" open><summary><h2>用餐總覽</h2></summary>\
                 <table class=\"meal-table\"><thead><tr><th scope=\"col\">日期</th>\
                 <th scope=\"col\">餐別</th><th scope=\"col\">主選</th><th scope=\"col\">備選</th>\
                 </tr></thead><tbody>"
            ),
            "got: {html}"
        );
        // Noon: a single meal → its short form, no backup (em dash).
        assert!(
            html.contains(
                "<tr><th scope=\"row\" class=\"ml-date\">11/14（六）</th>\
                 <td class=\"ml-session\">中午</td>\
                 <td class=\"ml-main\" data-label=\"主選\">去程午餐｜樂桃不含機上餐</td>\
                 <td class=\"ml-backup\" data-label=\"備選\">\u{2014}</td></tr>"
            ),
            "got: {html}"
        );
        // Evening: sort_order 0 is 主選; 1 and 2 are both 備選, one per line,
        // the AI-recommended one carrying the day cards' badge, the free-text
        // one cut to 30 characters (escaped).
        assert!(
            html.contains(
                "<tr><th scope=\"row\" class=\"ml-date\">11/14（六）</th>\
                 <td class=\"ml-session\">晚上</td>\
                 <td class=\"ml-main\" data-label=\"主選\">鰻魚飯｜京極かねよ</td>\
                 <td class=\"ml-backup\" data-label=\"備選\">親子丼｜鳥岩樓\
                 <span class=\"ai-rec-badge\">🤖 建議（未確認）</span>\
                 <br>Ramen &amp; &lt;gyoza&gt; at the station\u{2026}</td></tr>"
            ),
            "got: {html}"
        );
        assert!(html.ends_with("</tbody></table></details>"), "got: {html}");
        // The pin, the role prefix and the detail tail never reach the table.
        for raw in ["map:", "主選：", "新京極六角", "機場先買便當"] {
            assert!(!html.contains(raw), "{raw} leaked; got: {html}");
        }
    }

    #[test]
    fn meal_overview_en_renders_english_chrome() {
        let html = meal_overview(&[meal_day()], "en");
        assert!(html.contains("<summary><h2>Meal overview</h2></summary>"), "got: {html}");
        assert!(
            html.contains(
                "<th scope=\"col\">Date</th><th scope=\"col\">Meal</th>\
                 <th scope=\"col\">Main pick</th><th scope=\"col\">Backup</th>"
            ),
            "got: {html}"
        );
        assert!(
            html.contains(
                "<tr><th scope=\"row\" class=\"ml-date\">Sat 11/14</th>\
                 <td class=\"ml-session\">Evening</td>\
                 <td class=\"ml-main\" data-label=\"Main pick\">鰻魚飯｜京極かねよ</td>\
                 <td class=\"ml-backup\" data-label=\"Backup\">親子丼｜鳥岩樓\
                 <span class=\"ai-rec-badge\">🤖 AI-recommended (unconfirmed)</span>"
            ),
            "got: {html}"
        );
        for zh in ["用餐總覽", "餐別", "晚上", "（六）", "建議（未確認）"] {
            assert!(!html.contains(zh), "{zh} in EN page; got: {html}");
        }
    }

    #[test]
    fn meal_overview_rows_follow_day_then_session_order() {
        let day1 = Day {
            day_number: 1,
            date: "2026-11-13".into(),
            sessions: meal_sessions(vec![
                ("evening", vec![ml("D1 dinner", "confirmed")]),
                ("morning", vec![ml("D1 breakfast", "confirmed")]),
            ]),
            ..Default::default()
        };
        let day2 = Day {
            day_number: 2,
            date: "2026-11-14".into(),
            sessions: meal_sessions(vec![
                ("afternoon", vec![ml("D2 tea", "confirmed")]),
                ("noon", vec![ml("D2 lunch", "confirmed")]),
            ]),
            ..Default::default()
        };
        let html = meal_overview(&[day1, day2], "en");
        let order: Vec<usize> = ["D1 breakfast", "D1 dinner", "D2 lunch", "D2 tea"]
            .iter()
            .map(|m| html.find(m).unwrap_or_else(|| panic!("{m} missing: {html}")))
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}: {html}");
        // Sessions without meals get no row.
        assert_eq!(html.matches("scope=\"row\"").count(), 4, "got: {html}");
    }

    #[test]
    fn meal_overview_labels_main_and_variant_rows_with_their_option_names() {
        let lunch = || vec![("noon", vec![ml("湯豆腐｜奧丹：南禪寺參道｜map:奧丹", "confirmed")])];
        let variant = Day {
            day_number: 4,
            date: "2026-11-15".into(),
            variant_of: Some(3),
            variant_label: "Sunny-day plan".into(),
            variant_label_zh: "晴天方案".into(),
            // Same lunch as the main day: still listed, under the variant's label.
            sessions: meal_sessions(lunch()),
            ..Default::default()
        };
        let main = Day {
            day_number: 3,
            date: "2026-11-15".into(), // a Sunday
            variant_label: "Rainy-day plan".into(),
            variant_label_zh: "雨天方案".into(),
            sessions: meal_sessions(lunch()),
            variants: vec![variant],
            ..Default::default()
        };
        // A stray top-level variant (the assembler nests them) is skipped.
        let stray = Day {
            day_number: 5,
            date: "2026-11-15".into(),
            variant_of: Some(3),
            sessions: meal_sessions(vec![("noon", vec![ml("stray must not render", "confirmed")])]),
            ..Default::default()
        };
        let html = meal_overview(&[main, stray], "zh");
        assert_eq!(html.matches("scope=\"row\"").count(), 2, "got: {html}");
        let rainy = html
            .find("<th scope=\"row\" class=\"ml-date\">11/15（日）· 雨天方案</th><td class=\"ml-session\">中午</td><td class=\"ml-main\" data-label=\"主選\">湯豆腐｜奧丹</td>")
            .unwrap_or_else(|| panic!("rainy row: {html}"));
        let sunny = html
            .find("<th scope=\"row\" class=\"ml-date\">11/15（日）· 晴天方案</th><td class=\"ml-session\">中午</td><td class=\"ml-main\" data-label=\"主選\">湯豆腐｜奧丹</td>")
            .unwrap_or_else(|| panic!("sunny row: {html}"));
        assert!(rainy < sunny, "main day first; got: {html}");
        assert!(!html.contains("stray must not render"), "got: {html}");

        let en = meal_overview(
            &[Day {
                day_number: 3,
                date: "2026-11-15".into(),
                variant_label: "Rainy-day plan".into(),
                variant_label_zh: "雨天方案".into(),
                sessions: meal_sessions(lunch()),
                variants: vec![Day {
                    day_number: 4,
                    date: String::new(), // no own date → the main day's
                    variant_of: Some(3),
                    variant_label: "Sunny-day plan".into(),
                    variant_label_zh: "晴天方案".into(),
                    sessions: meal_sessions(lunch()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            "en",
        );
        assert!(en.contains("<th scope=\"row\" class=\"ml-date\">Sun 11/15 · Rainy-day plan</th>"), "got: {en}");
        assert!(en.contains("<th scope=\"row\" class=\"ml-date\">Sun 11/15 · Sunny-day plan</th>"), "got: {en}");
    }

    #[test]
    fn meal_overview_day_without_variants_has_no_option_label() {
        // A plain day keeps a bare date even if a stale variant label lingers.
        let day = Day { variant_label_zh: "雨天方案".into(), ..meal_day() };
        let html = meal_overview(&[day], "zh");
        assert!(html.contains("<th scope=\"row\" class=\"ml-date\">11/14（六）</th>"), "got: {html}");
        assert!(!html.contains("雨天方案"), "got: {html}");
    }

    #[test]
    fn meal_overview_renders_nothing_without_meals() {
        // Days with sessions but no meals, plus a stray top-level variant with
        // meals (skipped): nothing at all.
        let days = || {
            vec![
                Day {
                    day_number: 1,
                    date: "2026-11-13".into(),
                    sessions: meal_sessions(vec![]),
                    ..Default::default()
                },
                Day { day_number: 2, date: "2026-11-14".into(), ..Default::default() },
                Day { day_number: 3, variant_of: Some(2), ..meal_day() },
            ]
        };
        for lang in ["zh", "en"] {
            assert_eq!(meal_overview(&[], lang), "", "{lang}");
            assert_eq!(meal_overview(&days(), lang), "", "{lang}");
        }
    }
}
