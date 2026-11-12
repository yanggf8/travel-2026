//! The plan page's during-trip top blocks (user request 2026-11-12: on a phone
//! mid-trip the page held too much to find things — "what time does the return
//! flight land" at the parking lot, the hotel phone for the arrival card):
//!
//! - **現在・接下來 / Now & next** ([`now_card`]): today's main day as a vertical
//!   timeline — each session with its time range, focus line, timed route legs,
//!   main + backup meal — with the CURRENT session badged 現在 and the next
//!   session / leg 接下來; after the last session, tomorrow's first session as
//!   明天. Rendered only while today's JST date is a main day's date.
//! - **常用資訊 / Quick info** ([`quick_info`]): outbound / return flights and
//!   the booked hotel's name, address, phone and stay — always, when data exists.
//!
//! "Now" is Japan time (Asia/Tokyo, UTC+9, no DST) at request time ([`jst_from_ms`]),
//! or the strictly validated `?at=YYYY-MM-DDTHH:MM` override ([`parse_at`]) that
//! tests and `travel verify-site` use to pin the clock. Marking is server-side;
//! the plan page's inline script re-marks every minute from the `data-start` /
//! `data-end` (JST minutes) attributes (render/mod.rs `BLOCK_MEMORY_SCRIPT`).

use super::{esc, esc_url_attr, urlencode};
use crate::i18n::{t, tf};
use crate::model::{Day, Plan, RouteSegment, Session};

/// Element id / `data-block` key of the 現在・接下來 card.
pub const BLOCK_NOW: &str = "sec-now";
/// Element id / `data-block` key of the 常用資訊 card.
pub const BLOCK_QUICK: &str = "sec-quick";

/// Minutes in a day: the open end of a session with no later start.
const DAY_END: i64 = 24 * 60;
/// Japan Standard Time: UTC+9 all year.
const JST_OFFSET_MS: i64 = 9 * 3_600_000;

/// A JST wall-clock instant: the date and minutes since midnight. `pinned` is
/// set when it came from `?at=` — the inline script then never re-marks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clock {
    pub date: String,
    pub minutes: i64,
    pub pinned: bool,
}

/// The JST clock for a Unix epoch time in milliseconds (the worker's `Date::now()`).
pub fn jst_from_ms(ms: i64) -> Clock {
    let jst = ms + JST_OFFSET_MS;
    Clock {
        date: super::index::today_from_ms(jst),
        minutes: jst.rem_euclid(86_400_000) / 60_000,
        pinned: false,
    }
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// Strict `?at=YYYY-MM-DDTHH:MM` (JST): exactly that shape, a real calendar
/// date, 00:00–23:59. Anything else is None (the caller ignores it).
pub fn parse_at(s: &str) -> Option<Clock> {
    let b = s.as_bytes();
    if b.len() != 16 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let part = &b[r];
        if part.iter().all(u8::is_ascii_digit) {
            std::str::from_utf8(part).ok()?.parse().ok()
        } else {
            None
        }
    };
    let (y, m, d, hh, mm) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?);
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) || hh > 23 || mm > 59 {
        return None;
    }
    Some(Clock { date: s[..10].to_string(), minutes: hh * 60 + mm, pinned: true })
}

/// "H:MM" / "HH:MM" → minutes since midnight; "24:00" is the day's end.
pub fn parse_hhmm(s: &str) -> Option<i64> {
    let (h, m) = s.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 || !h.bytes().chain(m.bytes()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
    if m > 59 || h > 24 || (h == 24 && m > 0) {
        return None;
    }
    Some(h * 60 + m)
}

/// Today's MAIN day (variants never anchor the trip window): the main day whose
/// date is the clock's date. None = outside the trip — the page keeps its
/// regular, all-open shape.
pub fn trip_today<'a>(days: &'a [Day], clock: &Clock) -> Option<&'a Day> {
    days.iter().find(|d| d.variant_of.is_none() && !d.date.is_empty() && d.date == clock.date)
}

/// The next main day after `today` by date (the 明天 preview).
fn next_main_day<'a>(days: &'a [Day], today: &Day) -> Option<&'a Day> {
    days.iter()
        .filter(|d| d.variant_of.is_none() && !d.date.is_empty() && d.date > today.date)
        .min_by(|a, b| a.date.cmp(&b.date))
}

/// A shown session with its effective window `[start, end)` in JST minutes.
/// A missing start is the previous session's end (0 for the first); a missing
/// end is the next explicit start among the later sessions (end of day when
/// none) — e.g. a flight 「中午」 with no times spans morning-end → afternoon-start.
#[derive(Debug, PartialEq)]
pub struct Slot<'a> {
    pub sess: &'a Session,
    pub start: i64,
    pub end: i64,
}

