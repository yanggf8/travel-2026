//! 301 redirect worker: reclaims the retired `trip-dashboard.yanggf.workers.dev` URL and
//! forwards everything (path + query preserved) to the live -rs worker, so old share links
//! (`?plan=…&token=…`) keep landing. No secrets, no Turso, no OAuth — just a redirect.
//!
//! Ported from the original 17-line TS worker (all-Rust audit, 2026-08-29).
//! Deploy: `npx wrangler deploy` (worker name stays `trip-dashboard`).

use worker::*;

const TARGET_HOST: &str = "trip-dashboard-rs.yanggf.workers.dev";

#[event(fetch)]
pub async fn main(req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    let mut url = req.url()?;
    if let Err(e) = url.set_host(Some(TARGET_HOST)) {
        return Response::error(&format!("redirect host rewrite failed: {e}"), 500);
    }
    Response::redirect_with_status(url, 301)
}
