//! Request router: path match → auth gate → plan load → render.
//! Auth runs BEFORE any Turso read (per spec §5). The /map/* route is the only
//! exception — it's a pure R2 passthrough with no auth (map images are low-stakes
//! and the page already links to them; gating them would just add latency).

use crate::{auth, d1_compare, model, render, turso};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use worker::*;
use worker_github_oauth::{self as gho, CallbackOutcome, OauthConfig};

type HmacSha256 = Hmac<Sha256>;

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
    0x60, 0x82, // CRC IEND
];

pub async fn handle(mut req: Request, env: Env) -> Result<Response> {
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
                CallbackOutcome::Denied { login, .. } => Ok(html_no_store(
                    render::auth::not_authorized_page(&login, lang),
                )?
                .with_status(403)),
            };
        }
        "/auth/logout" => return gho::logout(&cfg),
        _ => {}
    }

    // All other routes: load secrets and resolve owner session before any
    // owner-only mutation route is allowed to use the write Turso token.
    let turso_url = env.secret("TURSO_URL")?.to_string();
    let turso_token = env.secret("TURSO_TOKEN")?.to_string(); // READ token
    let secret = env
        .secret("SESSION_SECRET")
        .map(|s| s.to_string())
        .unwrap_or_default();
    let allowed = env
        .var("ALLOWED_LOGIN")
        .map(|v| v.to_string())
        .unwrap_or_default();
    let allowed_id = gho::allowed_id(&env);
    let session_cookie = gho::read_cookie(&req, &cfg.session_cookie());
    let session_login = session_cookie
        .as_deref()
        .and_then(|c| gho::verify_session(&secret, &allowed, allowed_id, c));
    let is_owner_session = session_login.is_some();

    // /diag/d1-compare — D1 read-mirror pilot (Phase G, compare-only). Owner-only; 404 unless the
    // pilot is explicitly enabled AND a MIRROR_DB D1 binding exists, so a normal deploy is unaffected.
    // Reads the same tables from Turso + D1 and returns a plain-text delta. Never serves from D1.
    if path == "/diag/d1-compare" {
        if !is_owner_session {
            return Response::error("Forbidden", 403);
        }
        if !d1_compare::enabled(&env) {
            return Response::error("D1 compare pilot not enabled", 404);
        }
        let report = d1_compare::compare(&env, &turso_url, &turso_token).await;
        let h = Headers::new();
        h.set("Content-Type", "text/plain; charset=utf-8")?;
        h.set("Cache-Control", "private, no-store")?;
        return Ok(Response::ok(report)?.with_headers(h));
    }

    if path == "/grants/create" || path == "/grants/deactivate" {
        // Owner-session (403) and method (405) gate. These two checks need a live
        // `Method`/session, so they're covered by the deploy smoke + `curl` spot-checks,
        // not unit tests. The slug/CSRF/token/dispatch outcomes ARE unit-tested via
        // `decide_grant_post`.
        if !is_owner_session {
            return Response::error("Forbidden", 403);
        }
        if req.method() != Method::Post {
            return Response::error("Method not allowed", 405);
        }
        let write_token = env.secret("TURSO_WRITE_TOKEN")?.to_string();
        let csrf = GrantCsrf::new(&secret, session_cookie.as_deref().unwrap_or(""));
        let owner_login = session_login.as_deref().unwrap_or(allowed.as_str());
        return handle_grant_post(
            &mut req,
            &path,
            &turso_url,
            &write_token,
            &csrf,
            owner_login,
        )
        .await;
    }

    // Load grant tokens (one query; small table). DESC so the current-token map
    // keeps the newest active token while auth still accepts all active tokens.
    let grant_rows = turso::pipeline(
        &turso_url,
        &turso_token,
        &["SELECT token, plan_id, status, created_at, created_by, deactivated_at, deactivated_by \
           FROM plan_share_tokens ORDER BY created_at DESC, token DESC"
            .to_string()],
    )
    .await?;
    let grants =
        render::share::build_grant_maps(grant_rows.first().map(Vec::as_slice).unwrap_or(&[]));

    let mut scope = auth::resolve(
        query.get("token").map(|s| s.as_str()),
        &grants.token_to_plan,
    );
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
            return html_no_store(render::auth::sign_in_page(lang));
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
        let csrf = GrantCsrf::new(&secret, session_cookie.as_deref().unwrap_or(""));
        let body = format!(
            "{}{}{}",
            render::auth::signed_in_banner(owner_login, lang),
            render::index::render(&rows, &grants, &public_origin, &csrf, lang),
            render::share::COPY_SCRIPT,
        );
        return html_no_store(render::page("Plans", &body, lang));
    }

    // Single plan view.
    if let Some(slug) = query.get("plan") {
        if !auth::can_view_plan(&scope, slug) {
            let login_href = owner_login_href_for_plan(slug, lang);
            return Ok(
                html_no_store(render::auth::bad_share_page(&login_href, lang))?.with_status(403),
            );
        }
        let plan = load_plan(&turso_url, &turso_token, slug).await?;
        let map_status = check_map_status(&env, &plan.plan_id, &plan.days).await?;
        let token = query.get("token").map(|s| s.as_str());
        // Logged-in owner: copy a viewer share URL (share token) for others — never
        // the request ?token= and never the session cookie. Viewers opening a share
        // link get no chrome (they are not logged in as owner).
        let owner_chrome = if is_owner_session {
            let login = session_login.as_deref().unwrap_or(allowed.as_str());
            let csrf = GrantCsrf::new(&secret, session_cookie.as_deref().unwrap_or(""));
            render::share::owner_plan_chrome(
                slug,
                grants.plan_to_current.get(slug),
                &public_origin,
                login,
                &csrf.create(slug),
                lang,
            )
        } else {
            String::new()
        };
        return html_no_store(render::render_plan(
            &plan,
            lang,
            token,
            &map_status,
            &owner_chrome,
        ));
    }

    html_no_store(render::auth::sign_in_page(lang))
}

