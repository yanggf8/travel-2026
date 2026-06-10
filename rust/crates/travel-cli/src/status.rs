// `travel status [--full]` — port of src/cli/commands/status.ts. Read-only,
// plain-text. Mirrors the TS line-by-line including:
//   - the box-drawing header (60 chars wide)
//   - the literal status-icon map (TS uses `❓` as the default fallback)
//   - `formatDate()`-style date strings ("Fri, Feb 13, 2026")
//   - `¥2,720`-style locale thousands separators (en-US)
//   - the 6 process rows in TS order
//   - the activity line padding (session=9, time=11, "  " indent)
//
// The reader (`plan.rs`) is the minimal-slice plan reader; this formatter
// is byte-for-byte parity-checked against `TRAVEL_PLAN_ID=<id> npm run view:status`.

use crate::plan::{self, ActivityView, PlanView};

const PROCESSES: &[(&str, &str)] = &[
    ("process_1_date_anchor", "P1 Date Anchor"),
    ("process_2_destination", "P2 Destination"),
    ("process_3_4_packages", "P3+4 Packages"),
    ("process_3_transportation", "P3 Transport"),
    ("process_4_accommodation", "P4 Accommodation"),
    ("process_5_daily_itinerary", "P5 Itinerary"),
];

const SESSIONS_IN_ORDER: &[&str] = &["morning", "noon", "afternoon", "evening"];

pub async fn run(args: &[String], full: bool) -> Result<(), String> {
    let plan_id = crate::plan_resolver::resolve_plan_id(args).await?;
    let view = plan::load(&plan_id).await?;
    print!("{}", render(&view, full));
    Ok(())
}

fn render(view: &PlanView, full: bool) -> String {
    let mut out = String::new();
    // Header (box + blank line).
    out.push('\n');
    out.push_str("╔════════════════════════════════════════════════════════════╗\n");
    out.push_str("║              TRAVEL PLAN STATUS                            ║\n");
    out.push_str("╚════════════════════════════════════════════════════════════╝\n\n");

    out.push_str(&format!("Active Destination: {}\n", view.active_destination));

    if let Some(d) = &view.dates {
        out.push_str(&format!(
            "Travel Dates: {} → {} ({} days)\n",
            format_date(&d.start_date),
            format_date(&d.end_date),
            d.days
        ));
    }

    out.push('\n');
    out.push_str("Process Status:\n");
    out.push_str(&"─".repeat(50));
    out.push('\n');

    for (pid, name) in PROCESSES {
        let status = view
            .process_status
            .get(*pid)
            .map(String::as_str)
            .unwrap_or("pending");
        let icon = status_icon(status);
        let dirty = view.dirty_flags.get(*pid).copied().unwrap_or(false);
        let dirty_flag = if dirty { " ⚠️ DIRTY" } else { "" };
        out.push_str(&format!(
            "  {} {:<20} {}{}\n",
            icon, name, status, dirty_flag
        ));
    }

    // NOTE: TS quirk to mirror. The TS `showStatus` only prints a
    // "Selected Offer" section when `process_3_4_packages.chosen_offer` is set
    // in the in-memory plan object — that field is populated only by the
    // mutator (`setOfferSelection()`) and is *not* rehydrated from
    // `plan_offer_selection` by the assembler. On a fresh DB read (e.g.
    // fresh `npm run view:status`), `chosen_offer` is null and the section
    // is not rendered. We replicate that quirk for byte-parity.
    // `view.selected_offer` is still read for the next view port.

    if full {
        render_flight(&mut out, view);
        render_transfers(&mut out, view);
        render_hotel(&mut out, view);
        render_fixed_activities(&mut out, view);
    }

    // Trailing blank lines from `console.log('\n')` in TS — two `\n`s.
    out.push('\n');
    out.push('\n');
    out
}

