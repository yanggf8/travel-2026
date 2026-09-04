//! Booking summary: flights + hotel + airport transfers.
//! Renders from raw Turso `Row`s (BTreeMap<String, Value>). Turso returns every
//! scalar as a JSON STRING, so read fields via `rs()` (as_str → owned String).
//!
//! Regression note: the old TS worker rendered transfers as "—" (it read the
//! wrong/absent columns). Here transfers MUST render selected_route + price.

use super::{esc, esc_url_attr, urlencode};
use crate::i18n::t;
use crate::model::Plan;
use crate::turso::Row;

/// Read a Turso row field as an owned String (scalars come back as JSON strings).
fn rs(row: &Row, k: &str) -> String {
    row.get(k)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Group an integer with thousands separators (e.g. 27888 → "27,888"), matching the
/// TS worker's `Number.toLocaleString()` for prices. Handles negatives defensively.
fn group_thousands(n: i64) -> String {
    let neg = n < 0;
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    let len = digits.len();
    for (idx, ch) in digits.chars().enumerate() {
        if idx > 0 && (len - idx) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}

/// True when an image_url should NOT be loaded as an <img> — empty or the legacy
/// via.placeholder.com host which Chrome now reports as broken (0x0). In that case
/// we render a CSS placeholder div instead of an external request.
fn is_placeholder_image(url: &str) -> bool {
    let t = url.trim();
    t.is_empty() || t.contains("via.placeholder.com")
}

/// Join non-empty parts with a single space (skips blanks so we don't emit "  ").
fn join_parts(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Wrap flight display text in a Google search link (opens new tab). Port of the
/// TS worker's `flightLink` (render.ts:979-982): the search query is the
/// PERCENT-ENCODED flight number ONLY (`number.trim()`), never the airline. With
/// an empty flight number → plain escaped text (no anchor), mirroring the TS guard.
fn flight_link(display_text: &str, flight_number: &str) -> String {
    if flight_number.trim().is_empty() {
        return esc(display_text);
    }
    let query = urlencode(flight_number.trim());
    let url = format!("https://www.google.com/search?q={}", query);
    format!(
        "<a href=\"{}\" target=\"_blank\" rel=\"noopener\" style=\"color:inherit;text-decoration:underline dotted;text-underline-offset:3px\">{}</a>",
        esc_url_attr(&url),
        esc(display_text),
    )
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

pub fn render(plan: &Plan, lang: &str, token: Option<&str>) -> String {
    let mut h = String::new();
    // `summary-box` adds the dashed frame (user visibility request); the plan map
    // slot is rendered ABOVE this section (render_plan), never inside the frame.
    h.push_str("<section class=\"booking-summary summary-box\">");

    // Package offer (booking-summary package row) — the chosen plan_offer + its
    // selected-date price. Only shown when a package was selected (flight+hotel
    // booked-separately plans have no offer). Ported from render.ts:1078-1089.
    if let Some(offer) = &plan.offer {
        h.push_str(&format!("<h2>{}</h2>", esc(t("package", lang))));
        h.push_str("<div class=\"booking-grid\">");
        h.push_str("<div class=\"booking-item package\">");
        h.push_str("<span class=\"booking-icon\">📦</span>");
        h.push_str("<div class=\"booking-detail\">");
        let title = join_parts(&[offer.source_id.clone(), offer.product_code.clone()]);
        h.push_str(&format!(
            "<div class=\"booking-value\">{}</div>",
            esc(if title.is_empty() { "—" } else { &title })
        ));
        if offer.price > 0 {
            let cur = if offer.currency.is_empty() {
                "TWD"
            } else {
                &offer.currency
            };
            h.push_str(&format!(
                "<div class=\"booking-sub\">{} {}{} ({} {})</div>",
                esc(cur),
                group_thousands(offer.price),
                esc(t("perPerson", lang)),
                esc(t("forTwo", lang)),
                group_thousands(offer.price * 2),
            ));
        }
        h.push_str("</div></div>");
        h.push_str("</div>");
    }

    // Flights
    if !plan.flights.is_empty() {
        h.push_str(&format!("<h2>{}</h2>", esc(t("flights", lang))));
        h.push_str("<div class=\"booking-grid\">");
        for f in &plan.flights {
            let number = rs(f, "flight_number");
            let airline = rs(f, "airline");
            let dep = join_parts(&[
                rs(f, "departure_code"),
                rs(f, "departure_terminal"),
                rs(f, "departure_time"),
            ]);
            let arr = join_parts(&[
                rs(f, "arrival_code"),
                rs(f, "arrival_terminal"),
                rs(f, "arrival_time"),
            ]);
            let date = rs(f, "flight_date");
            h.push_str("<div class=\"booking-item flight\">");
            h.push_str("<span class=\"booking-icon\">✈️</span>");
            h.push_str("<div class=\"booking-detail\">");
            let display = join_parts(&[number.clone(), airline.clone()]);
            h.push_str(&format!(
                "<div class=\"booking-value\">{}</div>",
                flight_link(&display, &number)
            ));
            h.push_str(&format!(
                "<div class=\"booking-sub\">{} → {}</div>",
                esc(&dep),
                esc(&arr)
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
        let name = if lang == "zh" && !name_zh.is_empty() {
            name_zh
        } else {
            rs(hotel, "name")
        };
        let check_in = rs(hotel, "check_in");
        let notes = rs(hotel, "notes");
        let voucher_url = rs(hotel, "voucher_url");
        h.push_str(&format!("<h2>{}</h2>", esc(t("hotel", lang))));
        h.push_str("<div class=\"booking-grid\">");
        h.push_str("<div class=\"booking-item hotel\">");
        h.push_str("<span class=\"booking-icon\">🏨</span>");
        h.push_str("<div class=\"booking-detail\">");
        h.push_str(&format!(
            "<div class=\"booking-value\">{}</div>",
            esc(&name)
        ));
        if !check_in.is_empty() {
            h.push_str(&format!(
                "<div class=\"booking-sub\">{}</div>",
                esc(&check_in)
            ));
        }
        // Hotel access lines (transit directions to the hotel) — TS rendered these as
        // a comma-joined access list (render.ts:1143). One labeled sub-line; skipped
        // when there are no rows.
        if !plan.hotel_access_lines.is_empty() {
            h.push_str(&format!(
                "<div class=\"booking-sub\">{}: {}</div>",
                esc(t("hotelAccess", lang)),
                esc(&plan.hotel_access_lines.join(", "))
            ));
        }
        // Voucher PDF link (own /voucher/* R2 route). 404s until the PDF is uploaded.
        // The /voucher/* route is auth-gated (same scope as the plan view), so a
        // tokenless link would 403 on click. Thread the page's token onto local
        // voucher links so the click carries the SAME token the page loaded with.
        if !voucher_url.is_empty() {
            let mut href = voucher_url.clone();
            if voucher_url.starts_with("/voucher/") {
                if let Some(tok) = token.filter(|t| !t.is_empty()) {
                    href.push_str("?token=");
                    href.push_str(tok);
                }
            }
            h.push_str(&format!(
                "<a class=\"voucher-link\" href=\"{}\" target=\"_blank\" rel=\"noopener\">📄 {}</a>",
                esc_url_attr(&href), esc(t("voucher", lang))
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

    // Domestic stays (Taiwan, via bookings_current category=accommodation)
    // P4-aware: when p4_status == booked, this is the primary block; when pending/selecting
    // and there is no booked stay, the block is hidden (empty → no render).
    let is_booked = plan.p4_status == "booked";
    if !plan.domestic_stays.is_empty() {
        let domestic_title = if is_booked {
            if lang == "zh" { "🏠 已訂住宿" } else { "🏠 Booked Accommodation" }
        } else {
            t("domesticStay", lang)
        };
        h.push_str(&format!("<h2>{}</h2>", esc(domestic_title)));
        h.push_str("<div class=\"booking-grid\">");
        for stay in &plan.domestic_stays {
            // Prefer hotel_name + room_type split; fallback to raw title.
            let name = if !stay.hotel_name.is_empty() {
                if stay.room_type.is_empty() {
                    stay.hotel_name.clone()
                } else {
                    format!("{} {}", stay.hotel_name, stay.room_type)
                }
            } else {
                stay.title.clone()
            };
            let cur = if stay.currency.is_empty() {
                "TWD"
            } else {
                &stay.currency
            };
            // Booked stays get the obvious green "已訂" treatment (badge + tinted card).
            let item_class = if is_booked {
                "booking-item domestic domestic--booked"
            } else {
                "booking-item domestic"
            };
            h.push_str(&format!("<div class=\"{item_class}\">"));
            h.push_str("<span class=\"booking-icon\">🏠</span>");
            h.push_str("<div class=\"booking-detail\">");
            h.push_str(&format!(
                "<div class=\"booking-value\">{}</div>",
                esc(&name)
            ));
            if is_booked {
                h.push_str(&format!(
                    "<span class=\"booked-badge\">✓ {}</span>",
                    esc(t("bookedBadge", lang))
                ));
            }
            // Price line — TWD grouped, no hard-coded JPY.
            if stay.price_twd > 0 {
                h.push_str(&format!(
                    "<div class=\"booking-sub\">{} {}</div>",
                    esc(cur),
                    group_thousands(stay.price_twd)
                ));
            }
            if !stay.selected_date.is_empty() {
                h.push_str(&format!(
                    "<div class=\"booking-sub\">{}</div>",
                    esc(&stay.selected_date)
                ));
            }
            if !stay.status.is_empty() {
                h.push_str(&format!(
                    "<div class=\"booking-sub\">{}</div>",
                    esc(&stay.status)
                ));
            }
            h.push_str("</div></div>");
        }
        h.push_str("</div>");
    }

    // Domestic candidates — sea-view shortlist (jiufen three) from domestic_accommodations.
    // P4-aware headings (pure HTML/CSS, no JS):
    //   booked   → 「其他海景參考」 + 小字「僅供參考」
    //   pending/selecting/empty → 「🏨 海景候選 · 正在選」 (candidates as primary, dashed frame)
    if !plan.candidates.is_empty() {
        let (cand_title, cand_sub): (&str, Option<&str>) = if is_booked {
            (
                if lang == "zh" { "其他海景參考" } else { "Other Sea-View References" },
                Some(if lang == "zh" { "僅供參考 · 已選定住宿" } else { "For reference · accommodation booked" }),
            )
        } else {
            (
                if lang == "zh" { "🏨 海景候選 · 正在選" } else { "🏨 Sea-View Candidates · Selecting" },
                None,
            )
        };
        h.push_str(&format!("<h2>{}</h2>", esc(cand_title)));
        if let Some(sub) = cand_sub {
            h.push_str(&format!("<div class=\"candidate-sub\">{}</div>", esc(sub)));
        }
        h.push_str("<div class=\"candidate-grid\">");
        for c in &plan.candidates {
            let title = if c.room_type.is_empty() {
                c.hotel_name.clone()
            } else {
                format!("{} {}", c.hotel_name, c.room_type)
            };
            let cur = if c.currency.is_empty() { "TWD" } else { &c.currency };
            // Selecting state → dashed frame on each card (visual "not booked yet").
            let card_class = if is_booked {
                "candidate-card"
            } else {
                "candidate-card candidate-card--selecting"
            };
            h.push_str(&format!("<div class=\"{card_class}\">"));
            if is_placeholder_image(&c.image_url) {
                h.push_str(&format!(
                    "<div class=\"candidate-image candidate-image--placeholder\" aria-label=\"{}\">{}</div>",
                    esc(&c.hotel_name),
                    esc(&c.hotel_name)
                ));
            } else {
                h.push_str(&format!(
                    "<img class=\"candidate-image\" src=\"{}\" alt=\"{}\" loading=\"lazy\" />",
                    esc_url_attr(&c.image_url),
                    esc(&c.hotel_name)
                ));
            }
            // Gallery: one thumbnail per room type / area (child table rows), each
            // linking to the full image (SSR-only, no JS lightbox).
            let gallery: Vec<_> = c
                .images
                .iter()
                .filter(|g| !is_placeholder_image(&g.image_url))
                .collect();
            if !gallery.is_empty() {
                h.push_str("<div class=\"candidate-gallery\">");
                for g in gallery {
                    h.push_str("<figure class=\"candidate-gallery-item\">");
                    h.push_str(&format!(
                        "<a href=\"{}\" target=\"_blank\" rel=\"noopener\">\
                         <img class=\"candidate-gallery-img\" src=\"{}\" alt=\"{}\" loading=\"lazy\" /></a>",
                        esc_url_attr(&g.image_url),
                        esc_url_attr(&g.image_url),
                        esc(if g.label.is_empty() { &c.hotel_name } else { &g.label }),
                    ));
                    if !g.label.is_empty() {
                        h.push_str(&format!(
                            "<figcaption class=\"candidate-gallery-label\">{}</figcaption>",
                            esc(&g.label)
                        ));
                    }
                    h.push_str("</figure>");
                }
                h.push_str("</div>");
            }
            h.push_str(&format!(
                "<div class=\"candidate-name\">{}</div>",
                esc(&title)
            ));
            if c.price_twd > 0 {
                h.push_str(&format!(
                    "<div class=\"candidate-price\">{} {}</div>",
                    esc(cur),
                    group_thousands(c.price_twd)
                ));
            }
            // Tags: sea view + breakfast
            let mut tags: Vec<String> = Vec::new();
            if c.sea_view == 1 {
                tags.push(format!("<span class=\"candidate-tag candidate-tag--sea\">{}</span>", esc(t("seaView", lang))));
            }
            if c.breakfast_included == 1 {
                tags.push(format!("<span class=\"candidate-tag candidate-tag--bf\">{}</span>", esc(t("breakfast", lang))));
            }
            if !tags.is_empty() {
                h.push_str(&format!("<div class=\"candidate-tags\">{}</div>", tags.join(" ")));
            }
            // External "查看更多房型" link (official rooms page / OTA listing).
            if !c.link_url.is_empty() {
                h.push_str(&format!(
                    "<a class=\"candidate-link\" href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a>",
                    esc_url_attr(&c.link_url),
                    esc(t("moreRoomTypes", lang))
                ));
            }
            h.push_str("</div>");
        }
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
                h.push_str(&format!(
                    "<div class=\"booking-value\">{}</div>",
                    esc(&title)
                ));
            }
            if !route.is_empty() {
                h.push_str(&format!("<div class=\"booking-sub\">{}</div>", esc(&route)));
            }
            let mut meta: Vec<String> = Vec::new();
            if !dur.is_empty() {
                meta.push(format!("~{} min", dur));
            }
            if !price.is_empty() {
                meta.push(format!("¥{}", price));
            }
            if !meta.is_empty() {
                h.push_str(&format!(
                    "<div class=\"booking-sub\">{}</div>",
                    esc(&meta.join(" · "))
                ));
            }
            h.push_str("</div></div>");
        }
        h.push_str("</div>");
    }

    // Japan-only entry info (Visit Japan Web + Japan Tourism), shown when the
    // destination currency is JPY. Ported from render.ts:1049,1203-1218.
    if plan.currency == "JPY" {
        h.push_str(&format!("<h2>{}</h2>", esc(t("japanEntry", lang))));
        h.push_str("<div class=\"booking-grid\">");
        // Visit Japan Web (entry application)
        h.push_str("<div class=\"booking-item japan-entry\">");
        h.push_str("<span class=\"booking-icon\">🛂</span>");
        h.push_str("<div class=\"booking-detail\">");
        h.push_str(&format!(
            "<div class=\"booking-value\"><a href=\"https://www.vjw.digital.go.jp/main/#/vjwplo001\" \
             target=\"_blank\" rel=\"noopener\">{}</a></div>",
            esc(t("visitJapanWeb", lang))
        ));
        h.push_str("</div></div>");
        // Japan Tourism Agency
        h.push_str("<div class=\"booking-item japan-entry\">");
        h.push_str("<span class=\"booking-icon\">🗾</span>");
        h.push_str("<div class=\"booking-detail\">");
        h.push_str(&format!(
            "<div class=\"booking-value\"><a href=\"https://www.japan.travel/\" \
             target=\"_blank\" rel=\"noopener\">{}</a></div>",
            esc(t("japanTourism", lang))
        ));
        h.push_str(&format!(
            "<div class=\"booking-sub\">{}</div>",
            esc(t("japanTourismSub", lang))
        ));
        h.push_str("</div></div>");
        h.push_str("</div>");
    }

    h.push_str("</section>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Offer, Plan};
    use crate::turso::Row;

    #[test]
    fn group_thousands_formats_prices() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(27888), "27,888");
        assert_eq!(group_thousands(55776), "55,776");
        assert_eq!(group_thousands(1234567), "1,234,567");
    }

    #[test]
    fn package_offer_renders_with_per_person_and_for_two() {
        let plan = Plan {
            offer: Some(Offer {
                source_id: "besttour".into(),
                product_code: "TYO06MM260213AM2".into(),
                price: 27888,
                currency: "TWD".into(),
            }),
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(html.contains("besttour"));
        assert!(html.contains("TYO06MM260213AM2"));
        assert!(html.contains("TWD 27,888")); // per-person, grouped
        assert!(html.contains("55,776")); // for-2 = price*2
    }

    #[test]
    fn no_offer_means_no_package_block() {
        let plan = Plan::default(); // offer: None
        let html = render(&plan, "en", None);
        assert!(!html.contains("booking-item package"));
    }

    #[test]
    fn hotel_access_lines_render_in_hotel_block() {
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        let plan = Plan {
            hotel: Some(hotel),
            hotel_access_lines: vec!["Yui Rail Asato 3min".into(), "JR Naha 8min".into()],
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(html.contains("Yui Rail Asato 3min, JR Naha 8min"));
        assert!(html.contains("Access:"));
    }

    #[test]
    fn japan_entry_rows_shown_only_for_jpy() {
        let jpy = Plan {
            currency: "JPY".into(),
            ..Default::default()
        };
        let html = render(&jpy, "en", None);
        assert!(html.contains("Visit Japan Web"));
        assert!(html.contains("vjw.digital.go.jp"));

        let twd = Plan {
            currency: "TWD".into(),
            ..Default::default()
        };
        let html2 = render(&twd, "en", None);
        assert!(!html2.contains("Visit Japan Web"));
    }

    #[test]
    fn transfer_renders_route_and_price() {
        let mut tr = Row::new();
        tr.insert("direction".into(), serde_json::json!("arrival"));
        tr.insert("selected_title".into(), serde_json::json!("Yui Rail"));
        tr.insert(
            "selected_route".into(),
            serde_json::json!("Naha Airport → Asato"),
        );
        tr.insert("selected_duration_min".into(), serde_json::json!("24"));
        tr.insert("selected_price_yen".into(), serde_json::json!("340"));
        let plan = Plan {
            transfers: vec![tr],
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(html.contains("Naha Airport → Asato"));
        assert!(html.contains("340"));
        assert!(html.contains("Yui Rail"));
    }

    #[test]
    fn flight_renders_number_and_route() {
        let mut f = Row::new();
        for (k, v) in [
            ("flight_number", "CI120"),
            ("airline", "China Airlines"),
            ("departure_code", "TPE"),
            ("departure_time", "08:00"),
            ("arrival_code", "OKA"),
            ("arrival_time", "10:45"),
            ("flight_date", "2026-06-12"),
        ] {
            f.insert(k.into(), serde_json::json!(v));
        }
        let plan = Plan {
            flights: vec![f],
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(html.contains("CI120"));
        assert!(html.contains("TPE"));
        assert!(html.contains("OKA"));
    }

    #[test]
    fn flight_number_is_clickable_google_search_link() {
        let mut f = Row::new();
        for (k, v) in [
            ("flight_number", "CI 120"),
            ("airline", "China Airlines"),
            ("departure_code", "TPE"),
            ("arrival_code", "OKA"),
        ] {
            f.insert(k.into(), serde_json::json!(v));
        }
        let plan = Plan {
            flights: vec![f],
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        // (a) href contains the percent-encoded flight number ("CI 120" → "CI%20120").
        assert!(
            html.contains("href=\"https://www.google.com/search?q=CI%20120\""),
            "encoded flight number missing from href; got: {html}"
        );
        // (b) visible anchor text includes the airline (airline is NOT in the query).
        assert!(html.contains(">CI 120 China Airlines</a>"), "got: {html}");
        // anchor styling/attrs mirror the TS port.
        assert!(html.contains("target=\"_blank\""));
        assert!(html.contains("rel=\"noopener\""));
        assert!(html.contains("text-decoration:underline dotted"));
    }

    #[test]
    fn flight_without_number_renders_plain_text_no_anchor() {
        let mut f = Row::new();
        f.insert("airline".into(), serde_json::json!("China Airlines"));
        f.insert("departure_code".into(), serde_json::json!("TPE"));
        let plan = Plan {
            flights: vec![f],
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        // (c) empty flight_number → no <a tag in the flight value.
        assert!(
            !html.contains("<a "),
            "unexpected anchor for empty flight number; got: {html}"
        );
        assert!(html.contains("China Airlines"), "got: {html}");
    }

    #[test]
    fn hotel_notes_behind_details_and_zh_name() {
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("Hotel Aqua Citta Naha"));
        hotel.insert("name_zh".into(), serde_json::json!("那霸水都飯店"));
        hotel.insert("check_in".into(), serde_json::json!("2026-06-21 15:00"));
        hotel.insert(
            "notes".into(),
            serde_json::json!("CFM 1234567 cancellation by 2026-06-14"),
        );
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
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
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
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
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(html.contains("<li>Standard twin</li>"));
        assert!(!html.contains("<li></li>")); // no empty bullets
    }

    #[test]
    fn hotel_voucher_link_renders_with_href_and_target() {
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        hotel.insert(
            "voucher_url".into(),
            serde_json::json!("/voucher/okinawa-2026/azat-voucher.pdf"),
        );
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "en", Some("3b9412d0fa2b9961d80a044cab0ebbf4"));
        assert!(html.contains("class=\"voucher-link\""));
        // Gated route → href must carry the page token.
        assert!(html.contains(
            "href=\"/voucher/okinawa-2026/azat-voucher.pdf?token=3b9412d0fa2b9961d80a044cab0ebbf4\""
        ));
        assert!(html.contains("target=\"_blank\""));
        assert!(html.contains("rel=\"noopener\""));
        assert!(html.contains("Hotel voucher (PDF)")); // en label
    }

    #[test]
    fn hotel_voucher_link_echoes_loading_token() {
        // The href echoes WHATEVER token loaded the page (owner or share),
        // proving it is the request token and not a hardcoded one.
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        hotel.insert(
            "voucher_url".into(),
            serde_json::json!("/voucher/okinawa-2026/azat-voucher.pdf"),
        );
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "en", Some("dd90508f2efd063ee760197d127fffa4"));
        assert!(html.contains(
            "href=\"/voucher/okinawa-2026/azat-voucher.pdf?token=dd90508f2efd063ee760197d127fffa4\""
        ));
    }

    #[test]
    fn hotel_voucher_link_without_token_has_no_query() {
        // No token (shouldn't happen for a gated page) → bare href, no ?token=.
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        hotel.insert(
            "voucher_url".into(),
            serde_json::json!("/voucher/okinawa-2026/azat-voucher.pdf"),
        );
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(html.contains("href=\"/voucher/okinawa-2026/azat-voucher.pdf\""));
        assert!(!html.contains("?token="));
    }

    #[test]
    fn hotel_without_voucher_url_renders_no_link() {
        let mut hotel = Row::new();
        hotel.insert("name".into(), serde_json::json!("HOTEL AZAT NAHA"));
        let plan = Plan {
            hotel: Some(hotel),
            ..Default::default()
        };
        let html = render(&plan, "en", None);
        assert!(!html.contains("voucher-link"));
    }

    #[test]
    fn empty_plan_renders_no_section_headings() {
        let plan = Plan::default();
        let html = render(&plan, "en", None);
        assert!(!html.contains("Flights"));
        assert!(!html.contains("Hotel"));
        assert!(!html.contains("Transfers"));
    }

    #[test]
    fn summary_section_carries_dashed_box_class() {
        let plan = Plan::default();
        let html = render(&plan, "en", None);
        assert!(html.contains("<section class=\"booking-summary summary-box\">"));
    }

    #[test]
    fn booked_domestic_stay_shows_green_badge_and_icon_title() {
        use crate::model::DomesticStay;
        let plan = Plan {
            p4_status: "booked".into(),
            domestic_stays: vec![DomesticStay {
                title: "海論 海景雙人房".into(),
                hotel_name: "海論".into(),
                room_type: "海景雙人房".into(),
                price_twd: 5200,
                selected_date: "2026-10-12".into(),
                status: "booked".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(html.contains("🏠 已訂住宿"), "booked h2 title, got: {html}");
        assert!(html.contains("domestic--booked"), "green booked card class");
        assert!(html.contains("booked-badge"), "已訂 badge");
        assert!(html.contains("✓ 已訂"));
    }

    #[test]
    fn selecting_candidates_show_icon_title_and_dashed_cards() {
        use crate::model::DomesticCandidate;
        let plan = Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "海論".into(),
                room_type: "海景雙人房".into(),
                price_twd: 5200,
                sea_view: 1,
                breakfast_included: 1,
                image_url: "https://example.com/a.webp".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(html.contains("🏨 海景候選 · 正在選"), "selecting h2, got: {html}");
        assert!(html.contains("candidate-card--selecting"), "dashed card class");
        assert!(!html.contains("已訂住宿"));
    }

    #[test]
    fn booked_candidates_lose_dashed_frame_and_show_reference_sub() {
        use crate::model::DomesticCandidate;
        let plan = Plan {
            p4_status: "booked".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "海論".into(),
                room_type: "海景雙人房".into(),
                image_url: "https://example.com/a.webp".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(html.contains("其他海景參考"));
        assert!(html.contains("僅供參考"));
        assert!(!html.contains("candidate-card--selecting"));
    }

    #[test]
    fn candidate_gallery_renders_labeled_thumbs_and_room_link() {
        use crate::model::{CandidateImage, DomesticCandidate};
        let plan = Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "海論".into(),
                room_type: "海景雙人房".into(),
                image_url: "https://example.com/main.webp".into(),
                link_url: "https://example.com/rooms".into(),
                images: vec![
                    CandidateImage {
                        image_url: "https://example.com/quad.webp".into(),
                        label: "海景高級四人房".into(),
                    },
                    CandidateImage {
                        image_url: "https://example.com/common.webp".into(),
                        label: "公區".into(),
                    },
                    // placeholder/empty urls are skipped
                    CandidateImage {
                        image_url: "".into(),
                        label: "不該出現".into(),
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(html.contains("candidate-gallery"), "gallery wrapper");
        assert!(html.contains("https://example.com/quad.webp"));
        assert!(html.contains("海景高級四人房"));
        assert!(html.contains("公區"));
        assert!(!html.contains("不該出現"), "placeholder gallery rows skipped");
        assert_eq!(html.matches("candidate-gallery-img").count(), 2);
        // thumbs link to the full image in a new tab
        assert!(html.contains("target=\"_blank\""));
        // 查看更多房型 external link
        assert!(html.contains("class=\"candidate-link\""));
        assert!(html.contains("href=\"https://example.com/rooms\""));
        assert!(html.contains("查看更多房型 ↗"));
    }

    #[test]
    fn candidate_without_gallery_or_link_renders_neither() {
        use crate::model::DomesticCandidate;
        let plan = Plan {
            p4_status: "selecting".into(),
            candidates: vec![DomesticCandidate {
                id: "c1".into(),
                hotel_name: "CHLIV".into(),
                room_type: "海景雙人房".into(),
                image_url: "https://example.com/a.webp".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let html = render(&plan, "zh", None);
        assert!(!html.contains("candidate-gallery"));
        assert!(!html.contains("candidate-link"));
    }
}
