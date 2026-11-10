//! `travel set-fit-offer <offer-id> [--remove] [--order N] [--dest <slug>]`,
//! `travel set-fit-offer <offer-id> --booked | --not-booked [--dest <slug>]` and
//! `travel set-fit-offer --list` — curate which offers the dashboard FIT
//! comparison lists (`plan_fit_offers`). The worker used to hardcode
//! agency/hotel filters in its SQL; now the list is data.
//!
//! `--booked` marks a LISTED offer as the FIT that was actually booked — the
//! data the dashboard needs when the plan was booked through ingest-known
//! (set-flight / set-hotel) with no plan_offer_selection row. At most one per
//! plan+destination: marking one unmarks any other in the same transaction.
//!
//! Audited writes: one timeline `plan_events` row (`fit_offer_added` /
//! `fit_offer_removed` / `fit_offer_booked` / `fit_offer_unbooked`), its KV,
//! `operation_runs`, and a `plans.version` bump, all in one transaction.
//! `--list` is read-only; re-marking an already-booked offer (or unmarking an
//! unmarked one) changes nothing and writes no audit row.

use crate::cascade::common::{
    insert_event, insert_kv_rows, next_timeline_sort_order, now_db_datetime, now_rfc3339,
    read_version, record_operation, resolve_active_destination,
};
use travel_db::repo::plan_fit_offers;

#[derive(Debug, Default, PartialEq)]
struct Parsed {
    offer_id: Option<String>,
    remove: bool,
    list: bool,
    booked: Option<bool>,
    order: Option<i64>,
    dest: Option<String>,
}

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let parsed = parse(args)?;
    if parsed.list {
        let conn = crate::db::connect_read().await?;
        let destination =
            resolve_active_destination(&conn, &plan_id, parsed.dest.as_deref()).await?;
        return print_list(&conn, &plan_id, &destination).await;
    }
    let offer_id = parsed.offer_id.clone().expect("parse guarantees an offer id");
    let conn = crate::db::connect_write().await?;
    let destination = resolve_active_destination(&conn, &plan_id, parsed.dest.as_deref()).await?;

    conn.execute("BEGIN", libsql::params![])
        .await
        .map_err(|e| format!("set-fit-offer BEGIN failed: {e}"))?;
    match write(&conn, &plan_id, &destination, &offer_id, &parsed).await {
        Ok(lines) => {
            conn.execute("COMMIT", libsql::params![])
                .await
                .map_err(|e| format!("set-fit-offer COMMIT failed: {e}"))?;
            for line in lines {
                println!("{line}");
            }
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", libsql::params![]).await;
            return Err(e);
        }
    }
    print_list(&conn, &plan_id, &destination).await
}

