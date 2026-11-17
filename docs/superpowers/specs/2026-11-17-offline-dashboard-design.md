# 行程網站離線支援 — 設計

> 2026-11-17 · 需求：Yang「我們這網站能不能支持一下離線」「旅行移動時會到沒網路的地方呀,包括在移動中,或信號死角」。
> 狀態：Yang 已核可（2026-11-17，「照建議」：§11 五題皆依建議）。已併入 Codex、K3.1 Pro 兩份審查與 Codex v2 核對（§13）。下一步寫實作計畫。

## 1. 可行性（先看這節）

**情境**：Yang（owner，GitHub OAuth）與同行家人（viewer，`?token=` 分享連結），用 iPhone Safari / Android Chrome，在電車、地鐵、山區、國外漫遊斷線時打開行程頁。

| 斷線時必須能用 | 可行？ | 說明 |
|---|---|---|
| 今天／接下來的行程（day cards） | ✅ | 整趟行程本來就是**同一頁** SSR HTML（`render/mod.rs:92` `render_plan_at`），存一頁＝存整趟，不需要「今天+N 天」切片。 |
| 現在・接下來 卡片 | ⚠️ 部分 | 卡片只在「伺服器渲染當天」產生（`render/now.rs:103` `trip_today`）。inline script 每分鐘重標，但日期不符時把所有時段當成已過（`render/mod.rs:376` `m=1e9`）→ 昨天存的副本會顯示昨天的卡片。需要小改（§6）。 |
| 路線地圖 PNG | ✅ | `/map/*` 公開、`?v=<etag>` 版本化（`render/map.rs:182`、`router.rs:58-73`），適合 cache-first。 |
| 票券 PDF（換票用） | ✅ 需補版本鍵 | `/voucher/*` 用同一 token 授權（`router.rs:212-229`）。本機票券連結有**兩個來源、都沒有版本**：`plan_documents`（`render/summary.rs:634-651`）與飯店 `hotels.voucher_url`（`render/summary.rs:1081-1090`）。R2 key 固定（`plan_documents.rs:79`），重傳同 key 會讓舊快取一直留著 → 一律加 `?v=`（§6）。外部（非 `/voucher/`）票券連結本站**不保證離線**。 |
| 天氣 | ⚠️ 只能看存下時的版本 | 每日天氣已附「更新 11/15 08:39」（`render/day.rs:260`），離線讀者看得出新舊；**無法離線更新**。 |
| Google 地圖導航連結 | ❌ | 外部服務。替代：出發前在 Google Maps app 下載離線地區（§12）。 |
| 任何寫入（`/grants/*`、未來的旅行筆記上傳） | ❌ | 非目標。SW 一律不碰非 GET 請求，不做離線排隊。 |
| owner 首頁 `/`（計畫清單） | ❌（刻意） | 內含分享 URL 與 CSRF 表單（`router.rs:231-279`），不快取。 |

**結論：可行，但「打開一次就存好」不會自己成立。** 第一次造訪時 SW 才剛註冊，**不控制這次導航**，這一頁的 HTML 不會經過 SW（兩份審查共同 blocker）。所以設計核心是一個**明確的「存到本機」流程（§2、§6）**：存完、驗證完才告訴使用者「已存好」；沒看到這行字＝沒存好。總量估數 MB～數十 MB（地圖 640×440，`snapshot_maps.rs:27-28`；PDF 上限 10 MiB／份，`plan_documents.rs:18`），容量不是問題；限制是 iOS 儲存規則與入口差異（§4），每個入口要實機通過才算支援（§8）。

## 2. 方案比較

