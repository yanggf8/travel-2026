// `travel query-destination-ref` — show a destination reference (areas, POIs,
// clusters, transit, tips) from the normalized Turso tables. Read-only,
// plain-text. Ports loadDestinationReferenceFromTurso + the query-destination-ref
// renderer from src/services/turso-service.ts + src/cli/commands/query-destination-ref.ts.
//
// No JSON anywhere: list fields come from normalized child tables
// (destination_area_stations, destination_area_best_for, destination_poi_tags,
// destination_cluster_pois, destination_tips, destination_airports), one row per
// element, ordered by sort_order. Fail loud if the slug is unknown.

use crate::db;
use std::collections::BTreeMap;

pub struct DestRefArgs {
    pub slug: String,
}

impl DestRefArgs {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut slug: Option<String> = None;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--slug" => {
                    slug = Some(
                        args.get(i + 1)
                            .cloned()
                            .ok_or_else(|| "--slug requires a value".to_string())?,
                    );
                    i += 2;
                }
                "--json" => {
                    return Err("query-destination-ref does not support --json (plain text only)".to_string());
                }
                // First positional arg is treated as the slug (matches TS cleanArgs[0]).
                other if !other.starts_with("--") && slug.is_none() => {
                    slug = Some(other.to_string());
                    i += 1;
                }
                other => return Err(format!("unknown flag for query-destination-ref: {other}")),
            }
        }
        let slug = slug.ok_or_else(|| {
            "query-destination-ref requires --slug <destination_slug> (e.g. tokyo_2026).".to_string()
        })?;
        Ok(DestRefArgs { slug })
    }
}

fn sql_quote(v: &str) -> String {
    v.replace('\'', "''")
}

struct Area {
    name: String,
    typ: String,
    vibe: String,
    stations: Vec<String>,
    best_for: Vec<String>,
}

struct Poi {
    title: String,
    area: String,
    nearest_station: String,
    duration_min: Option<i64>,
    booking_required: bool,
    booking_url: Option<String>,
    cost_estimate: Option<i64>,
    notes: Option<String>,
    hours: Option<String>,
}

struct Cluster {
    id: String,
    name: String,
    description: String,
    duration_min: Option<i64>,
    best_area: Option<String>,
    pois: Vec<String>,
}

struct Transit {
    pair_key: String,
    kind: String,
    minutes: i64,
    line: String,
    station_from: Option<String>,
    station_to: Option<String>,
}

