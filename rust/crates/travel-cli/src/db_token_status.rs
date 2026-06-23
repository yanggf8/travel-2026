// `travel db token-status` — diagnose Turso credential resolution.
//
// The sandbox gotcha: `./bin/travel` resolves its Turso token via turso-util
// (env → cache → mint via the `turso` CLI broker). In a sandbox the broker /
// cache / login usually fail, so the CLI can't reach Turso even when `.env`
// exists — and every DB command then errors with a token failure that looks
// like a data problem. The documented fix is to export the static `.env` token
// into the env vars the CLI reads:
//
//   export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-)
//   export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
//   export TRAVEL_TURSO_WRITE_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)
//
// This command makes that state observable: it reports which env vars are set
// (NEVER printing their values), then actually attempts a live read- and
// write-tier connection + a trivial query, and prints the fix if either fails.
// Read-only; never mutates. Exit 0 if both tiers connect, else 1.

use crate::db;

pub async fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage:\n  travel db token-status\n  \
             (diagnoses Turso credential resolution: reports which TRAVEL_TURSO_*\n  \
              env vars are set — values never printed — then live-probes the read\n  \
              and write tiers. Exit 1 if either tier cannot connect.)"
        );
        return Ok(());
    }
    if !args.is_empty() {
        return Err("Usage: travel db token-status  (no arguments)".to_string());
    }

    println!("Turso credential diagnosis");
    println!("──────────────────────────");

    // 1) Which env vars are present? Report set/unset only — NEVER the value.
    //    (A token in stderr/scrollback is a credential leak.)
    let env_vars = [
        ("TRAVEL_TURSO_URL", "DB URL (read+write)"),
        ("TURSO_URL", "DB URL (fallback)"),
        ("TRAVEL_TURSO_READ_TOKEN", "read tier"),
        ("TRAVEL_TURSO_WRITE_TOKEN", "write tier"),
        ("TRAVEL_TURSO_SECRETS_TOKEN", "secrets tier"),
    ];
    println!("env:");
    for (name, what) in env_vars {
        let set = std::env::var(name).map(|v| !v.trim().is_empty()).unwrap_or(false);
        println!("  {:<28} {:<22} {}", name, what, if set { "✅ set" } else { "— unset" });
    }
    println!();

    // 2) Live-probe each tier: connect + a trivial query. This is the real test
    //    (env-set ≠ working — the token could be wrong/expired).
    let read_ok = probe("read", db::connect_read()).await;
    let write_ok = probe("write", db::connect_write()).await;

    println!();
    if read_ok && write_ok {
        println!("✅ both tiers OK — Turso is reachable.");
        Ok(())
    } else {
        println!("❌ at least one tier failed. If `.env` has a static token, export it:");
        println!("   export TRAVEL_TURSO_URL=$(grep '^TURSO_URL=' .env | cut -d= -f2-)");
        println!("   export TRAVEL_TURSO_READ_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)");
        println!("   export TRAVEL_TURSO_WRITE_TOKEN=$(grep '^TURSO_TOKEN=' .env | cut -d= -f2-)");
        println!("   (sandbox: the turso CLI broker/cache/login usually can't mint — see CLAUDE.md)");
        std::process::exit(1);
    }
}

/// Connect on a tier and run `SELECT 1`. Returns true on success. Prints a
/// one-line pass/fail; the error message (not the token) is shown on failure.
async fn probe(
    tier: &str,
    conn_fut: impl std::future::Future<Output = Result<libsql::Connection, String>>,
) -> bool {
    match conn_fut.await {
        Ok(conn) => match conn.query("SELECT 1", ()).await {
            Ok(_) => {
                println!("probe {tier:<6}: ✅ connected + query OK");
                true
            }
            Err(e) => {
                println!("probe {tier:<6}: ❌ connected but query failed: {e}");
                false
            }
        },
        Err(e) => {
            println!("probe {tier:<6}: ❌ {e}");
            false
        }
    }
}
