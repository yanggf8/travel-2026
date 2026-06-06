mod common;

const TIGERAIR_CAPTURE_ID: &str = "tigerair-flight-rule-test";
const TIGERAIR_RAW_TEXT: &str = "出發日期 2026/06/21\n台灣虎航\nIT 200\n10:30\n14:00\nTWD 3,500\n";

const AGODA_CAPTURE_ID: &str = "agoda-hotel-rule-test";
const AGODA_RAW_TEXT: &str =
    "2026/06/21~2026/06/24\n3晚\n難波東方飯店 (Namba Oriental Hotel)\nNT$ 8,888\n";

#[test]
fn tigerair_flight_rule_parses_without_hotel_or_nights() {
    if !common::can_access_turso() || !common::seed_rules() {
        return;
    }
    if !common::seed_capture(
        TIGERAIR_CAPTURE_ID,
        "tigerair",
        "https://booking.tigerairtw.com/zh-TW/index",
        TIGERAIR_RAW_TEXT,
    ) {
        return;
    }
    let actual = common::parse_offer_line(TIGERAIR_CAPTURE_ID, "tigerair").expect("offer line");

    assert_eq!(actual.kind, "flight");
    assert_eq!(actual.depart, "2026-06-21");
    assert_eq!(actual.ret, "");
    assert_eq!(actual.nights, 0);
    assert_eq!(actual.pp, 3500);
    assert_eq!(actual.total, 3500);
    assert_eq!(actual.hotel, "");
    assert_eq!(actual.flight, "IT200");
    assert_eq!(actual.airline, "台灣虎航");
}

#[test]
fn verify_reports_field_status_plain_text() {
    if !common::can_access_turso() || !common::seed_rules() {
        return;
    }
    if !common::seed_capture(
        TIGERAIR_CAPTURE_ID,
        "tigerair",
        "https://booking.tigerairtw.com/zh-TW/index",
        TIGERAIR_RAW_TEXT,
    ) {
        return;
    }
    let out = common::run(&["verify", "tigerair", TIGERAIR_CAPTURE_ID]);
    assert!(
        out.status.success(),
        "verify failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("verify\ttigerair\t"));
    assert!(stdout.contains("rule_product_kind\tflight"));
    assert!(stdout.contains("field\tdepart_return\tOK\t2026-06-21→"));
    assert!(stdout.contains("field\tnights\tOK\tnot-required:product_kind=flight"));
    assert!(stdout.contains("field\thotel\tOK\tnot-required:product_kind=flight"));
    assert!(stdout.contains("overall\tOK\toffers=1"));
}

#[test]
fn agoda_hotel_rule_parses_without_flight() {
    if !common::can_access_turso() || !common::seed_rules() {
        return;
    }
    if !common::seed_capture(
        AGODA_CAPTURE_ID,
        "agoda",
        "https://www.agoda.com/search?city=osaka",
        AGODA_RAW_TEXT,
    ) {
        return;
    }
    let actual = common::parse_offer_line(AGODA_CAPTURE_ID, "agoda").expect("offer line");

    assert_eq!(actual.kind, "hotel");
    assert_eq!(actual.depart, "2026-06-21");
    assert_eq!(actual.ret, "2026-06-24");
    assert_eq!(actual.nights, 3);
    assert_eq!(actual.pp, 8888);
    assert_eq!(actual.total, 8888);
    assert_eq!(actual.hotel, "難波東方飯店");
    assert_eq!(actual.flight, "");
}
