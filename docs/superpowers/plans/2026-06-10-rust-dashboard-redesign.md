# Rust Dashboard Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a greenfield Rust/WASM (`workers-rs`) Cloudflare Worker that replaces the TypeScript `trip-dashboard` with a redesigned, mobile-first, read-only, token-scoped trip view (session-block timeline + keyless 3-level maps), reading from Turso over HTTP.

**Architecture:** Pure SSR, zero client-side JS. Request → router → auth (token→scope) → Turso HTTP `/v2/pipeline` → typed model → focused `render/*` modules → HTML string. Map images are chromeport-snapshotted PNGs served from an R2 bucket; per-stop links are keyless `google.com/maps?q=lat,lon`. Built as isolated, unit-testable modules.

**Tech Stack:** Rust (`wasm32-unknown-unknown`), `worker` crate 0.8.x, `serde`/`serde_json`, `url`; Cloudflare Workers + R2 + wrangler; Turso HTTP pipeline API.

**Spec:** `docs/superpowers/specs/2026-06-10-rust-dashboard-redesign-design.md` — read it before starting.

**Reference (port faithfully, don't reuse):** `workers/trip-dashboard/src/turso.ts` (pipeline shape), `render.ts` (what to render — but redesign the HTML), `styles.ts` (visual baseline).

---

## File Structure

```
workers/trip-dashboard-rs/
├── Cargo.toml
├── wrangler.toml
├── src/
│   ├── lib.rs          # #[event(fetch)] entry → router::handle
│   ├── router.rs       # path match; calls auth before any Turso read
│   ├── auth.rs         # AccessScope { Owner, Plan(slug), Denied } from token
│   ├── turso.rs        # pipeline client + typed Row decode
│   ├── model.rs        # Plan/Day/Session/Activity/Meal/Transfer/Stop structs + assemble()
│   ├── render/
│   │   ├── mod.rs      # page shell (head, css, lang, notranslate)
│   │   ├── index.rs    # plans index (Owner only)
│   │   ├── summary.rs  # trip summary + booking block
│   │   ├── day.rs      # day card: theme, weather, 4 sessions, day map
│   │   ├── session.rs  # session block: activities, meals, transit, session map
│   │   └── map.rs      # <img> (R2 URL) + stop list + per-stop maps links
│   ├── styles.rs       # const CSS via include_str!("styles.css")
│   ├── styles.css
│   └── i18n.rs         # zh/en strings + weather translation
└── tests/              # (logic unit tests live inline with #[cfg(test)] per module)
```

Note: workers-rs unit tests run as native `cargo test` (the pure functions — auth, model assembly, render — don't touch the Worker runtime, so they test on the host target). Only end-to-end behavior needs `wrangler dev`.

---

## Task 0: Spike — verify the outbound-POST pattern compiles & runs

The `RequestInit` + `BodyInit::from(String)` outbound-POST pattern was not confirmed against a live example. Verify it before building the Turso client on top of it.

**Files:**
- Create: `workers/trip-dashboard-rs/Cargo.toml`
- Create: `workers/trip-dashboard-rs/wrangler.toml`
- Create: `workers/trip-dashboard-rs/src/lib.rs`

- [ ] **Step 1: Scaffold Cargo.toml**

```toml
[package]
name = "trip-dashboard-rs"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
worker = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
url = "2"

[profile.release]
opt-level = "s"
lto = true
```

- [ ] **Step 2: Scaffold wrangler.toml**

```toml
name = "trip-dashboard-rs"
main = "build/worker/shim.mjs"
compatibility_date = "2024-12-01"

[build]
command = "cargo install -q worker-build && worker-build --release"

[[r2_buckets]]
binding = "MAPS"
bucket_name = "trip-dashboard-maps"
```

- [ ] **Step 3: Write a minimal handler that does the outbound POST**

`src/lib.rs`:
```rust
use worker::*;

#[event(fetch)]
pub async fn main(_req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    // Spike: prove the outbound POST + JSON parse pattern compiles and runs.
    let body = serde_json::json!({ "ping": true });
    let mut headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    let init = RequestInit::default()
        .with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(worker::wasm_bindgen::JsValue::from_str(
            &serde_json::to_string(&body)?,
        )));
    let req = Request::new_with_init("https://httpbin.org/post", &init)?;
    let mut res = Fetch::Request(req).send().await?;
    let json: serde_json::Value = res.json().await?;
    Response::from_json(&json)
}
```

- [ ] **Step 4: Build to verify it compiles for wasm**

Run: `cd workers/trip-dashboard-rs && cargo install -q worker-build && worker-build --release`
Expected: builds without error, produces `build/worker/`. If `with_body`/`BodyInit` signature differs from above, fix per the compiler error — the goal of this spike is to lock the EXACT working signature before Task 3 depends on it. Record the working signature in a comment.

- [ ] **Step 5: Smoke-test live**

Run: `unset CLOUDFLARE_API_TOKEN && npx wrangler dev` then `curl http://localhost:8787/`
Expected: JSON echo from httpbin (proves outbound POST + JSON parse works end to end).

- [ ] **Step 6: Commit**

```bash
git add workers/trip-dashboard-rs/
git commit -m "spike(dashboard-rs): scaffold workers-rs + verify outbound POST pattern"
```

---

## Task 1: Turso pipeline client + typed Row decode

Port the proven `/v2/pipeline` shape from `turso.ts`. Pure logic (decode) is unit-tested on the host; the HTTP call is exercised in Task 9 integration.

**Files:**
- Create: `workers/trip-dashboard-rs/src/turso.rs`
- Modify: `workers/trip-dashboard-rs/src/lib.rs` (add `mod turso;`)

- [ ] **Step 1: Write the failing test for row decoding**

`src/turso.rs`:
```rust
//! Turso HTTP pipeline client. Ports the request/response shape of
//! workers/trip-dashboard/src/turso.ts (queryTursoPipeline + rowsToObjects).

use serde_json::Value;
use std::collections::BTreeMap;

/// One decoded row: column name -> scalar value (as serde_json::Value; Null for SQL null).
pub type Row = BTreeMap<String, Value>;

/// Decode one Turso pipeline result object (`{cols:[{name}], rows:[[{type,value}]]}`)
/// into a Vec<Row>. Mirrors rowsToObjects in turso.ts.
pub fn decode_result(result: &Value) -> Vec<Row> {
    let cols: Vec<String> = result
        .get("cols")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .map(|c| c.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    let rows = result.get("rows").and_then(|r| r.as_array()).cloned().unwrap_or_default();
    rows.into_iter()
        .map(|row| {
            let cells = row.as_array().cloned().unwrap_or_default();
            let mut obj = Row::new();
            for (i, name) in cols.iter().enumerate() {
                // Turso encodes each cell as {"type":"text"|"integer"|"null"|..., "value": "..."}
                let v = cells
                    .get(i)
                    .and_then(|cell| cell.get("value"))
                    .cloned()
                    .unwrap_or(Value::Null);
                obj.insert(name.clone(), v);
            }
            obj
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_cols_and_rows() {
        let result = json!({
            "cols": [{"name": "plan_id"}, {"name": "days"}],
            "rows": [
                [{"type":"text","value":"okinawa-2026"}, {"type":"integer","value":"5"}],
                [{"type":"text","value":"tokyo-2026"}, {"type":"null","value": null}]
            ]
        });
        let rows = decode_result(&result);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["plan_id"], json!("okinawa-2026"));
        assert_eq!(rows[0]["days"], json!("5"));
        assert_eq!(rows[1]["days"], Value::Null);
    }
}
```

- [ ] **Step 2: Run the test, verify it passes**

Run: `cd workers/trip-dashboard-rs && cargo test decodes_cols_and_rows`
Expected: PASS (this is pure logic; no wasm needed). Add `mod turso;` to `lib.rs` first if the test runner can't find it.

- [ ] **Step 3: Add the pipeline HTTP function (not unit-tested here; used in Task 9)**

Append to `src/turso.rs`:
```rust
use worker::*;

/// POST N SQL statements to Turso /v2/pipeline; return one Vec<Row> per statement.
/// turso_url is the libsql:// URL; converted to https + /v2/pipeline.
pub async fn pipeline(turso_url: &str, token: &str, sqls: &[String]) -> Result<Vec<Vec<Row>>> {
    let url = turso_url.replace("libsql://", "https://") + "/v2/pipeline";
    let mut requests: Vec<Value> = sqls
        .iter()
        .map(|sql| serde_json::json!({ "type": "execute", "stmt": { "sql": sql } }))
        .collect();
    requests.push(serde_json::json!({ "type": "close" }));
    let body = serde_json::json!({ "requests": requests });

    let mut headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {token}"))?;
    headers.set("Content-Type", "application/json")?;
    let init = RequestInit::default()
        .with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(wasm_bindgen::JsValue::from_str(&serde_json::to_string(&body)?)));
    let req = Request::new_with_init(&url, &init)?;
    let mut res = Fetch::Request(req).send().await?;
    if res.status_code() >= 400 {
        let t = res.text().await.unwrap_or_default();
        return Err(Error::RustError(format!("Turso HTTP {}: {t}", res.status_code())));
    }
    let json: Value = res.json().await?;
    let mut out = Vec::with_capacity(sqls.len());
    for (i, _) in sqls.iter().enumerate() {
        let entry = json.get("results").and_then(|r| r.get(i));
        if let Some(err) = entry.and_then(|e| e.get("response")).and_then(|r| r.get("error")) {
            return Err(Error::RustError(format!("Turso query {i} error: {err}")));
        }
        let result = entry
            .and_then(|e| e.get("response"))
            .and_then(|r| r.get("result"));
        out.push(result.map(decode_result).unwrap_or_default());
    }
    Ok(out)
}
```
**Note:** if Task 0's spike found a different working `with_body` signature, use THAT here.

- [ ] **Step 4: Build to confirm wasm compiles**

Run: `worker-build --release`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add workers/trip-dashboard-rs/src/turso.rs workers/trip-dashboard-rs/src/lib.rs
git commit -m "feat(dashboard-rs): Turso pipeline client + row decode (ported from turso.ts)"
```

---

## Task 2: Auth — AccessScope resolution

Token → scope. Pure logic, fully unit-tested. Owner token and per-plan tokens come from Turso (`plan_share_tokens`) + an owner secret.

**Files:**
- Create: `workers/trip-dashboard-rs/src/auth.rs`
- Modify: `src/lib.rs` (`mod auth;`)

- [ ] **Step 1: Write failing tests**

`src/auth.rs`:
```rust
//! Access scoping. A request carries an optional token (query param `token` or
//! the owner secret). Owner sees everything; a per-plan token sees exactly one plan.

use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq)]
pub enum AccessScope {
    Owner,
    Plan(String), // plan slug, e.g. "okinawa-2026"
    Denied,
}

