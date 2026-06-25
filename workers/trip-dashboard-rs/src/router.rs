//! Request router: path match → auth gate → plan load → render.
//! Auth runs BEFORE any Turso read (per spec §5). The /map/* route is the only
//! exception — it's a pure R2 passthrough with no auth (map images are low-stakes
//! and the page already links to them; gating them would just add latency).

use worker::*;
use std::collections::HashMap;
use worker_github_oauth::{self as gho, CallbackOutcome, OauthConfig};
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
                // Garbage 1-byte captures and tiny stubs are NOT real maps — fall through
                // to the valid 66-byte placeholder so clients never get image/png junk.
                if render::map::is_valid_map_png(&bytes) {
                    let h = Headers::new();
                    h.set("Content-Type", "image/png")?;
                    h.set("Cache-Control", "public, max-age=86400")?;
                    return Ok(Response::from_bytes(bytes)?.with_headers(h));
                }
            }
        }
        // R2 miss, no body, or invalid/garbage PNG → placeholder, not a broken-image icon.
        return serve_placeholder();
    }

    let public_origin = env.secret("PUBLIC_ORIGIN")?.to_string();
    let cfg = OauthConfig {
        callback_url: format!("{public_origin}/auth/callback"),
        user_agent: "trip-dashboard-rs".into(),
        cookie_prefix: "td".into(),
    };
    let lang = if query.get("lang").map(|s| s.as_str()) == Some("en") {
        "en"
    } else {
        "zh"
    };

    // OAuth routes — no Turso read.
    match path.as_str() {
        "/auth/login" => {
            let next = url
                .query_pairs()
                .find(|(k, _)| k == "next")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_else(|| "/".into());
            return gho::start_login(&env, &cfg, &next);
        }
        "/auth/callback" => {
            return match gho::callback(&req, &env, &cfg, &url).await? {
                CallbackOutcome::Authorized(r) => Ok(r),
                CallbackOutcome::BadState(r) => Ok(r),
                CallbackOutcome::Denied { login, .. } => Ok(Response::from_html(
                    render::auth::not_authorized_page(&login, lang),
                )?
                .with_status(403)),
            };
        }
        "/auth/logout" => return gho::logout(&cfg),
        _ => {}
    }

    // All other routes: load secrets, share-token auth data, then resolve scope.
    let turso_url = env.secret("TURSO_URL")?.to_string();
    let turso_token = env.secret("TURSO_TOKEN")?.to_string(); // READ token

    // Load share tokens (one query; small table). DESC so copy map picks a current
    // token per plan via or_insert; auth map still gets every token via insert.
    let share_rows = turso::pipeline(
        &turso_url,
        &turso_token,
        &["SELECT token, plan_id FROM plan_share_tokens ORDER BY created_at DESC".to_string()],
    )
    .await?;
    let mut share_pairs: Vec<(String, String)> = Vec::new();
    if let Some(rows) = share_rows.first() {
        for r in rows {
            if let (Some(t), Some(p)) = (
                r.get("token").and_then(|v| v.as_str()),
                r.get("plan_id").and_then(|v| v.as_str()),
            ) {
                share_pairs.push((t.to_string(), p.to_string()));
            }
        }
    }
    let (shares, plan_share_tokens) = render::share::build_share_maps(&share_pairs);

    let secret = env
        .secret("SESSION_SECRET")
        .map(|s| s.to_string())
        .unwrap_or_default();
    let allowed = env
        .var("ALLOWED_LOGIN")
        .map(|v| v.to_string())
        .unwrap_or_default();
    let allowed_id = gho::allowed_id(&env);
    let session_login = gho::read_cookie(&req, &cfg.session_cookie())
        .and_then(|c| gho::verify_session(&secret, &allowed, allowed_id, &c));
    let is_owner_session = session_login.is_some();

    let mut scope = auth::resolve(query.get("token").map(|s| s.as_str()), &shares);
    if is_owner_session {
        scope = auth::AccessScope::Owner;
    }

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
            return Ok(Response::from_html(render::auth::sign_in_page(lang))?);
        }
        let plans = turso::pipeline(
            &turso_url,
            &turso_token,
            &[
                // One row per plan (GROUP BY collapses the destination/anchor joins),
                // ordered chronologically by the plan's earliest trip date — earliest
                // first; plans with no date anchor sort last (NULL → far-future key).
                "SELECT p.plan_id, MIN(pd.display_name) AS display_name, \
                        MIN(d.start_date) AS start_date, MAX(d.end_date) AS end_date \
                 FROM plans p \
                 LEFT JOIN plan_destinations pd ON pd.plan_id = p.plan_id \
                 LEFT JOIN date_anchors d ON d.plan_id = p.plan_id \
                 WHERE p.deleted_at IS NULL \
                 GROUP BY p.plan_id \
                 ORDER BY COALESCE(MIN(d.start_date), '9999-12-31') ASC, p.plan_id ASC"
                    .to_string(),
            ],
        )
        .await?;
        let rows = plans.first().cloned().unwrap_or_default();
        // Owner banner name: the session login if present, else the configured
        // ALLOWED_LOGIN (never a hardcoded handle — honors "no hardcode").
        let owner_login = session_login.as_deref().unwrap_or(allowed.as_str());
        let body = format!(
            "{}{}{}",
            render::auth::signed_in_banner(owner_login, lang),
            render::index::render(&rows, &plan_share_tokens, &public_origin, lang),
            render::share::COPY_SCRIPT,
        );
        return Response::from_html(render::page("Plans", &body, lang));
    }

    // Single plan view.
    if let Some(slug) = query.get("plan") {
        if !auth::can_view_plan(&scope, slug) {
            let login_href = owner_login_href_for_plan(slug, lang);
            return Response::from_html(render::auth::bad_share_page(&login_href, lang))
                .map(|r| r.with_status(403));
        }
        let plan = load_plan(&turso_url, &turso_token, slug).await?;
        let map_status = check_map_status(&env, &plan.plan_id, &plan.days).await?;
        let token = query.get("token").map(|s| s.as_str());
        // Logged-in owner: copy a viewer share URL (share token) for others — never
        // the request ?token= and never the session cookie. Viewers opening a share
        // link get no chrome (they are not logged in as owner).
        let owner_chrome = if is_owner_session {
            let login = session_login.as_deref().unwrap_or(allowed.as_str());
            render::share::owner_plan_chrome(
                slug,
                plan_share_tokens.get(slug).map(|s| s.as_str()),
                &public_origin,
                login,
                lang,
            )
        } else {
            String::new()
        };
        return Response::from_html(render::render_plan(
            &plan, lang, token, &map_status, &owner_chrome,
        ));
    }

    Ok(Response::from_html(render::auth::sign_in_page(lang))?)
}

