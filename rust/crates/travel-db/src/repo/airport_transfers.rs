//! `airport_transfers` / `airport_transfer_candidates` domain writes for
//! `set-airport-transfer`.
//!
//! DAL boundary: owns the domain-table SQL. The audit triad
//! (`plan_events`/`plan_event_data`/`operation_runs`/`plans.version`) stays in
//! `travel-cli` (`cascade::common`) — this module never touches it.
//!
//! (stub — bodies added by the airport_transfers DAL migration.)

#![allow(unused_imports)]
use libsql::Connection;
