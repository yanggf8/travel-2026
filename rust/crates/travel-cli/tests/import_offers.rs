use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

mod common;
use common::{bin, db_exec, is_credless, nanos, seed_plan, teardown_plan, Guard};

static IMPORT_OFFERS_LOCK: Mutex<()> = Mutex::new(());

fn seed_package_status(plan: &str, dest: &str) {
    db_exec(&format!(
        "INSERT OR REPLACE INTO process_statuses (plan_id, destination, process_id, status) \
           VALUES ('{plan}', '{dest}', 'process_3_4_packages', 'pending');"
    ))
    .expect("creds");
}

fn preseed_warning(plan: &str, dest: &str) {
    let sql = format!(
        "INSERT INTO plan_offer_warnings (plan_id, destination, warning_type, message) \
         VALUES ('{plan}', '{dest}', 'parse', 'preseeded warning must survive import');"
    );
    db_exec(&sql).expect("creds");
}

fn teardown(plan: &str, dest: &str, temp_file: &Path) {
    teardown_plan(plan, dest);
    let _ = fs::remove_file(temp_file);
}

fn write_scrape_file(path: &Path, offer_id: &str) {
    let scrape = json!({
        "offers": [
            {
                "id": offer_id,
                "source_id": "besttour",
                "type": "package",
                "name": "Behavior Lock Okinawa Package",
                "url": "https://www.besttour.com.tw/e_web/GO/L_GO_Type.asp?iMGRUP_CD=BT123456",
                "price_per_person": 32100,
                "price_total": 64200,
                "currency": "TWD",
                "availability": "available",
                "scraped_at": "2026-07-02T01:02:03Z",
                "hotel": {
                    "name": "Codex Test Hotel Naha",
                    "slug": "codex-test-hotel-naha",
                    "area": "Naha Kokusai-dori",
                    "star_rating": 4,
                    "room_type": "Twin",
                    "access": [
                        "5 min walk from Makishi Station",
                        "Airport limousine stop nearby"
                    ]
                },
                "flight": {
                    "outbound": {
                        "flight_number": "CI120",
                        "airline": "China Airlines",
                        "airline_code": "CI",
                        "departure_code": "TPE",
                        "departure_time": "08:00",
                        "arrival_code": "OKA",
                        "arrival_time": "10:30",
                        "date": "2026-09-04"
                    },
                    "return": {
                        "flight_number": "CI121",
                        "airline": "China Airlines",
                        "airline_code": "CI",
                        "departure_code": "OKA",
                        "departure_time": "11:30",
                        "arrival_code": "TPE",
                        "arrival_time": "12:10",
                        "date": "2026-09-08"
                    }
                },
                "includes": [
                    "Round-trip airfare",
                    "Hotel breakfast",
                    "Airport transfer"
                ],
                "date_pricing": {
                    "2026-09-04": {
                        "price": 32100,
                        "price_total": 64200,
                        "availability": "available",
                        "seats_remaining": 6,
                        "notes": "best date"
                    },
                    "2026-09-05": {
                        "price_per_person": 32900,
                        "price_total": 65800,
                        "availability": "limited",
                        "seats_remaining": 3
                    }
                },
                "best_value": {
                    "date": "2026-09-04",
                    "price_per_person": 32100,
                    "price_total": 64200
                }
            }
        ]
    });
    fs::write(path, serde_json::to_vec_pretty(&scrape).unwrap()).expect("write scrape json");
}

