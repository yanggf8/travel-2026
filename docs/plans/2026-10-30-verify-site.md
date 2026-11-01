# verify-site：線上儀表板與地圖的真實結果檢查

Date: 2026-10-30. Status: **approved by Yang (2026-10-30), rev 2** — 已吸收 Codex（前端）與 K3.1 Pro（後端）
的設計審查；兩份審查的每一條說法都已逐條對照原始碼核對，全部屬實（見文末「審查紀錄」）。

**實作進度（2026-11-02）：** 第一、二、三階段都已實作（`verify_site.rs`、`verify_site_page.rs`、
`verify_site_sheet.rs`，worker `render/map.rs`、`render/mod.rs`）。worker 的 `data-map-key` /
`data-plan-id` / `data-view` 標記**尚未部署**；部署前，線上頁面的 `ident` 與 `slot` 檢查會照設計判 FAIL
（指向 `wrangler deploy`），這是預期中的結果。實作與設計的差異見文末「實作備註」。

## 問題

目前沒有任何測試讀取「線上網站實際送出的東西」。現有測試分三類，都碰不到線上結果：

- worker 單元測試：fixture → HTML。
- `snapshot_maps.rs` 單元測試：合成座標 → PNG。
- real-Turso 整合測試：資料層。

「HTTP 200 但內容是錯的」這類問題，現有檢查全部抓不到：

| 案例 | 現象 | 為什麼沒被抓到 |
|---|---|---|
| CARTO 改成需要 API key | 每張地圖都蓋上「API KEY REQUIRED」字樣，HTTP 200 | 沒有人檢查圖片內容 |
| worker placeholder | R2 沒有物件、物件沒 body、或物件無效時，`/map/*` 都回同一張 66 bytes 的 1×1 PNG，**狀態碼仍是 200**，快取 300 秒（`router.rs:52-70`、`router.rs:759`） | curl 只看狀態碼 |
| Wrangler 上傳失敗（2026-10-30 kyoto-nov `day-1.png`） | 線上是舊圖或 placeholder | 只有 `map_artifacts.status` 記錄，沒有人讀線上 |
| CDN `max-age=86400` | 重新 snapshot 後，線上一天內仍可能送舊圖 | 沒有人比對線上 bytes |
| `skipped` 卻仍在線上 | 先前 `failed` 的圖改成 `skipped` 時不會刪 R2 物件（`remove_stale_map` 只在前一次是 `uploaded` 才刪，`snapshot_maps.rs:2483-2505`），worker 只看 R2，舊圖照樣顯示 | 沒有人檢查「不該存在的圖」 |
| 只改路線 | `day_route_segments` 沒有 `updated_at`，`check-maps-fresh` 看不出來（`check_maps_fresh.rs:26-31`），舊圖的 sha 和 manifest 一致 | 時間戳式過期判斷有盲點 |
| 圖例和圖不一致 | `map_legend_stops` 有寫，但頁面沒渲染或筆數不對；worker 會靜默丟掉解不開的列（`router.rs` `decode_legend_row`） | worker 測試只用 fixture |

## 目標與非目標

**目標：**
- 一個 agent-first 的唯讀指令，針對真實方案和線上網址，逐項判定 PASS/WARN/FAIL。
- 只要有任何 FAIL，就以非零狀態結束。
- 輸出是純文字，每行一個判定，附上下一步該跑的指令。

**非目標：**
- 像素級 golden 比對：ArcGIS 底圖本來就會變。
- 用像素「數 pin」：每天的顏色也用在路線（`snapshot_maps.rs:745-756`）、圖例點和錨點（`:795-840`）；顏色每 7 天循環（`:56-64`）；declutter 找不到位置時 pin 會留在原地重疊（`:1062-1072`）。用像素數 pin，誤判 PASS 和誤判 FAIL 都避免不了。
- 放進 pre-commit 或 `make test`：這個指令要連外網。

## 指令

