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
        ("ticketsVouchers", false) => "🎫 Tickets & vouchers",
        ("ticketsVouchers", true) => "🎫 票券與憑證",
        // page navigation (render/mod.rs page_toc): the booking summary's own
        // collapsible title, the TOC heading, its collapse/expand-all buttons and
        // the floating back-to-TOC link
        ("bookingSummary", false) => "Booking summary",
        ("bookingSummary", true) => "預訂總表",
        ("toc", false) => "Contents",
        ("toc", true) => "目錄",
        ("tocBack", false) => "↑ Contents",
        ("tocBack", true) => "↑ 目錄",
        ("collapseAll", false) => "Collapse all",
        ("collapseAll", true) => "全部收合",
        ("expandAll", false) => "Expand all",
        ("expandAll", true) => "全部展開",
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
        // weather overview (render/day.rs weather_overview): heading + column labels
        ("weatherOverview", false) => "Weather overview",
        ("weatherOverview", true) => "天氣總覽",
        ("wxDate", false) => "Date",
        ("wxDate", true) => "日期",
        ("wxWeather", false) => "Weather",
        ("wxWeather", true) => "天氣",
        ("wxTemp", false) => "Temp",
        ("wxTemp", true) => "氣溫",
        ("wxRain", false) => "Rain",
        ("wxRain", true) => "降雨機率",
        ("wxWear", false) => "What to wear",
        ("wxWear", true) => "穿著建議",
        // packing summary under the weather table (render/day.rs packing_summary):
        // label + one "item (N days)" entry; the item name is content (Turso)
        ("packList", false) => "What to pack",
        ("packList", true) => "要帶的衣物",
        ("packItemOneDay", false) => "{item} ({n} day)",
        ("packItemOneDay", true) => "{item}（{n} 天）",
        ("packItemDays", false) => "{item} ({n} days)",
        ("packItemDays", true) => "{item}（{n} 天）",
        // meal overview (render/day.rs meal_overview): heading + column labels
        // (the date column reuses wxDate); dateOption labels a day-variant row —
        // ZH has no space after the full-width （六） paren: 「11/15（日）· 晴天方案」
        ("mealOverview", false) => "Meal overview",
        ("mealOverview", true) => "用餐總覽",
        ("mealSession", false) => "Meal",
        ("mealSession", true) => "餐別",
        ("mealMain", false) => "Main pick",
        ("mealMain", true) => "主選",
        ("mealBackup", false) => "Backup",
        ("mealBackup", true) => "備選",
        ("dateOption", false) => "{date} · {label}",
        ("dateOption", true) => "{date}· {label}",
        ("todaysRoute", false) => "Today's route",
        ("todaysRoute", true) => "今日路線",
        ("dayHeading", false) => "Day {day} · {date}",
        ("dayHeading", true) => "第 {day} 天 · {date}",
        ("dayVariant", false) => "Alternative: {label}",
        ("dayVariant", true) => "替代方案：{label}",
        ("dayOption", false) => "Day {day} · {label}",
        ("dayOption", true) => "第 {day} 天 · {label}",
        ("dayOptionHotelRoundTrip", false) => "Day {day} · {label} · hotel round trip",
        ("dayOptionHotelRoundTrip", true) => "第 {day} 天 · {label}・旅社來回",
        ("dayOptionAway", false) => "Day {day} · {label} · beyond the area",
        ("dayOptionAway", true) => "第 {day} 天 · {label}・本區以外的行程",
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

