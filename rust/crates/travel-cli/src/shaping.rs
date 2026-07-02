//! Shaping Stage commands (pre-plan triangle research, keyed by run_id).
//!
//! Port of src/cli/commands/shaping.ts + src/services/shaping-service.ts.
//! Tables: shaping_research_runs, shaping_research_destinations,
//! shaping_research_durations, shaping_rules, shaping_scrape_attempts,
//! shaping_candidates, shaping_candidate_flights, shaping_tour_group_offers
//! (+ notes), shaping_research_runs status. NO JSON columns.
//!
//! Subcommands: shaping-init / shaping-compare / shaping-adopt /
//! shaping-baseline / shaping-export / shaping-import.

use crate::cascade::common::{now_db_datetime, now_rfc3339, record_operation};
use crate::db;
use libsql::{params, params_from_iter, Connection};
use serde_json::Value;

// ── helpers ──────────────────────────────────────────────────────────

fn new_run_id() -> String {
    // Mirrors shaping.ts newRunId(): shaping-YYYYMMDD-HHMMSS in LOCAL time.
    use chrono::{Datelike, Local, Timelike};
    let d = Local::now();
    format!(
        "shaping-{:04}{:02}{:02}-{:02}{:02}{:02}",
        d.year(),
        d.month(),
        d.day(),
        d.hour(),
        d.minute(),
        d.second()
    )
}