/// Probe R2 for each expected map key and record whether a real PNG is present.
async fn check_map_status(
    env: &Env,
    plan_id: &str,
    days: &[model::Day],
) -> Result<render::map::MapStatus> {
    let bucket = env.bucket("MAPS")?;
    let plan_key = format!("{plan_id}/plan.png");
    let plan = r2_has_valid_map(&bucket, &plan_key).await?;
    let mut day_status = HashMap::new();
    for d in days {
        let key = format!("{plan_id}/day-{}.png", d.day_number);
        day_status.insert(d.day_number, r2_has_valid_map(&bucket, &key).await?);
    }
    Ok(render::map::MapStatus {
        plan,
        days: day_status,
    })
}

async fn r2_has_valid_map(bucket: &worker::Bucket, key: &str) -> Result<bool> {
    if let Some(obj) = bucket.get(key).execute().await? {
        if let Some(body) = obj.body() {
            let bytes = body.bytes().await?;
            return Ok(render::map::is_valid_map_png(&bytes));
        }
    }
    Ok(false)
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

/// Login URL for an owner who opened `/?plan=<slug>` without a valid session.
/// Preserve the plan and language only; never carry a bad viewer token into
/// the post-OAuth owner view.
fn owner_login_href_for_plan(slug: &str, lang: &str) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    q.append_pair("plan", slug);
    if lang == "en" {
        q.append_pair("lang", "en");
    }
    let next = format!("/?{}", q.finish());
    format!("/auth/login?next={}", render::urlencode(&next))
}

/// Load the full plan via a 12-statement Turso pipeline. Query order matches
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
            "SELECT day_number, date, day_type, theme, theme_zh, weather_label, \
             temp_low_c, temp_high_c, precipitation_pct, feels_like_low_c, feels_like_high_c \
             FROM days WHERE plan_id = '{slug}' ORDER BY day_number"
        ),
        format!(
            "SELECT day_number, session_type, focus_zh, transit_notes_zh \
             FROM timesofday WHERE plan_id = '{slug}'"
        ),
        format!(
            "SELECT day_number, session_type, title, poi_id, \
             book_by, booking_status, booking_url \
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
            "SELECT poi_id, title, lat, lon, address, cost_estimate \
             FROM destination_pois WHERE slug = '{dest}'"
        ),
        format!(
            "SELECT day_number, from_place, to_place, mode, duration_min, notes, start_time \
             FROM day_route_segments WHERE plan_id = '{slug}' ORDER BY day_number, sort_order"
        ),
        // [10] transit cheat-sheet key lines (feature #4), keyed dest:lang at render time.
        format!(
            "SELECT destination, lang, line \
             FROM itinerary_transit_key_lines WHERE plan_id = '{slug}' \
             ORDER BY destination, lang, sort_order"
        ),
        // [11] itinerary metadata — home-base station (en + zh) for the cheat-sheet.
        format!(
            "SELECT transit_hotel_station, transit_hotel_station_zh \
             FROM itinerary_metadata WHERE plan_id = '{slug}'"
        ),
    ];
    let r = turso::pipeline(turso_url, token, &sqls).await?;
    if r.len() < 12 {
        return Err(Error::RustError("Turso pipeline returned fewer than 12 results".into()));
    }
    Ok(model::assemble(
        &r[0], &r[1], &r[2], &r[3], &r[4], &r[5], &r[6], &r[7], &r[8], &r[9], &r[10], &r[11],
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

    #[test]
    fn owner_login_href_preserves_plan_target() {
        assert_eq!(
            owner_login_href_for_plan("okinawa-2026", "zh"),
            "/auth/login?next=%2F%3Fplan%3Dokinawa-2026"
        );
    }

    #[test]
    fn owner_login_href_preserves_language_but_not_token() {
        assert_eq!(
            owner_login_href_for_plan("okinawa-2026", "en"),
            "/auth/login?next=%2F%3Fplan%3Dokinawa-2026%26lang%3Den"
        );
    }
}
