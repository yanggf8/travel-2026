mod common;

const CAPTURE_ID: &str = "lifetour-parity-test";
const RAW_TEXT: &str = "2026/06/21~2026/06/24\n3天2夜\n虎航IT230\n虎航IT231\nNT$15,130元\n住宿\n沖繩那霸旭橋托麗芙特酒店 (Hotel Torifito Naha Asahibashi)\n";

#[test]
fn lifetour_rule_parser_matches_cloud_db_record() {
    if !common::can_access_turso() || !common::seed_rules() {
        return;
    }
    if !common::seed_capture(
        CAPTURE_ID,
        "lifetour",
        "https://tour.lifetour.com.tw/detail/okinawa",
        RAW_TEXT,
    ) {
        return;
    }
    let Some(rows) = common::query_rows(
        "SELECT depart_date, return_date, nights, price_per_person_twd, hotel_name, product_kind \
         FROM shaping_tour_group_offers WHERE offer_id = 'lifetour-okinawa-20260621-2n-mpnpatpt'",
    ) else {
        return;
    };
    let exp = rows.first().expect("lifetour record exists in Turso");
    let actual = common::parse_offer_line(CAPTURE_ID, "lifetour").expect("offer line");

    assert_eq!(actual.depart, exp[0]);
    assert_eq!(actual.ret, exp[1]);
    assert_eq!(actual.nights, exp[2].parse::<i64>().unwrap());
    assert_eq!(actual.pp, exp[3].parse::<i64>().unwrap());
    assert_eq!(actual.total, actual.pp * 2);
    assert_eq!(actual.kind, exp[5]);
    assert_eq!(actual.hotel, exp[4]);
}