pub async fn run(opts: &DestRefArgs) -> Result<(), String> {
    let conn = db::connect_read().await?;
    let esc = sql_quote(&opts.slug);

    // Config scalars (fail loud if missing).
    let mut cfg_rows = conn
        .query(
            &format!("SELECT display_name, timezone, currency FROM destination_config WHERE slug = '{esc}'"),
            (),
        )
        .await
        .map_err(|e| format!("failed to query destination_config: {e}"))?;
    let cfg = cfg_rows
        .next()
        .await
        .map_err(|e| format!("failed to read destination_config: {e}"))?
        .ok_or_else(|| format!("destination_config slug={} not found", opts.slug))?;
    let display_name: String = cfg.get(0).unwrap_or_default();
    let timezone: String = cfg.get(1).unwrap_or_default();
    let currency: String = cfg.get(2).unwrap_or_default();

    // List child rows → grouped maps (BTreeMap keeps deterministic key order;
    // within a key, rows arrive in sort_order from the ORDER BY).
    let stations_by_area = group_pairs(&conn, &format!(
        "SELECT area_id, station FROM destination_area_stations WHERE slug = '{esc}' ORDER BY area_id, sort_order")).await?;
    let best_for_by_area = group_pairs(&conn, &format!(
        "SELECT area_id, tag FROM destination_area_best_for WHERE slug = '{esc}' ORDER BY area_id, sort_order")).await?;
    // POI tags exist in destination_poi_tags but the TS renderer does not print
    // them, so they are intentionally not fetched here (output parity).
    let pois_by_cluster = group_pairs(&conn, &format!(
        "SELECT cluster_id, poi_id FROM destination_cluster_pois WHERE slug = '{esc}' ORDER BY cluster_id, sort_order")).await?;
    let tips = flat_list(&conn, &format!(
        "SELECT tip FROM destination_tips WHERE slug = '{esc}' ORDER BY sort_order")).await?;
    let airports = flat_list(&conn, &format!(
        "SELECT airport FROM destination_airports WHERE slug = '{esc}' ORDER BY sort_order")).await?;

    // Areas.
    let mut areas: Vec<Area> = Vec::new();
    let mut rows = conn
        .query(&format!("SELECT area_id, name, type, vibe FROM destination_areas WHERE slug = '{esc}' ORDER BY area_id"), ())
        .await
        .map_err(|e| format!("failed to query destination_areas: {e}"))?;
    while let Some(r) = rows.next().await.map_err(|e| format!("area row: {e}"))? {
        let id: String = r.get(0).unwrap_or_default();
        areas.push(Area {
            stations: stations_by_area.get(&id).cloned().unwrap_or_default(),
            best_for: best_for_by_area.get(&id).cloned().unwrap_or_default(),
            name: r.get(1).unwrap_or_default(),
            typ: r.get(2).unwrap_or_default(),
            vibe: r.get(3).unwrap_or_default(),
        });
    }

    // POIs.
    let mut pois: Vec<Poi> = Vec::new();
    let mut rows = conn
        .query(&format!(
            "SELECT poi_id, title, area, nearest_station, duration_min, booking_required, booking_url, cost_estimate, notes, hours \
             FROM destination_pois WHERE slug = '{esc}' ORDER BY poi_id"), ())
        .await
        .map_err(|e| format!("failed to query destination_pois: {e}"))?;
    while let Some(r) = rows.next().await.map_err(|e| format!("poi row: {e}"))? {
        pois.push(Poi {
            title: r.get(1).unwrap_or_default(),
            area: r.get(2).unwrap_or_default(),
            nearest_station: r.get(3).unwrap_or_default(),
            duration_min: r.get(4).ok(),
            booking_required: r.get::<i64>(5).unwrap_or(0) == 1,
            booking_url: opt_string(&r, 6),
            cost_estimate: r.get(7).ok(),
            notes: opt_string(&r, 8),
            hours: opt_string(&r, 9),
        });
    }

    // Clusters.
    let mut clusters: Vec<Cluster> = Vec::new();
    let mut rows = conn
        .query(&format!("SELECT cluster_id, name, description, duration_min, best_area FROM destination_clusters WHERE slug = '{esc}' ORDER BY cluster_id"), ())
        .await
        .map_err(|e| format!("failed to query destination_clusters: {e}"))?;
    while let Some(r) = rows.next().await.map_err(|e| format!("cluster row: {e}"))? {
        let cluster_id: String = r.get(0).unwrap_or_default();
        clusters.push(Cluster {
            pois: pois_by_cluster.get(&cluster_id).cloned().unwrap_or_default(),
            id: cluster_id.clone(),
            name: r.get(1).unwrap_or_default(),
            description: r.get(2).unwrap_or_default(),
            duration_min: r.get(3).ok(),
            best_area: opt_string(&r, 4),
        });
    }

    // Transit (split into estimates vs inter_city, matching TS).
    let mut transit: Vec<Transit> = Vec::new();
    let mut rows = conn
        .query(&format!("SELECT pair_key, kind, minutes, line, station_from, station_to FROM destination_transit WHERE slug = '{esc}' ORDER BY kind, pair_key"), ())
        .await
        .map_err(|e| format!("failed to query destination_transit: {e}"))?;
    while let Some(r) = rows.next().await.map_err(|e| format!("transit row: {e}"))? {
        transit.push(Transit {
            pair_key: r.get(0).unwrap_or_default(),
            kind: r.get(1).unwrap_or_default(),
            minutes: r.get(2).unwrap_or(0),
            line: r.get(3).unwrap_or_default(),
            station_from: opt_string(&r, 4),
            station_to: opt_string(&r, 5),
        });
    }

    print_ref(&opts.slug, &display_name, &timezone, &currency, &airports, &areas, &pois, &clusters, &transit, &tips);
    Ok(())
}