/// Resolve scope. `token` is the value from `?token=`; `owner_token` is the secret;
/// `share_tokens` maps token -> plan_id (loaded from plan_share_tokens).
pub fn resolve(token: Option<&str>, owner_token: &str, share_tokens: &HashMap<String, String>) -> AccessScope {
    match token {
        Some(t) if !owner_token.is_empty() && t == owner_token => AccessScope::Owner,
        Some(t) => match share_tokens.get(t) {
            Some(plan) => AccessScope::Plan(plan.clone()),
            None => AccessScope::Denied,
        },
        None => AccessScope::Denied,
    }
}

/// Can this scope view the given plan slug?
pub fn can_view_plan(scope: &AccessScope, slug: &str) -> bool {
    match scope {
        AccessScope::Owner => true,
        AccessScope::Plan(p) => p == slug,
        AccessScope::Denied => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shares() -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("share-oki-abc".into(), "okinawa-2026".into());
        m
    }

    #[test]
    fn owner_token_is_owner() {
        assert_eq!(resolve(Some("OWNER"), "OWNER", &shares()), AccessScope::Owner);
    }
    #[test]
    fn share_token_scopes_to_one_plan() {
        assert_eq!(resolve(Some("share-oki-abc"), "OWNER", &shares()), AccessScope::Plan("okinawa-2026".into()));
    }
    #[test]
    fn unknown_token_denied() {
        assert_eq!(resolve(Some("nope"), "OWNER", &shares()), AccessScope::Denied);
    }
    #[test]
    fn no_token_denied() {
        assert_eq!(resolve(None, "OWNER", &shares()), AccessScope::Denied);
    }
    #[test]
    fn plan_scope_cannot_view_other_plan() {
        let s = AccessScope::Plan("okinawa-2026".into());
        assert!(can_view_plan(&s, "okinawa-2026"));
        assert!(!can_view_plan(&s, "tokyo-2026"));
    }
    #[test]
    fn owner_views_any() {
        assert!(can_view_plan(&AccessScope::Owner, "anything"));
    }
}
```

- [ ] **Step 2: Run tests, verify they pass**

Run: `cargo test --lib auth`
Expected: 6 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add workers/trip-dashboard-rs/src/auth.rs workers/trip-dashboard-rs/src/lib.rs
git commit -m "feat(dashboard-rs): AccessScope token resolution (owner/plan/denied)"
```

