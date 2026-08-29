use crate::db;
use crate::ota::common::{self, VALID_PARAM_KEYS};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::process::Command;
use std::thread;
use std::time::Duration;
use travel_db::ids::new_run_id;
use travel_db::repo::{
    origin, ota_jobs, ota_source_workflow, product_type_inputs,
};

const PARAM_FLAGS: &[&str] = &[
    "--depart",
    "--return",
    "--nights",
    "--pax",
    "--region-code",
    "--region-label",
    "--destination",
    "--origin",
    "--currency",
    "--rooms",
    "--hotel",
];

const RUN_USAGE: &str = "Usage: travel ota run --capture-only <source_id> <product_type> \
             [--destination <slug>] [--hotel <slug>] [--depart YYYY-MM-DD] [--return YYYY-MM-DD] \
             [--nights N] [--pax N] [--origin <code>] [--currency <code>] [--rooms N] \
             [--region-code C] [--region-label L]";

/// Pure URL template interpolation: replace every `{name}` from `params`.
pub fn resolve_url(template: &str, params: &BTreeMap<String, String>) -> Result<String, String> {
    let placeholders = find_placeholders(template);
    let mut missing: Vec<&str> = Vec::new();
    for name in &placeholders {
        if !params.contains_key(name) {
            missing.push(name.as_str());
        }
    }
    if !missing.is_empty() {
        return Err(format!("missing placeholders: {}", missing.join(", ")));
    }
    let mut result = template.to_string();
    for name in &placeholders {
        if let Some(value) = params.get(name) {
            result = result.replace(&format!("{{{name}}}"), value);
        }
    }
    Ok(result)
}