```
travel verify-site [--plan-id <id>] [--contact-sheet <dir>]
```

- **預設範圍：** 所有未軟刪的方案，含已結束的行程。
- **base URL：** 沿用 `share_token.rs` 的 `dashboard_host()`，即 `TRAVEL_DASHBOARD_HOST` 加預設值。
- **連線方式：** HTTP 走 curl subprocess。帶 token 的 URL 用 `curl -K -` 從 stdin 傳入，不出現在 process 參數裡。
- **cookie：** 一律不帶。帶了擁有者的 session cookie，就會被當成擁有者（`router.rs:196`）。
- **輸出格式：** 每行一個判定，最後一行是彙總：

```
FAIL kyoto-nov-2026 asset   day-1.png  live = worker placeholder (object missing or rejected as invalid) → travel snapshot-maps --plan-id kyoto-nov-2026
FAIL kyoto-nov-2026 asset   day-3.png  status=skipped but live serves a real PNG (stale R2 object) → <刪除指令>
WARN kyoto-nov-2026 fresh   day-2.png  current input hash != map_artifacts.input_sha256 → travel snapshot-maps --plan-id kyoto-nov-2026
PASS kyoto-nov-2026 asset   plan.png   652891 bytes, sha256 match
summary: 6 plan(s) · 71 PASS · 1 WARN · 2 FAIL
```

## 檢查分層

### 第 2 層：線上檔案和資料庫是否一致（第一階段，不需要 token）

**1. 該有哪些圖：** 用 `check_maps_fresh.rs` 的 `expected_map_keys_with_excursion` 推算。這份清單和 snapshot-maps 是同一個來源，不另外維護。
- 預期中的 key 在 `map_artifacts` 沒有記錄 → **FAIL（MISSING）**。

**2. 每個 key 下載一次：**
- 一律帶 `?verify=<nonce>`（每次執行隨機產生）。worker 只用 path 找 R2 物件，所以多一個 query 不影響拿到的內容，但能避開 CDN 快取。
- 有拿到頁面上的 `?v=<ETag>` 時（第二階段），再另外用那個網址抓一次，驗證觀看者實際拿到的是同一份。

**3. 依 `map_artifacts.status` 判定：**

| status | 線上應該是 | 不符就判 |
|---|---|---|
| `uploaded` | 200、`image/png`，**sha256 和 byte_size 都等於 manifest** | FAIL：線上是 placeholder，或 bytes 不符（上傳失敗、CDN 舊圖） |
| `failed` | — | 一律 FAIL，附上 `skip_reason` |
| `skipped` | **必須是 placeholder** | FAIL：線上仍是真圖，代表 R2 留著舊物件 |

- placeholder 用**完整 bytes**比對 worker 的 `PLACEHOLDER_PNG` 來辨識，不只看大小。
- 錯誤訊息寫成「object missing or rejected as invalid」，不寫死成「R2 沒有物件」。

**4. 過期判斷改用輸入 hash：**
- 不靠時間戳，改成唯讀重算目前的地圖輸入 hash，和 `map_artifacts.input_sha256` 比對。輸入包括站點、路線、天數、`RENDER_REV`。
- 重算時把 snapshot-maps 組輸入的那段抽成共用函式，geocode 只讀快取、不打 Nominatim。
- 快取缺座標時給 WARN「cannot verify freshness」，不硬算。
- 這樣只改路線也抓得到，補上 `check-maps-fresh` 的盲點。

### 第 3 層：圖片內容的機器判定（第一階段，純函式）

PNG 解碼沿用 `decode_rgba_png`。所有常數和 renderer 共用，不另外抄一份。

