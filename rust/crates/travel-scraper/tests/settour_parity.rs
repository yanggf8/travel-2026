// Golden-file parity test for the settour parser.
//
// Runs the built `travel-scraper parse capture` against a real settour FIT
// capture fixture and asserts the emitted CanonicalOffer matches the
// known-correct values (the verified Turso record
// shaping_tour_group_offers.offer_id = settour-fit-kyoto-20260620-4n).
//
// This is the parity gate required before any Python settour parser is deleted.

use std::path::PathBuf;
use std::process::Command;

fn binary_path() -> PathBuf {
    // CARGO_BIN_EXE_<name> is set by cargo for integration tests of a bin crate.
    PathBuf::from(env!("CARGO_BIN_EXE_travel-scraper"))
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/settour-kyoto-20260620-capture.json")
}

#[test]
fn settour_capture_parses_to_known_offer() {
    let output = Command::new(binary_path())
        .arg("parse")
        .arg("capture")
        .arg(fixture_path())
        .arg("--source")
        .arg("settour")
        .output()
        .expect("failed to run travel-scraper");

    assert!(
        output.status.success(),
        "parse capture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let offers: serde_json::Value =
        serde_json::from_str(&stdout).expect("output is not valid JSON");
    let arr = offers.as_array().expect("output is not a JSON array");
    assert_eq!(arr.len(), 1, "expected exactly one offer");
    let o = &arr[0];

    // Known-correct values (settour-fit-kyoto-20260620-4n).
    assert_eq!(o["source_id"], "settour");
    assert_eq!(o["package_type"], "fit");
    assert_eq!(o["dates"]["departure_date"], "2026-06-20");
    assert_eq!(o["dates"]["return_date"], "2026-06-24");
    assert_eq!(o["dates"]["duration_nights"], 4);
    assert_eq!(o["price"]["price_total"], 36587);
    assert_eq!(o["price"]["per_person"], 18294);
    assert_eq!(o["price"]["currency"], "TWD");
    assert_eq!(o["flight"]["outbound"]["flight_number"], "IT212");
    assert_eq!(o["flight"]["return"]["flight_number"], "IT211");
    assert_eq!(o["hotel"]["name"], "微笑飯店京都烏丸五條");
}
