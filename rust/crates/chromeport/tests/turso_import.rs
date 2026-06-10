mod common;

const CAPTURE_ID: &str = "settour-import-roundtrip-test";
const RAW_TEXT: &str = "2026/06/20(六)~2026/06/24(三)\n飯店 Kyoto 入住：2026/06/20(六)   退房：2026/06/24(三)   (共4晚)修改入住日期\n微笑飯店京都烏丸五條\nSmile Hotel Kyoto karasumagojo\n台灣虎航IT212\n台灣虎航IT211\n機加酒未稅總價\n$36,587\n機票稅金 \n$4,404\n";

#[test]
fn rust_parse_import_round_trips_to_turso() {
    if !common::can_access_turso() || !common::seed_rules() {
        return;
    }
    if !common::seed_capture(
        CAPTURE_ID,
        "settour",
        "https://fit.settour.com.tw/product/v2?depDate=20260620,20260624",
        RAW_TEXT,
    ) {
        return;
    }

    let import = common::run(&["parse", "capture", CAPTURE_ID, "--source", "settour"]);
    assert!(
        import.status.success(),
        "parse/import failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&import.stdout),
        String::from_utf8_lossy(&import.stderr)
    );

    let Some(rows) = common::query_rows(
        "SELECT source_id, type, price_per_person, currency, departure_date, return_date, nights, hotel_name, airline \
         FROM offers WHERE source_id = 'settour' AND departure_date = '2026-06-20' AND nights = 4 \
         ORDER BY scraped_at DESC LIMIT 1",
    ) else {
        return;
    };
    let row = rows.first().expect("imported row exists");

    assert_eq!(row[0], "settour");
    assert_eq!(row[1], "package");
    assert_eq!(row[2], "18294");
    assert_eq!(row[3], "TWD");
    assert_eq!(row[4], "2026-06-20");
    assert_eq!(row[5], "2026-06-24");
    assert_eq!(row[6], "4");
    assert_eq!(row[7], "微笑飯店京都烏丸五條");
}