---

## Task 3: `plan_share_tokens` table + CLI command to mint tokens

The worker reads share tokens; the **CLI** writes them (CLI is the write path). Add to the Rust CLI.

**Files:**
- Modify: `rust/crates/travel-cli/src/db_migrate.rs` (add table DDL)
- Create: `rust/crates/travel-cli/src/share_token.rs`
- Modify: `rust/crates/travel-cli/src/main.rs` (dispatch `share-token`)
- Test: `rust/crates/travel-cli/tests/share_token.rs`

- [ ] **Step 1: Add the table DDL to db_migrate**

In `db_migrate.rs`, alongside the other `CREATE TABLE IF NOT EXISTS` statements, add:
```rust
conn.execute(
    "CREATE TABLE IF NOT EXISTS plan_share_tokens (
        plan_id TEXT NOT NULL,
        token TEXT NOT NULL PRIMARY KEY,
        created_at TEXT NOT NULL
    )",
    (),
).await?;
```

- [ ] **Step 2: Write the failing integration test**

`rust/crates/travel-cli/tests/share_token.rs` (follow the existing real-Turso test pattern — seed plan, run binary, SELECT, assert, teardown; skip if creds absent). Mirror an existing test file's harness exactly. Core assertion:
```rust
// after running: ./bin/travel share-token okinawa-2026
// a row exists in plan_share_tokens for plan_id='okinawa-2026' with a non-empty token,
// and the command prints the token (a shareable URL fragment).
```
Copy the setup/teardown boilerplate from `rust/crates/travel-cli/tests/set_mutation_bugs.rs`.

- [ ] **Step 3: Run the test, verify it FAILS**

Run: `cd rust && cargo test -p travel-cli --test share_token`
Expected: FAIL (command not implemented).

- [ ] **Step 4: Implement the command**

`rust/crates/travel-cli/src/share_token.rs`:
```rust
// `travel share-token <plan_id>` — mint (or show) a per-plan view-scope token.
// Token is an opaque random string; stored in plan_share_tokens. Plain-text output.
use crate::db;

pub async fn run(args: &[String], plan_id: String) -> Result<(), String> {
    let target = args.first().cloned().unwrap_or(plan_id);
    let conn = db::connect_write().await.map_err(|e| e.to_string())?;
    // Deterministic-enough random token without Date/rand: derive from a UUID-like
    // source available in the CLI (the CLI is native, so use the `uuid` crate if present,
    // else a hex of getrandom). Check Cargo.toml for an existing rng dep and reuse it.
    let token = crate::share_token::gen_token();
    conn.execute(
        "INSERT INTO plan_share_tokens (plan_id, token, created_at) VALUES (?1, ?2, datetime('now'))",
        libsql::params![target.clone(), token.clone()],
    ).await.map_err(|e| e.to_string())?;
    println!("share token for {target}: {token}");
    println!("url: https://<dashboard-host>/?plan={target}&token={token}");
    Ok(())
}

pub fn gen_token() -> String {
    // Use the same RNG approach the CLI already uses elsewhere (grep for `uuid` / `getrandom`
    // / `rand` in rust/crates/travel-cli/Cargo.toml). Produce a 32-hex-char opaque token.
    // If none exists, add `getrandom = "0.2"` and hex-encode 16 bytes.
    use std::fmt::Write;
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).expect("rng");
    let mut s = String::with_capacity(32);
    for b in buf { let _ = write!(s, "{b:02x}"); }
    s
}
```
Wire dispatch in `main.rs`:
```rust
[cmd, rest @ ..] if cmd == "share-token" => {
    let plan_id = /* resolve as other commands do */;
    share_token::run(rest, plan_id).await?;
}
```
Add `mod share_token;` and (if needed) `getrandom = "0.2"` to the CLI's Cargo.toml.

- [ ] **Step 5: Run the test, verify it PASSES**

Run: `cd rust && cargo test -p travel-cli --test share_token`
Expected: PASS.

- [ ] **Step 6: Run migrate + mint a real token for okinawa-2026**

Run: `./bin/travel db migrate && ./bin/travel share-token okinawa-2026`
Expected: prints a token + url; row appears in `plan_share_tokens`.

- [ ] **Step 7: Commit**

```bash
git add rust/crates/travel-cli/ 
git commit -m "feat(cli): plan_share_tokens table + share-token command (dashboard view scope)"
```

---

## Task 4: Model structs + assemble()

Typed plan model assembled from decoded rows. Pure logic, unit-tested.

**Files:**
- Create: `workers/trip-dashboard-rs/src/model.rs`
- Modify: `src/lib.rs` (`mod model;`)

- [ ] **Step 1: Write failing test for session-meal assembly (the noon bug, structurally fixed)**