/// Read string value of an optional `--flag value` (first occurrence).
fn opt(rest: &[String], flag: &str) -> Option<String> {
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == flag {
            return rest.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}

/// Read all values of a repeatable `--flag value`.
fn opt_all(rest: &[String], flag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == flag {
            if let Some(v) = rest.get(i + 1) {
                out.push(v.clone());
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    out
}

fn has_flag(rest: &[String], flag: &str) -> bool {
    rest.iter().any(|a| a == flag)
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(|c| c.is_ascii_digit())
        && b[5..7].iter().all(|c| c.is_ascii_digit())
        && b[8..10].iter().all(|c| c.is_ascii_digit())
}

fn is_int(s: &str) -> bool {
    let t = s.strip_prefix('-').unwrap_or(s);
    !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit())
}

fn fmt_thousands(n: i64) -> String {
    let neg = n < 0;
    let digits = n.abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::new();
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}

// ── parsed --shaping entry ───────────────────────────────────────────

struct ShapingEntry {
    aspect: String,
    role: String,
    kind: String,
    value_text: Option<String>,
    value_date: Option<String>,
    value_integer: Option<i64>,
    notes: Option<String>,
}

fn parse_shaping_entry(raw: &str) -> Result<ShapingEntry, String> {
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() < 4 {
        return Err(format!(
            "--shaping must be ASPECT:ROLE:KIND:VALUE[:NOTES], got: \"{raw}\""
        ));
    }
    let aspect = parts[0].to_lowercase();
    let role = parts[1].to_lowercase();
    let kind = parts[2].to_lowercase();
    let value = parts[3];
    let notes = if parts.len() > 4 {
        Some(parts[4..].join(":"))
    } else {
        None
    };

    let mut value_text = None;
    let mut value_date = None;
    let mut value_integer = None;
    if is_date(value) {
        value_date = Some(value.to_string());
    } else if is_int(value) {
        value_integer = value.parse::<i64>().ok();
    } else {
        value_text = Some(value.to_string());
    }

    Ok(ShapingEntry {
        aspect,
        role,
        kind,
        value_text,
        value_date,
        value_integer,
        notes: notes.filter(|s| !s.is_empty()),
    })
}

fn shaping_value_display(
    value_date: &Option<String>,
    value_text: &Option<String>,
    value_integer: &Option<i64>,
) -> String {
    if let Some(d) = value_date {
        d.clone()
    } else if let Some(t) = value_text {
        t.clone()
    } else if let Some(i) = value_integer {
        i.to_string()
    } else {
        String::new()
    }
}

// ── shaping-init ─────────────────────────────────────────────────────

pub async fn run_init(args: &[String]) -> Result<(), String> {
    if has_flag(args, "--help") || has_flag(args, "-h") {
        println!(
            "Usage:\n  travel shaping-init --origin TPE --start 2026-06-18 --end 2026-06-20 \
             --dest KIX:\"Osaka (KIX)\" --dest NRT:\"Tokyo (NRT)\" --nights 6 --nights 7 \
             [--pax 2] [--rate 32] [--shaping ASPECT:ROLE:KIND:VALUE[:NOTES]] (repeatable)"
        );
        return Ok(());
    }

    let origin = opt(args, "--origin");
    let start = opt(args, "--start");
    let end = opt(args, "--end");
    let dest_opts = opt_all(args, "--dest");
    let nights_opts = opt_all(args, "--nights");
    let pax: i64 = opt(args, "--pax").and_then(|s| s.parse().ok()).unwrap_or(2);
    let rate: f64 = opt(args, "--rate")
        .and_then(|s| s.parse().ok())
        .unwrap_or(32.0);
    let shaping_opts = opt_all(args, "--shaping");

    let (origin, start, end) = match (origin, start, end) {
        (Some(o), Some(s), Some(e)) if !dest_opts.is_empty() && !nights_opts.is_empty() => {
            (o, s, e)
        }
        _ => {
            eprintln!(
                "Error: shaping-init requires --origin, --start, --end, at least one \
                 --dest CODE:LABEL, and at least one --nights N"
            );
            std::process::exit(1);
        }
    };

    // destinations: CODE:LABEL
    let mut destinations: Vec<(String, String)> = Vec::new();
    for d in &dest_opts {
        match d.find(':') {
            Some(idx) => destinations.push((d[..idx].to_uppercase(), d[idx + 1..].to_string())),
            None => {
                eprintln!("Error: --dest must be CODE:LABEL (got: {d})");
                std::process::exit(1);
            }
        }
    }
    let durations: Vec<i64> = nights_opts
        .iter()
        .filter_map(|n| n.parse::<i64>().ok())
        .collect();

    // shaping entries
    let mut shaping: Vec<ShapingEntry> = Vec::new();
    for s in &shaping_opts {
        match parse_shaping_entry(s) {
            Ok(e) => shaping.push(e),
            Err(msg) => {
                eprintln!("Error parsing --shaping: {msg}");
                std::process::exit(1);
            }
        }
    }

    let run_id = new_run_id();
    let origin_uc = origin.to_uppercase();
    let ts = now_rfc3339();

    let conn = db::connect_write().await?;

    use travel_db::repo::shaping as repo_shaping;

    repo_shaping::insert_run(&conn, &run_id, &origin_uc, pax, &start, &end, rate, &ts).await?;

    for (i, (code, label)) in destinations.iter().enumerate() {
        repo_shaping::insert_destination(&conn, &run_id, code, label, i as i64).await?;
    }

    for nights in &durations {
        repo_shaping::insert_duration(&conn, &run_id, *nights, *nights + 1).await?;
    }

    for s in &shaping {
        let rule = repo_shaping::ShapingRuleWrite {
            aspect: s.aspect.clone(),
            role: s.role.clone(),
            kind: s.kind.clone(),
            value_text: s.value_text.clone(),
            value_date: s.value_date.clone(),
            value_integer: s.value_integer,
            notes: s.notes.clone(),
        };
        repo_shaping::insert_rule(&conn, &run_id, &rule, &ts).await?;
    }

    // Seed one 'pending' scrape-attempt per (dest x duration).
    for (code, _) in &destinations {
        for nights in &durations {
            repo_shaping::insert_pending_attempt(&conn, &run_id, code, *nights).await?;
        }
    }

    println!("\n✅ Shaping Stage research run created: {run_id}");
    println!(
        "   Origin: {origin_uc}  Window: {start} → {end}  Pax: {pax}"
    );
    println!(
        "   Destinations: {}",
        destinations
            .iter()
            .map(|(c, _)| c.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "   Durations: {}",
        durations
            .iter()
            .map(|n| format!("{n}n"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    if !shaping.is_empty() {
        println!("\n   Shaping rules recorded ({}):", shaping.len());
        for s in &shaping {
            let val = shaping_value_display(&s.value_date, &s.value_text, &s.value_integer);
            let note = s
                .notes
                .as_ref()
                .map(|n| format!("  // {n}"))
                .unwrap_or_default();
            println!("     {}/{}/{} = {val}{note}", s.aspect, s.role, s.kind);
        }
    } else {
        eprintln!("\n⚠️  No --shaping rules recorded for this run.");
        eprintln!("   The Shaping Stage exists to capture constraints/preferences BEFORE research.");
        eprintln!("   Did you (1) load prior shaping from the DB and (2) record the new rules?");
        eprintln!("   Runs are immutable — if you skipped shaping, create a new run with --shaping");
        eprintln!("   ASPECT:ROLE:KIND:VALUE[:NOTES] (e.g. date:hard_constraint:return_no_later_than:2026-06-24).");
    }

    println!("\nRun created: {run_id}");
    println!("Next: drive the real OTA pages with the Rust CDP scraper");
    println!("  ./rust/target/debug/chromeport fetch interact <url> --source <id> --step ...");
    println!("  ./rust/target/debug/chromeport parse capture <capture-id> --source <id>");

    Ok(())
}

// ── research-run read model ──────────────────────────────────────────

struct ResearchRun {
    run_id: String,
    origin_code: String,
    pax: i64,
    window_start: String,
    window_end: String,
    currency: String,
    #[allow(dead_code)]
    exchange_rate_usd_twd: f64,
    #[allow(dead_code)]
    status: String,
}

struct ShapingRule {
    aspect: String,
    role: String,
    kind: String,
    value_text: Option<String>,
    value_date: Option<String>,
    value_integer: Option<i64>,
    notes: Option<String>,
}

async fn get_research_run(
    conn: &Connection,
    run_id: &str,
) -> Result<Option<ResearchRun>, String> {
    let mut rows = conn
        .query(
            "SELECT run_id, origin_code, pax, window_start, window_end, currency,
                    exchange_rate_usd_twd, status
             FROM shaping_research_runs WHERE run_id = ?1",
            params![run_id.to_string()],
        )
        .await
        .map_err(|e| format!("query shaping_research_runs failed: {e}"))?;
    if let Some(row) = rows.next().await.map_err(|e| format!("row read: {e}"))? {
        Ok(Some(ResearchRun {
            run_id: row.get::<String>(0).unwrap_or_default(),
            origin_code: row.get::<String>(1).unwrap_or_default(),
            pax: row.get::<i64>(2).unwrap_or(0),
            window_start: row.get::<String>(3).unwrap_or_default(),
            window_end: row.get::<String>(4).unwrap_or_default(),
            currency: row.get::<String>(5).unwrap_or_default(),
            exchange_rate_usd_twd: row.get::<f64>(6).unwrap_or(0.0),
            status: row.get::<String>(7).unwrap_or_default(),
        }))
    } else {
        Ok(None)
    }
}

async fn get_research_shaping(
    conn: &Connection,
    run_id: &str,
) -> Result<Vec<ShapingRule>, String> {
    let mut rows = conn
        .query(
            "SELECT aspect, role, kind, value_text, value_date, value_integer, notes
             FROM shaping_rules WHERE run_id = ?1 ORDER BY aspect, role, kind",
            params![run_id.to_string()],
        )
        .await
        .map_err(|e| format!("query shaping_rules failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| format!("row read: {e}"))? {
        out.push(ShapingRule {
            aspect: row.get::<String>(0).unwrap_or_default(),
            role: row.get::<String>(1).unwrap_or_default(),
            kind: row.get::<String>(2).unwrap_or_default(),
            value_text: row.get::<Option<String>>(3).unwrap_or(None),
            value_date: row.get::<Option<String>>(4).unwrap_or(None),
            value_integer: row.get::<Option<i64>>(5).unwrap_or(None),
            notes: row.get::<Option<String>>(6).unwrap_or(None),
        });
    }
    Ok(out)
}

struct Candidate {
    dest_code: String,
    depart_date: String,
    return_date: String,
    nights: i64,
    flight_total_twd: Option<i64>,
    leave_days: Option<i64>,
    rank: Option<i64>,
    verdict: Option<String>,
}

async fn get_candidates(conn: &Connection, run_id: &str) -> Result<Vec<Candidate>, String> {
    let mut rows = conn
        .query(
            "SELECT dest_code, depart_date, return_date, nights, flight_total_twd,
                    leave_days, rank, verdict
             FROM shaping_candidates WHERE run_id = ?1
             ORDER BY rank IS NULL, rank ASC, depart_date ASC",
            params![run_id.to_string()],
        )
        .await
        .map_err(|e| format!("query shaping_candidates failed: {e}"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| format!("row read: {e}"))? {
        out.push(Candidate {
            dest_code: row.get::<String>(0).unwrap_or_default(),
            depart_date: row.get::<String>(1).unwrap_or_default(),
            return_date: row.get::<String>(2).unwrap_or_default(),
            nights: row.get::<i64>(3).unwrap_or(0),
            flight_total_twd: row.get::<Option<i64>>(4).unwrap_or(None),
            leave_days: row.get::<Option<i64>>(5).unwrap_or(None),
            rank: row.get::<Option<i64>>(6).unwrap_or(None),
            verdict: row.get::<Option<String>>(7).unwrap_or(None),
        });
    }
    Ok(out)
}

// ── shaping-compare ──────────────────────────────────────────────────

pub async fn run_compare(args: &[String]) -> Result<(), String> {
    if has_flag(args, "--help") || has_flag(args, "-h") {
        println!("Usage:\n  travel shaping-compare --run <run_id> [--limit N]");
        return Ok(());
    }
    let Some(run_id) = opt(args, "--run") else {
        eprintln!("Error: shaping-compare requires --run <run_id>");
        std::process::exit(1);
    };
    let conn = db::connect_read().await?;
    let Some(run) = get_research_run(&conn, &run_id).await? else {
        eprintln!("Error: research run not found: {run_id}");
        std::process::exit(1);
    };
    let shaping = get_research_shaping(&conn, &run_id).await?;
    let limit: usize = opt(args, "--limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut candidates = get_candidates(&conn, &run_id).await?;
    if limit > 0 && candidates.len() > limit {
        candidates.truncate(limit);
    }

    println!(
        "\nShaping Stage Research — {}  ({}, {} pax, window {}..{})",
        run.run_id, run.origin_code, run.pax, run.window_start, run.window_end
    );

    if !shaping.is_empty() {
        println!("\nResearch Shaping:");
        // Group by aspect preserving first-seen order.
        let mut order: Vec<String> = Vec::new();
        for s in &shaping {
            if !order.contains(&s.aspect) {
                order.push(s.aspect.clone());
            }
        }
        for aspect in &order {
            println!("  {aspect}:");
            for s in shaping.iter().filter(|s| &s.aspect == aspect) {
                let val =
                    shaping_value_display(&s.value_date, &s.value_text, &s.value_integer);
                let role_note = match s.role.as_str() {
                    "hard_constraint" => " [HARD]",
                    "soft_preference" => " [PREF]",
                    _ => "",
                };
                let val_part = if val.is_empty() {
                    String::new()
                } else {
                    format!(" = {val}")
                };
                let note = s
                    .notes
                    .as_ref()
                    .map(|n| format!("  // {n}"))
                    .unwrap_or_default();
                println!("    - {}{role_note} {}{val_part}{note}", s.role, s.kind);
            }
        }
    }

    println!();
    if candidates.is_empty() {
        println!("(no candidates — run the aggregator first)\n");
        return Ok(());
    }

    let header = format!(
        "{:<3} {:<5} {:<12} {:<12} {:<7} {:<16} {:<6} {}",
        "#", "Dest", "Depart", "Return", "Nights", "Flight (party)", "Leave", "Verdict"
    );
    println!("{header}");
    println!("{}", "─".repeat(header.chars().count()));
    for c in &candidates {
        let price = match c.flight_total_twd {
            None => "n/a".to_string(),
            Some(p) => format!("{} {}", run.currency, fmt_thousands(p)),
        };
        println!(
            "{:<3} {:<5} {:<12} {:<12} {:<7} {:<16} {:<6} {}",
            c.rank.map(|r| r.to_string()).unwrap_or_else(|| "-".into()),
            c.dest_code,
            c.depart_date,
            c.return_date,
            format!("{}n", c.nights),
            price,
            c.leave_days
                .map(|l| l.to_string())
                .unwrap_or_else(|| "-".into()),
            c.verdict.clone().unwrap_or_default(),
        );
    }
    println!();
    Ok(())
}

fn js_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn opt_str(v: &Option<String>) -> String {
    match v {
        Some(s) => js_str(s),
        None => "null".to_string(),
    }
}

fn opt_int(v: &Option<i64>) -> String {
    match v {
        Some(i) => i.to_string(),
        None => "null".to_string(),
    }
}

// ── shaping-adopt ────────────────────────────────────────────────────
// Without --create-plan: just sets adopted_plan_id + run status='adopted'.
// With --create-plan --dest <slug>: seeds a new plan (P1 dates + P2
// destination + process_statuses + event log) from the candidate, mirroring
// adoptCandidateToNewPlan in shaping-service.ts.

pub async fn run_adopt(args: &[String]) -> Result<(), String> {
    if has_flag(args, "--help") || has_flag(args, "-h") {
        println!(
            "Usage:\n  travel shaping-adopt <candidate_id> <plan_id> [--create-plan --dest <slug>]"
        );
        return Ok(());
    }

    // positional args (skip flags + their values)
    let positionals: Vec<&String> = collect_positionals(args, &["--dest"]);
    let candidate_id = positionals.first().map(|s| s.as_str());
    let plan_id = positionals.get(1).map(|s| s.as_str());
    let (candidate_id, plan_id) = match (candidate_id, plan_id) {
        (Some(c), Some(p)) => (c.to_string(), p.to_string()),
        _ => {
            eprintln!("Error: shaping-adopt requires <candidate_id> <plan_id>");
            std::process::exit(1);
        }
    };

    let conn = db::connect_write().await?;

    if has_flag(args, "--create-plan") {
        let Some(dest_slug) = opt(args, "--dest") else {
            eprintln!("Error: shaping-adopt --create-plan requires --dest <destination_slug>");
            std::process::exit(1);
        };
        adopt_candidate_to_new_plan(&conn, &candidate_id, &plan_id, &dest_slug).await?;
        println!("✅ Candidate {candidate_id} adopted into new plan {plan_id}");
        println!("   Destination: {dest_slug}");
        println!("   P1 dates and P2 destination are seeded from the Shaping Stage candidate.");
        println!(
            "   Next: npm run travel -- scaffold-itinerary --plan-id {plan_id} --dest {dest_slug}"
        );
        return Ok(());
    }

    // simple adopt
    let run_id = candidate_run_id(&conn, &candidate_id).await?;
    let ts = now_rfc3339();
    travel_db::repo::shaping::set_adopted(&conn, &candidate_id, &plan_id, &run_id, &ts).await?;
    println!("✅ Candidate {candidate_id} adopted into plan {plan_id}");
    println!("   Next: set the locked dates/destination via /p1-dates and /p2-destination");
    Ok(())
}

fn collect_positionals<'a>(args: &'a [String], value_flags: &[&str]) -> Vec<&'a String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a.starts_with("--") {
            if value_flags.contains(&a.as_str()) {
                i += 2; // skip flag + its value
            } else {
                i += 1; // boolean flag
            }
            continue;
        }
        out.push(a);
        i += 1;
    }
    out
}

async fn candidate_run_id(conn: &Connection, candidate_id: &str) -> Result<String, String> {
    let mut rows = conn
        .query(
            "SELECT run_id FROM shaping_candidates WHERE candidate_id = ?1",
            params![candidate_id.to_string()],
        )
        .await
        .map_err(|e| format!("query candidate failed: {e}"))?;
    match rows.next().await.map_err(|e| format!("row read: {e}"))? {
        Some(row) => Ok(row.get::<String>(0).unwrap_or_default()),
        None => Err(format!("Shaping Stage candidate not found: {candidate_id}")),
    }
}

async fn adopt_candidate_to_new_plan(
    conn: &Connection,
    candidate_id: &str,
    plan_id: &str,
    dest_slug: &str,
) -> Result<(), String> {
    let schema_version = "4.2.0";
    let ts = now_rfc3339();

    // candidate + origin
    let mut rows = conn
        .query(
            "SELECT c.run_id, c.dest_code, c.depart_date, c.return_date, c.nights, r.origin_code
             FROM shaping_candidates c
             JOIN shaping_research_runs r ON r.run_id = c.run_id
             WHERE c.candidate_id = ?1",
            params![candidate_id.to_string()],
        )
        .await
        .map_err(|e| format!("query candidate failed: {e}"))?;
    let Some(crow) = rows.next().await.map_err(|e| format!("row read: {e}"))? else {
        return Err(format!("Shaping Stage candidate not found: {candidate_id}"));
    };
    let run_id: String = crow.get(0).unwrap_or_default();
    let dest_code: String = crow.get(1).unwrap_or_default();
    let start_date: String = crow.get(2).unwrap_or_default();
    let end_date: String = crow.get(3).unwrap_or_default();
    let nights: i64 = crow.get(4).unwrap_or(0);
    let origin_code: Option<String> = crow.get(5).unwrap_or(None);
    let days = nights + 1;

    // plan already exists?
    let mut prow = conn
        .query(
            "SELECT plan_id FROM plans WHERE plan_id = ?1
             UNION SELECT plan_id FROM plan_metadata WHERE plan_id = ?1",
            params![plan_id.to_string()],
        )
        .await
        .map_err(|e| format!("query plans failed: {e}"))?;
    if prow
        .next()
        .await
        .map_err(|e| format!("row read: {e}"))?
        .is_some()
    {
        return Err(format!("Plan already exists: {plan_id}"));
    }

    // destination config
    let mut drow = conn
        .query(
            "SELECT display_name, ref_id FROM destination_config WHERE slug = ?1",
            params![dest_slug.to_string()],
        )
        .await
        .map_err(|e| format!("query destination_config failed: {e}"))?;
    let Some(dest_row) = drow.next().await.map_err(|e| format!("row read: {e}"))? else {
        return Err(format!("Destination config not found: {dest_slug}"));
    };
    let display_name: String = dest_row
        .get::<Option<String>>(0)
        .unwrap_or(None)
        .unwrap_or_else(|| dest_slug.to_string());
    let region: String = dest_row
        .get::<Option<String>>(1)
        .unwrap_or(None)
        .unwrap_or_else(|| dest_slug.to_string());

    // configured airports validation
    let mut arows = conn
        .query(
            "SELECT airport FROM destination_airports WHERE slug = ?1 ORDER BY sort_order",
            params![dest_slug.to_string()],
        )
        .await
        .map_err(|e| format!("query destination_airports failed: {e}"))?;
    let mut configured: Vec<String> = Vec::new();
    while let Some(ar) = arows.next().await.map_err(|e| format!("row read: {e}"))? {
        configured.push(ar.get::<String>(0).unwrap_or_default().to_uppercase());
    }
    let primary_airport = dest_code.clone();
    if !configured.is_empty() && !configured.contains(&primary_airport.to_uppercase()) {
        return Err(format!(
            "Candidate destination {primary_airport} does not match destination {dest_slug} \
             (configured airports: {})",
            configured.join(", ")
        ));
    }

    // shaping rows (for the event summary)
    let shaping_rows = get_research_shaping(conn, &run_id).await?;
    let shaping_summary = shaping_rows
        .iter()
        .map(|s| {
            let v = shaping_value_display(&s.value_date, &s.value_text, &s.value_integer);
            format!("{}/{}/{}={}", s.aspect, s.role, s.kind, v)
        })
        .collect::<Vec<_>>()
        .join("; ");
    let session = &ts[..10.min(ts.len())];

    // Sequential inserts (mirrors adoptCandidateToNewPlan executeMany order).
    let stmts: Vec<(&str, Vec<libsql::Value>)> = vec![
        (
            "INSERT INTO plans (plan_id, schema_version, updated_at) VALUES (?1, ?2, datetime('now'))",
            vec![plan_id.into(), schema_version.into()],
        ),
        (
            "INSERT INTO plan_metadata (plan_id, schema_version, active_destination, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))",
            vec![plan_id.into(), schema_version.into(), dest_slug.into()],
        ),
        (
            "INSERT INTO plan_destinations (plan_id, slug, display_name, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'active', ?4, ?5)",
            vec![plan_id.into(), dest_slug.into(), display_name.clone().into(), ts.clone().into(), ts.clone().into()],
        ),
        (
            "INSERT INTO destination_details (plan_id, destination, origin_city, region, primary_airport, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            vec![
                plan_id.into(),
                dest_slug.into(),
                origin_code.clone().map(libsql::Value::from).unwrap_or(libsql::Value::Null),
                region.clone().into(),
                primary_airport.clone().into(),
            ],
        ),
        (
            "INSERT INTO destination_cities (plan_id, destination, city_slug, display_name, role, nights, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'primary', ?5, datetime('now'))",
            vec![plan_id.into(), dest_slug.into(), dest_slug.into(), display_name.clone().into(), nights.into()],
        ),
        (
            "INSERT INTO date_anchors (plan_id, destination, start_date, end_date, days, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            vec![plan_id.into(), dest_slug.into(), start_date.clone().into(), end_date.clone().into(), days.into()],
        ),
    ];
    for (sql, p) in stmts {
        conn.execute(sql, params_from_iter(p))
            .await
            .map_err(|e| format!("adopt insert failed ({sql:.40}): {e}"))?;
    }

    // process_statuses
    let proc_rows: [(&str, &str); 7] = [
        ("process_1_date_anchor", "confirmed"),
        ("process_2_destination", "confirmed"),
        ("process_3_transportation", "pending"),
        ("process_3_4_packages", "pending"),
        ("process_4_accommodation", "pending"),
        ("process_5_daily_itinerary", "pending"),
        // (TS lists 6 rows; only the above; keep array sized 7 but last unused)
        ("", ""),
    ];
    for (pid, st) in proc_rows.iter().filter(|(p, _)| !p.is_empty()) {
        conn.execute(
            "INSERT INTO process_statuses (plan_id, destination, process_id, status, updated_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            params![plan_id, dest_slug, *pid, *st],
        )
        .await
        .map_err(|e| format!("insert process_statuses failed: {e}"))?;
    }

    // event log
    conn.execute(
        "INSERT INTO event_log_state (plan_id, session, project, version, current_focus, active_destination)
         VALUES (?1, ?2, 'japan-travel', '3.0', '', ?3)",
        params![plan_id, session, dest_slug],
    )
    .await
    .map_err(|e| format!("insert event_log_state failed: {e}"))?;
    conn.execute(
        "INSERT INTO event_log_destinations (plan_id, destination, status)
         VALUES (?1, ?2, 'active')",
        params![plan_id, dest_slug],
    )
    .await
    .map_err(|e| format!("insert event_log_destinations failed: {e}"))?;

    // plan_events + plan_event_data KV
    conn.execute(
        "INSERT INTO plan_events (plan_id, scope, destination, process_id, sort_order, event, event_at)
         VALUES (?1, 'timeline', '', '', 0, 'shaping_candidate_adopted', ?2)",
        params![plan_id, ts.clone()],
    )
    .await
    .map_err(|e| format!("insert plan_events failed: {e}"))?;

    let kv: Vec<(String, String)> = vec![
        ("candidate_id".into(), candidate_id.to_string()),
        ("run_id".into(), run_id.clone()),
        ("depart_date".into(), start_date.clone()),
        ("return_date".into(), end_date.clone()),
        ("dest_code".into(), primary_airport.clone()),
        ("shaping_count".into(), shaping_rows.len().to_string()),
        ("shaping_summary".into(), shaping_summary),
    ];
    for (k, v) in kv {
        conn.execute(
            "INSERT INTO plan_event_data (plan_id, scope, destination, process_id, sort_order, key, value)
             VALUES (?1, 'timeline', '', '', 0, ?2, ?3)",
            params![plan_id, k, v],
        )
        .await
        .map_err(|e| format!("insert plan_event_data failed: {e}"))?;
    }

    // Audit triad back half: the plan is brand new (plans.version defaults to
    // 0), so record the adoption as version 0 -> 1 with an operation_runs row.
    // Without this the adoption left no audit row and never advanced the
    // version counter (the rest of the CLI relies on both).
    record_operation(
        conn,
        plan_id,
        "shaping-adopt",
        candidate_id,
        0,
        1,
        &now_db_datetime(),
    )
    .await?;

    // pointers
    conn.execute(
        "UPDATE shaping_candidates SET adopted_plan_id = ?1 WHERE candidate_id = ?2",
        params![plan_id, candidate_id],
    )
    .await
    .map_err(|e| format!("update shaping_candidates failed: {e}"))?;
    conn.execute(
        "UPDATE shaping_research_runs SET status = 'adopted', updated_at = ?1 WHERE run_id = ?2",
        params![ts, run_id.clone()],
    )
    .await
    .map_err(|e| format!("update shaping_research_runs failed: {e}"))?;

    // Bridge the tour-group baseline audit set into the new plan (matches the TS
    // adoptCandidateToNewPlan flow). Region = destination_config.ref_id. Non-fatal:
    // if no tour-group offers were scraped for this run/region/nights, no-op.
    match crate::tour_group_bridge::bridge_audit_set(
        conn, &run_id, plan_id, dest_slug, &region, nights, None,
    )
    .await
    {
        Ok(n) if n > 0 => eprintln!("ℹ️  Bridged {n} tour-group baseline offers into {plan_id}."),
        Ok(_) => {}
        Err(e) => eprintln!("⚠️  Tour-group bridge skipped: {e}"),
    }

    Ok(())
}

// ── shaping-baseline ─────────────────────────────────────────────────
// Methodology view: group tour-group offers by (dest_region, depart_date,
// nights); pick cheapest group_tour vs cheapest fit per group; show savings.

struct TgOffer {
    dest_region: String,
    depart_date: String,
    return_date: Option<String>,
    nights: i64,
    price: i64,
    source_id: String,
    product_kind: String,
    raw_confidence: Option<String>,
}

struct BaselineRow {
    dest_region: String,
    depart_date: String,
    nights: i64,
    gt_price: Option<i64>,
    gt_source: Option<String>,
    gt_conf: Option<String>,
    gt_count: usize,
    fit_price: Option<i64>,
    fit_source: Option<String>,
    fit_conf: Option<String>,
    fit_count: usize,
    savings: Option<i64>,
    savings_pct: Option<i64>,
}

pub async fn run_baseline(args: &[String]) -> Result<(), String> {
    if has_flag(args, "--help") || has_flag(args, "-h") {
        println!(
            "Usage:\n  travel shaping-baseline --run <run_id> [--dest-region <region>] [--nights N]"
        );
        return Ok(());
    }
    let Some(run_id) = opt(args, "--run") else {
        eprintln!("Error: shaping-baseline requires --run <run_id>");
        std::process::exit(1);
    };
    let conn = db::connect_read().await?;
    let Some(run) = get_research_run(&conn, &run_id).await? else {
        eprintln!("Error: research run not found: {run_id}");
        std::process::exit(1);
    };

    let filter_region = opt(args, "--dest-region");
    let filter_nights: Option<i64> = opt(args, "--nights").and_then(|s| s.parse().ok());

    // listTourGroupOffers (run + optional region/nights), ORDER BY price ASC.
    let mut sql = String::from(
        "SELECT dest_region, depart_date, return_date, nights, price_per_person_twd, \
         source_id, product_kind, raw_confidence \
         FROM shaping_tour_group_offers WHERE run_id = ?1",
    );
    let mut binds: Vec<libsql::Value> = vec![run_id.clone().into()];
    let mut n = 2;
    if let Some(ref r) = filter_region {
        sql.push_str(&format!(" AND dest_region = ?{n}"));
        binds.push(r.clone().into());
        n += 1;
    }
    if let Some(v) = filter_nights {
        sql.push_str(&format!(" AND nights = ?{n}"));
        binds.push(v.into());
    }
    sql.push_str(" ORDER BY price_per_person_twd ASC");

    let mut rows = conn
        .query(&sql, params_from_iter(binds))
        .await
        .map_err(|e| format!("query shaping_tour_group_offers failed: {e}"))?;
    let mut offers: Vec<TgOffer> = Vec::new();
    while let Some(row) = rows.next().await.map_err(|e| format!("row read: {e}"))? {
        offers.push(TgOffer {
            dest_region: row.get::<String>(0).unwrap_or_default(),
            depart_date: row.get::<String>(1).unwrap_or_default(),
            return_date: row.get::<Option<String>>(2).unwrap_or(None),
            nights: row.get::<i64>(3).unwrap_or(0),
            price: row.get::<i64>(4).unwrap_or(0),
            source_id: row.get::<String>(5).unwrap_or_default(),
            product_kind: row.get::<Option<String>>(6).unwrap_or(None).unwrap_or_default(),
            raw_confidence: row.get::<Option<String>>(7).unwrap_or(None),
        });
    }

    // group by (region, depart, nights) preserving insertion order
    let mut keys: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (idx, o) in offers.iter().enumerate() {
        let key = format!("{}|{}|{}", o.dest_region, o.depart_date, o.nights);
        if !groups.contains_key(&key) {
            keys.push(key.clone());
        }
        groups.entry(key).or_default().push(idx);
    }

    let mut baseline_rows: Vec<BaselineRow> = Vec::new();
    for key in &keys {
        let idxs = &groups[key];
        let mut gts: Vec<&TgOffer> = idxs
            .iter()
            .map(|&i| &offers[i])
            .filter(|o| o.product_kind == "group_tour")
            .collect();
        let mut fits: Vec<&TgOffer> = idxs
            .iter()
            .map(|&i| &offers[i])
            .filter(|o| o.product_kind == "fit")
            .collect();
        gts.sort_by_key(|o| o.price);
        fits.sort_by_key(|o| o.price);

        let cheapest_gt = gts.first();
        let cheapest_fit = fits.first();
        let gt_price = cheapest_gt.map(|o| o.price);
        let fit_price = cheapest_fit.map(|o| o.price);
        let savings = match (gt_price, fit_price) {
            (Some(g), Some(f)) => Some(g - f),
            _ => None,
        };
        let pct = match (savings, gt_price) {
            (Some(s), Some(g)) if g != 0 => {
                Some(((s as f64 / g as f64) * 100.0).round() as i64)
            }
            _ => None,
        };
        let first = &offers[idxs[0]];
        baseline_rows.push(BaselineRow {
            dest_region: first.dest_region.clone(),
            depart_date: first.depart_date.clone(),
            nights: first.nights,
            gt_price,
            gt_source: cheapest_gt.map(|o| o.source_id.clone()),
            gt_conf: cheapest_gt.and_then(|o| o.raw_confidence.clone()),
            gt_count: gts.len(),
            fit_price,
            fit_source: cheapest_fit.map(|o| o.source_id.clone()),
            fit_conf: cheapest_fit.and_then(|o| o.raw_confidence.clone()),
            fit_count: fits.len(),
            savings,
            savings_pct: pct,
        });
        let _ = first.return_date.clone(); // return_date carried in offers, not displayed
    }

    // sort: region, depart_date, nights
    baseline_rows.sort_by(|a, b| {
        a.dest_region
            .cmp(&b.dest_region)
            .then(a.depart_date.cmp(&b.depart_date))
            .then(a.nights.cmp(&b.nights))
    });

    println!(
        "\nShaping Stage Baseline — {}  ({}, {} pax, window {}..{})",
        run.run_id, run.origin_code, run.pax, run.window_start, run.window_end
    );
    let mut fparts = Vec::new();
    if let Some(ref r) = filter_region {
        fparts.push(format!("region={r}"));
    }
    if let Some(v) = filter_nights {
        fparts.push(format!("nights={v}"));
    }
    if !fparts.is_empty() {
        println!("Filter: {}", fparts.join(", "));
    }
    println!();

    if baseline_rows.is_empty() {
        println!("(no tour-group offers in this run — scrape or import some first)\n");
        return Ok(());
    }

    println!(
        "REGION    DEPART       N  GROUP_TOUR (cheapest)         FIT (cheapest)                SAVINGS"
    );
    println!("{}", "─".repeat(105));

    for row in &baseline_rows {
        let gt_cell = match row.gt_price {
            Some(p) => format!(
                "{} {:<3} {:<16}",
                fmt_price(Some(p)),
                fmt_count(row.gt_count),
                truncate(&fmt_src(&row.gt_source, &row.gt_conf), 16)
            ),
            None => "       -                          ".to_string(),
        };
        let fit_cell = match row.fit_price {
            Some(p) => format!(
                "{} {:<3} {:<16}",
                fmt_price(Some(p)),
                fmt_count(row.fit_count),
                truncate(&fmt_src(&row.fit_source, &row.fit_conf), 16)
            ),
            None => "       -                          ".to_string(),
        };
        let savings_cell = match (row.savings, row.savings_pct) {
            (Some(s), Some(p)) => format!("cheaper by {} ({}%)", fmt_thousands(s), p),
            _ => String::new(),
        };
        println!(
            "{:<9} {:<12} {} {gt_cell}  {fit_cell}  {savings_cell}",
            row.dest_region,
            row.depart_date,
            format!("{}n", row.nights),
        );
    }
    println!();
    println!("Methodology: GROUP_TOUR = ceiling (the \"I gave up shopping\" upper bound). FIT = comparable; DIY must beat the FIT floor.");
    println!("Confidence tags in [brackets] from raw_confidence: high = bookable verified; medium = listing-shown; low = teaser/inferred.");
    println!();
    Ok(())
}

fn fmt_price(p: Option<i64>) -> String {
    match p {
        None => "       -".to_string(),
        Some(v) => format!("{:>8}", fmt_thousands(v)),
    }
}

fn fmt_count(n: usize) -> String {
    if n == 0 {
        " ".to_string()
    } else {
        format!("×{n}")
    }
}

fn fmt_src(s: &Option<String>, conf: &Option<String>) -> String {
    match s {
        None => String::new(),
        Some(src) => match conf {
            Some(c) => format!("{src} [{c}]"),
            None => src.clone(),
        },
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

// ── shaping-export ───────────────────────────────────────────────────
// Emits the run (run + destinations + durations + scrape attempts + shaping)
// as a single JSON object on stdout — this is a machine handoff to the Python
// aggregator (an external protocol where JSON is the required boundary), NOT a
// user-facing artifact, so JSON here is permitted.

pub async fn run_export(args: &[String]) -> Result<(), String> {
    if has_flag(args, "--help") || has_flag(args, "-h") {
        println!("Usage:\n  travel shaping-export --run <run_id> --json");
        return Ok(());
    }
    let Some(run_id) = opt(args, "--run") else {
        eprintln!("Error: shaping-export requires --run <run_id>");
        std::process::exit(1);
    };
    let conn = db::connect_read().await?;
    let Some(run) = get_research_run(&conn, &run_id).await? else {
        eprintln!("Error: research run not found: {run_id}");
        std::process::exit(1);
    };

    // destinations
    let mut drows = conn
        .query(
            "SELECT dest_code, dest_label, sort_order FROM shaping_research_destinations
             WHERE run_id = ?1 ORDER BY sort_order",
            params![run_id.clone()],
        )
        .await
        .map_err(|e| format!("query destinations failed: {e}"))?;
    let mut dests: Vec<String> = Vec::new();
    while let Some(r) = drows.next().await.map_err(|e| format!("row: {e}"))? {
        dests.push(format!(
            "{{\"dest_code\":{},\"dest_label\":{},\"sort_order\":{}}}",
            js_str(&r.get::<String>(0).unwrap_or_default()),
            js_str(&r.get::<String>(1).unwrap_or_default()),
            r.get::<i64>(2).unwrap_or(0),
        ));
    }

    // durations
    let mut urows = conn
        .query(
            "SELECT nights, duration_days FROM shaping_research_durations
             WHERE run_id = ?1 ORDER BY nights",
            params![run_id.clone()],
        )
        .await
        .map_err(|e| format!("query durations failed: {e}"))?;
    let mut durs: Vec<String> = Vec::new();
    while let Some(r) = urows.next().await.map_err(|e| format!("row: {e}"))? {
        durs.push(format!(
            "{{\"nights\":{},\"duration_days\":{}}}",
            r.get::<i64>(0).unwrap_or(0),
            r.get::<i64>(1).unwrap_or(0),
        ));
    }

    // scrape attempts
    let mut arows = conn
        .query(
            "SELECT dest_code, nights, status, candidate_count, error, attempted_at
             FROM shaping_scrape_attempts WHERE run_id = ?1 ORDER BY dest_code, nights",
            params![run_id.clone()],
        )
        .await
        .map_err(|e| format!("query scrape attempts failed: {e}"))?;
    let mut attempts: Vec<String> = Vec::new();
    while let Some(r) = arows.next().await.map_err(|e| format!("row: {e}"))? {
        attempts.push(format!(
            "{{\"destCode\":{},\"nights\":{},\"status\":{},\"candidateCount\":{},\"error\":{},\"attempted_at\":{}}}",
            js_str(&r.get::<String>(0).unwrap_or_default()),
            r.get::<i64>(1).unwrap_or(0),
            js_str(&r.get::<String>(2).unwrap_or_default()),
            opt_int(&r.get::<Option<i64>>(3).unwrap_or(None)),
            opt_str(&r.get::<Option<String>>(4).unwrap_or(None)),
            opt_str(&r.get::<Option<String>>(5).unwrap_or(None)),
        ));
    }

    // shaping
    let shaping = get_research_shaping(&conn, &run_id).await?;
    let shaping_json: Vec<String> = shaping
        .iter()
        .map(|s| {
            format!(
                "{{\"aspect\":{},\"role\":{},\"kind\":{},\"value_text\":{},\"value_date\":{},\"value_integer\":{},\"notes\":{}}}",
                js_str(&s.aspect),
                js_str(&s.role),
                js_str(&s.kind),
                opt_str(&s.value_text),
                opt_str(&s.value_date),
                opt_int(&s.value_integer),
                opt_str(&s.notes),
            )
        })
        .collect();

    // Single-line JSON object (mirrors TS console.log(JSON.stringify({...}))).
    println!(
        "{{\"run_id\":{},\"origin_code\":{},\"pax\":{},\"window_start\":{},\"window_end\":{},\"currency\":{},\"exchange_rate_usd_twd\":{},\"status\":{},\"destinations\":[{}],\"durations\":[{}],\"attempts\":[{}],\"shaping\":[{}]}}",
        js_str(&run.run_id),
        js_str(&run.origin_code),
        run.pax,
        js_str(&run.window_start),
        js_str(&run.window_end),
        js_str(&run.currency),
        run.exchange_rate_usd_twd,
        js_str(&run.status),
        dests.join(","),
        durs.join(","),
        attempts.join(","),
        shaping_json.join(","),
    );
    Ok(())
}

// ── shaping-import ───────────────────────────────────────────────────
// Consumes the Python aggregator's JSON handoff file. Idempotent per
// (dest, nights) pair: clears prior candidates for every pair this handoff
// processed before inserting, upserts scrape attempts, computes leave-days
// from the Turso holiday calendar, inserts candidates, then ranks the run.

pub async fn run_import(args: &[String]) -> Result<(), String> {
    if has_flag(args, "--help") || has_flag(args, "-h") {
        println!("Usage:\n  travel shaping-import --run <run_id> --file <path>");
        return Ok(());
    }
    let run_id = opt(args, "--run");
    let file = opt(args, "--file");
    let (run_id, file) = match (run_id, file) {
        (Some(r), Some(f)) => (r, f),
        _ => {
            eprintln!("Error: shaping-import requires --run <run_id> and --file <path>");
            std::process::exit(1);
        }
    };

    let conn = db::connect_write().await?;
    if get_research_run(&conn, &run_id).await?.is_none() {
        eprintln!("Error: research run not found: {run_id}");
        std::process::exit(1);
    }

    let content = std::fs::read_to_string(&file)
        .map_err(|e| format!("read file failed: {e}"))?;
    let payload: Value =
        serde_json::from_str(&content).map_err(|e| format!("parse json failed: {e}"))?;
    let candidates = payload
        .get("candidates")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let attempts = payload
        .get("attempts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Build delete set from attempts (authoritative pair list).
    let mut pairs: Vec<(String, i64)> = Vec::new();
    for a in &attempts {
        let dc = a
            .get("destCode")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let n = a.get("nights").and_then(|v| v.as_i64()).unwrap_or(0);
        let key = (dc, n);
        if !pairs.contains(&key) {
            pairs.push(key);
        }
    }
    for (dc, n) in &pairs {
        delete_candidates_for_pair(&conn, &run_id, dc, *n).await?;
    }

    // Upsert scrape attempts.
    for a in &attempts {
        let dc = a.get("destCode").and_then(|v| v.as_str()).unwrap_or("");
        let n = a.get("nights").and_then(|v| v.as_i64()).unwrap_or(0);
        let status = a.get("status").and_then(|v| v.as_str()).unwrap_or("pending");
        let cc = a.get("candidateCount").and_then(|v| v.as_i64());
        let err = a.get("error").and_then(|v| v.as_str());
        conn.execute(
            "INSERT OR REPLACE INTO shaping_scrape_attempts
              (run_id, dest_code, nights, status, candidate_count, error, attempted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                run_id.clone(),
                dc.to_string(),
                n,
                status.to_string(),
                cc,
                err.map(|s| s.to_string()),
                now_rfc3339(),
            ],
        )
        .await
        .map_err(|e| format!("upsert scrape attempt failed: {e}"))?;
    }

    // Insert candidates (leave-days from the Turso holiday calendar, market=taiwan).
    let mut inserted = 0usize;
    for c in &candidates {
        let candidate_id = c.get("candidateId").and_then(|v| v.as_str()).unwrap_or("");
        let dest_code = c.get("destCode").and_then(|v| v.as_str()).unwrap_or("");
        let depart = c.get("departDate").and_then(|v| v.as_str()).unwrap_or("");
        let ret = c.get("returnDate").and_then(|v| v.as_str()).unwrap_or("");
        let nights = c.get("nights").and_then(|v| v.as_i64()).unwrap_or(0);
        let flight_total = c.get("flightTotalTwd").and_then(|v| v.as_i64());
        let verdict = c.get("verdict").and_then(|v| v.as_str());

        let leave_days = compute_leave_days(depart, ret).await?;

        conn.execute(
            "INSERT INTO shaping_candidates
              (candidate_id, run_id, dest_code, depart_date, return_date, nights,
               flight_total_twd, leave_days, rank, verdict, adopted_plan_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, NULL)",
            params![
                candidate_id.to_string(),
                run_id.clone(),
                dest_code.to_string(),
                depart.to_string(),
                ret.to_string(),
                nights,
                flight_total,
                leave_days,
                verdict.map(|s| s.to_string()),
            ],
        )
        .await
        .map_err(|e| format!("insert shaping_candidates failed: {e}"))?;

        // candidate flights
        if let Some(flights) = c.get("flights").and_then(|v| v.as_array()) {
            for f in flights {
                let direction = f.get("direction").and_then(|v| v.as_str()).unwrap_or("");
                let airline = f.get("airline").and_then(|v| v.as_str());
                let depart_time = f.get("departTime").and_then(|v| v.as_str());
                let arrive_time = f.get("arriveTime").and_then(|v| v.as_str());
                let duration = f.get("duration").and_then(|v| v.as_str());
                let nonstop = f.get("nonstop").and_then(|v| v.as_bool()).map(|b| if b { 1i64 } else { 0 });
                let price = f.get("priceTotalTwd").and_then(|v| v.as_i64());
                conn.execute(
                    "INSERT INTO shaping_candidate_flights
                      (candidate_id, direction, airline, depart_time, arrive_time, duration, nonstop, price_total_twd)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        candidate_id.to_string(),
                        direction.to_string(),
                        airline.map(|s| s.to_string()),
                        depart_time.map(|s| s.to_string()),
                        arrive_time.map(|s| s.to_string()),
                        duration.map(|s| s.to_string()),
                        nonstop,
                        price,
                    ],
                )
                .await
                .map_err(|e| format!("insert shaping_candidate_flights failed: {e}"))?;
            }
        }
        inserted += 1;
    }

    // Rank if any candidates exist; otherwise mark failed.
    let all = get_candidates(&conn, &run_id).await?;
    if all.is_empty() {
        conn.execute(
            "UPDATE shaping_research_runs SET status = 'failed', updated_at = ?1 WHERE run_id = ?2",
            params![now_rfc3339(), run_id.clone()],
        )
        .await
        .map_err(|e| format!("update status failed: {e}"))?;
        println!("⚠️  No candidates for {run_id} — run marked failed.");
        return Ok(());
    }
    rank_run(&conn, &run_id).await?;
    println!(
        "✅ Imported {inserted} candidates for {run_id} ({} total), ranked.",
        all.len()
    );
    println!("   View: npm run travel -- shaping-compare --run {run_id}");
    Ok(())
}

async fn delete_candidates_for_pair(
    conn: &Connection,
    run_id: &str,
    dest_code: &str,
    nights: i64,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM shaping_candidate_flights WHERE candidate_id IN
          (SELECT candidate_id FROM shaping_candidates
           WHERE run_id = ?1 AND dest_code = ?2 AND nights = ?3)",
        params![run_id.to_string(), dest_code.to_string(), nights],
    )
    .await
    .map_err(|e| format!("delete candidate flights failed: {e}"))?;
    conn.execute(
        "DELETE FROM shaping_candidates WHERE run_id = ?1 AND dest_code = ?2 AND nights = ?3",
        params![run_id.to_string(), dest_code.to_string(), nights],
    )
    .await
    .map_err(|e| format!("delete candidates failed: {e}"))?;
    Ok(())
}

async fn compute_leave_days(depart: &str, ret: &str) -> Result<Option<i64>, String> {
    if depart.is_empty() || ret.is_empty() {
        return Ok(None);
    }
    let year = crate::leave::year_from_date(depart)?;
    let calendar = db::load_holiday_calendar("taiwan", year).await?;
    let result = crate::leave::calculate_leave_days(depart, ret, &calendar)?;
    Ok(Some(result.leave_days as i64))
}

/// Rank by flight_total_twd ASC (NULL last), then leave_days ASC (NULL last),
/// then depart_date ASC; write 1-based rank, set run status='ranked'.
async fn rank_run(conn: &Connection, run_id: &str) -> Result<(), String> {
    let mut rows = conn
        .query(
            "SELECT candidate_id, flight_total_twd, leave_days, depart_date
             FROM shaping_candidates WHERE run_id = ?1",
            params![run_id.to_string()],
        )
        .await
        .map_err(|e| format!("query candidates for rank failed: {e}"))?;
    let mut list: Vec<(String, Option<i64>, Option<i64>, String)> = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("row: {e}"))? {
        list.push((
            r.get::<String>(0).unwrap_or_default(),
            r.get::<Option<i64>>(1).unwrap_or(None),
            r.get::<Option<i64>>(2).unwrap_or(None),
            r.get::<String>(3).unwrap_or_default(),
        ));
    }
    list.sort_by(|a, b| {
        let pa = a.1.unwrap_or(i64::MAX);
        let pb = b.1.unwrap_or(i64::MAX);
        pa.cmp(&pb)
            .then_with(|| a.2.unwrap_or(i64::MAX).cmp(&b.2.unwrap_or(i64::MAX)))
            .then_with(|| a.3.cmp(&b.3))
    });
    for (i, (cid, _, _, _)) in list.iter().enumerate() {
        conn.execute(
            "UPDATE shaping_candidates SET rank = ?1 WHERE candidate_id = ?2",
            params![(i as i64) + 1, cid.clone()],
        )
        .await
        .map_err(|e| format!("update rank failed: {e}"))?;
    }
    conn.execute(
        "UPDATE shaping_research_runs SET status = 'ranked', updated_at = ?1 WHERE run_id = ?2",
        params![now_rfc3339(), run_id.to_string()],
    )
    .await
    .map_err(|e| format!("update run status failed: {e}"))?;
    Ok(())
}
