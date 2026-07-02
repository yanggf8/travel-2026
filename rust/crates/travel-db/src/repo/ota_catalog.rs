//! OTA provider-catalog domain writes for `set-ota-source`/`set-ota-coverage`/
//! `set-ota-region-code`/`set-ota-url-param` (the globally-scoped `ota_sources` /
//! `ota_source_coverage` / `ota_source_region_codes` / `ota_source_url_param`
//! tables; `ota_source_workflow` already has its own repo module).
//!
//! DAL boundary: owns the domain-table SQL. The `catalog_runs` audit row stays in
//! `travel-cli` — this module never touches it.
//!
//! (stub — bodies added by the ota_catalog DAL migration.)

#![allow(unused_imports)]
use libsql::Connection;
