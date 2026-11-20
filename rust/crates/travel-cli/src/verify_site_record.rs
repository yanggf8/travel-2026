//! Layer `record` of `travel verify-site` (audit H1, 2026-11-18): the plan's
//! trip-record rows against what the LIVE viewer page actually renders.
//!
//! Records only render on PAST sessions (worker session.rs `session_is_past`),
//! and the default verify-site pin is the day BEFORE the trip — so the layer-1
//! page fetch never shows them. This layer reads the plan's record rows once
//! through `travel-db` (`repo::records::list_plan_records`), then — only when
//! any exist — fetches the viewer page a second time through this run's verify
//! token, in both languages, with `?at=` pinned to NOON THE DAY AFTER the last
//! dated main day: every dated session is past, and the page is outside the
//! trip window (no 現在・接下來 card, every block served open). A plan with no
//! record rows emits one PASS and fetches nothing.
//!
//! Matching is by the stable identifiers the worker emits: the session anchor
//! `#day-<n>-<session>` (render/session.rs `session_anchor`) carries
//! `record_summary_zh` in its `.session-focus`; `div.activity[data-activity-id]`
//! (activities.id), `.meal[data-meal-order]` (session_meals.sort_order) and
//! `div.route-seg[data-seg-order]` (day_route_segments.sort_order) carry the
//! badges and notes. Badge words are worker i18n chrome, so the checks match by
//! class (`record-badge--actual`, `record-badge`, `record-note`,
//! `record-skipped`) and by the DB's own text — never by localized copy. The
//! badge/note spans after a MEAL element are its siblings (render/session.rs
//! `render_past_meal`), the ones of an activity are inside its div — the
//! document-order pairing below reads both without caring.

use std::collections::HashMap;
use std::path::Path;

use libsql::Connection;
use scraper::{ElementRef, Html, Selector};
use travel_db::repo::records::{ActivityNote, PlanRecords};

use crate::verify_site::{http_get, redact, Check, Next, Tally};

/// The tally layer name of every verdict this module emits.
const LAYER: &str = "record";

fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap_or_else(|e| panic!("static selector {s:?} does not parse: {e:?}"))
}

fn text_of(el: ElementRef) -> String {
    el.text().collect::<String>()
}

fn has_class(el: ElementRef, class: &str) -> bool {
    el.value().classes().any(|c| c == class)
}

/// What one badge/note-bearing row shows on the page.
#[derive(Default, Debug, PartialEq)]
struct RowShown {
    /// `span.record-badge--actual` (the 實際 badge of a done row).
    actual: bool,
    /// `span.record-badge` WITHOUT `--actual` (the 計畫・未紀錄 badge).
    unrecorded: bool,
    /// `span.record-note` text.
    note: Option<String>,
    /// Text of the activity's `div.act-notes` block (worker
    /// render/session.rs `activity_extras`); None when it has none.
    notes_text: Option<String>,
}

/// One session block as the page renders it (worker render/session.rs
/// `render_unit`).
#[derive(Default, Debug)]
struct SessionShown {
    /// `.session-focus` text ("" when the block has none).
    focus: String,
    /// Text of each `.record-skipped` div (the per-session 未前往 line).
    skipped: Vec<String>,
    activities: HashMap<String, RowShown>,
    meals: HashMap<i64, RowShown>,
}

/// One `div.route-seg` of a day card (worker render/day.rs
/// `render_route_block_at`); a done leg's badge is a plain `record-badge`
/// span inside the seg and its note is plain seg text.
#[derive(Default, Debug)]
struct LegShown {
    done: bool,
    /// Text of the leg's own `.record-note` element (worker render/day.rs
    /// wraps the record note in one) — never the planned notes text.
    note: Option<String>,
}

/// Everything the record layer reads from one page.
#[derive(Default, Debug)]
pub(crate) struct RecordPage {
    sessions: HashMap<String, SessionShown>,
    /// (day, sort_order) → the day card's route-seg.
    legs: HashMap<(i64, i64), LegShown>,
}

/// Which row a following badge/note span belongs to (document-order pairing).
#[derive(Clone, Debug)]
enum RowKey {
    Activity(String),
    Meal(i64),
}

/// The anchor id the worker gives a day's session block (worker session.rs
/// `session_anchor`): `day-<n>-<session_type>`.
fn anchor(day: i64, session: &str) -> String {
    format!("day-{day}-{session}")
}

