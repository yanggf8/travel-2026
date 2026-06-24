//! Minimal bilingual UI strings for the dashboard chrome (section headings, etc.).
//! Keep this small — DB-sourced content (themes, focus, activities) is NOT translated
//! here; only the fixed UI labels the render layer emits live in this table.

/// Look up a UI string by key. `lang == "en"` → English, anything else → Traditional Chinese.
pub fn t(key: &str, lang: &str) -> &'static str {
    let zh = lang != "en";
    match (key, zh) {
        ("flights", false) => "Flights",
        ("flights", true) => "航班",
        ("hotel", false) => "Hotel",
        ("hotel", true) => "住宿",
        ("transfers", false) => "Transfers",
        ("transfers", true) => "交通",
        ("plans", false) => "Plans",
        ("plans", true) => "行程",
        ("details", false) => "Booking details",
        ("details", true) => "訂位明細",
        ("voucher", false) => "Hotel voucher (PDF)",
        ("voucher", true) => "住宿券 PDF",
        // pending-booking alerts (feature #3)
        ("bookBy", false) => "Book by",
        ("bookBy", true) => "預約期限",
        // transit cheat-sheet (feature #4) — ported from render.ts:24,50-51
        ("transitCheat", false) => "Transit Cheat Sheet",
        ("transitCheat", true) => "交通速查表",
        ("dailyTransit", false) => "Daily transit: ~\u{00a5}600-800/person",
        ("dailyTransit", true) => "每日交通：約\u{00a5}600-800/人",
        ("homeBase", false) => "Home base",
        ("homeBase", true) => "住宿",
        // GitHub OAuth auth pages
        ("signIn", false) => "Sign in with GitHub",
        ("signIn", true) => "使用 GitHub 登入",
        ("signedInAs", false) => "Signed in as",
        ("signedInAs", true) => "已登入",
        ("logout", false) => "Log out",
        ("logout", true) => "登出",
        ("notAuthorized", false) => "Not authorized",
        ("notAuthorized", true) => "未授權",
        ("notAuthorizedBody", false) => "This account does not have access to the owner dashboard.",
        ("notAuthorizedBody", true) => "此帳號無法存取擁有者儀表板。",
        ("badShare", false) => "Invalid share link",
        ("badShare", true) => "分享連結無效",
        ("badShareBody", false) => "This trip link needs a valid share token.",
        ("badShareBody", true) => "此行程連結需要有效的分享權杖。",
        ("ownerSignIn", false) => "Owner? Sign in",
        ("ownerSignIn", true) => "擁有者？登入",
        // map slots (feature: server-side missing detection)
        ("tripOverview", false) => "Trip overview",
        ("tripOverview", true) => "行程總覽",
        ("mapNotAvailable", false) => "Map not available yet",
        ("mapNotAvailable", true) => "地圖尚未產生",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn returns_language_specific_heading() {
        assert_eq!(t("flights", "en"), "Flights");
        assert_eq!(t("flights", "zh"), "航班");
        assert_eq!(t("transfers", "zh"), "交通");
        assert_eq!(t("plans", "en"), "Plans");
    }
    #[test]
    fn unknown_key_is_empty() {
        assert_eq!(t("nope", "en"), "");
    }
    #[test]
    fn voucher_label_is_bilingual() {
        assert_eq!(t("voucher", "en"), "Hotel voucher (PDF)");
        assert_eq!(t("voucher", "zh"), "住宿券 PDF");
    }
    #[test]
    fn alert_and_transit_keys_are_bilingual() {
        assert_eq!(t("bookBy", "en"), "Book by");
        assert_eq!(t("bookBy", "zh"), "預約期限");
        assert_eq!(t("transitCheat", "en"), "Transit Cheat Sheet");
        assert_eq!(t("transitCheat", "zh"), "交通速查表");
        assert_eq!(t("homeBase", "en"), "Home base");
        assert_eq!(t("homeBase", "zh"), "住宿");
        assert!(t("dailyTransit", "en").contains("Daily transit"));
        assert!(t("dailyTransit", "zh").contains("每日交通"));
    }
}
