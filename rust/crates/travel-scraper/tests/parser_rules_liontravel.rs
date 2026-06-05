// Second-OTA parity for the generic Turso-rule parser.
//
// No committed JSON fixture: the test builds a minimal travel-capture-v1 body
// inline, writes a temp input only because the CLI contract is file-based, and
// compares parser output to the verified LionTravel booked record in Turso.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_travel-scraper"))
}

const INLINE_RAW_TEXT: &str = "\
2026/06/12~2026/06/16\n\
共4晚\n\
中華航空CI120\n\
中華航空CI121\n\
總金額\n\
TWD 37,108\n\
飯店\n\
HOTEL AZAT NAHA\n";

fn inline_capture_json() -> String {
    let capture = serde_json::json!({
        "schema": "travel-capture-v1",
        "source_id": "liontravel",
        "captured_at": "2026-06-05T18:00:00Z",
        "url": "https://vacation.liontravel.com/detail/170531004?FromDate=20260612&Days=5&roomlist=2-0-0",
        "title": "LionTravel booked Okinawa FIT",
        "raw_text": INLINE_RAW_TEXT,
        "links": [],
        "tables": [],
        "html": null,
        "screenshot_path": null
    });
    serde_json::to_string(&capture).unwrap()
}

fn run_parser_on_inline_capture() -> serde_json::Value {
    let mut tmp = std::env::temp_dir();
    tmp.push(format!(
        "liontravel-rule-parity-{}.json",
        std::process::id()
    ));
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp capture");
        f.write_all(inline_capture_json().as_bytes())
            .expect("write temp");
    }
    let output = Command::new(binary_path())
        .arg("parse")
        .arg("capture")
        .arg(&tmp)
        .arg("--source")
        .arg("liontravel")
        .output()
        .expect("run travel-scraper");
    let _ = std::fs::remove_file(&tmp);

    assert!(
        output.status.success(),
        "parse capture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let offers: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parser output is valid JSON");
    let arr = offers.as_array().expect("output is array");
    assert_eq!(arr.len(), 1, "expected one offer");
    arr[0].clone()
}

fn seed_default_parser_rules() {
    let output = Command::new(binary_path())
        .arg("parser")
        .arg("rules")
        .arg("seed-defaults")
        .output()
        .expect("run travel-scraper parser rules seed-defaults");
    assert!(
        output.status.success(),
        "parser rules seed-defaults failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn turso_creds() -> Option<(String, String)> {
    let body = read_repo_env()?;
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

fn read_repo_env() -> Option<String> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = path.join(".env");
        if let Ok(body) = std::fs::read_to_string(candidate) {
            return Some(body);
        }
        if !path.pop() {
            break;
        }
    }
    None
}

#[tokio::test]
async fn liontravel_rule_parser_matches_cloud_db_record() {
    let Some((url, token)) = turso_creds() else {
        eprintln!("SKIP: no TURSO_URL/TURSO_TOKEN in .env — cannot verify against cloud DB");
        return;
    };
    seed_default_parser_rules();

    let db = libsql::Builder::new_remote(url, token)
        .build()
        .await
        .expect("connect Turso");
    let conn = db.connect().expect("conn");
    let mut rows = conn
        .query(
            "SELECT depart_date, return_date, nights, price_per_person_twd, hotel_name, product_kind \
             FROM shaping_tour_group_offers WHERE offer_id = 'liontravel-170531004-oka-20260612-BOOKED'",
            (),
        )
        .await
        .expect("query");
    let row = rows
        .next()
        .await
        .expect("row result")
        .expect("record exists in Turso");

    let exp_depart: String = row.get(0).unwrap();
    let exp_return: String = row.get(1).unwrap();
    let exp_nights: i64 = row.get(2).unwrap();
    let exp_pp: i64 = row.get(3).unwrap();
    let exp_hotel: String = row.get(4).unwrap();
    let exp_kind: String = row.get(5).unwrap();

    let o = run_parser_on_inline_capture();
    println!(
        "generic_liontravel_output\t{}",
        serde_json::to_string(&o).unwrap()
    );

    assert_eq!(o["dates"]["departure_date"], exp_depart);
    assert_eq!(o["dates"]["return_date"], exp_return);
    assert_eq!(o["dates"]["duration_nights"], exp_nights);
    assert_eq!(o["price"]["per_person"], exp_pp);
    assert_eq!(o["price"]["price_total"], 37108);
    assert_eq!(o["package_type"], exp_kind);
    assert_eq!(o["hotel"]["name"], exp_hotel);
    assert_eq!(o["flight"]["outbound"]["flight_number"], "CI120");
    assert_eq!(o["flight"]["return"]["flight_number"], "CI121");
    assert_eq!(o["flight"]["outbound"]["airline"], "中華航空");
}
