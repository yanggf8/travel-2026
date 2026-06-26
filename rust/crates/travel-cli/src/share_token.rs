// `travel share-token` — mint/list/deactivate opaque, per-plan, view-scope share
// tokens for the trip dashboard.
//
// The Cloudflare Worker (dashboard read path) consumes the `plan_share_tokens`
// table to gate access to a single plan's view. The signed-in Worker UI is the
// primary day-to-day management path; the CLI remains the operator escape hatch.
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

/// Live host for the Rust dashboard worker (the read path the share token gates).
/// Override with `TRAVEL_DASHBOARD_HOST` when the URL cutover reclaims the primary
/// `trip-dashboard.yanggf.workers.dev` name.
const DEFAULT_DASHBOARD_HOST: &str = "trip-dashboard-rs.yanggf.workers.dev";

fn dashboard_host() -> String {
    std::env::var("TRAVEL_DASHBOARD_HOST")
        .ok()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_DASHBOARD_HOST.to_string())
}

/// Build the shareable dashboard URL. The Worker addresses plans by hyphenated slug.
fn share_url(plan_id: &str, token: &str) -> String {
    let plan_slug = plan_id.replace('_', "-");
    format!(
        "https://{}/?plan={plan_slug}&token={token}",
        dashboard_host()
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TokenRecord {
    token: String,
    status: String,
    created_at: String,
    deactivated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Mint,
    List { full: bool },
    Deactivate { token: String },
}

/// CLI entry: `travel share-token`. The plan is resolved by the dispatcher with
/// the same ladder as other commands. Default action MINTS a fresh token;
/// `--show` / `--list` lists fingerprints + status; `--show-full` prints the
/// sensitive full URLs; `deactivate <token>` inactivates one active token.
pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let action = parse_action(args)?;

    match action {
        Action::List { full } => {
            // Read-only path: list existing tokens (no mint). Read tier suffices.
            let conn = match crate::db::connect_read().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: failed to connect to Turso (read tier): {e}");
                    std::process::exit(1);
                }
            };
            match list_tokens(&conn, &plan_id).await {
                Ok(tokens) if tokens.is_empty() => {
                    eprintln!(
                        "No share token for plan_id={plan_id}. Mint one with: travel share-token"
                    );
                    std::process::exit(1);
                }
                Ok(tokens) => {
                    print_tokens(&plan_id, &tokens, full);
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Error: share-token --show failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Action::Deactivate { token } => {
            let conn = match crate::db::connect_write().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: failed to connect to Turso (write tier): {e}");
                    std::process::exit(1);
                }
            };
            match deactivate_token(&conn, &plan_id, &token).await {
                Ok(true) => {
                    println!(
                        "deactivated: {}  plan_id={plan_id}",
                        token_fingerprint(&token)
                    );
                    Ok(())
                }
                Ok(false) => {
                    eprintln!(
                        "Error: no active token {} for plan_id={plan_id}",
                        token_fingerprint(&token)
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Error: share-token deactivate failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Action::Mint => {
            let conn = match crate::db::connect_write().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: failed to connect to Turso (write tier): {e}");
                    std::process::exit(1);
                }
            };
            match execute(&conn, &plan_id).await {
                Ok(token) => {
                    println!("token: {token}");
                    println!("url:   {}", share_url(&plan_id, &token));
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Error: share-token failed: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}

fn parse_action(args: &[String]) -> Result<Action, String> {
    let mut action = Action::Mint;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--show" | "--list" => {
                action = Action::List { full: false };
                i += 1;
            }
            "--show-full" | "--full" => {
                action = Action::List { full: true };
                i += 1;
            }
            "deactivate" | "revoke" => {
                let Some(token) = args.get(i + 1) else {
                    return Err("share-token deactivate requires a token".to_string());
                };
                if !is_grant_token(token) {
                    return Err(format!("invalid token format: {token}"));
                }
                action = Action::Deactivate {
                    token: token.clone(),
                };
                i += 2;
            }
            "--deactivate" | "--revoke" => {
                let Some(token) = args.get(i + 1) else {
                    return Err(format!("{} requires a token", args[i]));
                };
                if !is_grant_token(token) {
                    return Err(format!("invalid token format: {token}"));
                }
                action = Action::Deactivate {
                    token: token.clone(),
                };
                i += 2;
            }
            "--plan-id" | "--dest" | "--travel-date" | "--travel-start" | "--travel-end" => {
                i += 2; // resolver-owned flag + value
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}"));
            }
            _ => i += 1, // ignore resolver positionals / stray text
        }
    }
    Ok(action)
}

/// List existing share tokens for a plan, newest first.
async fn list_tokens(conn: &Connection, plan_id: &str) -> Result<Vec<TokenRecord>, String> {
    let mut rows = conn
        .query(
            "SELECT token, COALESCE(status, 'active'), created_at, deactivated_at \
             FROM plan_share_tokens \
             WHERE plan_id = ?1 ORDER BY created_at DESC",
            libsql::params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("plan_share_tokens query failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("plan_share_tokens row read failed: {e}"))?
    {
        let token: String = row.get(0).map_err(|e| format!("token read: {e}"))?;
        let status: String = row.get(1).map_err(|e| format!("status read: {e}"))?;
        let created_at: String = row.get(2).map_err(|e| format!("created_at read: {e}"))?;
        let deactivated_at: Option<String> = row
            .get(3)
            .map_err(|e| format!("deactivated_at read: {e}"))?;
        out.push(TokenRecord {
            token,
            status,
            created_at,
            deactivated_at,
        });
    }
    Ok(out)
}

fn print_tokens(plan_id: &str, tokens: &[TokenRecord], full: bool) {
    for t in tokens {
        let suffix = t
            .deactivated_at
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|at| format!("  deactivated {at}"))
            .unwrap_or_default();
        println!(
            "token: {}  status={}  created {}{}",
            token_fingerprint(&t.token),
            t.status,
            t.created_at,
            suffix
        );
        if full {
            println!("url:   {}", share_url(plan_id, &t.token));
        }
    }
    if !full {
        println!("hint:  use --show-full to print full bearer URLs");
    }
}

async fn execute(conn: &Connection, plan_id: &str) -> Result<String, String> {
    // Fail loud if the plan does not exist — never mint a token for a phantom plan.
    if !plan_exists(conn, plan_id).await? {
        return Err(format!("plans row missing for plan_id={plan_id}"));
    }

    let token = mint_token();

    conn.execute(
        "INSERT INTO plan_share_tokens (plan_id, token, created_at, status, created_by) \
         VALUES (?1, ?2, datetime('now'), 'active', 'cli')",
        libsql::params![plan_id.to_string(), token.clone()],
    )
    .await
    .map_err(|e| format!("plan_share_tokens INSERT failed: {e}"))?;

    Ok(token)
}

async fn deactivate_token(conn: &Connection, plan_id: &str, token: &str) -> Result<bool, String> {
    let changed = conn
        .execute(
            "UPDATE plan_share_tokens \
             SET status = 'inactive', deactivated_at = datetime('now'), deactivated_by = 'cli' \
             WHERE plan_id = ?1 AND token = ?2 AND status = 'active'",
            libsql::params![plan_id.to_string(), token.to_string()],
        )
        .await
        .map_err(|e| format!("plan_share_tokens deactivate failed: {e}"))?;
    Ok(changed > 0)
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

fn is_grant_token(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn token_fingerprint(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() <= 12 {
        return token.to_string();
    }
    let head: String = chars.iter().take(6).collect();
    let tail: String = chars[chars.len() - 6..].iter().collect();
    format!("{head}...{tail}")
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
        assert_ne!(mint_token(), mint_token(), "successive tokens must differ");
    }

    #[test]
    fn share_url_uses_real_host_and_hyphen_slug() {
        // underscore plan_id → hyphenated slug; real default host (not a placeholder).
        let u = share_url("okinawa_2026", "deadbeef");
        assert_eq!(
            u,
            "https://trip-dashboard-rs.yanggf.workers.dev/?plan=okinawa-2026&token=deadbeef"
        );
        assert!(!u.contains('_'), "slug must be hyphenated");
        assert!(!u.contains("<"), "no placeholder host left in the URL");
    }

    #[test]
    fn parse_show_and_deactivate_actions() {
        assert_eq!(
            parse_action(&["--show".to_string()]),
            Ok(Action::List { full: false })
        );
        assert_eq!(
            parse_action(&["--show-full".to_string()]),
            Ok(Action::List { full: true })
        );
        assert_eq!(
            parse_action(&[
                "deactivate".to_string(),
                "0123456789abcdef0123456789abcdef".to_string()
            ]),
            Ok(Action::Deactivate {
                token: "0123456789abcdef0123456789abcdef".to_string()
            })
        );
    }

    #[test]
    fn grant_token_validator_accepts_lower_hex_only() {
        assert!(is_grant_token("0123456789abcdef0123456789abcdef"));
        assert!(!is_grant_token("0123456789ABCDEF0123456789ABCDEF"));
        assert!(!is_grant_token("short"));
    }

    #[test]
    fn token_fingerprint_hides_middle() {
        assert_eq!(token_fingerprint("0123456789abcdef"), "012345...abcdef");
    }
}