### (a) Service Worker ＋ 明確「存到本機」流程（**推薦**）
- **存（SAVE）**：每次線上開啟計畫頁，inline script 在 `navigator.serviceWorker.ready` 後送 `{type:"SAVE", url}` 給 SW（`MessageChannel` 回報）。SW 重新抓這個網址、驗證、存 HTML＋全部地圖＋全部票券，完成後回報數量與失敗清單（§6）。**這是必要流程，不靠 fetch handler 被動快取**——同一條路徑處理第一次與之後每一次，只有一套邏輯。代價：每次線上開啟多一次 SSR 渲染（家族規模，可忽略）。
- **讀**：HTML network-first，**4 秒逾時**改用快取（比 stale-while-revalidate 好——SWR 第一眼永遠是舊版，Yang 剛改完行程會困惑）；地圖、票券（版本化 URL）cache-first。
- 從快取送出時顯示橫幅「離線副本 · 11/15 21:30 儲存，可能不是最新」。
- 優點：存成功與否看得見、owner/viewer 都適用、不改 SSR（關 JS 照樣能看，只是沒有離線）。
- 代價：新增同源 `/sw.js`（worker 以字串常數供應，非外部腳本）——**牴觸「ONE inline script，no other JS」規則，需 Yang 決定（Q1）**。註冊與 SAVE 呼叫放進現有 `BLOCK_MEMORY_SCRIPT`（`render/mod.rs:330`），頁面上仍只有一段 inline script。

### (b) 不用 SW 的做法
| 做法 | 評估 |
|---|---|
| 瀏覽器「列印 → 存成 PDF」 | **零程式碼，今天就能用**。`beforeprint` 已會展開所有區塊（`render/mod.rs:339`）。缺點：手動、不會更新、沒有現在卡片、折疊內地圖可能來不及載入、PDF 裡的票券連結仍需網路。 |
| 票券 PDF 先存到「檔案」app／Google Drive | 零程式碼，換票最可靠的備援。 |
| 下載單一 HTML 檔 | iOS 開本機 HTML 體驗差，不推薦。 |
| iOS「加入主畫面」本身 | 不提供離線；開成 web app 反而有獨立儲存（§4）。 |
| Safari 閱讀列表「離線儲存」 | 未驗證、對 `no-store` 頁行為不確定，不列為方案。 |

**建議**：做 (a)；(b) 的三項零程式碼做法**無論 (a) 是否上線都保留**，寫成出發前清單（§12）。

## 3. 目標／非目標／反目標

- **目標**：斷線時 viewer 與 owner 能開啟最後一次「已存好」的行程頁、所有地圖、所有本機票券 PDF；清楚標示「幾點存的副本」；存失敗的項目看得到並可重試；有網路時永遠先拿最新版。
- **非目標**：離線寫入／排隊上傳；離線更新天氣；離線 Google 地圖；owner 首頁；PWA 安裝體驗（manifest、圖示、推播）；讓 LINE 內建瀏覽器離線。
- **反目標**：快取到錯的人（owner 頁給 viewer 或反之）；沒存好卻顯示「已存好」；用 feature flag 開關（使用者規則：no feature flag）；把行程內容寫進 JS／SW（內容在 Turso、文案在 `i18n.rs`）；為已撤銷連結做遠端抹除之類的過度設計。

## 4. 平台細節與限制

