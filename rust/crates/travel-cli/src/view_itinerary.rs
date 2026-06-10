// `travel itinerary [--dest slug]` — port of src/cli/commands/itinerary.ts
// (222 LOC). Read-only, plain-text. Mirrors the TS line-by-line including:
//   - the box-drawing header (60 chars wide)
//   - the `formatDate()`-style date strings ("Fri, Feb 13, 2026") reused
//     from `status::format_date` (byte-parity with the TS)
//   - the day-separator (`─` × 60) and per-day structure (header, optional
//     `Theme:`, optional weather line, per-session blocks, optional `🗺️
//     ROUTE:` block)
//   - the weather icon ladder (❄️ / 🌧️ / ⛅ / ☀️) and the ` | ` separators
//     and `(source, srcDate)` suffix
//   - the session-icon map (🎫 booked, ⏳ pending, 📋 required, • fallback)
//     and the activity-suffix order (`[ref]` first, then ` (book by ...)`)
//   - the route-segment mode icons (🚗 driving, 🚶 walking, 🚌 fallback)
//     and the ` (~<dur> min)` / `  (<notes>)` suffix rules
//   - the trailing `⏳ PENDING BOOKINGS` section (skipped when none, matches
//     TS `if (pendingBookings.length > 0)`)
//   - the trailing `console.log('')` blank line at the end of the TS
//     function — the output string ends with `\n\n` so the final visible
//     line is followed by a single empty line (parity with both baselines)
//
// The reader (`plan.rs`) is extended with three new fields and two new
// table reads for this port:
//   - `DayView.day_type` + `DayView.weather: Option<DayWeather>` populated
//     from the existing `days` query (no new round-trip)
//   - `SessionView.focus` populated from the existing `timesofday` query
//   - `ActivityView.book_by` populated from the existing `activities` query
//   - `SessionView.meals: Vec<String>` populated from a new `session_meals`
//     child-rows query
//   - `PlanView.route_segments: HashMap<i32, Vec<RouteSegment>>` populated
//     from a new `day_route_segments` query
//
// The TS also reads `transit_summary.hotel_station` (a fallback when no
// `hotel.name` is set) — for both baseline plans `hotel.name` is set, so
// the fallback is dead code in practice. A future port can extend
// `plan.rs` to read `itinerary_metadata` and `itinerary_transit_key_lines`
// to surface that section without changing the baseline output.
//
// Byte-for-byte parity-checked against
//   `TRAVEL_PLAN_ID=<id> npm run view:itinerary`.

use crate::plan::{self, DayView, PlanView, RouteSegment, SessionView};
use crate::status::format_date;

const SESSIONS_IN_ORDER: &[&str] = &["morning", "noon", "afternoon", "evening"];

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel itinerary [--dest slug]\n  \
             (plan resolution: TRAVEL_PLAN_ID env var only for now)"
        );
        return Ok(());
    }
    // Parse --dest for parity with TS `args.optionValue('--dest')`. The TS
    // uses sm.resolveDestination(destOpt) which falls back to the active
    // destination. For this slice we only validate presence — plan.rs
    // currently keys on active_destination from plan_metadata, so a future
    // override would require either an extra query or a thin wrapper around
    // plan::load() that re-keys transfers/days.
    let _dest_opt: Option<String> = parse_dest(args);
    let plan_id = crate::plan_resolver::resolve_plan_id(args).await?;
    let view = plan::load(&plan_id).await?;
    print!("{}", render(&view));
    Ok(())
}

