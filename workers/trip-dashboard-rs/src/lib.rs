use worker::*;

mod auth;
mod model;
mod turso;

// === WORKING OUTBOUND-POST SIGNATURE (worker 0.8.4) — Task 1 must copy verbatim ===
//
// RequestInit's builder methods (`with_method`/`with_headers`/`with_body`) take
// `&mut self` and return `&mut Self`, so the RequestInit MUST be a `let mut`
// binding first — you cannot chain off `RequestInit::default()` inline and then
// borrow it for `Request::new_with_init` (E0716: temporary dropped while borrowed).
//
// Body is set via `.with_body(Some(JsValue::from_str(&s)))` where `s: String`
// (use `worker::wasm_bindgen::JsValue`). Headers are built separately and moved in.
//
//   let headers = Headers::new();          // NB: Headers::set takes &self in 0.8
//   headers.set("Content-Type", "application/json")?;
//   let mut init = RequestInit::new();
//   init.with_method(Method::Post)
//       .with_headers(headers)
//       .with_body(Some(worker::wasm_bindgen::JsValue::from_str(&payload_string)));
//   let req = Request::new_with_init(url, &init)?;
//   let mut res = Fetch::Request(req).send().await?;
//   let json: serde_json::Value = res.json().await?;
//
// ===============================================================================

#[event(fetch)]
pub async fn main(_req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    let body = serde_json::json!({ "ping": true });
    let headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(worker::wasm_bindgen::JsValue::from_str(
            &serde_json::to_string(&body)?,
        )));
    let req = Request::new_with_init("https://httpbin.org/post", &init)?;
    let mut res = Fetch::Request(req).send().await?;
    let json: serde_json::Value = res.json().await?;
    Response::from_json(&json)
}
