# 天氣抓取強化＋ now 卡時區 設計（原「MET Norway failover」）

日期：2026-11-16　狀態：Yang 已核可（2026-11-16，「都照建議」）；下一步寫實作計畫，計畫審過才派工。
審查：agy（Gemini 4 Argon Medium）、K3.1 Pro、Fable 5.2；核對紀錄見 §9。

## 0. 可行性（先看這一節）

**實際使用情境**：旅途中打開儀表板，看「今天剩下的時段」和「明天」的天氣。2026-11-13～15 遇到的狀況是 Open-Meteo 卡住幾分鐘（TLS stall），`fetch-weather` 重試約 64 秒後失敗；**過一陣子重跑就成功了**。失敗期間網站顯示的是上一次抓到的資料，而且已經有「更新於 HH:MM」標示（`render/day.rs:245-251`）。

**MET Norway 自動備援在這個情境幫不上忙，還會更糟**（三位審查者各自指出，已對原始碼核實）：
1. **今天會被截斷**：MET 只回傳「現在以後」的時數。下午 failover 時，`replace_day` 先刪掉當天所有時段列（`day_weather_sessions.rs:42-47`），已過的上午、中午時段消失，當日高低溫也只剩傍晚以後（Fable#1、agy#5）。
2. **會用較差的資料蓋掉較好的**：MET 全球點沒有降雨機率、沒有體感溫度，`next_1_hours` 也只有前兩三天有。Open-Meteo 只要卡幾分鐘，第 3 天以後原本完整的時段、降雨機率就會被刪或寫成 NULL（Fable#2）。
3. **帶傘建議會消失**：沒有降雨機率，穿衣帶的「帶傘」永遠不會出現（agy#4）。旅途中最需要的就是這個。
4. **涵蓋不足**：MET 約 9–10 天，Open-Meteo 16 天；尾端日可能只剩幾個點卻被當成「有涵蓋」覆寫（agy#5、Fable#3）。

要讓備援不變壞，得做逐時段保留、跨來源合併、雙來源出處與更新時間、降雨量推帶傘規則…複雜度遠超過「過幾分鐘再重跑一次」。

**建議（設計者判斷，待 Yang 確認 Q1）**：
- **Phase 1 不做 MET 自動備援。** 改做真正有用、風險低的部分：
  1. **now 卡改用目的地時區**（真缺陷：台灣國內行程「現在」差一小時；行程紀錄的「已過」判斷也要用）。
  2. **抓取強化**：失敗時舊資料原封不動（現況已是如此，寫進規格並加測試）、錯誤訊息分得出 HTTP 狀態碼、缺欄位不再寫成 0°C／晴天、單一目的地的寫入包成一筆交易、`--all` 有失敗就非零退出。
- MET（或日本 JMA、台灣 CWA）若之後要做，只做「**補空**」：只寫目前完全沒有資料的日子，絕不覆寫既有列。列為 Phase 2，本文件只留附錄筆記。

## 1. 目標（Phase 1）
1. now 卡的時鐘（伺服器端＋每分鐘重標的 inline script）改用 `destination_config.timezone`。
2. `fetch-weather` 失敗時**一列都不動**：舊資料連同原本的 `source`／`fetched_at` 照常顯示，網站上的「更新於」如實反映資料新舊。
3. 錯誤可診斷：stderr 分得出連線逾時、TLS、HTTP 4xx（含狀態碼）、5xx、JSON 結構錯。4xx（429 除外）不重試。
4. 不發布虛構數值：缺溫度或天氣碼的日子不寫入，stderr 說明。
5. 寫入原子化：一個目的地的 `days.weather_*`、`day_weather_sessions`、audit 包在同一筆交易。
6. `--all`：任一目的地失敗，跑完其他目的地後非零退出。

## 2. 非目標
- 不做任何第二天氣來源、來源順序表、多來源出處（延到 Phase 2，見附錄）。
- 不改四個時段窗（上午 9–12、中午 12–14、下午 14–18、晚上 18–22）及聚合規則。
- 不改首頁 `today_from_ms` 的 UTC「今天」（router.rs:271），另案。

