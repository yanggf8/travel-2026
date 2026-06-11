//! Request router: path match → auth gate → plan load → render.
//! Auth runs BEFORE any Turso read (per spec §5). The /map/* route is the only
//! exception — it's a pure R2 passthrough with no auth (map images are low-stakes
//! and the page already links to them; gating them would just add latency).

use worker::*;
use std::collections::HashMap;
use crate::{auth, turso, model, render};

/// 1×1 transparent PNG — returned when an R2 map image is missing, so the
/// browser never shows a broken-image icon. Generated offline (standard
/// zlib-flate empty IDAT + standard PNG header/chunks).
const PLACEHOLDER_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, // PNG signature
    0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, // IHDR chunk len=13
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1×1
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, // 8-bit RGBA, filter=0
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, // CRC IHDR; IDAT len=10
    0x54, 0x78, 0x9c, 0x62, 0x00, 0x00, 0x00, 0x02, // compressed data (1×1 RGBA transparent)
    0x00, 0x01, 0xe5, 0x27, 0xde, 0xfc, 0x00, 0x00, // CRC IDAT; IEND len=0
    0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, // IEND
    0x60, 0x82,                                      // CRC IEND
];

pub async fn handle(req: Request, env: Env) -> Result<Response> {
    let url = req.url()?;
    let path = url.path().to_string();
    let query: HashMap<String, String> = url
        .query()
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .into_owned()
                .collect()
        })
        .unwrap_or_default();

    // /map/<plan>/<file>.png → R2 MAPS bucket passthrough (no auth; low-stakes asset).
    if let Some(rest) = path.strip_prefix("/map/") {
        let bucket = env.bucket("MAPS")?;
        if let Some(obj) = bucket.get(rest).execute().await? {
            if let Some(body) = obj.body() {
                let bytes = body.bytes().await?;
                let h = Headers::new();
                h.set("Content-Type", "image/png")?;
                h.set("Cache-Control", "public, max-age=86400")?;
                return Ok(Response::from_bytes(bytes)?.with_headers(h));
            }
        }
        // R2 miss (or no body) → placeholder, not a broken-image icon.
        return serve_placeholder();
    }

    // All other routes: load secrets + resolve auth BEFORE any Turso read.
    let turso_url = env.secret("TURSO_URL")?.to_string();
    let turso_token = env.secret("TURSO_TOKEN")?.to_string(); // READ token
    let owner_token = env.secret("OWNER_TOKEN")?.to_string();
    let lang = if query.get("lang").map(|s| s.as_str()) == Some("en") { "en" } else { "zh" };

    // Load share tokens (one query; small table).
    let share_rows = turso::pipeline(
        &turso_url,
        &turso_token,
        &["SELECT token, plan_id FROM plan_share_tokens".to_string()],
    )
    .await?;
    let mut shares: HashMap<String, String> = HashMap::new();
    if let Some(rows) = share_rows.first() {
        for r in rows {
            if let (Some(t), Some(p)) = (
                r.get("token").and_then(|v| v.as_str()),
                r.get("plan_id").and_then(|v| v.as_str()),
            ) {
                shares.insert(t.to_string(), p.to_string());
            }
        }
    }
    let scope = auth::resolve(query.get("token").map(|s| s.as_str()), &owner_token, &shares);

    // /voucher/<plan>/<file> → R2 VOUCHERS bucket passthrough (PDF).
    //
    // GATED: unlike /map/* (low-stakes images, ungated), vouchers embed booking
    // refs / guest names, so we require the same access scope as the plan view —
    // the plan slug is the FIRST path segment, checked via can_view_plan. We also
    // serve `Cache-Control: private, no-store` so intermediaries never cache it.
    // (Placed after scope resolution because it needs the share-token table.)
    if let Some(rest) = path.strip_prefix("/voucher/") {
        let plan_slug = rest.split('/').next().unwrap_or("");
        if !auth::can_view_plan(&scope, plan_slug) {
            return Response::error("Forbidden", 403);
        }
        let bucket = env.bucket("VOUCHERS")?;
        if let Some(obj) = bucket.get(rest).execute().await? {
            if let Some(body) = obj.body() {
                let bytes = body.bytes().await?;
                let h = Headers::new();
                h.set("Content-Type", "application/pdf")?;
                h.set("Cache-Control", "private, no-store")?;
                return Ok(Response::from_bytes(bytes)?.with_headers(h));
            }
        }
        // R2 miss (PDF not uploaded yet) → 404, not a placeholder.
        return Response::error("voucher not found", 404);
    }

    // Index — owner only.
    if path == "/" && query.get("plan").is_none() {
        if scope != auth::AccessScope::Owner {
            return Response::error("Forbidden", 403);
        }
        let plans = turso::pipeline(
            &turso_url,
            &turso_token,
            &[
                "SELECT p.plan_id, pd.display_name, d.start_date, d.end_date \
                 FROM plans p \
                 LEFT JOIN plan_destinations pd ON pd.plan_id = p.plan_id \
                 LEFT JOIN date_anchors d ON d.plan_id = p.plan_id \
                 WHERE p.deleted_at IS NULL \
                 ORDER BY p.plan_id"
                    .to_string(),
            ],
        )
        .await?;
        let rows = plans.first().cloned().unwrap_or_default();
        return Response::from_html(render::page(
            "Plans",
            &render::index::render(&rows, lang),
            lang,
        ));
    }

    // Single plan view.
    if let Some(slug) = query.get("plan") {
        if !auth::can_view_plan(&scope, slug) {
            return Response::error("Forbidden", 403);
        }
        let plan = load_plan(&turso_url, &turso_token, slug).await?;
        return Response::from_html(render::render_plan(&plan, lang));
    }

    Response::error("Forbidden", 403)
}

