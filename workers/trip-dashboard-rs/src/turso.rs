//! Turso HTTP pipeline client. Ports the request/response shape of
//! workers/trip-dashboard/src/turso.ts (queryTursoPipeline + rowsToObjects).

use serde_json::Value;
use std::collections::BTreeMap;

/// One decoded row: column name -> scalar value (Null for SQL null).
pub type Row = BTreeMap<String, Value>;

/// Decode one Turso pipeline result object (`{cols:[{name}], rows:[[{type,value}]]}`)
/// into a Vec<Row>. Mirrors rowsToObjects in turso.ts.
pub fn decode_result(result: &Value) -> Vec<Row> {
    let cols: Vec<String> = result
        .get("cols")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .map(|c| {
                    c.get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    let rows = result
        .get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    rows.into_iter()
        .map(|row| {
            let cells = row.as_array().cloned().unwrap_or_default();
            let mut obj = Row::new();
            for (i, name) in cols.iter().enumerate() {
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

use worker::*;

/// POST N SQL statements to Turso /v2/pipeline; return one Vec<Row> per statement.
/// `turso_url` is the libsql:// URL; converted to https + /v2/pipeline.
pub async fn pipeline(turso_url: &str, token: &str, sqls: &[String]) -> Result<Vec<Vec<Row>>> {
    let url = turso_url.replace("libsql://", "https://") + "/v2/pipeline";
    let mut requests: Vec<Value> = sqls
        .iter()
        .map(|sql| serde_json::json!({ "type": "execute", "stmt": { "sql": sql } }))
        .collect();
    requests.push(serde_json::json!({ "type": "close" }));
    let body = serde_json::json!({ "requests": requests });

    let headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {token}"))?;
    headers.set("Content-Type", "application/json")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(wasm_bindgen::JsValue::from_str(
            &serde_json::to_string(&body)?,
        )));
    let req = Request::new_with_init(&url, &init)?;
    let mut res = Fetch::Request(req).send().await?;
    if res.status_code() >= 400 {
        let t = res.text().await.unwrap_or_default();
        return Err(Error::RustError(format!(
            "Turso HTTP {}: {t}",
            res.status_code()
        )));
    }
    let json: Value = res.json().await?;
    decode_pipeline(&json, sqls.len()).map_err(Error::RustError)
}

/// Decode a /v2/pipeline response into one Vec<Row> per statement. Fails loud
/// when a statement failed (Hrana puts it at `results[i] = {type:"error",
/// error:{..}}`) or its result is missing — a failed query must never render
/// as an empty section.
pub fn decode_pipeline(json: &Value, n: usize) -> std::result::Result<Vec<Vec<Row>>, String> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let entry = json
            .get("results")
            .and_then(|r| r.get(i))
            .ok_or_else(|| format!("Turso query {i}: no result in the pipeline response"))?;
        if let Some(err) = entry
            .get("error")
            .or_else(|| entry.get("response").and_then(|r| r.get("error")))
        {
            return Err(format!("Turso query {i} error: {err}"));
        }
        if entry.get("type").and_then(|t| t.as_str()) == Some("error") {
            return Err(format!("Turso query {i} error: {entry}"));
        }
        let result = entry
            .get("response")
            .and_then(|r| r.get("result"))
            .ok_or_else(|| format!("Turso query {i}: response has no result"))?;
        out.push(decode_result(result));
    }
    Ok(out)
}

// ── Bound parameters + affected_row_count + baton transactions (travel notes T1) ──
//
// Hrana over HTTP v2 shapes (verified against the official Turso SQL-over-HTTP
// reference): the request body is `{baton?, requests:[...]}`; each execute op is
// `{"type":"execute","stmt":{"sql":...,"args":[...]}}`; `args` elements are typed
// values `{"type":"text","value":...}` / `{"type":"integer","value":"<string>"}` —
// integers are STRINGS in JSON to avoid losing precision — / `{"type":"null"}`.
// The response body carries a TOP-LEVEL `baton` when the request did NOT end with
// a `close` op; pass it back top-level to continue the same connection, and run
// BEGIN/COMMIT/ROLLBACK as ordinary SQL. Each execute result carries
// `affected_row_count`.
//
// The response also carries a TOP-LEVEL `base_url: string | null` — per
// HTTP_V2_SPEC.md: "If the `base_url` in the response is not `null`, the client
// should use this URL when sending further requests on this stream." (stream
// stickiness: the baton lives on one server). The transaction legs honour it
// via `follow_up_url`.
//
// The commit leg is ONE `batch` request, not a sequence of execute requests: a
// request that errors does not stop the remaining requests of the same
// pipeline, so plain `execute` + `COMMIT` would commit even after the event
// INSERT failed. A batch's own step conditions make COMMIT conditional — see
// `encode_commit_batch`.

/// One bound statement argument. Integer values are sent as strings per the
/// Hrana JSON encoding (JSON numbers may be treated as 64-bit floats).
#[derive(Debug, Clone)]
#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
pub enum Arg {
    Text(String),
    Int(i64),
    Null,
}

/// One SQL statement with bound parameters.
#[derive(Debug, Clone)]
#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
pub struct Stmt {
    pub sql: String,
    pub args: Vec<Arg>,
}

/// Per-statement result: decoded rows plus the statement's `affected_row_count`.
#[derive(Debug, Clone)]
#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
pub struct StmtResult {
    pub rows: Vec<Row>,
    pub affected: u64,
}

/// Failure of a `pipeline_txn`. `RowMismatch` means a content statement did not
/// touch the number of rows the caller expected (missing / soft-deleted note,
/// 0-row update) — the caller answers 404, not a crash; `Db` is a transport or
/// statement error.
#[derive(Debug, Clone)]
#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
pub enum TxnError {
    RowMismatch { index: usize, expected: u64, got: u64 },
    Db(String),
}

impl std::fmt::Display for TxnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TxnError::RowMismatch { index, expected, got } => write!(
                f,
                "statement {index} changed {got} rows, expected {expected}"
            ),
            TxnError::Db(e) => write!(f, "{e}"),
        }
    }
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
fn encode_arg(arg: &Arg) -> Value {
    match arg {
        Arg::Text(s) => serde_json::json!({"type": "text", "value": s}),
        Arg::Int(i) => serde_json::json!({"type": "integer", "value": i.to_string()}),
        Arg::Null => serde_json::json!({"type": "null"}),
    }
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
fn encode_stmt_value(stmt: &Stmt) -> Value {
    let args: Vec<Value> = stmt.args.iter().map(encode_arg).collect();
    serde_json::json!({"sql": stmt.sql, "args": args})
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
fn encode_stmt_request(stmt: &Stmt) -> Value {
    serde_json::json!({"type": "execute", "stmt": encode_stmt_value(stmt)})
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// One-shot request body for `stmts` (bound parameters), ending with `close`.
pub fn encode_pipeline_args(stmts: &[Stmt]) -> Value {
    let mut requests: Vec<Value> = stmts.iter().map(encode_stmt_request).collect();
    requests.push(serde_json::json!({"type": "close"}));
    serde_json::json!({"requests": requests})
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// Request body that keeps the session open (no `close`) so the response
/// returns a baton. Used as the first leg of `pipeline_txn`.
fn encode_open_body(baton: Option<&str>, stmts: &[Stmt]) -> Value {
    let requests: Vec<Value> = stmts.iter().map(encode_stmt_request).collect();
    let mut body = serde_json::json!({"requests": requests});
    if let Some(b) = baton {
        body["baton"] = serde_json::json!(b);
    }
    body
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// Rollback continuation on an existing baton: `ROLLBACK` + `close`, nothing else.
/// Sent whenever the first leg (BEGIN + content) fails AFTER the baton was
/// obtained — statement error, decode error, or expected-row-count mismatch:
/// no path may return with an open transaction.
fn encode_rollback(baton: &str) -> Value {
    serde_json::json!({
        "baton": baton,
        "requests": [
            {"type": "execute", "stmt": {"sql": "ROLLBACK"}},
            {"type": "close"},
        ]
    })
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// Commit leg: ONE `batch` request on the baton, so a failing event INSERT can
/// never be followed by an unconditional COMMIT (a request that errors does not
/// stop the remaining requests of the same pipeline — HTTP_V2_SPEC.md, so plain
/// `execute` + `COMMIT` requests would commit past a failed INSERT).
///
/// Batch/condition shapes, quoted from HRANA_3_SPEC.md:
///   `type Batch = { "steps": Array<BatchStep>, }`
///   `type BatchStep = { "condition"?: BatchCond | null, "stmt": Stmt, }`
///   "`ok` evaluates to true if the `step` (referenced by its 0-based index)
///    was executed successfully." (If the step was skipped, `ok` is false.)
///   `type BatchCond = { "type": "ok", "step": uint32 }`
///                   | `{ "type": "not", "cond": BatchCond }`
///                   | `{ "type": "and", "conds": Array<BatchCond> }`
/// HTTP_V2_SPEC.md wraps it: `type BatchStreamReq = { "type": "batch",
/// "batch": Batch }` — "The `batch` request has the same semantics as the
/// `batch` request in Hrana."
///
/// Steps: every `tail` statement (unconditional), then
///   COMMIT   with condition and(ok(0), …, ok(n-1)),
///   ROLLBACK with condition not(and(ok(0), …, ok(n-1))).
/// A tail statement that errors makes COMMIT's condition false — COMMIT is
/// skipped and ROLLBACK runs instead, in the same request, before `close`.
///
/// Tail row counts (review finding 5): the tail holds single-row
/// `INSERT … VALUES (…)` statements (the `note_events` audit rows). Such an
/// INSERT either writes exactly one row or errors — it cannot succeed with 0
/// rows — so `ok(step)` IS the row-count check for the tail; a separate
/// `affected_row_count` comparison would add nothing. Statements whose row
/// count genuinely varies (UPDATE … WHERE …) belong in `content` with
/// per-statement `expected`, which `pipeline_txn` already checks after leg 1.
pub fn encode_commit_batch(baton: &str, tail: &[Stmt]) -> Value {
    let mut steps: Vec<Value> = tail.iter().map(encode_stmt_value).map(|stmt| {
        serde_json::json!({"stmt": stmt})
    }).collect();
    if tail.is_empty() {
        // No tail steps to reference: and([]) is undefined by the spec, so an
        // empty tail commits unconditionally (nothing can fail before COMMIT).
        steps.push(serde_json::json!({"stmt": {"sql": "COMMIT"}}));
    } else {
        let all_tail_ok = serde_json::json!({
            "type": "and",
            "conds": (0..tail.len())
                .map(|i| serde_json::json!({"type": "ok", "step": i}))
                .collect::<Vec<Value>>(),
        });
        steps.push(serde_json::json!({
            "condition": all_tail_ok,
            "stmt": {"sql": "COMMIT"},
        }));
        steps.push(serde_json::json!({
            "condition": {"type": "not", "cond": all_tail_ok},
            "stmt": {"sql": "ROLLBACK"},
        }));
    }
    serde_json::json!({
        "baton": baton,
        "requests": [
            {"type": "batch", "batch": {"steps": steps}},
            {"type": "close"},
        ]
    })
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// Follow-up /v2/pipeline URL for the next request on a stream, honouring the
/// response's `base_url` (stream stickiness). HTTP_V2_SPEC.md: "If the
/// `base_url` in the response is not `null`, the client should use this URL
/// when sending further requests on this stream." / "If it is `null`, the
/// client should use the same URL that it has used for the previous request."
/// / "The `base_url` must be an absolute URL with "http" or "https" scheme."
/// A non-null base_url that is not a string or not an absolute http(s) URL is a
/// malformed response — an error, never a guess.
pub fn follow_up_url(current: &str, json: &Value) -> std::result::Result<String, String> {
    match json.get("base_url") {
        None | Some(Value::Null) => Ok(current.to_string()),
        Some(v) => {
            let s = v
                .as_str()
                .ok_or_else(|| "Turso pipeline response base_url is not a string".to_string())?;
            let parsed = url::Url::parse(s)
                .map_err(|e| format!("Turso response base_url {s:?} is not a valid URL: {e}"))?;
            if parsed.scheme() != "http" && parsed.scheme() != "https" {
                return Err(format!(
                    "Turso response base_url must be an absolute http(s) URL, got {s:?}"
                ));
            }
            Ok(s.to_string())
        }
    }
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// Decode one Hrana `StmtResult` object (`{cols, rows, affected_row_count, …}`).
/// `affected_row_count` is a REQUIRED uint64 — HRANA_3_SPEC.md: `type StmtResult
/// = { "cols": Array<Col>, "rows": Array<Array<Value>>,
/// "affected_row_count": uint64, … }`. Missing, null, or non-integer is a
/// malformed response and an ERROR, never a silent 0: with `expected == 0` a
/// silent 0 would wrongly read as success and COMMIT.
fn decode_stmt_result(result: &Value, i: usize) -> std::result::Result<StmtResult, String> {
    let rows = decode_result(result);
    let affected = result
        .get("affected_row_count")
        .and_then(|a| a.as_u64())
        .ok_or_else(|| {
            format!("Turso query {i}: affected_row_count missing or not an integer")
        })?;
    Ok(StmtResult { rows, affected })
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// Decode a /v2/pipeline response into one `StmtResult` per statement,
/// reading each execute result's `affected_row_count`. Same fail-loud
/// semantics as `decode_pipeline` — a failed or missing statement result
/// never decodes as an empty success.
pub fn decode_pipeline_results(json: &Value, n: usize) -> std::result::Result<Vec<StmtResult>, String> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let entry = json
            .get("results")
            .and_then(|r| r.get(i))
            .ok_or_else(|| format!("Turso query {i}: no result in the pipeline response"))?;
        if let Some(err) = entry
            .get("error")
            .or_else(|| entry.get("response").and_then(|r| r.get("error")))
        {
            return Err(format!("Turso query {i} error: {err}"));
        }
        if entry.get("type").and_then(|t| t.as_str()) == Some("error") {
            return Err(format!("Turso query {i} error: {entry}"));
        }
        let result = entry
            .get("response")
            .and_then(|r| r.get("result"))
            .ok_or_else(|| format!("Turso query {i}: response has no result"))?;
        out.push(decode_stmt_result(result, i)?);
    }
    Ok(out)
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// Decode the /v2/pipeline response of a commit-leg `batch` request
/// (`encode_commit_batch`) into the `StmtResult`s of the `n_tail` tail
/// statements. HTTP_V2_SPEC.md: `type BatchStreamResp = { "type": "batch",
/// "result": BatchResult }`; HRANA_3_SPEC.md: `type BatchResult = {
/// "step_results": Array<StmtResult | null>, "step_errors": Array<Error | null>
/// }` — "If the statement was skipped because its condition evaluated to
/// false … both `step_results[i]` and `step_errors[i]` will be `null`."
///
/// A non-null `step_errors[i]` means step i failed: the batch's negated
/// condition already rolled the transaction back server-side, but the failure
/// must still surface — never decode as success. Tail steps are unconditional,
/// so a null `step_results[i]` for one is impossible and an error.
pub fn decode_batch_response(json: &Value, n_tail: usize) -> std::result::Result<Vec<StmtResult>, String> {
    let entry = json
        .get("results")
        .and_then(|r| r.get(0))
        .ok_or_else(|| "Turso batch: no result in the pipeline response".to_string())?;
    if let Some(err) = entry
        .get("error")
        .or_else(|| entry.get("response").and_then(|r| r.get("error")))
    {
        return Err(format!("Turso batch error: {err}"));
    }
    if entry.get("type").and_then(|t| t.as_str()) == Some("error") {
        return Err(format!("Turso batch error: {entry}"));
    }
    let batch_result = entry
        .get("response")
        .and_then(|r| r.get("result"))
        .ok_or_else(|| "Turso batch: response has no result".to_string())?;
    let step_errors = batch_result
        .get("step_errors")
        .and_then(|s| s.as_array())
        .ok_or_else(|| "Turso batch: result has no step_errors".to_string())?;
    for (i, e) in step_errors.iter().enumerate() {
        if !e.is_null() {
            let msg = e
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("(no message)");
            return Err(format!("Turso batch step {i} error: {msg}"));
        }
    }
    let step_results = batch_result
        .get("step_results")
        .and_then(|s| s.as_array())
        .ok_or_else(|| "Turso batch: result has no step_results".to_string())?;
    // n_tail tail steps + COMMIT (+ ROLLBACK when there are tail steps to
    // condition on — an empty tail commits unconditionally, no ROLLBACK step).
    let expected_len = n_tail + if n_tail == 0 { 1 } else { 2 };
    if step_results.len() != expected_len {
        return Err(format!(
            "Turso batch returned {n} step results, expected {expected_len}", n = step_results.len()
        ));
    }
    let mut out = Vec::with_capacity(n_tail);
    for i in 0..n_tail {
        let r = step_results
            .get(i)
            .filter(|r| !r.is_null())
            .ok_or_else(|| {
                format!(
                    "Turso batch step {i} has no result (tail steps are unconditional — it cannot have been skipped)"
                )
            })?;
        out.push(decode_stmt_result(r, i)?);
    }
    Ok(out)
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// Extract the session baton from a pipeline response. No baton (or a null
/// one — what the server sends after `close`) is an error: fail loud rather
/// than silently open a second session the caller cannot roll back.
pub fn decode_baton(json: &Value) -> std::result::Result<String, String> {
    json.get("baton")
        .and_then(|b| b.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            "Turso pipeline response has no baton — the session was not kept open".to_string()
        })
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// Pure: every content statement must have touched exactly its expected
/// number of rows. A per-statement check — `changes()` only reflects the last
/// statement, so a multi-statement write whose FIRST statement touched 0 rows
/// would otherwise commit half-applied.
pub fn check_expected(results: &[StmtResult], expected: &[u64]) -> std::result::Result<(), TxnError> {
    if results.len() != expected.len() {
        return Err(TxnError::Db(format!(
            "row-count check got {} statements, expected {}",
            results.len(),
            expected.len()
        )));
    }
    for (i, (r, e)) in results.iter().zip(expected.iter()).enumerate() {
        if r.affected != *e {
            return Err(TxnError::RowMismatch { index: i, expected: *e, got: r.affected });
        }
    }
    Ok(())
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// Full /v2/pipeline endpoint URL derived from the libsql:// URL.
fn pipeline_url(turso_url: &str) -> String {
    turso_url.replace("libsql://", "https://") + "/v2/pipeline"
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// POST one already-built pipeline body to a FULL /v2/pipeline URL (the
/// original endpoint, or a stickier `base_url` from an earlier response) and
/// return the decoded JSON.
async fn post_pipeline(url: &str, token: &str, body: &Value) -> Result<Value> {
    let headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {token}"))?;
    headers.set("Content-Type", "application/json")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(wasm_bindgen::JsValue::from_str(
            &serde_json::to_string(body)?,
        )));
    let req = Request::new_with_init(url, &init)?;
    let mut res = Fetch::Request(req).send().await?;
    if res.status_code() >= 400 {
        let t = res.text().await.unwrap_or_default();
        return Err(Error::RustError(format!(
            "Turso HTTP {}: {t}",
            res.status_code()
        )));
    }
    Ok(res.json().await?)
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// POST bound-parameter statements as a one-shot pipeline (ends with `close`)
/// and return one `StmtResult` per statement, with `affected_row_count`.
pub async fn pipeline_args(turso_url: &str, token: &str, stmts: &[Stmt]) -> Result<Vec<StmtResult>> {
    let body = encode_pipeline_args(stmts);
    let json = post_pipeline(&pipeline_url(turso_url), token, &body).await?;
    decode_pipeline_results(&json, stmts.len()).map_err(Error::RustError)
}

/// Best-effort ROLLBACK + close on a baton: every error path in `pipeline_txn`
/// that owns a baton calls this before returning, so no path returns with an
/// open transaction. Its own failure is swallowed: with the stream already
/// broken there is nothing left to roll back — the server drops abandoned
/// streams after an idle timeout (HTTP_V2_SPEC.md) and SQLite journal
/// semantics discard the uncommitted transaction with the connection.
async fn rollback_and_close(url: &str, token: &str, baton: &str) {
    let _ = post_pipeline(url, token, &encode_rollback(baton)).await;
}

#[allow(dead_code)] // no callers until the notes routes land (plan T5/T6); tests cover it for now
/// Two-leg transaction on a Hrana baton:
/// 1. `BEGIN` + `content`, NOT closed — the response returns a baton and
///    each statement's `affected_row_count`.
/// 2. Every content statement is compared against `expected` (content SQL
///    should carry `deleted_at IS NULL` so a missing/soft-deleted note reads 0).
///    All match → ONE `batch` request: `tail` steps, then `COMMIT` conditioned
///    on and(ok(every tail step)) and `ROLLBACK` on the negation, then `close`
///    — a failing event INSERT can never be followed by a COMMIT. Any
///    first-leg mismatch or error → `ROLLBACK` + `close` on the same baton
///    (no tail, no events) and the error is returned.
///
/// Every follow-up request honours a non-null `base_url` in the previous
/// response (stream stickiness — the baton lives on one server).
pub async fn pipeline_txn(
    turso_url: &str,
    token: &str,
    content: &[Stmt],
    expected: &[u64],
    tail: &[Stmt],
) -> std::result::Result<Vec<StmtResult>, TxnError> {
    let begin = Stmt { sql: "BEGIN".to_string(), args: Vec::new() };
    let first: Vec<Stmt> = std::iter::once(begin).chain(content.iter().cloned()).collect();
    let first_body = encode_open_body(None, &first);
    let mut url = pipeline_url(turso_url);
    let json = post_pipeline(&url, token, &first_body)
        .await
        .map_err(|e| TxnError::Db(e.to_string()))?;
    // No baton means the server already closed the stream — there is no
    // session left to roll back, and its connection (with any open
    // transaction) is gone with it.
    let baton = decode_baton(&json).map_err(TxnError::Db)?;
    // From here we own the baton: EVERY error path rolls back on it first —
    // including a malformed base_url, which is a decode error in step one.
    match follow_up_url(&url, &json) {
        Ok(u) => url = u, // stream stickiness: leg 2 reaches the baton's server
        Err(e) => {
            rollback_and_close(&url, token, &baton).await;
            return Err(TxnError::Db(e));
        }
    }
    let results = match decode_pipeline_results(&json, 1 + content.len()) {
        Ok(r) => r,
        Err(e) => {
            rollback_and_close(&url, token, &baton).await;
            return Err(TxnError::Db(e));
        }
    };
    let content_results = &results[1..]; // skip BEGIN's own result
    if let Err(mismatch) = check_expected(content_results, expected) {
        rollback_and_close(&url, token, &baton).await;
        return Err(mismatch);
    }
    // Commit leg: a single batch — tail + conditional COMMIT/ROLLBACK + close.
    // If the POST itself fails, the stream is already broken: the server drops
    // it on idle timeout and the uncommitted transaction goes with the
    // connection; there is no baton left to send a rollback to.
    let commit_body = encode_commit_batch(&baton, tail);
    let json2 = post_pipeline(&url, token, &commit_body)
        .await
        .map_err(|e| TxnError::Db(e.to_string()))?;
    decode_batch_response(&json2, tail.len()).map_err(TxnError::Db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pipeline_ok_entries_decode_per_statement() {
        let ok = json!({"type": "ok", "response": {"type": "execute", "result": {
            "cols": [{"name": "n"}], "rows": [[{"type": "integer", "value": "1"}]]}}});
        let resp = json!({"results": [ok.clone(), ok, {"type": "ok", "response": {"type": "close"}}]});
        let out = decode_pipeline(&resp, 2).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[1][0]["n"], json!("1"));
    }

    #[test]
    fn pipeline_statement_error_fails_loud() {
        let resp = json!({"results": [
            {"type": "ok", "response": {"type": "execute", "result": {"cols": [], "rows": []}}},
            {"type": "error", "error": {"message": "SQLite error: no such table: plan_documents", "code": "SQLITE_UNKNOWN"}}
        ]});
        let err = decode_pipeline(&resp, 2).unwrap_err();
        assert!(err.contains("Turso query 1 error"), "{err}");
        assert!(err.contains("no such table: plan_documents"), "{err}");
    }

    #[test]
    fn pipeline_missing_or_resultless_entry_fails_loud() {
        let ok = json!({"type": "ok", "response": {"type": "execute", "result": {"cols": [], "rows": []}}});
        assert!(decode_pipeline(&json!({"results": [ok.clone()]}), 2).unwrap_err().contains("query 1: no result"));
        let bare = json!({"results": [ok, {"type": "ok", "response": {"type": "execute"}}]});
        assert!(decode_pipeline(&bare, 2).unwrap_err().contains("query 1: response has no result"));
    }

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

    // ── T1 (travel notes): bound parameters + affected_row_count + baton transactions ──

    fn stmt(sql: &str, args: Vec<Arg>) -> Stmt {
        Stmt { sql: sql.to_string(), args }
    }

    #[test]
    fn encode_int_as_string() {
        // Hrana over HTTP: integer values are STRINGS in JSON ("to avoid losing precision").
        let body = encode_pipeline_args(&[stmt("UPDATE t SET a = ?", vec![Arg::Int(42)])]);
        let args = &body["requests"][0]["stmt"]["args"];
        assert_eq!(args.as_array().unwrap().len(), 1);
        assert_eq!(args[0], json!({"type": "integer", "value": "42"}));
    }

    #[test]
    fn encode_text_roundtrip_quotes_emoji() {
        let text = "O'Brien; DROP TABLE x; ✈️";
        let body = encode_pipeline_args(&[stmt(
            "UPDATE notes SET title = ? WHERE id = ?",
            vec![Arg::Text(text.to_string()), Arg::Text("note_0123456789abcdef".to_string())],
        )]);
        let args = &body["requests"][0]["stmt"]["args"];
        assert_eq!(args[0]["type"], json!("text"));
        assert_eq!(args[0]["value"], json!(text));
        // The literal never enters the SQL string — that is the whole point of binding.
        let sql = body["requests"][0]["stmt"]["sql"].as_str().unwrap();
        assert!(!sql.contains(text), "text leaked into sql: {sql}");
        assert!(!sql.contains("O'Brien"), "quote leaked into sql: {sql}");
    }

    #[test]
    fn encode_null() {
        let body = encode_pipeline_args(&[stmt("UPDATE notes SET place_name = ?", vec![Arg::Null])]);
        assert_eq!(body["requests"][0]["stmt"]["args"][0], json!({"type": "null"}));
    }

    #[test]
    fn decode_reads_affected_row_count() {
        let ok = json!({"type": "ok", "response": {"type": "execute", "result": {
            "cols": [{"name": "title"}],
            "rows": [[{"type": "text", "value": "GODIVA"}]],
            "affected_row_count": 3
        }}});
        let resp = json!({"results": [ok, {"type": "ok", "response": {"type": "close"}}]});
        let out = decode_pipeline_results(&resp, 1).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].affected, 3);
        assert_eq!(out[0].rows[0]["title"], json!("GODIVA"));
    }

    fn sr(affected: u64) -> StmtResult {
        StmtResult { rows: Vec::new(), affected }
    }

    #[test]
    fn check_expected_missing_note() {
        // UPDATE ... WHERE id = ? on a note that does not exist: 0 rows.
        match check_expected(&[sr(0)], &[1]) {
            Err(TxnError::RowMismatch { index: 0, expected: 1, got: 0 }) => {}
            other => panic!("expected RowMismatch{{0,1,0}}, got {other:?}"),
        }
        assert!(check_expected(&[sr(1)], &[1]).is_ok());
    }

    #[test]
    fn check_expected_deleted_note() {
        // UPDATE ... WHERE id = ? AND deleted_at IS NULL on a soft-deleted note: 0 rows.
        // Simulate the decoded response of such a statement.
        let ok = json!({"type": "ok", "response": {"type": "execute", "result": {
            "cols": [], "rows": [], "affected_row_count": 0
        }}});
        let resp = json!({"results": [ok]});
        let out = decode_pipeline_results(&resp, 1).unwrap();
        match check_expected(&out, &[1]) {
            Err(TxnError::RowMismatch { index: 0, expected: 1, got: 0 }) => {}
            other => panic!("expected RowMismatch{{0,1,0}}, got {other:?}"),
        }
    }

    #[test]
    fn check_expected_partial_reorder() {
        // Two UPDATEs for a photo move-up/move-down; the first matched 0 rows.
        // A `changes()`-style single-statement check (only reflects the LAST statement)
        // would miss this — per-statement expected counts must not.
        match check_expected(&[sr(0), sr(1)], &[1, 1]) {
            Err(TxnError::RowMismatch { index: 0, expected: 1, got: 0 }) => {}
            other => panic!("expected RowMismatch{{0,1,0}}, got {other:?}"),
        }
        assert!(check_expected(&[sr(1), sr(1)], &[1, 1]).is_ok());
    }

    #[test]
    fn check_expected_zero_row_update() {
        // Apart from same-value updates (which SQLite still counts as changed),
        // a 0-row result is always a mismatch when a row was expected.
        assert!(matches!(
            check_expected(&[sr(0)], &[1]),
            Err(TxnError::RowMismatch { .. })
        ));
        // Expected 0 and got 0 (e.g. a deliberate no-op expectation) is fine.
        assert!(check_expected(&[sr(0)], &[0]).is_ok());
    }

    #[test]
    fn rollback_body_is_only_rollback_and_close() {
        // Fix 2: any first-leg failure after BEGIN must roll back on the same
        // baton — exactly ROLLBACK + close, nothing else.
        let body = encode_rollback("baton-abc");
        assert_eq!(body["baton"], json!("baton-abc"));
        let reqs = body["requests"].as_array().unwrap();
        assert_eq!(reqs.len(), 2, "rollback body must be exactly ROLLBACK + close: {body}");
        assert_eq!(reqs[0]["stmt"]["sql"], json!("ROLLBACK"));
        assert_eq!(reqs[1], json!({"type": "close"}));
        // No tail statement may ride along on a rollback.
        let s = serde_json::to_string(&body).unwrap();
        assert!(!s.contains("note_events"), "tail leaked into rollback body: {s}");
    }

    fn tail_stmts() -> Vec<Stmt> {
        vec![
            stmt(
                "INSERT INTO note_events (event_id, note_id, action) VALUES (?, ?, 'update')",
                vec![Arg::Text("evt1".to_string()), Arg::Text("note_1".to_string())],
            ),
            stmt(
                "UPDATE note_events SET detail = ? WHERE event_id = ?",
                vec![Arg::Text("T".to_string()), Arg::Text("evt1".to_string())],
            ),
        ]
    }

    #[test]
    fn commit_batch_shape_with_conditions() {
        // Fix 1: the commit leg is ONE `batch` request. HRANA_3_SPEC.md:
        //   type Batch = { "steps": Array<BatchStep>, }
        //   type BatchStep = { "condition"?: BatchCond | null, "stmt": Stmt, }
        //   "ok evaluates to true if the step (referenced by its 0-based index)
        //    was executed successfully."
        // COMMIT runs only if EVERY tail step was ok; ROLLBACK runs on the negation.
        let body = encode_commit_batch("baton-9", &tail_stmts());
        assert_eq!(body["baton"], json!("baton-9"));
        let reqs = body["requests"].as_array().unwrap();
        assert_eq!(reqs.len(), 2, "commit leg must be batch + close only: {body}");
        assert_eq!(reqs[1], json!({"type": "close"}));
        assert_eq!(reqs[0]["type"], json!("batch"));
        let steps = reqs[0]["batch"]["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 4, "2 tail + COMMIT + ROLLBACK: {body}");
        // Tail steps are unconditional; args ride in the stmt.
        assert!(steps[0].get("condition").is_none(), "steps[0] must be unconditional: {steps:?}");
        assert!(steps[1].get("condition").is_none(), "steps[1] must be unconditional: {steps:?}");
        assert_eq!(
            steps[0]["stmt"]["sql"],
            json!("INSERT INTO note_events (event_id, note_id, action) VALUES (?, ?, 'update')")
        );
        assert_eq!(steps[0]["stmt"]["args"][0], json!({"type": "text", "value": "evt1"}));
        let all_ok = json!({"type": "and", "conds": [
            {"type": "ok", "step": 0},
            {"type": "ok", "step": 1},
        ]});
        assert_eq!(steps[2]["stmt"]["sql"], json!("COMMIT"));
        assert_eq!(
            steps[2]["condition"], all_ok,
            "COMMIT must be conditioned on ok() of every tail step"
        );
        assert_eq!(steps[3]["stmt"]["sql"], json!("ROLLBACK"));
        assert_eq!(
            steps[3]["condition"],
            json!({"type": "not", "cond": all_ok}),
            "ROLLBACK must run exactly when COMMIT does not"
        );
    }

    #[test]
    fn commit_batch_empty_tail_commits_unconditionally() {
        // No tail steps to reference: and([]) is undefined by the spec, so an
        // empty tail commits unconditionally with no ROLLBACK step.
        let body = encode_commit_batch("b0", &[]);
        let steps = body["requests"][0]["batch"]["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 1, "{body}");
        assert!(steps[0].get("condition").is_none());
        assert_eq!(steps[0]["stmt"]["sql"], json!("COMMIT"));
    }

    #[test]
    fn follow_up_url_honours_non_null_base_url() {
        // Fix 3 (HTTP_V2_SPEC.md): "If the `base_url` in the response is not
        // `null`, the client should use this URL when sending further requests
        // on this stream." Null/absent means: same URL as the previous request.
        let cur = "https://db.example/v2/pipeline";
        assert_eq!(follow_up_url(cur, &json!({"baton": "x", "base_url": null})).unwrap(), cur);
        assert_eq!(follow_up_url(cur, &json!({"baton": "x"})).unwrap(), cur);
        assert_eq!(
            follow_up_url(cur, &json!({"base_url": "https://sticky.example/v2/pipeline"})).unwrap(),
            "https://sticky.example/v2/pipeline"
        );
        // Non-string or non-absolute-http(s) base_url is malformed — fail loud, never guess.
        assert!(follow_up_url(cur, &json!({"base_url": 7})).is_err());
        assert!(follow_up_url(cur, &json!({"base_url": "not-a-url"})).is_err());
    }

    #[test]
    fn affected_row_count_missing_or_non_integer_fails_loud() {
        // Fix 4: HRANA_3_SPEC.md marks StmtResult.affected_row_count a REQUIRED
        // uint64. Missing / null / non-integer must be an error, never a silent 0 —
        // a silent 0 against `expected == 0` would wrongly read as success.
        fn resp(arc: Option<Value>) -> Value {
            let mut result = json!({"cols": [], "rows": []});
            if let Some(v) = arc {
                result["affected_row_count"] = v;
            }
            json!({"results": [{"type": "ok", "response": {"type": "execute", "result": result}}]})
        }
        let err = decode_pipeline_results(&resp(None), 1).unwrap_err();
        assert!(err.contains("affected_row_count"), "{err}");
        assert!(decode_pipeline_results(&resp(Some(Value::Null)), 1).is_err());
        assert!(decode_pipeline_results(&resp(Some(json!("3"))), 1).is_err());
        assert!(decode_pipeline_results(&resp(Some(json!(1.5))), 1).is_err());
        assert!(decode_pipeline_results(&resp(Some(json!(-1))), 1).is_err());
        assert_eq!(decode_pipeline_results(&resp(Some(json!(3))), 1).unwrap()[0].affected, 3);
    }

    #[test]
    fn decode_batch_response_ok_and_failures() {
        // OK: one tail step executed; COMMIT executed (result present);
        // ROLLBACK skipped (null) — "both step_results[i] and step_errors[i]
        // will be null" for a skipped step (HRANA_3_SPEC.md, BatchResult).
        let resp = json!({"results": [{"type": "ok", "response": {"type": "batch", "result": {
            "step_results": [
                {"cols": [], "rows": [], "affected_row_count": 1},
                {"cols": [], "rows": [], "affected_row_count": 0},
                null
            ],
            "step_errors": [null, null, null]
        }}}]});
        let out = decode_batch_response(&resp, 1).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].affected, 1);

        // Tail step errored → the negated condition already rolled the batch
        // back server-side; decoding must still surface the step error.
        let err_resp = json!({"results": [{"type": "ok", "response": {"type": "batch", "result": {
            "step_results": [null, null, {"cols": [], "rows": [], "affected_row_count": 0}],
            "step_errors": [
                {"message": "SQLite error: UNIQUE constraint failed: note_events.event_id", "code": "SQLITE_CONSTRAINT"},
                null,
                null
            ]
        }}}]});
        let err = decode_batch_response(&err_resp, 1).unwrap_err();
        assert!(err.contains("step 0"), "{err}");
        assert!(err.contains("UNIQUE constraint"), "{err}");

        // Stream-level error entry.
        let stream_err = json!({"results": [{"type": "error", "error": {"message": "stream closed"}}]});
        assert!(decode_batch_response(&stream_err, 1).unwrap_err().contains("stream closed"));

        // Wrong step count.
        let short = json!({"results": [{"type": "ok", "response": {"type": "batch", "result": {
            "step_results": [{"cols": [], "rows": [], "affected_row_count": 1}],
            "step_errors": [null]
        }}}]});
        assert!(decode_batch_response(&short, 1).unwrap_err().contains("step results"));

        // A null result on an unconditional tail step cannot happen — fail loud.
        let skipped = json!({"results": [{"type": "ok", "response": {"type": "batch", "result": {
            "step_results": [null, {"cols": [], "rows": [], "affected_row_count": 0}, null],
            "step_errors": [null, null, null]
        }}}]});
        assert!(decode_batch_response(&skipped, 1).unwrap_err().contains("no result"));
    }

    #[test]
    fn pipeline_bodies_end_with_close() {
        // Every one-shot or continuation request ends with {"type":"close"} —
        // never leaves a server-side session dangling.
        let one_shot = encode_pipeline_args(&[stmt("SELECT 1", vec![])]);
        assert_eq!(
            one_shot["requests"].as_array().unwrap().last().unwrap(),
            &json!({"type": "close"})
        );
        let rollback = encode_rollback("b");
        assert_eq!(
            rollback["requests"].as_array().unwrap().last().unwrap(),
            &json!({"type": "close"})
        );
        let commit = encode_commit_batch(
            "b",
            &[stmt("INSERT INTO note_events (event_id) VALUES (?)", vec![Arg::Text("e".to_string())])],
        );
        assert_eq!(
            commit["requests"].as_array().unwrap().last().unwrap(),
            &json!({"type": "close"})
        );
    }

    #[test]
    fn decode_baton_missing_fails_loud() {
        assert!(decode_baton(&json!({"results": []})).is_err());
        // After an explicit close the server sends baton: null — that is NOT a usable baton.
        assert!(decode_baton(&json!({"baton": null, "results": []})).is_err());
        assert_eq!(decode_baton(&json!({"baton": "m7lVVgEvknpf1", "results": []})).unwrap(), "m7lVVgEvknpf1");
    }

    #[test]
    fn decode_error_entry_fails_loud() {
        // Same error semantics as decode_pipeline: a failed statement must never
        // decode as an empty result.
        let resp = json!({"results": [
            {"type": "error", "error": {"message": "SQLite error: no such table: notes", "code": "SQLITE_UNKNOWN"}}
        ]});
        let err = decode_pipeline_results(&resp, 1).unwrap_err();
        assert!(err.contains("Turso query 0 error"), "{err}");
        assert!(err.contains("no such table: notes"), "{err}");
        // Missing entry / entry without a result also fail loud.
        let empty = json!({"results": []});
        assert!(decode_pipeline_results(&empty, 1).unwrap_err().contains("query 0: no result"));
        let resultless = json!({"results": [{"type": "ok", "response": {"type": "execute"}}]});
        assert!(decode_pipeline_results(&resultless, 1).unwrap_err().contains("query 0: response has no result"));
    }
}
