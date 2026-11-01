//! Bilingual UI strings for the dashboard chrome: headings, captions, labels,
//! session names, units. EVERY interface string the render layer emits lives here
//! as a ZH/EN pair — never a CJK literal in render code. Trip/world CONTENT
//! (places, notes, advice texts, agency names) lives in Turso, not here; keyword
//! lists the worker matches data against stay in the code that matches them.

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
        ("domesticStay", false) => "Accommodation",
        ("domesticStay", true) => "住宿",
        ("candidates", false) => "Sea-View Candidates",
        ("candidates", true) => "海景候選",
        ("seaView", false) => "Sea view",
        ("seaView", true) => "海景",
        // Per-room bathtub fact. NULL (未查) renders NO tag — unverified ≠ 無浴缸.
        ("bathtub", false) => "Bathtub",
        ("bathtub", true) => "浴缸",
        ("noBathtub", false) => "No bathtub",
        ("noBathtub", true) => "無浴缸",
        ("breakfast", false) => "Breakfast included",
        ("breakfast", true) => "含早餐",
        // candidate gallery + external room-types link (domestic accommodations)
        ("moreRoomTypes", false) => "Rooms & availability ↗",
        ("moreRoomTypes", true) => "查看房型與空房 ↗",
        ("bookedBadge", false) => "Booked",
        ("bookedBadge", true) => "已訂",
        // Silence is not the same as "no breakfast": Taiwanese guesthouses often
        // include one, so an omitted tag was being read as "included".
        ("noBreakfast", false) => "No breakfast",
        ("noBreakfast", true) => "不含早餐",
        // Silence about cancellation reads as "same as the others". It is not:
        // an empty free_cancel_until means nobody checked.
        ("cancelUnknown", false) => "cancellation policy not checked",
        ("cancelUnknown", true) => "取消政策未查",
        ("perNight", false) => "per room / night",
        ("perNight", true) => "每房每晚",
        ("notBookedYet", false) => "Nothing booked yet — comparing {n} stays",
        ("notBookedYet", true) => "尚未預訂 · {n} 間比較中",
        // Comparison block: explicit ranking with reasons (user request 2026-09-28 —
        // the section must say which candidates, their pros/cons, and the order).
        ("recommendOrder", false) => "Recommended order",
        ("recommendOrder", true) => "推薦排序",
        ("firstChoice", false) => "Top pick",
        ("firstChoice", true) => "首選",
        ("prosConsLabel", false) => "Pros & cons",
        ("prosConsLabel", true) => "優劣比較",
        ("notRanked", false) => "Not in the recommendation",
        ("notRanked", true) => "未列入推薦",
        // Criteria line under the candidates heading — the denominator behind
        // 「N 間比較中」 (request 2026-09-28). {dates} is filled from the plan's
        // date anchors; the requirement enumeration is section chrome (same
        // precedent as the 「海景候選」 heading).
        ("criteriaLine", false) => "Requirements: 2 guests · bathtub + sea view + free parking (self-drive) · {dates}",
        ("criteriaLine", true) => "條件：2 人 · 浴缸＋海景＋免費停車（自駕） · {dates}",
        // candidate location minimap (ArcGIS static basemap, pin = the stay)
        ("locationMap", false) => "Location",
        ("locationMap", true) => "位置",
        ("openMap", false) => "Open in Google Maps",
        ("openMap", true) => "在 Google 地圖開啟",
        ("plans", false) => "Plans",
        ("plans", true) => "行程",
        ("tabUpcoming", false) => "Upcoming",
        ("tabUpcoming", true) => "計畫與進行中",
        ("tabPast", false) => "Past",
        ("tabPast", true) => "已過",
        ("noPlansThisTab", false) => "No plans in this tab yet.",
        ("noPlansThisTab", true) => "這個分頁尚無行程。",
        ("details", false) => "Booking details",
        ("details", true) => "訂位明細",
        ("voucher", false) => "Hotel voucher (PDF)",
        ("voucher", true) => "住宿券 PDF",
        // package offer (booking-summary package row) — ported from render.ts:1081,1086
        ("package", false) => "Package",
        ("package", true) => "套裝",
        ("perPerson", false) => "/person",
        ("perPerson", true) => "/人",
        ("forTwo", false) => "for 2",
        ("forTwo", true) => "兩人",
        // hotel access lines (transit directions to the hotel)
        ("hotelAccess", false) => "Access",
        ("hotelAccess", true) => "交通方式",
        // per-day map landmarks
        ("landmarks", false) => "Stops on the map",
        ("landmarks", true) => "地圖景點",
        // Japan-only entry info (render.ts:1206-1215) — shown when currency = JPY
        ("japanEntry", false) => "Japan Entry",
        ("japanEntry", true) => "日本入境申請",
        ("visitJapanWeb", false) => "Visit Japan Web",
        ("visitJapanWeb", true) => "Visit Japan Web",
        ("japanTourism", false) => "Visit Japan",
        ("japanTourism", true) => "日本觀光局",
        ("japanTourismSub", false) => "Japan Tourism Agency",
        ("japanTourismSub", true) => "日本觀光局官網",
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
        ("oauthError", false) => "The sign-in session expired or did not complete. Please sign in again.",
        ("oauthError", true) => "登入程序未完成或已逾時,請重新登入。",
        ("badShare", false) => "Invalid share link",
        ("badShare", true) => "分享連結無效",
        ("badShareBody", false) => "This trip link needs a valid share token.",
        ("badShareBody", true) => "此行程連結需要有效的分享權杖。",
        ("ownerSignIn", false) => "Owner? Sign in",
        ("ownerSignIn", true) => "擁有者？登入",
        // map slots (feature: server-side missing detection)
        ("tripOverview", false) => "Sightseeing overview",
        ("tripOverview", true) => "景點總覽",
        ("planLogisticsMap", false) => "Hotels & airports",
        ("planLogisticsMap", true) => "住宿與機場",
        ("planExcursionMap", false) => "Day trip",
        ("planExcursionMap", true) => "一日遊",
        ("planAirportMap", false) => "Airport days",
        ("planAirportMap", true) => "機場來回",
        ("mapNotAvailable", false) => "Map not available yet",
        ("mapNotAvailable", true) => "地圖尚未產生",
        // owner share-link copy (logged-in owner; recipients open ?token= link, no login)
        ("copyShareLink", false) => "Copy share link",
        ("copyShareLink", true) => "複製分享連結",
        ("noShareLink", false) => "No share link yet — run ./bin/travel share-token",
        ("noShareLink", true) => "尚無分享連結 — 請執行 ./bin/travel share-token",
        ("createGrantToken", false) => "Create grant token",
        ("createGrantToken", true) => "建立授權連結",
        ("grantTokens", false) => "Grant tokens",
        ("grantTokens", true) => "授權連結",
        ("makeInactive", false) => "Make inactive",
        ("makeInactive", true) => "停用",
        ("inactive", false) => "Inactive",
        ("inactive", true) => "已停用",
        ("noActiveGrantToken", false) => "No active grant token",
        ("noActiveGrantToken", true) => "目前沒有有效授權連結",
        ("created", false) => "Created",
        ("created", true) => "建立",
        ("inactivated", false) => "Inactivated",
        ("inactivated", true) => "停用",
        // ---- day card chrome (render/day.rs) ----
        ("feelsLike", false) => "Feels",
        ("feelsLike", true) => "體感",
        ("todaysRoute", false) => "Today's route",
        ("todaysRoute", true) => "今日路線",
        ("dayHeading", false) => "Day {day} · {date}",
        ("dayHeading", true) => "第 {day} 天 · {date}",
        // approximate leg / transfer duration
        ("aboutMinutes", false) => "~{n} min",
        ("aboutMinutes", true) => "約 {n} 分鐘",
        // punctuation that differs by language: label colon, quoted title,
        // joiner between two labels merged onto one pin
        ("labelColon", false) => ": ",
        ("labelColon", true) => "：",
        ("quotedTitle", false) => "\u{201C}{title}\u{201D}",
        ("quotedTitle", true) => "「{title}」",
        ("labelJoin", false) => " / ",
        ("labelJoin", true) => "／",
        // session names (render/session.rs)
        ("sessionMorning", false) => "Morning",
        ("sessionMorning", true) => "上午",
        ("sessionNoon", false) => "Noon",
        ("sessionNoon", true) => "中午",
        ("sessionAfternoon", false) => "Afternoon",
        ("sessionAfternoon", true) => "下午",
        ("sessionEvening", false) => "Evening",
        ("sessionEvening", true) => "晚上",
        // ---- map captions (render/map.rs) — {day} = day number ----
        ("dayRouteCaption", false) => "Day {day} route",
        ("dayRouteCaption", true) => "第 {day} 天路線",
        ("dayLabel", false) => "Day {day}",
        ("dayLabel", true) => "第 {day} 天",
        ("dayHotelRoundTrip", false) => "Day {day} hotel round trip",
        ("dayHotelRoundTrip", true) => "第 {day} 天・旅社來回",
        ("dayAway", false) => "Day {day} beyond the area",
        ("dayAway", true) => "第 {day} 天・本區以外的行程",
        // ---- companion options + AI badge (render/mod.rs) ----
        ("companionOptions", false) => "Companion options · pick one",
        ("companionOptions", true) => "同行方案 · 可選擇",
        ("companionMapAlt", false) => "Companion options route map",
        ("companionMapAlt", true) => "同行方案路線圖",
        ("companionUnconfirmed", false) => "unconfirmed, for discussion",
        ("companionUnconfirmed", true) => "尚未確認，僅供討論",
        ("aiRecBadge", false) => "\u{1F916} AI-recommended (unconfirmed)",
        ("aiRecBadge", true) => "\u{1F916} 建議（未確認）",
        // ---- owner share-link manager (render/share.rs) ----
        ("shareLinks", false) => "Share links",
        ("shareLinks", true) => "授權連結",
        ("noActiveShareLinks", false) => "No active share links.",
        ("noActiveShareLinks", true) => "目前沒有現用連結。",
        ("activeCount", false) => "active",
        ("activeCount", true) => "現用",
        ("deactivated", false) => "deactivated",
        ("deactivated", true) => "已停用",
        ("deactivatedAt", false) => "deactivated at",
        ("deactivatedAt", true) => "停用於",
        ("deactivateSelected", false) => "Deactivate selected",
        ("deactivateSelected", true) => "停用所選",
        // copy-button flash text (inline owner JS, filled into share::copy_script)
        ("copied", false) => "Copied!",
        ("copied", true) => "已複製！",
        ("copyFailed", false) => "Copy failed",
        ("copyFailed", true) => "複製失敗",
        // ---- booking summary (render/summary.rs) ----
        ("fitRecommended", false) => "Recommended",
        ("fitRecommended", true) => "推薦",
        ("fitLowest", false) => "Lowest",
        ("fitLowest", true) => "最低價",
        ("fitPriceDelta", false) => "+{currency} {amount} / person",
        ("fitPriceDelta", true) => "貴 {currency} {amount}／人",
        ("fitBookedHeading", false) => "Booked FIT package",
        ("fitBookedHeading", true) => "已訂 FIT 方案",
        ("fitCompareHeading", false) => "FIT options (preferred comparison)",
        ("fitCompareHeading", true) => "FIT 方案比較（優先評估，非已訂）",
        ("offersUnit", false) => "offers",
        ("offersUnit", true) => "個方案",
        // FIT offer card
        ("fitPricePerPerson", false) => "{currency} {amount} / person",
        ("fitPricePerPerson", true) => "{currency} {amount}／人",
        ("priceUnconfirmed", false) => "Price unconfirmed",
        ("priceUnconfirmed", true) => "價格未確認",
        ("dateUnconfirmed", false) => "Dates unconfirmed",
        ("dateUnconfirmed", true) => "日期未確認",
        ("departNoReturn", false) => "From {date} (return unconfirmed)",
        ("departNoReturn", true) => "{date} 起（回程未確認）",
        ("fitOneNight", false) => "{n} night",
        ("fitOneNight", true) => "{n} 晚",
        ("fitNights", false) => "{n} nights",
        ("fitNights", true) => "{n} 晚",
        ("fitAgency", false) => "Agency",
        ("fitAgency", true) => "旅行社",
        ("fitFlights", false) => "Flights",
        ("fitFlights", true) => "班機",
        ("fitHotel", false) => "Hotel",
        ("fitHotel", true) => "酒店",
        ("fitSize", false) => "Size",
        ("fitSize", true) => "面積",
        ("fitPrice", false) => "Price",
        ("fitPrice", true) => "價格",
        ("customFlightHotel", false) => "Custom flight + hotel",
        ("customFlightHotel", true) => "自訂機酒",
        ("bookedAccommodation", false) => "🏠 Booked Accommodation",
        ("bookedAccommodation", true) => "🏠 已訂住宿",
        ("otherSeaViewRefs", false) => "Other Sea-View References",
        ("otherSeaViewRefs", true) => "其他海景參考",
        ("referenceOnlyBooked", false) => "For reference · accommodation booked",
        ("referenceOnlyBooked", true) => "僅供參考 · 已選定住宿",
        ("candidatesSelecting", false) => "🏨 Sea-View Candidates · Selecting",
        ("candidatesSelecting", true) => "🏨 海景候選 · 正在選",
        // count unit after the folded reference summary ("· 3 間"); EN has none
        ("roomsUnit", false) => "",
        ("roomsUnit", true) => " 間",
        // guest ratings / price note (domestic candidate cards)
        ("reviewCount", false) => "{n} reviews",
        ("reviewCount", true) => "{n} 則",
        ("priceSourceRate", false) => "{source} rate",
        ("priceSourceRate", true) => "{source} 房價",
        ("stockThen", false) => " ({n} left then)",
        ("stockThen", true) => "（當時剩 {n} 間）",
        ("priceChecked", false) => "checked {date}{stock}",
        ("priceChecked", true) => "{date} 查價{stock}",
        ("stockDateUnknown", false) => "{n} left (date unknown)",
        ("stockDateUnknown", true) => "剩 {n} 間（查價日不明）",
        ("freeCancelUntil", false) => "free cancellation until {date}",
        ("freeCancelUntil", true) => "{date} 前免費取消",
        // candidate location minimap
        ("distToHub", false) => "~{km} km from {hub}",
        ("distToHub", true) => "距{hub} 約 {km} km",
        ("locationMapAlt", false) => "{name} location map",
        ("locationMapAlt", true) => "{name} 位置圖",
        // criteria-line date part: weekday names (0 = Sunday) + the stay span
        ("weekday0", false) => "Sun",
        ("weekday0", true) => "日",
        ("weekday1", false) => "Mon",
        ("weekday1", true) => "一",
        ("weekday2", false) => "Tue",
        ("weekday2", true) => "二",
        ("weekday3", false) => "Wed",
        ("weekday3", true) => "三",
        ("weekday4", false) => "Thu",
        ("weekday4", true) => "四",
        ("weekday5", false) => "Fri",
        ("weekday5", true) => "五",
        ("weekday6", false) => "Sat",
        ("weekday6", true) => "六",
        ("stayDate", false) => "{wd} {m}/{d}",
        ("stayDate", true) => "{m}/{d}（{wd}）",
        ("stayDateOneNight", false) => "{wd} {m}/{d} · {n} night",
        ("stayDateOneNight", true) => "{m}/{d}（{wd}）{n} 晚",
        ("stayDateNights", false) => "{wd} {m}/{d} · {n} nights",
        ("stayDateNights", true) => "{m}/{d}（{wd}）{n} 晚",
        _ => "",
    }
}

/// `t` plus `{name}` placeholder filling — see [`fill`].
pub fn tf(key: &str, lang: &str, args: &[(&str, &str)]) -> String {
    fill(t(key, lang), args)
}

/// Fill `{name}` placeholders in ONE pass: an inserted value is never re-scanned
/// (a hotel name containing "{date}" stays literal), and a `{…}` with no matching
/// arg is kept verbatim.
pub fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            if let Some((_, v)) = args.iter().find(|(k, _)| *k == &after[..close]) {
                out.push_str(v);
                rest = &after[close + 1..];
                continue;
            }
        }
        out.push('{');
        rest = after;
    }
    out.push_str(rest);
    out
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
    fn fill_is_single_pass_and_keeps_unknown_placeholders() {
        assert_eq!(fill("{a}-{b}", &[("a", "{b}"), ("b", "2")]), "{b}-2");
        assert_eq!(fill("x {nope} {a}", &[("a", "1")]), "x {nope} 1");
        assert_eq!(fill("{ unclosed", &[]), "{ unclosed");
        assert_eq!(tf("dayLabel", "zh", &[("day", "3")]), "第 3 天");
        assert_eq!(tf("dayLabel", "en", &[("day", "3")]), "Day 3");
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
