//! `db fetch holidays` — port of scripts/fetch-taiwan-holidays.ts.
//!
//! Fetches the official Taiwan public-holiday calendar (中華民國政府行政機關辦公
//! 日曆表) published by DGPA (行政院人事行政總處) as CSV, and upserts it into the
//! Turso `holidays` table. The CSV URL is supplied as the first arg (same as the
//! TS script) — get it from https://data.gov.tw/dataset/14718. ANY year's DGPA
//! CSV works; the 西元日期 (Gregorian date) column drives the inserted year.
//!
//! CSV format:
//!   西元日期,星期,是否放假,備註
//!   20260619,五,2,端午節
//!
//! 是否放假: 0 = workday, 1 = makeup workday (補班日), 2 = holiday/weekend off.
//!
//! Repo policy: live fetch only — NO fabricated/hardcoded holiday data. If the
//! source is unreachable the command fails loud (it never falls back).

use libsql::params;
use std::process::Command;

const COUNTRY: &str = "taiwan";
const SOURCE_LABEL: &str = "DGPA 行政院人事行政總處 中華民國政府行政機關辦公日曆表";

struct CsvRow {
    date: String,         // YYYY-MM-DD
    day_of_week: String,  // 一/二/三/四/五/六/日
    is_holiday: i64,      // 0 / 1 / 2
    name: Option<String>, // 備註 (holiday name); empty → None
}

fn parse_csv(text: &str) -> Result<Vec<CsvRow>, String> {
    // Strip UTF-8 BOM if present.
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return Ok(Vec::new());
    }
    let header = lines[0];
    if !header.contains("西元日期") {
        return Err(format!(
            "Unexpected CSV header (expected '西元日期,星期,是否放假,備註'): {header}"
        ));
    }
    let mut rows = Vec::new();
    for line in &lines[1..] {
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() < 3 {
            continue;
        }
        let raw_date = cols[0].trim();
        if raw_date.len() != 8 || !raw_date.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let date = format!("{}-{}-{}", &raw_date[0..4], &raw_date[4..6], &raw_date[6..8]);
        let day_of_week = cols[1].trim().to_string();
        let is_holiday = match cols[2].trim().parse::<i64>() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let name_raw = cols.get(3).map(|s| s.trim()).unwrap_or("");
        let name = if name_raw.is_empty() {
            None
        } else {
            Some(name_raw.to_string())
        };
        rows.push(CsvRow {
            date,
            day_of_week,
            is_holiday,
            name,
        });
    }
    Ok(rows)
}

pub async fn run(args: &[String]) -> Result<(), String> {
    let url = args.first().ok_or_else(|| {
        "Usage: db fetch holidays <csv-url>\n  \
         Get the URL from: https://data.gov.tw/dataset/14718"
            .to_string()
    })?;

    println!("Fetching: {url}");
    // Live fetch via curl (matches weather.rs — deliberate for WSL2 DNS).
    let output = Command::new("curl")
        .arg("-sfL")
        .arg(url)
        .output()
        .map_err(|e| format!("DGPA CSV request failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "DGPA CSV request failed: curl exit {:?}",
            output.status.code()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    println!("Got {} bytes", text.len());

    let rows = parse_csv(&text)?;
    if rows.is_empty() {
        return Err("No rows parsed from CSV — check format.".to_string());
    }
    let first_year = &rows[0].date[0..4];
    let last_year = &rows[rows.len() - 1].date[0..4];
    println!(
        "Parsed {} day rows, covering {}..{}",
        rows.len(),
        first_year,
        last_year
    );

    let holiday_count = rows
        .iter()
        .filter(|r| r.is_holiday == 2 && r.name.is_some())
        .count();
    let weekend_count = rows
        .iter()
        .filter(|r| r.is_holiday == 2 && r.name.is_none())
        .count();
    let makeup_count = rows.iter().filter(|r| r.is_holiday == 1).count();
    let workday_count = rows.iter().filter(|r| r.is_holiday == 0).count();
    println!("  {holiday_count} named holidays");
    println!("  {weekend_count} weekend days off");
    println!("  {makeup_count} makeup workdays (補班)");
    println!("  {workday_count} regular workdays");

    let fetched_at = chrono::Utc::now().to_rfc3339();
    let conn = crate::db::connect_write().await?;

    println!("\nWriting {} rows to Turso...", rows.len());
    let total = rows.len();
    for (i, r) in rows.iter().enumerate() {
        let year: i64 = r.date[0..4].parse().unwrap_or(0);
        conn.execute(
            "INSERT OR REPLACE INTO holidays \
                (country, year, date, name, day_of_week, is_holiday, \
                 source_url, source_label, fetched_at, confidence) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                COUNTRY,
                year,
                r.date.clone(),
                r.name.clone(),
                r.day_of_week.clone(),
                r.is_holiday,
                url.clone(),
                SOURCE_LABEL,
                fetched_at.clone(),
                "authoritative",
            ],
        )
        .await
        .map_err(|e| format!("holidays INSERT failed for {}: {e}", r.date))?;

        if (i + 1) % 200 == 0 || i + 1 == total {
            println!("  {}/{}...", i + 1, total);
        }
    }
    println!("\u{2705} {total} holidays/days written.");

    println!("\nNamed holidays imported:");
    for r in rows.iter().filter(|r| r.is_holiday == 2 && r.name.is_some()) {
        println!(
            "  {} ({}) — {}",
            r.date,
            r.day_of_week,
            r.name.as_deref().unwrap_or("")
        );
    }

    Ok(())
}
