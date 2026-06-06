mod common;

const CAPTURE_ID: &str = "liontravel-parity-test";
const RAW_TEXT: &str = "2026/06/12~2026/06/16\n共4晚\n中華航空CI120\n中華航空CI121\n總金額\nTWD 37,108\n飯店\nHOTEL AZAT NAHA\n";

#[test]
fn liontravel_rule_parser_matches_cloud_db_record() {
    if !common::can_access_turso() || !common::seed_rules() {
        return;
    }
    if !common::seed_capture(
        CAPTURE_ID,
        "liontravel",
        "https://vacation.liontravel.com/detail/170531004",
        RAW_TEXT,
    ) {
        return;
    }
    let Some(rows) = common::query_rows(
        "SELECT depart_date, return_date, nights, price_per_person_twd, hotel_name, product_kind \
         FROM shaping_tour_group_offers WHERE offer_id = 'liontravel-170531004-oka-20260612-BOOKED'",
    ) else {
        return;
    };
    let exp = rows.first().expect("liontravel record exists in Turso");
    let actual = common::parse_offer_line(CAPTURE_ID, "liontravel").expect("offer line");

    assert_eq!(actual.depart, exp[0]);
    assert_eq!(actual.ret, exp[1]);
    assert_eq!(actual.nights, exp[2].parse::<i64>().unwrap());
    assert_eq!(actual.pp, exp[3].parse::<i64>().unwrap());
    assert_eq!(actual.total, 37108);
    assert_eq!(actual.kind, exp[5]);
    assert_eq!(actual.hotel, exp[4]);
}