/// One mutation + its audit triad, inside the caller's transaction. Returns the
/// lines to print after COMMIT (nothing is printed before the write is durable).
async fn write(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
    offer_id: &str,
    parsed: &Parsed,
) -> Result<Vec<String>, String> {
    let now_db = now_db_datetime();
    let now_iso = now_rfc3339();
    let mut lines = Vec::new();

    let (event, summary, kv) = if let Some(mark) = parsed.booked {
        let current = plan_fit_offers::booked_flag(conn, plan_id, destination, offer_id)
            .await?
            .ok_or_else(|| {
                format!(
                    "set-fit-offer: {offer_id} is not listed for {plan_id} / {destination}; \
                     list it first with set-fit-offer {offer_id}"
                )
            })?;
        let others = if mark {
            plan_fit_offers::other_booked(conn, plan_id, destination, offer_id).await?
        } else {
            Vec::new()
        };
        if current == mark && others.is_empty() {
            lines.push(format!(
                "FIT offer {offer_id} is already {}; nothing changed",
                if mark { "marked booked" } else { "not booked" }
            ));
            return Ok(lines);
        }
        if mark {
            plan_fit_offers::clear_other_booked(conn, plan_id, destination, offer_id, &now_db)
                .await?;
            for other in &others {
                lines.push(format!("unmarked previously booked FIT offer: {other}"));
            }
        }
        plan_fit_offers::set_booked(conn, plan_id, destination, offer_id, mark, &now_db).await?;
        let mut kv = vec![
            ("destination", destination.to_string()),
            ("offer_id", offer_id.to_string()),
        ];
        if !others.is_empty() {
            kv.push(("unmarked_offer_id", others.join(",")));
        }
        lines.push(format!(
            "✅ FIT offer {}: {offer_id}",
            if mark { "marked booked" } else { "unmarked (not booked)" }
        ));
        (
            if mark { "fit_offer_booked" } else { "fit_offer_unbooked" },
            format!(
                "set-fit-offer {} {destination} offer={offer_id}{}",
                if mark { "booked" } else { "not-booked" },
                if others.is_empty() {
                    String::new()
                } else {
                    format!(" unmarked={}", others.join(","))
                }
            ),
            kv,
        )
    } else if parsed.remove {
        if plan_fit_offers::delete(conn, plan_id, destination, offer_id).await? == 0 {
            return Err(format!(
                "set-fit-offer: {offer_id} is not listed for {plan_id} / {destination}"
            ));
        }
        lines.push(format!("✅ FIT offer removed: {offer_id}"));
        (
            "fit_offer_removed",
            format!("set-fit-offer remove {destination} offer={offer_id}"),
            vec![
                ("destination", destination.to_string()),
                ("offer_id", offer_id.to_string()),
            ],
        )
    } else {
        match plan_fit_offers::offer_kind(conn, offer_id).await? {
            None => return Err(format!("set-fit-offer: unknown offer id {offer_id}")),
            Some((dest, kind)) if dest != destination || kind != "package" => {
                return Err(format!(
                    "set-fit-offer: {offer_id} is a {kind} offer for {dest}; \
                     the FIT list takes package offers for {destination}"
                ));
            }
            Some(_) => {}
        }
        let order = match parsed.order {
            Some(n) => n,
            None => plan_fit_offers::next_sort_order(conn, plan_id, destination).await?,
        };
        plan_fit_offers::upsert(conn, plan_id, destination, offer_id, order, &now_db).await?;
        lines.push(format!("✅ FIT offer listed: {offer_id}"));
        (
            "fit_offer_added",
            format!("set-fit-offer {destination} offer={offer_id} order={order}"),
            vec![
                ("destination", destination.to_string()),
                ("offer_id", offer_id.to_string()),
                ("sort_order", order.to_string()),
            ],
        )
    };

    let version_before = read_version(conn, plan_id).await?;
    let sort_order = next_timeline_sort_order(conn, plan_id).await?;
    insert_event(
        conn, plan_id, "timeline", "", "", sort_order, event, &now_iso, None, None,
    )
    .await?;
    insert_kv_rows(conn, plan_id, "timeline", "", "", sort_order, &kv).await?;
    record_operation(
        conn,
        plan_id,
        "set-fit-offer",
        &summary,
        version_before,
        version_before + 1,
        &now_db,
    )
    .await?;
    Ok(lines)
}

async fn print_list(
    conn: &libsql::Connection,
    plan_id: &str,
    destination: &str,
) -> Result<(), String> {
    let rows = plan_fit_offers::list(conn, plan_id, destination).await?;
    println!("FIT offers for {plan_id} / {destination}: {}", rows.len());
    for r in rows {
        let price = if r.price_per_person > 0 {
            r.price_per_person.to_string()
        } else {
            "-".to_string()
        };
        println!(
            "  {}  {}  {}  {}  {}{}",
            r.sort_order,
            if r.source_id.is_empty() { "(missing offer)" } else { &r.source_id },
            price,
            r.hotel_name,
            r.offer_id,
            if r.booked { "  [booked]" } else { "" }
        );
    }
    Ok(())
}