`src/model.rs`:
```rust
//! Typed plan model + assembly from decoded Turso rows.
use crate::turso::Row;
use serde_json::Value;

#[derive(Debug, Default, PartialEq)]
pub struct Stop { pub title: String, pub address: String, pub lat: Option<f64>, pub lon: Option<f64>, pub maps_link: String }

#[derive(Debug, Default, PartialEq)]
pub struct Session {
    pub session_type: String, // morning|noon|afternoon|evening
    pub focus_zh: String,
    pub transit_zh: String,
    pub activities: Vec<String>,
    pub meals: Vec<String>,
    pub stops: Vec<Stop>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Day {
    pub day_number: i64,
    pub date: String,
    pub day_type: String,
    pub theme: String,
    pub theme_zh: String,
    pub weather_label: String,
    pub sessions: Vec<Session>, // ALWAYS 4: morning, noon, afternoon, evening
}

/// The canonical 4 sessions, in display order. This is what makes "noon" impossible to drop.
pub const SESSION_ORDER: [&str; 4] = ["morning", "noon", "afternoon", "evening"];

fn s(row: &Row, key: &str) -> String {
    row.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}
fn i(row: &Row, key: &str) -> i64 {
    row.get(key).and_then(|v| v.as_str()).and_then(|s| s.parse().ok())
        .or_else(|| row.get(key).and_then(|v| v.as_i64())).unwrap_or(0)
}

/// Build the 4 sessions for one day from activity + meal rows already filtered to that day.
pub fn build_sessions(activities: &[Row], meals: &[Row]) -> Vec<Session> {
    SESSION_ORDER.iter().map(|&st| {
        Session {
            session_type: st.to_string(),
            activities: activities.iter().filter(|r| s(r, "session_type") == st)
                .map(|r| s(r, "title")).collect(),
            meals: meals.iter().filter(|r| s(r, "session_type") == st)
                .map(|r| s(r, "meal")).collect(),
            ..Default::default()
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn row(pairs: &[(&str, Value)]) -> Row {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn noon_meal_is_not_dropped() {
        let acts = vec![row(&[("session_type", json!("noon")), ("title", json!("Makishi Market"))])];
        let meals = vec![row(&[("session_type", json!("noon")), ("meal", json!("Lunch: Makishi"))])];
        let sessions = build_sessions(&acts, &meals);
        assert_eq!(sessions.len(), 4);
        let noon = sessions.iter().find(|s| s.session_type == "noon").unwrap();
        assert_eq!(noon.activities, vec!["Makishi Market".to_string()]);
        assert_eq!(noon.meals, vec!["Lunch: Makishi".to_string()]);
    }

    #[test]
    fn always_four_sessions_in_order() {
        let sessions = build_sessions(&[], &[]);
        let order: Vec<_> = sessions.iter().map(|s| s.session_type.as_str()).collect();
        assert_eq!(order, vec!["morning", "noon", "afternoon", "evening"]);
    }
}
```

- [ ] **Step 2: Run tests, verify PASS**

Run: `cargo test --lib model`
Expected: 2 PASS.

- [ ] **Step 3: Add the top-level Plan struct + a coarse assemble() signature**

Append:
```rust
#[derive(Debug, Default)]
pub struct Plan {
    pub plan_id: String,
    pub display_name: String,
    pub start_date: String,
    pub end_date: String,
    pub days: Vec<Day>,
    pub flights: Vec<Row>,      // rendered as-is in summary for now
    pub hotel: Option<Row>,
    pub transfers: Vec<Row>,
}

/// Assemble a Plan from the pipeline result vectors. The query order is defined in turso.rs
/// (Task 8). Each argument is the decoded rows for one query.
pub fn assemble(
    plan_rows: &[Row], day_rows: &[Row], session_rows: &[Row],
    activity_rows: &[Row], meal_rows: &[Row], flight_rows: &[Row],
    hotel_rows: &[Row], transfer_rows: &[Row], poi_rows: &[Row],
) -> Plan {
    let mut plan = Plan::default();
    if let Some(p) = plan_rows.first() {
        plan.plan_id = s(p, "plan_id");
        plan.display_name = s(p, "display_name");
        plan.start_date = s(p, "start_date");
        plan.end_date = s(p, "end_date");
    }
    plan.flights = flight_rows.to_vec();
    plan.hotel = hotel_rows.first().cloned();
    plan.transfers = transfer_rows.to_vec();
    for d in day_rows {
        let dn = i(d, "day_number");
        let acts: Vec<Row> = activity_rows.iter().filter(|r| i(r, "day_number") == dn).cloned().collect();
        let mls: Vec<Row> = meal_rows.iter().filter(|r| i(r, "day_number") == dn).cloned().collect();
        let mut sessions = build_sessions(&acts, &mls);
        merge_session_meta(&mut sessions, session_rows, dn); // focus_zh, transit_zh
        attach_stops(&mut sessions, &acts, poi_rows);        // lat/lon + maps link
        plan.days.push(Day {
            day_number: dn, date: s(d, "date"), day_type: s(d, "day_type"),
            theme: s(d, "theme"), theme_zh: s(d, "theme_zh"),
            weather_label: s(d, "weather_label"), sessions,
        });
    }
    plan
}

fn merge_session_meta(sessions: &mut [Session], session_rows: &[Row], day_number: i64) {
    for sess in sessions.iter_mut() {
        if let Some(r) = session_rows.iter().find(|r| i(r, "day_number") == day_number && s(r, "session_type") == sess.session_type) {
            sess.focus_zh = s(r, "focus_zh");
            sess.transit_zh = s(r, "transit_notes_zh");
        }
    }
}

fn attach_stops(sessions: &mut [Session], acts: &[Row], poi_rows: &[Row]) {
    for sess in sessions.iter_mut() {
        for a in acts.iter().filter(|r| s(r, "session_type") == sess.session_type) {
            let title = s(a, "title");
            // match POI by title to pull lat/lon/address
            let poi = poi_rows.iter().find(|p| s(p, "title") == title);
            let lat = poi.and_then(|p| p.get("lat")).and_then(json_f64);
            let lon = poi.and_then(|p| p.get("lon")).and_then(json_f64);
            let maps_link = match (lat, lon) {
                (Some(la), Some(lo)) => format!("https://www.google.com/maps?q={la},{lo}"),
                _ => format!("https://www.google.com/maps/search/{}", urlencode(&title)),
            };
            sess.stops.push(Stop {
                title,
                address: poi.map(|p| s(p, "address")).unwrap_or_default(),
                lat, lon, maps_link,
            });
        }
    }
}

fn json_f64(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}
fn urlencode(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z'|b'a'..=b'z'|b'0'..=b'9'|b'-'|b'_'|b'.'|b'~' => (b as char).to_string(),
        b' ' => "+".to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}
```

