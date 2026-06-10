//! P1 Rust-port STUB — see docs/plans/2026-06-10-rust-port-audit.md
//! Batch 5. Port behavior from src/cli/commands/search-compare.ts
//! (compare-offers / search-offers). Signatures fixed by main.rs dispatch.

#[allow(dead_code)]
pub async fn run_compare(args: &[String]) -> Result<(), String> {
    let _ = args;
    Err("compare-offers: not yet implemented (P1 Rust port)".to_string())
}

#[allow(dead_code)]
pub async fn run_search(args: &[String]) -> Result<(), String> {
    let _ = args;
    Err("search-offers: not yet implemented (P1 Rust port)".to_string())
}