fn find_placeholders(template: &str) -> Vec<String> {
    let chars: Vec<char> = template.chars().collect();
    let mut names = Vec::new();
    let mut seen = BTreeSet::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            if j < chars.len() && chars[j] == '}' && j > i + 1 {
                let name: String = chars[i + 1..j].iter().collect();
                if seen.insert(name.clone()) {
                    names.push(name);
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    names
}

fn insert_param(map: &mut BTreeMap<String, String>, key: &str, value: String) -> Result<(), String> {
    if let Some(existing) = map.get(key) {
        if existing != &value {
            return Err(format!(
                "param collision on '{key}': existing={existing}, new={value}"
            ));
        }
    } else {
        map.insert(key.to_string(), value);
    }
    Ok(())
}

fn caller_value(job_params: &BTreeMap<String, String>, input_name: &str) -> Option<String> {
    let keys: &[&str] = match input_name {
        "depart" => &["depart_date", "depart"],
        "return" => &["return_date", "return"],
        _ => &[input_name],
    };
    for key in keys {
        if let Some(v) = job_params.get(*key) {
            return Some(v.clone());
        }
    }
    None
}

fn apply_common_aliases(map: &mut BTreeMap<String, String>) -> Result<(), String> {
    if let Some(depart) = map.get("depart_date").cloned() {
        insert_param(map, "depart", depart.clone())?;
        insert_param(map, "checkin", depart)?;
    }
    if let Some(depart) = map.get("depart").cloned() {
        insert_param(map, "checkin", depart)?;
    }
    if let Some(ret) = map.get("return_date").cloned() {
        insert_param(map, "return", ret)?;
    }
    if let Some(pax) = map.get("pax").cloned() {
        insert_param(map, "adults", pax)?;
    }
    Ok(())
}

async fn fill_common_inputs(
    conn: &libsql::Connection,
    contract: &[product_type_inputs::ProductTypeInputRow],
    job_params: &BTreeMap<String, String>,
    map: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let db_defaults = origin::default_origin_airport_and_currency(conn).await?;

    for row in contract.iter().filter(|r| r.input_class == "common") {
        let value = if let Some(v) = caller_value(job_params, &row.input_name) {
            Some(v)
        } else if row.default_source.as_deref() == Some("db") {
            match row.input_name.as_str() {
                "origin" => Some(db_defaults.airport.clone()),
                "currency" => Some(db_defaults.currency.clone()),
                other => {
                    return Err(format!(
                        "no DB default for common input '{other}' (product_type={})",
                        row.product_type
                    ));
                }
            }
        } else if row.default_source.as_deref() == Some("code") && row.input_name == "rooms" {
            Some("1".to_string())
        } else {
            None
        };

        let value = match value {
            Some(v) => v,
            None if row.required != 0 => {
                return Err(format!(
                    "missing required common input '{}' for product_type {}",
                    row.input_name, row.product_type
                ));
            }
            None => continue,
        };

        insert_param(map, &row.input_name, value)?;
    }

    apply_common_aliases(map)
}

async fn resolve_token_placeholders(
    conn: &libsql::Connection,
    source_id: &str,
    product_type: &str,
    url_template: &str,
    contract: &[product_type_inputs::ProductTypeInputRow],
    map: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let legal_roles: BTreeSet<String> = contract
        .iter()
        .filter(|r| r.input_class == "token_key")
        .map(|r| r.input_name.clone())
        .collect();

    for placeholder in find_placeholders(url_template) {
        if map.contains_key(&placeholder) {
            continue;
        }

        let registered_input_names =
            ota_source_workflow::url_param_input_names(conn, source_id, product_type, &placeholder)
                .await?;
        for input_name in &registered_input_names {
            if !legal_roles.contains(input_name) {
                return Err(format!(
                    "input_name '{input_name}' registered for url_param '{placeholder}' is not a declared token_key for product_type '{product_type}' (legal token_key roles: {})",
                    legal_roles
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }

        let mut hits: Vec<(String, String)> = Vec::new();
        for role in &legal_roles {
            let Some(value) = map.get(role).cloned() else {
                continue;
            };
            if let Some(url_value) = ota_source_workflow::url_param_value(
                conn,
                source_id,
                product_type,
                &placeholder,
                role,
                &value,
            )
            .await?
            {
                hits.push((role.clone(), url_value));
            }
        }

        match hits.len() {
            0 => {
                let roles: Vec<&str> = legal_roles.iter().map(|r| r.as_str()).collect();
                return Err(format!(
                    "no url_param value for '{placeholder}' (source={source_id}, product_type={product_type}, tried token_key roles: {})",
                    roles.join(", ")
                ));
            }
            1 => {
                insert_param(map, &placeholder, hits[0].1.clone())?;
            }
            _ => {
                let roles: Vec<&str> = hits.iter().map(|(r, _)| r.as_str()).collect();
                return Err(format!(
                    "ambiguous url_param resolution for '{placeholder}' (source={source_id}, product_type={product_type}): multiple token_key roles matched ({})",
                    roles.join(", ")
                ));
            }
        }
    }

    Ok(())
}

pub async fn run(args: &[String]) -> Result<(), String> {
    if !args.iter().any(|a| a == "--capture-only") {
        return Err(RUN_USAGE.to_string());
    }

    let positional = common::positionals(args, PARAM_FLAGS);
    if positional.len() < 2 {
        return Err(RUN_USAGE.to_string());
    }
    let source_id = positional[0].as_str();
    let product_type = positional[1].as_str();

    let mut params: HashMap<String, String> = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--depart" => {
                let v = args.get(i + 1).ok_or("missing value for --depart")?;
                params.insert("depart_date".to_string(), v.clone());
                i += 2;
            }
            "--return" => {
                let v = args.get(i + 1).ok_or("missing value for --return")?;
                params.insert("return_date".to_string(), v.clone());
                i += 2;
            }
            "--nights" => {
                let v = args.get(i + 1).ok_or("missing value for --nights")?;
                params.insert("nights".to_string(), v.clone());
                i += 2;
            }
            "--pax" => {
                let v = args.get(i + 1).ok_or("missing value for --pax")?;
                params.insert("pax".to_string(), v.clone());
                i += 2;
            }
            "--region-code" => {
                let v = args.get(i + 1).ok_or("missing value for --region-code")?;
                params.insert("region_code".to_string(), v.clone());
                i += 2;
            }
            "--region-label" => {
                let v = args.get(i + 1).ok_or("missing value for --region-label")?;
                params.insert("region_label".to_string(), v.clone());
                i += 2;
            }
            "--destination" => {
                let v = args.get(i + 1).ok_or("missing value for --destination")?;
                params.insert("destination".to_string(), v.clone());
                i += 2;
            }
            "--origin" => {
                let v = args.get(i + 1).ok_or("missing value for --origin")?;
                params.insert("origin".to_string(), v.clone());
                i += 2;
            }
            "--currency" => {
                let v = args.get(i + 1).ok_or("missing value for --currency")?;
                params.insert("currency".to_string(), v.clone());
                i += 2;
            }
            "--rooms" => {
                let v = args.get(i + 1).ok_or("missing value for --rooms")?;
                params.insert("rooms".to_string(), v.clone());
                i += 2;
            }
            "--hotel" => {
                let v = args.get(i + 1).ok_or("missing value for --hotel")?;
                params.insert("hotel".to_string(), v.clone());
                i += 2;
            }
            _ => i += 1,
        }
    }

    for key in params.keys() {
        if !VALID_PARAM_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "Error: invalid param key '{key}'; allowed: {}",
                VALID_PARAM_KEYS.join(", ")
            ));
        }
    }

    let conn = db::connect_write().await?;
    let job_id = new_run_id();
    let now = common::now_iso();
    let lease = common::lease_expires(&now, 900)?;

    ota_jobs::enqueue(
        &conn,
        &ota_jobs::EnqueueInput {
            job_id: job_id.clone(),
            source_id: source_id.to_string(),
            product_type: product_type.to_string(),
            params,
            now: now.clone(),
        },
    )
    .await?;

    let claimed = ota_jobs::claim_specific(&conn, &job_id, "ota-run", &now, &lease)
        .await?
        .ok_or_else(|| format!("Error: failed to claim job {job_id}"))?;

    let workflow = ota_source_workflow::get(&conn, source_id, product_type)
        .await?
        .ok_or_else(|| {
            format!("no workflow row for ({source_id},{product_type}); add one")
        })?;

    match workflow.nav_kind.as_str() {
        "get" => {
            let job_params: BTreeMap<String, String> = ota_jobs::get_params(&conn, &job_id)
                .await?
                .into_iter()
                .collect();

            let contract = product_type_inputs::list_for_type(&conn, product_type).await?;
            if contract.is_empty() {
                return Err(format!(
                    "no product_type_inputs contract for product_type '{product_type}'"
                ));
            }

            let mut map = job_params.clone();
            fill_common_inputs(&conn, &contract, &job_params, &mut map).await?;
            resolve_token_placeholders(
                &conn,
                source_id,
                product_type,
                &workflow.url_template,
                &contract,
                &mut map,
            )
            .await?;

            let url = resolve_url(&workflow.url_template, &map)?;

            let gwebcdb_dir =
                std::env::var("GWEBCDB_DIR").unwrap_or_else(|_| "/home/yanggf/b/gwebcdb".to_string());

            let nav_out = Command::new("python")
                .current_dir(&gwebcdb_dir)
                .arg("bridge/navigate.py")
                .arg(&url)
                .output()
                .map_err(|e| format!("failed to run navigate.py: {e}"))?;
            if !nav_out.status.success() {
                return Err(format!(
                    "navigate.py failed: {}",
                    String::from_utf8_lossy(&nav_out.stderr)
                ));
            }

            thread::sleep(Duration::from_millis(workflow.settle_ms as u64));

            let mut capture_cmd = Command::new("python");
            capture_cmd
                .current_dir(&gwebcdb_dir)
                .arg("bridge/ota_capture.py")
                .arg("--source")
                .arg(source_id);
            if let Some(ref contains) = workflow.capture_url_contains {
                capture_cmd.arg("--url-contains").arg(contains);
            }
            let capture_out = capture_cmd
                .output()
                .map_err(|e| format!("failed to run ota_capture.py: {e}"))?;
            if !capture_out.status.success() {
                return Err(format!(
                    "ota_capture.py failed: {}",
                    String::from_utf8_lossy(&capture_out.stderr)
                ));
            }

            let stdout = String::from_utf8_lossy(&capture_out.stdout);
            let capture_id = stdout
                .lines()
                .find_map(|l| l.strip_prefix("capture_id\t").map(|v| v.trim().to_string()))
                .ok_or_else(|| format!("ota_capture.py did not print capture_id; stdout={stdout}"))?;

            println!("job_id\t{}", claimed.job_id);
            println!("claim_token\t{}", claimed.claim_token);
            println!("capture_id\t{capture_id}");
            println!("source_id\t{source_id}");
            println!("product_type\t{product_type}");
            println!(
                "agent_extraction_note\t{}",
                workflow.agent_extraction_note.as_deref().unwrap_or("")
            );
            eprintln!(
                "Next: read raw_text with `travel ota show-capture {capture_id}` then `travel ota write-offers {} --capture {capture_id} --claim-token {} --tsv <path> --dest <slug>`",
                claimed.job_id, claimed.claim_token
            );
            Ok(())
        }
        other => Err(format!("nav_kind '{other}' has no registered strategy")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn resolve_url_fills_all_placeholders_for_verified_get_workflows() {
        let settour_template = "https://fit.settour.com.tw/product/v2?tripType=RT&directFlightOnly=true&roomQty=1&depAirportCode=TPE&arrAirportCode={dest_code}&depDate={depart},{return}&hotelCheckInDate={depart}&hotelCheckOutDate={return}&adtCount={pax}&chdCount=0&regionId={region_id}";
        let settour_params = map(&[
            ("dest_code", "TYO"),
            ("depart", "2026-09-01"),
            ("return", "2026-09-05"),
            ("pax", "2"),
            ("region_id", "295"),
        ]);
        assert_eq!(
            resolve_url(settour_template, &settour_params).expect("settour url"),
            "https://fit.settour.com.tw/product/v2?tripType=RT&directFlightOnly=true&roomQty=1&depAirportCode=TPE&arrAirportCode=TYO&depDate=2026-09-01,2026-09-05&hotelCheckInDate=2026-09-01&hotelCheckOutDate=2026-09-05&adtCount=2&chdCount=0&regionId=295"
        );

        let eztravel_template = "https://packages.eztravel.com.tw/roundtrip-TPE-{dest_code}?checkin={depart}&checkout={return}&adult={pax}&child=0";
        let eztravel_params = map(&[
            ("dest_code", "TYO"),
            ("depart", "2026-09-01"),
            ("return", "2026-09-05"),
            ("pax", "2"),
        ]);
        assert_eq!(
            resolve_url(eztravel_template, &eztravel_params).expect("eztravel url"),
            "https://packages.eztravel.com.tw/roundtrip-TPE-TYO?checkin=2026-09-01&checkout=2026-09-05&adult=2&child=0"
        );

        let besttour_template = "https://www.besttour.com.tw/e_web/search?v=//////{region_id}///////";
        let besttour_params = map(&[("region_id", "295")]);
        assert_eq!(
            resolve_url(besttour_template, &besttour_params).expect("besttour url"),
            "https://www.besttour.com.tw/e_web/search?v=//////295///////"
        );
    }

    #[test]
    fn resolve_url_errors_with_all_missing_placeholder_names() {
        let params = map(&[("depart", "2026-09-01")]);
        let err = resolve_url(
            "https://example.com/search?depart={depart}&return={return}&dest={dest_code}&region={region_id}",
            &params,
        )
        .expect_err("missing placeholders should error");

        assert!(err.contains("return"), "err={err}");
        assert!(err.contains("dest_code"), "err={err}");
        assert!(err.contains("region_id"), "err={err}");
    }

    #[test]
    fn resolve_url_without_placeholders_is_unchanged() {
        let params = map(&[("depart", "2026-09-01")]);
        let template = "https://example.com/static/path?x=1";
        assert_eq!(
            resolve_url(template, &params).expect("static url"),
            template
        );
    }

    #[test]
    fn standard_input_aliases_still_fill_depart_and_return_placeholders() {
        let mut params = map(&[
            ("depart_date", "2026-09-01"),
            ("return_date", "2026-09-05"),
            ("pax", "2"),
        ]);

        if let Some(depart) = params.get("depart_date").cloned() {
            insert_param(&mut params, "depart", depart).expect("depart alias");
        }
        if let Some(ret) = params.get("return_date").cloned() {
            insert_param(&mut params, "return", ret).expect("return alias");
        }

        assert_eq!(
            resolve_url(
                "https://example.com/search?depart={depart}&return={return}&pax={pax}",
                &params
            )
            .expect("aliased url"),
            "https://example.com/search?depart=2026-09-01&return=2026-09-05&pax=2"
        );
    }

    #[test]
    fn token_fill_uses_destination_derived_values_in_param_map() {
        let mut params = map(&[
            ("destination", "tokyo"),
            ("depart", "2026-09-01"),
            ("return", "2026-09-05"),
            ("pax", "2"),
        ]);

        insert_param(&mut params, "dest_code", "NRT".to_string()).expect("dest_code token");
        insert_param(&mut params, "region_id", "179900".to_string()).expect("region_id token");

        assert_eq!(
            resolve_url(
                "https://example.com/search?dest={dest_code}&region={region_id}&depart={depart}&return={return}&pax={pax}",
                &params
            )
            .expect("token-filled url"),
            "https://example.com/search?dest=NRT&region=179900&depart=2026-09-01&return=2026-09-05&pax=2"
        );
    }

    #[test]
    fn insert_param_rejects_conflicting_token_value() {
        let mut params = map(&[("dest_code", "TYO")]);

        let err = insert_param(&mut params, "dest_code", "NRT".to_string())
            .expect_err("conflicting token should fail");

        assert!(err.contains("dest_code"), "err={err}");
        assert!(err.contains("TYO"), "err={err}");
        assert!(err.contains("NRT"), "err={err}");
    }

    #[test]
    fn common_contract_aliases_and_defaults_resolve_provider_placeholders() {
        let mut params = map(&[
            ("depart_date", "2026-09-01"),
            ("return_date", "2026-09-05"),
            ("pax", "2"),
        ]);

        insert_param(&mut params, "origin", "TPE".to_string()).expect("db default origin");
        insert_param(&mut params, "currency", "TWD".to_string()).expect("db default currency");
        insert_param(&mut params, "rooms", "1".to_string()).expect("code default rooms");

        if let Some(depart) = params.get("depart_date").cloned() {
            insert_param(&mut params, "depart", depart.clone()).expect("depart alias");
            insert_param(&mut params, "checkin", depart).expect("checkin alias");
        }
        if let Some(ret) = params.get("return_date").cloned() {
            insert_param(&mut params, "return", ret.clone()).expect("return alias");
        }
        if let Some(pax) = params.get("pax").cloned() {
            insert_param(&mut params, "adults", pax).expect("adults alias");
        }

        assert_eq!(
            resolve_url(
                "https://example.com/search?from={origin}&checkin={checkin}&depart={depart}&return={return}&adults={adults}&rooms={rooms}&currency={currency}",
                &params
            )
            .expect("common-filled url"),
            "https://example.com/search?from=TPE&checkin=2026-09-01&depart=2026-09-01&return=2026-09-05&adults=2&rooms=1&currency=TWD"
        );
    }

    #[test]
    fn caller_common_values_override_db_and_code_defaults() {
        let mut params = map(&[
            ("origin", "KHH"),
            ("currency", "JPY"),
            ("rooms", "2"),
            ("depart_date", "2026-09-01"),
        ]);

        if !params.contains_key("origin") {
            insert_param(&mut params, "origin", "TPE".to_string()).expect("db default origin");
        }
        if !params.contains_key("currency") {
            insert_param(&mut params, "currency", "TWD".to_string()).expect("db default currency");
        }
        if !params.contains_key("rooms") {
            insert_param(&mut params, "rooms", "1".to_string()).expect("code default rooms");
        }
        if let Some(depart) = params.get("depart_date").cloned() {
            insert_param(&mut params, "depart", depart.clone()).expect("depart alias");
            insert_param(&mut params, "checkin", depart).expect("checkin alias");
        }

        assert_eq!(
            resolve_url(
                "https://example.com/search?from={origin}&checkin={checkin}&rooms={rooms}&currency={currency}",
                &params
            )
            .expect("caller overrides"),
            "https://example.com/search?from=KHH&checkin=2026-09-01&rooms=2&currency=JPY"
        );
    }

    #[test]
    fn required_common_input_missing_fails_loud_via_unresolved_placeholder() {
        let params = map(&[("destination", "tokyo")]);

        let err = resolve_url(
            "https://example.com/search?depart={depart}&dest={destination}",
            &params,
        )
        .expect_err("missing required common input should fail");

        assert!(err.contains("missing placeholders"), "err={err}");
        assert!(err.contains("depart"), "err={err}");
    }
}