- [ ] **Step 4: Write a test for stop/maps-link assembly**

```rust
#[test]
fn stop_gets_maps_link_from_poi_latlon() {
    let acts = vec![row(&[("session_type", json!("morning")), ("title", json!("Naminoue Shrine"))])];
    let pois = vec![row(&[("title", json!("Naminoue Shrine")), ("lat", json!("26.2156")), ("lon", json!("127.6691")), ("address", json!("Naha"))])];
    let mut sessions = build_sessions(&acts, &[]);
    super::attach_stops(&mut sessions, &acts, &pois);
    let m = &sessions.iter().find(|s| s.session_type=="morning").unwrap().stops[0];
    assert_eq!(m.maps_link, "https://www.google.com/maps?q=26.2156,127.6691");
    assert_eq!(m.address, "Naha");
}
```

- [ ] **Step 5: Run tests, verify PASS**

Run: `cargo test --lib model`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add workers/trip-dashboard-rs/src/model.rs workers/trip-dashboard-rs/src/lib.rs
git commit -m "feat(dashboard-rs): typed Plan model + assemble (4-session, noon-safe, POI stops)"
```

---

## Task 5: Seed `destination_pois.lat/lon` for Okinawa (sourced)

Maps need coordinates. Add columns + seed sourced values via chromeport (no fabrication).

**Files:**
- Modify: `rust/crates/travel-cli/src/db_migrate.rs` (ALTER/ensure columns)
- One-shot: `./bin/travel db exec` seeds (this is a one-shot data backfill, direct SQL acceptable)

- [ ] **Step 1: Ensure lat/lon columns exist**

In `db_migrate.rs`, after the `destination_pois` create/ensure block, add idempotent column adds (SQLite ignores duplicates only if guarded — use a check or accept error):
```rust
// best-effort add; ignore "duplicate column" error
let _ = conn.execute("ALTER TABLE destination_pois ADD COLUMN lat REAL", ()).await;
let _ = conn.execute("ALTER TABLE destination_pois ADD COLUMN lon REAL", ()).await;
```
Run: `./bin/travel db migrate`

- [ ] **Step 2: Source coordinates via chromeport (per POI)**

For each Okinawa POI (`./bin/travel db exec "SELECT poi_id, title FROM destination_pois WHERE slug='okinawa_2026'"`), look up its coordinates from a real source (Google Maps place page or a geocode page) via chromeport `fetch url` and read the lat/lon out of the captured text/URL. Record provenance in a note. DO NOT invent coordinates.

- [ ] **Step 3: Backfill the coordinates (one-shot SQL)**

```bash
./bin/travel db exec "UPDATE destination_pois SET lat=<sourced>, lon=<sourced> WHERE slug='okinawa_2026' AND poi_id='<id>'"
# repeat per POI
```

- [ ] **Step 4: Verify all Okinawa POIs have coordinates**

Run: `./bin/travel db exec "SELECT poi_id, lat, lon FROM destination_pois WHERE slug='okinawa_2026' AND (lat IS NULL OR lon IS NULL)"`
Expected: zero rows.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/travel-cli/src/db_migrate.rs
git commit -m "feat(cli): destination_pois lat/lon columns (sourced coords for maps)"
```

---

## Task 6: Render — session & day modules (the timeline)

The redesigned HTML. Pure string functions, unit-tested for the noon/meal/transfer presence that broke before.

**Files:**
- Create: `workers/trip-dashboard-rs/src/render/mod.rs`, `session.rs`, `day.rs`
- Modify: `src/lib.rs` (`mod render;`)

- [ ] **Step 1: HTML-escape helper + failing session test**

`src/render/mod.rs`:
```rust
pub mod session;
pub mod day;
pub mod summary;
pub mod index;
pub mod map;

/// Escape text for HTML. Escape ONCE — never double-escape (the old TS bug rendered `&amp;amp;`).
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escapes_ampersand_once() {
        assert_eq!(esc("Museum & Art"), "Museum &amp; Art");
        // must NOT be &amp;amp;
        assert!(!esc("Museum & Art").contains("amp;amp;"));
    }
}
```

`src/render/session.rs`:
```rust
use crate::model::Session;
use super::esc;

/// Render one session block. Renders nothing-but-the-label when empty (keeps the 4-slot rhythm).
pub fn render(sess: &Session, lang: &str) -> String {
    let label = match (sess.session_type.as_str(), lang) {
        ("morning", "zh") => "上午", ("noon", "zh") => "中午",
        ("afternoon", "zh") => "下午", ("evening", "zh") => "晚上",
        ("morning", _) => "Morning", ("noon", _) => "Noon",
        ("afternoon", _) => "Afternoon", _ => "Evening",
    };
    let mut h = String::new();
    h.push_str(&format!("<div class=\"session session-{}\"><div class=\"session-label\">{}</div>", sess.session_type, label));
    if !sess.focus_zh.is_empty() {
        h.push_str(&format!("<div class=\"session-focus\">{}</div>", esc(&sess.focus_zh)));
    }
    for a in &sess.activities {
        h.push_str(&format!("<div class=\"activity\">{}</div>", esc(a)));
    }
    for m in &sess.meals {
        h.push_str(&format!("<div class=\"meal\">🍽️ {}</div>", esc(m)));
    }
    if !sess.transit_zh.is_empty() {
        h.push_str(&format!("<div class=\"transit\">🚃 {}</div>", esc(&sess.transit_zh)));
    }
    h.push_str("</div>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Session;
    #[test]
    fn noon_meal_renders() {
        let sess = Session { session_type: "noon".into(), meals: vec!["Lunch: Makishi".into()], ..Default::default() };
        let html = render(&sess, "zh");
        assert!(html.contains("中午"));
        assert!(html.contains("Lunch: Makishi"));
    }
}
```