| 檢查 | 判定 | 等級 |
|---|---|---|
| 尺寸 | 必須是 640×440 | FAIL |
| 有沒有背景 | 偵測 renderer 的「純格線」palette：只有格線、沒有 ArcGIS 底圖、也沒有 Overpass 道路色 → 代表背景沒拿到 | FAIL |
| Overpass 退回模式 | 用 Overpass 道路色辨識：保留區以外有 ≥ 20 px 連續的道路色。認得出來就視為有背景 | 通過 |
| 顏色過少或畫面太平 | 一般性的平坦度門檻。真實地圖可能大片海面，所以還沒用真圖校準前只給 WARN | WARN |
| 標題、© 版權文字存在 | 在保留區找深色文字像素。白底框和格線底兩種情況都接受（白底框只在有 ArcGIS 底圖時才畫，`snapshot_maps.rs:809-812`） | FAIL |
| 異常大片文字 | 底圖區塊出現大片高對比的重複字樣（CARTO 型）。照 Yang 的決定只給 WARN | WARN |

**取消的項目：**「每天的顏色像素數」和「每日圖的 pin 數下限」。原因見非目標。pin 有沒有畫對，改由第 1 層的圖例比對和第 4 層的 agent 看圖負責。

### 第 1 層：檢視頁（第二階段，每次執行自帶臨時 token）

**token（2026-11-02 改為每次執行臨時發放）：**
- 每次檢查一個方案時，才發一個 `created_by='verify-site'` 的 token，看完檢視頁**立刻刪除**，檢查出錯也一樣刪。不留任何長期 token：方案在 Yang 滿意之前不分享，長期 verify token 只會讓第 1 層永遠被略過。
- 每次執行開始時，先刪掉被中斷的執行留下的 verify token，並印出刪了幾個。因此**不能同時跑兩個 verify-site**：後開始的那個會把前一個正在用的 token 刪掉，造成假的 FAIL。
- worker 每個請求都即時讀 `plan_share_tokens`（`router.rs:186`），剛發的 token 馬上生效。
- 共用 `share-token` 的 mint helper（`created_by` 是參數）。
- 和 `share-token` 一樣是附屬表操作，**不寫**方案的稽核紀錄（`share_token.rs:8-13`）。
- token 全文永遠不印出，只印 fingerprint。
- **風險：** 檢視 token 也能打開憑證 PDF，裡面有訂位代號和姓名（`router.rs:200-225`）。所以 token 只存在檢查的那幾秒；刪除失敗時錯誤訊息指向 `travel share-token clear --plan-id <id>`。
- 原本的 `--provision-token [--rotate]` 已移除。

**頁面判定**（用 HTML parser，只看 `<body>` 裡的元素。每頁都嵌了整份 CSS，`mod.rs:22`，直接搜字串會誤中）：

1. **頁面正確：**
   - HTTP 200。
   - 要有 `section.booking-summary.summary-box`，以及總覽地圖的 figure（有圖或缺圖框都算）。
   - 不能出現 `.auth-page`，也就是登入頁、無權限頁、壞連結頁。
2. **語言：**
   - 預設版本是 `<html lang="zh-TW">`。
   - 加 `&lang=en` 的版本是 `<html lang="en">`。
   - 兩個版本都要有 `<meta name="google" content="notranslate">`。
3. **不能出現擁有者控制項：**
   - 不能出現 `.owner-chrome`、`.grant-manager`、`.grant-form`、`.copy-share-btn`。
   - 不能出現 action 是 `/grants/*` 的 form，也不能出現 `input[type=checkbox][name=token]`。
4. **地圖格子和 R2 一致：**
   - 頁面該顯示哪些圖，照 worker 的實際規則判斷（`router.rs:666-696`、`render/mod.rs` 的 `show_logistics`）：
     - `plan.png`：一定有一格，不是圖就是缺圖框。
     - `day-N.png`：目前存在的每一天都有一格，不是圖就是缺圖框。
     - `plan-logistics.png`：有這張圖，或方案有航班時才有一格；有航班但沒圖時顯示缺圖框。
     - `plan-excursion.png` 和 `day-N-logistics.png`：有圖才顯示，沒有缺圖框。
   - `uploaded` 的圖在頁面上必須是 `<img>`，不能是缺圖框。`skipped` 的圖不能出現 `<img>`。
   - 抓 `<img>` 時，`src` 要當 URL 解析，不能假設它是第一個屬性，也不能假設一定帶 `?v=`。
