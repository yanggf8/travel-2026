// `travel share-token` — mint an opaque, per-plan, view-scope share token for the
// trip dashboard, store it, and print the token + a share URL.
//
// The Cloudflare Worker (dashboard read path) consumes the `plan_share_tokens`
// table to gate access to a single plan's view; the CLI is the SOLE write path.
//
// This is a SIDE-TABLE op, not a plan-domain mutation: it does not change any
// plan content, so it deliberately does NOT write the plan-content audit triad
// (no plans.version bump, no plan_events, no operation_runs row). A version bump
// / `completed` audit should imply plan domain content actually changed — minting
// a share token does not. It is a single INSERT into a side channel, mirroring how
// the neighbouring tables that the Worker reads are populated.
//
// Token generation: because this token gates who may VIEW a plan on the dashboard,
// it must be cryptographically unpredictable. It is therefore generated from a
// CSPRNG via `getrandom` (128 random bits rendered as 32 lowercase hex chars) —
// NOT derived from time/pid/counter, which an attacker could guess.

use libsql::Connection;

/// Dashboard host is not finalized yet — use a literal placeholder in the URL.
const DASHBOARD_HOST: &str = "<dashboard-host>";

/// CLI entry: `travel share-token`. The plan is resolved by the dispatcher (via
/// TRAVEL_PLAN_ID / default), matching the other `set-*` mutation arms in this
/// crate; this command takes no required positional args.
pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    // Accept-and-skip the flags the dispatcher / resolver own so the catch-all
    // doesn't reject e.g. `--dest` that other commands tolerate.
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--plan-id" | "--dest" | "--travel-date" | "--travel-start" | "--travel-end" => {
                i += 2; // skip flag + its value
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => {
                i += 1; // ignore stray positionals
            }
        }
    }

    let conn = match crate::db::connect_write().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: failed to connect to Turso (write tier): {e}");
            std::process::exit(1);
        }
    };

    match execute(&conn, &plan_id).await {
        Ok(token) => {
            // Plan slug for the URL: the Worker addresses plans by hyphenated slug.
            let plan_slug = plan_id.replace('_', "-");
            println!("token: {token}");
            println!(
                "url: https://{DASHBOARD_HOST}/?plan={plan_slug}&token={token}"
            );
            Ok(())
        }
        Err(e) => {
            eprintln!("Error: share-token failed: {e}");
            std::process::exit(1);
        }
    }
}

async fn execute(conn: &Connection, plan_id: &str) -> Result<String, String> {
    // Fail loud if the plan does not exist — never mint a token for a phantom plan.
    if !plan_exists(conn, plan_id).await? {
        return Err(format!("plans row missing for plan_id={plan_id}"));
    }

    let token = mint_token();

    conn.execute(
        "INSERT INTO plan_share_tokens (plan_id, token, created_at) \
         VALUES (?1, ?2, datetime('now'))",
        libsql::params![plan_id.to_string(), token.clone()],
    )
    .await
    .map_err(|e| format!("plan_share_tokens INSERT failed: {e}"))?;

    Ok(token)
}

async fn plan_exists(conn: &Connection, plan_id: &str) -> Result<bool, String> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM plans WHERE plan_id = ?1",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plans existence query failed: {e}"))?;
    Ok(rows
        .next()
        .await
        .map_err(|e| format!("plans existence row read failed: {e}"))?
        .is_some())
}

/// Mint an opaque 32-hex-char (128-bit) bearer token from a CSPRNG.
/// This token scopes who may view a plan on the dashboard, so it must be
/// cryptographically unpredictable — do NOT derive it from time/pid/counter.
fn mint_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("CSPRNG (getrandom) failed");
    let mut s = String::with_capacity(32);
    use std::fmt::Write;
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_token_is_32_hex_chars() {
        let t = mint_token();
        assert_eq!(t.len(), 32, "token must be 32 chars");
        assert!(
            t.chars().all(|c| c.is_ascii_hexdigit()),
            "token must be all hex"
        );
    }

    #[test]
    fn mint_token_is_unique_per_call() {
        assert_ne!(
            mint_token(),
            mint_token(),
            "successive tokens must differ"
        );
    }
}