## 3. 反目標
- 不准在失敗時寫入部分結果；不准讀本地檔；不准用預設值填缺欄位。
- 不准加 JSON 欄位、CHECK 約束、feature flag；migrator 不塞資料。
- 時區不准退回寫死的 JST。

## 4. 現況（已核對）
- `weather.rs:451`：curl 用 `-sf`，所有 HTTP 錯誤都是 exit 22，分不出 403/429/400/503。
- `weather.rs:565-569`：缺 `temperature_2m_max/min`、`weather_code` 時寫 `0.0`／`0`（晴）——違反「no default data」。
- `weather.rs:229-240`：`--all` 單一目的地失敗只 `eprintln!`，最後 exit 0。
- `weather.rs:270-275`：逐日寫入沒有交易；`write_day_weather`（:582-633）先寫 `days`、再 `replace_day`（DELETE→INSERT）、再寫 events。
- `weather.rs:347`：私有 `read_active_destination`，`--dest` 不驗證是否屬於該 plan；違反 CLAUDE.md「`resolve_active_destination` 是唯一 resolver」。旗標解析以 `_ => {}` 忽略未知旗標（:208-219）。
- `day_weather_sessions.rs:42-47`：`replace_day` 先刪當天全部列。
- `travel-db/src/repo/plan.rs:386,423`：`precipitation_pct` 讀成 `f64`，NULL → 0.0；`view_itinerary.rs:475-505` 因此印出假的 `Rain: 0%`。目前 Open-Meteo 都有值，Phase 1 不受影響；Phase 2 引入無機率來源時必須先改。
- 時區：
  - `render/now.rs:30-31,43` `JST_OFFSET_MS`／`jst_from_ms`；`router.rs:324-330` 用它建 clock。
  - `render/mod.rs:371` inline script 寫死 `+324e5`。
  - `model.rs:48` 註解寫 JST。
  - `verify_site.rs:69,756,813`、`verify_site_page.rs:565` 的 `--at` 說明與錯誤訊息寫 JST。
  - `docs/reference/CLI.md:257`、`docs/reference/dashboard.md:30,31,47` 寫 JST。（CLAUDE.md 沒有 JST 字樣，原稿誤列。）
  - `router.rs:1044-1046` 已查 `destination_config.timezone`，目前只拿來產生 `weather_updated` 時間戳（:1061）。
  - `tzfmt.rs:50-51` 已處理午夜 `"24"`；非 wasm 下 `format_in_tz` 回 `None`（:94-97）。
- `validate.rs`、`validate_itinerary.rs` **沒有**檢查時區不可為空（原稿誤稱有）；時區為空只在 `fetch-weather`（weather.rs:426）與 `itinerary`（plan.rs:504）報錯。
- CLI 沒有 `chrono-tz`（`travel-cli/Cargo.toml:13`）；現有時區轉換呼叫系統 `date(1)`（plan.rs:288-311）。Phase 1 的 CLI 不需要新的時區運算。

## 5. Yang 已決定的
- 時段窗不變。
- now 卡改用 `destination_config.timezone`。
- 失敗要 fail loud、不准靜默回退；不准 hardcode；來源資料放 DB。
- Phase 2 方向：「根據國家改用不同的天氣主網站」（日本 JMA、台灣 CWA）。（本文件建議 Phase 2 只做補空，見附錄。）

## 6. 設計

### 6.1 now 卡時區（worker）
- router 把 [29] 的時區存進 `plan.timezone`。
- `tzfmt.rs` 新增：
  - `clock_from_parts(parts) -> Option<(date, minutes)>`：純函式，host 可測，處理 `"24"`→`00`。
  - `now_clock_in_tz(ms, tz) -> Option<Clock>`：wasm 端透過 `Intl`（`hourCycle:'h23'`、含 `year`），與 `format_in_tz` 共用建立 formatter 的 helper。
