//! Typed data-access layer for business-table SQL and row mapping.
//!
//! Boundary: owns SQL strings + row structs for OTA/plan tables. Does NOT write
//! `plan_events`, `operation_runs`, or bump `plans.version` — that stays in
//! `travel-cli` orchestration (`cascade::common`).

pub mod checksum;
pub mod ids;
pub mod repo;

pub use repo::offers::OfferRow;