fn parse(args: &[String]) -> Result<Parsed, String> {
    let mut p = Parsed::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--remove" => {
                p.remove = true;
                i += 1;
            }
            "--list" => {
                p.list = true;
                i += 1;
            }
            "--booked" | "--not-booked" => {
                let mark = a == "--booked";
                if p.booked.is_some_and(|b| b != mark) {
                    return Err("set-fit-offer: --booked and --not-booked cannot be combined".into());
                }
                p.booked = Some(mark);
                i += 1;
            }
            "--order" | "--dest" => {
                let v = args
                    .get(i + 1)
                    .filter(|v| !v.starts_with("--"))
                    .ok_or(format!("{a} requires a value"))?;
                if a == "--order" {
                    p.order = Some(
                        v.parse::<i64>()
                            .map_err(|_| format!("--order must be an integer, got {v}"))?,
                    );
                } else {
                    p.dest = Some(v.clone());
                }
                i += 2;
            }
            f if crate::plan_resolver::is_resolver_flag(f) => i += 2,
            f if f.starts_with("--") => {
                return Err(format!("set-fit-offer: unknown argument '{f}'"))
            }
            id => {
                if p.offer_id.is_some() {
                    return Err(format!("set-fit-offer: unexpected extra argument '{id}'"));
                }
                p.offer_id = Some(id.to_string());
                i += 1;
            }
        }
    }
    if p.list {
        if p.offer_id.is_some() || p.remove || p.order.is_some() || p.booked.is_some() {
            return Err(
                "set-fit-offer: --list takes no offer id / --remove / --order / --booked / --not-booked"
                    .into(),
            );
        }
        return Ok(p);
    }
    if p.offer_id.is_none() {
        return Err("set-fit-offer: requires <offer-id> (or --list)".into());
    }
    if p.remove && p.order.is_some() {
        return Err("set-fit-offer: --remove cannot be combined with --order".into());
    }
    if p.booked.is_some() && (p.remove || p.order.is_some()) {
        return Err(
            "set-fit-offer: --booked / --not-booked mark an already-listed offer; \
             do not combine with --remove or --order"
                .into(),
        );
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_add_remove_list() {
        let p = parse(&s(&["offer_a", "--order", "2", "--dest", "kyoto"])).unwrap();
        assert_eq!(p.offer_id.as_deref(), Some("offer_a"));
        assert_eq!(p.order, Some(2));
        assert_eq!(p.dest.as_deref(), Some("kyoto"));
        assert!(parse(&s(&["offer_a", "--remove"])).unwrap().remove);
        assert!(parse(&s(&["--list"])).unwrap().list);
    }

    #[test]
    fn parse_rejects_bad_combinations() {
        assert!(parse(&[]).is_err());
        assert!(parse(&s(&["a", "b"])).is_err());
        assert!(parse(&s(&["a", "--order", "x"])).is_err());
        assert!(parse(&s(&["a", "--remove", "--order", "1"])).is_err());
        assert!(parse(&s(&["--list", "a"])).is_err());
        assert!(parse(&s(&["a", "--bogus"])).is_err());
        assert!(parse(&s(&["a", "--booked", "--not-booked"])).is_err());
        assert!(parse(&s(&["a", "--booked", "--remove"])).is_err());
        assert!(parse(&s(&["a", "--not-booked", "--order", "1"])).is_err());
        assert!(parse(&s(&["--list", "--booked"])).is_err());
        assert!(parse(&s(&["--booked"])).is_err());
    }

    #[test]
    fn parse_booked_flags() {
        let p = parse(&s(&["offer_a", "--booked", "--dest", "kyoto"])).unwrap();
        assert_eq!(p.booked, Some(true));
        assert_eq!(p.dest.as_deref(), Some("kyoto"));
        assert_eq!(parse(&s(&["offer_a", "--not-booked"])).unwrap().booked, Some(false));
        assert_eq!(parse(&s(&["offer_a"])).unwrap().booked, None);
    }
}
