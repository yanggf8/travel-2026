//! D1 read-mirror pilot (OTA execution spec Phase G — "measure the libSQL↔D1 dialect delta").
//!
//! COMPARE-ONLY, OWNER-GATED, FLAG-GATED. This module NEVER serves live traffic from D1: it reads
//! the SAME 1–2 tables from BOTH Turso (the serving source, unchanged) and a D1 read-mirror, then
//! reports the delta as plain text. It is deliberately NOT wired into `turso::pipeline` so it can't
//! alter live query behavior (Codex advice, 2026-07-02).
//!
//! Inert unless BOTH are configured (runbook: docs/plans/2026-07-02-dashboard-d1-mirror-pilot.md):
//!   - var `D1_COMPARE_ENABLED = "1"`
//!   - a `[[d1_databases]] binding = "MIRROR_DB"` (provisioned via `wrangler d1 create`)
//! The route `/diag/d1-compare` (owner-only) returns 404 when the flag is off, so a normal deploy
//! with no D1 database behaves exactly as today.

use crate::turso::{self, Row};
use std::collections::BTreeMap;
use worker::{Env, Result};

/// The pilot table set: one core row-count-shaped table + one detail table the dashboard reads a lot.
/// Kept tiny on purpose — the goal is to surface SQL-dialect/type/ordering deltas, not to mirror the DB.
/// `plans` is the natural core (small, stable); `date_anchors` is a per-(plan,destination) detail read.
const PILOT_QUERIES: &[(&str, &str)] = &[
    (
        "plans",
        "SELECT plan_id, schema_version FROM plans ORDER BY plan_id",
    ),
    (
        "date_anchors",
        "SELECT plan_id, destination, start_date, end_date, days \
         FROM date_anchors ORDER BY plan_id, destination",
    ),
];

/// True only when the owner explicitly enabled the pilot AND a D1 mirror binding exists.
pub fn enabled(env: &Env) -> bool {
    let flag_on = env
        .var("D1_COMPARE_ENABLED")
        .map(|v| v.to_string() == "1")
        .unwrap_or(false);
    flag_on && env.d1("MIRROR_DB").is_ok()
}

/// Read `sql` from the D1 mirror and decode into the SAME `Row` shape Turso reads produce, so the two
/// backends are diffed field-by-field. D1's result columns are JSON objects keyed by column name.
async fn d1_rows(env: &Env, sql: &str) -> Result<Vec<Row>> {
    let db = env.d1("MIRROR_DB")?;
    let result = db.prepare(sql).all().await?;
    let raw: Vec<serde_json::Value> = result.results()?;
    Ok(raw
        .into_iter()
        .map(|v| {
            let mut row: Row = BTreeMap::new();
            if let Some(obj) = v.as_object() {
                for (k, val) in obj {
                    row.insert(k.clone(), val.clone());
                }
            }
            row
        })
        .collect())
}

/// Run every pilot query against Turso AND D1, return a plain-text delta report. Never fails the
/// request on a per-query error — it records the error in the report (the pilot IS the error signal).
pub async fn compare(env: &Env, turso_url: &str, turso_token: &str) -> String {
    let mut out = String::from("D1 read-mirror compare (Phase G pilot — compare-only, D1 never serves)\n");
    out.push_str("table            turso_rows  d1_rows  verdict\n");
    out.push_str("---------------- ----------  -------  ---------------------------------------------\n");

    let sqls: Vec<String> = PILOT_QUERIES.iter().map(|(_, q)| q.to_string()).collect();
    let turso_all = match turso::pipeline(turso_url, turso_token, &sqls).await {
        Ok(v) => v,
        Err(e) => return format!("{out}\nERROR: turso pipeline failed: {e}\n"),
    };

    for (i, (name, sql)) in PILOT_QUERIES.iter().enumerate() {
        let t_rows = turso_all.get(i).cloned().unwrap_or_default();
        let d_rows = match d1_rows(env, sql).await {
            Ok(r) => r,
            Err(e) => {
                out.push_str(&format!(
                    "{name:<16} {:>10}  {:>7}  D1 ERROR: {e}\n",
                    t_rows.len(),
                    "-"
                ));
                continue;
            }
        };
        let verdict = diff_verdict(&t_rows, &d_rows);
        out.push_str(&format!(
            "{name:<16} {:>10}  {:>7}  {verdict}\n",
            t_rows.len(),
            d_rows.len()
        ));
    }
    out.push_str(
        "\nNote: a mismatch is the SIGNAL, not a failure — it names the libSQL↔D1 dialect/type/order \
         delta to fix before D1 could ever serve reads. Turso remains the sole serving source.\n",
    );
    out
}

/// Compare two decoded result sets field-by-field: same length, same values in order.
/// Reports the first divergence (row index + differing key) so the delta is actionable.
fn diff_verdict(turso: &[Row], d1: &[Row]) -> String {
    if turso.len() != d1.len() {
        return format!("ROW COUNT DIFFERS ({} vs {})", turso.len(), d1.len());
    }
    for (idx, (tr, dr)) in turso.iter().zip(d1.iter()).enumerate() {
        // key set differs?
        let tkeys: Vec<&String> = tr.keys().collect();
        let dkeys: Vec<&String> = dr.keys().collect();
        if tkeys != dkeys {
            return format!("row {idx}: COLUMN SET DIFFERS (turso={tkeys:?} d1={dkeys:?})");
        }
        for (k, tv) in tr {
            match dr.get(k) {
                Some(dv) if values_equal(tv, dv) => {}
                Some(dv) => {
                    return format!("row {idx} key '{k}': VALUE DIFFERS (turso={tv} d1={dv})");
                }
                None => return format!("row {idx}: d1 missing key '{k}'"),
            }
        }
    }
    "MATCH".to_string()
}

/// Value equality that tolerates the number-vs-string representation delta the two backends may
/// produce for the same column (Turso pipeline decodes typed scalars; D1 returns JSON). Comparing
/// the string form catches real value differences while ignoring pure representation noise — a
/// representation-only difference is itself worth noting, but not as a "value differs" false alarm.
fn values_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    if a == b {
        return true;
    }
    scalar_string(a) == scalar_string(b)
}

fn scalar_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
