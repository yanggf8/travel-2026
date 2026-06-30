use crate::db;
use crate::ota::common::{self, VALID_PARAM_KEYS};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::process::Command;
use std::thread;
use std::time::Duration;
use travel_db::ids::new_run_id;
use travel_db::repo::{ota_jobs, ota_source_workflow};

const PARAM_FLAGS: &[&str] = &[
    "--depart",
    "--return",
    "--nights",
    "--pax",
    "--region-code",
    "--region-label",
];

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

pub async fn run(args: &[String]) -> Result<(), String> {
    if !args.iter().any(|a| a == "--capture-only") {
        return Err(
            "Usage: travel ota run --capture-only <source_id> <product_type> \
             [--depart YYYY-MM-DD] [--return YYYY-MM-DD] [--nights N] [--pax N] \
             [--region-code C] [--region-label L]"
                .to_string(),
        );
    }

    let positional = common::positionals(args, PARAM_FLAGS);
    if positional.len() < 2 {
        return Err(
            "Usage: travel ota run --capture-only <source_id> <product_type> \
             [--depart YYYY-MM-DD] [--return YYYY-MM-DD] [--nights N] [--pax N] \
             [--region-code C] [--region-label L]"
                .to_string(),
        );
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

    let mut map: BTreeMap<String, String> = ota_jobs::get_params(&conn, &job_id)
        .await?
        .into_iter()
        .collect();

    if let Some(depart) = map.get("depart_date").cloned() {
        insert_param(&mut map, "depart", depart)?;
    }
    if let Some(ret) = map.get("return_date").cloned() {
        insert_param(&mut map, "return", ret)?;
    }

    if workflow.url_template.contains("{region_id}") {
        let region_label = map
            .get("region_label")
            .ok_or("missing region_label for {region_id} lookup")?;
        let rid = ota_source_workflow::region_id(&conn, source_id, product_type, region_label)
            .await?
            .ok_or_else(|| format!("no region_id for region_label={region_label}"))?;
        insert_param(&mut map, "region_id", rid)?;
    }

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
    Ok(())
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
}