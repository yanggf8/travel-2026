//! P1 Rust-port STUB — see docs/plans/2026-06-10-rust-port-audit.md
//! Batch 4. Port behavior from src/cli/commands/shaping.ts and the shaping
//! service (src/services/shaping-service.ts). Pre-plan triangle-research domain
//! keyed by run_id (shaping_* tables). Signatures fixed by main.rs dispatch.

#[allow(dead_code)]
pub async fn run_init(args: &[String]) -> Result<(), String> {
    let _ = args;
    Err("shaping-init: not yet implemented (P1 Rust port)".to_string())
}

#[allow(dead_code)]
pub async fn run_compare(args: &[String]) -> Result<(), String> {
    let _ = args;
    Err("shaping-compare: not yet implemented (P1 Rust port)".to_string())
}

#[allow(dead_code)]
pub async fn run_adopt(args: &[String]) -> Result<(), String> {
    let _ = args;
    Err("shaping-adopt: not yet implemented (P1 Rust port)".to_string())
}

#[allow(dead_code)]
pub async fn run_baseline(args: &[String]) -> Result<(), String> {
    let _ = args;
    Err("shaping-baseline: not yet implemented (P1 Rust port)".to_string())
}

#[allow(dead_code)]
pub async fn run_export(args: &[String]) -> Result<(), String> {
    let _ = args;
    Err("shaping-export: not yet implemented (P1 Rust port)".to_string())
}

#[allow(dead_code)]
pub async fn run_import(args: &[String]) -> Result<(), String> {
    let _ = args;
    Err("shaping-import: not yet implemented (P1 Rust port)".to_string())
}
