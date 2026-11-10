// Process status machine: one ladder per process family, forward-only path
// planning, and the shared hop writer (design: docs/plans/2026-11-11-status-machine-forward-only.md).
//
// Ladders:
//   - Anchor    (P1 date anchor, P2 destination): no selecting/booking states.
//   - Booking   (P3_4 packages, P3 transport, P4 accommodation): `confirmed` is
//     reachable only through `booked`, and leaving `confirmed` goes back through
//     `booked` (confirmed→booked is the reopen edge).
//   - Itinerary (P5 daily itinerary): `confirmed` = the itinerary is final.
//
// Path planning (`plan_path`) is pure. Every writer computes ALL of its paths
// first (the preflight) and writes only when every path is Ok; `apply_path`
// then writes each hop through `repo::process_statuses::upsert` +
// `emit_status_changed`. It never opens a transaction and never calls
// `record_operation` — the command owns both.
//
// Test seam: TRAVEL_STATUS_FAIL_AT=<status> makes `apply_path` fail right before
// it writes a hop whose target is <status>, so an integration test can prove a
// mid-command failure inside the command's transaction leaves nothing written.
// Unset in normal use.

use super::common::emit_status_changed;
use libsql::Connection;
use std::collections::{HashSet, VecDeque};

const FAULT_ENV: &str = "TRAVEL_STATUS_FAIL_AT";

pub(crate) const P1: &str = "process_1_date_anchor";
pub(crate) const P2: &str = "process_2_destination";
pub(crate) const P34: &str = "process_3_4_packages";
pub(crate) const P3: &str = "process_3_transportation";
pub(crate) const P4: &str = "process_4_accommodation";
pub(crate) const P5: &str = "process_5_daily_itinerary";

/// Every status any ladder knows, in rank order with the two unranked ones last.
pub(crate) const ALL_STATUSES: &[&str] = &[
    "pending",
    "researching",
    "researched",
    "selecting",
    "selected",
    "populated",
    "booking",
    "booked",
    "confirmed",
    "skipped",
    "cancelled",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ladder {
    Anchor,
    Booking,
    Itinerary,
}

impl Ladder {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Ladder::Anchor => "anchor",
            Ladder::Booking => "booking",
            Ladder::Itinerary => "itinerary",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Only rank-increasing moves (plus the fixed first hop out of skipped/cancelled).
    Forward,
    /// Forward moves, plus backward/special targets (pending, skipped, cancelled, lower rank).
    Any,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hop {
    pub from: &'static str,
    pub to: &'static str,
}

/// The ladder a process belongs to. Unknown process id → Err.
pub(crate) fn ladder(process_id: &str) -> Result<Ladder, String> {
    match process_id {
        P1 | P2 => Ok(Ladder::Anchor),
        P34 | P3 | P4 => Ok(Ladder::Booking),
        P5 => Ok(Ladder::Itinerary),
        other => Err(format!("unknown process id: {other}")),
    }
}

/// The legal next statuses from `from` on ladder `l` (empty when `from` is not on it).
pub(crate) fn targets(l: Ladder, from: &str) -> &'static [&'static str] {
    match l {
        Ladder::Anchor => match from {
            "pending" => &["researching", "confirmed", "skipped"],
            "researching" => &["researched", "pending", "skipped"],
            "researched" => &["confirmed", "researching", "skipped"],
            "confirmed" => &["pending", "skipped"],
            "skipped" => &["pending"],
            _ => &[],
        },
        Ladder::Booking => match from {
            // The pre-2026-11-11 shared table minus pending→confirmed, plus confirmed→booked.
            "pending" => &["researching", "populated", "skipped"],
            "researching" => &["researched", "pending", "skipped"],
            "researched" => &["selecting", "selected", "researching", "skipped"],
            "selecting" => &["selected", "researched", "skipped"],
            "selected" => &["booking", "selecting", "populated", "skipped"],
            "populated" => &["booking", "selected", "pending", "skipped"],
            "booking" => &["booked", "selected", "skipped"],
            "booked" => &["confirmed", "cancelled", "skipped"],
            "cancelled" => &["pending", "selecting", "skipped"],
            "confirmed" => &["booked", "skipped"],
            "skipped" => &["pending"],
            _ => &[],
        },
        Ladder::Itinerary => match from {
            "pending" => &["researching", "skipped"],
            "researching" => &["researched", "pending", "skipped"],
            "researched" => &["confirmed", "researching", "skipped"],
            "confirmed" => &["researched", "skipped"],
            "skipped" => &["pending"],
            _ => &[],
        },
    }
}

/// True when `status` is a state of ladder `l` (every state has an outgoing edge).
pub(crate) fn on_ladder(l: Ladder, status: &str) -> bool {
    !targets(l, status).is_empty()
}

/// Ladder rank. `skipped` and `cancelled` are unranked (None), as is anything unknown.
pub(crate) fn rank(status: &str) -> Option<u8> {
    match status {
        "pending" => Some(0),
        "researching" => Some(1),
        "researched" => Some(2),
        "selecting" => Some(3),
        "selected" => Some(4),
        "populated" => Some(5),
        "booking" => Some(6),
        "booked" => Some(7),
        "confirmed" => Some(8),
        _ => None,
    }
}

/// Map a status string to its `'static` literal, or None when it is not a known status.
pub(crate) fn status_literal(s: &str) -> Option<&'static str> {
    ALL_STATUSES.iter().copied().find(|k| *k == s)
}