/// The pure outcome of validating a grant POST, decoupled from Worker I/O.
///
/// Everything that decides the *response* — slug/token validation, CSRF check,
/// sub-path dispatch — is computed here so it is host-unit-testable without a live
/// `Request`/`Env`. The plan-existence 404 is deliberately NOT decided here: it needs
/// a DB read, so `PlanCheckThenCreate` defers it to the caller. The owner-session 403
/// and method 405 gate lives upstream in `handle` (it needs `Method`/session) and is
/// covered by the deploy smoke test, not unit tests.
#[derive(Debug, PartialEq, Eq)]
enum GrantDecision {
    InvalidSlug,
    InvalidToken,
    BadCsrf,
    PlanCheckThenCreate { plan: String },
    Deactivate { plan: String, token: String },
    NotFound,
}

/// Pure decision logic for `/grants/{create,deactivate}`. Mirrors the original
/// inline checks 1:1 (slug first; for deactivate, token-format before CSRF), minus the
/// DB-backed plan-existence check which the caller performs for `PlanCheckThenCreate`.
fn decide_grant_post(
    path: &str,
    plan: &str,
    posted_csrf: &str,
    token: &str,
    csrf: &GrantCsrf,
) -> GrantDecision {
    if !is_safe_slug(plan) {
        return GrantDecision::InvalidSlug;
    }
    match path {
        "/grants/create" => {
            if !csrf.verify_create(plan, posted_csrf) {
                return GrantDecision::BadCsrf;
            }
            GrantDecision::PlanCheckThenCreate {
                plan: plan.to_string(),
            }
        }
        "/grants/deactivate" => {
            if !is_grant_token(token) {
                return GrantDecision::InvalidToken;
            }
            if !csrf.verify_deactivate(plan, token, posted_csrf) {
                return GrantDecision::BadCsrf;
            }
            GrantDecision::Deactivate {
                plan: plan.to_string(),
                token: token.to_string(),
            }
        }
        _ => GrantDecision::NotFound,
    }
}