- **iOS 7 天清除**：Safari（ITP）在使用者 7 天內沒有互動該網站時，會刪除該網站所有腳本可寫儲存，包含 SW 與 Cache。→ **旅途中每天線上開一次**（每次都會重新 SAVE）就不受影響。主畫面 web app 不適用這條 7 天上限。
- **主畫面 web app 儲存與 Safari 分開**：在 Safari 存的副本，從主畫面圖示開啟時看不到（反之亦然）。→ 從**實際要用的入口**有網路時開一次、看到「已存好」。不提供 manifest（避免固定 `start_url` 弄丟 `?token=`）。
- **容量**：iOS 17+／Android Chrome 每 origin 配額遠大於本案。SAVE 時呼叫 `navigator.storage.persist()`（Android 有效；iOS 無保證）。
- **LINE 內建瀏覽器**：iOS LINE 用 WKWebView，沒有 SW → **不支援離線，也不假裝支援**。inline script 偵測 `!('serviceWorker' in navigator)` 或註冊失敗時，顯示「這個瀏覽器不能離線保存，請用 Safari／Chrome 開啟」（功能偵測，不做 UA 判斷）。
- **兩個網域是兩個 origin**：`trip-dashboard-rs.yanggf.workers.dev` 與 `travel.ahexagram.com`（`wrangler.toml` routes）快取互不相通；以分享連結所用網域為準。
- **`private, no-store`**：所有 HTML 與票券都送 `private, no-store`（`router.rs:816-820`、`router.rs:223`）。這是 HTTP 快取指令，**Cache API 不受它約束**；保留標頭，由 SW 依明確規則自己存——刻意例外，文件與測試寫明。`router.rs:325-330` 註解「`no-store` 所以釘時頁不會被快取」在 SW 上線後不再成立，實作時一併改註解；`?at=` 由 SW 明確略過（§6）。
- **`target="_blank"` 開 PDF**：票券連結開新分頁（`render/summary.rs:646`）。iOS web app 模式下新分頁可能不經 SW → 離線打不開。實機驗證（§8）；失敗則該入口不宣稱支援票券離線，改走 §12 票券存檔案。

## 5. 快取鍵與 owner/viewer 隔離

伺服器身分判定：有 owner session 就一律是 Owner，即使網址帶 `?token=`（`router.rs:201-202`）；session 過期且帶有效 token → 以 viewer 身分回 200；session 過期且不帶 token → 403 `bad_share_page`（`router.rs:283-288`）。owner 頁有 owner chrome（`router.rs:307-323`），body 有 `data-view="owner|viewer"`（`render/mod.rs:61-66`）。SW 讀不到 HttpOnly cookie，所以：

1. Worker 在計畫頁回應加 `X-TD-View: owner|viewer`（與 `data-view` 同源判定，單元測試鎖一致）。**只有帶此標頭、且 body `data-view` 相符的 200 回應才存**；登入頁、403 頁、首頁、`/auth/*`、`/grants/*`、`/diag/*` 一律不存。
2. 兩個 cache：`td-viewer`、`td-owner`（名稱**不帶 SW 版本**，見 §6 生命週期）。
   - viewer 鍵：`/?plan=<slug>&lang=<zh|en>&token=<t>`（token 是授權的一部分）。
   - owner 鍵：`/?plan=<slug>&lang=<zh|en>`。
   - 其他參數（`nav`、`tab`、`verify`、fragment）正規化掉。
3. **URL→鍵對照表**：SAVE 成功時寫入 `td-index`（Cache API 的一筆 entry）：`正規化請求 URL → {role, key, savedAt}`，以實際回應的 `X-TD-View` 決定 role。離線查詢**只查對照表**，不從「網址有沒有 token」推斷 → owner 帶 token 開的頁離線也找得到自己的 owner 副本；同一 URL 後存者覆蓋先存者。
4. `/auth/logout` 導航時 SW 清空 `td-owner` 及對照表中的 owner 項。
5. 票券以完整 URL（含 `token` 與 `v`）為鍵，存在所屬 role 的 cache。

**403 規則（只針對 viewer token）**：
- 網址帶 token 的線上請求回 403 → token 已撤銷 → 刪除該 token 的所有 viewer HTML、對照項，再 sweep 其資產（§6）。**即使 403 在 4 秒逾時之後才到**（此時頁面已用快取顯示），SW 仍等原請求完成並執行清除（`event.waitUntil`）。
- 不帶 token 的 403（owner session 過期）→ **保留** owner 副本，從快取送出，橫幅改顯示「需要重新登入才能更新」；旅途中訊號差、OAuth 失敗時不能沒有副本。owner session 只有 8 小時（`gwebcdb/crates/worker-github-oauth/src/session.rs:21`），這是常態不是例外。

**撤銷的取捨（明講）**：`share-token clear` 之後，已存在同行者手機上的副本（含票券）**在對方下次線上開啟之前都還看得到**；下次線上拿到 403 即刪。Yang 已接受以分享連結管理為控制手段；不做遠端抹除。