- `now.rs`：刪除 `JST_OFFSET_MS`、`jst_from_ms`；`parse_at` 改解讀為目的地當地牆鐘時間（格式不變）。
- 決定 clock 的順序（agy#2、Fable#8）：
  1. 有 `?at=` → 直接用（釘選時間不需要時區，verify-site 不受時區資料影響）。
  2. 否則需要 `plan.timezone` 非空且 `Intl` 接受；成功 → 當地時間。
  3. 失敗 → `clock = None`，`console_error!` 記錄目的地。**不退回 JST。**
- `clock = None` 的頁面行為要寫明（agy#2）：沒有 now 卡、目錄沒有「今天」標籤、日卡全部展開（`CardOpen::Open`）。JS 關閉也能完整閱讀。
- inline script（mod.rs:371）：
  - now 卡帶 `data-now-tz="<IANA>"`（沒有時區就不輸出 now 卡，也就沒有這個屬性）。
  - 以 `Intl.DateTimeFormat('en-CA',{timeZone:tz,hourCycle:'h23',year,month,day,hour,minute}).formatToParts()` 取當地日期與分鐘，`hour` 做 `% 24` 正規化（agy#3），日期由 parts 組成後與 `data-now-date` 比對。
  - 整段包在 `try{…}catch(e){return;}`，例外時保留伺服器端標記。仍是唯一 inline script。
- 既有行為註記（Fable#8）：出發日人還在台灣時，卡片用目的地時間。接受，不改。
- `validate data` 新增：`destination_config.timezone` 為空 → WARN（K3#8、Fable#9）。
- 與行程紀錄共用：`plan.timezone` 與 `Clock` 也供 `2026-11-16-trip-record-design.md` 的「已過」判斷使用。

### 6.2 抓取強化（CLI）
- **HTTP 狀態碼**：curl 改 `-sS -w '\n%{http_code}'`（不用 `-f`），由最後一行取狀態碼。
  - 2xx → 解析。
  - 429、5xx、連線逾時（exit 28）、TLS（exit 35）→ 照現有有界重試（3 次）。
  - 其他 4xx → 不重試，訊息標明 `request error HTTP <code>`（例如行程尾端超出預報範圍；Open-Meteo 是否回 400 由實作者用真實請求確認）。
- **缺欄位**：某日缺 `temperature_2m_max/min` 或 `weather_code` → 該日不寫入（舊列保留），stderr 印 `day N: Open-Meteo returned no <field> — kept previous data`。其餘選填欄位維持 NULL。
- **交易**：每個目的地抓取完成後，以既有 `begin_immediate`／`finish_tx`（`cascade/common.rs:294-329`）包住 `days.weather_*`、`replace_day`、`plan_events` 與 `record_operation`。任一步失敗 rollback，該目的地一列不動。
- **零寫入不 bump**：實際寫入天數為 0 時不呼叫 `finalize`（agy#8）。
- **`--all`**：各目的地獨立交易、獨立 commit；跑完後若有任一失敗，stderr 彙總並以非零退出。成功目的地的 audit 各自完整（各自一筆 `operation_runs`），不會出現孤兒 `plan_events`。
- **resolver 與旗標**：改用 `cascade::common::resolve_active_destination`；未知旗標用 `plan_resolver::reject_unknown_flags` 拒絕（agy#7、K3#6）。
- **失敗時的訊息**：沿用現有 exit-code 說明，並加一句「已保留上次資料（更新於 <時間>）；幾分鐘後再試」。

### 6.3 不變的部分
- `weather_providers` 表、`set-weather-provider` 指令、出處行（每天一個來源）維持現狀；Phase 1 只有 Open-Meteo，`days.weather_source_id` 與 session 列必然同源。
- 出處行加「資料經彙整」字樣（CC BY 4.0 要求註明修改，對 Open-Meteo 同樣適用；agy#1、K3#2、Fable#12）：字串放 i18n.rs（例 ZH「天氣資料（經時段彙整）：」／EN「Weather data (aggregated):」）。「CC BY 4.0」改連到授權頁：在 `weather_providers` 加 `license_url` 欄（可為 NULL；migrator 只加欄，資料由 `set-weather-provider --license-url` 寫入）。（設計者判斷，見 Q3）

