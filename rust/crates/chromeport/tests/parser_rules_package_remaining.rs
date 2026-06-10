mod common;

const BESTTOUR_CAPTURE_ID: &str = "besttour-parity-test";
const BESTTOUR_RAW_TEXT: &str = "2026/06/12~2026/06/15\n4天\n去程\n2026/06/12\nIT230\n台灣虎航\n住宿\nWBF水之都那霸酒店\n售價: NT$ 16,888\n";

const TRAVEL4U_CAPTURE_ID: &str = "travel4u-parity-test";
const TRAVEL4U_RAW_TEXT: &str = "2026/06/21~2026/06/24\n4天\n虎航\nIT230\n住宿\n水之都那霸飯店 (Hotel AQUA CITTA Naha)\n售價: NT$ 16,199\n";

#[test]
fn besttour_rule_parser_matches_cloud_db_record() {
    if !common::can_access_turso() || !common::seed_rules() {
        return;
    }
    if !common::seed_capture(
        BESTTOUR_CAPTURE_ID,
        "besttour",
        "https://www.besttour.com.tw/detail/okinawa",
        BESTTOUR_RAW_TEXT,
    ) {
        return;
    }
    let Some(rows) = common::query_rows(
        "SELECT depart_date, return_date, nights, price_per_person_twd, hotel_name, product_kind \
         FROM shaping_tour_group_offers WHERE offer_id = 'besttour-okinawa-20260612-3n-mpnnpolq'",
    ) else {
        return;
    };
    let exp = rows.first().expect("besttour record exists in Turso");
    let actual = common::parse_offer_line(BESTTOUR_CAPTURE_ID, "besttour").expect("offer line");

    assert_eq!(actual.depart, exp[0]);
    assert_eq!(actual.ret, exp[1]);
    assert_eq!(actual.nights, exp[2].parse::<i64>().unwrap());
    assert_eq!(actual.pp, exp[3].parse::<i64>().unwrap());
    assert_eq!(actual.kind, exp[5]);
    assert_eq!(actual.hotel, exp[4]);
}

#[test]
fn travel4u_rule_parser_matches_cloud_db_record() {
    if !common::can_access_turso() || !common::seed_rules() {
        return;
    }
    if !common::seed_capture(
        TRAVEL4U_CAPTURE_ID,
        "travel4u",
        "https://www.travel4u.com.tw/group/product/okinawa",
        TRAVEL4U_RAW_TEXT,
    ) {
        return;
    }
    let Some(rows) = common::query_rows(
        "SELECT depart_date, return_date, nights, price_per_person_twd, hotel_name, product_kind \
         FROM shaping_tour_group_offers WHERE offer_id = 'travel4u-okinawa-20260621-3n-mpnp86ot'",
    ) else {
        return;
    };
    let exp = rows.first().expect("travel4u record exists in Turso");
    let actual = common::parse_offer_line(TRAVEL4U_CAPTURE_ID, "travel4u").expect("offer line");

    assert_eq!(actual.depart, exp[0]);
    assert_eq!(actual.ret, exp[1]);
    assert_eq!(actual.nights, exp[2].parse::<i64>().unwrap());
    assert_eq!(actual.pp, exp[3].parse::<i64>().unwrap());
    assert_eq!(actual.kind, exp[5]);
    assert_eq!(actual.hotel, exp[4]);
}