## 6. 存什麼、何時存、怎麼失效

### SAVE 流程（必要，每次線上開啟都跑）
1. 頁面：`register('/sw.js')` → `serviceWorker.ready` → 送 `SAVE`（有 `?at=` 不送）。顯示「正在存到本機…」。
2. SW：以原網址重新 `fetch`（同源帶 cookie）→ 必須 200 ＋ `X-TD-View` ＋ `data-view` 相符，否則回報失敗、**不動既有副本**（403 依 §5 處理）。
3. 從 HTML 擷取所有 `/map/…png?v=` 與 `/voucher/…` URL，寫成這份 HTML 的**資產清單（manifest）**。
4. 逐一抓資產：已在 cache 的版本化 URL 跳過；未版本化的 `/voucher/`（見下）每次重抓覆蓋。
5. **全部資產都成功，才替換既有副本**：寫入新 HTML、manifest、對照表（先資產後 HTML）。只要有任一資產失敗：
   - 這個網址**已有完整副本** → 保留舊 HTML 與舊 manifest 不動（新抓到的資產照樣留在 cache，下次 SAVE 可略過），不跑 sweep；回報「這次更新沒存完，離線時會看到 {舊副本時間} 的版本」＋失敗項＋「重試」。
   - 這個網址**還沒有副本** → 寫入新 HTML 與 manifest（部分可用總比沒有好），並列出失敗項。
6. Sweep（只在步驟 5 完整替換後才跑）：掃同 role 所有 manifest，刪除**沒有任何 manifest 引用**的資產（標記清除；ZH/EN 兩頁、兩個 token 共用的地圖不會被誤刪）。
7. 回報頁面：成功 →「已存好，可離線查看（地圖 n、票券 n）」；有失敗 →「已存，但 k 項未存」＋失敗項名稱（票券用 `data-doc` 標籤）＋「重試」按鈕（重送 `SAVE`）。HTML 本身失敗 → 「沒存好，離線時看不到」。

### 資源策略
| 資源 | 讀取策略 | 失效 |
|---|---|---|
| 計畫頁 HTML（整趟） | network-first，4 秒逾時退快取（查 §5 對照表） | 每次 SAVE 覆蓋；viewer 403 → 刪 |
| `/map/<plan>/*.png?v=<etag>` | cache-first | sweep：無 manifest 引用即刪 |
| `/voucher/<key>?v=<sha256 前 12 碼>&token=…` | cache-first | 同上；viewer 403 → 隨該 token 刪 |
| `/sw.js` | 瀏覽器自行檢查；送 `Cache-Control: no-cache` | 內容變更即更新 |
| 任何帶 `?at=` 的請求 | **SW 完全略過**：不讀快取、不寫快取 | — |

### 票券版本鍵（worker 端）
- `PlanDocument` 加 `sha256`（`model.rs:331-352`，含解碼），`router.rs:1025` 的 SELECT 補選 `sha256`（`plan_documents.sha256 TEXT NOT NULL` 已存在，`scripts/schema.sql:925`）。
- 所有本機 `/voucher/` 連結改由**同一個 helper** 產生（`document_link` 與飯店 `voucher_url` 共用）：飯店 `voucher_url` 的路徑若等於某筆 `plan_documents.r2_key` → 用其 `sha256`；對不到 → 不加 `v`，SW 每次 SAVE 重抓覆蓋（不會留舊券，只是多下載）。不新增欄位、不加 R2 `head()` 呼叫。
- 外部 URL 的票券不預抓、不保證離線。