pub fn slots(day: &Day) -> Vec<Slot<'_>> {
    let shown: Vec<&Session> = day.sessions.iter().filter(|s| super::session::has_content(s)).collect();
    let explicit: Vec<(Option<i64>, Option<i64>)> = shown
        .iter()
        .map(|s| (parse_hhmm(&s.time_start), parse_hhmm(&s.time_end)))
        .collect();
    let mut out = Vec::with_capacity(shown.len());
    let mut prev_end = 0;
    for (i, sess) in shown.into_iter().enumerate() {
        let start = explicit[i].0.unwrap_or(prev_end);
        let end = explicit[i]
            .1
            .or_else(|| explicit[i + 1..].iter().find_map(|e| e.0))
            .unwrap_or(DAY_END)
            .max(start);
        prev_end = end;
        out.push(Slot { sess, start, end });
    }
    out
}

/// Which slot is 現在 (the LATEST-starting window holding `now`, so an
/// overlapping later session wins once it has begun), which is 接下來 (the
/// first starting after `now`), and whether the day is done (neither).
#[derive(Debug, PartialEq, Eq)]
pub struct Marks {
    pub current: Option<usize>,
    pub next: Option<usize>,
    pub done: bool,
}

pub fn mark(windows: &[(i64, i64)], now: i64) -> Marks {
    let current = windows.iter().rposition(|&(s, e)| s <= now && now < e);
    let next = windows.iter().position(|&(s, _)| s > now);
    Marks { current, next, done: current.is_none() && next.is_none() }
}

/// The slot a TIMED leg belongs to: the window holding its start time, else
/// the first window starting after it (travel toward that session), else the
/// last. `day_route_segments` has no session key, so an untimed leg is never
/// placed — it shows in the card's full-route list only.
pub fn leg_slot(windows: &[(i64, i64)], at: i64) -> Option<usize> {
    windows
        .iter()
        .rposition(|&(s, e)| s <= at && at < e)
        .or_else(|| windows.iter().position(|&(s, _)| s > at))
        .or_else(|| windows.len().checked_sub(1))
}

fn badge(kind: &str, lang: &str) -> String {
    let key = match kind {
        "now" => "nowBadge",
        "next" => "nextBadge",
        _ => "tomorrowBadge",
    };
    let fixed = if kind == "tomorrow" { " data-fixed" } else { "" };
    format!("<span class=\"now-badge now-badge--{kind}\"{fixed}>{}</span>", esc(t(key, lang)))
}

/// 「06:30–08:15」, 「06:30–」, 「–08:15」 from the session's own fields; "" without either.
fn time_text(sess: &Session) -> String {
    let (a, b) = (sess.time_start.trim(), sess.time_end.trim());
    if a.is_empty() && b.is_empty() {
        String::new()
    } else {
        format!("{a}\u{2013}{b}")
    }
}

/// The session's main meal (first) + backup (second) in the short 「料理｜店名」
/// form; the main links to its map pin when it has one.
fn meal_line(sess: &Session, lang: &str) -> String {
    let Some(main) = sess.meals.first() else {
        return String::new();
    };
    let short = super::session::meal_short(&main.text);
    let main_html = match super::session::split_meal_pin(&main.text) {
        Some((_, query, _)) if !query.is_empty() => format!(
            "<a href=\"{}\" target=\"_blank\" rel=\"noopener\">{} 🗺️</a>",
            esc_url_attr(&format!("https://www.google.com/maps/search/{}", urlencode(&query))),
            esc(&short)
        ),
        _ => esc(&short),
    };
    let mut h = format!(
        "<div class=\"now-meal\">🍽️ <span class=\"now-meal-k\">{}</span> {main_html}",
        esc(t("mealMain", lang))
    );
    if let Some(backup) = sess.meals.get(1) {
        h.push_str(&format!(
            " <span class=\"now-meal-k\">{}</span> {}",
            esc(t("mealBackup", lang)),
            esc(&super::session::meal_short(&backup.text))
        ));
    }
    h.push_str("</div>");
    h
}

/// One timeline entry's body: focus line (the activities when there is no
/// focus), timed legs, meals.
fn session_body(sess: &Session, legs: &[(&RouteSegment, i64, bool)], lang: &str) -> String {
    let mut h = String::new();
    if !sess.focus_zh.is_empty() {
        h.push_str(&format!("<div class=\"now-focus\">{}</div>", esc(&sess.focus_zh)));
    } else {
        for a in &sess.activities {
            h.push_str(&format!("<div class=\"now-focus\">{}</div>", super::render_activity_text(&a.title)));
        }
    }
    if !legs.is_empty() {
        h.push_str("<ul class=\"now-legs\">");
        for (seg, at, is_next) in legs {
            let next = if *is_next { badge("next", lang) } else { String::new() };
            h.push_str(&format!(
                "<li class=\"now-leg\" data-at=\"{at}\">{next}{}</li>",
                super::day::route_seg_inner(seg, lang)
            ));
        }
        h.push_str("</ul>");
    }
    h.push_str(&meal_line(sess, lang));
    h
}

