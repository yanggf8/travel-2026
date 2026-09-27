// `travel road-leg ...` — the CLI surface for the OSRM road-geometry cache
// (`route_road_legs` + `route_road_leg_points`) that `snapshot-maps` reads and
// draws. snapshot-maps only re-RENDERS: a changed route narrative keeps drawing
// the old cached polyline (the 金山→淡水 陽明山 regression — the text said 台2
// coastal, the red line still cut inland). This command makes that cache
// visible and repairable:
//   travel road-leg list
//   travel road-leg refetch --from <place|lat,lon> --to <place|lat,lon> \
//                            [--via <a;b;c>] [--dest <slug>]
// `--via` waypoints are the whole point: OSRM always returns its fastest path,
// so re-fetching without waypoints reproduces the same mountain shortcut; a
// via point (石門洞; 三芝) is what forces the coastal 台2 route. When an existing
// leg matches the resolved endpoints, the re-fetch PRESERVES that leg's exact
// endpoints/key so itinerary stop pairs keep hitting the exact 5-dp key the
// renderer looks up first. OSRM-derived derived cache data, NOT domain content
// — no audit triad (same family as route_place_geocodes). Plain text output.

use libsql::{params, Connection};

pub async fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("list") => list(&args[1..]).await,
        Some("refetch") => refetch(&args[1..]).await,
        Some("--help") | Some("-h") | None => {
            println!("{}", usage());
            Ok(())
        }
        Some(other) => Err(format!("unknown road-leg subcommand: {other}\n{}", usage())),
    }
}

fn usage() -> &'static str {
    "Usage:\n  \
      travel road-leg list\n  \
      travel road-leg refetch --from <place|lat,lon> --to <place|lat,lon> [--via <a;b;c>] [--dest <slug>]\n\
      road-leg manages the OSRM road-geometry cache snapshot-maps draws. `refetch` re-fetches\n\
      a leg from OSRM; --via waypoints (place names or lat,lon, semicolon-separated) force a\n\
      specific road (e.g. 石門洞;三芝 for the 台2 coastal route) — without them OSRM returns\n\
      the same fastest path. Run `snapshot-maps` afterwards to re-render the map."
}

/// `road-leg list` — every cached leg with its fetch state, newest first.
async fn list(rest: &[String]) -> Result<(), String> {
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    for a in rest {
        if a.starts_with('-') {
            return Err(format!("unknown argument: {a}"));
        }
    }
    let read = crate::db::connect_read().await?;
    let mut rows = read
        .query(
            "SELECT leg_key, status, point_count, distance_m, failure_reason, fetched_at \
             FROM route_road_legs ORDER BY fetched_at DESC",
            params![],
        )
        .await
        .map_err(|e| format!("route_road_legs query failed: {e}"))?;
    let mut count = 0;
    println!("road legs in cache (newest first):");
    while let Some(row) = rows
        .next()
        .await
        .map_err(|e| format!("route_road_legs read failed: {e}"))?
    {
        count += 1;
        let key: String = row.get(0).map_err(|e| e.to_string())?;
        let status: String = row.get(1).map_err(|e| e.to_string())?;
        let points: i64 = row.get(2).map_err(|e| e.to_string())?;
        let distance: Option<f64> = row.get(3).ok().flatten();
        let failure: Option<String> = row.get(4).ok().flatten();
        let fetched: String = row.get(5).map_err(|e| e.to_string())?;
        let dist = match distance {
            Some(m) => format!("{:>8.1} km", m / 1000.0),
            None => format!("{:>8}", "-"),
        };
        match (status.as_str(), failure) {
            ("ok", _) => println!("  {dist}  {points:>5} pts  {fetched}  {key}"),
            (_, Some(why)) => println!("  {status}  {fetched}  {key}  ({why})"),
            (_, None) => println!("  {status}  {fetched}  {key}"),
        }
    }
    println!("{count} leg(s)");
    Ok(())
}

#[derive(Debug, Default)]
struct RefetchArgs {
    from: Option<String>,
    to: Option<String>,
    via: Vec<String>,
    dest: Option<String>,
}

fn parse_refetch(args: &[String]) -> Result<RefetchArgs, String> {
    let mut p = RefetchArgs::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--from" => {
                p.from = Some(args.get(i + 1).ok_or("--from requires a value")?.clone());
                i += 2
            }
            "--to" => {
                p.to = Some(args.get(i + 1).ok_or("--to requires a value")?.clone());
                i += 2
            }
            "--via" => {
                let v = args.get(i + 1).ok_or("--via requires a value")?;
                p.via = v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
                i += 2
            }
            "--dest" => {
                p.dest = Some(args.get(i + 1).ok_or("--dest requires a value")?.clone());
                i += 2
            }
            "--help" | "-h" => unreachable!("help handled before parsing"),
            x if x.starts_with('-') => return Err(format!("unknown argument: {x}")),
            x => return Err(format!("unexpected positional argument: {x} (use --from/--to/--via)")),
        }
    }
    Ok(p)
}

/// "25.22188,121.63619" → coords; anything else is a place label.
fn parse_latlon(tok: &str) -> Option<(f64, f64)> {
    let (a, b) = tok.trim().split_once(',')?;
    let lat = a.trim().parse::<f64>().ok()?;
    let lon = b.trim().parse::<f64>().ok()?;
    if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) {
        Some((lat, lon))
    } else {
        None
    }
}