### SW 生命週期與回退
- `install` 時 `skipWaiting()`、`activate` 時 `clients.claim()`：新版不會卡在 waiting。
- **內容 cache 名稱不隨 SW 版本變**，SW 更新從不刪除 `td-viewer`／`td-owner`／`td-index` → 旅途中更新 SW 不會讓離線能力倒退；舊副本一直在，直到下次 SAVE 成功覆蓋。只有儲存格式改變時才換名稱，且新名稱的副本存好後才刪舊名稱。
- **回退**：worker 內預先寫好的 kill-switch `/sw.js`（`unregister()` ＋ 刪所有 `td-*` cache），改送它再部署一次。不用 feature flag。

### 現在卡片小改
day card 加 `data-date`（目前只有 `data-day`，`render/day.rs:741`）；inline script 發現 `#sec-now` 的 `data-now-date` 早於今天時，隱藏該卡並展開今天的 day card。

## 7. 介面（文案全在 `i18n.rs`）

- 伺服器在 body 放 `data-rendered-at`（UTC epoch ms）與 `hidden` 的狀態列、橫幅元素（文案依頁面語言填好）；SW 從快取送出時把 `data-served="net"` 改寫成 `"cache"`（owner 403 時為 `"cache-relogin"`），inline script 據此顯示橫幅並用 `data-now-tz`（`render/now.rs:337`）的 `Intl` 填時間（沒有時區就不顯示時間，不猜）。
- i18n 鍵（ZH/EN 成對；`{…}` 由 inline script 填數字）：
  - `offlineSaving`：「正在存到本機…」
  - `offlineSaved`：「已存好，可離線查看（地圖 {maps}、票券 {pdfs}）」——**必要**，取代舊版選用的 `offlineReady`
  - `offlinePartial`：「已存，但 {n} 項未存：」＋清單；`offlineRetry`：「重試」
  - `offlineFailed`：「沒存好，離線時看不到這一頁」
  - `offlineCopy`：「離線副本 · {time} 儲存，可能不是最新」
  - `offlineRelogin`：「離線副本 · 需要重新登入才能更新」
  - `offlineUnsupported`：「這個瀏覽器不能離線保存，請用 Safari／Chrome 開啟」
  - `offlineMiss`（SW 離線備援頁）：「這一頁還沒存到這支手機。連上網路後打開，看到『已存好』之後就能離線看。」
- `/sw.js` 的備援頁文案在 worker 送出 `/sw.js` 時以 `i18n::fill` 帶入（同 `render/share.rs:12`），不在 JS 寫死。

## 8. 驗證與測試

- **Worker 單元測試**：`X-TD-View` 與 `data-view` 一致；非計畫頁沒有該標頭；所有本機 `/voucher/` 連結（plan_documents 與飯店）都經同一 helper、有 sha 時帶 `v`；鍵正規化規則（Rust 端與 `/sw.js` 常數同源）；`render/mod.rs:1205-1212`「一段 inline script、無 `<script src`」測試維持並明文 `/sw.js` 例外；kill-switch `/sw.js` 含 `unregister` 與 cache 刪除。
- **`verify-site` layer 5**（`--render`，真 CDP Chrome，`verify_site.rs:37-40`；離線案例用**不帶 `?at=`** 的網址，因 layer 5 現用 `&at=`，`verify_site.rs:558`）：
  1. **首次造訪→離線**：全新瀏覽器儲存區開 viewer 頁 → 等「已存好」→ CDP 離線 → 重新整理 → `data-view="viewer"`、橫幅可見、地圖無破圖、**逐一**開每個票券回 PDF。
  1b. **已有完整副本、下一次 SAVE 部分失敗**：存好後，用 CDP 攔截讓其中一張票券回 500 → 重新 SAVE → 應回報失敗項且顯示舊副本時間 → 離線重新整理 → 今天行程與**每一張原本已存好的票券**仍能開。
  2. **多語／多 token**：同次再 mint 第二個 token，各開 ZH、EN 存好 → 撤銷其一（下一步）後另一 token 的 ZH/EN 與共用地圖仍可離線開。
  3. **撤銷**：在 layer 5 流程中**提前明確刪除**該 run 的 token（現行是檢查完才刪，`verify_site.rs:480-495`）→ 線上重開得 403 → 離線 → `offlineMiss`、該 token 票券已刪。
  4. **慢網路晚到 403**：CDP 節流讓回應超過 4 秒 → 先見快取頁 → 403 到達後 cache 已清。
  5. 結束仍照現行規則刪除所有 per-run token。