fn render_flight(out: &mut String, view: &PlanView) {
    let Some(flight) = &view.flight else { return };
    let Some(outbound) = &flight.outbound else { return };
    if outbound.flight_number.is_empty() && outbound.departure_airport_code.is_empty() {
        return;
    }
    out.push('\n');
    out.push_str("Flight Details:\n");
    out.push_str(&"─".repeat(50));
    out.push('\n');
    // Header line: "[airline_code, outbound.flight_number].filter(Boolean).join(' ')"
    // + " (airline)" if airline is non-empty.
    let mut prefix_parts: Vec<&str> = Vec::new();
    if !flight.airline_code.is_empty() {
        prefix_parts.push(&flight.airline_code);
    }
    if !outbound.flight_number.is_empty() {
        prefix_parts.push(&outbound.flight_number);
    }
    let prefix = prefix_parts.join(" ");
    if !flight.airline.is_empty() {
        out.push_str(&format!("  {} ({})\n", prefix, flight.airline));
    } else {
        out.push_str(&format!("  {}\n", prefix));
    }
    // Route line: "<dep> <dep_time> → <arr> <arr_time>" (terminal shown if present).
    out.push_str(&format!(
        "  {} {} → {} {}\n",
        fmt_ap(&outbound.departure_airport_code, &outbound.departure_terminal),
        outbound.departure_time,
        fmt_ap(&outbound.arrival_airport_code, &outbound.arrival_terminal),
        outbound.arrival_time,
    ));
    if let Some(inbound) = &flight.inbound
        && (!inbound.flight_number.is_empty() || !inbound.departure_airport_code.is_empty())
    {
        out.push_str(&format!("  Return: {}\n", inbound.flight_number));
        out.push_str(&format!(
            "  {} {} → {} {}\n",
            fmt_ap(&inbound.departure_airport_code, &inbound.departure_terminal),
            inbound.departure_time,
            fmt_ap(&inbound.arrival_airport_code, &inbound.arrival_terminal),
            inbound.arrival_time,
        ));
    }
}

fn fmt_ap(code: &str, term: &str) -> String {
    if term.is_empty() {
        code.to_string()
    } else {
        format!("{code} {term}")
    }
}

fn render_transfers(out: &mut String, view: &PlanView) {
    if !view.transfers.contains_key("arrival") && !view.transfers.contains_key("departure") {
        return;
    }
    out.push('\n');
    out.push_str("Airport Transfers:\n");
    out.push_str(&"─".repeat(50));
    out.push('\n');
    for dir in ["arrival", "departure"] {
        let Some(seg) = view.transfers.get(dir) else { continue };
        let label = if dir == "arrival" { "Arrival" } else { "Departure" };
        let status = if seg.status.is_empty() { "planned" } else { &seg.status };
        // TS: `console.log('\n  ${label} (${segStatus})')` — the leading `\n`
        // is the blank line that separates each direction's block.
        out.push('\n');
        out.push_str(&format!("  {label} ({status})\n"));

        if let Some(sel) = &seg.selected {
            let mut line = format!("   ✓ {}", sel.title);
            if let Some(p) = sel.price_yen {
                line.push_str(&format!(" (¥{})", locale_i64(p)));
            }
            if let Some(d) = sel.duration_min {
                line.push_str(&format!(" ~{d} min"));
            }
            line.push('\n');
            out.push_str(&line);
            if !sel.route.is_empty() {
                out.push_str(&format!("     {}\n", sel.route));
            }
            if !sel.schedule.is_empty() {
                out.push_str(&format!("     Schedule: {}\n", sel.schedule));
            }
        }

        if !seg.candidates.is_empty() {
            out.push_str("   Candidates:\n");
            for c in seg.candidates.iter().take(5) {
                let mut line = format!("    - {}", c.title);
                if let Some(p) = c.price_yen {
                    line.push_str(&format!(" (¥{})", locale_i64(p)));
                }
                if let Some(d) = c.duration_min {
                    line.push_str(&format!(" ~{d} min"));
                }
                if !c.route.is_empty() {
                    line.push_str(&format!(" — {}", c.route));
                }
                line.push('\n');
                out.push_str(&line);
            }
            if seg.candidates.len() > 5 {
                out.push_str(&format!(
                    "    ... and {} more\n",
                    seg.candidates.len() - 5
                ));
            }
        }
    }
}