## 7. 測試
- CLI 單元測試（inline）：
  - HTTP 狀態碼解析：200、400、403、429、503、無狀態碼。
  - 4xx（非 429）不重試；429／5xx／exit 28／35 重試。
  - 缺 `temperature_2m_max` 或 `weather_code` 的日子不產生寫入列；其他日正常。
  - 既有「前一小時」降雨測試（weather.rs:889）維持綠燈。
- 真 Turso 整合測試（背景執行，`zz_test_<nanos>`，RAII Guard）：
  - 寫入中途失敗（例：注入一個非法 session key 讓 `replace_day` 失敗）→ `days.weather_*`、sessions、events 全部維持原值。
  - `--all` 一個目的地失敗 → 非零退出，另一個目的地照常寫入且 audit 完整。
  - 零寫入 → `plans.version` 不變。
- worker：
  - `clock_from_parts`：Asia/Taipei、Asia/Tokyo、午夜 `"24"`。
  - 有 `?at=`、沒有時區 → 仍出 now 卡；沒有 `?at=`、沒有時區 → 不出 now 卡、日卡全開。
  - 既有使用 `Plan::default()` 的測試補 `timezone`（now.rs:649-675、mod.rs:775、:1218 一帶）；script 字串斷言改成「不含 `324e5`、含 `data-now-tz`、含 `% 24`」。
  - 部署後以 `?at=` 加一個台北時區的 plan 實測；跑 `verify-site`。

## 8. 部署順序
1. `make build`（CLI）→ `./bin/travel db migrate`（只加 `weather_providers.license_url`）→ 重產 `scripts/schema.sql`。
2. `set-weather-provider open_meteo --license-url https://creativecommons.org/licenses/by/4.0/`。
3. build 並部署 worker：`cd workers/trip-dashboard-rs && unset CLOUDFLARE_API_TOKEN && npx wrangler deploy`。
4. `verify-site`；更新 CLI.md、dashboard.md、verify_site 的 JST 字樣。

## 9. 審查核對紀錄（2026-11-16）

| 議題 | 提出者 | 處理 | 理由 |
|---|---|---|---|
| MET 無過去時數，今天被截斷 | Fable#1、agy#5 | 採納→改為不做 MET 備援（§0） | 已核 `replace_day` 先刪全天 |
| failover 用較差資料蓋掉較好的 | Fable#2 | 採納→同上 | MET 全球點無降雨機率、無體感 |
| 零涵蓋被當成功 | Fable#3、agy#8 | 採納（零寫入不 bump；MET 部分隨之移除） | weather.rs:270-275 已核 |
| 寫入非原子 | Fable#4、K3#1、agy#8 | 採納（§6.2 交易） | weather.rs:582-633 已核 |
| HTTP 狀態碼分不出 | Fable#5、agy#11 | 採納（§6.2） | weather.rs:451 `-sf` 已核 |
| 缺值寫成 0°C／晴 | Fable#13、K3#4 | 採納（§6.2） | weather.rs:565-569 已核 |
| 私有 resolver、忽略未知旗標 | agy#7、K3#6 | 採納 | weather.rs:347、:208-219 已核 |
| `--all` exit 0、audit 孤兒 | agy#8、原稿 | 採納（各目的地獨立交易） | weather.rs:229-240 已核 |
| validate 沒有檢查時區 | Fable#9、K3#8 | 採納（新增 WARN，更正現況） | grep validate*.rs 無 timezone |
| now 卡 `?at=`／無時區／頁面行為 | agy#2、Fable#8 | 採納（§6.1 順序與行為寫明） | router.rs:327-338 |
| inline script 午夜 `"24"`、例外 | agy#3、Fable#8 | 採納 | tzfmt.rs:50-51 有同樣處理 |
| CC BY「註明修改」、授權連結 | agy#1、K3#2、Fable#12 | 採納（i18n 字樣＋`license_url`） | CC BY 4.0 對 Open-Meteo 同樣適用 |
| NULL 降雨機率、帶傘、CLI 假 `Rain: 0%` | agy#4、Fable#15 | 延後到 Phase 2 | Phase 1 只有 Open-Meteo，一定有值；已列 Phase 2 前置條件 |
| 混源出處、各來源更新時間 | agy#1、K3#5、Fable#17 | 不採納（移除需求） | 不做第二來源就不會混源 |
| 來源順序表、`set-weather-chain` | K3#3、K3#7、Fable#18、agy#10 | 不採納（移除需求） | 同上；也免去既有目的地回填 |
| MET symbol 對照、UA、座標位數、Expires | agy#11、Fable#12 | 延後（附錄） | 只在 Phase 2 需要 |
| 6h 區塊跨日、嚴重度排序 | Fable#6、#7、K3 問題 4 | 延後（附錄） | 同上 |
| `chrono-tz` 二選一 | Fable#11、agy#6 | 不採納（不需要） | Phase 1 CLI 無新時區運算 |
| `validate publish` 寫死 Open-Meteo 16 天 | agy#10 | 不採納（Phase 1 不變） | 只有 Open-Meteo 時文字正確 |
| 行號小誤（:52→:54、:31→:30、now.rs:13） | agy#6、Fable#12 | 採納（§4 已更正） | — |
| 同座標共用回應、adapter 取樣介面、Phase 2 雙層表 | agy#12、K3#10、Fable#19 | 不採納（移除） | 過度設計 |
| CLAUDE.md 有 JST 描述 | Fable#10 | 採納（更正：沒有） | grep 已核 |

