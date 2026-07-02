//! Plan-lifecycle domain writes (soft-delete etc.) for `mark-plan-deleted`.
//!
//! DAL boundary: owns the domain-table SQL (the `plans` soft-delete UPDATE). The
//! audit triad (`operation_runs`/`plans.version`) stays in `travel-cli`
//! (`cascade::common`) — this module never touches it.
//!
//! (stub — bodies added by the plan_lifecycle DAL migration.)

#![allow(unused_imports)]
use libsql::Connection;