5. **圖例：**
   - 每個 `figure.map-frame` 底下直接的 `ol.map-legend > li`，要和 `map_legend_stops` 依 seq 逐列比對：編號、名稱、座標連結都要一樣，不只比筆數。
   - 沒有 `<ol>` 視為 0 筆。

**worker 要配合的小改動**（屬前端，送 Codex 審）：
- 每個地圖 figure 加上 `data-map-key="<key>"`，有圖和缺圖框都要加，這樣缺圖框才能精準對應到 key。
- 方案頁的外層加上 `data-plan-id` 和 `data-view="viewer|owner"`，正面確認這是哪個方案、哪種視角。

**延伸檢查**（第二階段後段）：
- **同行方案卡片：** 地圖是 ArcGIS 外部網址（`mod.rs:231`、`:337`），要確認卡片、站點和外部圖片能取得。
- **已訂住宿的相簿圖、位置小圖：** 抓得到圖。
- **憑證連結：**
  - 帶同一個 verify token，抓得到 PDF：權限不足會回 403，R2 沒檔案會回 404。
  - URL 和 token 都不印出。

### 第 4 層：agent 看圖（第三階段）

`--contact-sheet <dir>` 會為每個方案輸出一張縮圖總表 PNG，每格標上 key。agent 一個方案看一張圖就夠。這一層是輔助，不參與 PASS/FAIL 判定。

## 什麼時候執行

- **snapshot-maps 之後：** 跑 `verify-site --plan-id <id>`。snapshot-maps 結尾會印出這個提示，但不自動執行。
- **worker deploy 之後：** 跑 `verify-site`，涵蓋全部方案。
- **不放進** pre-commit，也不放進 `make test`。

## 測試計畫

- **第 3 層：**
  - 用 `render_png` 畫三種已知圖：ArcGIS 平底色底圖、Overpass 道路網、純格線，都要得到預期判定。
  - 再逐一破壞，每種都要被對應的檢查抓到：抹掉標題文字、改尺寸、換成 placeholder bytes、背景退回純格線。
- **第 2 層：**
  - 判定表是純函式，輸入 status 和線上回應（bytes 與 sha），輸出判定結果。三種 status 各自正反兩面都要測。
  - 輸入 hash 用 snapshot-maps 的同一個函式重算，要測同一份輸入 hash 不變、只改路線就會變。
- **第 1 層：** 用 worker 單元測試的 HTML 片段，測 img 抽取、圖例比對、擁有者標記、`.auth-page` 判定。
- **連線上的煙霧測試：** 只有設了 `TRAVEL_VERIFY_SITE_LIVE=1` 才跑。

## 分階段

1. **第一階段（已完成）：** 第 2 層和第 3 層，不需要 token。涵蓋上表的 placeholder、上傳失敗、CDN 舊圖、skipped 但仍在線上、只改路線、CARTO 蓋字（WARN）。**不涵蓋**頁面和圖例的問題，那是第二階段的事。
2. **第二階段（已完成，worker 待部署）：** 第 1 層、worker 的 `data-map-key` 和 `data-plan-id` 改動、token 冪等與輪換，以及延伸檢查。
3. **第三階段（已完成）：** 第 4 層的縮圖總表（`--contact-sheet <dir>`）。

## 決定（Yang，2026-10-30）

1. **verify token：** 每個方案一個專用的檢視 token，規則見第 1 層。（2026-11-02 改為每次執行臨時發放、用完即刪。）
2. **檢查範圍：** 預設檢查所有未軟刪的方案，含已結束的行程。
3. **異常大片文字：** 只給 WARN，不判 FAIL。

## 另外發現的 bug（不在本設計範圍，另行修正）