/// WMO weather-code icons and descriptions (code, emoji, EN, ZH) — the full set
/// Open-Meteo's `weather_code` returns. A fixed code→wording table is chrome, not
/// content. Emoji by group: 0 clear ☀️, 1 mainly clear 🌤️, 2 partly cloudy ⛅,
/// 3 overcast ☁️, 45/48 fog 🌫️, 51–57 drizzle 🌦️, 61–67 rain and 80–82 rain
/// showers 🌧️, 71–77 snow and 85–86 snow showers 🌨️, 95–99 thunderstorm ⛈️.
/// The EN text matches `travel fetch-weather`'s stored `weather_label`
/// byte-for-byte (rust/crates/travel-cli/src/weather.rs `WMO_LABELS`), so the
/// English page reads the same whether it renders from the code or the label.
const WMO_WEATHER: &[(i64, &str, &str, &str)] = &[
    (0, "☀️", "Clear sky", "晴"),
    (1, "🌤️", "Mainly clear", "大致晴朗"),
    (2, "⛅", "Partly cloudy", "晴時多雲"),
    (3, "☁️", "Overcast", "陰"),
    (45, "🌫️", "Foggy", "霧"),
    (48, "🌫️", "Depositing rime fog", "霧凇"),
    (51, "🌦️", "Light drizzle", "小毛毛雨"),
    (53, "🌦️", "Moderate drizzle", "毛毛雨"),
    (55, "🌦️", "Dense drizzle", "大毛毛雨"),
    (56, "🌦️", "Light freezing drizzle", "小凍毛毛雨"),
    (57, "🌦️", "Dense freezing drizzle", "大凍毛毛雨"),
    (61, "🌧️", "Slight rain", "小雨"),
    (63, "🌧️", "Moderate rain", "中雨"),
    (65, "🌧️", "Heavy rain", "大雨"),
    (66, "🌧️", "Light freezing rain", "小凍雨"),
    (67, "🌧️", "Heavy freezing rain", "大凍雨"),
    (71, "🌨️", "Slight snowfall", "小雪"),
    (73, "🌨️", "Moderate snowfall", "中雪"),
    (75, "🌨️", "Heavy snowfall", "大雪"),
    (77, "🌨️", "Snow grains", "雪粒"),
    (80, "🌧️", "Slight rain showers", "小陣雨"),
    (81, "🌧️", "Moderate rain showers", "陣雨"),
    (82, "🌧️", "Violent rain showers", "強陣雨"),
    (85, "🌨️", "Slight snow showers", "小陣雪"),
    (86, "🌨️", "Heavy snow showers", "大陣雪"),
    (95, "⛈️", "Thunderstorm", "雷雨"),
    (96, "⛈️", "Thunderstorm with slight hail", "雷雨伴隨小冰雹"),
    (99, "⛈️", "Thunderstorm with heavy hail", "雷雨伴隨大冰雹"),
];

/// Localized description for a WMO weather code. `None` for an unmapped code —
/// the caller then shows the stored `weather_label` as-is (it is data).
pub fn wmo_weather(code: i64, lang: &str) -> Option<&'static str> {
    WMO_WEATHER
        .iter()
        .find(|(c, _, _, _)| *c == code)
        .map(|(_, _, en, zh)| if lang == "en" { *en } else { *zh })
}

/// Neutral icon for a day whose weather code is NULL or unmapped: a thermometer
/// claims nothing about sun or rain.
pub const WEATHER_EMOJI_UNKNOWN: &str = "🌡️";

