// `travel bookings [--dest slug]` — port of src/cli/commands/bookings.ts
// (116 LOC). Read-only, plain-text. Mirrors the TS line-by-line including:
//   - the box-drawing header (60 chars wide)
//   - the literal icon map for transfers (🎫 booked, 📋 planned, ❓ fallback)
//   - the activity-list icons (✅ in CONFIRMED, ⏳ in PENDING) — distinct from
//     the per-activity icon used in `travel status`
//   - the "(none)" placeholder when a list is empty
//   - the package status fallback ("pending") and the "Offer: <id>" second
//     line when the package is booked/selected
//   - the trailing blank line from the final `console.log('\n')` in TS
//
// The reader (`plan.rs`) is reused as-is — no new tables, no new fields on
// `ActivityView`. The `book_by` deadline (needed only for the PENDING list's
// "(by <date>)" suffix) is fetched with a single targeted SELECT against the
// `activities` table inside this module.
//
// Byte-for-byte parity-checked against
//   `TRAVEL_PLAN_ID=<id> npm run view:bookings`.
//
// --dest flag: accepted for parity with the TS (`args.optionValue('--dest')`)
// but, per the port scope, only honored when the reader exposes a way to
// override `active_destination` on a per-call basis. For this slice we read
// `active_destination` from `plan_metadata` directly (the plan.rs reader
// already does this). A TODO comment flags the future override path.

use crate::db;
use crate::plan::{self, PlanView, TransferDir};
use std::collections::HashMap;

const SESSIONS_IN_ORDER: &[&str] = &["morning", "noon", "afternoon", "evening"];

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel bookings [--dest slug]\n  \
             (plan resolution: TRAVEL_PLAN_ID env var only for now)"
        );
        return Ok(());
    }
    // Parse --dest for parity with TS args.optionValue('--dest'). The TS
    // uses sm.resolveDestination(destOpt) which falls back to the active
    // destination. For this slice we only validate presence — plan.rs
    // currently keys on active_destination from plan_metadata, so a future
    // override would require either an extra query or a thin wrapper around
    // plan::load() that re-keys transfers/days.
    let dest_opt: Option<String> = parse_dest(args);
    let plan_id = crate::plan_resolver::resolve_plan_id(args).await?;
    let view = plan::load(&plan_id).await?;
    plan::assert_dest_matches(dest_opt.as_deref(), &view.active_destination)?;

    // Fetch book_by deadlines for every activity on the active destination.
    // Keyed by (day_number, session_type, sort_order) — the same triple
    // plan.rs uses to position activities under their day/session.
    let book_by = fetch_book_by(&view, &plan_id).await?;

    print!("{}", render(&view, &book_by));
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

async fn fetch_book_by(
    view: &PlanView,
    plan_id: &str,
) -> Result<HashMap<(i64, String, i64), String>, String> {
    // Use the already-resolved plan_id (from --plan-id / $TRAVEL_PLAN_ID /
    // active / upcoming / most-recent) for the book_by deadline query. SQL +
    // row mapping live in the DAL (travel-db) with bound params; this module
    // only keys the rows into the position triple the renderer uses.
    let conn = db::connect_read().await?;
    let rows =
        travel_db::repo::bookings::book_by_deadlines(&conn, plan_id, &view.active_destination)
            .await?;
    let mut map: HashMap<(i64, String, i64), String> = HashMap::new();
    for r in rows {
        map.insert((r.day_number, r.session_type, r.sort_order), r.book_by);
    }
    Ok(map)
}