- **無法自動化（verify-site 沒有 OAuth session）→ Yang 實機手測**：owner 開頁存好→離線；owner 帶 token 網址離線；owner session 過期（8 小時後）線上開→仍見副本與「需要重新登入」；登出→owner 副本清除。
- **實機入口驗收**（每個入口：線上開 → 看到「已存好」→ 飛航模式 → 重開頁、開每張票券）：iPhone Safari、iPhone 主畫面 app、Android Chrome、LINE→「用 Safari 開啟」。iOS LINE 內建瀏覽器只驗證 `offlineUnsupported` 提示出現。**入口未在實機通過前，不對同行者宣稱支援。**
- **回退演練**：首次上線後在實機做一次：部署 kill-switch → 線上開 → 確認 SW 已註銷、`td-*` 已清 → 重新部署正式版。

## 9. 部署順序

1. 單一 worker 部署：`/sw.js` 路由、`X-TD-View`、`data-*` 標記、狀態列／橫幅、票券 `?v=`（`PlanDocument.sha256`）、註冊＋SAVE 程式碼一起上（無資料表變更，無需先 `db migrate`）。
2. `verify-site --render` 全綠 → 回退演練 → Yang 與一位同行者實機入口驗收（§8）。
3. 回退：kill-switch `/sw.js` 再部署（§6）。

## 10. 風險

- 以為存了其實沒有 → SAVE 必須明示「已存好」與失敗清單；沒看到＝沒存好。
- iOS 7 天清除／主畫面獨立儲存 → 出發前清單「從要用的入口開到『已存好』；旅途中每天線上開一次」。
- LINE 內建瀏覽器無 SW → `offlineUnsupported` 提示＋分享時附「用 Safari／Chrome 開」。
- 舊副本被當最新 → 橫幅＋每日天氣「更新」時間。
- 已撤銷連結在離線手機上可讀到下次上線 → 已接受的取捨（§5）。
- SW 錯誤讓網站卡舊版 → network-first、`/sw.js` 不快取、skipWaiting、kill-switch 已演練。
- 每次線上開啟多一次渲染 → 家族規模可忽略；若日後成本有感，再改成「已受控的導航直接存」。
- 共用裝置：owner 副本在登出前留在裝置上 → 只在 Yang 自己手機登入。

## 11. Yang 的回答（2026-11-17：「照建議」，五題皆依建議）

1. **放寬「ONE inline script，no other JS」**，允許同源、由 worker 供應的 `/sw.js`？——建議：**同意**（頁面仍只有一段 inline script，SW 是漸進增強，關 JS 照常可看）。
2. **票券 PDF 每次存頁都全部預抓**（每份上限 10 MiB，已存過的版本不重抓）？——建議：**全部預抓**；換票是最需要離線的場景，「已存好」也只有在券都存了才有意義。
3. **owner 頁也要離線嗎？**（登出才清；session 過期保留並提示重新登入）——建議：**要**。
4. **撤銷取捨**：已存到同行者手機的副本在對方下次上線前仍可讀，接受嗎？——建議：**接受**，下次上線 403 自動刪除。
5. **下一趟出發日？** 若兩週內出發，這趟先只用 §12 零程式碼清單，SW 之後再上？——建議：**是**；§12 清單之後也一直保留為備援。

## 12. 出發前清單（零程式碼，SW 上線前後都做）

- [ ] 行程頁「列印 → 存成 PDF」（`beforeprint` 會展開所有區塊），存到「檔案」app。
- [ ] 每張票券 PDF 各自存到「檔案」app（或 Google Drive 離線）——換票最可靠的備援。
- [ ] Google Maps 下載目的地離線地區。
- [ ] （SW 上線後）用**實際要用的入口**（Safari 或主畫面圖示，非 LINE 內建）線上開一次，看到「已存好，可離線查看」且無失敗項；開飛航模式抽開一張票券確認。
- [ ] 旅途中每天線上開一次（避開 iOS 7 天清除、拿到最新版）。