/// Weather emoji for a WMO code (same table as [`wmo_weather`]); a NULL or
/// unmapped code gets the neutral [`WEATHER_EMOJI_UNKNOWN`].
pub fn wmo_emoji(code: Option<i64>) -> &'static str {
    code.and_then(|code| WMO_WEATHER.iter().find(|(c, _, _, _)| *c == code))
        .map(|(_, emoji, _, _)| *emoji)
        .unwrap_or(WEATHER_EMOJI_UNKNOWN)
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
    fn page_navigation_keys_are_bilingual() {
        for key in ["bookingSummary", "toc", "tocBack", "collapseAll", "expandAll"] {
            let (en, zh) = (t(key, "en"), t(key, "zh"));
            assert!(!en.is_empty() && !zh.is_empty(), "{key} missing a language");
            assert!(en.chars().all(|c| c.is_ascii() || c == '↑'), "{key} EN must be English: {en}");
            assert!(!zh.is_ascii(), "{key} ZH must be Chinese: {zh}");
        }
        assert_eq!(t("toc", "zh"), "目錄");
        assert_eq!(t("collapseAll", "zh"), "全部收合");
        assert_eq!(t("expandAll", "en"), "Expand all");
    }
    #[test]
    fn packing_summary_keys_are_bilingual() {
        assert_eq!(t("packList", "zh"), "要帶的衣物");
        assert_eq!(t("packList", "en"), "What to pack");
        assert_eq!(tf("packItemOneDay", "en", &[("item", "Scarf"), ("n", "1")]), "Scarf (1 day)");
        assert_eq!(tf("packItemDays", "en", &[("item", "Scarf"), ("n", "3")]), "Scarf (3 days)");
        assert_eq!(tf("packItemOneDay", "zh", &[("item", "圍巾"), ("n", "1")]), "圍巾（1 天）");
        assert_eq!(tf("packItemDays", "zh", &[("item", "圍巾"), ("n", "3")]), "圍巾（3 天）");
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
    #[test]
    fn wmo_weather_covers_open_meteo_codes_in_both_languages() {
        let codes = [
            0, 1, 2, 3, 45, 48, 51, 53, 55, 56, 57, 61, 63, 65, 66, 67, 71, 73, 75, 77, 80, 81,
            82, 85, 86, 95, 96, 99,
        ];
        assert_eq!(WMO_WEATHER.len(), codes.len());
        for c in codes {
            let en = wmo_weather(c, "en").unwrap_or_else(|| panic!("EN missing for {c}"));
            let zh = wmo_weather(c, "zh").unwrap_or_else(|| panic!("ZH missing for {c}"));
            assert!(!en.is_empty() && !zh.is_empty(), "empty text for {c}");
            assert!(en.is_ascii(), "EN must be English for {c}: {en}");
            assert!(!zh.is_ascii(), "ZH must be Chinese for {c}: {zh}");
        }
        assert_eq!(wmo_weather(81, "zh"), Some("陣雨"));
        assert_eq!(wmo_weather(81, "en"), Some("Moderate rain showers"));
        assert_eq!(wmo_weather(45, "zh"), Some("霧"));
        assert_eq!(wmo_weather(45, "en"), Some("Foggy"));
        assert_eq!(wmo_weather(123, "zh"), None);
        assert_eq!(wmo_weather(-1, "en"), None);
    }

    #[test]
    fn wmo_emoji_maps_each_code_group_and_falls_back_to_thermometer() {
        let groups: &[(&[i64], &str)] = &[
            (&[0], "\u{2600}\u{FE0F}"),           // ☀️
            (&[1], "\u{1F324}\u{FE0F}"),          // 🌤️
            (&[2], "\u{26C5}"),                    // ⛅
            (&[3], "\u{2601}\u{FE0F}"),           // ☁️
            (&[45, 48], "\u{1F32B}\u{FE0F}"),     // 🌫️
            (&[51, 53, 55, 56, 57], "\u{1F326}\u{FE0F}"), // 🌦️
            (&[61, 63, 65, 66, 67, 80, 81, 82], "\u{1F327}\u{FE0F}"), // 🌧️
            (&[71, 73, 75, 77, 85, 86], "\u{1F328}\u{FE0F}"), // 🌨️
            (&[95, 96, 99], "\u{26C8}\u{FE0F}"),  // ⛈️
        ];
        let mut covered = 0;
        for (codes, emoji) in groups {
            for &c in *codes {
                assert_eq!(wmo_emoji(Some(c)), *emoji, "code {c}");
                covered += 1;
            }
        }
        // Every table row has an icon from the groups above.
        assert_eq!(covered, WMO_WEATHER.len());
        assert_eq!(WEATHER_EMOJI_UNKNOWN, "\u{1F321}\u{FE0F}"); // 🌡️
        assert_eq!(wmo_emoji(None), WEATHER_EMOJI_UNKNOWN);
        assert_eq!(wmo_emoji(Some(123)), WEATHER_EMOJI_UNKNOWN);
        assert_eq!(wmo_emoji(Some(52)), WEATHER_EMOJI_UNKNOWN);
    }
}