async fn group_pairs(conn: &libsql::Connection, sql: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut rows = conn.query(sql, ()).await.map_err(|e| format!("query: {e}"))?;
    let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("row: {e}"))? {
        let k: String = r.get(0).unwrap_or_default();
        let v: String = r.get(1).unwrap_or_default();
        m.entry(k).or_default().push(v);
    }
    Ok(m)
}

async fn flat_list(conn: &libsql::Connection, sql: &str) -> Result<Vec<String>, String> {
    let mut rows = conn.query(sql, ()).await.map_err(|e| format!("query: {e}"))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await.map_err(|e| format!("row: {e}"))? {
        out.push(r.get::<String>(0).unwrap_or_default());
    }
    Ok(out)
}

fn opt_string(row: &libsql::Row, idx: i32) -> Option<String> {
    match row.get_value(idx).ok()? {
        libsql::Value::Text(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn print_ref(
    slug: &str,
    display_name: &str,
    timezone: &str,
    currency: &str,
    airports: &[String],
    areas: &[Area],
    pois: &[Poi],
    clusters: &[Cluster],
    transit: &[Transit],
    tips: &[String],
) {
    let mut lines: Vec<String> = Vec::new();
    let dn = if display_name.is_empty() { slug } else { display_name };
    lines.push(format!("# {dn} ({slug})"));
    let tz = if timezone.is_empty() { "?" } else { timezone };
    let cur = if currency.is_empty() { "?" } else { currency };
    lines.push(format!("{tz} · {cur} · airports: {}", airports.join(", ")));
    lines.push(String::new());

    lines.push(format!("## Areas ({})", areas.len()));
    for a in areas {
        lines.push(format!("- {} [{}] — {}", a.name, a.typ, a.vibe));
        lines.push(format!(
            "    stations: {} | best_for: {}",
            a.stations.join(", "),
            a.best_for.join(", ")
        ));
    }
    lines.push(String::new());

    lines.push(format!("## POIs ({})", pois.len()));
    for p in pois {
        let book = if p.booking_required {
            match &p.booking_url {
                Some(u) => format!(" [booking required: {u}]"),
                None => " [booking required]".to_string(),
            }
        } else {
            String::new()
        };
        let cost = match p.cost_estimate {
            Some(c) if c != 0 => format!(" ¥{c}"),
            _ => " free".to_string(),
        };
        let dur = p.duration_min.map(|d| d.to_string()).unwrap_or_default();
        lines.push(format!(
            "- {} ({}, {}) ~{}min{}{}",
            p.title, p.area, p.nearest_station, dur, cost, book
        ));
        if let Some(n) = &p.notes {
            lines.push(format!("    {n}"));
        }
        if let Some(h) = &p.hours {
            lines.push(format!("    hours: {h}"));
        }
    }
    lines.push(String::new());

    lines.push(format!("## Clusters ({})", clusters.len()));
    for c in clusters {
        let dur = c.duration_min.map(|d| d.to_string()).unwrap_or_default();
        let best = c.best_area.clone().unwrap_or_default();
        lines.push(format!(
            "- {} [{}] (~{}min, best in {}): {}",
            c.name, c.id, dur, best, c.description
        ));
        lines.push(format!("    POIs: {}", c.pois.join(", ")));
    }
    lines.push(String::new());

    lines.push("## Transit".to_string());
    let inter_city: Vec<&Transit> = transit.iter().filter(|t| t.kind == "inter_city").collect();
    for t in transit.iter().filter(|t| t.kind != "inter_city") {
        lines.push(format!("- {}: {}min via {}", t.pair_key, t.minutes, t.line));
    }
    if !inter_city.is_empty() {
        lines.push("### Inter-city".to_string());
        for t in inter_city {
            let stations = match (&t.station_from, &t.station_to) {
                (Some(f), Some(to)) => format!(" ({f} → {to})"),
                _ => String::new(),
            };
            lines.push(format!("- {}: {}min via {}{}", t.pair_key, t.minutes, t.line, stations));
        }
    }
    lines.push(String::new());

    if !tips.is_empty() {
        lines.push("## Tips".to_string());
        for t in tips {
            lines.push(format!("- {t}"));
        }
    }

    println!("{}", lines.join("\n"));
}
