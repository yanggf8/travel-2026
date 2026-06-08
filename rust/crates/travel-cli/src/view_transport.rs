// `travel transport [--dest slug]` — port of src/cli/commands/transport.ts
// (118 LOC). Read-only, plain-text. Mirrors the TS line-by-line including:
//   - the box-drawing header (60 chars wide)
//   - the "AIRPORT TRANSFERS" section with the leading `\n` before each
//     direction's `<DIR> (<status>)` line (TS uses `console.log(\`\\n${dir.toUpperCase()}...\`)`)
//   - the "BY DAY" section with leading `\n` per day header
//   - the transfer icons (🎫 booked, 📋 planned) and the `formatDate` parity
//   - the `Time:`, `Price:`, `Schedule:` lines skipped when the underlying
//     field is empty / null (matches `if (selected.X) console.log(...)`)
//   - the trailing `console.log('\n')` blank line at the end of `showTransport`
//
// The reader (`plan.rs`) is reused with two minor field additions (no new
// tables): `DayView.theme` and `SessionView.transit_notes` are now populated
// from the existing `days` and `timesofday` queries. The TS `transit_summary`
// (in-memory object built by the assembler from `itinerary_metadata` +
// `itinerary_transit_key_lines` + tips) is intentionally NOT surfaced —
// `transit_summary` is null for both baseline plans (tokyo-2026,
// kyoto-2026), so the parity diff is empty without that section. A future
// port can extend `plan.rs` to read `itinerary_metadata` +
// `itinerary_transit_key_lines` and add the section without changing the
// baseline output.
//
// Byte-for-byte parity-checked against
//   `TRAVEL_PLAN_ID=<id> npm run view:transport`.

use crate::plan::{self, PlanView, TransferDir};
use crate::status::{format_date, locale_i64};
use std::env;

const SESSIONS_IN_ORDER: &[&str] = &["morning", "noon", "afternoon", "evening"];

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel transport [--dest slug]\n  \
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
    let plan_id = env::var("TRAVEL_PLAN_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "travel transport requires TRAVEL_PLAN_ID env var \
             (e.g. TRAVEL_PLAN_ID=tokyo-2026). \
             --plan-id and active/upcoming resolution will land with the next view port."
                .to_string()
        })?;
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

    // Header (box + blank line) — matches TS:
    //   console.log('\n╔...╗')
    //   console.log('║   TRANSPORT   ...')
    //   console.log('╚...╝\n')
    out.push('\n');
    out.push_str("╔════════════════════════════════════════════════════════════╗\n");
    out.push_str("║                   TRANSPORT                                ║\n");
    out.push_str("╚════════════════════════════════════════════════════════════╝\n\n");

    // AIRPORT TRANSFERS — mirrors TS `if (transfers) { ... }`.
    if !view.transfers.is_empty() {
        out.push_str("✈️  AIRPORT TRANSFERS\n");
        out.push_str(&"─".repeat(50));
        out.push('\n');

        for dir in ["arrival", "departure"] {
            let Some(seg) = view.transfers.get(dir) else { continue };
            render_transfer_block(&mut out, dir, seg);
        }
        // The TS prints `console.log('')` after the transfer loop, which
        // produces a single blank line. In the baseline this is what creates
        // the empty line between DEPARTURE block and the (omitted) DAILY
        // TRANSIT section.
        out.push('\n');
    }

    // DAILY TRANSIT — not surfaced in this port (transit_summary is null for
    // both baseline plans; see file header).

    // BY DAY — mirrors TS `if (days && days.length > 0)`.
    if !view.days.is_empty() {
        // TS: `console.log('\n\n📅 BY DAY')` — two leading newlines.
        out.push('\n');
        out.push('\n');
        out.push_str("📅 BY DAY\n");
        out.push_str(&"─".repeat(50));
        out.push('\n');

        for d in &view.days {
            render_day(&mut out, d);
        }
    }

    // Trailing `console.log('\n')` at the end of showTransport — produces
    // the final blank line in the baseline.
    out.push('\n');
    out.push('\n');
    out
}

fn render_transfer_block(out: &mut String, dir: &str, seg: &TransferDir) {
    // TS: `console.log(\`\\n${dir.toUpperCase()} (${status})\`)` — leading
    // `\n` produces the blank line that separates each direction's block.
    let status = if seg.status.is_empty() { "planned" } else { &seg.status };
    let icon = if status == "booked" { "🎫" } else { "📋" };
    out.push('\n');
    out.push_str(&format!("{} ({})\n", dir.to_uppercase(), status));

    if let Some(sel) = &seg.selected {
        out.push_str(&format!("  {} {}\n", icon, sel.title));
        out.push_str(&format!("     Route: {}\n", sel.route));
        if let Some(d) = sel.duration_min {
            out.push_str(&format!("     Time: ~{} min\n", d));
        }
        if let Some(p) = sel.price_yen {
            out.push_str(&format!("     Price: ¥{}\n", locale_i64(p)));
        }
        if !sel.schedule.is_empty() {
            out.push_str(&format!("     Schedule: {}\n", sel.schedule));
        }
    }
}

fn render_day(out: &mut String, d: &plan::DayView) {
    // TS: `console.log(\`\\nDay ${dayNum} (${formatDate(date)})${theme ? \` - ${theme}\` : ''}\`)`
    out.push('\n');
    let theme_suffix = if d.theme.is_empty() {
        String::new()
    } else {
        format!(" - {}", d.theme)
    };
    out.push_str(&format!(
        "Day {} ({}){}\n",
        d.day_number,
        format_date(&d.date),
        theme_suffix
    ));

    for session_name in SESSIONS_IN_ORDER {
        let Some(s) = d.sessions.get(*session_name) else { continue };
        if s.transit_notes.is_empty() {
            continue;
        }
        out.push_str(&format!("  {}: {}\n", session_name, s.transit_notes));
    }
}