fn render_hotel(out: &mut String, view: &PlanView) {
    let Some(hotel) = &view.hotel else { return };
    if hotel.name.is_empty() {
        return;
    }
    out.push('\n');
    out.push_str("Hotel Details:\n");
    out.push_str(&"─".repeat(50));
    out.push('\n');
    out.push_str(&format!("  {}\n", hotel.name));
    if !hotel.access.is_empty() {
        let first4: Vec<&str> = hotel.access.iter().take(4).map(String::as_str).collect();
        out.push_str(&format!("  Access: {}\n", first4.join(", ")));
    }
    // `Includes:` is rendered by TS only when `chosenOffer.includes` is a
    // non-empty array; on a fresh read `chosen_offer` is null (see the
    // Selected Offer note above), so this line is intentionally never
    // emitted. `view.offer_includes` is still read for the next view port.
}

fn render_fixed_activities(out: &mut String, view: &PlanView) {
    let mut fixed: Vec<FixedActivity> = Vec::new();
    for d in &view.days {
        for session_name in SESSIONS_IN_ORDER {
            let Some(s) = d.sessions.get(*session_name) else { continue };
            for a in &s.activities {
                if is_fixed_or_reservation(a) {
                    fixed.push(FixedActivity {
                        day: d.day_number,
                        session: session_name.to_string(),
                        title: a.title.clone(),
                        start: a.start_time.clone(),
                        end: a.end_time.clone(),
                        session_start: s.time_range_start.clone(),
                        session_end: s.time_range_end.clone(),
                        booking_status: a.booking_status.clone(),
                        booking_ref: a.booking_ref.clone(),
                        booking_required: a.booking_required,
                    });
                }
            }
        }
    }
    if fixed.is_empty() {
        return;
    }
    out.push('\n');
    out.push_str("Fixed-Time Activities & Reservations:\n");
    out.push_str(&"─".repeat(50));
    out.push('\n');

    // sort: day, session-order, title
    let session_order: std::collections::HashMap<&str, i32> = SESSIONS_IN_ORDER
        .iter()
        .enumerate()
        .map(|(i, s)| (*s, i as i32))
        .collect();
    fixed.sort_by(|a, b| {
        a.day
            .cmp(&b.day)
            .then(
                session_order
                    .get(a.session.as_str())
                    .copied()
                    .unwrap_or(99)
                    .cmp(
                        &session_order
                            .get(b.session.as_str())
                            .copied()
                            .unwrap_or(99),
                    ),
            )
            .then(a.title.cmp(&b.title))
    });

    for fa in &fixed {
        let time_str = if !fa.start.is_empty() && !fa.end.is_empty() {
            format!("{}-{}", fa.start, fa.end)
        } else if !fa.start.is_empty() {
            fa.start.clone()
        } else if !fa.end.is_empty() {
            format!("by {}", fa.end)
        } else if !fa.session_start.is_empty() && !fa.session_end.is_empty() {
            format!("{}-{}", fa.session_start, fa.session_end)
        } else {
            String::new()
        };

        let icon = if fa.booking_status == "booked" {
            "🎫"
        } else if fa.booking_required
            || fa.booking_status == "pending"
            || fa.booking_status == "waitlist"
        {
            "⏳"
        } else {
            // Mirrors the TS `📌` for fixed-time + default fallback
            // (`isFixedTime ? '📌' : '📌'` — the TS has identical branches
            // in both arms; the Rust port collapses them but keeps the
            // behavior).
            "📌"
        };
        let ref_str = if fa.booking_ref.is_empty() {
            String::new()
        } else {
            format!(" [{}]", fa.booking_ref)
        };
        out.push_str(&format!(
            "  {} Day {} {:<9} {:<11} {}{}\n",
            icon, fa.day, fa.session, time_str, fa.title, ref_str
        ));
    }
}