/// One hop against the process's ladder. `from == to` is Ok (no move).
pub(crate) fn validate_transition(l: Ladder, from: &str, to: &str) -> Result<(), String> {
    if from == to {
        return Ok(());
    }
    if targets(l, from).contains(&to) {
        Ok(())
    } else {
        Err(format!("Invalid transition: {from} → {to} on the {} ladder", l.name()))
    }
}

/// Plan the hops from `from` to `to` on ladder `l` (pure, no I/O).
///
/// - `from == to` → empty path.
/// - unranked `from` (skipped/cancelled): a fixed first hop (skipped → pending;
///   cancelled → skipped when that is the target, → selecting when `to` ranks
///   at or above selecting, else → pending), then the rules below.
/// - forward target (`rank(to) > rank(from)`): BFS over rank-increasing edges
///   only; no fallback to an unconstrained search.
/// - backward/special target: `Mode::Any` only — BFS over all edges where an
///   intermediate may not be pending or skipped.
pub(crate) fn plan_path(l: Ladder, from: &str, to: &str, mode: Mode) -> Result<Vec<Hop>, String> {
    let from_s = status_literal(from).ok_or_else(|| format!("unknown current status: {from}"))?;
    if !on_ladder(l, from_s) {
        return Err(format!(
            "unknown current status: {from} (not on the {} ladder)",
            l.name()
        ));
    }
    let to_s = status_literal(to).ok_or_else(|| format!("unknown target status: {to}"))?;
    if !on_ladder(l, to_s) {
        return Err(format!("status {to} is not on the {} ladder", l.name()));
    }
    if from_s == to_s {
        return Ok(Vec::new());
    }

    if rank(from_s).is_none() {
        let first = match from_s {
            "skipped" => "pending",
            "cancelled" => {
                if to_s == "skipped" {
                    "skipped"
                } else if rank(to_s).is_some_and(|r| r >= RANK_SELECTING) {
                    "selecting"
                } else {
                    "pending"
                }
            }
            other => return Err(format!("unknown current status: {other}")),
        };
        validate_transition(l, from_s, first)?;
        let mut hops = vec![Hop { from: from_s, to: first }];
        if first != to_s {
            hops.extend(plan_path(l, first, to_s, mode)?);
        }
        return Ok(hops);
    }

    let forward = matches!((rank(from_s), rank(to_s)), (Some(a), Some(b)) if b > a);
    if forward {
        return bfs(l, from_s, to_s, true)
            .ok_or_else(|| format!("no legal status path from {from} to {to}"));
    }
    if mode == Mode::Forward {
        return Err(format!(
            "no legal status path from {from} to {to}: this command only moves a process forward"
        ));
    }
    bfs(l, from_s, to_s, false).ok_or_else(|| format!("no legal status path from {from} to {to}"))
}

