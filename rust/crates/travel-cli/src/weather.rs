//! P1 Rust-port STUB — see docs/plans/2026-06-10-rust-port-audit.md
//! Batch 5. Port behavior from src/cli/commands/weather.ts (fetch-weather).
//! Fetches live forecast (must hit the real source — no fabricated data) and
//! writes days.weather_* columns. Signature fixed by main.rs dispatch.

#[allow(dead_code)]
pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let _ = (args, plan_id);
    Err("fetch-weather: not yet implemented (P1 Rust port)".to_string())
}