- 已訂住宿的相簿（`summary.rs:1064`）沒有過濾 `via.placeholder.com` 假圖網址；候選住宿的相簿有過濾（`:1236`、`:1355`）。

## 審查紀錄

- **Codex（前端）：** 16 條具體說法逐條核對，全部屬實。
  - 已採納：img `src` 解析、各類圖的出現條件表、缺圖框無 key、圖例在同一個 figure 內、CSS 內嵌所以要解析 HTML、擁有者與 `.auth-page` 標記、cookie 會覆蓋 token、`lang="en"`、同行卡片和住宿圖片、憑證連結。
- **K3.1 Pro（後端）：** 7 條逐條核對，全部屬實。
  - 已採納：取消像素數 pin、加 nonce 避開快取、檢查 MISSING 和 skipped 但仍在線上、Overpass 背景不能誤判、改用輸入 hash 判斷過期、token 冪等與輪換、placeholder 用 bytes 辨識。
  - 同時更正原設計的一個錯誤：產生 token **不寫**稽核紀錄。

## 實作備註（第二、三階段，2026-11-02）

- **`data-plan-id` / `data-view` 放在 `<body>` 上**，不另外包一層 div：方案頁的最外層就是 `<body>`，這樣除了這兩個屬性，頁面其餘 bytes 完全不變。`data-view` 依 owner chrome 是否存在判定（只有擁有者 session 才有 owner chrome）。
- **輸出行的 layer：** `token`、`page`、`lang`、`owner`、`ident`、`slot`、`legend`、`cached`（`?v=` 網址）、`extra`（延伸檢查）。第 1 層的 key 前面帶語言，例如 `zh:day-1.png`。
- **`plan-airport.png`、`day-N-away.png`** 和 `plan-excursion.png` 一樣，只有有圖時才有格子，沒有缺圖框。
- **沒有 `data-map-key` 的頁面**（worker 尚未部署）：每頁一行 FAIL，指向 deploy，並略過該頁的格子與圖例比對。設計沒有允許降成 WARN，所以維持 FAIL。
- **圖例**依 worker `map_legend_html` 的實際順序比對：沒有 day 的列照 seq；有 day 時先依 day 首次出現的順序分組，再接無 day 的列；day 區塊數也要相符。座標連結把 `q=` 解析成數字比對。
- **外部圖片**（同行方案地圖、已訂住宿相簿與位置小圖）：只有對所有觀看者都壞掉的回應才判 FAIL（404、410、200 但空內容）；401/403/429/5xx 或連線錯誤只給 WARN，因為可能只是拒絕非瀏覽器的抓取。外部圖片允許 https-only 轉址。
- **延伸檢查只看 zh 頁**：語言不影響這些網址。
- **同行方案的站點**：每個選項的 `div.companion-stops` 裡至少要有一個 `span.stop-chip`；站點清單是空的給 WARN（`companion_option_stops` 缺資料），不判 FAIL。
- **修復提示**：頁面上出現 `map_artifacts` 沒有紀錄（或 `skipped`）的 `<img>` 時，提示刪除 R2 物件，而不是重跑 snapshot-maps——snapshot-maps 不會再處理它不認得的 key。
- **token：** 改為每次執行臨時發放、用完即刪（見第 1 層），`--provision-token`／`--rotate` 和「沒有 token 就 WARN 略過」都已移除。
- **所有網址**（不只帶 token 的）都經 `curl -K -` 從 stdin 傳入；錯誤訊息中的 token 一律換成 fingerprint。每次抓取前先刪掉暫存檔，避免沒有 body 的回應讀到上一次的內容。
- **縮圖總表**：每格是線上地圖縮小一半，下方標 key；非正常狀態另標 `SKIPPED`、`PLACEHOLDER`、`MISSING`、`SKIPPED BUT SERVED` 等。使用 snapshot-maps 的 `Raster`、點陣字、`TITLE_INK` 和 PNG 編碼器。