/// The 現在・接下來 card for `today` at `clock` (see the module doc). Its
/// `data-now-*` attributes carry what the inline script needs to re-mark.
pub fn now_card(plan: &Plan, today: &Day, clock: &Clock, lang: &str, token: Option<&str>) -> String {
    let now = clock.minutes;
    let slots = slots(today);
    let windows: Vec<(i64, i64)> = slots.iter().map(|s| (s.start, s.end)).collect();
    let marks = mark(&windows, now);

    // Timed legs per slot, in leg order; 接下來 is the EARLIEST-starting leg at
    // or after now (first in route order on a tie) — the same rule the inline
    // script's minute re-marking applies, whatever order the slots list legs in.
    let next_at = today
        .route_segments
        .iter()
        .filter_map(|seg| parse_hhmm(&seg.start_time))
        .filter(|at| *at >= now)
        .min();
    let mut per_slot: Vec<Vec<(&RouteSegment, i64, bool)>> = vec![Vec::new(); slots.len()];
    let mut next_leg_marked = false;
    for seg in &today.route_segments {
        let Some(at) = parse_hhmm(&seg.start_time) else { continue };
        let Some(i) = leg_slot(&windows, at) else { continue };
        let is_next = !next_leg_marked && Some(at) == next_at;
        next_leg_marked |= is_next;
        per_slot[i].push((seg, at, is_next));
    }

    let date = super::summary::date_with_weekday(&today.date, lang).unwrap_or_else(|| today.date.clone());
    let pinned = if clock.pinned { " data-now-pinned" } else { "" };
    let mut h = format!(
        "<section class=\"now-card\" id=\"{BLOCK_NOW}\" data-now-date=\"{}\" data-now-min=\"{now}\"{pinned} \
         data-l-now=\"{}\" data-l-next=\"{}\"><details data-block=\"{BLOCK_NOW}\" open><summary><h2>{}</h2></summary>\
         <div class=\"now-body\">",
        esc(&today.date),
        esc(t("nowBadge", lang)),
        esc(t("nextBadge", lang)),
        esc(&tf("nowHeading", lang, &[("date", &date)])),
    );
    let theme = super::day::day_theme(today, lang);
    if !theme.is_empty() {
        h.push_str(&format!("<div class=\"now-theme\">{}</div>", esc(theme)));
    }
    if !today.variants.is_empty() {
        h.push_str("<div class=\"now-variants\">");
        for v in &today.variants {
            h.push_str(&format!(
                "<a class=\"now-variant\" href=\"#{}\">{}</a>",
                super::day::variant_block_id(v.day_number),
                esc(&tf("dayVariant", lang, &[("label", super::day_option_name(v, lang))]))
            ));
        }
        h.push_str("</div>");
    }
    h.push_str(&super::summary::day_documents(&plan.documents, today.day_number, lang, token));

    h.push_str("<ol class=\"now-timeline\">");
    for (i, slot) in slots.iter().enumerate() {
        let mut class = format!("now-item now-s-{}", esc(&slot.sess.session_type));
        let mut b = String::new();
        if marks.current == Some(i) {
            class.push_str(" is-now");
            b = badge("now", lang);
        } else if marks.next == Some(i) {
            class.push_str(" is-next");
            b = badge("next", lang);
        }
        if slot.end <= now {
            class.push_str(" is-past");
        }
        let time = time_text(slot.sess);
        let time = if time.is_empty() {
            String::new()
        } else {
            format!("<span class=\"now-time\">{}</span>", esc(&time))
        };
        h.push_str(&format!(
            "<li class=\"{class}\" data-start=\"{}\" data-end=\"{}\"><div class=\"now-head\">\
             <span class=\"now-name\">{}</span>{time}{b}</div>{}</li>",
            slot.start,
            slot.end,
            esc(super::session::session_label(&slot.sess.session_type, lang)),
            session_body(slot.sess, &per_slot[i], lang),
        ));
    }
    // 明天: tomorrow's first session, shown once today is done (the script
    // un-hides it when the day ends while the page is open).
    if let Some(tomorrow) = next_main_day(&plan.days, today) {
        if let Some(first) = slots_first(tomorrow) {
            let hidden = if marks.done { "" } else { " hidden" };
            let tdate = super::summary::date_with_weekday(&tomorrow.date, lang).unwrap_or_else(|| tomorrow.date.clone());
            let name = tf(
                "dateOption",
                lang,
                &[("date", &tdate), ("label", super::session::session_label(&first.session_type, lang))],
            );
            let time = time_text(first);
            let time = if time.is_empty() {
                String::new()
            } else {
                format!("<span class=\"now-time\">{}</span>", esc(&time))
            };
            h.push_str(&format!(
                "<li class=\"now-item now-tomorrow\"{hidden}><div class=\"now-head\">{}<span class=\"now-name\">{}</span>{time}</div>{}\
                 <a class=\"now-more\" href=\"#{}\">{}</a></li>",
                badge("tomorrow", lang),
                esc(&name),
                session_body(first, &[], lang),
                super::day::day_block_id(tomorrow.day_number),
                esc(&tf("nowOpenDay", lang, &[("day", &tomorrow.day_number.to_string())])),
            ));
        }
    }
    h.push_str("</ol>");
    if !today.route_segments.is_empty() {
        h.push_str(&format!(
            "<details class=\"now-route\"><summary>{}</summary>{}</details>",
            esc(t("nowFullRoute", lang)),
            super::day::render_route_block(&today.route_segments, lang, None)
        ));
    }
    h.push_str(&format!(
        "<a class=\"now-more\" href=\"#{}\">{}</a>",
        super::day::day_block_id(today.day_number),
        esc(&tf("nowOpenDay", lang, &[("day", &today.day_number.to_string())]))
    ));
    h.push_str("</div></details></section>");
    h
}

