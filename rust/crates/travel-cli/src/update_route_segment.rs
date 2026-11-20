//! `travel update-route-segment` / `travel delete-route-segment` — partial edit and
//! delete-with-renumber of ONE `day_route_segments` row, plus its stored via points
//! (spec 2026-11-20-cli-itinerary-editing-design.md §4 / §6).
//!
//! Every write is ONE transaction (`begin_immediate` → repo writes → `touch_day` →
//! `record::emit_record_event` → `finish_tx`). No network work happens in here: `--via`
//! takes literal `lat,lon` only; fetching the road geometry is `road-leg refetch --segment`.

use travel_db::repo::route_segments::{self, SegmentPatch, SegmentRow, Via};

pub(crate) const UPDATE_USAGE: &str = "travel update-route-segment <day> <sort_order> \
[--from <t>] [--to <t>] [--mode <m>] [--duration <min>] [--start-time HH:MM] \
[--notes <t> | --append-notes <t>] [--clear duration,start_time,notes] \
[--via \"<lat,lon;lat,lon>\" | --clear via] [--dest <slug>] [--plan-id <id>]\n  \
(partial edit of one route segment; keeps source/record fields. --append-notes joins with \"；\".\n  \
--via replaces the whole waypoint list offline; then run the printed `road-leg refetch --segment` command.)";

pub(crate) const DELETE_USAGE: &str =
    "travel delete-route-segment <day> <sort_order> [--dest <slug>] [--plan-id <id>]\n  \
(deletes one route segment and renumbers the later ones down by one; refuses a segment that carries a trip record)";

/// How `--via` / `--clear via` changes the stored waypoint list.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ViaChange {
    Keep,
    Set(Vec<Via>),
    Clear,
}

#[derive(Debug)]
pub(crate) struct UpdateParsed {
    pub day: i64,
    pub sort_order: i64,
    pub patch: SegmentPatch,
    pub via: ViaChange,
    pub dest: Option<String>,
}

fn arg_value(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i + 1)
        .cloned()
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn nonempty(flag: &str, v: String) -> Result<String, String> {
    let t = v.trim().to_string();
    if t.is_empty() {
        return Err(format!("{flag} cannot be empty (use --clear for nullable fields)"));
    }
    Ok(t)
}

/// Max waypoints per segment (a public OSRM URL carries them all).
pub(crate) const MAX_VIA: usize = 25;

/// Parse `--via "lat,lon;lat,lon"` — literal coordinates only, each through
/// `checks::validate_coords` (so null island is refused). Order = travel order.
pub(crate) fn parse_via_list(s: &str) -> Result<Vec<Via>, String> {
    let mut out = Vec::new();
    for (i, tok) in s.split(';').enumerate() {
        let tok = tok.trim();
        if tok.is_empty() {
            return Err(format!(
                "--via token #{} is empty (expected \"lat,lon;lat,lon\"; use --clear via to remove all)",
                i + 1
            ));
        }
        let (a, b) = tok.split_once(',').ok_or_else(|| {
            format!("--via token #{} \"{tok}\" is not lat,lon (place names are not accepted here)", i + 1)
        })?;
        let lat = a
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("--via token #{} \"{tok}\": lat is not a number", i + 1))?;
        let lon = b
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("--via token #{} \"{tok}\": lon is not a number", i + 1))?;
        out.push(
            crate::checks::validate_coords(lat, lon)
                .map_err(|e| format!("--via token #{} \"{tok}\": {e}", i + 1))?,
        );
    }
    if out.len() > MAX_VIA {
        return Err(format!("--via accepts at most {MAX_VIA} points (got {})", out.len()));
    }
    Ok(out)
}