const RANK_SELECTING: u8 = 3;

fn bfs(l: Ladder, from: &'static str, to: &'static str, forward: bool) -> Option<Vec<Hop>> {
    let mut queue: VecDeque<(&'static str, Vec<Hop>)> = VecDeque::new();
    let mut visited: HashSet<&'static str> = HashSet::new();
    queue.push_back((from, Vec::new()));
    visited.insert(from);
    while let Some((node, path)) = queue.pop_front() {
        for &next in targets(l, node) {
            if visited.contains(next) {
                continue;
            }
            if forward {
                match (rank(node), rank(next)) {
                    (Some(a), Some(b)) if b > a => {}
                    _ => continue,
                }
            } else if next != to && matches!(next, "pending" | "skipped") {
                continue;
            }
            let mut new_path = path.clone();
            new_path.push(Hop { from: node, to: next });
            if next == to {
                return Some(new_path);
            }
            visited.insert(next);
            queue.push_back((next, new_path));
        }
    }
    None
}

/// The single-hop rule select-offer uses: a missing row or `from == to` is an
/// empty path; otherwise `from → to` must be exactly one edge of the ladder.
pub(crate) fn single_hop(l: Ladder, from: Option<&str>, to: &str) -> Result<Vec<Hop>, String> {
    let to_s = status_literal(to).ok_or_else(|| format!("unknown target status: {to}"))?;
    let Some(from) = from else {
        return Ok(Vec::new());
    };
    if from == to_s {
        return Ok(Vec::new());
    }
    let from_s = status_literal(from).ok_or_else(|| format!("unknown current status: {from}"))?;
    validate_transition(l, from_s, to_s)?;
    Ok(vec![Hop { from: from_s, to: to_s }])
}

/// Raise a process to `to` only from below: a missing row → `Some(vec![])`
/// (insert it directly); a ranked status below `to` → `Some(forward path)`;
/// anything else (at/above `to`, skipped, cancelled) → `None` (left alone,
/// never regressed). promote-offers / import-offers use it for P3_4 → researched.
pub(crate) fn raise_to(l: Ladder, from: Option<&str>, to: &str) -> Result<Option<Vec<Hop>>, String> {
    let to_rank = rank(to).ok_or_else(|| format!("raise_to needs a ranked target, got {to}"))?;
    match from {
        None => Ok(Some(Vec::new())),
        Some(f) => match rank(f) {
            Some(r) if r < to_rank => Ok(Some(plan_path(l, f, to, Mode::Forward)?)),
            _ => Ok(None),
        },
    }
}

/// `a -> b -> c` (with the given separator) for the start status plus each hop target.
pub(crate) fn format_path(from: &str, hops: &[Hop], sep: &str) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(hops.len() + 1);
    parts.push(from);
    for hop in hops {
        parts.push(hop.to);
    }
    parts.join(sep)
}

