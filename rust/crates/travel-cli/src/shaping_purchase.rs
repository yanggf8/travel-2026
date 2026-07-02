// `travel shaping-purchase-matrix --run <run_id> [--qualified-only] [--limit N]`
// — read-only purchase decision matrix for one Shaping run (the "purchase helper").
//
// Scores every purchase OPTION (each flight candidate + each package offer) against that run's
// shaping_rules: HARD rules + intrinsic purchasability as GATES (violate ⇒ DISQUALIFIED, shown with
// the reason), SOFT rules as NUDGES (integer score). Plain-text; NO writes, NO new schema. Reads via
// travel_db::repo::shaping_purchase. Complements shaping-compare (date rank) / shaping-baseline
// (group-tour-vs-FIT) — this is the constraint-fit view. Spec:
// docs/plans/2026-07-02-shaping-purchase-matrix-impl-plan.md.

use crate::db;
use travel_db::repo::shaping_purchase as repo;

fn opt(args: &[String], flag: &str) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

/// One scored purchase option (a flight candidate or a package offer).
struct Option_ {
    id: String,           // "flight:<cid>" | "offer:<oid>"
    kind: &'static str,   // "direct" | "package"
    cost_scope: &'static str, // "FLIGHT_ONLY" | "PACKAGE_TOTAL"
    source: String,
    depart: String,
    ret: String,
    nights: i64,
    pp_twd: Option<i64>,   // per-person (packages); None for flight
    total_twd: Option<i64>, // party total
    hotel: String,
    leave_days: Option<i64>,
    score: i64,
    disqualified: bool,
    reasons: Vec<String>, // gate FAIL reasons + flags
}

pub async fn run(args: &[String]) -> Result<(), String> {
    if has_flag(args, "--help") || has_flag(args, "-h") {
        println!(
            "Usage:\n  travel shaping-purchase-matrix --run <run_id> [--qualified-only] [--limit N]\n\n\
             Read-only: scores each purchase option (flight candidates + package offers) against the\n\
             run's shaping_rules. HARD rules + availability are GATES (violate ⇒ DISQUALIFIED);\n\
             SOFT rules are NUDGES (score). DISQUALIFIED rows shown by default (use --qualified-only to hide)."
        );
        return Ok(());
    }
    let Some(run_id) = opt(args, "--run") else {
        return Err("Error: shaping-purchase-matrix requires --run <run_id>".to_string());
    };
    let qualified_only = has_flag(args, "--qualified-only");
    let limit: Option<usize> = opt(args, "--limit").and_then(|s| s.parse().ok());

    let conn = db::connect_read().await?;
    let Some(header) = repo::run_header(&conn, &run_id).await? else {
        return Err(format!("Error: research run not found: {run_id}"));
    };
    let rules = repo::rules(&conn, &run_id).await?;
    let candidates = repo::candidates(&conn, &run_id).await?;
    let offers = repo::offers(&conn, &run_id).await?;

    let pax = header.pax.max(1);

    let mut options: Vec<Option_> = Vec::new();
    for c in &candidates {
        options.push(score_flight(c, &rules));
    }
    for o in &offers {
        options.push(score_package(o, &rules, pax));
    }

    if options.is_empty() {
        println!("(no options — import candidates/offers for this run first)");
        return Ok(());
    }

    // Sort: qualified first (DISQUALIFIED always last, regardless of score); then score desc;
    // then total_twd asc; then leave_days asc; then depart asc.
    options.sort_by(|a, b| {
        a.disqualified
            .cmp(&b.disqualified)
            .then(b.score.cmp(&a.score))
            .then(a.total_twd.unwrap_or(i64::MAX).cmp(&b.total_twd.unwrap_or(i64::MAX)))
            .then(a.leave_days.unwrap_or(i64::MAX).cmp(&b.leave_days.unwrap_or(i64::MAX)))
            .then(a.depart.cmp(&b.depart))
    });

    print!("{}", render(&run_id, &header, &options, qualified_only, limit));
    Ok(())
}