struct FixedActivity {
    day: i64,
    session: String,
    title: String,
    start: String,
    end: String,
    session_start: String,
    session_end: String,
    booking_status: String,
    booking_ref: String,
    booking_required: bool,
}

fn is_fixed_or_reservation(a: &ActivityView) -> bool {
    if a.is_fixed_time {
        return true;
    }
    if a.booking_required {
        return true;
    }
    matches!(a.booking_status.as_str(), "booked" | "pending" | "waitlist")
}

fn status_icon(status: &str) -> &'static str {
    match status {
        "pending" => "⏳",
        "researching" => "🔍",
        "researched" => "📋",
        "selecting" => "🎯",
        "selected" => "✅",
        "populated" => "📦",
        "booking" => "💳",
        "booked" => "🎫",
        "confirmed" => "✓",
        "skipped" => "⏭️",
        _ => "❓",
    }
}

/// `formatDate()` parity: matches `Date.toLocaleDateString('en-US', { weekday: 'short', month: 'short', day: 'numeric', year: 'numeric' })`.
///
/// Output: `Fri, Feb 13, 2026` (comma after weekday, regular space, abbreviated
/// month, numeric day, comma+year). Built from a hand-rolled weekday map (the
/// pure Rust equivalent of JS Intl with en-US locale for these 4 fields).
pub(crate) fn format_date(date_str: &str) -> String {
    use chrono::{Datelike, NaiveDate};
    let d = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .unwrap_or_else(|_| NaiveDate::from_ymd_opt(1970, 1, 1).expect("epoch"));
    let weekday = match d.weekday().num_days_from_sunday() {
        0 => "Sun",
        1 => "Mon",
        2 => "Tue",
        3 => "Wed",
        4 => "Thu",
        5 => "Fri",
        6 => "Sat",
        _ => "???",
    };
    let month = match d.month() {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "???",
    };
    format!("{}, {} {}, {}", weekday, month, d.day(), d.year())
}

/// en-US thousands separator (matches `Number.toLocaleString()` for non-negative
/// integers). `2720 -> "2,720"`, `450 -> "450"`, `1000000 -> "1,000,000"`.
pub(crate) fn locale_i64(n: i64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let neg = n < 0;
    let digits = n.unsigned_abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::new();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    if neg { format!("-{}", out) } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_date_parity() {
        // Anchors: 2026-02-13 = Friday, 2026-02-17 = Tuesday, 2026-02-24 = Tuesday, 2026-02-28 = Saturday.
        assert_eq!(format_date("2026-02-13"), "Fri, Feb 13, 2026");
        assert_eq!(format_date("2026-02-17"), "Tue, Feb 17, 2026");
        assert_eq!(format_date("2026-02-24"), "Tue, Feb 24, 2026");
        assert_eq!(format_date("2026-02-28"), "Sat, Feb 28, 2026");
        // 2026-01-01 is a Thursday.
        assert_eq!(format_date("2026-01-01"), "Thu, Jan 1, 2026");
    }

    #[test]
    fn test_locale_i64() {
        assert_eq!(locale_i64(0), "0");
        assert_eq!(locale_i64(450), "450");
        assert_eq!(locale_i64(2720), "2,720");
        assert_eq!(locale_i64(1_520), "1,520");
        assert_eq!(locale_i64(1_000_000), "1,000,000");
        assert_eq!(locale_i64(-2720), "-2,720");
    }

    #[test]
    fn test_status_icon() {
        assert_eq!(status_icon("confirmed"), "✓");
        assert_eq!(status_icon("booked"), "🎫");
        assert_eq!(status_icon("pending"), "⏳");
        assert_eq!(status_icon("researched"), "📋");
        assert_eq!(status_icon("not-a-status"), "❓");
    }

    #[test]
    fn test_fmt_ap() {
        assert_eq!(fmt_ap("NRT", "T2"), "NRT T2");
        assert_eq!(fmt_ap("NRT", ""), "NRT");
        assert_eq!(fmt_ap("", "T2"), " T2"); // matches TS behavior (term shown even if code empty)
    }
}