pub(crate) fn parse_record_page(html: &str) -> RecordPage {
    let doc = Html::parse_document(html);
    let mut out = RecordPage::default();
    let Some(body) = doc.select(&sel("body")).next() else {
        return out;
    };
    for sess in body.select(&sel("div.session[id]")) {
        let mut shown = SessionShown {
            focus: sess.select(&sel(".session-focus")).next().map(text_of).unwrap_or_default(),
            ..SessionShown::default()
        };
        for line in sess.select(&sel(".record-skipped")) {
            shown.skipped.push(text_of(line));
        }
        // Meal/activity elements and the badge/note spans that follow them, in
        // DOCUMENT order: an activity's spans are inside its div, a meal's are
        // siblings after the element (meal-alt and AI badges sit between and are
        // simply not selected, so the pairing survives them).
        let mut current: Option<RowKey> = None;
        for el in sess.select(&sel(
            ".activity[data-activity-id], .meal[data-meal-order], .record-badge, .record-note, .act-notes",
        )) {
            if has_class(el, "act-notes") {
                if let Some(RowKey::Activity(id)) = &current {
                    if let Some(row) = shown.activities.get_mut(id) {
                        row.notes_text = Some(text_of(el));
                    }
                }
                continue;
            }
            if el.value().name() != "span" {
                current = if has_class(el, "activity") {
                    let id = el.value().attr("data-activity-id").unwrap_or_default().to_string();
                    shown.activities.insert(id.clone(), RowShown::default());
                    Some(RowKey::Activity(id))
                } else {
                    let order: i64 =
                        el.value().attr("data-meal-order").and_then(|o| o.parse().ok()).unwrap_or(-1);
                    shown.meals.insert(order, RowShown::default());
                    Some(RowKey::Meal(order))
                };
                continue;
            }
            let Some(key) = current.clone() else { continue };
            let Some(row) = (match &key {
                RowKey::Activity(id) => shown.activities.get_mut(id),
                RowKey::Meal(order) => shown.meals.get_mut(order),
            }) else {
                continue;
            };
            if has_class(el, "record-note") {
                row.note = Some(text_of(el));
            } else if has_class(el, "record-badge--actual") {
                row.actual = true;
            } else if has_class(el, "record-badge") {
                row.unrecorded = true;
            }
        }
        out.sessions
            .insert(sess.value().attr("id").unwrap_or_default().to_string(), shown);
    }
    for day_card in body.select(&sel("section.day[data-day]")) {
        let day: i64 = day_card
            .value()
            .attr("data-day")
            .and_then(|d| d.parse().ok())
            .unwrap_or(-1);
        for seg in day_card.select(&sel(".route-seg[data-seg-order]")) {
            // A variant's route block is NESTED inside its main day's card
            // (worker render/day.rs): its segs belong to the variant's own
            // day, never to `day` — left in, a variant seg with the same
            // sort_order would overwrite the main day's leg (records can
            // never reference a variant: record.rs `require_main_day`).
            if in_variant_subtree(seg) {
                continue;
            }
            let order: i64 = seg
                .value()
                .attr("data-seg-order")
                .and_then(|o| o.parse().ok())
                .unwrap_or(-1);
            out.legs.insert(
                (day, order),
                LegShown {
                    done: seg.select(&sel(".record-badge")).next().is_some(),
                    note: seg.select(&sel(".record-note")).next().map(text_of),
                },
            );
        }
    }
    out
}

/// True when `el` sits inside a `details.day-variant` subtree. ElementRef
/// derefs to the DOM tree's NodeRef (scraper 0.27), so the ancestors walk
/// needs no extra state; `day-variant` is the class the worker puts on every
/// variant `<details>`.
fn in_variant_subtree(el: ElementRef) -> bool {
    el.ancestors().filter_map(ElementRef::wrap).any(|a| has_class(a, "day-variant"))
}

/// The `?at=` pin for the record pages: noon the day AFTER the plan's last
/// dated main day — every dated session is then past, and the page is outside
/// the trip window. None when the date does not parse (same style as
/// `verify_site_page::outside_trip_pin`).
pub(crate) fn after_trip_pin(last_main_date: &str) -> Option<String> {
    let day = chrono::NaiveDate::parse_from_str(last_main_date.trim(), "%Y-%m-%d").ok()?;
    Some(format!("{}T12:00", day.succ_opt()?.format("%Y-%m-%d")))
}

/// A status the record CLI never writes (only raw `db exec` can): done|skipped
/// and empty are legal, anything else can never render as either.
fn illegal_status(status: &Option<String>) -> Option<&str> {
    status
        .as_deref()
        .filter(|s| !s.is_empty() && *s != "done" && *s != "skipped")
}

/// The names a session's 未前往 line must list, in the worker's order
/// (activities in sort order, then meals — render_unit's `skipped` vec).
fn expected_skipped_line<'a>(
    records: &'a PlanRecords,
    day: i64,
    session: &str,
) -> Vec<&'a str> {
    let mut names: Vec<&str> = records
        .activities
        .iter()
        .filter(|a| a.day == day && a.session == session && a.status.as_deref() == Some("skipped"))
        .map(|a| a.title.as_str())
        .collect();
    names.extend(
        records
            .meals
            .iter()
            .filter(|m| m.day == day && m.session == session && m.status.as_deref() == Some("skipped"))
            .map(|m| m.text.as_str()),
    );
    names
}

