// Golden parity test for the generic Turso-rule parser — no JSON anywhere.
//
// Flow: seed a `captures` row in Turso (raw_text only), run
// `parse capture <id> --source settour --dry-run` (plain-text output), parse the
// plain-text offer line, and compare to the live Turso record
// shaping_tour_group_offers.offer_id = 'settour-fit-kyoto-20260620-4n'.
//
// Skips (does not fail) if Turso creds are absent.

use std::path::PathBuf;
use std::process::Command;

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_travel-scraper"))
}

const CAPTURE_ID: &str = "settour-parity-test";

// Minimal raw_text — the exact lines the generic settour rule reads, from a real
// settour FIT capture (6/20→6/24 京都). Single-quotes escaped for the SQL insert.
const RAW_TEXT: &str = "2026/06/20(六)~2026/06/24(三)\n飯店 Kyoto 入住：2026/06/20(六)   退房：2026/06/24(三)   (共4晚)修改入住日期\n微笑飯店京都烏丸五條\nSmile Hotel Kyoto karasumagojo\n台灣虎航IT212\n台灣虎航IT211\n機加酒未稅總價\n$36,587\n機票稅金 \n$4,404\n";

fn run(args: &[&str]) -> std::process::Output {
    Command::new(binary_path())
        .args(args)
        .output()
        .expect("run travel-scraper")
}

fn seed_capture_row() {
    // captures table is created on demand by scrape/insert_capture; ensure it
    // exists, then upsert the test row.
    let create = run(&[
        "db",
        "exec",
        "CREATE TABLE IF NOT EXISTS captures (capture_id TEXT PRIMARY KEY, source_id TEXT NOT NULL, url TEXT, title TEXT, captured_at TEXT NOT NULL, raw_text TEXT NOT NULL)",
    ]);
    assert!(
        create.status.success(),
        "create captures: {}",
        String::from_utf8_lossy(&create.stderr)
    );

    let sql = format!(
        "INSERT OR REPLACE INTO captures (capture_id, source_id, url, title, captured_at, raw_text) \
         VALUES ('{CAPTURE_ID}', 'settour', 'https://fit.settour.com.tw/product/v2', 't', '2026-06-06T00:00:00Z', '{}')",
        RAW_TEXT.replace('\'', "''")
    );
    let seed = run(&["db", "exec", &sql]);
    assert!(
        seed.status.success(),
        "seed capture: {}",
        String::from_utf8_lossy(&seed.stderr)
    );
}

fn seed_default_parser_rules() {
    let out = run(&["parser", "rules", "seed-defaults"]);
    assert!(
        out.status.success(),
        "seed-defaults: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run `parse capture <id> --dry-run` and parse the plain-text offer line:
///   source\tkind\tdepart→return\tNn\tpp=N\ttotal=N\thotel
fn parse_offer_plain() -> (String, String, String, i64, i64, i64, String) {
    let out = run(&[
        "parse",
        "capture",
        CAPTURE_ID,
        "--source",
        "settour",
        "--dry-run",
    ]);
    assert!(
        out.status.success(),
        "parse capture: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find(|l| l.starts_with("settour\t"))
        .unwrap_or_else(|| panic!("no offer line in output:\n{stdout}"));
    let f: Vec<&str> = line.split('\t').collect();
    assert!(f.len() >= 7, "unexpected offer line: {line}");
    let dates: Vec<&str> = f[2].split('→').collect();
    let nights: i64 = f[3].trim_end_matches('n').parse().unwrap();
    let pp: i64 = f[4].trim_start_matches("pp=").parse().unwrap();
    let total: i64 = f[5].trim_start_matches("total=").parse().unwrap();
    (
        f[1].to_string(),     // kind
        dates[0].to_string(), // depart
        dates[1].to_string(), // return
        nights,
        pp,
        total,
        f[6].to_string(), // hotel
    )
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
async fn settour_parser_matches_cloud_db_record() {
    let Some((url, token)) = turso_creds() else {
        eprintln!("SKIP: no TURSO_URL/TURSO_TOKEN in .env — cannot verify against cloud DB");
        return;
    };
    seed_default_parser_rules();
    seed_capture_row();

    // Expected: live from Turso.
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
    let row = rows
        .next()
        .await
        .expect("row")
        .expect("record exists in Turso");
    let exp_depart: String = row.get(0).unwrap();
    let exp_return: String = row.get(1).unwrap();
    let exp_nights: i64 = row.get(2).unwrap();
    let exp_pp: i64 = row.get(3).unwrap();
    let exp_hotel: String = row.get(4).unwrap();
    let exp_kind: String = row.get(5).unwrap();

    // Actual: generic parser, rules + capture from Turso, plain-text output.
    let (kind, depart, ret, nights, pp, total, hotel) = parse_offer_plain();

    assert_eq!(depart, exp_depart);
    assert_eq!(ret, exp_return);
    assert_eq!(nights, exp_nights);
    assert_eq!(pp, exp_pp);
    assert_eq!(kind, exp_kind);
    assert!(
        exp_hotel.starts_with(&hotel),
        "hotel mismatch: parser='{hotel}' vs turso='{exp_hotel}'"
    );
    assert_eq!(total, 36587);
}