/// Score a flight candidate (transport-only, FLIGHT_ONLY cost scope).
fn score_flight(c: &repo::CandidateRow, rules: &[repo::RuleRow]) -> Option_ {
    let mut o = Option_ {
        id: format!("flight:{}", c.candidate_id),
        kind: "direct",
        cost_scope: "FLIGHT_ONLY",
        source: "direct".to_string(),
        depart: c.depart_date.clone(),
        ret: c.return_date.clone(),
        nights: c.nights,
        pp_twd: None,
        total_twd: c.flight_total_twd,
        hotel: "-".to_string(),
        leave_days: c.leave_days,
        score: 0,
        disqualified: false,
        reasons: Vec::new(),
    };
    apply_date_gates(&mut o, rules);
    // Availability: N/A for flights (no departure_status captured for the flight leg).
    // Lodging exclude_hotel: N/A for flights.
    apply_soft_nudges(&mut o, rules);
    o
}

/// Score a package offer (flight+hotel, PACKAGE_TOTAL cost scope).
fn score_package(off: &repo::OfferRow, rules: &[repo::RuleRow], pax: i64) -> Option_ {
    let mut o = Option_ {
        id: format!("offer:{}", off.offer_id),
        kind: "package",
        cost_scope: "PACKAGE_TOTAL",
        source: off.source_id.clone(),
        depart: off.depart_date.clone(),
        ret: off.return_date.clone(),
        nights: off.nights,
        pp_twd: Some(off.price_per_person_twd),
        total_twd: Some(off.price_per_person_twd * pax),
        hotel: off.hotel_name.clone().unwrap_or_else(|| "-".to_string()),
        leave_days: None,
        score: 0,
        disqualified: false,
        reasons: Vec::new(),
    };
    // AVAILABILITY gate (system, not a shaping rule): a package you can't buy is out.
    match off.departure_status.as_deref() {
        Some("sold_out") => gate_fail(&mut o, "FAIL_AVAIL: sold out"),
        Some(s) if s.starts_with("limited_") => o.reasons.push("limited seats".to_string()),
        Some("booking_in_change") | None => o.reasons.push("CHECK_AVAIL: status unknown".to_string()),
        _ => {} // available | guaranteed ⇒ OK
    }
    if let Some(seats) = off.seats_available {
        if seats < pax {
            gate_fail(&mut o, &format!("FAIL_AVAIL: {seats} seats < {pax} pax"));
        }
    }
    apply_date_gates(&mut o, rules);
    // Lodging exclude_hotel gate.
    for r in rules.iter().filter(|r| r.role == "hard_constraint" && r.aspect == "lodging" && r.kind == "exclude_hotel") {
        if let Some(excl) = r.value_text.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            if o.hotel.contains(excl) {
                gate_fail(&mut o, &format!("FAIL_LODGING: excluded hotel {excl}"));
            }
        }
    }
    // mobility/no-public-bus: no structured column ⇒ CHECK, never silent-fail (don't infer from text).
    apply_soft_nudges(&mut o, rules);
    o
}

fn gate_fail(o: &mut Option_, reason: &str) {
    o.disqualified = true;
    o.reasons.push(reason.to_string());
}

/// Date hard-constraint gates, common to flights and packages.
fn apply_date_gates(o: &mut Option_, rules: &[repo::RuleRow]) {
    for r in rules.iter().filter(|r| r.role == "hard_constraint" && r.aspect == "date") {
        match r.kind.as_str() {
            "return_no_later_than" => {
                if let Some(cap) = r.value_date.as_deref().filter(|s| !s.is_empty()) {
                    if !o.ret.is_empty() && o.ret.as_str() > cap {
                        gate_fail(o, &format!("FAIL_DATE: returns {} > {cap}", o.ret));
                    }
                }
            }
            "exclude_depart" => {
                if let Some(bad) = r.value_date.as_deref().filter(|s| !s.is_empty()) {
                    if o.depart == bad {
                        gate_fail(o, &format!("FAIL_DATE: depart {bad} excluded"));
                    }
                }
            }
            "depart_window" => {
                // value_text "YYYY-MM-DD..YYYY-MM-DD"; unparseable ⇒ CHECK (never silent-pass).
                match r.value_text.as_deref().and_then(parse_window) {
                    Some((lo, hi)) => {
                        if o.depart.as_str() < lo.as_str() || o.depart.as_str() > hi.as_str() {
                            gate_fail(o, &format!("FAIL_DATE: depart {} outside {lo}..{hi}", o.depart));
                        }
                    }
                    None => o.reasons.push("CHECK_DATE: depart_window unparseable".to_string()),
                }
            }
            _ => {}
        }
    }
}

