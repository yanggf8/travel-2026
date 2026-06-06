mod common;

const CAPTURE_ID: &str = "settour-parity-test";
const RAW_TEXT: &str = "2026/06/20(六)~2026/06/24(三)\n飯店 Kyoto 入住：2026/06/20(六)   退房：2026/06/24(三)   (共4晚)修改入住日期\n微笑飯店京都烏丸五條\nSmile Hotel Kyoto karasumagojo\n台灣虎航IT212\n台灣虎航IT211\n機加酒未稅總價\n$36,587\n機票稅金 \n$4,404\n";

#[test]
fn settour_parser_matches_cloud_db_record() {
    if !common::can_access_turso() || !common::seed_rules() {
        return;
    }
    if !common::seed_capture(
        CAPTURE_ID,
        "settour",
        "https://fit.settour.com.tw/product/v2",
        RAW_TEXT,
    ) {
        return;
    }
    let Some(rows) = common::query_rows(
        "SELECT depart_date, return_date, nights, price_per_person_twd, hotel_name, product_kind \
         FROM shaping_tour_group_offers WHERE offer_id = 'settour-fit-kyoto-20260620-4n'",
    ) else {
        return;
    };
    let exp = rows.first().expect("settour record exists in Turso");
    let actual = common::parse_offer_line(CAPTURE_ID, "settour").expect("offer line");

    assert_eq!(actual.depart, exp[0]);
    assert_eq!(actual.ret, exp[1]);
    assert_eq!(actual.nights, exp[2].parse::<i64>().unwrap());
    assert_eq!(actual.pp, exp[3].parse::<i64>().unwrap());
    assert_eq!(actual.kind, exp[5]);
    assert!(
        exp[4].starts_with(&actual.hotel),
        "hotel mismatch: parser='{}' vs turso='{}'",
        actual.hotel,
        exp[4]
    );
    assert_eq!(actual.total, 36587);
}
