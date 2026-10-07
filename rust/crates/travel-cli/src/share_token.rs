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

impl TokenRecord {
    fn is_active(&self) -> bool {
        self.status == "active"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Mint,
    List { full: bool, all: bool },
    /// One or more tokens (full 32-hex, or a unique prefix of one).
    Deactivate { tokens: Vec<String> },
}

/// CLI entry: `travel share-token`. The plan is resolved by the dispatcher with
/// the same ladder as other commands. Default action MINTS a fresh token;
/// `--show` / `--list` lists fingerprints + status (ACTIVE only by default —
/// `--all` unfolds the deactivated block); `--show-full` prints the sensitive
/// full URLs; `deactivate <t1> <t2> ...` inactivates one or more tokens, each
/// given in full or as a unique prefix (what `--show` prints).
pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let action = parse_action(args)?;

    match action {
        Action::List { full, all } => {
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
                    print!("{}", render_tokens(&plan_id, &tokens, full, all));
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Error: share-token --show failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Action::Deactivate { tokens } => {
            let conn = match crate::db::connect_write().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error: failed to connect to Turso (write tier): {e}");
                    std::process::exit(1);
                }
            };
            let records = list_tokens(&conn, &plan_id).await?;
            let mut deactivated = 0usize;
            let mut failures: Vec<String> = Vec::new();
            for arg in &tokens {
                let token = match resolve_token_arg(arg, &records) {
                    Ok(t) => t.to_string(),
                    Err(reason) => {
                        failures.push(format!("{arg}: {reason}"));
                        continue;
                    }
                };
                match deactivate_token(&conn, &plan_id, &token).await {
                    Ok(true) => {
                        deactivated += 1;
                        println!(
                            "deactivated: {}  plan_id={plan_id}",
                            token_fingerprint(&token)
                        );
                    }
                    Ok(false) => failures.push(format!(
                        "{arg}: no active token {}",
                        token_fingerprint(&token)
                    )),
                    Err(e) => failures.push(format!("{arg}: {e}")),
                }
            }
            println!("{deactivated} deactivated, {} skipped", tokens.len() - deactivated);
            if failures.is_empty() {
                Ok(())
            } else {
                for f in &failures {
                    eprintln!("Error: {f}");
                }
                std::process::exit(1);
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
    let mut mint = true;
    let mut full = false;
    let mut all = false;
    let mut deactivate: Option<Vec<String>> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--show" | "--list" => {
                mint = false;
                i += 1;
            }
            "--show-full" | "--full" => {
                mint = false;
                full = true;
                i += 1;
            }
            "--all" => {
                all = true;
                i += 1;
            }
            "deactivate" | "revoke" | "--deactivate" | "--revoke" => {
                mint = false;
                let mut toks: Vec<String> = Vec::new();
                i += 1;
                while i < args.len() && !args[i].starts_with("--") {
                    let t = &args[i];
                    if !is_tokenish(t) {
                        return Err(format!("invalid token format: {t}"));
                    }
                    toks.push(t.clone());
                    i += 1;
                }
                if toks.is_empty() {
                    return Err("share-token deactivate requires at least one token (full 32-hex, or a unique prefix of what --show prints)".to_string());
                }
                deactivate = Some(toks);
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
    if let Some(tokens) = deactivate {
        return Ok(Action::Deactivate { tokens });
    }
    if mint {
        return Ok(Action::Mint);
    }
    Ok(Action::List { full, all })
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

/// Render the listing. DEFAULT collapses deactivated tokens (they are history;
/// the active set is what a manager scans) — `all` unfolds them into a labelled
/// block after the actives, and a hint says how many are hidden.
fn render_tokens(plan_id: &str, tokens: &[TokenRecord], full: bool, all: bool) -> String {
    let mut out = String::new();
    let mut hidden = 0usize;
    let mut inactive_block: Vec<&TokenRecord> = Vec::new();
    for t in tokens {
        if t.is_active() {
            out.push_str(&token_line(plan_id, t, full));
        } else {
            inactive_block.push(t);
            if !all {
                hidden += 1;
            }
        }
    }
    if all && !inactive_block.is_empty() {
        out.push_str(&format!("\n停用 ({}):\n", inactive_block.len()));
        for t in &inactive_block {
            out.push_str(&token_line(plan_id, t, full));
        }
    }
    if hidden > 0 {
        out.push_str(&format!(
            "hint:  {hidden} deactivated hidden — --all to unfold\n"
        ));
    }
    if !full {
        out.push_str("hint:  use --show-full to print full bearer URLs\n");
    }
    out
}

fn token_line(plan_id: &str, t: &TokenRecord, full: bool) -> String {
    let mut line = format!(
        "token: {}  status={}  created {}",
        token_fingerprint(&t.token),
        t.status,
        t.created_at
    );
    if let Some(at) = t.deactivated_at.as_deref().filter(|s| !s.is_empty()) {
        line.push_str(&format!("  deactivated {at}"));
    }
    line.push('\n');
    if full {
        line.push_str(&format!("url:   {}\n", share_url(plan_id, &t.token)));
    }
    line
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
        .map_err(|e| e.to_string())?
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

/// Accepts a full 32-hex token or a prefix of one (≥6 chars) — the fingerprint
/// head that `--show` prints is the intended copy source.
fn is_tokenish(s: &str) -> bool {
    s.len() >= 6 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Resolve one deactivate argument against the plan's tokens: exact token, or
/// a unique prefix. Error strings name the failure (not found / ambiguous /
/// already inactive) so the multi-token summary can report per argument.
fn resolve_token_arg<'a>(
    arg: &str,
    records: &'a [TokenRecord],
) -> Result<&'a str, String> {
    if let Some(t) = records.iter().map(|r| r.token.as_str()).find(|t| *t == arg) {
        return Ok(t);
    }
    if arg.len() < 6 {
        return Err("prefix too short (min 6 chars — copy the fingerprint head from --show)".into());
    }
    let matches: Vec<&str> = records
        .iter()
        .map(|r| r.token.as_str())
        .filter(|t| t.starts_with(arg))
        .collect();
    match matches.len() {
        1 => Ok(matches[0]),
        0 => Err("no token matches".into()),
        _ => Err(format!(
            "ambiguous prefix — {} tokens match; copy more characters",
            matches.len()
        )),
    }
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
            Ok(Action::List { full: false, all: false })
        );
        assert_eq!(
            parse_action(&["--show-full".to_string()]),
            Ok(Action::List { full: true, all: false })
        );
        // --all unfolds the deactivated block in either listing form.
        assert_eq!(
            parse_action(&["--show".to_string(), "--all".to_string()]),
            Ok(Action::List { full: false, all: true })
        );
        // Multi-select deactivate: one subcommand, several tokens.
        assert_eq!(
            parse_action(&[
                "deactivate".to_string(),
                "0123456789abcdef0123456789abcdef".to_string(),
                "0123456789abcdef".to_string(),
            ]),
            Ok(Action::Deactivate {
                tokens: vec![
                    "0123456789abcdef0123456789abcdef".to_string(),
                    "0123456789abcdef".to_string(),
                ]
            })
        );
        // Resolver flags after the token list are tolerated (the dispatcher
        // already consumed --plan-id; the variadic scan stops at any flag).
        assert_eq!(
            parse_action(&[
                "--deactivate".to_string(),
                "0123456789abcdef".to_string(),
                "--plan-id".to_string(),
                "x".to_string(),
            ]),
            Ok(Action::Deactivate {
                tokens: vec!["0123456789abcdef".to_string()]
            })
        );
    }

    #[test]
    fn grant_token_validator_accepts_lower_hex_only() {
        assert!(is_tokenish("0123456789abcdef0123456789abcdef"));
        assert!(is_tokenish("0123456789abcdef")); // prefix
        assert!(!is_tokenish("ABCDEF")); // uppercase
        assert!(!is_tokenish("abcde")); // < 6
    }

    #[test]
    fn token_fingerprint_hides_middle() {
        assert_eq!(token_fingerprint("0123456789abcdef"), "012345...abcdef");
    }

    fn rec(token: &str, status: &str) -> TokenRecord {
        TokenRecord {
            token: token.to_string(),
            status: status.to_string(),
            created_at: "2026-10-07".into(),
            deactivated_at: None,
        }
    }

    #[test]
    fn resolve_accepts_exact_and_unique_prefix() {
        let records = vec![
            rec("0123456789abcdef0123456789abcdef", "active"),
            rec("fedcba9876543210fedcba9876543210", "active"),
        ];
        assert_eq!(
            resolve_token_arg("0123456789abcdef0123456789abcdef", &records).unwrap(),
            "0123456789abcdef0123456789abcdef"
        );
        // Unique prefix — the fingerprint head --show prints.
        assert_eq!(resolve_token_arg("fedcba", &records).unwrap(), "fedcba9876543210fedcba9876543210");
        assert!(resolve_token_arg("ffff", &records).is_err(), "<6 chars rejected");
        assert!(
            resolve_token_arg("999999", &records)
                .err()
                .unwrap()
                .contains("no token matches")
        );
    }

    #[test]
    fn resolve_names_ambiguity_instead_of_guessing() {
        let records = vec![
            rec("0123456789abcdef0123456789abcdef", "active"),
            rec("0123456789ffffffffffffffffffff", "active"),
        ];
        let err = resolve_token_arg("012345", &records).err().unwrap();
        assert!(err.contains("ambiguous"), "{err}");
        // Longer prefix disambiguates.
        assert_eq!(
            resolve_token_arg("0123456789abcdef", &records).unwrap(),
            "0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn render_collapses_deactivated_by_default_and_all_unfolds() {
        let plan = "p";
        let tokens = vec![
            rec("0123456789abcdef0123456789abcdef", "active"),
            rec("fedcba9876543210fedcba9876543210", "inactive"),
        ];
        // Default: actives only; the deactivated one is a hidden count.
        let folded = render_tokens(plan, &tokens, false, false);
        assert!(folded.contains("status=active"), "{folded}");
        assert!(!folded.contains("status=inactive"), "{folded}");
        assert!(folded.contains("1 deactivated hidden — --all to unfold"), "{folded}");
        // --all unfolds them into a labelled block.
        let unfolded = render_tokens(plan, &tokens, false, true);
        assert!(unfolded.contains("停用 (1):"), "{unfolded}");
        assert!(unfolded.contains("status=inactive"), "{unfolded}");
    }
}