- [ ] **Step 2: Run tests, verify PASS**

Run: `cargo test --lib render`
Expected: PASS (escape + noon meal).

- [ ] **Step 3: Day module renders theme + weather + 4 sessions + day map slot**

`src/render/day.rs`:
```rust
use crate::model::Day;
use super::{esc, session, map};

pub fn render(day: &Day, plan_id: &str, lang: &str) -> String {
    let theme = if lang == "zh" && !day.theme_zh.is_empty() { &day.theme_zh } else { &day.theme };
    let mut h = String::new();
    h.push_str(&format!("<section class=\"day day-{}\">", esc(&day.day_type)));
    h.push_str(&format!("<h2>Day {} · {}</h2>", day.day_number, esc(&day.date)));
    h.push_str(&format!("<div class=\"theme\">{}</div>", esc(theme)));
    if !day.weather_label.is_empty() {
        h.push_str(&format!("<div class=\"weather\">🌧️ {}</div>", esc(&day.weather_label)));
    }
    h.push_str(&map::day_map_img(plan_id, day.day_number)); // <img> to R2 (Task 7)
    for sess in &day.sessions {
        // skip wholly-empty sessions to avoid 4 empty boxes on a light day
        if sess.activities.is_empty() && sess.meals.is_empty() && sess.focus_zh.is_empty() { continue; }
        h.push_str(&session::render(sess, lang));
    }
    h.push_str("</section>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Day, Session};
    #[test]
    fn renders_zh_theme_and_sessions() {
        let day = Day {
            day_number: 3, date: "2026-06-14".into(), day_type: "full".into(),
            theme_zh: "壺屋陶器街".into(),
            sessions: vec![Session{ session_type:"noon".into(), meals:vec!["Lunch".into()], ..Default::default()}],
            ..Default::default()
        };
        let html = render(&day, "okinawa-2026", "zh");
        assert!(html.contains("壺屋陶器街"));
        assert!(html.contains("中午"));
        assert!(html.contains("Lunch"));
    }
}
```

- [ ] **Step 4: Run tests, verify PASS** — `cargo test --lib render`. Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add workers/trip-dashboard-rs/src/render/
git commit -m "feat(dashboard-rs): session+day render (noon-safe, escape-once)"
```

---

## Task 7: Render — map module (R2 image + stop list)

**Files:**
- Create: `workers/trip-dashboard-rs/src/render/map.rs`

- [ ] **Step 1: map image + stop list with per-stop maps links**

`src/render/map.rs`:
```rust
use crate::model::{Stop};
use super::esc;

/// <img> pointing at the R2-served map for this day. The image key convention:
/// maps/<plan_id>/day-<n>.png  (plan-level = maps/<plan_id>/plan.png).
/// Served by the worker's /map/* route (Task 8) from the MAPS bucket.
pub fn day_map_img(plan_id: &str, day_number: i64) -> String {
    format!("<img class=\"daymap\" loading=\"lazy\" alt=\"Day {day_number} map\" src=\"/map/{}/day-{}.png\">", esc(plan_id), day_number)
}
pub fn plan_map_img(plan_id: &str) -> String {
    format!("<img class=\"planmap\" loading=\"lazy\" alt=\"Trip map\" src=\"/map/{}/plan.png\">", esc(plan_id))
}

/// A list of stops with their Google Maps links (keyless q=lat,lon).
pub fn stop_list(stops: &[Stop]) -> String {
    if stops.is_empty() { return String::new(); }
    let mut h = String::from("<ul class=\"stoplist\">");
    for s in stops {
        h.push_str(&format!(
            "<li><a href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a>{}</li>",
            esc(&s.maps_link), esc(&s.title),
            if s.address.is_empty() { String::new() } else { format!("<span class=\"addr\">{}</span>", esc(&s.address)) }
        ));
    }
    h.push_str("</ul>");
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Stop;
    #[test]
    fn stop_list_links_to_maps() {
        let stops = vec![Stop{ title:"Naminoue".into(), maps_link:"https://www.google.com/maps?q=26.2,127.6".into(), ..Default::default()}];
        let h = stop_list(&stops);
        assert!(h.contains("q=26.2,127.6"));
        assert!(h.contains("Naminoue"));
    }
    #[test]
    fn day_map_points_at_r2_route() {
        assert!(day_map_img("okinawa-2026", 2).contains("/map/okinawa-2026/day-2.png"));
    }
}
```

- [ ] **Step 2: Run tests, verify PASS** — `cargo test --lib render::map`. Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add workers/trip-dashboard-rs/src/render/map.rs
git commit -m "feat(dashboard-rs): map render — R2 <img> + keyless per-stop maps links"
```

---

## Task 8: Summary + index render, page shell, styles, i18n

**Files:**
- Create: `src/render/summary.rs`, `src/render/index.rs`, `src/styles.rs`, `src/styles.css`, `src/i18n.rs`
- Modify: `src/render/mod.rs` (page() shell)

- [ ] **Step 1: Page shell + summary (booking block with progressive disclosure)**

`src/render/summary.rs` — render flights/hotel/transfers from `Plan` (each transfer shows title/route/duration/price; flight shows number/route/time; PNR & hotel CFM wrapped in `<details>`). Include a unit test asserting a transfer's route+price appear (the old TS "—" bug):
```rust
#[test]
fn transfer_renders_route_and_price() {
    // build a Plan with one transfer row having selected_route/selected_price_yen,
    // assert the rendered summary contains the route text and the yen figure.
}
```
(Write the full render fn + test; mirror the field names from `airport_transfers`: `selected_title`, `selected_route`, `selected_duration_min`, `selected_price_yen`.)

- [ ] **Step 2: Page shell in `mod.rs`**

