use crate::db;
use crate::ota::common::{self, VALID_PARAM_KEYS};
use std::collections::HashMap;
use travel_db::ids::new_run_id;
use travel_db::repo::ota_jobs;

pub async fn run(args: &[String]) -> Result<(), String> {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if positional.len() < 2 {
        return Err(
            "Usage: travel ota enqueue <source_id> <product_type> \
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
    if !common::table_exists(&conn, "ota_sources", "source_id", source_id).await? {
        return Err(format!(
            "Error: source_id '{source_id}' not found in ota_sources"
        ));
    }
    if !common::table_exists(&conn, "product_types", "code", product_type).await? {
        return Err(format!(
            "Error: product_type '{product_type}' not found in product_types"
        ));
    }

    let job_id = new_run_id();
    let now = common::now_iso();
    ota_jobs::enqueue(
        &conn,
        &ota_jobs::EnqueueInput {
            job_id: job_id.clone(),
            source_id: source_id.to_string(),
            product_type: product_type.to_string(),
            params,
            now,
        },
    )
    .await?;

    println!("job_id\t{job_id}");
    println!("status\tqueued");
    println!("source_id\t{source_id}");
    println!("product_type\t{product_type}");
    Ok(())
}