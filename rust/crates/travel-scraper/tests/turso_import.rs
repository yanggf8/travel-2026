// Turso import round-trip via the native Rust CLI — no JSON anywhere.
//
// Seeds a `captures` row, runs `parse capture <id>` (which parses + imports
// straight to the Turso offers table — no JSON file boundary), queries the
// imported offer back, asserts, and cleans up. Skips if creds absent.

use std::path::PathBuf;
use std::process::Command;

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_travel-scraper"))
}

const CAPTURE_ID: &str = "settour-import-roundtrip-test";
const RAW_TEXT: &str = "2026/06/20(六)~2026/06/24(三)\n飯店 Kyoto 入住：2026/06/20(六)   退房：2026/06/24(三)   (共4晚)修改入住日期\n微笑飯店京都烏丸五條\nSmile Hotel Kyoto karasumagojo\n台灣虎航IT212\n台灣虎航IT211\n機加酒未稅總價\n$36,587\n機票稅金 \n$4,404\n";

fn run(args: &[&str]) -> std::process::Output {
    Command::new(binary_path()).args(args).output().expect("run travel-scraper")
}

fn turso_creds() -> Option<(String, String)> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let body = loop {
        if let Ok(b) = std::fs::read_to_string(path.join(".env")) {
            break b;
        }
        if !path.pop() {
            return None;
        }
    };
    let mut url = None;
    let mut token = None;
    for line in body.lines() {
        if let Some(v) = line.strip_prefix("TURSO_URL=") {
            url = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("TURSO_TOKEN=") {
            token = Some(v.trim().to_string());
        }
    }
    Some((url?, token?))
}

#[tokio::test]
async fn rust_parse_import_round_trips_to_turso() {
    let Some((url, token)) = turso_creds() else {
        eprintln!("SKIP: no TURSO_URL/TURSO_TOKEN in .env — cannot round-trip cloud DB");
        return;
    };

    // Seed parser rules + a capture row.
    let seed_rules = run(&["parser", "rules", "seed-defaults"]);
    assert!(seed_rules.status.success(), "seed-defaults: {}", String::from_utf8_lossy(&seed_rules.stderr));
    let create = run(&[
        "db",
        "exec",
        "CREATE TABLE IF NOT EXISTS captures (capture_id TEXT PRIMARY KEY, source_id TEXT NOT NULL, url TEXT, title TEXT, captured_at TEXT NOT NULL, raw_text TEXT NOT NULL)",
    ]);
    assert!(create.status.success(), "create captures: {}", String::from_utf8_lossy(&create.stderr));
    let seed_sql = format!(
        "INSERT OR REPLACE INTO captures (capture_id, source_id, url, title, captured_at, raw_text) \
         VALUES ('{CAPTURE_ID}', 'settour', 'https://fit.settour.com.tw/product/v2?depDate=20260620,20260624', 't', '2026-06-06T00:00:00Z', '{}')",
        RAW_TEXT.replace('\'', "''")
    );
    let seed = run(&["db", "exec", &seed_sql]);
    assert!(seed.status.success(), "seed capture: {}", String::from_utf8_lossy(&seed.stderr));

    // Parse + import (no dry-run) → writes to the offers table, no JSON file.
    let import = run(&["parse", "capture", CAPTURE_ID, "--source", "settour"]);
    assert!(
        import.status.success(),
        "parse/import failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&import.stdout),
        String::from_utf8_lossy(&import.stderr)
    );

    // Query the imported offer back.
    let db = libsql::Builder::new_remote(url, token).build().await.expect("connect Turso");
    let conn = db.connect().expect("conn");
    let mut rows = conn
        .query(
            "SELECT source_id, type, price_per_person, currency, departure_date, return_date, nights, hotel_name, airline \
             FROM offers WHERE source_id = 'settour' AND departure_date = '2026-06-20' AND nights = 4 \
             ORDER BY scraped_at DESC LIMIT 1",
            (),
        )
        .await
        .expect("query imported offer");
    let row = rows.next().await.expect("row").expect("imported row exists");

    let source_id: String = row.get(0).unwrap();
    let kind: String = row.get(1).unwrap();
    let pp: i64 = row.get(2).unwrap();
    let currency: String = row.get(3).unwrap();
    let depart: String = row.get(4).unwrap();
    let ret: String = row.get(5).unwrap();
    let nights: i64 = row.get(6).unwrap();
    let hotel: String = row.get(7).unwrap();

    assert_eq!(source_id, "settour");
    assert_eq!(kind, "package");
    assert_eq!(pp, 18294);
    assert_eq!(currency, "TWD");
    assert_eq!(depart, "2026-06-20");
    assert_eq!(ret, "2026-06-24");
    assert_eq!(nights, 4);
    assert_eq!(hotel, "微笑飯店京都烏丸五條");
}
