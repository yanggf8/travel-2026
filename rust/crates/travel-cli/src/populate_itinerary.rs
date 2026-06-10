//! P1 Rust-port STUB — see docs/plans/2026-06-10-rust-port-audit.md
//! Batch 2. Port behavior from src/cli/commands/populate-itinerary.ts.
//! Signature is fixed by main.rs dispatch — do not change it; replace the body.

#[allow(dead_code)]
pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let _ = (args, plan_id);
    Err("populate-itinerary: not yet implemented (P1 Rust port)".to_string())
}