/// Write a planned path: one `process_statuses` upsert + one `status_changed`
/// event (dest_process + timeline) per hop.
///
/// - `from == None` (no row yet): `hops` must be empty; the target is inserted
///   directly with one `status_changed` whose from_state is NULL.
/// - `from == Some(to)` with no hops: nothing is written.
///
/// Runs inside the caller's transaction; never calls `record_operation`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_path(
    conn: &Connection,
    plan_id: &str,
    dest: &str,
    process_id: &str,
    from: Option<&str>,
    hops: &[Hop],
    to: &str,
    now_iso: &str,
    now_db: &str,
) -> Result<(), String> {
    let l = ladder(process_id)?;
    let fault = std::env::var(FAULT_ENV).ok().filter(|v| !v.is_empty());
    // The path was planned before the command's BEGIN IMMEDIATE; re-read under
    // the write lock so a concurrent change in between fails loud instead of
    // writing hops (and from_state events) that no longer match the row.
    let now = travel_db::repo::process_statuses::get(conn, plan_id, dest, process_id).await?;
    if now.as_deref() != from {
        return Err(format!(
            "{dest}.{process_id} changed since this command planned its path (planned from {}, now {}); re-run the command",
            from.unwrap_or("<no row>"),
            now.as_deref().unwrap_or("<no row>")
        ));
    }
    let Some(from) = from else {
        if !hops.is_empty() {
            return Err(format!(
                "internal: {dest}.{process_id} has no status row but a planned path was given"
            ));
        }
        if !on_ladder(l, to) {
            return Err(format!("status {to} is not on the {} ladder", l.name()));
        }
        if fault.as_deref() == Some(to) {
            return Err(format!(
                "{FAULT_ENV}={to}: injected failure before inserting {dest}.{process_id}"
            ));
        }
        travel_db::repo::process_statuses::upsert(conn, plan_id, dest, process_id, to, now_db)
            .await?;
        return emit_status_changed(conn, plan_id, dest, process_id, None, to, now_iso).await;
    };

    if hops.is_empty() {
        if from == to {
            return Ok(());
        }
        return Err(format!(
            "internal: no planned hops for {dest}.{process_id} {from} -> {to}"
        ));
    }
    if hops[0].from != from || hops[hops.len() - 1].to != to {
        return Err(format!(
            "internal: planned path {} does not run {from} -> {to} for {dest}.{process_id}",
            format_path(hops[0].from, hops, " -> ")
        ));
    }
    let mut at = from;
    for hop in hops {
        if hop.from != at {
            return Err(format!(
                "internal: broken planned path at {} -> {} for {dest}.{process_id}",
                hop.from, hop.to
            ));
        }
        validate_transition(l, hop.from, hop.to)
            .map_err(|e| format!("{e} for {dest}.{process_id}"))?;
        if fault.as_deref() == Some(hop.to) {
            return Err(format!(
                "{FAULT_ENV}={}: injected failure before the {} -> {} hop of {dest}.{process_id}",
                hop.to, hop.from, hop.to
            ));
        }
        travel_db::repo::process_statuses::upsert(conn, plan_id, dest, process_id, hop.to, now_db)
            .await?;
        emit_status_changed(conn, plan_id, dest, process_id, Some(hop.from), hop.to, now_iso)
            .await?;
        at = hop.to;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LADDERS: [Ladder; 3] = [Ladder::Anchor, Ladder::Booking, Ladder::Itinerary];
    const MODES: [Mode; 2] = [Mode::Forward, Mode::Any];

    fn states(l: Ladder) -> Vec<&'static str> {
        ALL_STATUSES.iter().copied().filter(|s| on_ladder(l, s)).collect()
    }

    fn hops(pairs: &[(&'static str, &'static str)]) -> Vec<Hop> {
        pairs.iter().map(|(f, t)| Hop { from: f, to: t }).collect()
    }

    #[test]
    fn ladder_by_process() {
        assert_eq!(ladder(P1).unwrap(), Ladder::Anchor);
        assert_eq!(ladder(P2).unwrap(), Ladder::Anchor);
        assert_eq!(ladder(P34).unwrap(), Ladder::Booking);
        assert_eq!(ladder(P3).unwrap(), Ladder::Booking);
        assert_eq!(ladder(P4).unwrap(), Ladder::Booking);
        assert_eq!(ladder(P5).unwrap(), Ladder::Itinerary);
        assert!(ladder("process_9_bogus").is_err());
    }

    #[test]
    fn ladder_states() {
        assert_eq!(
            states(Ladder::Anchor),
            vec!["pending", "researching", "researched", "confirmed", "skipped"]
        );
        assert_eq!(
            states(Ladder::Itinerary),
            vec!["pending", "researching", "researched", "confirmed", "skipped"]
        );
        assert_eq!(states(Ladder::Booking), ALL_STATUSES.to_vec());
    }

    /// Every ladder × every (from, to, mode): an Ok path is a chain of ladder
    /// edges from `from` to `to`; a forward path from a ranked status is strictly
    /// rank-increasing with no skipped/cancelled/pending intermediate; an Any path
    /// never routes through pending/skipped except as the target or the fixed
    /// first hop out of skipped/cancelled.
    #[test]
    fn every_ladder_every_pair_every_mode() {
        let mut ok_paths = 0;
        for l in LADDERS {
            for from in states(l) {
                for to in states(l) {
                    for mode in MODES {
                        let Ok(path) = plan_path(l, from, to, mode) else {
                            continue;
                        };
                        ok_paths += 1;
                        if from == to {
                            assert!(path.is_empty(), "{l:?} {from}->{to} {mode:?} must be empty");
                            continue;
                        }
                        assert!(!path.is_empty(), "{l:?} {from}->{to} {mode:?}");
                        assert_eq!(path[0].from, from);
                        assert_eq!(path[path.len() - 1].to, to);
                        for w in path.windows(2) {
                            assert_eq!(w[0].to, w[1].from, "{l:?} {from}->{to} chain");
                        }
                        for hop in &path {
                            assert!(
                                targets(l, hop.from).contains(&hop.to),
                                "{l:?} {from}->{to} {mode:?}: {}->{} is not an edge",
                                hop.from,
                                hop.to
                            );
                        }
                        let start = if rank(from).is_none() { 1 } else { 0 };
                        let tail = &path[start..];
                        if mode == Mode::Forward {
                            // Forward never contains a backward hop.
                            for hop in tail {
                                let (a, b) = (rank(hop.from), rank(hop.to));
                                assert!(
                                    matches!((a, b), (Some(a), Some(b)) if b > a)
                                        || (rank(from).is_none() && hop.to == to && rank(to).is_none()),
                                    "{l:?} {from}->{to} Forward has a non-increasing hop {}->{}",
                                    hop.from,
                                    hop.to
                                );
                            }
                        }
                        for hop in &tail[..tail.len().saturating_sub(1)] {
                            assert!(
                                !matches!(hop.to, "pending" | "skipped"),
                                "{l:?} {from}->{to} {mode:?} routes through {}",
                                hop.to
                            );
                            if mode == Mode::Forward && rank(from).is_some() {
                                assert_ne!(hop.to, "cancelled");
                            }
                        }
                    }
                }
            }
        }
        assert!(ok_paths > 100, "sanity: {ok_paths} ok paths");
    }

    #[test]
    fn forward_targets_from_ranked_states_are_strictly_increasing() {
        for l in LADDERS {
            for from in states(l).into_iter().filter(|s| rank(s).is_some()) {
                for to in states(l) {
                    let Ok(path) = plan_path(l, from, to, Mode::Forward) else {
                        continue;
                    };
                    let mut last = rank(from).unwrap();
                    for hop in &path {
                        let r = rank(hop.to).expect("forward hop lands on a ranked status");
                        assert!(r > last, "{l:?} {from}->{to}: {}->{}", hop.from, hop.to);
                        last = r;
                    }
                }
            }
        }
    }

    #[test]
    fn booking_confirmed_always_goes_through_booked() {
        for from in states(Ladder::Booking) {
            if from == "confirmed" {
                continue;
            }
            for mode in MODES {
                if let Ok(path) = plan_path(Ladder::Booking, from, "confirmed", mode) {
                    assert!(
                        path.iter().any(|h| h.to == "booked") || from == "booked",
                        "{from}->confirmed {mode:?} skips booked: {path:?}"
                    );
                    assert_eq!(path.last().unwrap().from, "booked");
                }
            }
        }
    }

    #[test]
    fn regressions_from_the_shared_table_are_gone() {
        // researching → confirmed no longer goes through pending.
        assert_eq!(
            plan_path(Ladder::Booking, "researching", "confirmed", Mode::Forward).unwrap(),
            hops(&[
                ("researching", "researched"),
                ("researched", "selected"),
                ("selected", "booking"),
                ("booking", "booked"),
                ("booked", "confirmed"),
            ])
        );
        assert_eq!(
            plan_path(Ladder::Booking, "populated", "confirmed", Mode::Any).unwrap(),
            hops(&[("populated", "booking"), ("booking", "booked"), ("booked", "confirmed")])
        );
        assert_eq!(
            plan_path(Ladder::Booking, "selecting", "confirmed", Mode::Any).unwrap(),
            hops(&[
                ("selecting", "selected"),
                ("selected", "booking"),
                ("booking", "booked"),
                ("booked", "confirmed"),
            ])
        );
        assert_eq!(
            plan_path(Ladder::Booking, "researching", "populated", Mode::Forward).unwrap(),
            hops(&[
                ("researching", "researched"),
                ("researched", "selected"),
                ("selected", "populated"),
            ])
        );
    }

    #[test]
    fn anchor_researching_to_confirmed() {
        assert_eq!(
            plan_path(Ladder::Anchor, "researching", "confirmed", Mode::Forward).unwrap(),
            hops(&[("researching", "researched"), ("researched", "confirmed")])
        );
        assert_eq!(
            plan_path(Ladder::Anchor, "pending", "confirmed", Mode::Forward).unwrap(),
            hops(&[("pending", "confirmed")])
        );
    }

    #[test]
    fn itinerary_researching_to_confirmed_takes_two_hops() {
        assert_eq!(
            plan_path(Ladder::Itinerary, "researching", "confirmed", Mode::Any).unwrap(),
            hops(&[("researching", "researched"), ("researched", "confirmed")])
        );
    }

    #[test]
    fn confirmed_reopens_in_any() {
        assert_eq!(
            plan_path(Ladder::Anchor, "confirmed", "pending", Mode::Any).unwrap(),
            hops(&[("confirmed", "pending")])
        );
        assert_eq!(
            plan_path(Ladder::Booking, "confirmed", "selecting", Mode::Any).unwrap(),
            hops(&[
                ("confirmed", "booked"),
                ("booked", "cancelled"),
                ("cancelled", "selecting"),
            ])
        );
        assert_eq!(
            plan_path(Ladder::Itinerary, "confirmed", "researching", Mode::Any).unwrap(),
            hops(&[("confirmed", "researched"), ("researched", "researching")])
        );
        // ...but never in Forward.
        assert!(plan_path(Ladder::Booking, "confirmed", "selecting", Mode::Forward).is_err());
        assert!(plan_path(Ladder::Booking, "confirmed", "booked", Mode::Forward).is_err());
        assert!(plan_path(Ladder::Anchor, "confirmed", "pending", Mode::Forward).is_err());
    }

    #[test]
    fn skipped_to_booked_forward() {
        assert_eq!(
            plan_path(Ladder::Booking, "skipped", "booked", Mode::Forward).unwrap(),
            hops(&[
                ("skipped", "pending"),
                ("pending", "populated"),
                ("populated", "booking"),
                ("booking", "booked"),
            ])
        );
    }

    #[test]
    fn cancelled_first_hop() {
        assert_eq!(
            plan_path(Ladder::Booking, "cancelled", "booked", Mode::Forward).unwrap(),
            hops(&[
                ("cancelled", "selecting"),
                ("selecting", "selected"),
                ("selected", "booking"),
                ("booking", "booked"),
            ])
        );
        assert_eq!(
            plan_path(Ladder::Booking, "cancelled", "researched", Mode::Forward).unwrap(),
            hops(&[
                ("cancelled", "pending"),
                ("pending", "researching"),
                ("researching", "researched"),
            ])
        );
        assert_eq!(
            plan_path(Ladder::Booking, "cancelled", "skipped", Mode::Any).unwrap(),
            hops(&[("cancelled", "skipped")])
        );
    }

    #[test]
    fn from_equals_to_is_empty() {
        for l in LADDERS {
            for s in states(l) {
                for mode in MODES {
                    assert!(plan_path(l, s, s, mode).unwrap().is_empty());
                }
            }
        }
    }

    #[test]
    fn unknown_or_off_ladder_statuses_err() {
        let e = plan_path(Ladder::Itinerary, "researched", "booked", Mode::Any).unwrap_err();
        assert_eq!(e, "status booked is not on the itinerary ladder");
        assert!(plan_path(Ladder::Anchor, "pending", "selecting", Mode::Any).is_err());
        assert!(plan_path(Ladder::Booking, "archived", "booked", Mode::Any)
            .unwrap_err()
            .contains("unknown current status"));
        assert!(plan_path(Ladder::Itinerary, "booked", "confirmed", Mode::Any)
            .unwrap_err()
            .contains("unknown current status"));
        assert!(plan_path(Ladder::Booking, "pending", "archived", Mode::Any).is_err());
    }

    #[test]
    fn forward_mode_refuses_backward_targets() {
        let e = plan_path(Ladder::Booking, "booked", "selecting", Mode::Forward).unwrap_err();
        assert!(e.contains("no legal status path from booked to selecting"), "{e}");
        assert!(plan_path(Ladder::Booking, "selected", "skipped", Mode::Forward).is_err());
        assert!(plan_path(Ladder::Booking, "selected", "skipped", Mode::Any).is_ok());
    }

    #[test]
    fn single_hop_rule() {
        assert!(single_hop(Ladder::Booking, None, "selected").unwrap().is_empty());
        assert!(single_hop(Ladder::Booking, Some("selected"), "selected").unwrap().is_empty());
        for from in ["researched", "selecting", "populated", "booking"] {
            assert_eq!(
                single_hop(Ladder::Booking, Some(from), "selected").unwrap(),
                vec![Hop { from: status_literal(from).unwrap(), to: "selected" }]
            );
        }
        assert!(single_hop(Ladder::Booking, Some("booked"), "selected").is_err());
        assert!(single_hop(Ladder::Booking, Some("pending"), "selected").is_err());
        for from in ["pending", "selected"] {
            assert!(single_hop(Ladder::Booking, Some(from), "populated").is_ok());
        }
        for from in ["booking", "booked", "confirmed"] {
            assert!(single_hop(Ladder::Booking, Some(from), "populated").is_err());
        }
    }

    #[test]
    fn raise_to_never_regresses() {
        assert_eq!(raise_to(Ladder::Booking, None, "researched").unwrap(), Some(vec![]));
        assert_eq!(
            raise_to(Ladder::Booking, Some("pending"), "researched").unwrap(),
            Some(hops(&[("pending", "researching"), ("researching", "researched")]))
        );
        assert_eq!(
            raise_to(Ladder::Booking, Some("researching"), "researched").unwrap(),
            Some(hops(&[("researching", "researched")]))
        );
        for s in ["researched", "selecting", "selected", "populated", "booking", "booked", "confirmed", "skipped", "cancelled"] {
            assert_eq!(raise_to(Ladder::Booking, Some(s), "researched").unwrap(), None, "{s}");
        }
    }

    // The four pre-2026-11-11 BFS unit tests (set-process-status), moved to the
    // shared planner.
    #[test]
    fn path_pending_to_booked_walks_shortest_chain() {
        assert_eq!(
            plan_path(Ladder::Booking, "pending", "booked", Mode::Any).unwrap(),
            hops(&[("pending", "populated"), ("populated", "booking"), ("booking", "booked")])
        );
    }

    #[test]
    fn path_idempotent_is_empty() {
        assert!(plan_path(Ladder::Booking, "booked", "booked", Mode::Any).unwrap().is_empty());
    }

    #[test]
    fn path_booking_to_confirmed() {
        assert_eq!(
            plan_path(Ladder::Booking, "booking", "confirmed", Mode::Any).unwrap(),
            hops(&[("booking", "booked"), ("booked", "confirmed")])
        );
    }

    #[test]
    fn path_unknown_current_fails() {
        assert!(plan_path(Ladder::Booking, "archived", "booked", Mode::Any).is_err());
    }

    #[test]
    fn format_path_joins() {
        let p = hops(&[("selected", "booking"), ("booking", "booked")]);
        assert_eq!(format_path("selected", &p, " → "), "selected → booking → booked");
        assert_eq!(format_path("booked", &[], " -> "), "booked");
    }
}
