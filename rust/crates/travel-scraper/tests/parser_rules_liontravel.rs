// Second-OTA parity for the generic Turso-rule parser — no JSON anywhere.
//
// Seeds a `captures` row in Turso, runs `parse capture <id> --dry-run`
// (plain-text output), and compares to the verified LionTravel booked record in
// Turso. Skips if creds absent.

use std::path::PathBuf;
use std::process::Command;

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_travel-scraper"))
}

const CAPTURE_ID: &str = "liontravel-parity-test";
const RAW_TEXT: &str = "2026/06/12~2026/06/16\n共4晚\n中華航空CI120\n中華航空CI121\n總金額\nTWD 37,108\n飯店\nHOTEL AZAT NAHA\n";

fn run(args: &[&str]) -> std::process::Output {
    Command::new(binary_path()).args(args).output().expect("run travel-scraper")
}

fn seed_capture_row() {
    let create = run(&[
        "db",
        "exec",
        "CREATE TABLE IF NOT EXISTS captures (capture_id TEXT PRIMARY KEY, source_id TEXT NOT NULL, url TEXT, title TEXT, captured_at TEXT NOT NULL, raw_text TEXT NOT NULL)",
    ]);
    assert!(create.status.success(), "create captures: {}", String::from_utf8_lossy(&create.stderr));
    let sql = format!(
        "INSERT OR REPLACE INTO captures (capture_id, source_id, url, title, captured_at, raw_text) \
         VALUES ('{CAPTURE_ID}', 'liontravel', 'https://vacation.liontravel.com/detail/170531004', 't', '2026-06-06T00:00:00Z', '{}')",
        RAW_TEXT.replace('\'', "''")
    );
    let seed = run(&["db", "exec", &sql]);
    assert!(seed.status.success(), "seed capture: {}", String::from_utf8_lossy(&seed.stderr));
}

fn seed_default_parser_rules() {
    let out = run(&["parser", "rules", "seed-defaults"]);
    assert!(out.status.success(), "seed-defaults: {}", String::from_utf8_lossy(&out.stderr));
}

/// plain-text offer line: source\tkind\tdepart→return\tNn\tpp=N\ttotal=N\thotel
fn parse_offer_plain() -> (String, String, String, i64, i64, i64, String) {
    let out = run(&["parse", "capture", CAPTURE_ID, "--source", "liontravel", "--dry-run"]);
    assert!(out.status.success(), "parse capture: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find(|l| l.starts_with("liontravel\t"))
        .unwrap_or_else(|| panic!("no offer line:\n{stdout}"));
    let f: Vec<&str> = line.split('\t').collect();
    assert!(f.len() >= 7, "unexpected line: {line}");
    let dates: Vec<&str> = f[2].split('→').collect();
    (
        f[1].to_string(),
        dates[0].to_string(),
        dates[1].to_string(),
        f[3].trim_end_matches('n').parse().unwrap(),
        f[4].trim_start_matches("pp=").parse().unwrap(),
        f[5].trim_start_matches("total=").parse().unwrap(),
        f[6].to_string(),
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
async fn liontravel_rule_parser_matches_cloud_db_record() {
    let Some((url, token)) = turso_creds() else {
        eprintln!("SKIP: no TURSO_URL/TURSO_TOKEN in .env — cannot verify against cloud DB");
        return;
    };
    seed_default_parser_rules();
    seed_capture_row();

    let db = libsql::Builder::new_remote(url, token).build().await.expect("connect Turso");
    let conn = db.connect().expect("conn");
    let mut rows = conn
        .query(
            "SELECT depart_date, return_date, nights, price_per_person_twd, hotel_name, product_kind \
             FROM shaping_tour_group_offers WHERE offer_id = 'liontravel-170531004-oka-20260612-BOOKED'",
            (),
        )
        .await
        .expect("query");
    let row = rows.next().await.expect("row").expect("record exists in Turso");
    let exp_depart: String = row.get(0).unwrap();
    let exp_return: String = row.get(1).unwrap();
    let exp_nights: i64 = row.get(2).unwrap();
    let exp_pp: i64 = row.get(3).unwrap();
    let exp_hotel: String = row.get(4).unwrap();
    let exp_kind: String = row.get(5).unwrap();

    let (kind, depart, ret, nights, pp, total, hotel) = parse_offer_plain();

    assert_eq!(depart, exp_depart);
    assert_eq!(ret, exp_return);
    assert_eq!(nights, exp_nights);
    assert_eq!(pp, exp_pp);
    assert_eq!(total, 37108);
    assert_eq!(kind, exp_kind);
    assert_eq!(hotel, exp_hotel);
}