pub(crate) fn parse_update(args: &[String]) -> Result<UpdateParsed, String> {
    let mut patch = SegmentPatch::default();
    let mut dest: Option<String> = None;
    let mut via_set: Option<Vec<Via>> = None;
    let mut clear: Vec<String> = Vec::new();
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--dest" => {
                dest = Some(arg_value(args, i, "--dest")?);
                i += 2;
            }
            "--from" => {
                patch.from_place = Some(nonempty("--from", arg_value(args, i, "--from")?)?);
                i += 2;
            }
            "--to" => {
                patch.to_place = Some(nonempty("--to", arg_value(args, i, "--to")?)?);
                i += 2;
            }
            "--mode" => {
                let v = arg_value(args, i, "--mode")?;
                patch.mode = Some(crate::checks::normalize_mode(&v)?.to_string());
                i += 2;
            }
            "--duration" => {
                let v = arg_value(args, i, "--duration")?;
                let n = v
                    .parse::<i64>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| "--duration must be a positive integer (minutes)".to_string())?;
                patch.duration_min = Some(n);
                i += 2;
            }
            "--start-time" => {
                let v = arg_value(args, i, "--start-time")?;
                crate::checks::validate_time_flag("--start-time", &v)?;
                patch.start_time = Some(v);
                i += 2;
            }
            "--notes" => {
                patch.notes = Some(nonempty("--notes", arg_value(args, i, "--notes")?)?);
                i += 2;
            }
            "--append-notes" => {
                patch.append_notes = Some(nonempty(
                    "--append-notes",
                    arg_value(args, i, "--append-notes")?,
                )?);
                i += 2;
            }
            "--via" => {
                via_set = Some(parse_via_list(&arg_value(args, i, "--via")?)?);
                i += 2;
            }
            "--clear" => {
                for f in arg_value(args, i, "--clear")?.split(',') {
                    clear.push(f.trim().to_string());
                }
                i += 2;
            }
            f if crate::plan_resolver::is_resolver_flag(f) => i += 2,
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}. Usage:\n{UPDATE_USAGE}"));
            }
            _ => {
                positional.push(a.clone());
                i += 1;
            }
        }
    }
    if positional.len() != 2 {
        return Err(format!("Usage:\n{UPDATE_USAGE}"));
    }
    let mut via_clear = false;
    let mut seen: Vec<&String> = Vec::new();
    for f in &clear {
        if seen.contains(&f) {
            continue;
        }
        seen.push(f);
        match f.as_str() {
            "duration" => {
                if patch.duration_min.is_some() {
                    return Err("--duration and --clear duration conflict".to_string());
                }
                patch.clear_duration = true;
            }
            "start_time" | "start" => {
                if patch.start_time.is_some() {
                    return Err("--start-time and --clear start_time conflict".to_string());
                }
                patch.clear_start_time = true;
            }
            "notes" => {
                if patch.notes.is_some() || patch.append_notes.is_some() {
                    return Err("--notes/--append-notes and --clear notes conflict".to_string());
                }
                patch.clear_notes = true;
            }
            "via" => {
                if via_set.is_some() {
                    return Err("--via and --clear via conflict".to_string());
                }
                via_clear = true;
            }
            other => {
                return Err(format!(
                    "--clear accepts only duration,start_time,notes,via (got \"{other}\"; \
                     from/to/mode cannot be NULL)"
                ));
            }
        }
    }
    if patch.notes.is_some() && patch.append_notes.is_some() {
        return Err("--notes and --append-notes conflict".to_string());
    }
    let via = match (via_set, via_clear) {
        (Some(v), _) => ViaChange::Set(v),
        (None, true) => ViaChange::Clear,
        (None, false) => ViaChange::Keep,
    };
    if patch == SegmentPatch::default() && via == ViaChange::Keep {
        return Err(format!("pass at least one field flag or --clear. Usage:\n{UPDATE_USAGE}"));
    }
    let day = positional[0]
        .parse::<i64>()
        .ok()
        .filter(|d| *d >= 1)
        .ok_or_else(|| format!("<day> must be a positive integer. Usage:\n{UPDATE_USAGE}"))?;
    let sort_order = positional[1]
        .parse::<i64>()
        .ok()
        .filter(|d| *d >= 0)
        .ok_or_else(|| format!("<sort_order> must be a non-negative integer. Usage:\n{UPDATE_USAGE}"))?;
    Ok(UpdateParsed { day, sort_order, patch, via, dest })
}