fn run_import(plan: &str, dest: &str, temp_file: &Path) -> (bool, String, String) {
    let out = Command::new(bin())
        .args([
            "import-offers",
            "--files",
            temp_file.to_str().expect("temp path is UTF-8"),
            "--dest",
            dest,
        ])
        .env("TRAVEL_PLAN_ID", plan)
        .output()
        .expect("run import-offers");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn import_offers_writes_full_offer_family_and_preserves_warnings() {
    let _lock = IMPORT_OFFERS_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    if db_exec("SELECT 1 AS n").is_none() {
        eprintln!("skipping import-offers test (no Turso creds)");
        return;
    }

    let tag = nanos();
    let plan = format!("zztest{tag}");
    let dest = format!("zztest_dest_{tag}");
    let offer_id = format!("zztest_offer_{tag}");
    let temp_file: PathBuf = std::env::temp_dir().join(format!("zztest_import_offers_{tag}.json"));

    teardown(&plan, &dest, &temp_file);
    let _g = Guard::new({
        let plan = plan.clone();
        let dest = dest.clone();
        let temp_file = temp_file.clone();
        move || teardown(&plan, &dest, &temp_file)
    });

    seed_plan(&plan, &dest, 0);
    seed_package_status(&plan, &dest);
    preseed_warning(&plan, &dest);
    write_scrape_file(&temp_file, &offer_id);

    let (ok, stdout, stderr) = run_import(&plan, &dest, &temp_file);
    if !ok && is_credless(&stderr) {
        eprintln!(
            "skipping import-offers test (no Turso creds): {}",
            stderr.trim()
        );
        return;
    }
    assert!(
        ok,
        "import-offers should succeed; stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains(&format!("Importing offers for {dest} from 1 file(s)...")),
        "stdout should include import header; stdout={stdout}"
    );
    assert!(
        stdout.contains("  besttour: 1 offer(s)")
            && stdout.contains("Saved to Turso (plan_offers)."),
        "stdout should include source count and save line; stdout={stdout}"
    );

    let offers = db_exec(&format!(
        "SELECT source_id || '|' || type || '|' || title || '|' || price_per_person || '|' || \
                currency || '|' || availability || '|' || url || '|' || scraped_at || '|' || \
                CASE WHEN product_code IS NULL THEN 'NULL' ELSE product_code END || '|' || \
                price_total AS v \
         FROM plan_offers \
         WHERE plan_id = '{plan}' AND destination = '{dest}' AND id = '{offer_id}'"
    ))
    .expect("creds");
    assert_eq!(
        offers.scalar().as_deref(),
        Some(
            "besttour|package|Behavior Lock Okinawa Package|32100|TWD|available|https://www.besttour.com.tw/e_web/GO/L_GO_Type.asp?iMGRUP_CD=BT123456|2026-07-02T01:02:03Z|NULL|64200"
        ),
        "plan_offers scalar fields should match scrape JSON and product_code must stay NULL; out={offers}"
    );

    let includes = db_exec(&format!(
        "SELECT sort_order || ':' || item AS li \
         FROM plan_offer_includes \
         WHERE plan_id = '{plan}' AND destination = '{dest}' AND offer_id = '{offer_id}' \
         ORDER BY sort_order"
    ))
    .expect("creds");
    assert_eq!(
        includes.column(),
        vec![
            "0:Round-trip airfare",
            "1:Hotel breakfast",
            "2:Airport transfer"
        ],
        "includes should persist in scrape order; out={includes}"
    );

    let flights = db_exec(&format!(
        "SELECT direction || ':' || flight_number || ':' || airline || ':' || \
                COALESCE(airline_code, 'NULL') || ':' || departure_code || ':' || \
                departure_time || ':' || arrival_code || ':' || arrival_time AS li \
         FROM plan_offer_flights \
         WHERE plan_id = '{plan}' AND destination = '{dest}' AND offer_id = '{offer_id}' \
         ORDER BY direction"
    ))
    .expect("creds");
    assert_eq!(
        flights.column(),
        vec![
            "outbound:CI120:China Airlines:CI:TPE:08:00:OKA:10:30",
            "return:CI121:China Airlines:CI:OKA:11:30:TPE:12:10"
        ],
        "both flight legs should persist; out={flights}"
    );

    let hotel = db_exec(&format!(
        "SELECT name || '|' || COALESCE(slug, 'NULL') || '|' || area || '|' || star_rating AS v \
         FROM plan_offer_hotels \
         WHERE plan_id = '{plan}' AND destination = '{dest}' AND offer_id = '{offer_id}'"
    ))
    .expect("creds");
    assert_eq!(
        hotel.scalar().as_deref(),
        Some("Codex Test Hotel Naha|codex-test-hotel-naha|Naha Kokusai-dori|4"),
        "hotel row should persist; out={hotel}"
    );

    let access = db_exec(&format!(
        "SELECT sort_order || ':' || line AS li \
         FROM plan_offer_hotel_access \
         WHERE plan_id = '{plan}' AND destination = '{dest}' AND offer_id = '{offer_id}' \
         ORDER BY sort_order"
    ))
    .expect("creds");
    assert_eq!(
        access.column(),
        vec![
            "0:5 min walk from Makishi Station",
            "1:Airport limousine stop nearby"
        ],
        "hotel access lines should persist in scrape order; out={access}"
    );

    let pricing = db_exec(&format!(
        "SELECT date || ':' || price || ':' || availability || ':' || COALESCE(seats_remaining, 'NULL') AS li \
         FROM plan_offer_date_pricing \
         WHERE plan_id = '{plan}' AND destination = '{dest}' AND offer_id = '{offer_id}' \
         ORDER BY date"
    ))
    .expect("creds");
    let pricing_rows = pricing.column();
    assert_eq!(
        pricing_rows,
        vec!["2026-09-04:32100:available:6", "2026-09-05:32900:limited:3"],
        "one date_pricing row should be written per scrape date; out={pricing}"
    );
    assert!(
        pricing_rows.len() >= 2,
        "test fixture must exercise multi-date pricing"
    );

    let best_value = db_exec(&format!(
        "SELECT best_date || ':' || best_price || ':' || currency AS v \
         FROM plan_offer_best_value \
         WHERE plan_id = '{plan}' AND destination = '{dest}' AND offer_id = '{offer_id}'"
    ))
    .expect("creds");
    assert_eq!(
        best_value.scalar().as_deref(),
        Some("2026-09-04:32100:TWD"),
        "best_value row should persist; out={best_value}"
    );

    let provenance = db_exec(&format!(
        "SELECT source_id || '|' || file_path || '|' || offer_count AS v \
         FROM plan_offer_provenance \
         WHERE plan_id = '{plan}' AND destination = '{dest}' AND source_id = 'besttour'"
    ))
    .expect("creds");
    assert_eq!(
        provenance.scalar().as_deref(),
        Some(format!("besttour|{}|1", temp_file.to_str().unwrap()).as_str()),
        "provenance should record source, scrape file path, and offer count; out={provenance}"
    );

    let warnings = db_exec(&format!(
        "SELECT COUNT(*) AS n \
         FROM plan_offer_warnings \
         WHERE plan_id = '{plan}' AND destination = '{dest}' \
           AND warning_type = 'parse' AND message = 'preseeded warning must survive import'"
    ))
    .expect("creds");
    assert_eq!(
        warnings.scalar().as_deref(),
        Some("1"),
        "import-offers must not delete pre-existing warnings; out={warnings}"
    );

    let p34 = db_exec(&format!(
        "SELECT status AS v \
         FROM process_statuses \
         WHERE plan_id = '{plan}' AND destination = '{dest}' \
           AND process_id = 'process_3_4_packages'"
    ))
    .expect("creds");
    assert_eq!(
        p34.scalar().as_deref(),
        Some("researched"),
        "P3_4 should move pending -> researched; out={p34}"
    );

    let op_runs = db_exec(&format!(
        "SELECT COUNT(*) AS n \
         FROM operation_runs \
         WHERE plan_id = '{plan}' AND command_type = 'import-offers'"
    ))
    .expect("creds");
    assert_eq!(
        op_runs.scalar().as_deref(),
        Some("1"),
        "exactly one import-offers operation_run should be written; out={op_runs}"
    );

    let version = db_exec(&format!(
        "SELECT version AS v FROM plans WHERE plan_id = '{plan}'"
    ))
    .expect("creds");
    assert_eq!(
        version.scalar().as_deref(),
        Some("1"),
        "plans.version should bump by one; out={version}"
    );
}