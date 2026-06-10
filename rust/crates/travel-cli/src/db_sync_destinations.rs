//! `db sync destinations` — port of scripts/turso-sync-destinations.ts.
//!
//! Deprecated. Destination configuration is seeded and maintained in Turso
//! by the migrate path and direct DB operations. There is no local
//! destination JSON source of truth to sync from, so this command fails
//! loud (1:1 with the TS script, which throws and exits 1).
//!
//! Signature fixed by main.rs dispatch: `run(&[String])`.

pub async fn run(args: &[String]) -> Result<(), String> {
    let _ = args;
    Err("Destination sync from local files is disabled. Use npm run db:migrate:turso or direct Turso SQL for destination_config changes.".to_string())
}