/// A day's first shown session.
fn slots_first(day: &Day) -> Option<&Session> {
    day.sessions.iter().find(|s| super::session::has_content(s))
}

/// `T2` for a bare terminal number, the stored text otherwise ("" when unset).
fn terminal(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        String::new()
    } else if raw.bytes().all(|c| c.is_ascii_digit()) {
        format!("T{raw}")
    } else {
        raw.to_string()
    }
}

/// One flight leg on one line: 「MM027 11/16（一）15:25 KIX T2 → 17:55 TPE T1」
/// (airport name when stored, else its code; terminal only when stored).
fn flight_line(f: &crate::turso::Row, lang: &str) -> String {
    let rs = |k: &str| super::summary::rs(f, k).trim().to_string();
    let place = |name: &str, code: &str, term: &str| {
        let name = rs(name);
        let base = if name.is_empty() { rs(code) } else { name };
        [base, terminal(&rs(term))].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
    };
    let date = rs("flight_date");
    let date = super::summary::date_with_weekday(&date, lang).unwrap_or(date);
    let dep = [rs("departure_time"), place("departure_airport", "departure_code", "departure_terminal")]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let arr = [rs("arrival_time"), place("arrival_airport", "arrival_code", "arrival_terminal")]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let route = match (dep.is_empty(), arr.is_empty()) {
        (true, true) => String::new(),
        _ => format!("{dep} → {arr}").trim().to_string(),
    };
    [rs("flight_number"), date, route]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `tel:` target: the digits (and a leading +) of a stored phone; None without digits.
fn tel_href(phone: &str) -> Option<String> {
    let mut out = String::new();
    for (i, c) in phone.trim().chars().enumerate() {
        if c.is_ascii_digit() || (c == '+' && i == 0) {
            out.push(c);
        }
    }
    out.chars().any(|c| c.is_ascii_digit()).then(|| format!("tel:{out}"))
}

/// The stay line: 「11/12（四）入住 · 4 晚 · 11/16（一）退房」 — nights and
/// check-out from the plan's end date when it is after the check-in; else the
/// check-in alone. "" without a parseable check-in.
fn stay_line(check_in: &str, plan: &Plan, lang: &str) -> String {
    let Some((y, m, d)) = super::summary::parse_ymd(check_in) else {
        return String::new();
    };
    let Some(in_text) = super::summary::date_with_weekday(check_in, lang) else {
        return String::new();
    };
    let nights = super::summary::parse_ymd(&plan.end_date)
        .map(|(ey, em, ed)| super::summary::civil_days(ey, em, ed) - super::summary::civil_days(y, m, d))
        .filter(|n| *n > 0);
    match (nights, super::summary::date_with_weekday(&plan.end_date, lang)) {
        (Some(n), Some(out)) => tf(
            if n == 1 { "qiStayOneNight" } else { "qiStayNights" },
            lang,
            &[("in", &in_text), ("n", &n.to_string()), ("out", &out)],
        ),
        _ => tf("qiCheckIn", lang, &[("in", &in_text)]),
    }
}

/// The 常用資訊 card: flight legs (去程 / 回程), then the booked hotel's name,
/// address, phone (`tel:` link) and stay. Each row is a search unit; a row
/// without data is omitted, and so is the whole card when nothing is left.
/// Address / phone carry the 「填入境卡／海關申報用」 caption.
pub fn quick_info(plan: &Plan, lang: &str) -> String {
    let title = t("quickInfo", lang);
    let mut rows: Vec<(String, String)> = Vec::new();
    for f in &plan.flights {
        let line = flight_line(f, lang);
        if line.is_empty() {
            continue;
        }
        let key = if super::summary::rs(f, "direction") == "return" { "qiReturn" } else { "qiOutbound" };
        rows.push((t(key, lang).to_string(), esc(&line)));
    }
    if let Some(hotel) = &plan.hotel {
        let rs = |k: &str| super::summary::rs(hotel, k).trim().to_string();
        let name = super::summary::hotel_display_name(hotel, lang);
        if !name.trim().is_empty() {
            rows.push((t("hotel", lang).to_string(), esc(name.trim())));
        }
        // The form caption once, on the first of address / phone.
        let mut hint = format!("<span class=\"qi-hint\">{}</span>", esc(t("qiFormHint", lang)));
        let address = rs("address");
        if !address.is_empty() {
            rows.push((
                t("qiAddress", lang).to_string(),
                format!("<span class=\"qi-copy\">{}</span>{}", esc(&address), std::mem::take(&mut hint)),
            ));
        }
        let phone = rs("phone");
        if !phone.is_empty() {
            let shown = match tel_href(&phone) {
                Some(href) => format!("<a class=\"qi-copy\" href=\"{}\">{}</a>", esc_url_attr(&href), esc(&phone)),
                None => format!("<span class=\"qi-copy\">{}</span>", esc(&phone)),
            };
            rows.push((t("qiPhone", lang).to_string(), format!("{shown}{hint}")));
        }
        let stay = stay_line(&rs("check_in"), plan, lang);
        if !stay.is_empty() {
            rows.push((t("qiStay", lang).to_string(), esc(&stay)));
        }
    }
    if rows.is_empty() {
        return String::new();
    }
    let mut h = format!(
        "<section class=\"quick-info\" id=\"{BLOCK_QUICK}\"><details data-block=\"{BLOCK_QUICK}\" open>\
         <summary><h2>{}</h2></summary><dl class=\"qi-list\">",
        esc(title)
    );
    for (label, value) in rows {
        h.push_str(&format!(
            "<div class=\"qi-row\"{}><dt>{}</dt><dd>{value}</dd></div>",
            super::day::search_unit_attrs(Some(&format!("{title} · {label}"))),
            esc(&label),
        ));
    }
    h.push_str("</dl></details></section>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Meal, PlanDocument};
    use serde_json::json;

    fn sess(kind: &str, start: &str, end: &str, focus: &str) -> Session {
        Session {
            session_type: kind.into(),
            time_start: start.into(),
            time_end: end.into(),
            focus_zh: focus.into(),
            ..Default::default()
        }
    }
    fn leg(from: &str, to: &str, at: &str) -> RouteSegment {
        RouteSegment {
            from_place: from.into(),
            to_place: to.into(),
            mode: "walking".into(),
            duration_min: 7,
            start_time: at.into(),
            source: "confirmed".into(),
            ..Default::default()
        }
    }
    fn clock(date: &str, hhmm: &str) -> Clock {
        parse_at(&format!("{date}T{hhmm}")).expect("valid at")
    }
    /// Departure-day shape (kyoto-nov day 5): an empty afternoon and an
    /// evening with no times.
    fn last_day() -> Day {
        let mut morning = sess("morning", "06:30", "08:15", "拉麵早餐後退房");
        morning.meals = vec![
            Meal { text: "主選：京都醬油拉麵｜本家 第一旭：京都站東側｜map:本家 第一旭".into(), source: "confirmed".into() },
            Meal { text: "備選：京都黑醬油拉麵｜新福菜館 本店：第一旭隔壁".into(), source: "confirmed".into() },
        ];
        Day {
            day_number: 5,
            date: "2026-11-16".into(),
            theme_zh: "返程".into(),
            sessions: vec![
                morning,
                sess("noon", "10:15", "12:55", "11:00搭HARUKA"),
                sess("afternoon", "", "", ""),
                sess("evening", "", "", "樂桃MM027 17:55抵達桃園；取車回家"),
            ],
            route_segments: vec![
                leg("飯店", "本家 第一旭", "08:15"),
                leg("本家 第一旭", "飯店", ""),
                leg("飯店", "京都車站", "10:00"),
                leg("桃園第一航廈", "停車場", "18:45"),
            ],
            ..Default::default()
        }
    }
    fn first_day() -> Day {
        Day {
            day_number: 4,
            date: "2026-11-15".into(),
            sessions: vec![
                sess("morning", "08:00", "11:15", "東福寺通天橋"),
                sess("evening", "17:00", "20:00", "伊勢丹11樓晚餐"),
            ],
            variants: vec![Day {
                day_number: 6,
                date: "2026-11-15".into(),
                variant_of: Some(4),
                variant_label_zh: "晴天行程".into(),
                variant_label: "Sunny-day plan".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }
    fn trip() -> Plan {
        Plan {
            plan_id: "kyoto-nov-2026".into(),
            start_date: "2026-11-12".into(),
            end_date: "2026-11-16".into(),
            days: vec![first_day(), last_day()],
            documents: vec![PlanDocument {
                doc_key: "flight".into(),
                kind: "flight_eticket".into(),
                label_zh: "樂桃電子機票".into(),
                r2_key: "kyoto-nov-2026/doc-flight.pdf".into(),
                days: vec![5],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn jst_now_is_utc_plus_nine_across_midnight() {
        let midnight_utc = crate::render::summary::civil_days(2026, 11, 12) * 86_400_000;
        let c = jst_from_ms(midnight_utc + 30 * 60_000);
        assert_eq!(c, Clock { date: "2026-11-12".into(), minutes: 9 * 60 + 30, pinned: false });
        // 15:30 UTC is 00:30 the next JST day.
        let c = jst_from_ms(midnight_utc + (15 * 60 + 30) * 60_000);
        assert_eq!((c.date.as_str(), c.minutes), ("2026-11-13", 30));
    }

    #[test]
    fn at_override_is_strict() {
        assert_eq!(
            parse_at("2026-11-16T19:05"),
            Some(Clock { date: "2026-11-16".into(), minutes: 19 * 60 + 5, pinned: true })
        );
        assert!(parse_at("2028-02-29T00:00").is_some());
        for bad in [
            "", "x", "2026-11-16", "2026-11-16 19:05", "2026-11-16T19:05Z", "2026-11-16T9:05",
            "2026-02-29T10:00", "2026-04-31T10:00", "2026-13-01T00:00", "2026-00-10T00:00",
            "2026-11-16T24:00", "2026-11-16T23:60", "2026-11-16T-1:00", "+026-11-16T10:00",
            "２026-11-16T10:00",
        ] {
            assert_eq!(parse_at(bad), None, "{bad}");
        }
        assert_eq!(parse_hhmm("07:50"), Some(470));
        assert_eq!(parse_hhmm(" 8:00 "), Some(480));
        assert_eq!(parse_hhmm("24:00"), Some(1440));
        for bad in ["", "24:01", "7:5", "07:60", "0750", "a7:50", "107:50"] {
            assert_eq!(parse_hhmm(bad), None, "{bad}");
        }
    }

    #[test]
    fn trip_window_is_a_main_day_date() {
        let plan = trip();
        assert_eq!(trip_today(&plan.days, &clock("2026-11-15", "10:00")).map(|d| d.day_number), Some(4));
        assert_eq!(trip_today(&plan.days, &clock("2026-11-16", "23:59")).map(|d| d.day_number), Some(5));
        // A date between the days of a plan whose days skip it, before or after → none.
        assert!(trip_today(&plan.days, &clock("2026-11-14", "10:00")).is_none());
        assert!(trip_today(&plan.days, &clock("2026-11-17", "00:00")).is_none());
        // A variant alone never anchors the window.
        let variant_only = vec![Day { day_number: 6, date: "2026-11-20".into(), variant_of: Some(4), ..Default::default() }];
        assert!(trip_today(&variant_only, &clock("2026-11-20", "10:00")).is_none());
    }

    #[test]
    fn slots_infer_missing_times_and_skip_empty_sessions() {
        let day = last_day();
        let w: Vec<(i64, i64)> = slots(&day).iter().map(|s| (s.start, s.end)).collect();
        // The empty afternoon is skipped; the timeless evening runs from noon's end to midnight.
        assert_eq!(w, [(390, 495), (615, 775), (775, 1440)]);
        // A timeless flight 「中午」 spans morning end → the next explicit start.
        let arrival = Day {
            sessions: vec![
                sess("morning", "06:00", "07:40", "出發"),
                sess("noon", "", "", "樂桃MM024"),
                sess("afternoon", "13:10", "16:30", "入住"),
                sess("evening", "16:45", "20:30", "夜遊"),
            ],
            ..Default::default()
        };
        let w: Vec<(i64, i64)> = slots(&arrival).iter().map(|s| (s.start, s.end)).collect();
        assert_eq!(w, [(360, 460), (460, 790), (790, 990), (1005, 1230)]);
    }

    #[test]
    fn marks_before_between_during_and_after_sessions() {
        let w = [(360, 460), (460, 790), (790, 990), (1005, 1230)];
        assert_eq!(mark(&w, 300), Marks { current: None, next: Some(0), done: false });
        assert_eq!(mark(&w, 400), Marks { current: Some(0), next: Some(1), done: false });
        // In the gap between afternoon and evening: nothing current, evening next.
        assert_eq!(mark(&w, 995), Marks { current: None, next: Some(3), done: false });
        assert_eq!(mark(&w, 1229), Marks { current: Some(3), next: None, done: false });
        assert_eq!(mark(&w, 1230), Marks { current: None, next: None, done: true });
        // Overlap: the later session wins once it has begun.
        assert_eq!(mark(&[(470, 630), (620, 735)], 625).current, Some(1));
        assert_eq!(mark(&[], 600), Marks { current: None, next: None, done: true });
        // Legs: in a window, in a gap (→ the next session), after everything (→ last).
        let w = [(390, 495), (615, 775), (775, 1440)];
        assert_eq!(leg_slot(&w, 495), Some(1));
        assert_eq!(leg_slot(&w, 600), Some(1));
        assert_eq!(leg_slot(&w, 1125), Some(2));
        assert_eq!(leg_slot(&[(390, 495)], 600), Some(0));
        assert_eq!(leg_slot(&[], 600), None);
    }

    #[test]
    fn now_card_marks_current_and_next_with_legs_meals_and_documents() {
        let plan = trip();
        let today = &plan.days[1];
        let html = now_card(&plan, today, &clock("2026-11-16", "07:00"), "zh", Some("tok"));
        assert!(html.starts_with(
            "<section class=\"now-card\" id=\"sec-now\" data-now-date=\"2026-11-16\" data-now-min=\"420\" data-now-pinned \
             data-l-now=\"現在\" data-l-next=\"接下來\"><details data-block=\"sec-now\" open><summary><h2>現在・接下來 · 11/16（一）</h2></summary>"
        ), "{html}");
        assert!(html.contains(
            "<li class=\"now-item now-s-morning is-now\" data-start=\"390\" data-end=\"495\"><div class=\"now-head\">\
             <span class=\"now-name\">上午</span><span class=\"now-time\">06:30\u{2013}08:15</span>\
             <span class=\"now-badge now-badge--now\">現在</span></div>"
        ), "{html}");
        assert!(html.contains("<li class=\"now-item now-s-noon is-next\" data-start=\"615\" data-end=\"775\">"), "{html}");
        assert!(html.contains("<span class=\"now-badge now-badge--next\">接下來</span>"), "{html}");
        // The first timed leg at/after now is 接下來; the untimed leg is only in the full route.
        assert!(html.contains("<li class=\"now-leg\" data-at=\"495\"><span class=\"now-badge now-badge--next\">接下來</span><span class=\"route-mode\">🚶</span> 飯店 → 本家 第一旭 · 08:15"), "{html}");
        assert!(html.contains("<li class=\"now-leg\" data-at=\"600\"><span class=\"route-mode\">"), "{html}");
        assert_eq!(html.matches("本家 第一旭 → 飯店").count(), 1, "{html}");
        assert!(html.contains("<details class=\"now-route\"><summary>今日完整路線</summary>"), "{html}");
        // The now card's route copy is never a search unit (no duplicate hits).
        assert!(!html.contains("data-search-unit"), "{html}");
        // Main meal (map link) + backup, short forms.
        assert!(html.contains(
            "<div class=\"now-meal\">🍽️ <span class=\"now-meal-k\">主選</span> <a href=\"https://www.google.com/maps/search/%E6%9C%AC%E5%AE%B6%20%E7%AC%AC%E4%B8%80%E6%97%AD\" \
             target=\"_blank\" rel=\"noopener\">京都醬油拉麵｜本家 第一旭 🗺️</a> <span class=\"now-meal-k\">備選</span> 京都黑醬油拉麵｜新福菜館 本店</div>"
        ), "{html}");
        // Today's documents, same voucher links as the day card.
        assert!(html.contains("<div class=\"day-documents\"><a class=\"doc-link\" data-doc=\"flight\" href=\"/voucher/kyoto-nov-2026/doc-flight.pdf?token=tok\""), "{html}");
        // The last day has no 明天.
        assert!(!html.contains("now-tomorrow"), "{html}");
        assert!(html.contains("<a class=\"now-more\" href=\"#day-5\">第 5 天完整內容 ↓</a>"), "{html}");
        // An unpinned clock lets the script re-mark.
        let live = now_card(&plan, today, &jst_from_ms(0), "zh", None);
        assert!(!live.contains("data-now-pinned"), "{live}");
    }

    #[test]
    fn evening_without_times_is_current_at_the_parking_lot() {
        let plan = trip();
        let html = now_card(&plan, &plan.days[1], &clock("2026-11-16", "19:00"), "zh", None);
        assert!(html.contains("<li class=\"now-item now-s-evening is-now\" data-start=\"775\" data-end=\"1440\">"), "{html}");
        assert!(html.contains("<li class=\"now-item now-s-morning is-past\""), "{html}");
        assert!(html.contains("樂桃MM027 17:55抵達桃園"), "{html}");
        // The 18:45 leg is past (in the evening slot), no leg left → no 接下來 at all.
        assert!(!html.contains("now-badge--next"), "{html}");
    }

    #[test]
    fn tomorrow_shows_after_the_last_session_and_variants_link_out() {
        let plan = trip();
        let today = &plan.days[0];
        let html = now_card(&plan, today, &clock("2026-11-15", "21:00"), "zh", None);
        assert!(html.contains(
            "<li class=\"now-item now-tomorrow\"><div class=\"now-head\"><span class=\"now-badge now-badge--tomorrow\" data-fixed>明天</span>\
             <span class=\"now-name\">11/16（一）· 上午</span><span class=\"now-time\">06:30\u{2013}08:15</span></div>\
             <div class=\"now-focus\">拉麵早餐後退房</div>"
        ), "{html}");
        assert!(html.contains("<a class=\"now-more\" href=\"#day-5\">第 5 天完整內容 ↓</a></li>"), "{html}");
        assert!(!html.contains("is-now"), "{html}");
        // Mid-day: tomorrow is in the DOM but hidden (the script un-hides it).
        let html = now_card(&plan, today, &clock("2026-11-15", "12:00"), "zh", None);
        assert!(html.contains("<li class=\"now-item now-tomorrow\" hidden>"), "{html}");
        assert!(html.contains("<li class=\"now-item now-s-evening is-next\""), "{html}");
        // The day's variant is one tap away.
        assert!(html.contains("<div class=\"now-variants\"><a class=\"now-variant\" href=\"#variant-6\">替代方案：晴天行程</a></div>"), "{html}");
        let en = now_card(&plan, today, &clock("2026-11-15", "12:00"), "en", None);
        assert!(en.contains("<h2>Now &amp; next · Sun 11/15</h2>") && en.contains("Alternative: Sunny-day plan"), "{en}");
    }

    fn flight(direction: &str, number: &str, date: &str, dep: (&str, &str, &str), arr: (&str, &str, &str)) -> crate::turso::Row {
        [
            ("direction", direction),
            ("flight_number", number),
            ("flight_date", date),
            ("departure_code", dep.0),
            ("departure_terminal", dep.1),
            ("departure_time", dep.2),
            ("arrival_code", arr.0),
            ("arrival_terminal", arr.1),
            ("arrival_time", arr.2),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect()
    }
    fn hotel(fields: &[(&str, &str)]) -> crate::turso::Row {
        fields.iter().map(|(k, v)| (k.to_string(), json!(v))).collect()
    }

    #[test]
    fn quick_info_lists_flights_and_hotel_contact() {
        let mut plan = trip();
        plan.flights = vec![
            flight("outbound", "MM024", "2026-11-12", ("TPE", "1", "09:40"), ("KIX", "2", "13:10")),
            flight("return", "MM027", "2026-11-16", ("KIX", "", "15:25"), ("TPE", "Terminal 1", "17:55")),
        ];
        plan.hotel = Some(hotel(&[
            ("name", "APA Hotel Kyoto Ekihigashi"),
            ("check_in", "2026-11-12"),
            ("address", "〒600-8216 京都市下京区東塩小路町552-1"),
            ("phone", "+81 75-123-4567"),
        ]));
        let html = quick_info(&plan, "zh");
        assert!(html.starts_with("<section class=\"quick-info\" id=\"sec-quick\"><details data-block=\"sec-quick\" open><summary><h2>常用資訊</h2></summary><dl class=\"qi-list\">"), "{html}");
        assert!(html.contains(
            "<div class=\"qi-row\" data-search-unit data-search-label=\"常用資訊 · 去程\"><dt>去程</dt><dd>MM024 11/12（四） 09:40 TPE T1 → 13:10 KIX T2</dd></div>"
        ), "{html}");
        assert!(html.contains("<dt>回程</dt><dd>MM027 11/16（一） 15:25 KIX → 17:55 TPE Terminal 1</dd>"), "{html}");
        assert!(html.contains("<dt>住宿</dt><dd>APA Hotel Kyoto Ekihigashi</dd>"), "{html}");
        assert!(html.contains(
            "<dt>地址</dt><dd><span class=\"qi-copy\">〒600-8216 京都市下京区東塩小路町552-1</span><span class=\"qi-hint\">填入境卡／海關申報用</span></dd>"
        ), "{html}");
        // The caption once; the phone is a tel: link of its digits.
        assert_eq!(html.matches("qi-hint").count(), 1, "{html}");
        assert!(html.contains("<dt>電話</dt><dd><a class=\"qi-copy\" href=\"tel:+81751234567\">+81 75-123-4567</a></dd>"), "{html}");
        assert!(html.contains("<dt>入住</dt><dd>11/12（四）入住 · 4 晚 · 11/16（一）退房</dd>"), "{html}");

        let en = quick_info(&plan, "en");
        assert!(en.contains("<dt>Return</dt><dd>MM027 Mon 11/16 15:25 KIX → 17:55 TPE Terminal 1</dd>"), "{en}");
        assert!(en.contains("<dt>Stay</dt><dd>Check-in Thu 11/12 · 4 nights · check-out Mon 11/16</dd>"), "{en}");
        assert!(en.contains("for the arrival card / customs form"), "{en}");
    }

    #[test]
    fn quick_info_omits_missing_rows_and_the_empty_card() {
        let mut plan = trip();
        // A hotel row from a worker deployed before the migration: no address/phone keys.
        plan.hotel = Some(hotel(&[("name", "APA"), ("check_in", "2026-11-12")]));
        let html = quick_info(&plan, "zh");
        assert!(!html.contains("<dt>地址</dt>") && !html.contains("<dt>電話</dt>") && !html.contains("qi-hint"), "{html}");
        assert!(!html.contains("<dt>去程</dt>"), "{html}");
        assert!(html.contains("<dt>住宿</dt><dd>APA</dd>"), "{html}");
        // Phone only: the caption moves to the phone; no digits → plain text, no tel: link.
        plan.hotel = Some(hotel(&[("phone", "ask the front desk")]));
        let html = quick_info(&plan, "zh");
        assert!(html.contains("<dt>電話</dt><dd><span class=\"qi-copy\">ask the front desk</span><span class=\"qi-hint\">"), "{html}");
        assert!(!html.contains("tel:"), "{html}");
        // Check-in not before the plan end → check-in alone.
        plan.hotel = Some(hotel(&[("check_in", "2026-11-16")]));
        assert!(quick_info(&plan, "zh").contains("<dt>入住</dt><dd>11/16（一）入住</dd>"));
        // Nothing at all → no card.
        plan.hotel = None;
        assert_eq!(quick_info(&plan, "zh"), "");
        plan.hotel = Some(hotel(&[("name", " ")]));
        assert_eq!(quick_info(&plan, "zh"), "");
    }
}