/// Resolve one place token: a literal lat,lon; else a destination POI (when
/// --dest is given); else the geocode cache; else Nominatim (needs --dest for
/// country context — fail loud rather than geocode into the wrong country).
async fn resolve_token(
    tok: &str,
    read: &Connection,
    write: &Connection,
    dest: Option<&str>,
    dest_pois: &[crate::snapshot_maps::PoiRow],
    geocodes: &mut std::collections::HashMap<String, (f64, f64)>,
) -> Result<(f64, f64), String> {
    if let Some(c) = parse_latlon(tok) {
        return Ok(c);
    }
    // dest_pois is already scoped to --dest (empty when absent).
    if let Some(c) = crate::snapshot_maps::match_poi(tok, dest_pois) {
        return Ok(c);
    }
    let key = tok.trim().to_ascii_lowercase();
    if let Some(&c) = geocodes.get(&key) {
        return Ok(c);
    }
    let Some(dest) = dest else {
        return Err(format!(
            "cannot resolve place '{tok}' without --dest (Nominatim needs the destination's country context); \
             pass --dest <slug> or use literal lat,lon"
        ));
    };
    let context = crate::snapshot_maps::geocode_context(read, dest).await?;
    match crate::snapshot_maps::resolve_place(read, write, tok, &context, geocodes).await? {
        Some(c) => Ok(c),
        None => Err(format!("Nominatim could not resolve place '{tok}' (context {context}); \
             pin it with set-place-geocode or use literal lat,lon")),
    }
}

async fn refetch(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let p = parse_refetch(args)?;
    let from_tok = p.from.clone().ok_or("missing --from (place or lat,lon)")?;
    let to_tok = p.to.clone().ok_or("missing --to (place or lat,lon)")?;

    let read = crate::db::connect_read().await?;
    let write = crate::db::connect_write().await?;

    let dest_pois = match p.dest.as_deref() {
        Some(d) => crate::snapshot_maps::query_destination_pois(&read, d).await?,
        None => Vec::new(),
    };
    let mut geocodes = crate::snapshot_maps::load_geocodes(&read).await?;

    let from = resolve_token(&from_tok, &read, &write, p.dest.as_deref(), &dest_pois, &mut geocodes)
        .await
        .map_err(|e| format!("--from: {e}"))?;
    let to = resolve_token(&to_tok, &read, &write, p.dest.as_deref(), &dest_pois, &mut geocodes)
        .await
        .map_err(|e| format!("--to: {e}"))?;
    let mut via = Vec::new();
    for (i, v) in p.via.iter().enumerate() {
        via.push(
            resolve_token(v, &read, &write, p.dest.as_deref(), &dest_pois, &mut geocodes)
                .await
                .map_err(|e| format!("--via #{}: {e}", i + 1))?,
        );
    }
    println!("  from: {from_tok}  →  {:.5},{:.5}", from.0, from.1);
    for (i, c) in via.iter().enumerate() {
        println!("  via {}: {}  →  {:.5},{:.5}", i + 1, p.via[i], c.0, c.1);
    }
    println!("  to:   {to_tok}  →  {:.5},{:.5}", to.0, to.1);

    // An existing leg matching the resolved endpoints keeps its exact
    // coordinates for the re-fetch — the itinerary's stop pair built that key,
    // and re-keying to freshly geocoded coordinates would leave the old leg
    // orphaned and the render falling back to fuzzy matching.
    let mut legs = Vec::new();
    {
        let mut r = read
            .query(
                "SELECT leg_key, from_lat, from_lon, to_lat, to_lon FROM route_road_legs",
                params![],
            )
            .await
            .map_err(|e| format!("route_road_legs query failed: {e}"))?;
        while let Some(row) = r
            .next()
            .await
            .map_err(|e| format!("route_road_legs read failed: {e}"))?
        {
            let (Ok(key), Ok(flat), Ok(flon), Ok(tlat), Ok(tlon)) =
                (row.get(0), row.get(1), row.get(2), row.get(3), row.get(4))
            else {
                continue;
            };
            legs.push(crate::snapshot_maps::LegRow {
                key,
                from: (flat, flon),
                to: (tlat, tlon),
            });
        }
    }
    let (from, to) = match crate::snapshot_maps::match_leg(&legs, from, to) {
        Some(existing) => {
            println!("  replacing cached leg {existing_key}", existing_key = existing.key);
            (existing.from, existing.to)
        }
        None => (from, to),
    };

    let pts = crate::snapshot_maps::fetch_osrm_leg(&write, from, to, via).await?;
    let key = format!(
        "{:.5},{:.5}>{:.5},{:.5}|osrm-demo|driving",
        from.0, from.1, to.0, to.1
    );
    let mut r = read
        .query(
            "SELECT distance_m FROM route_road_legs WHERE leg_key=?1",
            params![key.clone()],
        )
        .await
        .map_err(|e| format!("leg re-read failed: {e}"))?;
    let dist_km = match r
        .next()
        .await
        .map_err(|e| format!("leg re-read failed: {e}"))?
    {
        Some(row) => row
            .get::<Option<f64>>(0)
            .ok()
            .flatten()
            .map(|m| m / 1000.0)
            .unwrap_or(0.0),
        None => 0.0,
    };
    println!("✅ road leg cached: {key}");
    println!("  {dist_km:.1} km, {} geometry points", pts.len());
    println!("  run `travel snapshot-maps` to re-render the map with this route");
    Ok(())
}
