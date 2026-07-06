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
    use travel_db::repo::destination_ref as repo;

    let conn = db::connect_read().await?;
    let slug = opts.slug.as_str();

    // Config scalars (fail loud if missing).
    let (display_name, timezone, currency) = repo::config_scalars(&conn, slug)
        .await?
        .ok_or_else(|| format!("destination_config slug={slug} not found"))?;

    // List child rows → grouped maps (BTreeMap keeps deterministic key order;
    // within a key, rows arrive in sort_order from the ORDER BY).
    let stations_by_area = repo::stations_by_area(&conn, slug).await?;
    let best_for_by_area = repo::best_for_by_area(&conn, slug).await?;
    // POI tags exist in destination_poi_tags but the TS renderer does not print
    // them, so they are intentionally not fetched here (output parity).
    let pois_by_cluster = repo::pois_by_cluster(&conn, slug).await?;
    let tips = repo::tips(&conn, slug).await?;
    let airports = repo::airports(&conn, slug).await?;

    let areas: Vec<Area> = repo::areas(&conn, slug)
        .await?
        .into_iter()
        .map(|a| Area {
            stations: stations_by_area.get(&a.area_id).cloned().unwrap_or_default(),
            best_for: best_for_by_area.get(&a.area_id).cloned().unwrap_or_default(),
            name: a.name,
            typ: a.typ,
            vibe: a.vibe,
        })
        .collect();

    let pois: Vec<Poi> = repo::pois(&conn, slug)
        .await?
        .into_iter()
        .map(|p| Poi {
            title: p.title,
            area: p.area,
            nearest_station: p.nearest_station,
            duration_min: p.duration_min,
            booking_required: p.booking_required,
            booking_url: p.booking_url,
            cost_estimate: p.cost_estimate,
            notes: p.notes,
            hours: p.hours,
        })
        .collect();

    let clusters: Vec<Cluster> = repo::clusters(&conn, slug)
        .await?
        .into_iter()
        .map(|c| Cluster {
            pois: pois_by_cluster.get(&c.cluster_id).cloned().unwrap_or_default(),
            id: c.cluster_id,
            name: c.name,
            description: c.description,
            duration_min: c.duration_min,
            best_area: c.best_area,
        })
        .collect();

    // Transit (split into estimates vs inter_city, matching TS).
    let transit: Vec<Transit> = repo::transit(&conn, slug)
        .await?
        .into_iter()
        .map(|t| Transit {
            pair_key: t.pair_key,
            kind: t.kind,
            minutes: t.minutes,
            line: t.line,
            station_from: t.station_from,
            station_to: t.station_to,
        })
        .collect();

    print_ref(&opts.slug, &display_name, &timezone, &currency, &airports, &areas, &pois, &clusters, &transit, &tips);
    Ok(())
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
