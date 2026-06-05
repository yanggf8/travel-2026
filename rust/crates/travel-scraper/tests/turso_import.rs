// Turso-backed import round trip for the native Rust CLI.
//
// No committed JSON fixture: the test builds one offer in memory, writes a temp
// input file only because the CLI contract is `import offers <offers.json>`,
// imports via the binary, queries the real Turso offers table, and cleans up.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_travel-scraper"))
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
async fn rust_import_offers_round_trips_to_turso() {
    let Some((url, token)) = turso_creds() else {
        eprintln!("SKIP: no TURSO_URL/TURSO_TOKEN in .env — cannot round-trip cloud DB");
        return;
    };

    let suffix = format!("{}-{}", std::process::id(), chrono_like_timestamp());
    let product_code = format!("rust-roundtrip-{suffix}");
    let offer_id = format!("rusttest_{product_code}_20260620_4n");
    let scraped_at = format!(
        "2026-06-05T18:{}Z",
        suffix.chars().take(2).collect::<String>()
    );
    let offer = serde_json::json!([{
        "source_id": "rusttest",
        "package_type": "fit",
        "url": format!("https://example.invalid/product/{product_code}"),
        "scraped_at": scraped_at,
        "dates": {
            "departure_date": "2026-06-20",
            "return_date": "2026-06-24",
            "duration_nights": 4
        },
        "price": {
            "per_person": 18294,
            "price_total": 36587,
            "currency": "TWD"
        },
        "flight": {
            "outbound": {
                "flight_number": "IT212",
                "airline": "台灣虎航",
                "date": "2026-06-20"
            },
            "return": {
                "flight_number": "IT211",
                "airline": "台灣虎航",
                "date": "2026-06-24"
            }
        },
        "hotel": { "name": "微笑飯店京都烏丸五條" }
    }]);

    let db = libsql::Builder::new_remote(url, token)
        .build()
        .await
        .expect("connect Turso");
    let conn = db.connect().expect("conn");
    conn.execute(
        "DELETE FROM offers WHERE id = ?1 AND scraped_at = ?2",
        libsql::params![offer_id.clone(), scraped_at.clone()],
    )
    .await
    .expect("pre-clean test row");

    let mut tmp = std::env::temp_dir();
    tmp.push(format!("travel-scraper-offers-{suffix}.json"));
    {
        let mut f = std::fs::File::create(&tmp).expect("create temp offers");
        f.write_all(serde_json::to_string(&offer).unwrap().as_bytes())
            .expect("write temp offers");
    }

    let output = Command::new(binary_path())
        .arg("import")
        .arg("offers")
        .arg(&tmp)
        .output()
        .expect("run travel-scraper import offers");
    let _ = std::fs::remove_file(&tmp);

    assert!(
        output.status.success(),
        "import offers failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut rows = conn
        .query(
            "SELECT source_id, type, price_per_person, currency, departure_date, return_date, nights, hotel_name, airline \
             FROM offers WHERE id = ?1 AND scraped_at = ?2",
            libsql::params![offer_id.clone(), scraped_at.clone()],
        )
        .await
        .expect("query imported offer");
    let row = rows
        .next()
        .await
        .expect("row result")
        .expect("imported row exists");

    let source_id: String = row.get(0).unwrap();
    let kind: String = row.get(1).unwrap();
    let pp: i64 = row.get(2).unwrap();
    let currency: String = row.get(3).unwrap();
    let depart: String = row.get(4).unwrap();
    let ret: String = row.get(5).unwrap();
    let nights: i64 = row.get(6).unwrap();
    let hotel: String = row.get(7).unwrap();
    let airline: String = row.get(8).unwrap();

    assert_eq!(source_id, "rusttest");
    assert_eq!(kind, "package");
    assert_eq!(pp, 18294);
    assert_eq!(currency, "TWD");
    assert_eq!(depart, "2026-06-20");
    assert_eq!(ret, "2026-06-24");
    assert_eq!(nights, 4);
    assert_eq!(hotel, "微笑飯店京都烏丸五條");
    assert_eq!(airline, "台灣虎航");

    conn.execute(
        "DELETE FROM offers WHERE id = ?1 AND scraped_at = ?2",
        libsql::params![offer_id, scraped_at],
    )
    .await
    .expect("cleanup test row");
}

fn chrono_like_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    now.to_string()
}