fn serve_placeholder() -> Result<Response> {
    let h = Headers::new();
    h.set("Content-Type", "image/png")?;
    h.set("Cache-Control", "public, max-age=300")?;
    Ok(Response::from_bytes(PLACEHOLDER_PNG.to_vec())?.with_headers(h))
}

/// Reject anything that is not `[a-z0-9_-]+`. The slug is interpolated into SQL
/// in load_plan; even though it comes from a token-scoped match or the owner
/// (trusted), defense-in-depth.
fn is_safe_slug(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Load the full plan via a 10-statement Turso pipeline. Query order matches
/// model::assemble()'s argument order exactly.
async fn load_plan(turso_url: &str, token: &str, slug: &str) -> Result<model::Plan> {
    if !is_safe_slug(slug) {
        return Err(Error::RustError(format!("invalid plan slug: {slug}")));
    }
    let dest = slug.replace('-', "_");
    let sqls: Vec<String> = vec![
        format!(
            "SELECT p.plan_id, d.start_date, d.end_date, pd.display_name \
             FROM plans p \
             JOIN date_anchors d ON d.plan_id = p.plan_id \
             LEFT JOIN plan_destinations pd ON pd.plan_id = p.plan_id \
             WHERE p.plan_id = '{slug}' AND p.deleted_at IS NULL"
        ),
        format!(
            "SELECT day_number, date, day_type, theme, theme_zh, weather_label \
             FROM days WHERE plan_id = '{slug}' ORDER BY day_number"
        ),
        format!(
            "SELECT day_number, session_type, focus_zh, transit_notes_zh \
             FROM timesofday WHERE plan_id = '{slug}'"
        ),
        format!(
            "SELECT day_number, session_type, title \
             FROM activities WHERE plan_id = '{slug}' ORDER BY day_number, sort_order"
        ),
        format!(
            "SELECT day_number, session_type, meal \
             FROM session_meals WHERE plan_id = '{slug}' ORDER BY day_number, sort_order"
        ),
        format!(
            "SELECT direction, flight_number, airline, departure_code, departure_terminal, \
             departure_time, arrival_code, arrival_terminal, arrival_time, flight_date \
             FROM flight_legs WHERE plan_id = '{slug}' ORDER BY direction, leg_order"
        ),
        format!(
            "SELECT name, name_zh, check_in, notes, voucher_url \
             FROM hotels WHERE plan_id = '{slug}'"
        ),
        format!(
            "SELECT direction, selected_title, selected_route, selected_duration_min, \
             selected_price_yen FROM airport_transfers WHERE plan_id = '{slug}'"
        ),
        format!(
            "SELECT title, lat, lon, address \
             FROM destination_pois WHERE slug = '{dest}'"
        ),
        format!(
            "SELECT day_number, from_place, to_place, mode, duration_min, notes, start_time \
             FROM day_route_segments WHERE plan_id = '{slug}' ORDER BY day_number, sort_order"
        ),
    ];
    let r = turso::pipeline(turso_url, token, &sqls).await?;
    if r.len() < 10 {
        return Err(Error::RustError("Turso pipeline returned fewer than 10 results".into()));
    }
    Ok(model::assemble(
        &r[0], &r[1], &r[2], &r[3], &r[4], &r[5], &r[6], &r[7], &r[8], &r[9],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_slug_accepts_valid() {
        assert!(is_safe_slug("okinawa-2026"));
        assert!(is_safe_slug("tokyo_2026"));
        assert!(is_safe_slug("a-b_c"));
    }

    #[test]
    fn safe_slug_rejects_invalid() {
        assert!(!is_safe_slug(""));
        assert!(!is_safe_slug("Tokyo"));       // uppercase
        assert!(!is_safe_slug("plan;DROP"));   // injection
        assert!(!is_safe_slug("a b"));         // space
        assert!(!is_safe_slug("a.b"));         // dot
        assert!(!is_safe_slug("'or'1'='1"));   // SQL injection
    }

    #[test]
    fn placeholder_png_is_valid_header() {
        // first 8 bytes must be the PNG signature
        assert_eq!(
            &PLACEHOLDER_PNG[..8],
            &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]
        );
    }
}
