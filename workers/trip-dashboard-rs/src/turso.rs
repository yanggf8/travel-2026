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
    let mut out = Vec::with_capacity(sqls.len());
    for (i, _) in sqls.iter().enumerate() {
        let entry = json.get("results").and_then(|r| r.get(i));
        if let Some(err) = entry
            .and_then(|e| e.get("response"))
            .and_then(|r| r.get("error"))
        {
            return Err(Error::RustError(format!("Turso query {i} error: {err}")));
        }
        let result = entry
            .and_then(|e| e.get("response"))
            .and_then(|r| r.get("result"));
        out.push(result.map(decode_result).unwrap_or_default());
    }
    Ok(out)
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
