// Golden parity test for the settour parser — Turso-backed, no JSON fixture file.
//
// Input: a minimal travel-capture-v1 (raw_text only) built INLINE from the real
// settour FIT render (no committed JSON blob).
// Expected: read live from the cloud DB (Turso) — the verified record
// shaping_tour_group_offers.offer_id = 'settour-fit-kyoto-20260620-4n'.
//
// This both proves parser parity AND exercises the Rust→Turso path the no-npm
// end-state needs. Skips (does not fail) if Turso creds are absent, so the test
// is runnable offline without silently passing on missing data.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_travel-scraper"))
}

// Minimal travel-capture-v1 raw_text — the exact lines parse_settour reads,
// lifted verbatim from a real settour FIT capture (6/20→6/24 京都).
const INLINE_RAW_TEXT: &str = "\
2026/06/20(六)~2026/06/24(三)\n\
飯店 Kyoto 入住：2026/06/20(六)   退房：2026/06/24(三)   (共4晚)修改入住日期\n\
微笑飯店京都烏丸五條\n\
Smile Hotel Kyoto karasumagojo\n\
台灣虎航IT212\n\
台灣虎航IT211\n\
機加酒未稅總價\n\
$36,587\n\
機票稅金 \n\
$4,404\n";

fn inline_capture_json() -> String {
    let capture = serde_json::json!({
        "schema": "travel-capture-v1",
        "source_id": "settour",
        "captured_at": "2026-06-05T17:40:23Z",
        "url": "https://fit.settour.com.tw/product/v2?depDate=20260620,20260624&adtCount=2&displayName=京都",
        "title": "機加酒自由行搜尋推薦 | 東南旅遊",
        "raw_text": INLINE_RAW_TEXT,
        "links": [],
        "tables": [],
        "html": null,
        "screenshot_path": null
    });
    serde_json::to_string(&capture).unwrap()
}

fn run_parser_on_inline_capture() -> serde_json::Value {
    // parse capture takes a file path; write the inline capture to a temp file.
    let mut tmp = std::env::temp_dir();
    tmp.push(format!("settour-parity-{}.json", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp capture");
        f.write_all(inline_capture_json().as_bytes()).expect("write temp");
    }
    let output = Command::new(binary_path())
        .arg("parse")
        .arg("capture")
        .arg(&tmp)
        .arg("--source")
        .arg("settour")
        .output()
        .expect("run travel-scraper");
    let _ = std::fs::remove_file(&tmp);

    assert!(
        output.status.success(),
        "parse capture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let offers: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parser output is valid JSON");
    let arr = offers.as_array().expect("output is array");
    assert_eq!(arr.len(), 1, "expected one offer");
    arr[0].clone()
}

/// Read TURSO_URL/TURSO_TOKEN from the repo .env (../../.env relative to the crate).
fn turso_creds() -> Option<(String, String)> {
    let env_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.env");
    let body = std::fs::read_to_string(env_path).ok()?;
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
async fn settour_parser_matches_cloud_db_record() {
    let Some((url, token)) = turso_creds() else {
        eprintln!("SKIP: no TURSO_URL/TURSO_TOKEN in .env — cannot verify against cloud DB");
        return;
    };

    // Expected values: live from Turso.
    let db = libsql::Builder::new_remote(url, token)
        .build()
        .await
        .expect("connect Turso");
    let conn = db.connect().expect("conn");
    let mut rows = conn
        .query(
            "SELECT depart_date, return_date, nights, price_per_person_twd, hotel_name, product_kind \
             FROM shaping_tour_group_offers WHERE offer_id = 'settour-fit-kyoto-20260620-4n'",
            (),
        )
        .await
        .expect("query");
    let row = rows.next().await.expect("row result").expect("record exists in Turso");

    let exp_depart: String = row.get(0).unwrap();
    let exp_return: String = row.get(1).unwrap();
    let exp_nights: i64 = row.get(2).unwrap();
    let exp_pp: i64 = row.get(3).unwrap();
    let exp_hotel: String = row.get(4).unwrap();
    let exp_kind: String = row.get(5).unwrap();

    // Actual values: from the Rust parser.
    let o = run_parser_on_inline_capture();

    assert_eq!(o["dates"]["departure_date"], exp_depart);
    assert_eq!(o["dates"]["return_date"], exp_return);
    assert_eq!(o["dates"]["duration_nights"], exp_nights);
    assert_eq!(o["price"]["per_person"], exp_pp);
    assert_eq!(o["package_type"], exp_kind);
    // hotel_name in Turso has an English suffix; parser yields the ZH name — assert prefix match.
    let parsed_hotel = o["hotel"]["name"].as_str().unwrap();
    assert!(
        exp_hotel.starts_with(parsed_hotel),
        "hotel mismatch: parser='{parsed_hotel}' vs turso='{exp_hotel}'"
    );
    // total is parser-derived; sanity-check it's per_person*2.
    assert_eq!(o["price"]["price_total"], exp_pp * 2);
}
