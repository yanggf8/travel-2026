#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Command, Output};

pub fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_chromeport"))
}

pub fn run(args: &[&str]) -> Output {
    Command::new(binary_path())
        .args(args)
        .output()
        .expect("run chromeport")
}

pub fn can_access_turso() -> bool {
    let out = run(&["db", "query", "SELECT 1 AS ok"]);
    if out.status.success() {
        true
    } else {
        eprintln!(
            "SKIP: Turso credential broker unavailable:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        false
    }
}

pub fn exec_sql(sql: &str) -> bool {
    let out = run(&["db", "exec", sql]);
    if out.status.success() {
        true
    } else {
        eprintln!(
            "SKIP: db exec failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        false
    }
}

pub fn seed_rules() -> bool {
    let out = run(&["parser", "rules", "seed-defaults"]);
    if out.status.success() {
        true
    } else {
        eprintln!(
            "SKIP: seed-defaults failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        false
    }
}

pub fn seed_capture(capture_id: &str, source_id: &str, url: &str, raw_text: &str) -> bool {
    if !exec_sql(
        "CREATE TABLE IF NOT EXISTS captures (capture_id TEXT PRIMARY KEY, source_id TEXT NOT NULL, url TEXT, title TEXT, captured_at TEXT NOT NULL, raw_text TEXT NOT NULL)",
    ) {
        return false;
    }
    let sql = format!(
        "INSERT OR REPLACE INTO captures (capture_id, source_id, url, title, captured_at, raw_text) \
         VALUES ('{}', '{}', '{}', 't', '2026-06-06T00:00:00Z', '{}')",
        escape_sql(capture_id),
        escape_sql(source_id),
        escape_sql(url),
        escape_sql(raw_text)
    );
    exec_sql(&sql)
}

pub fn query_rows(sql: &str) -> Option<Vec<Vec<String>>> {
    let out = run(&["db", "query", sql]);
    if !out.status.success() {
        eprintln!(
            "SKIP: db query failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(parse_table(&String::from_utf8_lossy(&out.stdout)))
}

pub fn parse_offer_line(capture_id: &str, source_id: &str) -> Option<PlainOffer> {
    let out = run(&[
        "parse",
        "capture",
        capture_id,
        "--source",
        source_id,
        "--dry-run",
    ]);
    if !out.status.success() {
        eprintln!(
            "parse capture failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().find(|line| line.starts_with(source_id))?;
    PlainOffer::from_line(line)
}

#[derive(Debug)]
pub struct PlainOffer {
    pub kind: String,
    pub depart: String,
    pub ret: String,
    pub nights: i64,
    pub pp: i64,
    pub total: i64,
    pub hotel: String,
    pub flight: String,
    pub airline: String,
}

impl PlainOffer {
    fn from_line(line: &str) -> Option<Self> {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 7 {
            return None;
        }
        let dates: Vec<&str> = fields[2].split('→').collect();
        let flight = fields
            .iter()
            .find_map(|field| field.strip_prefix("flight="))
            .unwrap_or("")
            .to_string();
        let airline = fields
            .iter()
            .find_map(|field| field.strip_prefix("airline="))
            .unwrap_or("")
            .to_string();
        Some(Self {
            kind: fields[1].to_string(),
            depart: dates.first().copied().unwrap_or("").to_string(),
            ret: dates.get(1).copied().unwrap_or("").to_string(),
            nights: fields[3].trim_end_matches('n').parse().ok()?,
            pp: fields[4].trim_start_matches("pp=").parse().ok()?,
            total: fields[5].trim_start_matches("total=").parse().ok()?,
            hotel: fields[6].to_string(),
            flight,
            airline,
        })
    }
}

fn parse_table(stdout: &str) -> Vec<Vec<String>> {
    let mut lines = stdout.lines().filter(|line| !line.trim().is_empty());
    let Some(first) = lines.next() else {
        return Vec::new();
    };
    if first.starts_with('(') {
        return Vec::new();
    }
    lines
        .take_while(|line| !line.starts_with('('))
        .map(|line| line.split('\t').map(str::to_string).collect())
        .collect()
}

fn escape_sql(value: &str) -> String {
    value.replace('\'', "''")
}