統計：約 24 項；採納 14、延後 4、不採納（含因縮小範圍而移除）6。
**收斂情形**：三位審查者都各自指出「MET 備援會把資料變差／今天被截斷」「寫入非原子」「出處要註明修改」；可信度高。只有 Fable 明確量化了「較差資料蓋掉較好的」；只有 agy 指出 inline script 午夜 bug 與 CLI 假 `Rain: 0%`。

## 10. Yang 的回答（2026-11-16）

**Yang 決定（2026-11-16）：全部照建議（「都照建議」）。**

1. **MET Norway 自動備援先不做，Phase 1 只做時區修正＋抓取強化，可以嗎？** 建議：可以；Open-Meteo 卡住時保留舊資料、網站顯示更新時間、幾分鐘後重跑即可。
2. **日後若要第二來源（MET／JMA／CWA），只允許「補空」（只寫完全沒資料的日子、絕不覆寫），可以嗎？** 建議：可以。
3. **出處行加「資料經彙整」字樣，「CC BY 4.0」連到授權頁，可以嗎？** 建議：可以（授權要求，對 Open-Meteo 也適用）。
4. **時區設定壞掉時，網站不出 now 卡、日卡全部展開（不退回日本時間），可以嗎？** 建議：可以；`validate data` 會先警告。

## 附錄：Phase 2 第二來源筆記（不實作）
- 只做「補空」：只寫 `days.weather_*` 與 sessions 都不存在的日子；既有列一律不動，所以不會混源。
- 前置條件：`precipitation_pct` 在 CLI（`travel-db/repo/plan.rs:386,423`、`view_itinerary.rs`）改成 `Option<f64>`；帶傘建議要決定無機率時的規則（例如以雨天碼或降雨量判斷）。
- MET Norway：
  - `locationforecast/2.0/complete`；UA 必填且應附聯絡網址（無法聯絡可能被無預警封鎖）；座標最多 4 位小數；遵守 `Expires`。
  - 時間戳為 UTC；`next_1_hours` 蓋 T..T+1（與 Open-Meteo 的前一小時相反）；全球點只有前段有 1h 資料。
  - 授權 NLOD 2.0／CC BY 4.0，措辭「Data from MET Norway」。
  - symbol 對照需完整窮舉（注意官方雙 s 拼字 `lightssleetshowersandthunder`、`lightssnowshowersandthunder`），未知 symbol 視為失敗。
  - 嚴重度要另訂排序表，不能直接比 WMO 數字（45 霧 > 3 陰、71 小雪 > 65 大雨）。
- JMA：需要 area code（Turso 資料）；6 小時粒度，對不齊時段窗就只寫 daily。
- CWA：需要 API key（`.env`），有鄉鎮 3 小時預報。