```rust
use crate::model::Plan;
pub fn page(title: &str, body: &str, lang: &str) -> String {
    format!(
        "<!doctype html><html lang=\"{lang_attr}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"google\" content=\"notranslate\">\
         <title>{}</title><style>{}</style></head><body>{}</body></html>",
        esc(title), crate::styles::CSS, body,
        lang_attr = if lang == "en" { "en" } else { "zh-TW" },
    )
}
pub fn render_plan(plan: &Plan, lang: &str) -> String {
    let mut body = String::new();
    body.push_str(&summary::render(plan, lang));
    body.push_str(&map::plan_map_img(&plan.plan_id));
    for d in &plan.days { body.push_str(&day::render(d, &plan.plan_id, lang)); }
    page(&plan.display_name, &body, lang)
}
```

- [ ] **Step 3: styles.rs + styles.css (mobile-first RWD)**

`src/styles.rs`:
```rust
pub const CSS: &str = include_str!("styles.css");
```
`src/styles.css`: port the visual baseline from `workers/trip-dashboard/src/styles.ts` (it's clean), then add the RWD two-column desktop layout (`@media (min-width: 900px)`: summary beside plan map; day timeline beside day map) and day-type left-border accents. Keep mobile single-column.

- [ ] **Step 4: index.rs (owner-only plans list)**

Render a list of plans (id, dates, display name) as links. Used only when scope == Owner.

- [ ] **Step 5: Run all render tests** — `cargo test --lib`. Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add workers/trip-dashboard-rs/src/
git commit -m "feat(dashboard-rs): summary+index render, page shell, RWD styles, i18n"
```

---

## Task 9: Router + wiring + R2 map route + end-to-end

**Files:**
- Modify: `src/lib.rs`, create `src/router.rs`

- [ ] **Step 1: Router with auth gate, plan render, owner index, and /map/* R2 passthrough**

`src/router.rs`:
```rust
use worker::*;
use std::collections::HashMap;
use crate::{auth, turso, model, render};

pub async fn handle(req: Request, env: Env) -> Result<Response> {
    let url = req.url()?;
    let path = url.path().to_string();
    let q: HashMap<_, _> = url.query_pairs().into_owned().collect();

    // /map/<plan>/<file>.png  -> stream from R2 MAPS bucket
    if let Some(rest) = path.strip_prefix("/map/") {
        let bucket = env.bucket("MAPS")?;
        match bucket.get(rest).execute().await? {
            Some(obj) => {
                let bytes = obj.body().ok_or_else(|| Error::RustError("no body".into()))?.bytes().await?;
                let mut h = Headers::new();
                h.set("Content-Type", "image/png")?;
                h.set("Cache-Control", "public, max-age=86400")?;
                return Ok(Response::from_bytes(bytes)?.with_headers(h));
            }
            None => return Ok(Response::error("map not found", 404)?),
        }
    }

    let turso_url = env.secret("TURSO_URL")?.to_string();
    let turso_token = env.secret("TURSO_TOKEN")?.to_string();   // READ token
    let owner_token = env.secret("OWNER_TOKEN")?.to_string();
    let lang = if q.get("lang").map(|s| s.as_str()) == Some("en") { "en" } else { "zh" };

    // load share tokens
    let share_rows = turso::pipeline(&turso_url, &turso_token,
        &["SELECT token, plan_id FROM plan_share_tokens".to_string()]).await?;
    let mut shares = HashMap::new();
    for r in &share_rows[0] {
        if let (Some(t), Some(p)) = (r.get("token").and_then(|v| v.as_str()), r.get("plan_id").and_then(|v| v.as_str())) {
            shares.insert(t.to_string(), p.to_string());
        }
    }
    let scope = auth::resolve(q.get("token").map(|s| s.as_str()), &owner_token, &shares);

    // index (owner only)
    if path == "/" && q.get("plan").is_none() {
        if scope != auth::AccessScope::Owner { return Ok(Response::error("Forbidden", 403)?); }
        let plans = turso::pipeline(&turso_url, &turso_token,
            &["SELECT plan_id FROM plans ORDER BY plan_id".to_string()]).await?;
        return Response::from_html(render::index::render(&plans[0], lang));
    }

    // single plan
    if let Some(slug) = q.get("plan") {
        if !auth::can_view_plan(&scope, slug) { return Ok(Response::error("Forbidden", 403)?); }
        let plan = load_plan(&turso_url, &turso_token, slug).await?;
        return Response::from_html(render::render_plan(&plan, lang));
    }
    Ok(Response::error("Forbidden", 403)?)
}

async fn load_plan(turso_url: &str, token: &str, slug: &str) -> Result<model::Plan> {
    let dest = slug.replace('-', "_");
    let q = |s: String| s;
    let sqls = vec![
        format!("SELECT p.plan_id, d.start_date, d.end_date, pd.display_name FROM plans p JOIN date_anchors d ON d.plan_id=p.plan_id LEFT JOIN plan_destinations pd ON pd.plan_id=p.plan_id WHERE p.plan_id='{slug}'"),
        format!("SELECT day_number, date, day_type, theme, theme_zh, weather_label FROM days WHERE plan_id='{slug}' ORDER BY day_number"),
        format!("SELECT day_number, session_type, focus_zh, transit_notes_zh FROM timesofday WHERE plan_id='{slug}'"),
        format!("SELECT day_number, session_type, title FROM activities WHERE plan_id='{slug}' ORDER BY day_number, sort_order"),
        format!("SELECT day_number, session_type, meal FROM session_meals WHERE plan_id='{slug}' ORDER BY day_number, sort_order"),
        format!("SELECT direction, flight_number, airline, departure_code, departure_terminal, departure_time, arrival_code, arrival_terminal, arrival_time, flight_date FROM flight_legs WHERE plan_id='{slug}' ORDER BY direction"),
        format!("SELECT name, name_zh, check_in, notes FROM hotels WHERE plan_id='{slug}'"),
        format!("SELECT direction, selected_title, selected_route, selected_duration_min, selected_price_yen FROM airport_transfers WHERE plan_id='{slug}'"),
        format!("SELECT title, lat, lon, address FROM destination_pois WHERE slug='{dest}'"),
    ];
    let r = turso::pipeline(turso_url, token, &sqls.into_iter().map(q).collect::<Vec<_>>()).await?;
    Ok(model::assemble(&r[0], &r[1], &r[2], &r[3], &r[4], &r[5], &r[6], &r[7], &r[8]))
}
```
`src/lib.rs`:
```rust
use worker::*;
mod auth; mod turso; mod model; mod render; mod styles; mod i18n; mod router;
#[event(fetch)]
pub async fn main(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    router::handle(req, env).await
}
```
**Note:** SQL here interpolates `slug`; slug comes from a token-scoped match or owner, but still sanitize: reject any slug not matching `^[a-z0-9_-]+$` before use. Add that guard in `load_plan`.

- [ ] **Step 2: Build for wasm** — `worker-build --release`. Expected: compiles.

- [ ] **Step 3: Set secrets + create R2 bucket**

```bash
cd workers/trip-dashboard-rs
npx wrangler r2 bucket create trip-dashboard-maps
for s in TURSO_URL TURSO_TOKEN OWNER_TOKEN; do
  v=$(grep "^$s=" ../../.env | cut -d= -f2-); unset CLOUDFLARE_API_TOKEN && echo "$v" | npx wrangler secret put "$s"; done
```
(TURSO_TOKEN here is a READ token.)

- [ ] **Step 4: Run dev + verify the okinawa plan with its share token**

```bash
unset CLOUDFLARE_API_TOKEN && npx wrangler dev
# owner:
curl "http://localhost:8787/?plan=okinawa-2026&token=$OWNER_TOKEN" | grep -o '中午\|Lunch: Makishi\|Yui Rail\|波上宮' | sort -u
# share token (from Task 3):
curl "http://localhost:8787/?plan=okinawa-2026&token=<share-token>" -o /dev/null -w '%{http_code}\n'   # 200
# wrong plan with okinawa share token:
curl "http://localhost:8787/?plan=tokyo-2026&token=<okinawa-share-token>" -o /dev/null -w '%{http_code}\n' # 403
# no token:
curl "http://localhost:8787/" -o /dev/null -w '%{http_code}\n'   # 403
```
Expected: noon/meal/transfer content present (the bugs that broke the TS page), and the 200/403 access matrix holds.

- [ ] **Step 5: Commit**

```bash
git add workers/trip-dashboard-rs/src/
git commit -m "feat(dashboard-rs): router, auth gate, plan loader, R2 map route, e2e"
```

---

## Task 10: Map snapshot pipeline (chromeport → R2)

Produce the plan/day map PNGs and upload to R2. This is operational tooling, run at plan-build time.

**Files:**
- Create: `scripts/snapshot-maps.sh` (or a small CLI subcommand if preferred)

- [ ] **Step 1: Decide the capture mechanic**

Drive a map view (e.g. a Google Maps URL centered on the day's stops, or an OSM static export) in real Chrome via chromeport `browser snapshot` / screenshot, producing a PNG per `(plan, day)` and one per plan. Document the exact URL/zoom per level. (No Maps API key — it's a screenshot.)

- [ ] **Step 2: Upload to R2 with the key convention**

```bash
npx wrangler r2 object put trip-dashboard-maps/okinawa-2026/day-2.png --file day-2.png
npx wrangler r2 object put trip-dashboard-maps/okinawa-2026/plan.png   --file plan.png
```
Key convention MUST match `render/map.rs`: `<plan_id>/day-<n>.png` and `<plan_id>/plan.png`.

- [ ] **Step 3: Verify the worker serves them** — `curl -I "http://localhost:8787/map/okinawa-2026/day-2.png"` → `200 image/png`.

- [ ] **Step 4: Commit**

```bash
git add scripts/snapshot-maps.sh
git commit -m "feat(dashboard-rs): chromeport→R2 map snapshot pipeline"
```

---

## Task 11: Deploy to staging, compare, cut over

- [ ] **Step 1: Deploy** — `cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy`. Note the `*.workers.dev` URL.

- [ ] **Step 2: Parity check all three plans + both formats**

```bash
for p in okinawa-2026 tokyo-2026 kyoto-2026; do
  curl -s "https://trip-dashboard-rs.<acct>.workers.dev/?plan=$p&token=$OWNER_TOKEN" -o /tmp/$p.html
done
```
Strip tags and confirm: okinawa noon/meals/transfers present; tokyo (session-based) renders; kyoto (schedule-based) renders without blank itinerary. Compare against the (bug-fixed) TS dashboard for content parity + the new design/RWD.

- [ ] **Step 3: Cut over** — once the Rust worker is at parity-or-better, point the production route/DNS at it; retire/disable the TS worker. (User decision to flip.)

- [ ] **Step 4: Final commit / note**

```bash
git commit --allow-empty -m "chore(dashboard-rs): cut over from TS worker to Rust worker"
```

---

## Self-Review Notes (filled by author)

- **Spec coverage:** §4 modules → Tasks 1,2,4,6,7,8,9. §5 access → Tasks 2,3,9. §6 keyless maps → Tasks 5,7,9,10. §7 data → Tasks 3,5. §8 layout → Tasks 6,8. §9 (no CLI-crate reuse) → Task 1 ports HTTP, never imports turso-util. §10 build order → task order. §11 testing → inline tests + Task 11 parity. §12 open items → Task 10 (capture mechanic), Task 9 (JSON API dropped — not implemented; note below).
- **Dropped from spec intentionally:** `/api/plan/<id>` JSON route — §12 listed it as open; this plan OMITS it (smaller surface). If wanted, add a route mirroring the plan loader returning `Response::from_json`.
- **Known soft spots to verify during execution:** Task 0 outbound-POST signature (spike resolves it); `with_body`/`BodyInit` exact type; `bucket.get().execute()` return shape; slug sanitization guard (added in Task 9 Step 1 note — implement it).
- **Type consistency:** `Row`=`BTreeMap<String,Value>` used everywhere; `AccessScope`/`can_view_plan` names consistent; map key convention `<plan>/day-<n>.png` matches between `render/map.rs` and Task 10.