fn parse_window(v: &str) -> Option<(String, String)> {
    let (lo, hi) = v.split_once("..")?;
    let (lo, hi) = (lo.trim(), hi.trim());
    if lo.len() == 10 && hi.len() == 10 {
        Some((lo.to_string(), hi.to_string()))
    } else {
        None
    }
}

/// Soft-preference nudges (score deltas). Never disqualify.
fn apply_soft_nudges(o: &mut Option_, rules: &[repo::RuleRow]) {
    for r in rules.iter().filter(|r| r.role == "soft_preference") {
        match (r.aspect.as_str(), r.kind.as_str()) {
            ("budget", "flight_max_twd") => {
                // FLIGHT PARTY-TOTAL cap (not per-person). Nudge flights only; packages not comparable.
                if o.kind == "direct" {
                    if let (Some(total), Some(cap)) = (o.total_twd, r.value_integer) {
                        if total <= cap {
                            o.score += 2;
                        } else {
                            o.score -= 2;
                            o.reasons.push(format!("over flight cap {cap}"));
                        }
                    }
                } // package ⇒ CHECK/0 (do not penalize on a flight-only cap)
            }
            ("channel", "preferred_sources") => {
                if o.kind == "package" {
                    let list: Vec<String> = r
                        .value_text
                        .as_deref()
                        .unwrap_or("")
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !list.is_empty() {
                        if list.iter().any(|s| s == &o.source) {
                            o.score += 2;
                        } else {
                            o.score -= 1;
                        }
                    }
                } // direct flight ⇒ 0
            }
            ("lodging", "preferred_hotel_area") => {
                if o.kind == "package" {
                    if let Some(area) = r.value_text.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                        // match against hotel name (title not carried on Option_; hotel is the signal)
                        if o.hotel != "-" {
                            if o.hotel.contains(area) {
                                o.score += 1;
                            } else {
                                o.score -= 1;
                            }
                        } // unknown hotel ⇒ 0
                    }
                }
            }
            _ => {} // other soft prefs: context only, no score
        }
    }
}

fn render(
    run_id: &str,
    header: &repo::RunHeader,
    options: &[Option_],
    qualified_only: bool,
    limit: Option<usize>,
) -> String {
    let mut out = String::new();
    out.push('\n');
    out.push_str(&format!(
        "Purchase matrix — run {run_id} (pax {}, {})\n",
        header.pax, header.currency
    ));
    out.push_str(&"─".repeat(60));
    out.push('\n');
    out.push_str(
        "#  OPTION                             KIND     COST_SCOPE     SOURCE      DATES                    N  PP_TWD   TOTAL_TWD  HOTEL                VERDICT       SCORE  GATES / REASONS\n",
    );

    let mut n = 0usize;
    let mut shown = 0usize;
    for o in options {
        if qualified_only && o.disqualified {
            continue;
        }
        if let Some(lim) = limit {
            if shown >= lim {
                break;
            }
        }
        n += 1;
        shown += 1;
        let pp = o.pp_twd.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string());
        let total = o.total_twd.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string());
        let verdict = if o.disqualified { "DISQUALIFIED" } else { "QUALIFIED" };
        let reasons = o.reasons.join("; ");
        // OPTION carries the FULL id (never truncated — it's the identifier; truncating it would
        // hide which offer, e.g. two ids sharing a long run-id prefix). Pad to a min width for
        // alignment but let long ids overflow the column rather than lose the disambiguating suffix.
        out.push_str(&format!(
            "{n:<3}{:<35}{:<9}{:<15}{:<12}{} → {}  {:<3}{pp:<9}{total:<11}{:<21}{verdict:<14}{:<7}{reasons}\n",
            o.id,
            o.kind,
            o.cost_scope,
            trunc(&o.source, 11),
            o.depart,
            o.ret,
            o.nights,
            trunc(&o.hotel, 20),
            o.score,
        ));
    }
    if shown == 0 {
        out.push_str("  (all options disqualified — drop --qualified-only to see why)\n");
    }
    out.push('\n');
    out
}

/// Truncate a display string to `max` chars (char-safe for CJK).
fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}