async fn handle_grant_post(
    req: &mut Request,
    path: &str,
    turso_url: &str,
    write_token: &str,
    csrf: &GrantCsrf,
    owner_login: &str,
) -> Result<Response> {
    let form = req.form_data().await?;
    let plan = form.get_field("plan").unwrap_or_default();
    let posted_csrf = form.get_field("csrf").unwrap_or_default();
    let token = form.get_field("token").unwrap_or_default();
    match decide_grant_post(path, &plan, &posted_csrf, &token, csrf) {
        GrantDecision::InvalidSlug => Response::error("invalid plan", 400),
        GrantDecision::InvalidToken => Response::error("invalid token", 400),
        GrantDecision::BadCsrf => Response::error("invalid csrf", 403),
        GrantDecision::NotFound => Response::error("not found", 404),
        GrantDecision::PlanCheckThenCreate { plan } => {
            if !grant_plan_exists(turso_url, write_token, &plan).await? {
                return Response::error("plan not found", 404);
            }
            create_grant(turso_url, write_token, &plan, owner_login).await?;
            redirect_after_grant("created", &plan)
        }
        GrantDecision::Deactivate { plan, token } => {
            deactivate_grant(turso_url, write_token, &plan, &token, owner_login).await?;
            redirect_after_grant("inactive", &plan)
        }
    }
}

async fn grant_plan_exists(turso_url: &str, write_token: &str, plan: &str) -> Result<bool> {
    let rows = turso::pipeline(
        turso_url,
        write_token,
        &[format!(
            "SELECT 1 FROM plans WHERE plan_id = '{plan}' AND deleted_at IS NULL"
        )],
    )
    .await?;
    Ok(rows.first().map(|r| !r.is_empty()).unwrap_or(false))
}

// SQL-builder seam: the two `build_*_grant_sql` fns below are pure so the SQL shape
// (column set, the create `WHERE EXISTS` guard, the deactivate `status = 'active'`
// guard, owner escaping) is host-unit-testable without a DB or the Worker runtime.
//
// SAFETY — interpolation: `turso::pipeline` sends raw SQL with NO bind parameters, so
// every interpolated value must be safe by construction. `plan` and `token` reach here
// only after `is_safe_slug` / `is_grant_token` (enforced in `decide_grant_post`).
// `owner_login` originates ONLY from `gho::verify_session`, which returns the login
// solely after it matches the allow-listed `ALLOWED_LOGIN`, so it can only ever be that
// one constant value; `sql_quote` (single-quote doubling) is kept as defense-in-depth.
// If the OAuth crate is ever relaxed to accept multiple logins / org members, this
// becomes an injection sink — keep the allow-list invariant or switch to bound params.

fn build_create_grant_sql(plan: &str, token: &str, owner_login: &str) -> String {
    let owner_sql = sql_quote(owner_login);
    format!(
        "INSERT INTO plan_share_tokens (plan_id, token, created_at, status, created_by) \
         SELECT '{plan}', '{token}', datetime('now'), 'active', '{owner_sql}' \
         WHERE EXISTS (SELECT 1 FROM plans WHERE plan_id = '{plan}' AND deleted_at IS NULL)"
    )
}

fn build_deactivate_grant_sql(plan: &str, token: &str, owner_login: &str) -> String {
    let owner_sql = sql_quote(owner_login);
    format!(
        "UPDATE plan_share_tokens \
         SET status='inactive', deactivated_at=datetime('now'), deactivated_by='{owner_sql}' \
         WHERE plan_id = '{plan}' AND token = '{token}' AND status = 'active'"
    )
}

async fn create_grant(
    turso_url: &str,
    write_token: &str,
    plan: &str,
    owner_login: &str,
) -> Result<()> {
    let token = mint_grant_token();
    let sql = build_create_grant_sql(plan, &token, owner_login);
    turso::pipeline(turso_url, write_token, &[sql]).await?;
    Ok(())
}

async fn deactivate_grant(
    turso_url: &str,
    write_token: &str,
    plan: &str,
    token: &str,
    owner_login: &str,
) -> Result<()> {
    let sql = build_deactivate_grant_sql(plan, token, owner_login);
    turso::pipeline(turso_url, write_token, &[sql]).await?;
    Ok(())
}

fn redirect_after_grant(status: &str, plan: &str) -> Result<Response> {
    let h = Headers::new();
    h.set("Location", &format!("/?grant={status}&plan={plan}"))?;
    Ok(Response::empty()?.with_status(303).with_headers(h))
}