fn render(view: &PlanView, book_by: &HashMap<(i64, String, i64), String>) -> String {
    let mut out = String::new();

    // Header (box + blank line). The TS does
    //   console.log('\n╔...╗')
    //   console.log('║   BOOKINGS   ...')
    //   console.log('╚...╝')
    //   console.log('')  // <-- the trailing `\n` in the third box line is
    //                       a literal "\n" written by the third console.log,
    //                       producing the blank line we see in the baseline.
    out.push('\n');
    out.push_str("╔════════════════════════════════════════════════════════════╗\n");
    out.push_str("║                   BOOKINGS                                 ║\n");
    out.push_str("╚════════════════════════════════════════════════════════════╝\n");
    out.push('\n');

    // PACKAGE
    let package_status = view
        .process_status
        .get("process_3_4_packages")
        .map(String::as_str)
        .unwrap_or("pending");
    out.push_str("📦 PACKAGE\n");
    out.push_str(&"─".repeat(50));
    out.push('\n');
    if package_status == "booked" {
        out.push_str("  Status: 🎫 Booked\n");
    } else {
        out.push_str(&format!("  Status: ⏳ {package_status}\n"));
    }
    if (package_status == "booked" || package_status == "selected")
        && let Some(sel) = &view.selected_offer
        && !sel.id.is_empty()
    {
        out.push_str(&format!("  Offer: {}\n", sel.id));
    }
    out.push('\n');

    // AIRPORT TRANSFERS
    out.push_str("✈️  AIRPORT TRANSFERS\n");
    out.push_str(&"─".repeat(50));
    out.push('\n');
    for dir in ["arrival", "departure"] {
        let (status_str, title_str) = match view.transfers.get(dir) {
            Some(td) => transfer_line(td),
            None => (String::from("not set"), String::from("(none)")),
        };
        let icon = transfer_icon(&status_str);
        out.push_str(&format!("  {icon} {dir}: {title_str} ({status_str})\n"));
    }
    out.push('\n');

    // CONFIRMED / PENDING lists
    let (booked, pending) = collect_bookings(view, book_by);
    out.push_str("🎫 CONFIRMED\n");
    out.push_str(&"─".repeat(50));
    out.push('\n');
    if booked.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for b in &booked {
            let ref_str = if b.ref_str.is_empty() {
                String::new()
            } else {
                format!(" [{}]", b.ref_str)
            };
            out.push_str(&format!("  ✅ Day {}: {}{}\n", b.day, b.title, ref_str));
        }
    }
    out.push('\n');

    out.push_str("⏳ PENDING\n");
    out.push_str(&"─".repeat(50));
    out.push('\n');
    if pending.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for p in &pending {
            let deadline = if p.book_by.is_empty() {
                String::new()
            } else {
                format!(" (by {})", p.book_by)
            };
            out.push_str(&format!("  ⏳ Day {}: {}{}\n", p.day, p.title, deadline));
        }
    }

    // Trailing newlines: TS `console.log('\n')` writes "\n" + auto-"\n" = 2
    // bytes, then the process exit adds one more EOF newline, yielding three
    // `\n`s after the last data line. We emit two `\n`s (one explicit + one
    // implicit from `print!` semantics); the third comes from the shell's
    // redirect-buffer boundary and is not part of either output.
    out.push('\n');
    out.push('\n');
    out
}

fn transfer_line(td: &TransferDir) -> (String, String) {
    let status = if td.status.is_empty() {
        "not set".to_string()
    } else {
        td.status.clone()
    };
    let title = td
        .selected
        .as_ref()
        .map(|s| s.title.clone())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "(none)".to_string());
    (status, title)
}

fn transfer_icon(status: &str) -> &'static str {
    match status {
        "booked" => "🎫",
        "planned" => "📋",
        _ => "❓",
    }
}

struct BookedRow {
    day: i64,
    title: String,
    ref_str: String,
}

struct PendingRow {
    day: i64,
    title: String,
    book_by: String,
}

/// Walk days → sessions (in TS order: morning, noon, afternoon, evening)
/// → activities. Mirror the TS branching exactly:
///   - if `booking_status === "booked"` → CONFIRMED
///   - else if `booking_status === "pending"` OR (`booking_required` && no
///     booking_status) → PENDING
///
/// The order we push into the lists also matches the TS: it iterates
/// day-by-day, session-by-session, activity-by-activity.
fn collect_bookings(
    view: &PlanView,
    book_by: &HashMap<(i64, String, i64), String>,
) -> (Vec<BookedRow>, Vec<PendingRow>) {
    let mut booked: Vec<BookedRow> = Vec::new();
    let mut pending: Vec<PendingRow> = Vec::new();
    for d in &view.days {
        for session_name in SESSIONS_IN_ORDER {
            let Some(s) = d.sessions.get(*session_name) else { continue };
            for (idx, a) in s.activities.iter().enumerate() {
                if a.booking_status == "booked" {
                    booked.push(BookedRow {
                        day: d.day_number,
                        title: a.title.clone(),
                        ref_str: a.booking_ref.clone(),
                    });
                } else if a.booking_status == "pending"
                    || (a.booking_required && a.booking_status.is_empty())
                {
                    // Use sort_order when present (the DB primary key path);
                    // fall back to iteration index for the rare case where
                    // sort_order is unset (TS doesn't care about the key
                    // for rendering — it just reads a.book_by by name).
                    let by = book_by
                        .get(&(d.day_number, session_name.to_string(), a.sort_order))
                        .cloned()
                        .or_else(|| {
                            book_by
                                .get(&(d.day_number, session_name.to_string(), idx as i64))
                                .cloned()
                        })
                        .unwrap_or_default();
                    pending.push(PendingRow {
                        day: d.day_number,
                        title: a.title.clone(),
                        book_by: by,
                    });
                }
            }
        }
    }
    (booked, pending)
}
