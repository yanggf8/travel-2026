//! Trip dashboard worker — entry point. Dispatches to `router::handle`.
//!
//! === WORKING OUTBOUND-POST SIGNATURE (worker 0.8.4) — used in turso.rs ===
//!
//! RequestInit's builder methods (`with_method`/`with_headers`/`with_body`) take
//! `&mut self` and return `&mut Self`, so the RequestInit MUST be a `let mut`
//! binding first — you cannot chain off `RequestInit::default()` inline and then
//! borrow it for `Request::new_with_init` (E0716: temporary dropped while borrowed).
//!
//! Body is set via `.with_body(Some(JsValue::from_str(&s)))` where `s: String`
//! (use `worker::wasm_bindgen::JsValue`). Headers are built separately and moved in.

use worker::*;

mod auth;
mod i18n;
mod model;
mod render;
mod router;
mod styles;
mod turso;

#[event(fetch)]
pub async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    router::handle(req, env).await
}