fn mint_grant_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("CSPRNG failed");
    let mut s = String::with_capacity(32);
    use std::fmt::Write;
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn is_grant_token(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn sql_quote(s: &str) -> String {
    s.replace('\'', "''")
}

#[derive(Debug, Clone)]
pub struct GrantCsrf {
    secret: String,
    session_cookie: String,
}

impl GrantCsrf {
    pub fn new(secret: &str, session_cookie: &str) -> Self {
        Self {
            secret: secret.to_string(),
            session_cookie: session_cookie.to_string(),
        }
    }

    #[cfg(test)]
    pub fn test() -> Self {
        Self::new("test-secret", "test-session-cookie")
    }

    pub fn create(&self, plan: &str) -> String {
        self.sign(&format!("grant:create:{plan}:{}", self.session_cookie))
    }

    pub fn deactivate(&self, plan: &str, token: &str) -> String {
        self.sign(&format!(
            "grant:deactivate:{plan}:{token}:{}",
            self.session_cookie
        ))
    }

    fn verify_create(&self, plan: &str, provided: &str) -> bool {
        self.verify(
            &format!("grant:create:{plan}:{}", self.session_cookie),
            provided,
        )
    }

    fn verify_deactivate(&self, plan: &str, token: &str, provided: &str) -> bool {
        self.verify(
            &format!("grant:deactivate:{plan}:{token}:{}", self.session_cookie),
            provided,
        )
    }

    fn sign(&self, msg: &str) -> String {
        let mut mac =
            HmacSha256::new_from_slice(self.secret.as_bytes()).expect("HMAC accepts any key");
        mac.update(msg.as_bytes());
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    }

    fn verify(&self, msg: &str, provided: &str) -> bool {
        let Ok(bytes) = URL_SAFE_NO_PAD.decode(provided) else {
            return false;
        };
        let Ok(mut mac) = HmacSha256::new_from_slice(self.secret.as_bytes()) else {
            return false;
        };
        mac.update(msg.as_bytes());
        mac.verify_slice(&bytes).is_ok()
    }
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
    Ok(bucket
        .head(key)
        .await?
        .map(|obj| obj.size() as usize > render::map::MIN_MAP_PNG_BYTES)
        .unwrap_or(false))
}

fn serve_placeholder() -> Result<Response> {
    let h = Headers::new();
    h.set("Content-Type", "image/png")?;
    h.set("Cache-Control", "public, max-age=300")?;
    Ok(Response::from_bytes(PLACEHOLDER_PNG.to_vec())?.with_headers(h))
}

fn html_no_store(body: String) -> Result<Response> {
    let h = Headers::new();
    h.set("Cache-Control", "private, no-store")?;
    Ok(Response::from_html(body)?.with_headers(h))
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

/// Load the full plan via a 18-statement Turso pipeline. Query order matches
/// model::assemble()'s argument order exactly.
async fn load_plan(turso_url: &str, token: &str, slug: &str) -> Result<model::Plan> {
    if !is_safe_slug(slug) {
        return Err(Error::RustError(format!("invalid plan slug: {slug}")));
    }
    // Fallback only for legacy plans without a plan_destinations row; the
    // authoritative dest is the plan_destinations subquery used in [8], [15], [16].
    let dest_fallback = slug.replace('-', "_");
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
             book_by, booking_status, booking_url, source \
             FROM activities WHERE plan_id = '{slug}' ORDER BY day_number, sort_order"
        ),
        format!(
            "SELECT day_number, session_type, meal, source \
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
             FROM destination_pois WHERE slug = COALESCE((SELECT destination FROM plan_destinations WHERE plan_id = '{slug}' LIMIT 1), '{dest_fallback}')"
        ),
        format!(
            "SELECT day_number, from_place, to_place, mode, duration_min, notes, start_time, source \
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
        // [12] selected package offer + its price at the selected date (falls back to
        // price_per_person when there is no date-specific row). Booking-summary package row.
        format!(
            "SELECT o.source_id, o.product_code, o.price_per_person, o.currency, \
             sel.selected_date, dp.price AS date_price \
             FROM plan_offer_selection sel \
             JOIN plan_offers o ON o.plan_id = sel.plan_id AND o.destination = sel.destination \
               AND o.id = sel.selected_offer_id \
             LEFT JOIN plan_offer_date_pricing dp ON dp.plan_id = sel.plan_id \
               AND dp.destination = sel.destination AND dp.offer_id = sel.selected_offer_id \
               AND dp.date = sel.selected_date \
             WHERE sel.plan_id = '{slug}'"
        ),
        // [13] hotel access lines (transit directions to the hotel) — booking-summary hotel block.
        format!(
            "SELECT line FROM hotel_access_lines WHERE plan_id = '{slug}' ORDER BY sort_order"
        ),
        // [14] per-day map landmarks (waypoints listed under each day's map).
        format!(
            "SELECT day_number, landmark FROM day_landmarks WHERE plan_id = '{slug}' \
             ORDER BY day_number, sort_order"
        ),
        // [15] destination currency — JPY drives the Japan-only entry-info rows.
        // Use plan_destinations as authoritative dest, fallback to slug.replace for legacy.
        format!(
            "SELECT currency FROM destination_config WHERE slug = COALESCE((SELECT destination FROM plan_destinations WHERE plan_id = '{slug}' LIMIT 1), '{dest_fallback}')"
        ),
        // [16] domestic accommodation stays (booked) for Booking Summary — jiufen et al.
        // Filter by authoritative dest when available; fallback to any dest for legacy plans.
        format!(
            "SELECT trip_id, destination, title, price_amount, price_currency, status, selected_date FROM bookings_current WHERE trip_id = '{slug}' AND category = 'accommodation' AND status = 'booked' AND destination = COALESCE((SELECT destination FROM plan_destinations WHERE plan_id = '{slug}' LIMIT 1), destination)"
        ),
        // [17] domestic accommodation candidates (sea-view shortlist) — jiufen three
        format!(
            "SELECT hotel_name, room_type, price_twd, currency, sea_view, breakfast_included, image_url, source FROM domestic_accommodations WHERE destination = COALESCE((SELECT destination FROM plan_destinations WHERE plan_id = '{slug}' LIMIT 1), '{dest_fallback}') ORDER BY price_twd ASC"
        ),
    ];
    let r = turso::pipeline(turso_url, token, &sqls).await?;
    if r.len() < 18 {
        return Err(Error::RustError(
            "Turso pipeline returned fewer than 18 results".into(),
        ));
    }
    Ok(model::assemble(
        &r[0], &r[1], &r[2], &r[3], &r[4], &r[5], &r[6], &r[7], &r[8], &r[9], &r[10], &r[11],
        &r[12], &r[13], &r[14], &r[15], &r[16], &r[17],
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
        assert!(!is_safe_slug("Tokyo")); // uppercase
        assert!(!is_safe_slug("plan;DROP")); // injection
        assert!(!is_safe_slug("a b")); // space
        assert!(!is_safe_slug("a.b")); // dot
        assert!(!is_safe_slug("'or'1'='1")); // SQL injection
    }

    #[test]
    fn grant_token_validator_accepts_lower_hex_only() {
        assert!(is_grant_token("0123456789abcdef0123456789abcdef"));
        assert!(!is_grant_token("0123456789ABCDEF0123456789ABCDEF"));
        assert!(!is_grant_token("short"));
        assert!(!is_grant_token("0123456789abcdef0123456789abcdeg"));
    }

    #[test]
    fn grant_csrf_verifies_action_and_token_scope() {
        let csrf = GrantCsrf::test();
        let create = csrf.create("okinawa-2026");
        assert!(csrf.verify_create("okinawa-2026", &create));
        assert!(!csrf.verify_create("tokyo-2026", &create));

        let token = "0123456789abcdef0123456789abcdef";
        let deactivate = csrf.deactivate("okinawa-2026", token);
        assert!(csrf.verify_deactivate("okinawa-2026", token, &deactivate));
        assert!(!csrf.verify_deactivate(
            "okinawa-2026",
            "ffffffffffffffffffffffffffffffff",
            &deactivate
        ));
    }

    // A valid 32-lowercase-hex token reused across the builder/decision tests.
    const TEST_TOKEN: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn sql_quote_doubles_single_quotes() {
        assert_eq!(sql_quote("o'brien"), "o''brien");
        assert_eq!(sql_quote("plain"), "plain");
        assert_eq!(sql_quote("a'b'c"), "a''b''c");
        assert_eq!(sql_quote("'; DROP TABLE x;--"), "''; DROP TABLE x;--");
    }

    #[test]
    fn create_sql_has_where_exists_guard() {
        let sql = build_create_grant_sql("okinawa-2026", TEST_TOKEN, "yanggf8");
        assert!(sql.contains(
            "WHERE EXISTS (SELECT 1 FROM plans WHERE plan_id = 'okinawa-2026' \
             AND deleted_at IS NULL)"
        ));
        // Insert path sets the new row active and records the owner.
        assert!(sql.contains("'active'"));
        assert!(sql.contains("'yanggf8'"));
        assert!(sql.contains(&format!("'{TEST_TOKEN}'")));
    }

    #[test]
    fn deactivate_sql_keeps_active_guard() {
        let sql = build_deactivate_grant_sql("okinawa-2026", TEST_TOKEN, "yanggf8");
        assert!(sql.contains("status='inactive'"));
        // The guard that prevents re-deactivating an already-inactive row.
        assert!(sql.contains("AND status = 'active'"));
        assert!(sql.contains(&format!("token = '{TEST_TOKEN}'")));
    }

    #[test]
    fn create_sql_escapes_owner_login() {
        let sql = build_create_grant_sql("p", TEST_TOKEN, "o'brien");
        // sql_quote must be wired into the builder: the escaped form is present...
        assert!(sql.contains("'o''brien'"));
        // ...and the un-escaped bare form (which would break out of the string) is not.
        assert!(!sql.contains("'o'brien'"));
    }

    #[test]
    fn mint_grant_token_is_32_lowercase_hex() {
        // NOTE: host-side this exercises NATIVE getrandom. The WASM Web-Crypto (js
        // feature) path is proven by the live create smoke test, NOT by this test.
        let tok = mint_grant_token();
        assert!(
            is_grant_token(&tok),
            "minted token must pass is_grant_token: {tok}"
        );
    }

    #[test]
    fn decide_rejects_unsafe_slug() {
        let csrf = GrantCsrf::test();
        assert_eq!(
            decide_grant_post("/grants/create", "bad slug", "", "", &csrf),
            GrantDecision::InvalidSlug
        );
        assert_eq!(
            decide_grant_post("/grants/deactivate", "bad slug", "", TEST_TOKEN, &csrf),
            GrantDecision::InvalidSlug
        );
    }

    #[test]
    fn decide_rejects_bad_csrf_on_create() {
        let csrf = GrantCsrf::test();
        assert_eq!(
            decide_grant_post("/grants/create", "okinawa-2026", "wrong", "", &csrf),
            GrantDecision::BadCsrf
        );
        let good = csrf.create("okinawa-2026");
        assert_eq!(
            decide_grant_post("/grants/create", "okinawa-2026", &good, "", &csrf),
            GrantDecision::PlanCheckThenCreate {
                plan: "okinawa-2026".to_string()
            }
        );
    }

    #[test]
    fn decide_rejects_bad_token_format_on_deactivate() {
        let csrf = GrantCsrf::test();
        // Token format is checked before CSRF (matches source order).
        assert_eq!(
            decide_grant_post("/grants/deactivate", "okinawa-2026", "", "short", &csrf),
            GrantDecision::InvalidToken
        );
    }

    #[test]
    fn decide_rejects_bad_csrf_on_deactivate() {
        let csrf = GrantCsrf::test();
        assert_eq!(
            decide_grant_post(
                "/grants/deactivate",
                "okinawa-2026",
                "wrong",
                TEST_TOKEN,
                &csrf
            ),
            GrantDecision::BadCsrf
        );
        let good = csrf.deactivate("okinawa-2026", TEST_TOKEN);
        assert_eq!(
            decide_grant_post(
                "/grants/deactivate",
                "okinawa-2026",
                &good,
                TEST_TOKEN,
                &csrf
            ),
            GrantDecision::Deactivate {
                plan: "okinawa-2026".to_string(),
                token: TEST_TOKEN.to_string()
            }
        );
    }

    #[test]
    fn decide_unknown_path_is_not_found() {
        let csrf = GrantCsrf::test();
        assert_eq!(
            decide_grant_post("/grants/wat", "okinawa-2026", "", "", &csrf),
            GrantDecision::NotFound
        );
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
