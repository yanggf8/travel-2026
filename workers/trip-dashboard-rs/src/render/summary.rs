//! Booking summary: flights + hotel + airport transfers.
//! Renders from raw Turso `Row`s (BTreeMap<String, Value>). Turso returns every
//! scalar as a JSON STRING, so read fields via `rs()` (as_str → owned String).
//!
//! Regression note: the old TS worker rendered transfers as "—" (it read the
//! wrong/absent columns). Here transfers MUST render selected_route + price.

use crate::model::Plan;
use crate::turso::Row;
use super::{esc, esc_url_attr};
use crate::i18n::t;

/// Read a Turso row field as an owned String (scalars come back as JSON strings).
fn rs(row: &Row, k: &str) -> String {
    row.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

/// Join non-empty parts with a single space (skips blanks so we don't emit "  ").
fn join_parts(parts: &[String]) -> String {
    parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join(" ")
}

/// Render the hotel `notes` blob into grouped <ul> bullets.
///
/// Notes are newline-delimited. A line starting with `## ` opens a new group
/// (label = the rest after the marker, rendered in `.hotel-group-label`); every
/// following non-empty, non-`##` line becomes a `<li>` inside that group's
/// `<ul>`. Blank lines are skipped. Lines before any `## ` header (or notes with
/// no headers at all) fall into an implicit unlabeled leading group so legacy
/// flat notes still render as a bullet list. Every label and line is esc()'d.
fn render_notes(notes: &str) -> String {
    let mut h = String::new();
    let mut group_open = false; // a <ul> (with optional label div) is open
    let mut wrapper_open = false; // the <div class="hotel-group"> is open

    let close_group = |h: &mut String, group_open: &mut bool, wrapper_open: &mut bool| {
        if *group_open {
            h.push_str("</ul>");
            *group_open = false;
        }
        if *wrapper_open {
            h.push_str("</div>");
            *wrapper_open = false;
        }
    };

    for raw in notes.split('\n') {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(label) = line.strip_prefix("## ") {
            close_group(&mut h, &mut group_open, &mut wrapper_open);
            h.push_str("<div class=\"hotel-group\">");
            wrapper_open = true;
            h.push_str(&format!(
                "<div class=\"hotel-group-label\">{}</div>",
                esc(label.trim())
            ));
            h.push_str("<ul>");
            group_open = true;
        } else {
            // A fact line before any header → open an unlabeled leading group.
            if !group_open {
                if !wrapper_open {
                    h.push_str("<div class=\"hotel-group\">");
                    wrapper_open = true;
                }
                h.push_str("<ul>");
                group_open = true;
            }
            h.push_str(&format!("<li>{}</li>", esc(line)));
        }
    }
    close_group(&mut h, &mut group_open, &mut wrapper_open);
    h
}

pub fn render(plan: &Plan, lang: &str) -> String {
    let mut h = String::new();
    h.push_str("<section class=\"booking-summary\">");

    // Flights
    if !plan.flights.is_empty() {
        h.push_str(&format!("<h2>{}</h2>", esc(t("flights", lang))));
        h.push_str("<div class=\"booking-grid\">");
        for f in &plan.flights {
            let number = rs(f, "flight_number");
            let airline = rs(f, "airline");
            let dep = join_parts(&[rs(f, "departure_code"), rs(f, "departure_terminal"), rs(f, "departure_time")]);
            let arr = join_parts(&[rs(f, "arrival_code"), rs(f, "arrival_terminal"), rs(f, "arrival_time")]);
            let date = rs(f, "flight_date");
            h.push_str("<div class=\"booking-item flight\">");
            h.push_str("<span class=\"booking-icon\">✈️</span>");
            h.push_str("<div class=\"booking-detail\">");
            h.push_str(&format!(
                "<div class=\"booking-value\">{} {}</div>",
                esc(&number), esc(&airline)
            ));
            h.push_str(&format!(
                "<div class=\"booking-sub\">{} → {}</div>",
                esc(&dep), esc(&arr)
            ));
            if !date.is_empty() {
                h.push_str(&format!("<div class=\"booking-sub\">{}</div>", esc(&date)));
            }
            h.push_str("</div></div>");
        }
        h.push_str("</div>");
    }

    // Hotel
    if let Some(hotel) = &plan.hotel {
        let name_zh = rs(hotel, "name_zh");
        let name = if lang == "zh" && !name_zh.is_empty() { name_zh } else { rs(hotel, "name") };
        let check_in = rs(hotel, "check_in");
        let notes = rs(hotel, "notes");
        let voucher_url = rs(hotel, "voucher_url");
        h.push_str(&format!("<h2>{}</h2>", esc(t("hotel", lang))));
        h.push_str("<div class=\"booking-grid\">");
        h.push_str("<div class=\"booking-item hotel\">");
        h.push_str("<span class=\"booking-icon\">🏨</span>");
        h.push_str("<div class=\"booking-detail\">");
        h.push_str(&format!("<div class=\"booking-value\">{}</div>", esc(&name)));
        if !check_in.is_empty() {
            h.push_str(&format!("<div class=\"booking-sub\">{}</div>", esc(&check_in)));
        }
        // Voucher PDF link (own /voucher/* R2 route). 404s until the PDF is uploaded.
        if !voucher_url.is_empty() {
            h.push_str(&format!(
                "<a class=\"voucher-link\" href=\"{}\" target=\"_blank\" rel=\"noopener\">📄 {}</a>",
                esc_url_attr(&voucher_url), esc(t("voucher", lang))
            ));
        }
        // PNR / cancellation text behind native <details> (progressive disclosure, no JS),
        // reformatted into grouped bullets (## Group headers → labeled <ul> lists).
        if !notes.is_empty() {
            h.push_str(&format!(
                "<details class=\"booking-notes\"><summary>{}</summary><div class=\"booking-notes-body\">{}</div></details>",
                esc(t("details", lang)), render_notes(&notes)
            ));
        }
        h.push_str("</div></div>");
        h.push_str("</div>");
    }

    // Transfers (the old "—" bug lived here)
    if !plan.transfers.is_empty() {
        h.push_str(&format!("<h2>{}</h2>", esc(t("transfers", lang))));
        h.push_str("<div class=\"booking-grid\">");
        for tr in &plan.transfers {
            let title = rs(tr, "selected_title");
            let route = rs(tr, "selected_route");
            let dur = rs(tr, "selected_duration_min");
            let price = rs(tr, "selected_price_yen");
            h.push_str("<div class=\"booking-item transfer\">");
            h.push_str("<span class=\"booking-icon\">🚃</span>");
            h.push_str("<div class=\"booking-detail\">");
            if !title.is_empty() {
                h.push_str(&format!("<div class=\"booking-value\">{}</div>", esc(&title)));
            }
            if !route.is_empty() {
                h.push_str(&format!("<div class=\"booking-sub\">{}</div>", esc(&route)));
            }
            let mut meta: Vec<String> = Vec::new();
            if !dur.is_empty() { meta.push(format!("~{} min", dur)); }
            if !price.is_empty() { meta.push(format!("¥{}", price)); }
            if !meta.is_empty() {
                h.push_str(&format!("<div class=\"booking-sub\">{}</div>", esc(&meta.join(" · "))));
            }
            h.push_str("</div></div>");
        }
        h.push_str("</div>");
    }

    h.push_str("</section>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Plan;
    use crate::turso::Row;

    #[test]
    fn transfer_renders_route_and_price() {
        let mut tr = Row::new();
        tr.insert("direction".into(), serde_json::json!("arrival"));
        tr.insert("selected_title".into(), serde_json::json!("Yui Rail"));
        tr.insert("selected_route".into(), serde_json::json!("Naha Airport → Asato"));
        tr.insert("selected_duration_min".into(), serde_json::json!("24"));
        tr.insert("selected_price_yen".into(), serde_json::json!("340"));
        let plan = Plan { transfers: vec![tr], ..Default::default() };
        let html = render(&plan, "en");
        assert!(html.contains("Naha Airport → Asato"));
        assert!(html.contains("340"));
        assert!(html.contains("Yui Rail"));
    }

    #[test]
    fn flight_renders_number_and_route() {
        let mut f = Row::new();
        for (k, v) in [
            ("flight_number", "CI120"), ("airline", "China Airlines"),
            ("departure_code", "TPE"), ("departure_time", "08:00"),
            ("arrival_code", "OKA"), ("arrival_time", "10:45"),
            ("flight_date", "2026-06-12"),
        ] {
            f.insert(k.into(), serde_json::json!(v));
        }
        let plan = Plan { flights: vec![f], ..Default::default() };
        let html = render(&plan, "en");
        assert!(html.contains("CI120"));
        assert!(html.contains("TPE"));
        assert!(html.contains("OKA"));
    }

    #[test]
    fn hotel_notes_behind_details_and_zh_name() {
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("Hotel Aqua Citta Naha"));
        hotel.insert("name_zh".into(), serde_json::json!("那霸水都飯店"));
        hotel.insert("check_in".into(), serde_json::json!("2026-06-21 15:00"));
        hotel.insert("notes".into(), serde_json::json!("CFM 1234567 cancellation by 2026-06-14"));
        let plan = Plan { hotel: Some(hotel), ..Default::default() };
        let html = render(&plan, "zh");
        assert!(html.contains("那霸水都飯店")); // zh name preferred
        assert!(!html.contains("Hotel Aqua Citta Naha")); // en name not shown when zh present
        assert!(html.contains("<details"));
        assert!(html.contains("CFM 1234567")); // PNR present but collapsed
    }

    #[test]
    fn hotel_grouped_notes_render_labels_and_bullets() {
        let notes = "## 房型 Room\nStandard twin · non-smoking\n## 訂單 Booking\n4 nights: 2026-06-12 → 2026-06-16\n⚠ Non-refundable\n## 用餐 Dining\nBreakfast ONLY\n## 交通 Access\nYui Rail: Asato Station";
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        hotel.insert("notes".into(), serde_json::json!(notes));
        let plan = Plan { hotel: Some(hotel), ..Default::default() };
        let html = render(&plan, "zh");
        // wrapper preserved
        assert!(html.contains("<details"));
        // group labels present (rendered without the "## " prefix)
        assert!(html.contains("房型 Room"));
        assert!(html.contains("訂單 Booking"));
        assert!(html.contains("用餐 Dining"));
        assert!(html.contains("交通 Access"));
        assert!(!html.contains("## ")); // the marker prefix must be stripped
        // fact lines become <li> items
        assert!(html.contains("<li>Standard twin · non-smoking</li>"));
        assert!(html.contains("<li>⚠ Non-refundable</li>"));
        // 4 groups → 4 <ul> blocks
        assert_eq!(html.matches("<ul").count(), 4);
        assert_eq!(html.matches("hotel-group-label").count(), 4);
    }

    #[test]
    fn hotel_notes_blank_lines_skipped() {
        let notes = "## 房型 Room\n\nStandard twin\n\n";
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        hotel.insert("notes".into(), serde_json::json!(notes));
        let plan = Plan { hotel: Some(hotel), ..Default::default() };
        let html = render(&plan, "en");
        assert!(html.contains("<li>Standard twin</li>"));
        assert!(!html.contains("<li></li>")); // no empty bullets
    }

    #[test]
    fn hotel_voucher_link_renders_with_href_and_target() {
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        hotel.insert("voucher_url".into(), serde_json::json!("/voucher/okinawa-2026/azat-voucher.pdf"));
        let plan = Plan { hotel: Some(hotel), ..Default::default() };
        let html = render(&plan, "en");
        assert!(html.contains("class=\"voucher-link\""));
        assert!(html.contains("href=\"/voucher/okinawa-2026/azat-voucher.pdf\""));
        assert!(html.contains("target=\"_blank\""));
        assert!(html.contains("rel=\"noopener\""));
        assert!(html.contains("Hotel voucher (PDF)")); // en label
    }

    #[test]
    fn hotel_without_voucher_url_renders_no_link() {
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        let plan = Plan { hotel: Some(hotel), ..Default::default() };
        let html = render(&plan, "en");
        assert!(!html.contains("voucher-link"));
    }

    #[test]
    fn empty_plan_renders_no_section_headings() {
        let plan = Plan::default();
        let html = render(&plan, "en");
        assert!(!html.contains("Flights"));
        assert!(!html.contains("Hotel"));
        assert!(!html.contains("Transfers"));
    }
}
