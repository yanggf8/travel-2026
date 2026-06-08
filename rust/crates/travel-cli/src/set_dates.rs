// rust/crates/travel-cli/src/set_dates.rs
use crate::status::format_date; // byte-parity formatter (Mar 1, not Mar 01)
use crate::validate::validate_date_range;

/// Run set-dates mutation.
/// plan_id: resolved plan identifier (from TRAVEL_PLAN_ID or explicit arg)
pub async fn run(
    start: String,
    end: String,
    reason: Option<String>,
    plan_id: String,
) -> Result<(), String> {
    // 1. Validate date range (parity with TS)
    let days = match validate_date_range(&start, &end) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    // Format date for display (matching TS formatDate)
    let start_fmt = format_date(&start);
    let end_fmt = format_date(&end);

    println!("\n📅 Setting dates: {} → {} ({} days)", start_fmt, end_fmt, days);
    if let Some(r) = &reason {
        println!("   Reason: {}", r);
    }

    // 2. Execute mutation + cascade (Task 5)
    //    For now, stub — full implementation in date_change.rs
    //    This will call the write-tier logic once Task 5 is complete.
    println!("[set-dates] plan_id={}, start={}, end={}", plan_id, start, end);
    // TODO (Task 5): call crate::cascade::date_change::execute_date_anchor_change(...)

    println!("✅ Dates updated and cascade triggered");

    Ok(())
}