## 13. 審查核對紀錄

| # | 發現 | 審查者 | 處置 | 理由（已對照原始碼） |
|---|---|---|---|---|
| 1 | 首次造訪 SW 不控制導航，HTML／票券沒存到 | Codex、K3 | 採用 | 瀏覽器標準行為；改為必要 SAVE 流程＋「已存好」明示（§6、§7）。 |
| 2 | 飯店 `voucher_url` 也產生 `/voucher/` 連結，沒有版本 | Codex | 採用（簡化） | 確認 `summary.rs:1081-1090`；同一 helper，對到 `plan_documents` 用其 sha，對不到則每次 SAVE 重抓，不加欄位／R2 head。 |
| 3 | 逾時後晚到的 403 也要清除 | Codex、K3 | 採用 | `waitUntil` 等原請求完成再清（§5）。 |
| 4 | owner session 過期 403 會誤刪唯一副本 | K3 | 採用 | 確認無 token 時 `router.rs:283-288` 回 403；403 清除只限 viewer token，owner 保留＋提示重新登入。 |
| 5 | owner 帶 token 網址離線查錯 cache | Codex、K3 | 採用 | 舊規則確會 miss；改用 URL→鍵對照表（§5）。 |
| 6 | 地圖／PDF 刪除需每頁資產清單＋引用計數 | K3 | 採用（簡化） | 用每頁 manifest＋標記清除取代計數器，效果相同、狀態更少。 |
| 7 | SW 更新可能先刪副本、新版卡 waiting、回退未驗 | K3 | 採用（簡化） | cache 名稱不帶 SW 版本，更新從不刪內容；skipWaiting；kill-switch 實機演練一次。 |
| 8 | `?at=` 須同時略過讀與寫 | Codex、K3 | 採用 | 另 `router.rs:325-330` 註解以 `no-store` 推論不會被存，SW 上線後失效，一併改。 |
| 9 | `PlanDocument` 沒有 `sha256` | K3 | 採用 | 確認 `model.rs:331-352` 與 `router.rs:1025` SELECT 皆無；表已有 `sha256 NOT NULL`。 |
| 10 | verify token 在檢查結束才刪，無法在 layer 5 內測 403 | Codex、K3 | 採用 | 確認 `verify_site.rs:480-495`；layer 5 內提前明確刪除。 |
| 11 | 補首次造訪、多語、多 token、慢網路案例 | Codex、K3 | 採用 | 都可在 CDP layer 5 自動化（§8）。 |
| 12 | 自動測試 owner／owner+token／session 過期 | K3 | 駁回（改手測） | `verify-site` 沒有 OAuth session，無法自動產生 owner 頁；列為 Yang 實機手測。 |
| 13 | 每個入口實機通過前不宣稱支援；LINE 顯示改用 Safari/Chrome 提示 | Codex、K3 | 採用 | 以功能偵測顯示 `offlineUnsupported`，不做 UA 判斷。 |
| 14 | 背景預抓可能被手機中止 | K3 | 採用 | SAVE 回報每項結果＋重試，取代無回報的背景預抓。 |
| 15 | 零程式碼備援（列印 PDF、票券存檔案）應保留 | Codex、K3 | 採用 | 寫成 §12 出發前清單，SW 上線後仍保留。 |
| 16 | v2 核對：資產部分失敗仍覆寫完整副本 | Codex v2#1 | 採用 | §SAVE 步驟 5：全部成功才替換；已有完整副本時保留舊版 |
| 17 | v2 核對：缺「完整副本＋下次 SAVE 部分失敗」驗收 | Codex v2#2 | 採用 | §8 layer 5 案例 1b |