fn missing_session(anc: &str) -> Check {
    Check::fail(
        format!("no session block #{anc} on the page (undated day or variant: the record never renders)"),
        Some(Next::Deploy),
    )
}

/// The first line of `notes` as the page shows it: text before any URL (the
/// worker may fold a URL into a short labeled link), whitespace collapsed.
fn notes_probe(notes: &str) -> String {
    let first = notes.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let cut = first.find("http").map_or(first, |i| &first[..i]);
    cut.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Notes check: every non-skipped activity with notes must render them inside
/// its own `div.activity` (the first line, HTML-unescaped, appears in the
/// `div.act-notes`). One verdict per (activity x language).
pub(crate) fn notes_checks(pages: &[(&str, &RecordPage)], notes: &[ActivityNote]) -> Vec<(String, Check)> {
    let mut out = Vec::new();
    for (lang, page) in pages {
        for n in notes {
            let anc = anchor(n.day, &n.session);
            let key = format!("{lang}:notes:{}", n.id);
            let Some(sess) = page.sessions.get(&anc) else {
                out.push((key, missing_session(&anc)));
                continue;
            };
            let probe = notes_probe(&n.notes);
            let shown = sess.activities.get(&n.id).and_then(|r| r.notes_text.as_deref());
            let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
            let check = match shown {
                Some(t) if norm(t).contains(&probe) => {
                    Check::pass(format!("notes of 「{}」 shown in #{anc}", n.title))
                }
                Some(t) => Check::fail(
                    format!("notes of 「{}」 render {:?}, want {:?}", n.title, norm(t), probe),
                    None,
                ),
                None => Check::fail(
                    format!("activity 「{}」 has notes but renders no act-notes block in #{anc}", n.title),
                    None,
                ),
            };
            out.push((key, check));
        }
    }
    out
}

/// The pure judge: every record row against every parsed page. Emits one
/// verdict per (row × language), plus one per illegal status value.
pub(crate) fn record_checks(pages: &[(&str, &RecordPage)], records: &PlanRecords) -> Vec<(String, Check)> {
    let mut out = Vec::new();
    for a in &records.activities {
        if let Some(bad) = illegal_status(&a.status) {
            out.push((
                format!("activity:{}", a.id),
                Check::fail(format!("activity 「{}」 has record_status {bad:?} (legal: done | skipped)", a.title), None),
            ));
        }
    }
    for m in &records.meals {
        if let Some(bad) = illegal_status(&m.status) {
            out.push((
                format!("meal:day-{}-{}-{}", m.day, m.session, m.sort_order),
                Check::fail(format!("meal 「{}」 has record_status {bad:?} (legal: done | skipped)", m.text), None),
            ));
        }
    }
    for l in &records.legs {
        if let Some(bad) = illegal_status(&l.status) {
            out.push((
                format!("leg:day-{}-{}", l.day, l.sort_order),
                Check::fail(
                    format!("leg 「{} → {}」 has record_status {bad:?} (legal: done | skipped)", l.from_place, l.to_place),
                    None,
                ),
            ));
        }
    }
    for (lang, page) in pages {
        for s in &records.summaries {
            let anc = anchor(s.day, &s.session);
            let check = match page.sessions.get(&anc) {
                None => missing_session(&anc),
                Some(shown) if shown.focus == s.summary_zh => {
                    Check::pass(format!("record_summary_zh shown in #{anc}"))
                }
                Some(shown) => Check::fail(
                    format!(
                        "#{} shows focus {:?}, want the record summary {:?}",
                        anc, shown.focus, s.summary_zh
                    ),
                    None,
                ),
            };
            out.push((format!("{lang}:summary:{anc}"), check));
        }
        for a in &records.activities {
            let anc = anchor(a.day, &a.session);
            let Some(sess) = page.sessions.get(&anc) else {
                out.push((format!("{lang}:activity:{}", a.id), missing_session(&anc)));
                continue;
            };
            let row = sess.activities.get(&a.id);
            let mut problems: Vec<String> = Vec::new();
            match a.status.as_deref() {
                Some("done") => match row {
                    None => problems.push(format!(
                        "done activity 「{}」 renders no line in #{anc}",
                        a.title
                    )),
                    Some(r) if r.actual => {}
                    Some(r) => problems.push(format!(
                        "done activity 「{}」 shows no 實際 badge (record-badge--actual){}",
                        a.title,
                        r.unrecorded.then_some(" — the 計畫・未紀錄 badge instead").unwrap_or("")
                    )),
                },
                Some("skipped") => {
                    if row.is_some() {
                        problems.push(format!("skipped activity 「{}」 still renders its own line", a.title));
                    }
                    let expected = expected_skipped_line(records, a.day, &a.session);
                    let named = sess.skipped.iter().any(|t| t.contains(&a.title));
                    let whole_line = sess
                        .skipped
                        .iter()
                        .any(|t| expected.is_empty() || t.ends_with(&expected.join("、")));
                    if !named {
                        problems.push(format!("skipped activity 「{}」 is named in no 未前往 line", a.title));
                    } else if !whole_line {
                        problems.push(format!(
                            "the 未前往 line of #{anc} does not list the session's skipped rows in order (want {:?})",
                            expected.join("、")
                        ));
                    }
                }
                _ => {}
            }
            if let Some(note) = a.note.as_deref().filter(|n| !n.is_empty()) {
                let shown_note = row.and_then(|r| r.note.as_deref());
                if shown_note != Some(note) {
                    problems.push(format!("record_note {note:?} is not shown (page shows {shown_note:?})"));
                }
            }
            out.push((
                format!("{lang}:activity:{}", a.id),
                if problems.is_empty() {
                    Check::pass(format!("「{}」 renders as recorded in #{anc}", a.title))
                } else {
                    Check::fail(problems.join("; "), Some(Next::Deploy))
                },
            ));
        }
        for m in &records.meals {
            let anc = anchor(m.day, &m.session);
            let key = format!("{lang}:meal:day-{}-{}-{}", m.day, m.session, m.sort_order);
            let Some(sess) = page.sessions.get(&anc) else {
                out.push((key, missing_session(&anc)));
                continue;
            };
            let row = sess.meals.get(&m.sort_order);
            let mut problems: Vec<String> = Vec::new();
            match m.status.as_deref() {
                Some("done") => match row {
                    None => problems.push(format!("done meal 「{}」 renders no line in #{anc}", m.text)),
                    Some(r) if r.actual => {}
                    Some(_) => problems.push(format!("done meal 「{}」 shows no 實際 badge", m.text)),
                },
                Some("skipped") => {
                    if row.is_some() {
                        problems.push(format!("skipped meal 「{}」 still renders its own line", m.text));
                    }
                    if !sess.skipped.iter().any(|t| t.contains(&m.text)) {
                        problems.push(format!("skipped meal 「{}」 is named in no 未前往 line", m.text));
                    }
                }
                _ => {}
            }
            if let Some(note) = m.note.as_deref().filter(|n| !n.is_empty()) {
                let shown_note = row.and_then(|r| r.note.as_deref());
                if shown_note != Some(note) {
                    problems.push(format!("record_note {note:?} is not shown (page shows {shown_note:?})"));
                }
            }
            out.push((
                key,
                if problems.is_empty() {
                    Check::pass(format!("「{}」 renders as recorded in #{anc}", m.text))
                } else {
                    Check::fail(problems.join("; "), Some(Next::Deploy))
                },
            ));
        }
        for l in &records.legs {
            let key = format!("{lang}:leg:day-{}-{}", l.day, l.sort_order);
            let leg = page.legs.get(&(l.day, l.sort_order));
            let mut problems: Vec<String> = Vec::new();
            match l.status.as_deref() {
                Some("done") => match leg {
                    None => problems.push(format!(
                        "done leg 「{} → {}」 renders no line in day {}'s route block",
                        l.from_place, l.to_place, l.day
                    )),
                    Some(s) if s.done => {}
                    Some(_) => problems.push(format!(
                        "done leg 「{} → {}」 shows no 實際 badge",
                        l.from_place, l.to_place
                    )),
                },
                Some("skipped") => {
                    if leg.is_some() {
                        problems.push(format!(
                            "skipped leg 「{} → {}」 is still drawn in the route block",
                            l.from_place, l.to_place
                        ));
                    }
                }
                _ => {}
            }
            if let Some(note) = l.note.as_deref().filter(|n| !n.is_empty()) {
                if leg.and_then(|s| s.note.as_deref()) != Some(note) {
                    problems.push(format!("record_note {note:?} is not shown in the leg's record-note element"));
                }
            }
            out.push((
                key,
                if problems.is_empty() {
                    Check::pass(format!(
                        "「{} → {}」 renders as recorded",
                        l.from_place, l.to_place
                    ))
                } else {
                    Check::fail(problems.join("; "), Some(Next::Deploy))
                },
            ));
        }
    }
    out
}

/// The live half: read the plan's record rows, then check both language pages
/// pinned after the trip. Runs inside the plan's verify-token lifetime.
pub(crate) async fn run(
    conn: &Connection,
    plan: &str,
    token: &str,
    tmp: &Path,
    tally: &mut Tally,
) {
    let dest = match crate::cascade::common::resolve_active_destination(conn, plan, None).await {
        Ok(d) => d,
        Err(e) => {
            tally.emit(plan, LAYER, "read", &Check::fail(format!("cannot resolve the plan's destination: {e}"), None));
            return;
        }
    };
    let records = match travel_db::repo::records::list_plan_records(conn, plan, &dest).await {
        Ok(r) => r,
        Err(e) => {
            tally.emit(plan, LAYER, "read", &Check::fail(format!("record rows read failed: {e}"), None));
            return;
        }
    };
    let notes = match travel_db::repo::records::list_activity_notes(conn, plan, &dest).await {
        Ok(n) => n,
        Err(e) => {
            tally.emit(plan, LAYER, "notes", &Check::fail(format!("activity notes read failed: {e}"), None));
            return;
        }
    };
    if records.is_empty() && notes.is_empty() {
        tally.emit(plan, LAYER, "none", &Check::pass("no record rows (nothing recorded yet)"));
        return;
    }
    let last_main_date = match travel_db::repo::days::main_day_dates(conn, plan).await {
        Ok(mut dates) => dates.pop(),
        Err(e) => {
            tally.emit(plan, LAYER, "none", &Check::fail(format!("main-day dates read failed: {e}"), None));
            return;
        }
    };
    let pin = last_main_date.as_deref().and_then(after_trip_pin);
    let Some(pin) = pin else {
        tally.emit(
            plan,
            LAYER,
            "none",
            &Check::fail(
                "record rows exist but no main day has a date — a session without a date is never past, so the records can never render",
                None,
            ),
        );
        return;
    };
    let mut pages: Vec<(&str, RecordPage)> = Vec::new();
    for (lang, suffix) in [("zh", ""), ("en", "&lang=en")] {
        let url = format!("{}&at={pin}{suffix}", crate::share_token::share_url(plan, token));
        match http_get(&url, tmp) {
            Ok(live) => {
                let html = String::from_utf8_lossy(&live.body);
                let doc = crate::verify_site_page::parse_page(&html);
                if !crate::verify_site_page::page_usable(live.http, &doc) {
                    tally.emit(
                        plan,
                        LAYER,
                        &format!("{lang}:page"),
                        &Check::fail(format!("HTTP {} — not the plan page (auth page / no access)", live.http), None),
                    );
                    continue;
                }
                pages.push((lang, parse_record_page(&html)));
            }
            Err(e) => tally.emit(
                plan,
                LAYER,
                &format!("{lang}:page"),
                &Check::fail(format!("page fetch failed: {}", redact(&e, token)), None),
            ),
        }
    }
    let borrowed: Vec<(&str, &RecordPage)> = pages.iter().map(|(l, p)| (*l, p)).collect();
    for (key, check) in record_checks(&borrowed, &records).into_iter().chain(notes_checks(&borrowed, &notes)) {
        tally.emit(plan, LAYER, &key, &check);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use travel_db::repo::records::{ActivityRecord, LegRecord, MealRecord, SessionSummaryRecord};

    fn page(body: &str) -> String {
        format!(
            "<!doctype html><html lang=\"zh-TW\"><head><style>.record-badge{{}} .record-skipped{{}} .meal{{}}</style></head>\
             <body data-plan-id=\"p-1\" data-view=\"viewer\">{body}</body></html>"
        )
    }

    /// A past session exactly as the worker renders it (render/session.rs
    /// `render_unit`): done activity (badge+note INSIDE the div), done meal
    /// (badge+note as siblings AFTER the element, with the 備案 and AI badge
    /// spans between), the skipped rows collapsed to one line, and the record
    /// summary as the focus. `mutations` edit the HTML for the bite tests.
    fn recorded_session(mutations: &[(&str, &str)]) -> String {
        let mut html = "\
<section class=\"day day-full\" data-day=\"2\" data-date=\"2026-11-15\" id=\"day-2\"><details data-block=\"day-2\" open><summary><h2>第 2 天</h2></summary><div class=\"day-body\">\
<div class=\"route-block\"><div class=\"route-heading\">今日路線</div>\
<div class=\"route-seg\" data-seg-order=\"0\" data-search-unit data-search-label=\"11/15（日）· 今日路線\"><span class=\"route-mode\">🚌</span> 飯店 → 東寺 · 08:30 · 約 20 分鐘 <span class=\"record-badge\">實際</span> · <span class=\"record-note\">走路10分</span></div>\
<div class=\"route-seg\" data-seg-order=\"2\" data-search-unit data-search-label=\"11/15（日）· 今日路線\"><span class=\"route-mode\">🚌</span> 東寺 → 飯店 <span class=\"record-badge\">實際</span></div>\
</div>\
<div class=\"session session-noon\" id=\"day-2-noon\" data-search-unit data-search-label=\"11/15（日）· 中午\">\
<div class=\"session-label\">中午 <span class=\"session-time\">11:30–13:00</span></div>\
<div class=\"session-focus\">實際去了東寺，改吃隔壁</div>\
<div class=\"activity\" data-activity-id=\"a1\">東寺 <span class=\"record-badge record-badge--actual\">實際</span> <span class=\"record-note\">很棒</span></div>\
<a class=\"meal\" data-meal-order=\"0\" href=\"https://www.google.com/maps/search/x\" target=\"_blank\" rel=\"noopener\">🍽️ 主選店 🗺️</a><span class=\"meal meal-alt\">｜備案：別家</span><span class=\"ai-rec-badge\">🤖</span> <span class=\"record-badge record-badge--actual\">實際</span> <span class=\"record-note\">好吃</span>\
<div class=\"record-skipped\">未前往：未前往店、備選店</div>\
</div></div></details></section>"
            .to_string();
        for (old, new) in mutations {
            assert!(html.contains(old), "mutation target {old:?} not in the fixture");
            html = html.replace(old, new);
        }
        html
    }

    fn records() -> PlanRecords {
        PlanRecords {
            activities: vec![
                ActivityRecord {
                    id: "a1".into(),
                    day: 2,
                    session: "noon".into(),
                    title: "東寺".into(),
                    status: Some("done".into()),
                    note: Some("很棒".into()),
                },
                ActivityRecord {
                    id: "a2".into(),
                    day: 2,
                    session: "noon".into(),
                    title: "未前往店".into(),
                    status: Some("skipped".into()),
                    note: None,
                },
            ],
            meals: vec![
                MealRecord {
                    day: 2,
                    session: "noon".into(),
                    sort_order: 0,
                    text: "主選店".into(),
                    status: Some("done".into()),
                    note: Some("好吃".into()),
                },
                MealRecord {
                    day: 2,
                    session: "noon".into(),
                    sort_order: 1,
                    text: "備選店".into(),
                    status: Some("skipped".into()),
                    note: None,
                },
            ],
            legs: vec![
                LegRecord {
                    day: 2,
                    sort_order: 0,
                    from_place: "飯店".into(),
                    to_place: "東寺".into(),
                    status: Some("done".into()),
                    note: Some("走路10分".into()),
                },
                LegRecord {
                    day: 2,
                    sort_order: 1,
                    from_place: "東寺".into(),
                    to_place: "未前往店".into(),
                    status: Some("skipped".into()),
                    note: None,
                },
                LegRecord {
                    day: 2,
                    sort_order: 2,
                    from_place: "東寺".into(),
                    to_place: "飯店".into(),
                    status: Some("done".into()),
                    note: None,
                },
            ],
            summaries: vec![SessionSummaryRecord {
                day: 2,
                session: "noon".into(),
                summary_zh: "實際去了東寺，改吃隔壁".into(),
            }],
        }
    }

    fn levels(out: &[(String, Check)]) -> Vec<(String, crate::verify_site::Level)> {
        out.iter().map(|(k, c)| (k.clone(), c.level)).collect()
    }

    #[test]
    fn a_fully_recorded_session_passes_every_check() {
        let parsed = parse_record_page(&page(&recorded_session(&[])));
        let out = record_checks(&[("zh", &parsed)], &records());
        assert!(!out.is_empty());
        for (key, check) in &out {
            assert_eq!(check.level, crate::verify_site::Level::Pass, "{key}: {check:?}");
        }
        // The meal's sibling badge/note were paired onto the right row.
        assert_eq!(
            parsed.sessions["day-2-noon"].meals[&0],
            RowShown { actual: true, unrecorded: false, note: Some("好吃".into()), notes_text: None }
        );
        // One verdict per row (plus the summary), all keyed and lang-prefixed.
        let mut keys: Vec<&str> = out.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "zh:activity:a1",
                "zh:activity:a2",
                "zh:leg:day-2-0",
                "zh:leg:day-2-1",
                "zh:leg:day-2-2",
                "zh:meal:day-2-noon-0",
                "zh:meal:day-2-noon-1",
                "zh:summary:day-2-noon",
            ]
        );
    }

    #[test]
    fn missing_actual_badge_missing_note_wrong_summary_and_stale_skipped_line_fail() {
        let mutations: &[(&str, &str, &str)] = &[
            (
                "done activity shows no badge",
                "<span class=\"record-badge record-badge--actual\">實際</span> <span class=\"record-note\">很棒</span>",
                "<span class=\"record-badge\">計畫・未紀錄</span> <span class=\"record-note\">很棒</span>",
            ),
            (
                "note not shown",
                "<span class=\"record-note\">好吃</span>",
                "",
            ),
            (
                "summary not shown (focus still the plan text)",
                "<div class=\"session-focus\">實際去了東寺，改吃隔壁</div>",
                "<div class=\"session-focus\">原本計畫的焦點</div>",
            ),
            (
                "skipped line missing its rows",
                "未前往：未前往店、備選店",
                "未前往：別的店",
            ),
        ];
        for (name, old, new) in mutations {
            let parsed = parse_record_page(&page(&recorded_session(&[(old, new)])));
            let out = record_checks(&[("zh", &parsed)], &records());
            let fails: Vec<&(String, Check)> = out.iter().filter(|(_, c)| c.level == crate::verify_site::Level::Fail).collect();
            assert!(!fails.is_empty(), "{name}: nothing failed");
            // The other rows still pass: the failure is pinned to its own row.
            assert!(
                out.iter().any(|(_, c)| c.level == crate::verify_site::Level::Pass),
                "{name}: everything failed ({:?})",
                levels(&out)
            );
        }
    }

    #[test]
    fn skipped_rows_still_rendering_and_missing_blocks_fail() {
        // The skipped activity renders its own line (no collapsed skip).
        let parsed = parse_record_page(&page(&recorded_session(&[(
            "<div class=\"record-skipped\">未前往：未前往店、備選店</div>",
            "<div class=\"activity\" data-activity-id=\"a2\">未前往店</div><div class=\"record-skipped\">未前往：備選店</div>",
        )])));
        let out = record_checks(&[("zh", &parsed)], &records());
        let c = out.iter().find(|(k, _)| k == "zh:activity:a2").unwrap();
        assert!(
            c.1.level == crate::verify_site::Level::Fail
                && c.1.detail.contains("still renders its own line")
                && c.1.next == Some(Next::Deploy),
            "{:?}",
            c.1
        );

        // The skipped leg is still drawn in the route block.
        let parsed = parse_record_page(&page(&recorded_session(&[(
            "<div class=\"route-seg\" data-seg-order=\"2\"",
            "<div class=\"route-seg\" data-seg-order=\"1\"",
        )])));
        let out = record_checks(&[("zh", &parsed)], &records());
        let c = out.iter().find(|(k, _)| k == "zh:leg:day-2-1").unwrap();
        assert!(c.1.level == crate::verify_site::Level::Fail && c.1.detail.contains("still drawn"), "{:?}", c.1);

        // The done leg's line is gone entirely.
        let parsed = parse_record_page(&page(&recorded_session(&[(
            "<div class=\"route-seg\" data-seg-order=\"0\" data-search-unit data-search-label=\"11/15（日）· 今日路線\"><span class=\"route-mode\">🚌</span> 飯店 → 東寺 · 08:30 · 約 20 分鐘 <span class=\"record-badge\">實際</span> · <span class=\"record-note\">走路10分</span></div>",
            "",
        )])));
        let out = record_checks(&[("zh", &parsed)], &records());
        let c = out.iter().find(|(k, _)| k == "zh:leg:day-2-0").unwrap();
        assert!(c.1.level == crate::verify_site::Level::Fail && c.1.detail.contains("renders no line"), "{:?}", c.1);

        // No session block at all (undated day / variant): every row of the session fails.
        let parsed = parse_record_page(&page(&recorded_session(&[("id=\"day-2-noon\"", "id=\"day-2-evening\"")])));
        let out = record_checks(&[("zh", &parsed)], &records());
        assert!(out.iter().filter(|(k, _)| k.starts_with("zh:activity:") || k.starts_with("zh:meal:") || k.starts_with("zh:summary:"))
            .all(|(_, c)| c.level == crate::verify_site::Level::Fail && c.detail.contains("no session block")));
    }

    #[test]
    fn variant_route_legs_never_hijack_the_main_days_leg_mapping() {
        // Worker render/day.rs nests each variant INSIDE its main day's card,
        // and variants render with no clock — a variant seg can never carry a
        // record badge. Its data-seg-order keys the VARIANT's own day, so it
        // must not enter the main day's (day, sort_order) map: with the same
        // sort_order a variant leg overwrites the main one (done legs lose
        // their badge; a skipped main slot reads as "still drawn").
        let variant = concat!(
            "<details class=\"day-variant\" id=\"variant-5\" data-variant-of=\"2\" data-variant-day=\"5\">",
            "<summary class=\"day-variant-summary\">雨天方案</summary><div class=\"day-variant-body\">",
            "<div class=\"route-block\"><div class=\"route-heading\">今日路線</div>",
            "<div class=\"route-seg\" data-seg-order=\"0\"><span class=\"route-mode\">🚕</span> 飯店 → 備案景點</div>",
            "<div class=\"route-seg\" data-seg-order=\"1\"><span class=\"route-mode\">🚕</span> 備案景點 → 備案餐廳</div>",
            "<div class=\"route-seg\" data-seg-order=\"2\"><span class=\"route-mode\">🚕</span> 備案餐廳 → 飯店</div>",
            "</div></div></details>",
        );
        let with_variant = format!("</div></div>{variant}</details></section>");
        let parsed = parse_record_page(&page(&recorded_session(&[(
            "</div></div></details></section>",
            with_variant.as_str(),
        )])));
        let out = record_checks(&[("zh", &parsed)], &records());
        for key in ["zh:leg:day-2-0", "zh:leg:day-2-1", "zh:leg:day-2-2"] {
            let c = out.iter().find(|(k, _)| k == key).unwrap();
            assert_eq!(c.1.level, crate::verify_site::Level::Pass, "{key}: {:?}", c.1);
        }
    }

    #[test]
    fn leg_record_note_must_be_in_the_record_note_element_not_the_planned_text() {
        // The same words sit in the leg's PLANNED notes, but the worker did not
        // render the record note (no .record-note element): must FAIL.
        let parsed = parse_record_page(&page(&recorded_session(&[(
            "<span class=\"record-note\">走路10分</span>",
            "走路10分",
        )])));
        let out = record_checks(&[("zh", &parsed)], &records());
        let c = out.iter().find(|(k, _)| k == "zh:leg:day-2-0").unwrap();
        assert_eq!(c.1.level, crate::verify_site::Level::Fail, "{:?}", c.1);
        assert!(c.1.detail.contains("走路10分"), "{:?}", c.1);
    }

    #[test]
    fn illegal_status_fails_once_regardless_of_language() {
        let mut rec = records();
        rec.activities.push(ActivityRecord {
            id: "a7".into(),
            day: 2,
            session: "noon".into(),
            title: "亂數列".into(),
            status: Some("maybe".into()),
            note: None,
        });
        let parsed = parse_record_page(&page(&recorded_session(&[])));
        let out = record_checks(&[("zh", &parsed), ("en", &parsed)], &rec);
        let illegal: Vec<&(String, Check)> =
            out.iter().filter(|(k, _)| k == "activity:a7").collect();
        assert_eq!(illegal.len(), 1, "reported once, not per language: {out:?}");
        assert!(illegal[0].1.level == crate::verify_site::Level::Fail && illegal[0].1.detail.contains("maybe"), "{:?}", illegal[0].1);
        // All the legal rows are checked per language.
        assert_eq!(out.iter().filter(|(k, _)| k.starts_with("zh:")).count(), out.iter().filter(|(k, _)| k.starts_with("en:")).count());
    }

    fn note(id: &str, notes: &str) -> ActivityNote {
        ActivityNote { id: id.into(), day: 2, session: "noon".into(), title: "東寺".into(), notes: notes.into() }
    }
    fn noted_page(block: &str) -> RecordPage {
        parse_record_page(&page(&format!(
            "<div class=\"session session-noon\" id=\"day-2-noon\"><div class=\"activity\" data-activity-id=\"a1\">東寺 <span class=\"req-chip\">Ping 指定</span>{block}</div></div>"
        )))
    }
    #[test]
    fn notes_check_passes_when_first_line_is_in_the_act_notes_block() {
        let p = noted_page("<div class=\"act-notes\">營業 9-17 &lt;b&gt;<br>停車 <a href=\"https://x\">地圖</a></div>");
        let out = notes_checks(&[("zh", &p)], &[note("a1", "營業 9-17 <b>\n停車 https://x")]);
        assert_eq!(levels(&out), vec![("zh:notes:a1".into(), crate::verify_site::Level::Pass)], "{out:?}");
    }
    #[test]
    fn notes_check_fails_on_missing_block_wrong_text_and_missing_session() {
        let none = noted_page("");
        let out = notes_checks(&[("en", &none)], &[note("a1", "營業 9-17")]);
        assert!(matches!(out[0].1.level, crate::verify_site::Level::Fail), "{out:?}");
        let wrong = noted_page("<div class=\"act-notes\">別的字</div>");
        let out = notes_checks(&[("en", &wrong)], &[note("a1", "營業 9-17")]);
        assert!(matches!(out[0].1.level, crate::verify_site::Level::Fail), "{out:?}");
        let mut other = note("a1", "x");
        other.day = 9;
        let out = notes_checks(&[("en", &wrong)], &[other]);
        assert!(matches!(out[0].1.level, crate::verify_site::Level::Fail), "{out:?}");
    }
    #[test]
    fn req_chip_and_notes_block_never_disturb_the_record_pairing() {
        let p = parse_record_page(&page("<div class=\"session session-noon\" id=\"day-2-noon\"><div class=\"activity\" data-activity-id=\"a1\">東寺 <span class=\"req-chip\">Ping 指定</span><div class=\"act-notes\">n</div> <span class=\"record-badge record-badge--actual\">實際</span> <span class=\"record-note\">很棒</span></div></div>"));
        let r = &p.sessions["day-2-noon"].activities["a1"];
        assert!(r.actual && !r.unrecorded, "{r:?}");
        assert_eq!(r.note.as_deref(), Some("很棒"));
        assert_eq!(r.notes_text.as_deref(), Some("n"));
    }
    #[test]
    fn notes_probe_cuts_at_url_and_takes_first_nonblank_line() {
        assert_eq!(notes_probe("\n  營業  9-17 \n停車"), "營業 9-17");
        assert_eq!(notes_probe("地圖：https://x/y"), "地圖：");
    }
    #[test]
    fn after_trip_pin_is_the_day_after_the_last_main_day() {
        assert_eq!(after_trip_pin("2026-11-16").as_deref(), Some("2026-11-17T12:00"));
        assert_eq!(after_trip_pin("2026-11-30").as_deref(), Some("2026-12-01T12:00"));
        assert_eq!(after_trip_pin(" 2026-11-16 ").as_deref(), Some("2026-11-17T12:00"));
        assert_eq!(after_trip_pin("2026-11-xx"), None);
    }
}