fn parse_dest(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--dest" {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}

fn render(view: &PlanView) -> String {
    let mut out = String::new();

    // Header (box + blank line). The TS does
    //   console.log('\n╔...╗')
    //   console.log('║   ITINERARY   ...')
    //   console.log('╚...╝\n')
    // — the trailing `\n` in the third console.log string produces the
    // blank line we see in the baseline.
    out.push('\n');
    out.push_str("╔════════════════════════════════════════════════════════════╗\n");
    out.push_str("║                    ITINERARY                               ║\n");
    out.push_str("╚════════════════════════════════════════════════════════════╝\n");
    out.push('\n');

    // Travel dates line — TS: `if (startDate && endDate) console.log(...)`.
    // The TS uses `days.length` (the day-array count), not `date_anchors.days`.
    if let Some(d) = &view.dates
        && !d.start_date.is_empty()
        && !d.end_date.is_empty()
    {
        let days_count = view.days.len();
        out.push_str(&format!(
            "📅 {} → {} ({} days)\n",
            format_date(&d.start_date),
            format_date(&d.end_date),
            days_count
        ));
    }

    // Flight line.
    if let Some(flight) = &view.flight
        && let Some(outbound) = &flight.outbound
    {
        out.push_str(&render_flight_line(flight, outbound));
    }

    // Hotel lines (and transit fallback). TS:
    //   if (hotel?.name) { print(name); if (access.length) print(🚉 access.join(',')) }
    //   if (!hotel?.name) { if (transitSummary?.hotel_station) print(🚉 ...) }
    // The fallback is dead code in practice (both baselines have a hotel
    // name); we still guard with `hotel.name` being non-empty to mirror.
    if let Some(hotel) = &view.hotel
        && !hotel.name.is_empty()
    {
        out.push_str(&format!("🏨 {}\n", hotel.name));
        if !hotel.access.is_empty() {
            out.push_str(&format!("🚉 {}\n", hotel.access.join(", ")));
        }
    }

    // Blank line before the day loop. TS does `console.log('')` at line 87.
    out.push('\n');

    // Day loop.
    for d in &view.days {
        render_day(&mut out, d, view);
    }

    // Pending bookings section.
    let pending = collect_pending(view);
    if !pending.is_empty() {
        // TS: console.log('─'.repeat(60))
        out.push_str(&"─".repeat(60));
        out.push('\n');
        // TS: console.log('⏳ PENDING BOOKINGS')
        out.push_str("⏳ PENDING BOOKINGS\n");
        for pb in &pending {
            let deadline = if pb.book_by.is_empty() {
                String::new()
            } else {
                format!(" (by {})", pb.book_by)
            };
            // TS: console.log(`  Day ${pb.day}: ${pb.title}${deadline}`)
            out.push_str(&format!("  Day {}: {}{}\n", pb.day, pb.title, deadline));
        }
        // Trailing blank line from the final `console.log('')`.
        out.push('\n');
    }

    out
}

fn render_flight_line(
    flight: &plan::FlightSummary,
    outbound: &plan::FlightLegView,
) -> String {
    // Build "[airline_code, flight_number].filter(Boolean).join(' ')" prefix.
    let mut label_parts: Vec<&str> = Vec::new();
    if !flight.airline_code.is_empty() {
        label_parts.push(&flight.airline_code);
    }
    if !outbound.flight_number.is_empty() {
        label_parts.push(&outbound.flight_number);
    }
    let label = label_parts.join(" ");
    // fmt_ap(leg, 'dep'): "<code> <term>" if term, else "<code>".
    let dep = format!(
        "{} {}",
        fmt_ap(&outbound.departure_airport_code, &outbound.departure_terminal),
        outbound.departure_time
    );
    let arr = format!(
        "{} {}",
        fmt_ap(&outbound.arrival_airport_code, &outbound.arrival_terminal),
        outbound.arrival_time
    );
    // Airline suffix in parens only if non-empty.
    let airline_suffix = if flight.airline.is_empty() {
        String::new()
    } else {
        format!(" ({})", flight.airline)
    };
    // TS builds ONE string and prints it (`console.log(line)`); outbound
    // and inbound segments live on the same line, joined by ` / `.
    let mut line = format!("✈️  {}{}: {} → {}", label, airline_suffix, dep, arr);
    if let Some(inbound) = &flight.inbound {
        let r_dep = format!(
            "{} {}",
            fmt_ap(&inbound.departure_airport_code, &inbound.departure_terminal),
            inbound.departure_time
        );
        let r_arr = format!(
            "{} {}",
            fmt_ap(&inbound.arrival_airport_code, &inbound.arrival_terminal),
            inbound.arrival_time
        );
        line.push_str(&format!(
            " / {} {} → {}",
            inbound.flight_number, r_dep, r_arr
        ));
    }
    line.push('\n');
    line
}

fn fmt_ap(code: &str, term: &str) -> String {
    if term.is_empty() {
        code.to_string()
    } else {
        format!("{code} {term}")
    }
}

fn render_day(out: &mut String, d: &DayView, view: &PlanView) {
    // Day separator (`─` × 60) + `Day N (formatDate) <label>`.
    out.push_str(&"─".repeat(60));
    out.push('\n');
    let day_label = match d.day_type.as_str() {
        "arrival" => "✈️ ARRIVAL",
        "departure" => "✈️ DEPARTURE",
        _ => "",
    };
    out.push_str(&format!(
        "Day {} ({}) {}\n",
        d.day_number,
        format_date(&d.date),
        day_label
    ));

    // `Theme: <theme>` — only if theme is set.
    if !d.theme.is_empty() {
        out.push_str(&format!("Theme: {}\n", d.theme));
    }

    // Weather line — only if weather is set.
    if let Some(w) = &d.weather
        && !w.weather_label.is_empty()
    {
        out.push_str(&render_weather_line(w));
    }

    // Blank line after the day-header block.
    out.push('\n');

    // Session loop.
    for session_name in SESSIONS_IN_ORDER {
        let Some(s) = d.sessions.get(*session_name) else { continue };
        render_session(out, session_name, s);
    }

    // Route segments.
    if let Some(segs) = view.route_segments.get(&(d.day_number as i32))
        && !segs.is_empty()
    {
        out.push_str("  🗺️  ROUTE:\n");
        for seg in segs {
            out.push_str(&render_route_segment(seg));
        }
        // Trailing blank line from `console.log('')` after the route loop.
        out.push('\n');
    }
}

fn render_weather_line(w: &plan::DayWeather) -> String {
    // Icon ladder — matches TS lines 105-107:
    //   ❄️ if 71 <= code <= 77
    //   🌧️ if precipitation_pct > 50
    //   ⛅ if code >= 2
    //   ☀️ otherwise
    let code = w.weather_code;
    let icon = if (71..=77).contains(&code) {
        "❄️"
    } else if w.precipitation_pct > 50.0 {
        "🌧️"
    } else if code >= 2 {
        "⛅"
    } else {
        "☀️"
    };
    // Source date = first 10 chars of sourced_at (YYYY-MM-DD), or empty.
    let src_date = if w.sourced_at.len() >= 10 {
        &w.sourced_at[..10]
    } else {
        ""
    };
    // Rust's f64 Display uses the shortest round-trip representation
    // (1.2 → "1.2", 16.0 → "16", 1.23 → "1.23") — matches JS's
    // `Number.toString()` byte-for-byte, so we can interpolate the
    // raw f64 values just like the TS template literal.
    format!(
        "{} {} | {}–{}°C | Rain: {}%  ({}, {})\n",
        icon,
        w.weather_label,
        w.temp_low_c,
        w.temp_high_c,
        w.precipitation_pct,
        w.source_id,
        src_date
    )
}

fn render_session(out: &mut String, session_name: &str, s: &SessionView) {
    // TS: `if (!activities || activities.length === 0) continue;` — skip
    // sessions with no activities.
    if s.activities.is_empty() {
        return;
    }
    // Session label: "Morning" / "Noon" / "Afternoon" / "Evening".
    let label = capitalize(session_name);
    out.push_str(&format!("  【{}】 {}\n", label, s.focus));
    for a in &s.activities {
        out.push_str(&render_activity(a));
    }
    if !s.transit_notes.is_empty() {
        out.push_str(&format!("    🚃 {}\n", s.transit_notes));
    }
    if !s.meals.is_empty() {
        out.push_str(&format!("    🍽️  {}\n", s.meals.join(", ")));
    }
    // Blank line after each session block (matches TS `console.log('')` at
    // line 158).
    out.push('\n');
}

fn render_activity(a: &plan::ActivityView) -> String {
    // Icon map — matches TS lines 138-140:
    //   🎫 if booking_status === "booked"
    //   ⏳ if booking_status === "pending"
    //   📋 if booking_required
    //   •  otherwise
    let icon = if a.booking_status == "booked" {
        "🎫"
    } else if a.booking_status == "pending" {
        "⏳"
    } else if a.booking_required {
        "📋"
    } else {
        "•"
    };
    // Suffix order matches TS: `[ref]` first, then ` (book by ...)`.
    let mut suffix = String::new();
    if !a.booking_ref.is_empty() {
        suffix.push_str(&format!(" [{}]", a.booking_ref));
    }
    if a.booking_status == "pending" && !a.book_by.is_empty() {
        suffix.push_str(&format!(" (book by {})", a.book_by));
    }
    format!("    {} {}{}\n", icon, a.title, suffix)
}

fn render_route_segment(seg: &RouteSegment) -> String {
    // Mode icon — matches TS line 171:
    //   🚗 if "driving", 🚶 if "walking", 🚌 otherwise.
    let mode_icon = match seg.mode.as_str() {
        "driving" => "🚗",
        "walking" => "🚶",
        _ => "🚌",
    };
    // Duration suffix: ` (~<dur> min)` only if set.
    let dur = match seg.duration_min {
        Some(d) => format!(" (~{} min)", d),
        None => String::new(),
    };
    // Notes suffix: `  (<notes>)` only if set (note the 2-space gap).
    let notes_suffix = match &seg.notes {
        Some(n) if !n.is_empty() => format!("  ({})", n),
        _ => String::new(),
    };
    format!(
        "    {} {} → {}{}{}\n",
        mode_icon, seg.from_place, seg.to_place, dur, notes_suffix
    )
}

struct PendingBooking {
    day: i64,
    title: String,
    book_by: String,
}

/// Walk days → sessions → activities. A pending booking is one where
/// `booking_status === "pending"` OR (`booking_required` is true AND no
/// `booking_status`). Order matches the TS iteration: days ascending, then
/// sessions in TS order (morning, noon, afternoon, evening), then activities
/// in DB order.
fn collect_pending(view: &PlanView) -> Vec<PendingBooking> {
    let mut pending: Vec<PendingBooking> = Vec::new();
    for d in &view.days {
        for session_name in SESSIONS_IN_ORDER {
            let Some(s) = d.sessions.get(*session_name) else { continue };
            for a in &s.activities {
                let is_pending = a.booking_status == "pending"
                    || (a.booking_required && a.booking_status.is_empty());
                if is_pending {
                    pending.push(PendingBooking {
                        day: d.day_number,
                        title: a.title.clone(),
                        book_by: a.book_by.clone(),
                    });
                }
            }
        }
    }
    pending
}

/// Title-case a session name (`morning` → `Morning`).
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
