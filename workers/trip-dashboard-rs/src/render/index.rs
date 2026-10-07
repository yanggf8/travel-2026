//! Owner-only plans index: list every plan as a link to `/?plan=<plan_id>`.
//! `plan_id` is a controlled slug (hyphenated), safe in an href by construction;
//! all displayed text still goes through esc().

use super::{esc, share};
use crate::i18n::t;
use crate::turso::Row;

fn rs(row: &Row, k: &str) -> String {
    row.get(k)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Today as `YYYY-MM-DD` from epoch milliseconds (UTC). Pure (no JS runtime)
/// so the tab split is unit-testable; the router feeds `worker::Date::now()`.
pub fn today_from_ms(ms: i64) -> String {
    let (y, m, d) = civil_from_days(ms.div_euclid(86_400_000));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Inverse of Hinnant's days_from_civil: days since 1970-01-01 → (y, m, d).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A plan is PAST only when its trip has fully ended (`end_date` parses and is
/// strictly before today); undated or unparseable plans are still being planned
/// and stay on the upcoming tab.
fn is_past(row: &Row, today: &str) -> bool {
    let end = rs(row, "end_date");
    !end.is_empty() && end.as_str() < today
}

pub fn render(
    plan_rows: &[Row],
    grants: &share::GrantMaps,
    public_origin: &str,
    csrf: &crate::router::GrantCsrf,
    lang: &str,
    tab: &str,
    today: &str,
) -> String {
    let show_past = tab == "past";
    let upcoming: Vec<&Row> = plan_rows
        .iter()
        .filter(|p| !is_past(p, today))
        .collect();
    let past: Vec<&Row> = plan_rows.iter().filter(|p| is_past(p, today)).collect();
    let (rows, _) = if show_past { (&past, &upcoming) } else { (&upcoming, &past) };

    let mut h = String::new();
    h.push_str(&format!("<h1>{}</h1>", esc(t("plans", lang))));

    // Server-side tabs (SSR, no client JS): links reload with ?tab=; the active
    // tab renders its group only. Counts on both pills so the other side is visible.
    let tab_href = |name: &str| {
        let mut href = format!("/?tab={name}");
        if lang == "en" {
            href.push_str("&amp;lang=en");
        }
        href
    };
    h.push_str("<nav class=\"plan-tabs\">");
    for (name, label, count, active) in [
        ("upcoming", t("tabUpcoming", lang), upcoming.len(), !show_past),
        ("past", t("tabPast", lang), past.len(), show_past),
    ] {
        h.push_str(&format!(
            "<a class=\"plan-tab{}\" href=\"{}\">{} <span class=\"plan-tab-count\">{}</span></a>",
            if active { " plan-tab--active" } else { "" },
            tab_href(name),
            esc(&label),
            count
        ));
    }
    h.push_str("</nav>");

    if rows.is_empty() {
        h.push_str(&format!(
            "<div class=\"plan-index-empty\">{}</div>",
            esc(t("noPlansThisTab", lang))
        ));
        return h;
    }
    h.push_str("<ul class=\"plan-index\">");
    for p in rows {
        let plan_id = rs(p, "plan_id");
        if plan_id.is_empty() {
            continue;
        }
        let name = {
            let dn = rs(p, "display_name");
            if dn.is_empty() {
                plan_id.clone()
            } else {
                dn
            }
        };
        let start = rs(p, "start_date");
        let end = rs(p, "end_date");
        let dates = match (start.is_empty(), end.is_empty()) {
            (false, false) => format!("{} – {}", start, end),
            (false, true) => start.clone(),
            _ => String::new(),
        };
        // plan_id is a known-safe slug; embed directly in the href.
        h.push_str(&format!(
            "<li class=\"plan-card\"><a class=\"plan-card-main\" href=\"/?plan={}\">",
            esc(&plan_id)
        ));
        h.push_str(&format!(
            "<div class=\"plan-card-name\">{}</div>",
            esc(&name)
        ));
        if !dates.is_empty() {
            h.push_str(&format!(
                "<div class=\"plan-card-dates\">{}</div>",
                esc(&dates)
            ));
        }
        h.push_str("</a><div class=\"plan-card-actions\">");
        if let Some(grant) = grants.plan_to_current.get(&plan_id) {
            h.push_str(&share::copy_button(
                &share::share_url(public_origin, &plan_id, &grant.token),
                lang,
            ));
        } else {
            h.push_str(&share::create_grant_form(
                &plan_id,
                &csrf.create(&plan_id),
                lang,
            ));
        }
        h.push_str("</div>");
        h.push_str(&share::grant_manager(
            &plan_id,
            grants
                .plan_to_history
                .get(&plan_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            public_origin,
            &csrf.deactivate_batch(&plan_id),
            lang,
        ));
        h.push_str("</li>");
    }
    h.push_str("</ul>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turso::Row;

    #[test]
    fn plan_appears_as_link() {
        let mut p = Row::new();
        p.insert("plan_id".into(), serde_json::json!("okinawa-2026"));
        p.insert(
            "display_name".into(),
            serde_json::json!("Okinawa June 2026"),
        );
        p.insert("start_date".into(), serde_json::json!("2026-06-21"));
        p.insert("end_date".into(), serde_json::json!("2026-06-24"));
        let html = render(
            &[p],
            &share::GrantMaps::default(),
            "https://example.dev",
            &crate::router::GrantCsrf::test(),
            "en",
            "upcoming",
            "2026-06-20",
        );
        assert!(html.contains("href=\"/?plan=okinawa-2026\""));
        assert!(html.contains("Okinawa June 2026"));
        assert!(html.contains("2026-06-21"));
    }

    #[test]
    fn falls_back_to_plan_id_when_name_absent() {
        let mut p = Row::new();
        p.insert("plan_id".into(), serde_json::json!("tokyo-2026"));
        let html = render(
            &[p],
            &share::GrantMaps::default(),
            "https://example.dev",
            &crate::router::GrantCsrf::test(),
            "en",
            "upcoming",
            "2026-09-28",
        );
        assert!(html.contains("href=\"/?plan=tokyo-2026\""));
        assert!(html.contains("tokyo-2026"));
    }

    #[test]
    fn heading_is_localized() {
        let html = render(
            &[],
            &share::GrantMaps::default(),
            "https://example.dev",
            &crate::router::GrantCsrf::test(),
            "zh",
            "upcoming",
            "2026-09-28",
        );
        assert!(html.contains("行程"));
    }

    #[test]
    fn owner_index_includes_copy_share_button_when_token_exists() {
        let mut p = Row::new();
        p.insert("plan_id".into(), serde_json::json!("okinawa-2026"));
        let mut grants = share::GrantMaps::default();
        grants.plan_to_current.insert(
            "okinawa-2026".into(),
            share::GrantToken {
                token: "share123".into(),
                plan_slug: "okinawa-2026".into(),
                status: share::GrantStatus::Active,
                created_at: "2026-06-25".into(),
                created_by: None,
                deactivated_at: None,
                deactivated_by: None,
            },
        );
        let html = render(
            &[p],
            &grants,
            "https://example.dev",
            &crate::router::GrantCsrf::test(),
            "en",
            "upcoming",
            "2026-09-28",
        );
        assert!(html.contains("copy-share-btn"));
        assert!(html.contains(
            "data-copy-url=\"https://example.dev/?plan=okinawa-2026&amp;token=share123\""
        ));
        assert!(!html.contains("<a class=\"plan-card\""));
    }

    fn row(id: &str, end: Option<&str>) -> Row {
        let mut r = Row::new();
        r.insert("plan_id".into(), serde_json::json!(id));
        if let Some(e) = end {
            r.insert("end_date".into(), serde_json::json!(e));
        }
        r
    }

    #[test]
    fn tabs_split_ended_trips_from_upcoming() {
        // today 2026-09-28: oct trip upcoming, sept trip fully ended, undated planned.
        let rows = [
            row("jiufen-2026", Some("2026-10-13")),
            row("okinawa-2026", Some("2026-09-20")),
            row("draft-2027", None),
        ];
        let up = render(
            &rows,
            &share::GrantMaps::default(),
            "https://example.dev",
            &crate::router::GrantCsrf::test(),
            "zh",
            "upcoming",
            "2026-09-28",
        );
        assert!(up.contains("jiufen-2026"));
        assert!(up.contains("draft-2027"));
        assert!(!up.contains("okinawa-2026"));
        assert!(up.contains("計畫與進行中 <span class=\"plan-tab-count\">2</span>"));
        assert!(up.contains("已過 <span class=\"plan-tab-count\">1</span>"));
        // default tab = upcoming is the ACTIVE pill; tab links preserve lang.
        assert!(up.contains("plan-tab--active\" href=\"/?tab=upcoming\">"));
        assert!(up.contains("href=\"/?tab=past\">"));

        let past = render(
            &rows,
            &share::GrantMaps::default(),
            "https://example.dev",
            &crate::router::GrantCsrf::test(),
            "zh",
            "past",
            "2026-09-28",
        );
        assert!(past.contains("okinawa-2026"));
        assert!(!past.contains("jiufen-2026"));
        assert!(past.contains("href=\"/?tab=upcoming\">"));
        // the past pill is active on the past tab
        assert!(past.contains("plan-tab--active\" href=\"/?tab=past\">"));
    }

    #[test]
    fn trip_ending_today_is_still_upcoming() {
        // `end_date < today` is the boundary — a trip ending today is in progress.
        let rows = [row("today-trip", Some("2026-09-28"))];
        let html = render(
            &rows,
            &share::GrantMaps::default(),
            "https://example.dev",
            &crate::router::GrantCsrf::test(),
            "zh",
            "upcoming",
            "2026-09-28",
        );
        assert!(html.contains("today-trip"));
    }

    #[test]
    fn empty_tab_gets_message_and_english_tab_links_carry_lang() {
        let rows = [row("jiufen-2026", Some("2026-10-13"))];
        let html = render(
            &rows,
            &share::GrantMaps::default(),
            "https://example.dev",
            &crate::router::GrantCsrf::test(),
            "en",
            "past",
            "2026-09-28",
        );
        assert!(html.contains("No plans in this tab yet."));
        assert!(!html.contains("<ul class=\"plan-index\">"));
        assert!(html.contains("href=\"/?tab=upcoming&amp;lang=en\""));
    }

    #[test]
    fn today_from_ms_roundtrips_known_dates() {
        assert_eq!(today_from_ms(0), "1970-01-01");
        // 2026-09-28 UTC = day 20724 = 1_790_553_600_000 ms
        assert_eq!(today_from_ms(1_790_553_600_000), "2026-09-28");
        assert_eq!(today_from_ms(1_790_640_000_000), "2026-09-29");
    }
}