pub(crate) fn parse_delete(args: &[String]) -> Result<(i64, i64, Option<String>), String> {
    let mut dest = None;
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dest" => {
                dest = Some(arg_value(args, i, "--dest")?);
                i += 2;
            }
            f if crate::plan_resolver::is_resolver_flag(f) => i += 2,
            other if other.starts_with("--") => {
                return Err(format!("unknown argument: {other}. Usage:\n{DELETE_USAGE}"));
            }
            _ => {
                positional.push(args[i].clone());
                i += 1;
            }
        }
    }
    if positional.len() != 2 {
        return Err(format!("Usage:\n{DELETE_USAGE}"));
    }
    let day = positional[0]
        .parse::<i64>()
        .ok()
        .filter(|d| *d >= 1)
        .ok_or_else(|| format!("<day> must be a positive integer. Usage:\n{DELETE_USAGE}"))?;
    let sort_order = positional[1]
        .parse::<i64>()
        .ok()
        .filter(|d| *d >= 0)
        .ok_or_else(|| format!("<sort_order> must be a non-negative integer. Usage:\n{DELETE_USAGE}"))?;
    Ok((day, sort_order, dest))
}

fn shown(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "(none)".to_string())
}

fn via_text(v: &[Via]) -> String {
    if v.is_empty() {
        return "(none)".to_string();
    }
    v.iter()
        .map(|(a, b)| format!("{a:.6},{b:.6}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// The exact next-step command printed after a via change (spec Q4).
pub(crate) fn refetch_hint(day: i64, sort_order: i64, plan_id: &str, dest: &str) -> String {
    format!(
        "travel road-leg refetch --segment {day} {sort_order} --plan-id {plan_id} --dest {dest}"
    )
}

/// A route segment's neighbour as seen by the join hint (itinerary order, see
/// `route_segments::neighbour_segment`).
pub(crate) type Neighbour = route_segments::NeighbourSegment;

/// Hints (never writes) for a `--from`/`--to` change that BREAKS a join: the previous
/// segment used to end where this one started (`prev.to == old_from`) and no longer does
/// (`prev.to != new_from`); the next segment used to start where this one ended and no
/// longer does. A pair that was never joined (flight gap, overnight hop) stays silent.
/// Each hint names the exact `update-route-segment` command (with --plan-id/--dest).
pub(crate) fn join_hints(
    old: (&str, &str),
    new: (&str, &str),
    prev: Option<&Neighbour>,
    next: Option<&Neighbour>,
    plan_id: &str,
    dest: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(p) = prev {
        if p.to_place == old.0 && p.to_place != new.0 {
            out.push(format!(
                "hint: D{d} S{s} ends at \"{to}\" but this segment now starts at \"{nf}\" — \
                 join with: travel update-route-segment {d} {s} --to \"{nf}\" --plan-id {plan_id} --dest {dest}",
                d = p.day, s = p.sort_order, to = p.to_place, nf = new.0
            ));
        }
    }
    if let Some(n) = next {
        if n.from_place == old.1 && n.from_place != new.1 {
            out.push(format!(
                "hint: D{d} S{s} starts at \"{from}\" but this segment now ends at \"{nt}\" — \
                 join with: travel update-route-segment {d} {s} --from \"{nt}\" --plan-id {plan_id} --dest {dest}",
                d = n.day, s = n.sort_order, from = n.from_place, nt = new.1
            ));
        }
    }
    out
}

/// What `run_update` decided inside the transaction.
struct UpdateOutcome {
    diff: Vec<(&'static str, Option<String>, Option<String>)>,
    via_before: Vec<Via>,
    via_after: Vec<Via>,
    via_cleared_by_endpoints: bool,
    warnings: Vec<String>,
}

pub async fn run_update(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse_update(args)?;
    let conn = crate::db::connect_write().await?;
    let destination = crate::cascade::common::resolve_active_destination(
        &conn,
        &plan_id,
        parsed.dest.as_deref(),
    )
    .await?;
    let (day, so) = (parsed.day, parsed.sort_order);

    let outcome = async {
        crate::cascade::common::begin_immediate(&conn, "update-route-segment").await?;
        let before = match route_segments::read_segment(&conn, &plan_id, &destination, day, so).await? {
            Some(r) => r,
            None => {
                let have = route_segments::list_sort_orders(&conn, &plan_id, &destination, day).await?;
                return Err(format!(
                    "no route segment at day {day} sort_order {so}; existing sort_orders: {}",
                    if have.is_empty() {
                        "(none)".to_string()
                    } else {
                        have.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")
                    }
                ));
            }
        };
        let after: SegmentRow = parsed.patch.apply(&before)?;
        let diff = route_segments::segment_diff(&before, &after);
        let endpoints_changed = after.from_place != before.from_place || after.to_place != before.to_place;
        let via_before = route_segments::list_via(&conn, &plan_id, &destination, day, so).await?;
        let (via_after, via_cleared_by_endpoints) = match &parsed.via {
            ViaChange::Set(v) => (v.clone(), false),
            ViaChange::Clear => (Vec::new(), false),
            ViaChange::Keep if endpoints_changed && !via_before.is_empty() => (Vec::new(), true),
            ViaChange::Keep => (via_before.clone(), false),
        };
        let via_changed = via_after != via_before;
        if diff.is_empty() && !via_changed {
            return Ok(None);
        }
        // A recorded leg already points at THIS road; a different pair would orphan it.
        if endpoints_changed && before.record_status.is_some() {
            return Err(format!(
                "day {day} segment {so} carries a trip record ({}) — --from/--to cannot be changed; \
                 duration/start-time/notes/mode edits are allowed",
                before.record_status.clone().unwrap_or_default()
            ));
        }
        // Same guard bundle as set-route-segment, on the MERGED values.
        if endpoints_changed || after.mode != before.mode {
            if let Err(reason) = crate::checks::guard_segment(&after.from_place, &after.to_place, &after.mode) {
                return Err(format!("route segment rejected — {reason}"));
            }
        }
        let version_before = crate::cascade::common::read_version(&conn, &plan_id).await?;
        if !diff.is_empty() {
            route_segments::update_segment_fields(&conn, &plan_id, &destination, day, &after).await?;
        }
        if via_changed {
            route_segments::replace_via(&conn, &plan_id, &destination, day, so, &via_after).await?;
        }
        route_segments::touch_day(
            &conn,
            &plan_id,
            &destination,
            day,
            &crate::cascade::common::now_db_datetime(),
        )
        .await?;

        let mut warnings = Vec::new();
        if before.source == "ai_recommended" {
            warnings.push(format!(
                "note: D{day} S{so} is an ai_recommended segment — a later `derive-routes` run may rewrite the day \
                 and drop this edit (and its via); freeze the day first with \
                 `travel confirm-recommendations --day {day} --kind route`."
            ));
        }
        if !via_after.is_empty() {
            for (d2, s2) in route_segments::segments_with_endpoints(
                &conn,
                &plan_id,
                &destination,
                &after.from_place,
                &after.to_place,
            )
            .await?
            {
                if (d2, s2) == (day, so) {
                    continue;
                }
                let other = route_segments::list_via(&conn, &plan_id, &destination, d2, s2).await?;
                if other != via_after {
                    warnings.push(format!(
                        "warn: D{d2} S{s2} has the same endpoints {}→{} but a different via — the road-leg cache is keyed by endpoints only, so the later refetch wins",
                        after.from_place, after.to_place
                    ));
                }
            }
        }

        // Join hints (read-only): neighbours in itinerary order, only for a from/to change.
        if after.from_place != before.from_place || after.to_place != before.to_place {
            let prev = route_segments::neighbour_segment(&conn, &plan_id, &destination, day, so, false).await?;
            let next = route_segments::neighbour_segment(&conn, &plan_id, &destination, day, so, true).await?;
            warnings.extend(join_hints(
                (&before.from_place, &before.to_place),
                (&after.from_place, &after.to_place),
                prev.as_ref(),
                next.as_ref(),
                &plan_id,
                &destination,
            ));
        }

        let changed: Vec<&str> = diff.iter().map(|(c, _, _)| *c).collect();
        let mut kv: Vec<(String, String)> = vec![
            ("day_number".into(), day.to_string()),
            ("sort_order".into(), so.to_string()),
        ];
        let mut changed_all: Vec<String> = changed.iter().map(|s| s.to_string()).collect();
        if via_changed {
            changed_all.push("via".to_string());
        }
        kv.push(("changed".into(), changed_all.join(",")));
        let undef = |v: &Option<String>| v.clone().unwrap_or_else(|| "undefined".to_string());
        for (col, from, to) in &diff {
            kv.push((format!("from.{col}"), undef(from)));
            kv.push((format!("to.{col}"), undef(to)));
        }
        if via_changed {
            kv.push(("via_count".into(), via_after.len().to_string()));
        }
        if parsed.patch.append_notes.is_some() {
            kv.push(("notes_mode".into(), "append".into()));
        }
        let kv_ref: Vec<(&str, String)> = kv.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        crate::record::emit_record_event(
            &conn,
            &plan_id,
            &destination,
            "route_segment_patched",
            &kv_ref,
            version_before,
            "update-route-segment",
            &format!("D{day} S{so} patched: {}", changed_all.join(",")),
        )
        .await?;
        Ok(Some(UpdateOutcome {
            diff,
            via_before,
            via_after,
            via_cleared_by_endpoints,
            warnings,
        }))
    }
    .await;
    let result = crate::cascade::common::finish_tx(&conn, "update-route-segment", outcome).await?;
    match result {
        None => println!("ℹ️ No change (D{day} S{so})"),
        Some(o) => {
            println!("\n📝 Updating route segment: D{day} S{so}");
            for (col, from, to) in &o.diff {
                println!("   {col}: {} → {}", shown(from), shown(to));
            }
            if o.via_after != o.via_before {
                println!("   via: {} → {}", via_text(&o.via_before), via_text(&o.via_after));
            }
            if o.via_cleared_by_endpoints {
                println!(
                    "   via cleared: D{day} S{so} 端點已變（{} 點）— 需要的話用 --via 重新設定",
                    o.via_before.len()
                );
            }
            println!("✅ Route segment updated (source and record fields untouched)");
            for w in &o.warnings {
                println!("{w}");
            }
            if o.via_after != o.via_before {
                println!(
                    "next: {}  then  travel snapshot-maps --plan-id {plan_id}",
                    refetch_hint(day, so, &plan_id, &destination)
                );
            } else {
                println!("note: run `travel snapshot-maps` to refresh the maps");
            }
        }
    }
    Ok(())
}

pub async fn run_delete(args: &[String], plan_id: String) -> Result<(), String> {
    let (day, so, dest) = parse_delete(args)?;
    let conn = crate::db::connect_write().await?;
    let destination =
        crate::cascade::common::resolve_active_destination(&conn, &plan_id, dest.as_deref()).await?;

    let outcome = async {
        crate::cascade::common::begin_immediate(&conn, "delete-route-segment").await?;
        let version_before = crate::cascade::common::read_version(&conn, &plan_id).await?;
        let (row, via_count, renumbered) =
            route_segments::delete_segment_renumber(&conn, &plan_id, &destination, day, so).await?;
        route_segments::touch_day(
            &conn,
            &plan_id,
            &destination,
            day,
            &crate::cascade::common::now_db_datetime(),
        )
        .await?;
        let undef = |v: &Option<String>| v.clone().unwrap_or_else(|| "undefined".to_string());
        let kv: Vec<(&str, String)> = vec![
            ("day_number", day.to_string()),
            ("sort_order", so.to_string()),
            ("from_place", row.from_place.clone()),
            ("to_place", row.to_place.clone()),
            ("mode", row.mode.clone()),
            ("duration_min", undef(&row.duration_min.map(|n| n.to_string()))),
            ("start_time", undef(&row.start_time)),
            ("notes", undef(&row.notes)),
            ("source", row.source.clone()),
            ("via_count", via_count.to_string()),
            ("renumbered_count", renumbered.to_string()),
        ];
        crate::record::emit_record_event(
            &conn,
            &plan_id,
            &destination,
            "route_segment_deleted",
            &kv,
            version_before,
            "delete-route-segment",
            &format!("D{day} S{so} {}→{} deleted", row.from_place, row.to_place),
        )
        .await?;
        Ok((row, via_count, renumbered))
    }
    .await;
    let (row, via_count, renumbered) =
        crate::cascade::common::finish_tx(&conn, "delete-route-segment", outcome).await?;
    println!(
        "🗑️  Deleted route segment D{day} S{so}: {} → {} ({}); {renumbered} later segment(s) renumbered{}",
        row.from_place,
        row.to_place,
        row.mode,
        if via_count > 0 { format!(", {via_count} via point(s) removed") } else { String::new() }
    );
    println!("✅ Route segment deleted");
    if row.source == "ai_recommended" {
        println!(
            "note: this was an ai_recommended segment — a later `derive-routes` run may regenerate it \
             (confirm the day's other segments to freeze them)."
        );
    }
    println!("note: run `travel snapshot-maps --plan-id {plan_id}` to refresh the maps (the road-leg cache is kept).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(a: &[&str]) -> Vec<String> {
        a.iter().map(|x| x.to_string()).collect()
    }
    fn upd(extra: &[&str]) -> Result<UpdateParsed, String> {
        let mut v = vec!["2", "1"];
        v.extend_from_slice(extra);
        parse_update(&s(&v))
    }

    #[test]
    fn parse_only_given_fields() {
        let u = upd(&["--duration", "30", "--plan-id", "p"]).unwrap();
        assert_eq!((u.day, u.sort_order), (2, 1));
        assert_eq!(u.patch.duration_min, Some(30));
        assert_eq!(u.patch.from_place, None);
        assert_eq!(u.via, ViaChange::Keep);
    }

    #[test]
    fn parse_clear_whitelist() {
        let u = upd(&["--clear", "duration,start_time,notes"]).unwrap();
        assert!(u.patch.clear_duration && u.patch.clear_start_time && u.patch.clear_notes);
        for bad in ["from", "to", "mode", "nonsense"] {
            let e = upd(&["--clear", bad]).unwrap_err();
            assert!(e.contains("--clear accepts only"), "{bad}: {e}");
        }
        assert_eq!(upd(&["--clear", "via"]).unwrap().via, ViaChange::Clear);
    }

    #[test]
    fn parse_conflicts_and_requirements() {
        assert!(upd(&[]).unwrap_err().contains("at least one"));
        assert!(upd(&["--duration", "5", "--clear", "duration"]).unwrap_err().contains("conflict"));
        assert!(upd(&["--notes", "a", "--append-notes", "b"]).unwrap_err().contains("conflict"));
        assert!(upd(&["--append-notes", "b", "--clear", "notes"]).unwrap_err().contains("conflict"));
        assert!(upd(&["--via", "25.1,121.5", "--clear", "via"]).unwrap_err().contains("conflict"));
        assert!(upd(&["--duration", "0"]).unwrap_err().contains("positive"));
        assert!(upd(&["--start-time", "9am"]).unwrap_err().contains("HH:MM"));
        assert!(upd(&["--mode", "teleport"]).is_err());
        assert!(upd(&["--bogus", "x"]).unwrap_err().contains("unknown argument"));
        assert!(parse_update(&s(&["1", "--duration", "5"])).is_err());
        assert!(parse_update(&s(&["0", "1", "--duration", "5"])).is_err());
        assert!(parse_update(&s(&["1", "-1", "--duration", "5"])).is_err());
    }

    #[test]
    fn via_parse_accepts_ordered_pairs() {
        let v = parse_via_list("25.289700,121.569000; 25.231700,121.501800").unwrap();
        assert_eq!(v, vec![(25.2897, 121.569), (25.2317, 121.5018)]);
        assert_eq!(upd(&["--via", "25.1,121.5"]).unwrap().via, ViaChange::Set(vec![(25.1, 121.5)]));
    }

    #[test]
    fn via_parse_rejects_bad_tokens() {
        assert!(parse_via_list("").unwrap_err().contains("empty"));
        assert!(parse_via_list("25.1,121.5;").unwrap_err().contains("empty"));
        assert!(parse_via_list("三芝").unwrap_err().contains("not lat,lon"));
        assert!(parse_via_list("a,b").unwrap_err().contains("not a number"));
        assert!(parse_via_list("0,0").unwrap_err().contains("null island"));
        assert!(parse_via_list("91,121").unwrap_err().contains("lat"));
        assert!(parse_via_list("25,181").unwrap_err().contains("lon"));
        let many = vec!["25.1,121.5"; MAX_VIA + 1].join(";");
        assert!(parse_via_list(&many).unwrap_err().contains("at most"));
    }

    fn nb(d: i64, s: i64, from: &str, to: &str) -> Neighbour {
        Neighbour { day: d, sort_order: s, from_place: from.into(), to_place: to.into() }
    }
    const P: &str = "plan-x";
    const D: &str = "dest_x";

    #[test]
    fn join_hint_prev_names_the_fixing_command_with_plan_and_dest() {
        // prev ended at the old start 舊飯店; start changed to 新飯店 => broken join.
        let prev = nb(2, 0, "X", "舊飯店");
        let h = join_hints(("舊飯店", "B"), ("新飯店", "B"), Some(&prev), None, P, D);
        assert_eq!(h.len(), 1, "{h:?}");
        assert!(h[0].contains("D2 S0 ends at \"舊飯店\""), "{h:?}");
        assert!(
            h[0].contains("travel update-route-segment 2 0 --to \"新飯店\" --plan-id plan-x --dest dest_x"),
            "{h:?}"
        );
    }

    #[test]
    fn join_hint_next_day_first_segment() {
        let next = nb(3, 1, "頭城（住宿待定）", "C");
        let h = join_hints(("A", "頭城（住宿待定）"), ("A", "旅館"), None, Some(&next), P, D);
        assert_eq!(h.len(), 1, "{h:?}");
        assert!(h[0].contains("D3 S1 starts at \"頭城（住宿待定）\""), "{h:?}");
        assert!(
            h[0].contains("travel update-route-segment 3 1 --from \"旅館\" --plan-id plan-x --dest dest_x"),
            "{h:?}"
        );
    }

    #[test]
    fn join_hint_flight_gap_never_joined_is_silent() {
        // KIX第一航廈→KIX第二航廈, next S3 starts at 桃園 (a flight gap): never joined.
        let next = nb(5, 3, "桃園國際機場第一航廈", "家");
        let h = join_hints(("A", "KIX第一航廈"), ("A", "KIX第二航廈"), None, Some(&next), P, D);
        assert!(h.is_empty(), "{h:?}");
        let prev = nb(5, 1, "X", "別處");
        assert!(join_hints(("舊", "B"), ("新", "B"), Some(&prev), None, P, D).is_empty());
    }

    #[test]
    fn join_hint_silent_when_still_joined_or_duration_only_edit() {
        // endpoints unchanged (--duration edit): old == new, so neighbours never "break".
        let prev = nb(1, 0, "x", "A");
        let next = nb(1, 2, "B", "y");
        assert!(join_hints(("A", "B"), ("A", "B"), Some(&prev), Some(&next), P, D).is_empty());
        // start changed away from what prev ended at: that IS a break.
        assert_eq!(join_hints(("Z", "B"), ("A", "B"), Some(&nb(1, 0, "x", "Z")), None, P, D).len(), 1);
        let joined = nb(1, 0, "x", "Z");
        assert!(join_hints(("Z", "B"), ("Z", "B"), Some(&joined), None, P, D).is_empty());
    }

    #[test]
    fn join_hint_both_sides_two_lines() {
        let prev = nb(1, 0, "x", "A");
        let next = nb(1, 2, "B", "y");
        let h = join_hints(("A", "B"), ("A2", "B2"), Some(&prev), Some(&next), P, D);
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn delete_parse() {
        assert_eq!(parse_delete(&s(&["3", "1", "--dest", "x"])).unwrap(), (3, 1, Some("x".into())));
        assert!(parse_delete(&s(&["3"])).is_err());
        assert!(parse_delete(&s(&["3", "1", "--bogus"])).is_err());
    }

    #[test]
    fn refetch_hint_is_the_exact_command() {
        assert_eq!(
            refetch_hint(3, 2, "ne-coast-2026", "ne_coast_2026"),
            "travel road-leg refetch --segment 3 2 --plan-id ne-coast-2026 --dest ne_coast_2026"
        );
    }
}
